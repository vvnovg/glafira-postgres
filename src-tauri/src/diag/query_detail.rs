//! Карточка запроса (спец. P3 §7.2): накопительные строки pgss по ролям и `toplevel`, окна и история из буфера
//! сэмплера pgss, выполняющиеся экземпляры из `pg_stat_activity` (14+), ожидания из профиля `pg_wait_sampling` (P3b),
//! а без него — из буфера сэмплера активности.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pg_advisor::ash::Sample;
use pg_advisor::findings::Finding;
use pg_advisor::pgss::PgssCaps;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat};
use serde::Serialize;
use tokio_postgres::Client;

use crate::catalog::ddl::quote_ident;
use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::queries::{
    Detect, PgssInfo, PgssInstalled, detect, info_sql, missing, preloaded, row_from,
    statements_sql, too_old,
};
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::sql::BACKEND_START_US;
use crate::diag::{Head, Unavailable, WINDOWS_S, head, on_member};
use crate::error::{AppResult, ErrorCode};
use crate::product::SQL_MARKER;
use crate::sampler::statements::{HistoryPoint, KeysWindow, StatementsBuffer};
use crate::state::AppState;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

/// История и ожидания — за последний час.
const LOOKBACK_MS: i64 = 3_600_000;

/// Выполняющийся экземпляр запроса: активный ведущий бэкенд с этим `query_id` в `pg_stat_activity` (14+).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningInstance {
    pub pid: i32,
    pub backend_start_us: i64,
    pub state: Option<String>,
    pub started_ms: Option<i64>,
    pub duration_s: Option<f64>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub usename: Option<String>,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub query: String,
    /// Текст упёрся в `track_activity_query_size` или срезан до `QUERY_LIMIT`: литералы могут быть обрезаны, для плана
    /// такой текст не годится.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitsSource {
    Sampler,
    /// P3b: `pg_wait_sampling`.
    PgWaitSampling,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitItem {
    pub event_type: String,
    pub event: Option<String>,
    pub count: u64,
    pub share_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryWaits {
    pub source: WaitsSource,
    pub since_ms: i64,
    /// Сколько секунд охватывают сэмплы (от `since_ms` до момента чтения; не больше часа). У профиля — `None`
    /// (накопительный с его сброса, время сброса профиль не хранит).
    pub coverage_s: Option<i64>,
    pub total: u64,
    pub items: Vec<WaitItem>,
}

/// Строка pgss запроса по роли и `toplevel`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ByUser {
    pub rolname: Option<String>,
    pub toplevel: bool,
    pub stat: QueryStat,
}

/// Статистика запроса за окно буфера сэмплера; `None` — дельт запроса в окне нет.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStat {
    pub window_s: u32,
    pub stat: Option<QueryStat>,
}

/// Промежуток, когда сэмплер pgss стоял.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub from_ms: i64,
    pub to_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetail {
    pub text: Option<String>,
    pub text_truncated: bool,
    /// Сумма строк pgss запроса в базе; `None` — запроса в pgss уже нет.
    pub since_reset: Option<QueryStat>,
    pub windows: Vec<WindowStat>,
    pub totals: Totals,
    pub by_user: Vec<ByUser>,
    pub history: Vec<HistoryPoint>,
    pub gaps: Vec<Gap>,
    pub running: Vec<RunningInstance>,
    /// Профиль `pg_wait_sampling` (если расширение читается и в нём есть запрос), иначе сэмплы активности; `None` —
    /// ни того, ни другого (буфера сэмплера на узле нет: вкладка «Профиль ожиданий» не открывалась).
    pub waits: Option<QueryWaits>,
    pub findings: Vec<Finding>,
    pub unavailable: Vec<Unavailable>,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub pgss_version: Option<String>,
    /// `pg_stat_statements.track_planning`: выключен — колонки планирования нулевые, а не «планирование бесплатно».
    pub track_planning: bool,
    /// `pg_stat_kcache` установлен и читается: строки CPU и диска в метриках показываются (без данных у запроса — «—»).
    pub kcache: bool,
}

impl QueryDetail {
    fn empty(h: &Head) -> Self {
        Self {
            text: None,
            text_truncated: false,
            since_reset: None,
            windows: WINDOWS_S
                .iter()
                .map(|w| WindowStat {
                    window_s: *w,
                    stat: None,
                })
                .collect(),
            totals: Totals::default(),
            by_user: Vec::new(),
            history: Vec::new(),
            gaps: Vec::new(),
            running: Vec::new(),
            waits: None,
            findings: Vec::new(),
            unavailable: Vec::new(),
            server_version_num: h.version_num,
            in_recovery: h.in_recovery,
            pgss_version: None,
            track_planning: false,
            kcache: false,
        }
    }

    /// Карточка без pgss: только шапка и пометка.
    pub fn unavailable_pgss(version_num: i32, in_recovery: bool) -> Self {
        let mut d = Self::empty(&Head {
            version_num,
            in_recovery,
            now_ms: 0,
        });
        d.unavailable.push(missing());
        d
    }
}

/// Сумма строк запроса (по ролям и `toplevel`): счётчики складываются, `min` — наименьший, `max` — наибольший,
/// `stddev` — у строки с наибольшим числом вызовов (объединить стандартные отклонения точно нельзя).
pub fn aggregate(rows: &[Cumulative]) -> Option<(Counters, (f64, f64, f64))> {
    let first = rows.first()?;
    let mut c = Counters::default();
    let (mut min, mut max) = (first.min_exec_ms, first.max_exec_ms);
    let mut busiest = first;
    for r in rows {
        c.add(&r.counters);
        min = min.min(r.min_exec_ms);
        max = max.max(r.max_exec_ms);
        if r.counters.calls > busiest.counters.calls {
            busiest = r;
        }
    }
    Some((c, (min, max, busiest.stddev_exec_ms)))
}

/// Строка окна по ключам запроса (`StatementsBuffer::window_for`); доля — от всего окна.
pub fn window_stat(w: &KeysWindow, key: QueryKey) -> QueryStat {
    stat(key, None, None, &w.sum, None, &w.totals)
}

/// Порядок строк ожиданий: больше сэмплов — выше, при равенстве — по типу и событию.
fn by_samples(a: &WaitItem, b: &WaitItem) -> std::cmp::Ordering {
    b.count
        .cmp(&a.count)
        .then_with(|| a.event_type.cmp(&b.event_type))
        .then_with(|| a.event.cmp(&b.event))
}

/// Ожидания запроса из сэмплов активности: группировка по типу и событию; активные без ожидания — `CPU`.
pub fn waits_of(samples: &[Sample<'_>], query_id: i64, since_ms: i64) -> Option<QueryWaits> {
    let mut groups: HashMap<(String, Option<String>), u64> = HashMap::new();
    let mut total = 0u64;
    for s in samples
        .iter()
        .filter(|s| s.at_ms >= since_ms && s.query_id == Some(query_id))
    {
        let key = match s.wait_type {
            Some(t) => (t.to_string(), s.wait_event.map(str::to_string)),
            None => ("CPU".to_string(), None),
        };
        *groups.entry(key).or_insert(0) += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }
    let mut items: Vec<WaitItem> = groups
        .into_iter()
        .map(|((event_type, event), count)| WaitItem {
            event_type,
            event,
            count,
            share_pct: count as f64 * 100.0 / total as f64,
        })
        .collect();
    items.sort_by(by_samples);
    Some(QueryWaits {
        source: WaitsSource::Sampler,
        since_ms,
        coverage_s: None,
        total,
        items,
    })
}

/// Профиль `pg_wait_sampling` запроса по типу и событию, сумма по процессам (`profile_pid`). Строки без ожидания
/// (`sample_cpu`, NULL в `event_type`) — `CPU`, как у сэмплов активности. `queryid` профиля — запрос верхнего уровня
/// при `profile_queries = top` (ожидания вложенных инструкций приписаны вызывающей) и текущая инструкция при `all`.
/// Базы в профиле нет (`dbid` он не хранит): у одного `queryid` (одинаковый нормализованный текст в разных базах)
/// ожидания всех баз суммируются.
pub fn waits_sql(ns: &str) -> String {
    format!(
        "{SQL_MARKER}SELECT COALESCE(event_type, 'CPU')::text, event::text, sum(count)::bigint \
         FROM {ns}.pg_wait_sampling_profile WHERE queryid = $1 GROUP BY 1, 2 ORDER BY 3 DESC, 1, 2"
    )
}

/// Профиль `pg_wait_sampling` (накопительный с момента `pg_wait_sampling_reset_profile()`, поэтому `since_ms: 0`):
/// доли по типу и событию. Пусто — `None` (карточка возьмёт сэмплы активности).
pub fn waits_from_profile(rows: Vec<(String, Option<String>, i64)>) -> Option<QueryWaits> {
    let total: u64 = rows.iter().map(|r| r.2.max(0) as u64).sum();
    if total == 0 {
        return None;
    }
    let mut items: Vec<WaitItem> = rows
        .into_iter()
        .filter(|r| r.2 > 0)
        .map(|(event_type, event, count)| WaitItem {
            event_type,
            event,
            count: count as u64,
            share_pct: count as f64 * 100.0 / total as f64,
        })
        .collect();
    items.sort_by(by_samples);
    Some(QueryWaits {
        source: WaitsSource::PgWaitSampling,
        since_ms: 0,
        coverage_s: None,
        total,
        items,
    })
}

/// Расширение `pg_wait_sampling` в базе: схема, `profile_queries` (NULL — библиотека не загружена: параметр не
/// определён), `shared_preload_libraries` (NULL — не видна роли), право читать профиль и колонка `queryid` у профиля
/// (её нет в 1.0).
const WAIT_SAMPLING_SQL: &str = marked_sql!(
    "SELECT n.nspname::text, current_setting('pg_wait_sampling.profile_queries', true), \
    (SELECT setting FROM pg_settings WHERE name = 'shared_preload_libraries'), \
    COALESCE(has_function_privilege(to_regprocedure(format('%I.pg_wait_sampling_get_profile()', n.nspname)), 'EXECUTE') \
      AND has_table_privilege(to_regclass(format('%I.pg_wait_sampling_profile', n.nspname)), 'SELECT'), false), \
    EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = to_regclass(format('%I.pg_wait_sampling_profile', n.nspname)) \
      AND a.attname = 'queryid' AND NOT a.attisdropped) \
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_wait_sampling'"
);

/// Что делать с профилем `pg_wait_sampling` в карточке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitSampling {
    /// Не трогать: расширения нет, библиотека не загружена, нет прав или `queryid`. Ожидания — из сэмплов, без пометки.
    Skip,
    /// `profile_queries` выключен: профиль есть, но без запросов — пометка `waitsProfile`, ожидания из сэмплов.
    Off,
    /// Читать профиль из схемы (уже в кавычках).
    Read(String),
}

/// Решение по строке `WAIT_SAMPLING_SQL`. `profile_queries` в 1.1.4+ — перечисление `none`/`top`/`all` (с синонимами
/// булевых значений), раньше — булев параметр: `on`/`off`.
pub fn wait_sampling_plan(
    schema: &str,
    profile_queries: Option<&str>,
    preload: Option<&str>,
    can_read: bool,
    has_queryid: bool,
) -> WaitSampling {
    if !can_read || !has_queryid || !preloaded(preload, "pg_wait_sampling") {
        return WaitSampling::Skip;
    }
    match profile_queries
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("none" | "off" | "no" | "false" | "0") => WaitSampling::Off,
        Some("top" | "all" | "on" | "yes" | "true" | "1") => {
            WaitSampling::Read(quote_ident(schema))
        }
        _ => WaitSampling::Skip,
    }
}

fn waits_profile_off() -> Unavailable {
    Unavailable {
        part: "waitsProfile".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.waitsProfileOff",
            "{{part}}: pg_wait_sampling.profile_queries is off, waits come from the Sessions sampler",
            part = "waitsProfile"
        ),
    }
}

/// Сколько карточка не читает профиль `pg_wait_sampling` узла после ошибки состояния сервера (расширение создано, но не
/// читается): каждая ошибка — строка ERROR в логе сервера, а карточка опрашивается каждые 5 с.
pub const WAIT_SAMPLING_RETRY_MS: i64 = 600_000;

/// Сколько живёт прочитанный профиль запроса. Профиль отдаёт процесс-сборщик расширения: на каждый запрос он целиком
/// пересылает свой хеш через `shm_mq` (`send_profile`), а фильтр по `queryid` применяется уже после — на pg-perf это
/// около 65 мс на 358 тыс. строк. Карточка опрашивается каждые 5 с, поэтому чтение профиля прорежено: в пределах TTL
/// повторный опрос карточки берёт готовый результат (дешёвый запрос к каталогу при этом выполняется).
pub const WAIT_PROFILE_TTL_MS: i64 = 30_000;

/// Узел открытого подключения: номер открытия (`generation`) отличает переподключение — после него прошлые ошибки и
/// результаты не действуют.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeKey {
    pub id: String,
    pub generation: u64,
    pub member: String,
}

/// Прочитанный профиль запроса: время чтения и результат (`None` — запроса в профиле нет).
type ReadProfile = (i64, Option<QueryWaits>);

/// Кеши профиля `pg_wait_sampling` карточек: ошибки чтения по узлу и прочитанные профили по узлу и запросу. Устаревшие
/// записи выбрасываются при каждой записи, записи закрытого подключения и ушедшего узла — по `CloseListener`.
#[derive(Debug, Default)]
pub struct WaitProfiles {
    failures: Mutex<HashMap<NodeKey, i64>>,
    results: Mutex<HashMap<(NodeKey, i64), ReadProfile>>,
}

impl WaitProfiles {
    /// Чтение профиля узла отложено: ошибка состояния сервера случилась меньше `WAIT_SAMPLING_RETRY_MS` назад.
    pub fn failed_recently(&self, node: &NodeKey, now_ms: i64) -> bool {
        self.failures
            .lock_safe()
            .get(node)
            .is_some_and(|at| now_ms - at < WAIT_SAMPLING_RETRY_MS)
    }

    pub fn remember_failure(&self, node: &NodeKey, now_ms: i64) {
        let mut m = self.failures.lock_safe();
        m.retain(|_, at| now_ms - *at < WAIT_SAMPLING_RETRY_MS);
        m.insert(node.clone(), now_ms);
    }

    /// Прочитанный меньше `WAIT_PROFILE_TTL_MS` назад профиль запроса (`Some(None)` — запроса в профиле не было).
    pub fn cached(&self, node: &NodeKey, query_id: i64, now_ms: i64) -> Option<Option<QueryWaits>> {
        self.results
            .lock_safe()
            .get(&(node.clone(), query_id))
            .filter(|(at, _)| now_ms - at < WAIT_PROFILE_TTL_MS)
            .map(|(_, w)| w.clone())
    }

    pub fn remember(&self, node: &NodeKey, query_id: i64, now_ms: i64, waits: Option<QueryWaits>) {
        let mut m = self.results.lock_safe();
        m.retain(|_, (at, _)| now_ms - *at < WAIT_PROFILE_TTL_MS);
        m.insert((node.clone(), query_id), (now_ms, waits));
    }

    #[cfg(test)]
    fn len(&self) -> (usize, usize) {
        (
            self.failures.lock_safe().len(),
            self.results.lock_safe().len(),
        )
    }
}

impl CloseListener for WaitProfiles {
    fn connection_closed(&self, connection_id: &str) {
        self.failures
            .lock_safe()
            .retain(|k, _| k.id != connection_id);
        self.results
            .lock_safe()
            .retain(|(k, _), _| k.id != connection_id);
    }

    fn member_removed(&self, connection_id: &str, member: &str) {
        let gone = |k: &NodeKey| k.id == connection_id && k.member == member;
        self.failures.lock_safe().retain(|k, _| !gone(k));
        self.results.lock_safe().retain(|(k, _), _| !gone(k));
    }
}

/// Ошибка чтения профиля — от состояния сервера, которое само не пройдёт: библиотека не загружена или сборщик не
/// запущен (`55…`, сообщения расширения `pg_wait_sampling …` с `XX000`), нет объекта или прав (`42…`), нет файла
/// библиотеки (`58P01`). Отмена (`57014`), тайм-аут и обрыв соединения — разовые: их не запоминаем.
pub fn server_state_error(code: Option<&str>, message: &str) -> bool {
    match code {
        Some(c) if c.starts_with("55") || c.starts_with("42") || c == "58P01" => true,
        Some("XX000") => {
            message.contains("pg_wait_sampling") || message.contains("shared_preload_libraries")
        }
        _ => false,
    }
}

/// Профиль ожиданий запроса: `Ok(None)` — читать нечего. Ошибка чтения не ломает карточку: ожидания берутся из
/// сэмплов, а при ошибке состояния сервера (`server_state_error`) узел на `WAIT_SAMPLING_RETRY_MS` запоминается.
/// Обрыв соединения — ошибка карточки, как у остальных её запросов. Инструкции — вне транзакции: ошибка не обрывает
/// следующие запросы.
async fn read_waits_profile(
    c: &Client,
    query_id: i64,
    node: &NodeKey,
    cache: &WaitProfiles,
    unavailable: &mut Vec<Unavailable>,
) -> AppResult<Option<QueryWaits>> {
    let Some(r) = c
        .query_opt(WAIT_SAMPLING_SQL, &[])
        .await
        .map_err(|e| from_pg(&e))?
    else {
        return Ok(None);
    };
    let plan = wait_sampling_plan(
        &r.get::<_, String>(0),
        r.get::<_, Option<String>>(1).as_deref(),
        r.get::<_, Option<String>>(2).as_deref(),
        r.get::<_, Option<bool>>(3).unwrap_or(false),
        r.get::<_, Option<bool>>(4).unwrap_or(false),
    );
    let schema = match plan {
        WaitSampling::Skip => return Ok(None),
        WaitSampling::Off => {
            unavailable.push(waits_profile_off());
            return Ok(None);
        }
        WaitSampling::Read(schema) => schema,
    };
    let now = crate::topology::model::now_ms();
    if let Some(waits) = cache.cached(node, query_id, now) {
        return Ok(waits);
    }
    if cache.failed_recently(node, now) {
        return Ok(None);
    }
    match c.query(&waits_sql(&schema), &[&query_id]).await {
        Ok(rows) => {
            let waits = waits_from_profile(
                rows.iter()
                    .map(|r| (r.get(0), r.get(1), r.get(2)))
                    .collect(),
            );
            cache.remember(node, query_id, now, waits.clone());
            Ok(waits)
        }
        Err(e) if e.is_closed() => Err(from_pg(&e)),
        Err(e) => {
            tracing::debug!(error = %e, "pg_wait_sampling profile read failed; waits from the sampler");
            let db = e.as_db_error();
            if server_state_error(db.map(|d| d.code().code()), db.map_or("", |d| d.message())) {
                cache.remember_failure(node, now);
            }
            Ok(None)
        }
    }
}

/// Начало ожиданий из сэмплов: час назад или первый тик буфера, если сэмплер работает меньше часа.
pub fn sampler_since(now_ms: i64, oldest_tick: Option<i64>) -> i64 {
    let start = now_ms - LOOKBACK_MS;
    oldest_tick.map_or(start, |t| t.max(start))
}

/// Ожидания карточки: профиль `pg_wait_sampling`, если в нём есть запрос, иначе сэмплы активности.
pub fn pick_waits(
    profile: Option<QueryWaits>,
    sampler: impl FnOnce() -> Option<QueryWaits>,
) -> Option<QueryWaits> {
    profile.or_else(sampler)
}

/// Выполняющиеся экземпляры (14+): `$1` — `query_id`, `$2` — база. Перечисляются только активные ведущие бэкенды
/// (`state = 'active'`, `leader_pid IS NULL`), как и у сэмплера ожиданий: `query_id` в `pg_stat_activity` сохраняется
/// и после завершения инструкции (простаивающая сессия не выполняется), а параллельные воркеры несут `query_id`
/// лидера (дубликаты). Признак обрезки: сервер режет текст до `track_activity_query_size - 1` байт по границе
/// символа (в UTF-8 — не более 3 байт недобора), а колонка `query` дополнительно срезана `left(.., QUERY_LIMIT)`;
/// предел берётся из `pg_settings` (байты), а не из `current_setting` (строка с единицей, например `1kB`).
pub fn running_sql() -> String {
    format!(
        "{SQL_MARKER}SELECT a.pid, {BACKEND_START_US}, a.state, (extract(epoch FROM a.query_start) * 1000)::bigint, \
         extract(epoch FROM now() - a.query_start)::float8, a.wait_event_type, a.wait_event, a.usename::text, \
         a.application_name, host(a.client_addr), left(a.query, {QUERY_LIMIT}), \
         (octet_length(a.query) >= (SELECT setting::int FROM pg_settings WHERE name = 'track_activity_query_size') - 4 \
          OR length(a.query) > {QUERY_LIMIT}) \
         FROM pg_stat_activity a \
         WHERE a.query_id = $1 AND a.datname = $2 AND a.state = 'active' AND a.leader_pid IS NULL \
           AND a.pid <> pg_backend_pid() ORDER BY a.query_start"
    )
}

/// Текст запроса с ключом строки, у которой он прочитан: по ключу текст кладётся в кеш узла. Строки с текстом — первыми:
/// NULL в ответе значит, что текста нет ни у одной строки запроса в базе.
fn text_sql(ns: &str, caps: PgssCaps) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    format!(
        "{SQL_MARKER}SELECT s.dbid, s.userid, {toplevel}, left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} \
         FROM {ns}.pg_stat_statements(true) s JOIN pg_database d ON d.oid = s.dbid \
         WHERE s.queryid = $1 AND d.datname = $2 ORDER BY s.query IS NULL LIMIT 1"
    )
}

/// Откуда карточка берёт текст. `pg_stat_statements(true)` читает весь файл текстов, а карточка опрашивается каждые 5 с
/// (спец. §7.3): с сервера текст читается только при промахе кеша узла.
#[derive(Debug, Clone, PartialEq)]
pub enum TextSource {
    /// Из кеша текстов узла.
    Cached(String, bool),
    /// Не читать: запроса нет в pgss (строк нет) или уже известно, что текста у него нет (NULL).
    Skip,
    /// Промах кеша — чтение с сервера.
    Read,
}

/// Решение о тексте по ключам строк запроса: кеш (`cached`), отметки «текста нет» (`missing`).
pub fn text_source(
    keys: &[QueryKey],
    cached: impl Fn(&QueryKey) -> Option<(Arc<str>, bool)>,
    missing: impl Fn(&QueryKey) -> bool,
) -> TextSource {
    if keys.is_empty() {
        return TextSource::Skip;
    }
    if let Some((text, truncated)) = text_from_cache(keys, cached) {
        return TextSource::Cached(text, truncated);
    }
    if keys.iter().all(missing) {
        return TextSource::Skip;
    }
    TextSource::Read
}

/// Текст карточки из кеша текстов узла (буфер сэмплера pgss, спец. §4.6): первый ключ запроса, чей текст там уже есть.
/// `None` — промах.
pub fn text_from_cache(
    keys: &[QueryKey],
    lookup: impl Fn(&QueryKey) -> Option<(Arc<str>, bool)>,
) -> Option<(String, bool)> {
    keys.iter()
        .find_map(lookup)
        .map(|(text, truncated)| (text.to_string(), truncated))
}

fn totals_sql(ns: &str) -> String {
    format!(
        "{SQL_MARKER}SELECT COALESCE(sum(calls), 0)::bigint, COALESCE(sum(total_exec_time), 0)::float8, COALESCE(sum(rows), 0)::bigint \
         FROM {ns}.pg_stat_statements(false) WHERE queryid IS NOT NULL"
    )
}

/// Что читается с сервера за одно соединение.
struct Fetched {
    head: Head,
    pgss: Option<PgssInstalled>,
    rows: Vec<Cumulative>,
    text: Option<(String, bool)>,
    totals: Totals,
    info: PgssInfo,
    running: Vec<RunningInstance>,
    waits_profile: Option<QueryWaits>,
    unavailable: Vec<Unavailable>,
}

/// `cache` — буфер сэмплера pgss узла (если сэмплер на узле запускался): из него берётся текст, в него же кладётся
/// прочитанный с сервера. Блокировка короткая и никогда не держится через `await`.
async fn fetch(
    c: &Client,
    database: &str,
    query_id: i64,
    cache: Option<&Mutex<StatementsBuffer>>,
    node: &NodeKey,
    profiles: &WaitProfiles,
) -> AppResult<Fetched> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let h = head(c).await?;
    let caps = Caps::new(h.version_num);
    let mut unavailable = Vec::new();
    let (pgss, rows, text, t, info) = match detect(c).await? {
        Detect::Installed(pgss) => {
            let raw = c
                .query(
                    &statements_sql(
                        &pgss.schema,
                        pgss.caps,
                        pgss.kcache.as_ref(),
                        " WHERE s.queryid = $1 AND d.datname = $2",
                    ),
                    &[&query_id, &database],
                )
                .await
                .map_err(pg)?;
            let rows: Vec<Cumulative> = raw.iter().filter_map(row_from).collect();
            let keys: Vec<QueryKey> = rows.iter().map(|r| r.key).collect();
            let source = match cache {
                Some(b) => {
                    let b = b.lock_safe();
                    text_source(&keys, |k| b.text(k), |k| b.text_missing(k))
                }
                None => text_source(&keys, |_| None, |_| false),
            };
            let text = match source {
                TextSource::Cached(text, truncated) => Some((text, truncated)),
                TextSource::Skip => None,
                TextSource::Read => {
                    let read = c
                        .query_opt(&text_sql(&pgss.schema, pgss.caps), &[&query_id, &database])
                        .await
                        .map_err(pg)?
                        .map(|r| {
                            let key = QueryKey {
                                query_id,
                                dbid: r.get(0),
                                userid: r.get(1),
                                toplevel: r.get::<_, Option<bool>>(2).unwrap_or(true),
                            };
                            let text = r.get::<_, Option<String>>(3);
                            (key, text, r.get::<_, Option<bool>>(4).unwrap_or(false))
                        });
                    // Прочитанное — в кеш узла: следующий опрос карточки и отчёты возьмут текст оттуда. Строка без текста
                    // (NULL у всех строк запроса) — отметка «текста нет» для ключей, иначе каждый опрос читал бы файл
                    // текстов. Строки не нашлось (вытеснена между запросами) — не запоминается ничего.
                    if let Some(b) = cache {
                        match &read {
                            Some((key, Some(text), truncated)) => {
                                b.lock_safe()
                                    .remember_texts([(*key, text.clone(), *truncated)])
                            }
                            Some((_, None, _)) => b.lock_safe().remember_no_text(&keys),
                            None => {}
                        }
                    }
                    read.and_then(|(_, text, truncated)| text.map(|t| (t, truncated)))
                }
            };
            let tr = c
                .query_one(&totals_sql(&pgss.schema), &[])
                .await
                .map_err(pg)?;
            let t = Totals {
                calls: tr.get(0),
                total_exec_ms: tr.get(1),
                rows: tr.get(2),
            };
            let i = c
                .query_one(&info_sql(&pgss.schema, pgss.caps), &[])
                .await
                .map_err(pg)?;
            let info = PgssInfo {
                stats_reset_ms: i.get(0),
                dealloc: i.get(1),
                max: i.get(2),
                track_planning: i.get::<_, Option<String>>(3).as_deref() == Some("on"),
            };
            (Some(pgss), rows, text, t, info)
        }
        Detect::TooOld(v) => {
            unavailable.push(too_old(&v));
            (
                None,
                Vec::new(),
                None,
                Totals::default(),
                PgssInfo::default(),
            )
        }
        Detect::Missing => {
            unavailable.push(missing());
            (
                None,
                Vec::new(),
                None,
                Totals::default(),
                PgssInfo::default(),
            )
        }
    };
    let running = if caps.at_least(V14) {
        c.query(&running_sql(), &[&query_id, &database])
            .await
            .map_err(pg)?
            .iter()
            .map(|r| RunningInstance {
                pid: r.get(0),
                backend_start_us: r.get(1),
                state: r.get(2),
                started_ms: r.get(3),
                duration_s: r.get(4),
                wait_event_type: r.get(5),
                wait_event: r.get(6),
                usename: r.get(7),
                application_name: r.get(8),
                client_addr: r.get(9),
                query: r.get::<_, Option<String>>(10).unwrap_or_default(),
                truncated: r.get::<_, Option<bool>>(11).unwrap_or(false),
            })
            .collect()
    } else {
        unavailable.extend(caps.missing(V14, "running"));
        Vec::new()
    };
    let waits_profile = read_waits_profile(c, query_id, node, profiles, &mut unavailable).await?;
    Ok(Fetched {
        head: h,
        pgss,
        rows,
        text,
        totals: t,
        info,
        running,
        waits_profile,
        unavailable,
    })
}

pub async fn detail(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    query_id: i64,
) -> AppResult<QueryDetail> {
    let target = st.runtimes.diag_target(id, Some(member), false)?;
    // Замыкание отдаёт будущее на любое время жизни соединения, поэтому база и буфер уходят в него владеемыми.
    let database = database.to_string();
    let cache = st.statements.buffer(id, member);
    let node = NodeKey {
        id: id.to_string(),
        generation: target.generation,
        member: member.to_string(),
    };
    let profiles = st.wait_profiles.clone();
    let f = on_member(st, id, &target, |c| {
        Box::pin(
            async move { fetch(c, &database, query_id, cache.as_deref(), &node, &profiles).await },
        )
    })
    .await?;
    let mut d = QueryDetail::empty(&f.head);
    d.unavailable = f.unavailable;
    d.running = f.running;
    d.totals = f.totals;
    d.pgss_version = f.pgss.as_ref().map(|p| p.version.clone());
    d.track_planning = f.info.track_planning;
    d.kcache = f.pgss.as_ref().is_some_and(|p| p.kcache.is_some());
    if let Some((text, truncated)) = f.text {
        d.text = Some(text);
        d.text_truncated = truncated;
    }
    if let Some((c, minmax)) = aggregate(&f.rows) {
        let key = f.rows[0].key;
        let s = stat(
            key,
            f.rows[0].datname.clone(),
            None,
            &c,
            Some(minmax),
            &f.totals,
        );
        let state = PgssState {
            track_planning: f.info.track_planning,
            dealloc: f.info.dealloc,
            dealloc_grew: false,
            max: f.info.max,
        };
        d.findings = rules::evaluate(
            Some(member),
            std::slice::from_ref(&s),
            &f.totals,
            &state,
            Mode::SinceReset,
        )
        .into_iter()
        .filter(|x| x.id != "queries_incomplete")
        .collect();
        d.since_reset = Some(s);
        d.by_user = f
            .rows
            .iter()
            .map(|r| ByUser {
                rolname: r.rolname.clone(),
                toplevel: r.key.toplevel,
                stat: stat(
                    r.key,
                    r.datname.clone(),
                    r.rolname.clone(),
                    &r.counters,
                    Some((r.min_exec_ms, r.max_exec_ms, r.stddev_exec_ms)),
                    &f.totals,
                ),
            })
            .collect();
    }
    let keys: Vec<QueryKey> = f.rows.iter().map(|r| r.key).collect();
    let now_ms = crate::topology::model::now_ms();
    // Буферы сэмплеров читаются под короткой блокировкой, без `await`: наружу копируется только нужное.
    if !keys.is_empty()
        && let Some(b) = st.statements.buffer(id, member)
    {
        let b = b.lock_safe();
        d.windows = WINDOWS_S
            .iter()
            .map(|w| WindowStat {
                window_s: *w,
                stat: b.window_for(&keys, *w).map(|kw| window_stat(&kw, keys[0])),
            })
            .collect();
        d.history = b.history(&keys, now_ms - LOOKBACK_MS);
        d.gaps = b
            .gaps(now_ms - LOOKBACK_MS)
            .into_iter()
            .map(|(from_ms, to_ms)| Gap { from_ms, to_ms })
            .collect();
    }
    d.waits = pick_waits(f.waits_profile, || {
        let b = st.samplers.buffer(id, member)?;
        let b = b.lock_safe();
        let since = sampler_since(now_ms, b.oldest_tick());
        waits_of(&b.samples(since), query_id, since).map(|w| QueryWaits {
            coverage_s: Some((now_ms - since) / 1000),
            ..w
        })
    });
    Ok(d)
}

/// Идентификатор запроса приходит строкой: 64-битный хеш не помещается в число JS.
pub fn parse_query_id(s: &str) -> AppResult<i64> {
    s.trim().parse().map_err(|_| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.queries.badQueryId",
            "Query id must be a 64-bit integer"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::ash::Sample;
    use pg_advisor::queries::Counters;

    fn key(id: i64, userid: u32, toplevel: bool) -> QueryKey {
        QueryKey {
            query_id: id,
            dbid: 1,
            userid,
            toplevel,
        }
    }

    fn row(
        userid: u32,
        toplevel: bool,
        calls: i64,
        ms: f64,
        min: f64,
        max: f64,
        sd: f64,
    ) -> Cumulative {
        Cumulative {
            key: key(7, userid, toplevel),
            datname: Some("perf".into()),
            rolname: Some(format!("r{userid}")),
            counters: Counters {
                calls,
                total_exec_ms: ms,
                ..Counters::default()
            },
            min_exec_ms: min,
            max_exec_ms: max,
            stddev_exec_ms: sd,
        }
    }

    #[test]
    fn waits_sql_groups_the_profile_by_event() {
        let sql = waits_sql("public");
        assert!(
            sql.contains("FROM public.pg_wait_sampling_profile")
                && sql.contains("WHERE queryid = $1")
                && sql.contains("GROUP BY 1, 2"),
            "{sql}"
        );
        assert!(crate::product::is_marked_sql(&sql), "{sql}");
        assert!(sql.contains("COALESCE(event_type, 'CPU')"), "{sql}");
        assert!(WAIT_SAMPLING_SQL.starts_with("/* glafira */"));
    }

    #[test]
    fn profile_rows_become_waits() {
        let w = waits_from_profile(vec![
            ("IO".into(), Some("DataFileRead".into()), 70),
            ("Client".into(), Some("ClientRead".into()), 30),
        ]);
        let w = w.unwrap();
        assert_eq!(
            (w.source, w.since_ms, w.total),
            (WaitsSource::PgWaitSampling, 0, 100)
        );
        assert_eq!(
            (w.items[0].event_type.as_str(), w.items[0].share_pct),
            ("IO", 70.0)
        );
        assert!(waits_from_profile(vec![]).is_none());
        assert!(waits_from_profile(vec![("IO".into(), None, 0)]).is_none());
        // Порядок — по числу сэмплов, независимо от порядка строк.
        let w = waits_from_profile(vec![("CPU".into(), None, 1), ("IO".into(), None, 3)]).unwrap();
        assert_eq!(w.items[0].event_type, "IO");
    }

    #[test]
    fn wait_sampling_plan_follows_the_setting_and_the_catalog() {
        let read = WaitSampling::Read("public".into());
        let plan = |pq: Option<&str>| wait_sampling_plan("public", pq, None, true, true);
        // 1.1.4+ — перечисление, раньше — булев параметр; синонимы булевых значений тоже.
        for v in ["top", "all", "on", "true", "yes", "1", "TOP"] {
            assert_eq!(plan(Some(v)), read, "{v}");
        }
        for v in ["none", "off", "false", "no", "0"] {
            assert_eq!(plan(Some(v)), WaitSampling::Off, "{v}");
        }
        // Параметр не определён — библиотека не загружена.
        assert_eq!(plan(None), WaitSampling::Skip);
        assert_eq!(plan(Some("weird")), WaitSampling::Skip);
        assert_eq!(
            wait_sampling_plan(
                "public",
                Some("top"),
                Some("pg_stat_statements"),
                true,
                true
            ),
            WaitSampling::Skip
        );
        assert_eq!(
            wait_sampling_plan(
                "public",
                Some("top"),
                Some("pg_stat_statements, \"pg_wait_sampling\""),
                true,
                true
            ),
            read
        );
        assert_eq!(
            wait_sampling_plan("public", Some("top"), None, false, true),
            WaitSampling::Skip
        );
        assert_eq!(
            wait_sampling_plan("public", Some("top"), None, true, false),
            WaitSampling::Skip
        );
        assert_eq!(
            wait_sampling_plan("w s", Some("all"), None, true, true),
            WaitSampling::Read("\"w s\"".into())
        );
        // Выключенный профиль при невидимой и не загруженной библиотеке не возникает: параметр тогда не определён.
        assert_eq!(waits_profile_off().part, "waitsProfile");
    }

    fn node(id: &str, generation: u64, member: &str) -> NodeKey {
        NodeKey {
            id: id.into(),
            generation,
            member: member.into(),
        }
    }

    fn profile_waits(count: i64) -> Option<QueryWaits> {
        waits_from_profile(vec![("IO".into(), Some("DataFileRead".into()), count)])
    }

    #[test]
    fn wait_profiles_hold_off_failed_nodes_for_a_while() {
        let p = WaitProfiles::default();
        let a = node("c", 1, "pg-1");
        assert!(!p.failed_recently(&a, 1_000));
        p.remember_failure(&a, 1_000);
        assert!(p.failed_recently(&a, 1_000 + WAIT_SAMPLING_RETRY_MS - 1));
        assert!(
            !p.failed_recently(&node("c", 1, "pg-2"), 1_000),
            "другой узел не затронут"
        );
        assert!(
            !p.failed_recently(&node("c", 2, "pg-1"), 1_000),
            "переподключение — новый номер открытия"
        );
        assert!(!p.failed_recently(&a, 1_000 + WAIT_SAMPLING_RETRY_MS));
        // Устаревшие записи выбрасываются при следующей записи.
        p.remember_failure(&node("d", 1, "pg-1"), 1_000 + WAIT_SAMPLING_RETRY_MS);
        assert_eq!(p.len().0, 1);
        // Закрытие подключения и уход узла стирают его записи.
        p.remember_failure(&a, 5_000);
        p.remember(&a, 7, 5_000, profile_waits(3));
        p.member_removed("c", "pg-1");
        assert!(!p.failed_recently(&a, 5_001) && p.cached(&a, 7, 5_001).is_none());
        p.remember_failure(&a, 5_000);
        p.remember(&a, 7, 5_000, profile_waits(3));
        p.connection_closed("c");
        assert!(!p.failed_recently(&a, 5_001) && p.cached(&a, 7, 5_001).is_none());
        assert!(p.failed_recently(
            &node("d", 1, "pg-1"),
            5_001 + WAIT_SAMPLING_RETRY_MS - 10_000
        ));
    }

    #[test]
    fn wait_profiles_reuse_a_read_within_the_ttl() {
        let p = WaitProfiles::default();
        let a = node("c", 1, "pg-1");
        assert_eq!(p.cached(&a, 7, 0), None);
        p.remember(&a, 7, 1_000, profile_waits(5));
        p.remember(&a, 8, 1_000, None);
        assert_eq!(
            p.cached(&a, 7, 1_000 + WAIT_PROFILE_TTL_MS - 1),
            Some(profile_waits(5))
        );
        assert_eq!(
            p.cached(&a, 8, 1_000),
            Some(None),
            "запроса в профиле нет — тоже результат"
        );
        assert_eq!(p.cached(&a, 9, 1_000), None, "другой запрос — промах");
        assert_eq!(
            p.cached(&node("c", 2, "pg-1"), 7, 1_000),
            None,
            "после переподключения — промах"
        );
        assert_eq!(
            p.cached(&a, 7, 1_000 + WAIT_PROFILE_TTL_MS),
            None,
            "TTL истёк"
        );
        // Запись выбрасывает устаревшие результаты: в кеше только свежие.
        p.remember(&a, 10, 1_000 + WAIT_PROFILE_TTL_MS, None);
        assert_eq!(p.len().1, 1);
    }

    #[test]
    fn only_server_state_errors_are_remembered() {
        assert!(server_state_error(
            Some("55000"),
            "must be loaded via shared_preload_libraries"
        ));
        assert!(server_state_error(
            Some("42501"),
            "permission denied for function"
        ));
        assert!(server_state_error(Some("42P01"), "relation does not exist"));
        assert!(server_state_error(Some("58P01"), "could not access file"));
        assert!(server_state_error(
            Some("XX000"),
            "pg_wait_sampling shared memory wasn't initialized yet"
        ));
        assert!(server_state_error(
            Some("XX000"),
            "pg_wait_sampling collector wasn't started"
        ));
        assert!(!server_state_error(
            Some("XX000"),
            "unexpected chunk number"
        ));
        assert!(!server_state_error(
            Some("57014"),
            "canceling statement due to statement timeout"
        ));
        assert!(!server_state_error(Some("57P01"), "terminating connection"));
        assert!(!server_state_error(Some("53200"), "out of memory"));
        assert!(!server_state_error(None, "connection closed"));
    }

    #[test]
    fn detail_prefers_profile_waits_over_sampler_waits() {
        let sampler = || {
            Some(QueryWaits {
                source: WaitsSource::Sampler,
                since_ms: 1,
                coverage_s: Some(60),
                total: 1,
                items: Vec::new(),
            })
        };
        let w = pick_waits(profile_waits(4), || panic!("сэмплы не нужны"));
        assert_eq!(w.map(|w| w.source), Some(WaitsSource::PgWaitSampling));
        assert_eq!(
            pick_waits(None, sampler).map(|w| w.source),
            Some(WaitsSource::Sampler)
        );
        assert_eq!(pick_waits(None, || None), None);
    }

    #[test]
    fn sampler_waits_cover_at_most_the_lookback() {
        let now = 10 * LOOKBACK_MS;
        assert_eq!(sampler_since(now, None), now - LOOKBACK_MS);
        assert_eq!(
            sampler_since(now, Some(now - 600_000)),
            now - 600_000,
            "сэмплер работает 10 мин"
        );
        assert_eq!(sampler_since(now, Some(0)), now - LOOKBACK_MS);
    }

    #[test]
    fn aggregate_sums_counters_and_merges_minmax() {
        assert!(aggregate(&[]).is_none());
        let (c, (min, max, sd)) = aggregate(&[
            row(1, true, 10, 100.0, 1.0, 50.0, 5.0),
            row(2, false, 30, 60.0, 0.5, 20.0, 9.0),
        ])
        .unwrap();
        assert_eq!((c.calls, c.total_exec_ms), (40, 160.0));
        assert_eq!((min, max), (0.5, 50.0));
        assert_eq!(sd, 9.0, "stddev — у строки с наибольшим числом вызовов");
    }

    #[test]
    fn window_stat_shares_the_whole_window() {
        let w = KeysWindow {
            sum: Counters {
                calls: 4,
                total_exec_ms: 40.0,
                ..Counters::default()
            },
            totals: Totals {
                calls: 104,
                total_exec_ms: 1040.0,
                rows: 0,
            },
        };
        let s = window_stat(&w, key(7, 1, true));
        assert_eq!((s.calls, s.total_exec_ms, s.mean_exec_ms), (4, 40.0, 10.0));
        assert_eq!(s.stddev_exec_ms, None);
        assert!(
            (s.share_pct - 40.0 / 1040.0 * 100.0).abs() < 1e-9,
            "доля — от всего окна"
        );
    }

    #[test]
    fn text_is_read_only_on_a_cache_miss_of_a_present_query() {
        let keys = [key(7, 1, true), key(7, 2, false)];
        let never = |_: &QueryKey| -> Option<(Arc<str>, bool)> { panic!("кеш не нужен") };
        assert_eq!(
            text_source(&[], never, |_| panic!("отметки не нужны")),
            TextSource::Skip,
            "строк нет — запроса нет в pgss, файл текстов не читается"
        );
        assert_eq!(
            text_source(&keys, |_| None, |_| false),
            TextSource::Read,
            "промах — чтение с сервера"
        );
        assert_eq!(
            text_source(
                &keys,
                |k| (k.userid == 2).then(|| (Arc::<str>::from("SELECT $1"), false)),
                |_| true
            ),
            TextSource::Cached("SELECT $1".into(), false),
            "текст из кеша важнее отметок"
        );
        assert_eq!(
            text_source(&keys, |_| None, |_| true),
            TextSource::Skip,
            "у всех ключей отмечено «текста нет»"
        );
        assert_eq!(
            text_source(&keys, |_| None, |k| k.userid == 1),
            TextSource::Read,
            "отмечен не каждый ключ — чтение"
        );
    }

    #[test]
    fn waits_group_samples_of_the_query() {
        let s = |at_ms: i64,
                 qid: Option<i64>,
                 wt: Option<&'static str>,
                 we: Option<&'static str>| Sample {
            at_ms,
            pid: 1,
            leader_pid: None,
            wait_type: wt,
            wait_event: we,
            query_id: qid,
            query: "",
            application: "",
            user: "",
            database: "",
            client: "",
        };
        let samples = [
            s(10, Some(7), Some("IO"), Some("DataFileRead")),
            s(11, Some(7), Some("IO"), Some("DataFileRead")),
            s(12, Some(7), None, None),
            s(13, Some(8), Some("Lock"), Some("relation")),
            s(5, Some(7), Some("Lock"), Some("relation")),
        ];
        let w = waits_of(&samples, 7, 10).unwrap();
        assert_eq!(
            (w.total, w.since_ms, w.source),
            (3, 10, WaitsSource::Sampler)
        );
        assert_eq!(w.items[0].event_type, "IO");
        assert_eq!((w.items[0].count, w.items[0].share_pct), (2, 200.0 / 3.0));
        assert_eq!(
            (
                w.items[1].event_type.as_str(),
                w.items[1].event.as_deref(),
                w.items[1].count
            ),
            ("CPU", None, 1)
        );
        assert!(
            waits_of(&samples, 99, 0).is_none(),
            "сэмплов запроса нет — None"
        );
    }

    #[test]
    fn running_sql_filters_by_query_id_and_database() {
        let sql = running_sql();
        assert!(
            sql.contains("a.query_id = $1")
                && sql.contains("a.datname = $2")
                && sql.contains("pg_backend_pid()"),
            "{sql}"
        );
        assert!(sql.contains("track_activity_query_size"));
        assert!(
            sql.contains("a.state = 'active'") && sql.contains("a.leader_pid IS NULL"),
            "только активные ведущие бэкенды: {sql}"
        );
        assert!(
            sql.contains(&format!("OR length(a.query) > {QUERY_LIMIT})")),
            "truncated учитывает срез QUERY_LIMIT: {sql}"
        );
    }

    #[test]
    fn text_comes_from_the_cache_by_the_first_cached_key() {
        let keys = [key(7, 1, true), key(7, 2, false)];
        assert_eq!(
            text_from_cache(&keys, |_| None),
            None,
            "промах — чтение с сервера"
        );
        assert_eq!(
            text_from_cache(&[], |_| Some((Arc::from("x"), false))),
            None
        );
        let seen = std::cell::RefCell::new(Vec::new());
        let hit = text_from_cache(&keys, |k| {
            seen.borrow_mut().push(k.userid);
            (k.userid == 2).then(|| (Arc::<str>::from("SELECT $1"), true))
        });
        assert_eq!(hit, Some(("SELECT $1".to_string(), true)));
        assert_eq!(*seen.borrow(), [1, 2], "ключи — по порядку строк");
    }

    #[test]
    fn text_sql_returns_the_key_of_the_text() {
        let new = text_sql("public", PgssCaps::from_version("1.10").unwrap());
        assert!(
            new.starts_with("/* glafira */ SELECT s.dbid, s.userid, s.toplevel,")
                && new.contains("pg_stat_statements(true)")
                && new.contains("ORDER BY s.query IS NULL LIMIT 1"),
            "{new}"
        );
        let old = text_sql("public", PgssCaps::from_version("1.8").unwrap());
        assert!(
            old.starts_with("/* glafira */ SELECT s.dbid, s.userid, true,"),
            "toplevel — с 1.9: {old}"
        );
    }

    #[test]
    fn detail_json_shape() {
        let d = QueryDetail::unavailable_pgss(170_000, false);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(
            v["windows"],
            serde_json::json!([{"windowS": 300, "stat": null}, {"windowS": 900, "stat": null}, {"windowS": 3600, "stat": null}])
        );
        assert_eq!(v["unavailable"][0]["part"], "statements");
        assert_eq!(
            (&v["trackPlanning"], &v["serverVersionNum"]),
            (&false.into(), &170_000.into())
        );
        assert_eq!(
            (&v["text"], &v["sinceReset"], &v["waits"]),
            (
                &serde_json::Value::Null,
                &serde_json::Value::Null,
                &serde_json::Value::Null
            )
        );
    }
}
