//! Сэмплер `pg_stat_statements` (спец. P3 §5): раз в минуту `pg_stat_statements(showtext := false)` на соединении
//! пула `Sampler`, буфер дельт на 60 мин; живёт, пока подписаны инструмент «Запросы» или карточки запроса узла.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use pg_advisor::queries::{Counters, Cumulative, QueryKey, Totals};
use serde::Serialize;
use tokio_postgres::Client;

use crate::commands::catalog::{catalog_limit, on_catalog_node_split};
use crate::conn::pool::PoolPurpose;
use crate::diag::kcache::KcacheNode;
use crate::diag::queries::{Detect, PgssInfo, PgssInstalled, Snapshot, detect, read_cumulative_on};
use crate::diag::{Head, head};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::registry::{
    SAMPLE_TIMEOUT, SampleSource, SamplerRegistry, SamplerSink, Sampling,
};
use crate::session::SessionKind;
use crate::state::AppState;
use crate::sync::MutexExt;

pub const INTERVAL: Duration = Duration::from_secs(60);
/// Записей `(ключ, дельта)` на узел — порядка 50 МБ.
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_AGE_MS: i64 = 3_600_000;
/// Пауза между тиками дольше — промежуток: сэмплер стоял (вкладка была закрыта), и следующий тик — новая база.
pub const GAP_AFTER_MS: i64 = 150_000;

/// Сэмпл: шапка узла, установленное расширение и строки pgss.
pub struct StatementsSample {
    pub head: Head,
    pub pgss: PgssInstalled,
    pub snapshot: Snapshot,
}

/// Событие канала вкладки: `tick` — раз в минуту (`ticks` — тиков с дельтами в часовом окне, `changed` — ключей с
/// дельтами в этом тике), `gap`, `error`, `stopped` — как у сэмплера активности.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StatementsEvent {
    #[serde(rename_all = "camelCase")]
    Tick {
        at_ms: i64,
        coverage_s: u32,
        ticks: u32,
        changed: u32,
    },
    #[serde(rename_all = "camelCase")]
    Gap {
        from_ms: i64,
        to_ms: i64,
    },
    Error {
        error: AppError,
    },
    Stopped,
}

/// Точка истории карточки: дельты тика по ключам запроса.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub at_ms: i64,
    pub calls: i64,
    pub total_exec_ms: f64,
    pub mean_exec_ms: f64,
}

struct Tick {
    at_ms: i64,
    dealloc_delta: i64,
    deltas: Vec<(QueryKey, Counters)>,
}

/// Сумма дельт за окно.
pub struct WindowSum {
    pub since_ms: i64,
    pub until_ms: i64,
    pub ticks: u32,
    pub coverage_s: u32,
    pub dealloc_grew: bool,
    pub sums: HashMap<QueryKey, Counters>,
}

/// Окно для карточки запроса: сумма дельт только его ключей и итог окна по всем ключам — знаменатель доли.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeysWindow {
    pub sum: Counters,
    pub totals: Totals,
}

struct TickNames {
    datname: Option<String>,
    rolname: Option<String>,
    ticks: usize,
}

/// Скан текстов поиском: строка поиска, `stats_reset` и `dealloc` на момент скана, ключи pgss, прочитанные до скана
/// (их тексты проверены), и найденные ключи.
struct SearchHit {
    needle: String,
    reset: (Option<i64>, Option<i64>),
    checked: HashSet<QueryKey>,
    matched: HashSet<QueryKey>,
}

pub struct StatementsBuffer {
    latest: HashMap<QueryKey, Cumulative>,
    ticks: VecDeque<Tick>,
    entries: usize,
    gaps: Vec<(i64, i64)>,
    last_at_ms: Option<i64>,
    last_head: Option<Head>,
    info: Option<PgssInfo>,
    pgss: Option<PgssInstalled>,
    hidden: bool,
    /// Имена базы и роли ключей, на которые ссылаются тики, и число таких тиков: ключ может уйти из pgss (сброс,
    /// вытеснение), а его дельты — остаться в окне.
    tick_names: HashMap<QueryKey, TickNames>,
    /// Последний скан текстов поиском: повторный опрос с той же строкой его не повторяет, пока ключи и сброс те же.
    search: Option<SearchHit>,
    max_entries: usize,
    max_age_ms: i64,
}

impl Default for StatementsBuffer {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_AGE_MS)
    }
}

impl StatementsBuffer {
    pub fn with_limits(max_entries: usize, max_age_ms: i64) -> Self {
        Self {
            latest: HashMap::new(),
            ticks: VecDeque::new(),
            entries: 0,
            gaps: Vec::new(),
            last_at_ms: None,
            last_head: None,
            info: None,
            pgss: None,
            hidden: false,
            tick_names: HashMap::new(),
            search: None,
            max_entries,
            max_age_ms,
        }
    }

    /// Тик: дельты изменившихся ключей против прошлых накопительных значений. Первый тик и тик после промежутка —
    /// только база (дельт нет); смена `pg_stat_kcache` — новая база для его колонок. Ключ с уменьшившимися счётчиками
    /// (вытеснен и вернулся) и все ключи после смены `stats_reset` получают новую базу pgss: дельта счётчиков pgss равна
    /// накопительному значению, а колонки kcache — дельта, пока они не уменьшились (свой сброс у kcache).
    pub fn push(&mut self, at_ms: i64, s: StatementsSample) -> Vec<StatementsEvent> {
        let mut out = Vec::with_capacity(2);
        let gap = self
            .last_at_ms
            .filter(|last| at_ms - last > GAP_AFTER_MS)
            .map(|last| (last + INTERVAL.as_millis() as i64, at_ms));
        if let Some((from_ms, to_ms)) = gap {
            self.gaps.push((from_ms, to_ms));
            out.push(StatementsEvent::Gap { from_ms, to_ms });
        }
        let baseline = self.last_at_ms.is_none() || gap.is_some();
        let reset = self
            .info
            .is_some_and(|i| i.stats_reset_ms != s.snapshot.info.stats_reset_ms);
        // `pg_stat_kcache` появился, исчез или сменил схему: для его колонок этот тик — новая база (прошлые значения
        // относятся к другому чтению, и накопительное не должно выдать себя за дельту).
        let kcache_changed = self
            .pgss
            .as_ref()
            .is_some_and(|p| p.kcache != s.pgss.kcache);
        let mut deltas = Vec::new();
        if !baseline {
            for row in &s.snapshot.rows {
                let delta = match self.latest.get(&row.key) {
                    Some(prev) if !reset && !row.counters.decreased_from(&prev.counters) => {
                        if !row.counters.changed_since(&prev.counters) {
                            continue;
                        }
                        let d = row.counters.delta(&prev.counters);
                        if kcache_changed {
                            d.without_kcache()
                        } else {
                            d
                        }
                    }
                    // Новая база pgss (сброс, вытесненная и вернувшаяся строка): у kcache свой сброс — его колонки
                    // дельтой против прошлого значения, если расширение то же.
                    Some(_) if kcache_changed => row.counters.without_kcache(),
                    Some(prev) => row.counters.rebased_with_kcache_delta(&prev.counters),
                    None => row.counters,
                };
                if delta.calls == 0 && delta.total_exec_ms == 0.0 {
                    continue;
                }
                deltas.push((row.key, delta));
                let names = self.tick_names.entry(row.key).or_insert(TickNames {
                    datname: None,
                    rolname: None,
                    ticks: 0,
                });
                names.datname.clone_from(&row.datname);
                names.rolname.clone_from(&row.rolname);
                names.ticks += 1;
            }
        }
        // Вытеснений за тик: рост `dealloc`; на тике сброса `dealloc` обнулён вместе со статистикой, и новая база
        // даёт дельту, равную накопительному значению, — то же правило, что у счётчиков.
        let dealloc_delta = match (self.info.and_then(|i| i.dealloc), s.snapshot.info.dealloc) {
            _ if baseline => 0,
            (_, Some(now)) if reset => now,
            (Some(prev), Some(now)) if now > prev => now - prev,
            _ => 0,
        };
        self.latest = s.snapshot.rows.into_iter().map(|r| (r.key, r)).collect();
        self.hidden = s.snapshot.hidden;
        self.info = Some(s.snapshot.info);
        self.last_head = Some(s.head);
        self.pgss = Some(s.pgss);
        self.last_at_ms = Some(at_ms);
        let changed = deltas.len() as u32;
        if !baseline {
            self.entries += deltas.len();
            self.ticks.push_back(Tick {
                at_ms,
                dealloc_delta,
                deltas,
            });
        }
        self.evict(at_ms);
        // Покрытие часового окна — без карты сумм: она нужна отчётам, а не событию тика.
        let (ticks, coverage_s) = self
            .coverage((self.max_age_ms / 1000) as u32)
            .unwrap_or((0, 0));
        out.push(StatementsEvent::Tick {
            at_ms,
            coverage_s,
            ticks,
            changed,
        });
        out
    }

    /// Старое вытесняется целыми тиками: по возрасту и по пределу записей.
    fn evict(&mut self, now_ms: i64) {
        while let Some(t) = self.ticks.front()
            && (self.entries > self.max_entries || now_ms - t.at_ms > self.max_age_ms)
        {
            let t = self.ticks.pop_front().expect("front checked");
            self.entries -= t.deltas.len();
            for (k, _) in &t.deltas {
                if let Some(n) = self.tick_names.get_mut(k) {
                    n.ticks -= 1;
                    if n.ticks == 0 {
                        self.tick_names.remove(k);
                    }
                }
            }
        }
        self.gaps.retain(|(_, to)| now_ms - to <= self.max_age_ms);
    }

    /// Тики с дельтами в окне `(последний − window_s, последний]` (по времени) и границы окна; `None` — тиков ещё не было.
    fn in_window(&self, window_s: u32) -> Option<(i64, i64, impl Iterator<Item = &Tick>)> {
        let until_ms = self.last_at_ms?;
        let since_ms = until_ms - i64::from(window_s) * 1000;
        Some((
            since_ms,
            until_ms,
            self.ticks.iter().filter(move |t| t.at_ms > since_ms),
        ))
    }

    /// Покрытие — собранное: каждый тик с дельтами покрывает свой интервал, не больше окна. Пролёт от первого тика до
    /// последнего засчитал бы и паузу сэмплера внутри окна (вкладка была закрыта).
    fn covered_s(ticks: u32, window_s: u32) -> u32 {
        ticks
            .saturating_mul(INTERVAL.as_secs() as u32)
            .min(window_s)
    }

    /// Тиков с дельтами в окне и покрытие окна; `None` — тиков с дельтами в окне нет.
    fn coverage(&self, window_s: u32) -> Option<(u32, u32)> {
        let (_, _, ticks) = self.in_window(window_s)?;
        let n = ticks.count() as u32;
        (n > 0).then(|| (n, Self::covered_s(n, window_s)))
    }

    /// Сумма дельт по тикам в `(последний − window_s, последний]`; `None` — тиков с дельтами в окне нет.
    pub fn window(&self, window_s: u32) -> Option<WindowSum> {
        let (since_ms, until_ms, in_window) = self.in_window(window_s)?;
        let mut sums: HashMap<QueryKey, Counters> = HashMap::new();
        let (mut ticks, mut dealloc_grew) = (0u32, false);
        for t in in_window {
            ticks += 1;
            dealloc_grew |= t.dealloc_delta > 0;
            for (k, d) in &t.deltas {
                sums.entry(*k).or_default().add(d);
            }
        }
        if ticks == 0 {
            return None;
        }
        Some(WindowSum {
            since_ms,
            until_ms,
            ticks,
            coverage_s: Self::covered_s(ticks, window_s),
            dealloc_grew,
            sums,
        })
    }

    /// Окно для ключей одного запроса за один проход, без карты сумм всех ключей: суммы — только по `keys`, итог — по
    /// всем дельтам окна. `None` — у этих ключей в окне дельт нет (или тиков нет).
    pub fn window_for(&self, keys: &[QueryKey], window_s: u32) -> Option<KeysWindow> {
        let (_, _, in_window) = self.in_window(window_s)?;
        let (mut sum, mut totals, mut any) = (Counters::default(), Totals::default(), false);
        for t in in_window {
            for (k, d) in &t.deltas {
                totals.calls += d.calls;
                totals.total_exec_ms += d.total_exec_ms;
                totals.rows += d.rows;
                if keys.contains(k) {
                    sum.add(d);
                    any = true;
                }
            }
        }
        any.then_some(KeysWindow { sum, totals })
    }

    /// Имена базы и роли ключа: из последнего сэмпла, а ушедшего из pgss ключа — из тиков, которые на него ссылаются.
    pub fn names(&self, key: &QueryKey) -> (Option<String>, Option<String>) {
        if let Some(c) = self.latest.get(key) {
            return (c.datname.clone(), c.rolname.clone());
        }
        self.tick_names
            .get(key)
            .map(|n| (n.datname.clone(), n.rolname.clone()))
            .unwrap_or_default()
    }

    /// История по тикам с `from_ms`: сумма дельт ключей `keys` в каждом тике, где они были.
    pub fn history(&self, keys: &[QueryKey], from_ms: i64) -> Vec<HistoryPoint> {
        self.ticks
            .iter()
            .filter(|t| t.at_ms >= from_ms)
            .filter_map(|t| {
                let mut sum = Counters::default();
                let mut any = false;
                for (k, d) in &t.deltas {
                    if keys.contains(k) {
                        sum.add(d);
                        any = true;
                    }
                }
                any.then(|| HistoryPoint {
                    at_ms: t.at_ms,
                    calls: sum.calls,
                    total_exec_ms: sum.total_exec_ms,
                    mean_exec_ms: if sum.calls > 0 {
                        sum.total_exec_ms / sum.calls as f64
                    } else {
                        0.0
                    },
                })
            })
            .collect()
    }

    pub fn gaps(&self, from_ms: i64) -> Vec<(i64, i64)> {
        self.gaps
            .iter()
            .copied()
            .filter(|(_, to)| *to >= from_ms)
            .collect()
    }

    pub fn last_head(&self) -> Option<Head> {
        self.last_head
    }

    pub fn info(&self) -> Option<PgssInfo> {
        self.info
    }

    pub fn pgss(&self) -> Option<&PgssInstalled> {
        self.pgss.as_ref()
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// Ключи pgss последнего тика.
    pub fn latest_keys(&self) -> HashSet<QueryKey> {
        self.latest.keys().copied().collect()
    }

    /// Ключи, найденные прошлым сканом, если строка поиска, `stats_reset` и `dealloc` те же и среди `keys` нет
    /// непроверенных: текст ключа в pgss не меняется, пока строку не сбросили или не вытеснили, и новый скан файла
    /// текстов ничего бы не добавил. Иначе — `None`, нужен скан.
    pub fn search_hit<'a>(
        &self,
        needle: &str,
        info: &PgssInfo,
        mut keys: impl Iterator<Item = &'a QueryKey>,
    ) -> Option<HashSet<QueryKey>> {
        let s = self.search.as_ref()?;
        (s.needle == needle
            && s.reset == (info.stats_reset_ms, info.dealloc)
            && keys.all(|k| s.checked.contains(k)))
        .then(|| s.matched.clone())
    }

    /// Итог скана: `checked` — ключи pgss, прочитанные до него.
    pub fn remember_search(
        &mut self,
        needle: &str,
        info: &PgssInfo,
        checked: HashSet<QueryKey>,
        matched: HashSet<QueryKey>,
    ) {
        self.search = Some(SearchHit {
            needle: needle.to_string(),
            reset: (info.stats_reset_ms, info.dealloc),
            checked,
            matched,
        });
    }
}

/// Вид сэмплера pgss для реестра.
pub struct Statements;

impl Sampling for Statements {
    type Sample = StatementsSample;
    type Buffer = StatementsBuffer;
    type Event = StatementsEvent;

    fn interval(&self) -> Duration {
        INTERVAL
    }

    fn push(
        buffer: &mut StatementsBuffer,
        at_ms: i64,
        sample: StatementsSample,
    ) -> Vec<StatementsEvent> {
        buffer.push(at_ms, sample)
    }

    fn error_event(error: AppError) -> StatementsEvent {
        StatementsEvent::Error { error }
    }

    fn stopped_event() -> StatementsEvent {
        StatementsEvent::Stopped
    }

    /// Расширение исчезло или слишком старое — остановка, а не повторы с паузой.
    fn fatal(error: &AppError) -> bool {
        matches!(
            error.code,
            ErrorCode::AuthFailed | ErrorCode::FeatureUnavailable
        )
    }
}

pub type StatementsRegistry = SamplerRegistry<Statements>;

pub fn pgss_missing() -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.queries.pgssMissing",
        "pg_stat_statements is not installed in this database"
    )
}

pub fn pgss_too_old(version: &str) -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.queries.pgssTooOld",
        "pg_stat_statements {{version}} is older than 1.8",
        version = version
    )
}

/// Сэмпл pgss узла через пул `Sampler`: шапка, расширение (кеш прошлого сэмпла; при ошибке чтения ищется заново),
/// строки без текстов.
pub struct PoolStatementsSource {
    st: AppState,
    connection_id: String,
    member: String,
    /// Расширение, найденное раньше; блокировка короткая и никогда не держится через `await`.
    pgss: Arc<Mutex<Option<PgssInstalled>>>,
}

impl PoolStatementsSource {
    pub fn new(st: AppState, connection_id: &str, member: &str) -> Self {
        Self {
            st,
            connection_id: connection_id.to_string(),
            member: member.to_string(),
            pgss: Arc::new(Mutex::new(None)),
        }
    }

    /// Предел получения соединения — как у каталога (таймаут подключения профиля + 10 с).
    fn acquire_limit(&self) -> Duration {
        self.st
            .runtimes
            .target(&self.connection_id, Some(&self.member))
            .map(|(_, profile)| catalog_limit(&profile))
            .unwrap_or(SAMPLE_TIMEOUT)
    }
}

impl SampleSource<StatementsSample> for PoolStatementsSource {
    fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<StatementsSample>> {
        Box::pin(async move {
            let cache = Arc::clone(&self.pgss);
            let kcache = KcacheNode::new(
                &self.st.kcache_probes,
                &self.connection_id,
                self.st
                    .runtimes
                    .generation(&self.connection_id)
                    .unwrap_or(0),
                &self.member,
            );
            on_catalog_node_split(
                self.acquire_limit(),
                SAMPLE_TIMEOUT,
                self.st.client_with(
                    &self.connection_id,
                    Some(&self.member),
                    None,
                    PoolPurpose::Sampler,
                ),
                move |c| {
                    Box::pin(async move {
                        let head = head(c).await?;
                        let (pgss, snapshot) = read_known_or_detect(c, &cache, &kcache).await?;
                        Ok(StatementsSample {
                            head,
                            pgss,
                            snapshot,
                        })
                    })
                },
            )
            .await
        })
    }

    /// Получение соединения плюс запрос: задача сэмплера не обрывает источник раньше его собственных пределов.
    fn limit(&self) -> Duration {
        self.acquire_limit() + SAMPLE_TIMEOUT
    }
}

/// Что делать после ошибки чтения при закешированном расширении.
#[derive(Debug, PartialEq)]
pub enum ReadRetry {
    /// Остановить сэмпл этой ошибкой: расширение исчезло или слишком старое (`FEATURE_UNAVAILABLE` — для
    /// сэмплера фатально) либо сбой временный (исходная ошибка, дальше — паузы реестра).
    Fail(AppError),
    /// Схема или версия расширения изменились: один повтор чтения с новыми возможностями.
    Retry(PgssInstalled),
}

/// Расширение из результата поиска или ошибка `FEATURE_UNAVAILABLE`, если его нет или оно старше 1.8.
fn require_installed(found: Detect) -> AppResult<PgssInstalled> {
    match found {
        Detect::Installed(p) => Ok(p),
        Detect::Missing => Err(pgss_missing()),
        Detect::TooOld(v) => Err(pgss_too_old(&v)),
    }
}

/// Решение после ошибки `original` чтения с закешированным `known`, когда расширение найдено заново
/// (`redetected`): исчезло или устарело — ошибка `FEATURE_UNAVAILABLE`; установлено иначе (другая схема или
/// версия, например `ALTER EXTENSION … UPDATE`) — повтор с новыми возможностями; как прежде — ошибка
/// временная, возвращается исходная.
pub fn after_read_error(
    known: &PgssInstalled,
    redetected: Detect,
    original: AppError,
) -> ReadRetry {
    match require_installed(redetected) {
        Ok(p) if p != *known => ReadRetry::Retry(p),
        Ok(_) => ReadRetry::Fail(original),
        Err(gone) => ReadRetry::Fail(gone),
    }
}

/// Найденное расширение запоминается в кеше источника, только если итог kcache узла известен (`settled`): после
/// разового сбоя пробы kcache (отмена, тайм-аут) кеш остаётся пустым, и следующий сэмпл ищет снова, а не живёт без
/// колонок kcache до перезапуска сэмплера.
pub fn remember_detected(
    cache: &Mutex<Option<PgssInstalled>>,
    found: &PgssInstalled,
    settled: bool,
) {
    *cache.lock_safe() = (settled || found.kcache.is_some()).then(|| found.clone());
}

/// Чтение строк pgss: расширение берётся из кеша источника; нет кеша — ищется. Ошибка чтения при кеше — повторный
/// поиск на том же соединении (`after_read_error`): «расширение исчезло» останавливает сэмплер, а не повторяется.
async fn read_known_or_detect(
    c: &Client,
    cache: &Mutex<Option<PgssInstalled>>,
    kcache: &KcacheNode,
) -> AppResult<(PgssInstalled, Snapshot)> {
    // Копия под короткой блокировкой: защёлка не доживает до `await`.
    let known = cache.lock_safe().clone();
    let Some(known) = known else {
        let found = require_installed(detect(c, Some(kcache)).await?)?;
        remember_detected(
            cache,
            &found,
            kcache.settled(crate::topology::model::now_ms()),
        );
        let snapshot = read_cumulative_on(c, &found, kcache).await?;
        return Ok((found, snapshot));
    };
    match read_cumulative_on(c, &known, kcache).await {
        Ok(snapshot) => Ok((known, snapshot)),
        Err(original) => {
            // Сбой самого поиска — тот же временный сбой: возвращается исходная ошибка чтения.
            let step = match detect(c, Some(kcache)).await {
                Ok(redetected) => after_read_error(&known, redetected, original),
                Err(_) => ReadRetry::Fail(original),
            };
            match step {
                ReadRetry::Fail(error) => Err(error),
                ReadRetry::Retry(found) => {
                    remember_detected(
                        cache,
                        &found,
                        kcache.settled(crate::topology::model::now_ms()),
                    );
                    let snapshot = read_cumulative_on(c, &found, kcache).await?;
                    Ok((found, snapshot))
                }
            }
        }
    }
}

/// Подписка на сэмплер pgss узла (`member = None` — «за лидером» на момент подписки); id сессии — для `stop_session`.
pub fn start(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    sink: Arc<dyn SamplerSink<StatementsEvent>>,
) -> AppResult<String> {
    crate::sampler::start_with(
        st,
        id,
        member,
        sink,
        &st.statements,
        SessionKind::Statements,
        |st, id, m| Arc::new(PoolStatementsSource::new(st.clone(), id, m)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::pgss::PgssCaps;
    use pg_advisor::queries::Counters;

    fn key(id: i64) -> QueryKey {
        QueryKey {
            query_id: id,
            dbid: 1,
            userid: 1,
            toplevel: true,
        }
    }

    fn row(id: i64, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: key(id),
            datname: Some("perf".into()),
            rolname: Some("app".into()),
            counters: Counters {
                calls,
                total_exec_ms: ms,
                rows: calls,
                ..Counters::default()
            },
            min_exec_ms: 0.0,
            max_exec_ms: 0.0,
            stddev_exec_ms: 0.0,
        }
    }

    fn sample(rows: Vec<Cumulative>, reset_ms: i64, dealloc: i64) -> StatementsSample {
        StatementsSample {
            head: Head {
                version_num: 170_000,
                in_recovery: false,
                now_ms: 0,
            },
            pgss: PgssInstalled {
                schema: "public".into(),
                version: "1.11".into(),
                caps: PgssCaps::from_version("1.11").unwrap(),
                kcache: None,
            },
            snapshot: Snapshot {
                rows,
                info: PgssInfo {
                    stats_reset_ms: Some(reset_ms),
                    dealloc: Some(dealloc),
                    max: 5000,
                    track_planning: true,
                },
                hidden: false,
            },
        }
    }

    const M: i64 = 60_000;

    fn ticks(e: &[StatementsEvent]) -> Vec<(u32, u32)> {
        e.iter()
            .filter_map(|e| match e {
                StatementsEvent::Tick { ticks, changed, .. } => Some((*ticks, *changed)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn first_tick_is_a_baseline_and_deltas_follow() {
        let mut b = StatementsBuffer::default();
        let e = b.push(M, sample(vec![row(1, 10, 100.0), row(2, 5, 50.0)], 1, 0));
        assert_eq!(ticks(&e), [(0, 0)], "первый тик — база без дельт");
        assert!(b.window(300).is_none());
        // Второй тик: изменился только ключ 1; ключ 3 появился — дельта равна накопительному.
        let e = b.push(
            2 * M,
            sample(
                vec![row(1, 14, 160.0), row(2, 5, 50.0), row(3, 2, 20.0)],
                1,
                0,
            ),
        );
        assert_eq!(ticks(&e), [(1, 2)]);
        // Тик с дельтами покрывает свой интервал: от базы (минута 1) до минуты 2 — 60 с.
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.coverage_s, w.dealloc_grew), (1, 60, false));
        assert_eq!(w.sums[&key(1)].calls, 4);
        assert_eq!(w.sums[&key(1)].total_exec_ms, 60.0);
        assert_eq!(w.sums[&key(3)].calls, 2);
        assert!(!w.sums.contains_key(&key(2)));
        assert_eq!(b.names(&key(1)), (Some("perf".into()), Some("app".into())));
        assert_eq!(b.last_head().map(|h| h.version_num), Some(170_000));
        assert_eq!(b.info().and_then(|i| i.stats_reset_ms), Some(1));
    }

    /// kcache появился между тиками: его колонки на этом тике — новая база, дельты CPU — со следующего тика.
    #[test]
    fn kcache_change_starts_a_new_base_for_its_columns() {
        let with_cpu = |calls: i64, cpu: f64| {
            let mut r = row(1, calls, calls as f64);
            r.counters.cpu_user_ms = Some(cpu);
            r.counters.cpu_sys_ms = Some(0.0);
            r
        };
        let kcache = |mut s: StatementsSample| {
            s.pgss.kcache = Some(crate::diag::queries::KcacheCaps {
                schema: "public".into(),
                exec_prefix: true,
                has_top: true,
            });
            s
        };
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 10, 10.0)], 1, 0));
        b.push(2 * M, kcache(sample(vec![with_cpu(20, 5000.0)], 1, 0)));
        let w = b.window(300).unwrap();
        assert_eq!(w.sums[&key(1)].calls, 10);
        assert_eq!(
            w.sums[&key(1)].cpu_user_ms,
            None,
            "накопительное CPU — не дельта"
        );
        b.push(3 * M, kcache(sample(vec![with_cpu(30, 5040.0)], 1, 0)));
        let w = b.window(300).unwrap();
        assert_eq!(w.sums[&key(1)].cpu_user_ms, Some(40.0));
        // Замечание 15 смоука P3b: сброс pgss (`stats_reset` сменился) kcache не сбрасывает — счётчики pgss целиком,
        // CPU дельтой; уменьшившийся CPU (сброшен и kcache) — целиком.
        b.push(4 * M, kcache(sample(vec![with_cpu(3, 5100.0)], 2, 0)));
        let w = b.window(60).unwrap();
        assert_eq!(
            (w.sums[&key(1)].calls, w.sums[&key(1)].cpu_user_ms),
            (3, Some(60.0))
        );
        b.push(5 * M, kcache(sample(vec![with_cpu(4, 7.0)], 3, 0)));
        let w = b.window(60).unwrap();
        assert_eq!(
            (w.sums[&key(1)].calls, w.sums[&key(1)].cpu_user_ms),
            (4, Some(7.0))
        );
        // kcache исчез: колонок нет, счётчики pgss идут дальше.
        b.push(6 * M, sample(vec![row(1, 9, 9.0)], 3, 0));
        let w = b.window(60).unwrap();
        assert_eq!(
            (w.sums[&key(1)].calls, w.sums[&key(1)].cpu_user_ms),
            (5, None)
        );
        // kcache вернулся вместе со сбросом pgss: его прошлые значения — от другого чтения, колонок нет.
        b.push(7 * M, kcache(sample(vec![with_cpu(2, 9000.0)], 4, 0)));
        let w = b.window(60).unwrap();
        assert_eq!(
            (w.sums[&key(1)].calls, w.sums[&key(1)].cpu_user_ms),
            (2, None)
        );
    }

    #[test]
    fn decreased_counters_and_reset_start_a_new_base() {
        let mut b = StatementsBuffer::default();
        b.push(
            M,
            sample(vec![row(1, 100, 1000.0), row(2, 100, 1000.0)], 1, 0),
        );
        // Ключ 1 вытеснен и вернулся с малыми счётчиками: дельта — его накопительное значение, не отрицательная.
        let e = b.push(
            2 * M,
            sample(vec![row(1, 3, 30.0), row(2, 101, 1010.0)], 1, 1),
        );
        assert_eq!(ticks(&e), [(1, 2)]);
        let w = b.window(300).unwrap();
        assert_eq!(
            (w.sums[&key(1)].calls, w.sums[&key(2)].calls, w.dealloc_grew),
            (3, 1, true)
        );
        // Сброс статистики: новая база для всех ключей, прошлые тики остаются. `dealloc` обнулён сбросом и снова
        // вырос до 2: дельта тика сброса — накопительное значение 2, а не разность со старым (1).
        let e = b.push(3 * M, sample(vec![row(1, 2, 20.0), row(2, 7, 70.0)], 2, 2));
        assert_eq!(ticks(&e), [(2, 2)]);
        let w = b.window(300).unwrap();
        assert_eq!(
            (w.ticks, w.sums[&key(1)].calls, w.sums[&key(2)].calls),
            (2, 5, 8)
        );
        assert!(
            w.dealloc_grew,
            "рост dealloc во втором тике остаётся в окне"
        );
        // Окно из одного последнего тика (рост в прошлом тике сюда не входит): вытеснения после сброса видны.
        let last = b.window(60).unwrap();
        assert_eq!(last.ticks, 1);
        assert!(
            last.dealloc_grew,
            "dealloc на тике сброса — вытеснения с момента сброса"
        );
        assert_eq!(b.ticks.back().map(|t| t.dealloc_delta), Some(2));
        // Сброс без вытеснений: dealloc = 0, окно из одного тика роста не показывает.
        b.push(4 * M, sample(vec![row(1, 3, 30.0), row(2, 8, 80.0)], 3, 0));
        let last = b.window(60).unwrap();
        assert_eq!(last.ticks, 1);
        assert!(
            !last.dealloc_grew,
            "сброс с dealloc = 0 роста вытеснений не даёт"
        );
        assert_eq!(b.ticks.back().map(|t| t.dealloc_delta), Some(0));
    }

    #[test]
    fn long_pause_is_a_gap_and_a_new_base() {
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 10, 100.0)], 1, 0));
        b.push(2 * M, sample(vec![row(1, 12, 120.0)], 1, 0));
        let e = b.push(10 * M, sample(vec![row(1, 500, 5000.0)], 1, 0));
        assert!(
            matches!(e[0], StatementsEvent::Gap { from_ms, to_ms } if from_ms == 3 * M && to_ms == 10 * M),
            "{e:?}"
        );
        // После промежутка — база: изменённых ключей нет, активность за паузу не приписывается одному тику; тик
        // до паузы остаётся в часовом окне.
        assert_eq!(ticks(&e), [(1, 0)]);
        assert_eq!(b.gaps(0), [(3 * M, 10 * M)]);
        assert!(b.window(300).is_none(), "в окне 5 мин тиков с дельтами нет");
        assert_eq!(
            b.window(3600).unwrap().ticks,
            1,
            "старый тик виден в часовом окне"
        );
    }

    #[test]
    fn window_covers_only_recent_ticks_and_coverage_is_capped() {
        let mut b = StatementsBuffer::default();
        for i in 0..8 {
            b.push(
                (i + 1) * M,
                sample(vec![row(1, 10 * (i + 1), 100.0 * (i + 1) as f64)], 1, 0),
            );
        }
        // Окно 5 мин от последнего тика (8 мин): тики 4…8 — пять тиков с дельтами, покрытие 5 мин.
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.coverage_s, w.sums[&key(1)].calls), (5, 300, 50));
        let w = b.window(3600).unwrap();
        // Семь тиков с дельтами (минуты 2…8): от минуты 1 (база) до минуты 8 — 420 с.
        assert_eq!((w.ticks, w.coverage_s, w.sums[&key(1)].calls), (7, 420, 70));
        let h = b.history(&[key(1)], 0);
        assert_eq!(h.len(), 7);
        assert_eq!(
            (h[0].at_ms, h[0].calls, h[0].mean_exec_ms),
            (2 * M, 10, 10.0)
        );
    }

    /// Покрытие — собранное, а не пролёт: пауза сэмплера внутри окна (вкладка была закрыта) его уменьшает.
    #[test]
    fn coverage_counts_collected_ticks_not_the_span() {
        let mut b = StatementsBuffer::default();
        for i in 1..=3 {
            b.push(i * M, sample(vec![row(1, 10 * i, 10.0 * i as f64)], 1, 0));
        }
        // Пауза дольше GAP_AFTER_MS: тик на 6-й минуте — новая база, на 7-й — снова дельта.
        b.push(6 * M, sample(vec![row(1, 100, 1000.0)], 1, 0));
        let e = b.push(7 * M, sample(vec![row(1, 110, 1100.0)], 1, 0));
        // Окно 5 мин от 7-й минуты: тики с дельтами на 3-й и 7-й минутах — 2 минуты сбора, а не 5.
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.coverage_s), (2, 120));
        assert!(
            matches!(
                e.last(),
                Some(StatementsEvent::Tick {
                    ticks: 3,
                    coverage_s: 180,
                    ..
                })
            ),
            "часовое окно: три тика с дельтами — {e:?}"
        );
        // Покрытие не больше окна.
        let mut b = StatementsBuffer::default();
        for i in 1..=10 {
            b.push(i * M, sample(vec![row(1, i, i as f64)], 1, 0));
        }
        assert_eq!(
            b.window(300).map(|w| (w.ticks, w.coverage_s)),
            Some((5, 300))
        );
    }

    /// Ключ ушёл из pgss (сброс, вытеснение), а его дельты ещё в окне: имена базы и роли остаются, пока на ключ
    /// ссылаются тики; после вытеснения этих тиков — забываются.
    #[test]
    fn names_outlive_the_key_while_ticks_reference_it() {
        let mut b = StatementsBuffer::default();
        let other = |id: i64| Cumulative {
            datname: Some("shop".into()),
            rolname: Some("web".into()),
            ..row(id, 1, 1.0)
        };
        b.push(M, sample(vec![row(1, 1, 1.0), other(2)], 1, 0));
        b.push(2 * M, sample(vec![row(1, 2, 2.0), other(2)], 1, 0));
        b.push(
            3 * M,
            sample(
                vec![
                    row(1, 3, 3.0),
                    Cumulative {
                        counters: Counters {
                            calls: 5,
                            total_exec_ms: 5.0,
                            ..Counters::default()
                        },
                        ..other(2)
                    },
                ],
                1,
                0,
            ),
        );
        // Сброс: в pgss только ключ 1, ключ 2 остался лишь в тиках окна.
        b.push(4 * M, sample(vec![row(1, 1, 1.0)], 2, 0));
        assert!(b.window(300).unwrap().sums.contains_key(&key(2)));
        assert_eq!(b.names(&key(2)), (Some("shop".into()), Some("web".into())));
        assert_eq!(b.names(&key(1)), (Some("perf".into()), Some("app".into())));
        // Тики с ключом 2 вытеснены возрастом (после паузы — новая база): имён больше нет.
        b.push(4 * M + MAX_AGE_MS + 1, sample(vec![row(1, 2, 2.0)], 2, 0));
        assert!(b.window(3600).is_none());
        assert_eq!(b.names(&key(2)), (None, None));
        assert!(b.tick_names.is_empty(), "имена без ссылок тиков не копятся");
    }

    #[test]
    fn eviction_drops_whole_old_ticks() {
        let mut b = StatementsBuffer::with_limits(5, MAX_AGE_MS);
        b.push(M, sample((1..=4).map(|i| row(i, 1, 1.0)).collect(), 1, 0));
        b.push(
            2 * M,
            sample((1..=4).map(|i| row(i, 2, 2.0)).collect(), 1, 0),
        ); // 4 записи
        b.push(
            3 * M,
            sample((1..=4).map(|i| row(i, 3, 3.0)).collect(), 1, 0),
        ); // 8 > 5: первый тик дельт вытеснен
        let w = b.window(3600).unwrap();
        assert_eq!((w.ticks, w.sums[&key(1)].calls), (1, 1));
        // Возраст: тик старше часа уходит целиком.
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 1, 1.0)], 1, 0));
        b.push(2 * M, sample(vec![row(1, 2, 2.0)], 1, 0));
        b.push(2 * M + MAX_AGE_MS + 1, sample(vec![row(1, 3, 3.0)], 1, 0));
        assert_eq!(
            b.window(3600).map(|w| w.ticks),
            None,
            "после промежутка новая база, старый тик вытеснен возрастом"
        );
    }

    #[test]
    fn search_is_reused_while_keys_and_reset_stay() {
        let mut b = StatementsBuffer::default();
        let info = PgssInfo {
            stats_reset_ms: Some(1),
            dealloc: Some(0),
            ..PgssInfo::default()
        };
        let checked: HashSet<QueryKey> = [key(1), key(2)].into();
        assert_eq!(b.search_hit("orders", &info, checked.iter()), None);
        b.remember_search("orders", &info, checked.clone(), [key(2)].into());
        assert_eq!(
            b.search_hit("orders", &info, [key(2)].iter()),
            Some([key(2)].into()),
            "те же ключи (или меньше) — без скана"
        );
        assert_eq!(
            b.search_hit("Orders", &info, checked.iter()),
            None,
            "другая строка"
        );
        assert_eq!(
            b.search_hit("orders", &info, [key(1), key(3)].iter()),
            None,
            "новый ключ — его текст не проверен"
        );
        let reset = PgssInfo {
            stats_reset_ms: Some(2),
            ..info
        };
        assert_eq!(
            b.search_hit("orders", &reset, checked.iter()),
            None,
            "сброс"
        );
        let dealloc = PgssInfo {
            dealloc: Some(1),
            ..info
        };
        assert_eq!(
            b.search_hit("orders", &dealloc, checked.iter()),
            None,
            "вытеснение"
        );
        b.push(M, sample(vec![row(1, 1, 1.0), row(3, 1, 1.0)], 1, 0));
        assert_eq!(b.latest_keys(), [key(1), key(3)].into());
    }

    /// `window_for` — то же, что `window` с суммированием по ключам: суммы только ключей запроса, итог — по всему окну.
    #[test]
    fn window_for_equals_the_full_window_restricted_to_the_keys() {
        let mut b = StatementsBuffer::default();
        for i in 0..8i64 {
            b.push(
                (i + 1) * M,
                sample(
                    vec![
                        row(1, 10 * (i + 1), 100.0 * (i + 1) as f64),
                        row(2, 3 * (i + 1) * (i % 2), 8.0 * (i + 1) as f64),
                        row(3, 7 * (i + 1), 50.0 * (i + 1) as f64),
                    ],
                    1,
                    0,
                ),
            );
        }
        for w in [60, 300, 900, 3600] {
            let full = b.window(w).unwrap();
            for keys in [vec![key(1)], vec![key(1), key(2)], vec![key(2), key(3)]] {
                let mut sum = Counters::default();
                let mut any = false;
                for k in &keys {
                    if let Some(c) = full.sums.get(k) {
                        sum.add(c);
                        any = true;
                    }
                }
                let got = b
                    .window_for(&keys, w)
                    .unwrap_or_else(|| panic!("{w} {keys:?}"));
                assert!(any);
                assert_eq!(got.sum, sum, "{w} {keys:?}");
                assert_eq!(
                    got.totals,
                    pg_advisor::queries::totals(full.sums.values()),
                    "{w}"
                );
            }
            assert!(
                b.window_for(&[key(9)], w).is_none(),
                "у ключа нет дельт в окне"
            );
            assert!(b.window_for(&[], w).is_none());
        }
        let empty = StatementsBuffer::default();
        assert!(empty.window_for(&[key(1)], 300).is_none(), "тиков нет");
    }

    fn installed(version: &str, schema: &str) -> PgssInstalled {
        PgssInstalled {
            schema: schema.into(),
            version: version.into(),
            caps: PgssCaps::from_version(version).unwrap(),
            kcache: None,
        }
    }

    /// Разовый сбой пробы kcache (итога узла нет) не закрепляется в кеше сэмплера: следующий сэмпл ищет снова.
    #[test]
    fn transient_kcache_probe_failure_is_not_kept_for_the_sampler_lifetime() {
        let cache = Mutex::new(None);
        let found = installed("1.11", "public");
        remember_detected(&cache, &found, false);
        assert_eq!(
            *cache.lock_safe(),
            None,
            "итог kcache неизвестен — поиск повторится"
        );
        remember_detected(&cache, &found, true);
        assert_eq!(
            *cache.lock_safe(),
            Some(found.clone()),
            "отказ или «не установлено» — итог"
        );
        let with = PgssInstalled {
            kcache: Some(crate::diag::queries::KcacheCaps {
                schema: "public".into(),
                exec_prefix: true,
                has_top: true,
            }),
            ..found.clone()
        };
        remember_detected(&cache, &with, false);
        assert_eq!(*cache.lock_safe(), Some(with));
        remember_detected(&cache, &found, false);
        assert_eq!(
            *cache.lock_safe(),
            None,
            "повтор после ошибки чтения с разовым сбоем"
        );
    }

    /// Обычная ошибка PostgreSQL после `DROP EXTENSION`: `undefined_function` (42883), код — не `FEATURE_UNAVAILABLE`.
    fn read_failure() -> AppError {
        app_err!(
            ErrorCode::Internal,
            "errors.pg.serverError",
            "PostgreSQL error {{code}}",
            code = "42883"
        )
    }

    #[test]
    fn read_error_decides_between_stop_retry_and_backoff() {
        let known = installed("1.11", "public");
        // Расширение удалено: FEATURE_UNAVAILABLE, для сэмплера фатально — остановка, а не повторы.
        let ReadRetry::Fail(gone) = after_read_error(&known, Detect::Missing, read_failure())
        else {
            panic!("исчезнувшее расширение — не повтор");
        };
        assert_eq!(gone.code, ErrorCode::FeatureUnavailable);
        assert_eq!(gone, pgss_missing());
        assert!(Statements::fatal(&gone));
        // Версию откатили ниже 1.8: тоже FEATURE_UNAVAILABLE, с версией в параметрах.
        let ReadRetry::Fail(old) =
            after_read_error(&known, Detect::TooOld("1.7".into()), read_failure())
        else {
            panic!("слишком старое расширение — не повтор");
        };
        assert_eq!(old.code, ErrorCode::FeatureUnavailable);
        assert_eq!(old, pgss_too_old("1.7"));
        assert!(Statements::fatal(&old));
        // `ALTER EXTENSION … UPDATE` или другая схема: один повтор с новыми возможностями.
        let updated = installed("1.12", "public");
        assert_eq!(
            after_read_error(&known, Detect::Installed(updated.clone()), read_failure()),
            ReadRetry::Retry(updated)
        );
        let moved = installed("1.11", "ext");
        assert_eq!(
            after_read_error(&known, Detect::Installed(moved.clone()), read_failure()),
            ReadRetry::Retry(moved)
        );
        // Расширение то же: сбой временный — исходная ошибка, не фатальная; дальше паузы реестра.
        let ReadRetry::Fail(same) =
            after_read_error(&known, Detect::Installed(known.clone()), read_failure())
        else {
            panic!("то же расширение — не повтор");
        };
        assert_eq!(same, read_failure());
        assert!(!Statements::fatal(&same));
    }

    #[test]
    fn statements_events_json_and_fatal_errors() {
        let v = serde_json::to_value(StatementsEvent::Tick {
            at_ms: 1,
            coverage_s: 120,
            ticks: 2,
            changed: 7,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "tick", "atMs": 1, "coverageS": 120, "ticks": 2, "changed": 7})
        );
        assert!(Statements::fatal(&app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.queries.pgssMissing",
            "pg_stat_statements is not installed in this database"
        )));
        assert!(Statements::fatal(&app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.authFailed",
            "x"
        )));
        assert!(!Statements::fatal(&app_err!(
            ErrorCode::Timeout,
            "errors.pg.statementTimeout",
            "x"
        )));
        assert_eq!(Statements.interval(), INTERVAL);
    }
}
