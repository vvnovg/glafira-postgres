//! Сэмплер `pg_stat_statements` (спец. P3 §5): раз в минуту `pg_stat_statements(showtext := false)` на соединении
//! пула `Sampler`, буфер дельт на 60 мин; живёт, пока подписаны инструмент «Запросы» или карточки запроса узла.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use pg_advisor::queries::{Counters, Cumulative, QueryKey};
use serde::Serialize;

use crate::commands::catalog::{catalog_limit, on_catalog_node_split};
use crate::conn::pool::PoolPurpose;
use crate::diag::queries::{Detect, PgssInfo, PgssInstalled, Snapshot, detect, read_cumulative};
use crate::diag::{Head, head};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::registry::{
    SAMPLE_TIMEOUT, SampleSource, SamplerRegistry, SamplerSink, Sampling,
};
use crate::session::SessionKind;
use crate::state::AppState;

pub const INTERVAL: Duration = Duration::from_secs(60);
/// Записей `(ключ, дельта)` на узел — порядка 50 МБ.
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_AGE_MS: i64 = 3_600_000;
/// Пауза между тиками дольше — промежуток: сэмплер стоял (вкладка была закрыта), и следующий тик — новая база.
pub const GAP_AFTER_MS: i64 = 150_000;
/// Кеш текстов узла; при переполнении очищается целиком.
pub const MAX_TEXTS: usize = 2000;

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
    texts: HashMap<QueryKey, (Arc<str>, bool)>,
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
            texts: HashMap::new(),
            max_entries,
            max_age_ms,
        }
    }

    /// Тик: дельты изменившихся ключей против прошлых накопительных значений. Первый тик и тик после промежутка —
    /// только база (дельт нет). Ключ с уменьшившимися счётчиками (вытеснен и вернулся) и все ключи после смены
    /// `stats_reset` получают новую базу: дельта равна накопительному значению.
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
        let mut deltas = Vec::new();
        if !baseline {
            for row in &s.snapshot.rows {
                let delta = match self.latest.get(&row.key) {
                    Some(prev) if !reset && !row.counters.decreased_from(&prev.counters) => {
                        if !row.counters.changed_since(&prev.counters) {
                            continue;
                        }
                        row.counters.delta(&prev.counters)
                    }
                    _ => row.counters,
                };
                if delta.calls == 0 && delta.total_exec_ms == 0.0 {
                    continue;
                }
                deltas.push((row.key, delta));
            }
        }
        let dealloc_delta = match (self.info.and_then(|i| i.dealloc), s.snapshot.info.dealloc) {
            (Some(prev), Some(now)) if !baseline && !reset && now > prev => now - prev,
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
        let (coverage_s, ticks) = self
            .window((self.max_age_ms / 1000) as u32)
            .map_or((0, 0), |w| (w.coverage_s, w.ticks));
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
        }
        self.gaps.retain(|(_, to)| now_ms - to <= self.max_age_ms);
    }

    /// Сумма дельт по тикам в `(последний − window_s, последний]`; `None` — тиков с дельтами в окне нет.
    /// Покрытие — от первого тика окна до последнего плюс минута первого тика, не больше окна.
    pub fn window(&self, window_s: u32) -> Option<WindowSum> {
        let until_ms = self.last_at_ms?;
        let since_ms = until_ms - i64::from(window_s) * 1000;
        let mut sums: HashMap<QueryKey, Counters> = HashMap::new();
        let (mut ticks, mut first_at, mut dealloc_grew) = (0u32, None, false);
        for t in self.ticks.iter().filter(|t| t.at_ms > since_ms) {
            ticks += 1;
            first_at.get_or_insert(t.at_ms);
            dealloc_grew |= t.dealloc_delta > 0;
            for (k, d) in &t.deltas {
                sums.entry(*k).or_default().add(d);
            }
        }
        let first_at = first_at?;
        let span_s = ((until_ms - first_at) / 1000) as u32 + INTERVAL.as_secs() as u32;
        Some(WindowSum {
            since_ms,
            until_ms,
            ticks,
            coverage_s: span_s.min(window_s),
            dealloc_grew,
            sums,
        })
    }

    pub fn names(&self, key: &QueryKey) -> (Option<String>, Option<String>) {
        self.latest
            .get(key)
            .map(|c| (c.datname.clone(), c.rolname.clone()))
            .unwrap_or_default()
    }

    /// Накопительные строки запроса в базе (по ролям и `toplevel`).
    pub fn latest_for(&self, query_id: i64, dbid: u32) -> Vec<&Cumulative> {
        let mut v: Vec<&Cumulative> = self
            .latest
            .values()
            .filter(|c| c.key.query_id == query_id && c.key.dbid == dbid)
            .collect();
        v.sort_by_key(|c| (c.key.userid, !c.key.toplevel));
        v
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

    pub fn text(&self, key: &QueryKey) -> Option<(Arc<str>, bool)> {
        self.texts.get(key).cloned()
    }

    pub fn remember_texts(&mut self, items: impl IntoIterator<Item = (QueryKey, String, bool)>) {
        let items: Vec<(QueryKey, String, bool)> = items.into_iter().collect();
        if self.texts.len() + items.len() > MAX_TEXTS {
            self.texts.clear();
        }
        for (k, t, truncated) in items {
            self.texts.insert(k, (Arc::from(t.as_str()), truncated));
        }
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

/// Сэмпл pgss узла через пул `Sampler`: шапка, расширение (ищется один раз), строки без текстов.
pub struct PoolStatementsSource {
    st: AppState,
    connection_id: String,
    member: String,
    pgss: OnceLock<PgssInstalled>,
}

impl PoolStatementsSource {
    pub fn new(st: AppState, connection_id: &str, member: &str) -> Self {
        Self {
            st,
            connection_id: connection_id.to_string(),
            member: member.to_string(),
            pgss: OnceLock::new(),
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
            let known = self.pgss.get().cloned();
            let (sample, pgss) = on_catalog_node_split(
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
                        let pgss = match known {
                            Some(p) => p,
                            None => match detect(c).await? {
                                Detect::Installed(p) => p,
                                Detect::Missing => return Err(pgss_missing()),
                                Detect::TooOld(v) => return Err(pgss_too_old(&v)),
                            },
                        };
                        let snapshot = read_cumulative(c, &pgss).await?;
                        Ok((
                            StatementsSample {
                                head,
                                pgss: pgss.clone(),
                                snapshot,
                            },
                            pgss,
                        ))
                    })
                },
            )
            .await?;
            let _ = self.pgss.set(pgss);
            Ok(sample)
        })
    }

    /// Получение соединения плюс запрос: задача сэмплера не обрывает источник раньше его собственных пределов.
    fn limit(&self) -> Duration {
        self.acquire_limit() + SAMPLE_TIMEOUT
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
        // Сброс статистики: новая база для всех ключей, прошлые тики остаются.
        let e = b.push(3 * M, sample(vec![row(1, 2, 20.0), row(2, 7, 70.0)], 2, 0));
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
        // Окно из одного последнего тика: после сброса dealloc (1 → 0) не сравнивается со старым значением.
        let last = b.window(60).unwrap();
        assert_eq!(last.ticks, 1);
        assert!(
            !last.dealloc_grew,
            "dealloc после сброса не сравнивается со старым"
        );
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
    fn texts_cache_is_bounded() {
        let mut b = StatementsBuffer::default();
        b.remember_texts([(key(1), "SELECT 1".to_string(), false)]);
        assert_eq!(
            b.text(&key(1)).map(|(t, tr)| (t.to_string(), tr)),
            Some(("SELECT 1".into(), false))
        );
        assert!(b.text(&key(2)).is_none());
        b.remember_texts((2..=(MAX_TEXTS as i64 + 1)).map(|i| (key(i), format!("q{i}"), false)));
        assert!(
            b.text(&key(1)).is_none(),
            "переполнение очищает кеш целиком"
        );
        assert!(
            b.text(&key(MAX_TEXTS as i64 + 1)).is_some(),
            "новые тексты записаны"
        );
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
