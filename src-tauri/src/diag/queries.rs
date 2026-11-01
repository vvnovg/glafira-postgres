//! Инструмент «Запросы» (спец. P3 §4): поиск `pg_stat_statements`, SQL по версии расширения, режим «с момента
//! сброса» и ленивые тексты. Режим «за окно» — из буфера сэмплера pgss (`sampler::statements`).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use pg_advisor::pgss::PgssCaps;
use pg_advisor::queries::rules::{self, Mode, PgssState, n_plus_one_share};
use pg_advisor::queries::{
    Counters, Cumulative, QueryKey, QuerySort, QueryStat, Totals, stat, top, totals,
};
use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, Row};

use crate::catalog::ddl::quote_ident;
use crate::conn::errors::from_pg;
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::{
    DiagData, DiagReport, Head, Tool, Unavailable, WINDOWS_S, head, make_report, on_member,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::product::SQL_MARKER;
use crate::product::is_marked_sql;
use crate::sampler::statements::StatementsBuffer;
use crate::state::AppState;
use crate::sync::MutexExt;
use crate::topology::probe::is_unmarked_poll_sql;
use crate::topology::runtime::DiagTarget;

/// Пределы строк отчёта (спец. §4.2).
pub const LIMITS: [usize; 3] = [50, 100, 200];

/// Установленное расширение: схема (для вызова функции и представления), версия и её возможности.
#[derive(Debug, Clone, PartialEq)]
pub struct PgssInstalled {
    pub schema: String,
    pub version: String,
    pub caps: PgssCaps,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Detect {
    Installed(PgssInstalled),
    Missing,
    /// Версия старше 1.8: нет `*_exec_time`, планирования и WAL.
    TooOld(String),
}

const DETECT_SQL: &str = marked_sql!(
    "SELECT e.extversion::text, n.nspname::text FROM pg_extension e \
    JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_stat_statements'"
);

pub async fn detect(c: &Client) -> AppResult<Detect> {
    let Some(r) = c
        .query_opt(DETECT_SQL, &[])
        .await
        .map_err(|e| from_pg(&e))?
    else {
        return Ok(Detect::Missing);
    };
    let (version, schema): (String, String) = (r.get(0), r.get(1));
    Ok(match PgssCaps::from_version(&version) {
        Some(caps) => Detect::Installed(PgssInstalled {
            schema: quote_ident(&schema),
            version,
            caps,
        }),
        None => Detect::TooOld(version),
    })
}

/// Строки pgss без текстов (`showtext := false`): файл текстов не читается. `where_clause` — пусто или
/// ` WHERE s.queryid = $1 AND d.datname = $2` (карточка). Порядок колонок — `row_from`.
pub fn statements_sql(ns: &str, caps: PgssCaps, where_clause: &str) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    let (read_ms, write_ms) = if caps.shared_io_names {
        ("s.shared_blk_read_time", "s.shared_blk_write_time")
    } else {
        ("s.blk_read_time", "s.blk_write_time")
    };
    let temp_io = if caps.temp_io_time {
        "s.temp_blk_read_time + s.temp_blk_write_time"
    } else {
        "NULL::float8"
    };
    // `jit_deform_time` (1.11) входит в `jit_generation_time` и отдельно не прибавляется.
    let (jit_functions, jit_ms) = if caps.jit {
        (
            "s.jit_functions",
            "s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time",
        )
    } else {
        ("NULL::bigint", "NULL::float8")
    };
    let (par_planned, par_launched) = if caps.parallel {
        (
            "s.parallel_workers_to_launch",
            "s.parallel_workers_launched",
        )
    } else {
        ("NULL::bigint", "NULL::bigint")
    };
    format!(
        "{SQL_MARKER}SELECT s.userid, s.dbid, s.queryid, {toplevel}, d.datname::text, r.rolname::text, \
         s.calls, s.total_exec_time, s.min_exec_time, s.max_exec_time, s.mean_exec_time, s.stddev_exec_time, s.rows, \
         s.plans, s.total_plan_time, \
         s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
         s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
         s.temp_blks_read, s.temp_blks_written, {read_ms}, {write_ms}, {temp_io}, \
         s.wal_records, s.wal_fpi, s.wal_bytes::bigint, {jit_functions}, {jit_ms}, {par_planned}, {par_launched} \
         FROM {ns}.pg_stat_statements(false) s \
         LEFT JOIN pg_database d ON d.oid = s.dbid \
         LEFT JOIN pg_roles r ON r.oid = s.userid{where_clause}"
    )
}

/// `stats_reset` (мс эпохи) и `dealloc` из `pg_stat_statements_info` (1.9+), предел строк и `track_planning`.
pub fn info_sql(ns: &str, caps: PgssCaps) -> String {
    let info = if caps.info {
        format!(
            "(SELECT (extract(epoch FROM stats_reset) * 1000)::bigint FROM {ns}.pg_stat_statements_info), \
             (SELECT dealloc FROM {ns}.pg_stat_statements_info)"
        )
    } else {
        "NULL::bigint, NULL::bigint".to_string()
    };
    format!(
        "{SQL_MARKER}SELECT {info}, current_setting('pg_stat_statements.max')::bigint, \
         current_setting('pg_stat_statements.track_planning', true)"
    )
}

/// Тексты по ключам (`showtext := true`, `$1` — массив `queryid`): не длиннее `QUERY_LIMIT` и признак обрезки.
pub fn texts_sql(ns: &str, caps: PgssCaps) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    format!(
        "{SQL_MARKER}SELECT s.queryid, s.dbid, s.userid, {toplevel}, left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} \
         FROM {ns}.pg_stat_statements(true) s WHERE s.queryid = ANY($1)"
    )
}

/// Ключи строк, текст которых содержит подстроку `$1` без учёта регистра (`$1` — уже экранированный `like_escape`).
pub fn search_sql(ns: &str, caps: PgssCaps) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    format!(
        "{SQL_MARKER}SELECT s.queryid, s.dbid, s.userid, {toplevel} \
         FROM {ns}.pg_stat_statements(true) s WHERE s.query ILIKE '%' || $1 || '%'"
    )
}

/// Подстрока для `LIKE` / `ILIKE` буквально: `%`, `_` и `\` (символ экранирования по умолчанию) экранируются.
pub fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Ключи строк pgss, текст которых содержит `search` (без учёта регистра).
pub async fn search_keys(
    c: &Client,
    pgss: &PgssInstalled,
    search: &str,
) -> AppResult<HashSet<QueryKey>> {
    let rows = c
        .query(
            &search_sql(&pgss.schema, pgss.caps),
            &[&like_escape(search)],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(QueryKey {
                query_id: r.get::<_, Option<i64>>(0)?,
                dbid: r.get(1),
                userid: r.get(2),
                toplevel: r.get::<_, Option<bool>>(3).unwrap_or(true),
            })
        })
        .collect())
}

/// Ключи по поиску с кешем узла: прошлый скан (`StatementsBuffer::search_hit`), если ключи pgss, прочитанные до
/// этого вызова (`keys`), все проверены и сброс тот же; иначе — скан `search_keys`, итог запоминается. Без буфера
/// сэмплера — всегда скан.
async fn search_keys_cached(
    c: &Client,
    pgss: &PgssInstalled,
    search: &str,
    info: &PgssInfo,
    keys: HashSet<QueryKey>,
    buffer: Option<&Arc<Mutex<StatementsBuffer>>>,
) -> AppResult<HashSet<QueryKey>> {
    let hit = buffer.and_then(|b| b.lock_safe().search_hit(search, info, keys.iter()));
    if let Some(hit) = hit {
        return Ok(hit);
    }
    let matched = search_keys(c, pgss, search).await?;
    if let Some(b) = buffer {
        b.lock_safe()
            .remember_search(search, info, keys, matched.clone());
    }
    Ok(matched)
}

/// Ключи набора, чей текст (`text`, кеш узла) содержит `search` без учёта регистра. В окне остаются дельты ключей,
/// ушедших из pgss: скан текстов их не находит, а кеш узла может помнить текст.
pub fn cached_matches(
    entries: &[Entry],
    search: &str,
    text: impl Fn(&QueryKey) -> Option<Arc<str>>,
) -> HashSet<QueryKey> {
    let needle = search.to_lowercase();
    entries
        .iter()
        .filter(|e| text(&e.key).is_some_and(|t| t.to_lowercase().contains(&needle)))
        .map(|e| e.key)
        .collect()
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PgssInfo {
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
    pub max: i64,
    pub track_planning: bool,
}

/// Чтение pgss: строки и состояние. `hidden` — были строки с `queryid = NULL` (чужие без `pg_read_all_stats`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<Cumulative>,
    pub info: PgssInfo,
    pub hidden: bool,
}

/// Строка `statements_sql`; `None` — `queryid` скрыт правами.
pub fn row_from(r: &Row) -> Option<Cumulative> {
    let query_id: i64 = r.get::<_, Option<i64>>(2)?;
    Some(Cumulative {
        key: QueryKey {
            query_id,
            dbid: r.get(1),
            userid: r.get(0),
            toplevel: r.get::<_, Option<bool>>(3).unwrap_or(true),
        },
        datname: r.get(4),
        rolname: r.get(5),
        counters: Counters {
            calls: r.get(6),
            total_exec_ms: r.get(7),
            rows: r.get(12),
            plans: r.get(13),
            total_plan_ms: r.get(14),
            shared_hit: r.get(15),
            shared_read: r.get(16),
            shared_dirtied: r.get(17),
            shared_written: r.get(18),
            local_hit: r.get(19),
            local_read: r.get(20),
            local_dirtied: r.get(21),
            local_written: r.get(22),
            temp_read: r.get(23),
            temp_written: r.get(24),
            io_read_ms: r.get(25),
            io_write_ms: r.get(26),
            temp_io_ms: r.get(27),
            wal_records: r.get(28),
            wal_fpi: r.get(29),
            wal_bytes: r.get(30),
            jit_functions: r.get(31),
            jit_ms: r.get(32),
            parallel_planned: r.get(33),
            parallel_launched: r.get(34),
        },
        min_exec_ms: r.get(8),
        max_exec_ms: r.get(9),
        stddev_exec_ms: r.get(11),
    })
}

pub async fn read_cumulative(c: &Client, pgss: &PgssInstalled) -> AppResult<Snapshot> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let raw = c
        .query(&statements_sql(&pgss.schema, pgss.caps, ""), &[])
        .await
        .map_err(pg)?;
    let mut hidden = false;
    let rows = raw
        .iter()
        .filter_map(|r| {
            let row = row_from(r);
            hidden |= row.is_none();
            row
        })
        .collect();
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
    Ok(Snapshot { rows, info, hidden })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueriesMode {
    SinceReset,
    Window {
        s: u32,
    },
    /// Между снимками — P3b; в P3a отклоняется `validate`.
    Between {
        from: String,
        to: String,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueriesParams {
    pub mode: QueriesMode,
    pub sort: QuerySort,
    pub limit: usize,
    pub database: Option<String>,
    #[serde(default = "default_true")]
    pub toplevel_only: bool,
    /// Подстрока текста запроса без учёта регистра; пусто или `None` — без фильтра.
    #[serde(default)]
    pub search: Option<String>,
}

impl QueriesParams {
    /// Строка поиска, если она задана (не пустая и не из одних пробелов).
    pub fn search(&self) -> Option<&str> {
        self.search.as_deref().filter(|s| !s.trim().is_empty())
    }
}

pub fn validate(p: &QueriesParams) -> AppResult<()> {
    if !LIMITS.contains(&p.limit) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.queries.badLimit",
            "The row limit must be 50, 100 or 200"
        ));
    }
    match &p.mode {
        QueriesMode::SinceReset => Ok(()),
        QueriesMode::Window { s } if WINDOWS_S.contains(s) => Ok(()),
        QueriesMode::Window { .. } => Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.diag.badWindow",
            "The window must be 300, 900 or 3600 seconds"
        )),
        QueriesMode::Between { .. } => Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.queries.betweenUnsupported",
            "Comparing snapshots is not available yet"
        )),
    }
}

/// Состояние расширения в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PgssMeta {
    pub version: String,
    /// Есть `pg_stat_statements_info` (1.9+): без него время сброса и число вытеснений недоступны.
    pub has_info: bool,
    pub max: i64,
    pub track_planning: bool,
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
}

impl PgssMeta {
    pub fn of(pgss: &PgssInstalled, info: &PgssInfo) -> Self {
        Self {
            version: pgss.version.clone(),
            has_info: pgss.caps.info,
            max: info.max,
            track_planning: info.track_planning,
            stats_reset_ms: info.stats_reset_ms,
            dealloc: info.dealloc,
        }
    }
}

/// Данные инструмента (спец. §4.5). `coverage_s` и `ticks` — только у режима окна.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueriesData {
    pub mode: QueriesMode,
    pub coverage_s: Option<u32>,
    pub ticks: Option<u32>,
    pub pgss: Option<PgssMeta>,
    pub totals: Totals,
    pub rows: Vec<QueryStat>,
}

impl QueriesData {
    pub fn empty(mode: QueriesMode) -> Self {
        Self {
            mode,
            coverage_s: None,
            ticks: None,
            pgss: None,
            totals: Totals::default(),
            rows: Vec::new(),
        }
    }
}

/// Строка набора до сортировки: ключ, имена, счётчики и `(min, max, stddev)` — только у накопительных данных.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub counters: Counters,
    pub minmax: Option<(f64, f64, f64)>,
}

/// Фильтр строк по параметрам: база и верхний уровень.
pub fn keep(key: &QueryKey, datname: Option<&str>, p: &QueriesParams) -> bool {
    (!p.toplevel_only || key.toplevel) && p.database.as_deref().is_none_or(|d| datname == Some(d))
}

/// Набор «с момента сброса» после фильтра базы и верхнего уровня.
pub fn entries_since_reset(rows: &[Cumulative], p: &QueriesParams) -> Vec<Entry> {
    rows.iter()
        .filter(|c| keep(&c.key, c.datname.as_deref(), p))
        .map(|c| Entry {
            key: c.key,
            datname: c.datname.clone(),
            rolname: c.rolname.clone(),
            counters: c.counters,
            minmax: Some((c.min_exec_ms, c.max_exec_ms, c.stddev_exec_ms)),
        })
        .collect()
}

/// Итоги набора без собственных запросов Глафиры — знаменатели долей.
fn totals_without(entries: &[Entry], own: &HashSet<QueryKey>) -> Totals {
    totals(
        entries
            .iter()
            .filter(|e| !own.contains(&e.key))
            .map(|e| &e.counters),
    )
}

/// Строки отчёта: итоги — по всему набору без своих запросов (`own`); строки — только ключи поиска (`matched`, если
/// он был), затем сортировка и обрезка. Свои строки остаются в таблице с признаком `own`.
pub fn build(
    entries: &[Entry],
    own: &HashSet<QueryKey>,
    matched: Option<&HashSet<QueryKey>>,
    p: &QueriesParams,
) -> (Vec<QueryStat>, Totals) {
    let t = totals_without(entries, own);
    let mut out: Vec<QueryStat> = entries
        .iter()
        .filter(|e| matched.is_none_or(|m| m.contains(&e.key)))
        .map(|e| {
            let mut s = stat(
                e.key,
                e.datname.clone(),
                e.rolname.clone(),
                &e.counters,
                e.minmax,
                &t,
            );
            s.own = own.contains(&e.key);
            s
        })
        .collect();
    top(&mut out, p.sort, p.limit);
    (out, t)
}

/// Ключи набора, текст которых нужно прочитать, чтобы узнать, не свой ли это запрос: строка прошла бы правило N+1 при
/// итогах без уже известных своих (`own`), а текст её неизвестен (`unknown`).
pub fn own_candidates(
    entries: &[Entry],
    own: &HashSet<QueryKey>,
    unknown: impl Fn(&QueryKey) -> bool,
) -> Vec<QueryKey> {
    let t = totals_without(entries, own);
    entries
        .iter()
        .filter(|e| !own.contains(&e.key) && unknown(&e.key))
        .filter(|e| {
            let c = &e.counters;
            let mean = if c.calls > 0 {
                c.total_exec_ms / c.calls as f64
            } else {
                0.0
            };
            n_plus_one_share(c.calls, mean, &t).is_some()
        })
        .map(|e| e.key)
        .collect()
}

/// Ключи набора — запросы Глафиры: текст с меткой или запрос опроса узла без метки (строка записана до метки).
pub fn own_keys(
    entries: &[Entry],
    text: impl Fn(&QueryKey) -> Option<Arc<str>>,
) -> HashSet<QueryKey> {
    entries
        .iter()
        .filter(|e| text(&e.key).is_some_and(|t| is_marked_sql(&t) || is_unmarked_poll_sql(&t)))
        .map(|e| e.key)
        .collect()
}

/// Тексты, прочитанные отчётом: `None` — строки нет или текста у неё нет.
type Fetched = HashMap<QueryKey, Option<(Arc<str>, bool)>>;

/// Свои запросы Глафиры в наборе (спец. P3 §4). Текст — из кеша узла; кандидатам N+1 без известного текста он
/// читается с сервера (как `texts`), пока кандидаты не кончатся: исключение своих из итогов поднимает доли остальных.
/// Сбой чтения не роняет отчёт — строки остаются не своими.
async fn resolve_own(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    member: &str,
    entries: &[Entry],
) -> (HashSet<QueryKey>, Fetched) {
    let buffer = st.statements.buffer(id, member);
    let mut fetched: Fetched = HashMap::new();
    loop {
        let (own, candidates) = {
            let b = buffer.as_ref().map(|b| b.lock_safe());
            let text = |k: &QueryKey| match fetched.get(k) {
                Some(t) => t.as_ref().map(|(t, _)| Arc::clone(t)),
                None => b.as_ref().and_then(|b| b.text(k)).map(|(t, _)| t),
            };
            let own = own_keys(entries, text);
            let candidates = own_candidates(entries, &own, |k| {
                !fetched.contains_key(k)
                    && b.as_ref()
                        .is_none_or(|b| b.text(k).is_none() && !b.text_missing(k))
            });
            (own, candidates)
        };
        if candidates.is_empty() {
            return (own, fetched);
        }
        match fetch_texts(st, id, target, candidates.clone()).await {
            Ok(texts) => {
                for k in candidates {
                    fetched.insert(k, None);
                }
                for t in texts {
                    fetched.insert(t.key, t.text.map(|x| (Arc::from(x), t.truncated)));
                }
            }
            Err(e) => {
                tracing::warn!(connection = %id, member, "query texts for own-query detection not read: {}", e.message);
                return (own, fetched);
            }
        }
    }
}

/// Подставляет тексты из кеша: `lookup` отдаёт текст и признак обрезки.
pub fn attach_texts(
    rows: &mut [QueryStat],
    lookup: impl Fn(&QueryKey) -> Option<(Arc<str>, bool)>,
) {
    for r in rows {
        if let Some((text, truncated)) = lookup(&r.key) {
            r.text = Some(text.to_string());
            r.text_truncated = truncated;
        }
    }
}

pub fn missing() -> Unavailable {
    Unavailable {
        part: "statements".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsMissing",
            "{{part}}: pg_stat_statements is not installed in this database (CREATE EXTENSION pg_stat_statements)",
            part = "statements"
        ),
    }
}

pub fn too_old(version: &str) -> Unavailable {
    Unavailable {
        part: "statements".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsOld",
            "{{part}}: pg_stat_statements {{version}} is older than 1.8",
            part = "statements",
            version = version
        ),
    }
}

pub fn hidden_rows() -> Unavailable {
    Unavailable {
        part: "others".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsHidden",
            "{{part}}: needs the pg_read_all_stats role",
            part = "others"
        ),
    }
}

pub fn window_starting() -> Unavailable {
    Unavailable {
        part: "window".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsStarting",
            "{{part}}: the statements sampler is starting",
            part = "window"
        ),
    }
}

fn sort_unavailable() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.queries.sortUnavailable",
        "This sort order is not available for this version or mode"
    )
}

fn member_name(target: &DiagTarget) -> AppResult<String> {
    target
        .member
        .as_ref()
        .map(|m| m.name.clone())
        .ok_or_else(|| AppError::internal_with("queries without a member"))
}

pub async fn report(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    p: &QueriesParams,
) -> AppResult<DiagReport> {
    validate(p)?;
    match &p.mode {
        QueriesMode::SinceReset => report_since_reset(st, id, target, p).await,
        QueriesMode::Window { s } => report_window(st, id, target, p, *s).await,
        QueriesMode::Between { .. } => {
            Err(AppError::internal_with("between mode passed validation"))
        }
    }
}

async fn report_since_reset(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    p: &QueriesParams,
) -> AppResult<DiagReport> {
    let member = member_name(target)?;
    let search = p.search().map(str::to_string);
    let buffer = st.statements.buffer(id, &member);
    let (h, detected, snapshot) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let d = detect(c).await?;
            let snap = match &d {
                Detect::Installed(pgss) => {
                    // Сначала строки: их ключи существовали до скана текстов, и он их проверит.
                    let snap = read_cumulative(c, pgss).await?;
                    let matched = match &search {
                        Some(s) => {
                            let keys = snap.rows.iter().map(|r| r.key).collect();
                            Some(
                                search_keys_cached(c, pgss, s, &snap.info, keys, buffer.as_ref())
                                    .await?,
                            )
                        }
                        None => None,
                    };
                    Some((snap, matched))
                }
                _ => None,
            };
            Ok((h, d, snap))
        })
    })
    .await?;
    let mut unavailable = Vec::new();
    let (data, findings) = match (detected, snapshot) {
        (Detect::Installed(pgss), Some((snap, matched))) => {
            if !p.sort.available(&pgss.caps, false) {
                return Err(sort_unavailable());
            }
            let entries = entries_since_reset(&snap.rows, p);
            let (own, fetched) = resolve_own(st, id, target, &member, &entries).await;
            let (mut rows, t) = build(&entries, &own, matched.as_ref(), p);
            attach_texts(&mut rows, |k| report_text(st, id, &member, &fetched, k));
            if snap.hidden {
                unavailable.push(hidden_rows());
            }
            let state = PgssState {
                track_planning: snap.info.track_planning,
                dealloc: snap.info.dealloc,
                dealloc_grew: false,
                max: snap.info.max,
            };
            let findings = rules::evaluate(Some(&member), &rows, &t, &state, Mode::SinceReset);
            (
                QueriesData {
                    mode: p.mode.clone(),
                    coverage_s: None,
                    ticks: None,
                    pgss: Some(PgssMeta::of(&pgss, &snap.info)),
                    totals: t,
                    rows,
                },
                findings,
            )
        }
        (Detect::TooOld(v), _) => {
            unavailable.push(too_old(&v));
            (QueriesData::empty(p.mode.clone()), Vec::new())
        }
        _ => {
            unavailable.push(missing());
            (QueriesData::empty(p.mode.clone()), Vec::new())
        }
    };
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(data),
        findings,
        unavailable,
    ))
}

/// Срез буфера под блокировкой: всё, что нужно отчёту, копируется наружу.
struct WindowSlice {
    pgss: PgssInstalled,
    info: PgssInfo,
    head: Option<Head>,
    hidden: bool,
    window: Option<crate::sampler::statements::WindowSum>,
    names: std::collections::HashMap<QueryKey, (Option<String>, Option<String>)>,
    /// Ключи pgss последнего тика — существовали до скана текстов поиском.
    latest: HashSet<QueryKey>,
}

fn window_slice(st: &AppState, id: &str, member: &str, window_s: u32) -> Option<WindowSlice> {
    let buffer = st.statements.buffer(id, member)?;
    let b = buffer.lock_safe();
    let pgss = b.pgss()?.clone();
    let window = b.window(window_s);
    let names = window
        .as_ref()
        .map(|w| w.sums.keys().map(|k| (*k, b.names(k))).collect())
        .unwrap_or_default();
    Some(WindowSlice {
        pgss,
        info: b.info().unwrap_or_default(),
        head: b.last_head(),
        hidden: b.hidden(),
        window,
        names,
        latest: b.latest_keys(),
    })
}

/// Режим окна — из буфера сэмплера pgss, без обращения к серверу (спец. §4.2). Буфера или тиков с дельтами ещё
/// нет — пустой отчёт с пометкой «сэмплер запускается».
async fn report_window(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    p: &QueriesParams,
    window_s: u32,
) -> AppResult<DiagReport> {
    let member = member_name(target)?;
    let slice = window_slice(st, id, &member, window_s);
    let head_of = |h: Option<Head>| async move {
        match h {
            Some(h) => Ok(h),
            None => on_member(st, id, target, |c| Box::pin(head(c))).await,
        }
    };
    let Some(WindowSlice {
        pgss,
        info,
        head: h,
        hidden,
        window: Some(w),
        names,
        latest,
    }) = slice
    else {
        let h = head_of(slice.as_ref().and_then(|s| s.head)).await?;
        return Ok(make_report(
            Tool::Queries,
            target,
            Some(&h),
            DiagData::Queries(QueriesData::empty(p.mode.clone())),
            Vec::new(),
            vec![window_starting()],
        ));
    };
    if !p.sort.available(&pgss.caps, true) {
        return Err(sort_unavailable());
    }
    let h = head_of(h).await?;
    let matched = match p.search().map(str::to_string) {
        Some(s) => {
            let pgss = pgss.clone();
            let buffer = st.statements.buffer(id, &member);
            Some(
                on_member(st, id, target, |c| {
                    Box::pin(async move {
                        search_keys_cached(c, &pgss, &s, &info, latest, buffer.as_ref()).await
                    })
                })
                .await?,
            )
        }
        None => None,
    };
    let entries: Vec<Entry> = w
        .sums
        .iter()
        .map(|(k, c)| {
            let (datname, rolname) = names.get(k).cloned().unwrap_or_default();
            Entry {
                key: *k,
                datname,
                rolname,
                counters: *c,
                minmax: None,
            }
        })
        .filter(|e| keep(&e.key, e.datname.as_deref(), p))
        .collect();
    let (own, fetched) = resolve_own(st, id, target, &member, &entries).await;
    // Ключи окна, ушедшие из pgss, скан не находит — их ищем по текстам, которые помнит кеш узла.
    let matched = matched.map(|mut m| {
        if let Some(s) = p.search() {
            let buffer = st.statements.buffer(id, &member);
            let b = buffer.as_ref().map(|b| b.lock_safe());
            m.extend(cached_matches(&entries, s, |k| match fetched.get(k) {
                Some(Some((t, _))) => Some(Arc::clone(t)),
                _ => b.as_ref().and_then(|b| b.text(k)).map(|(t, _)| t),
            }));
        }
        m
    });
    let (mut rows, t) = build(&entries, &own, matched.as_ref(), p);
    attach_texts(&mut rows, |k| report_text(st, id, &member, &fetched, k));
    let mut unavailable = Vec::new();
    if hidden {
        unavailable.push(hidden_rows());
    }
    let state = PgssState {
        track_planning: info.track_planning,
        dealloc: info.dealloc,
        dealloc_grew: w.dealloc_grew,
        max: info.max,
    };
    let findings = rules::evaluate(Some(&member), &rows, &t, &state, Mode::Window);
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(QueriesData {
            mode: p.mode.clone(),
            coverage_s: Some(w.coverage_s),
            ticks: Some(w.ticks),
            pgss: Some(PgssMeta::of(&pgss, &info)),
            totals: t,
            rows,
        }),
        findings,
        unavailable,
    ))
}

/// Текст из кеша узла (буфер сэмплера pgss, спец. §4.6).
pub fn cached_text(
    st: &AppState,
    id: &str,
    member: &str,
    key: &QueryKey,
) -> Option<(Arc<str>, bool)> {
    st.statements
        .buffer(id, member)
        .and_then(|b| b.lock_safe().text(key))
}

/// Текст строки отчёта: прочитанный этим отчётом или из кеша узла.
fn report_text(
    st: &AppState,
    id: &str,
    member: &str,
    fetched: &Fetched,
    key: &QueryKey,
) -> Option<(Arc<str>, bool)> {
    match fetched.get(key) {
        Some(Some(t)) => Some(t.clone()),
        _ => cached_text(st, id, member, key),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryText {
    pub key: QueryKey,
    pub text: Option<String>,
    pub truncated: bool,
}

/// Тексты запросов по ключам — одним запросом (`showtext := true`); ключи, которых в pgss уже нет, пропускаются.
pub async fn texts(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    keys: Vec<QueryKey>,
) -> AppResult<Vec<QueryText>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let target = st.runtimes.diag_target(id, member, false)?;
    fetch_texts(st, id, &target, keys).await
}

/// Тексты по ключам на узле `target` одним запросом; прочитанное — в кеш узла (строка без текста — отметкой «текста
/// нет»).
async fn fetch_texts(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    keys: Vec<QueryKey>,
) -> AppResult<Vec<QueryText>> {
    let ids: Vec<i64> = keys.iter().map(|k| k.query_id).collect();
    let rows = on_member(st, id, target, |c| {
        Box::pin(async move {
            let Detect::Installed(pgss) = detect(c).await? else {
                return Ok(Vec::new());
            };
            let rows = c
                .query(&texts_sql(&pgss.schema, pgss.caps), &[&ids])
                .await
                .map_err(|e| from_pg(&e))?;
            Ok(rows
                .iter()
                .filter_map(|r| {
                    let key = QueryKey {
                        query_id: r.get::<_, Option<i64>>(0)?,
                        dbid: r.get(1),
                        userid: r.get(2),
                        toplevel: r.get::<_, Option<bool>>(3).unwrap_or(true),
                    };
                    Some(QueryText {
                        key,
                        text: r.get(4),
                        truncated: r.get::<_, Option<bool>>(5).unwrap_or(false),
                    })
                })
                .filter(|t| keys.contains(&t.key))
                .collect())
        })
    })
    .await?;
    // Прочитанные тексты — в кеш узла: следующие отчёты подставят их без обращения к серверу.
    if let Some(m) = target.member.as_ref().map(|m| m.name.clone())
        && let Some(b) = st.statements.buffer(id, &m)
    {
        let mut b = b.lock_safe();
        b.remember_texts(
            rows.iter()
                .filter_map(|t| t.text.clone().map(|text| (t.key, text, t.truncated))),
        );
        let no_text: Vec<QueryKey> = rows
            .iter()
            .filter(|t| t.text.is_none())
            .map(|t| t.key)
            .collect();
        b.remember_no_text(&no_text);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::pgss::PgssCaps;

    fn caps(v: &str) -> PgssCaps {
        PgssCaps::from_version(v).unwrap()
    }

    fn cumulative(id: i64, toplevel: bool, db: &str, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: QueryKey {
                query_id: id,
                dbid: 1,
                userid: 1,
                toplevel,
            },
            datname: Some(db.into()),
            rolname: Some("app".into()),
            counters: Counters {
                calls,
                total_exec_ms: ms,
                ..Counters::default()
            },
            min_exec_ms: 0.0,
            max_exec_ms: 0.0,
            stddev_exec_ms: 0.0,
        }
    }

    fn params(mode: QueriesMode) -> QueriesParams {
        QueriesParams {
            mode,
            sort: QuerySort::TotalTime,
            limit: 50,
            database: None,
            toplevel_only: true,
            search: None,
        }
    }

    fn since_reset(rows: &[Cumulative], p: &QueriesParams) -> (Vec<QueryStat>, Totals) {
        build(&entries_since_reset(rows, p), &HashSet::new(), None, p)
    }

    #[test]
    fn sql_follows_the_extension_version() {
        let old = statements_sql("public", caps("1.8"), "");
        assert!(
            old.contains("true, d.datname") && !old.contains("s.toplevel"),
            "{old}"
        );
        assert!(
            old.contains("s.blk_read_time, s.blk_write_time, NULL::float8"),
            "{old}"
        );
        assert!(
            old.contains("NULL::bigint, NULL::float8, NULL::bigint, NULL::bigint"),
            "{old}"
        );
        assert!(old.contains("FROM public.pg_stat_statements(false) s"));
        let v10 = statements_sql("public", caps("1.10"), "");
        assert!(
            v10.contains("s.toplevel")
                && v10.contains("s.temp_blk_read_time + s.temp_blk_write_time"),
            "{v10}"
        );
        assert!(v10.contains("s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time"), "{v10}");
        let v11 = statements_sql(
            "\"my schema\"",
            caps("1.11"),
            " WHERE s.queryid = $1 AND d.datname = $2",
        );
        assert!(
            v11.contains("s.shared_blk_read_time, s.shared_blk_write_time"),
            "{v11}"
        );
        assert!(
            v11.contains("FROM \"my schema\".pg_stat_statements(false) s")
                && v11.ends_with("WHERE s.queryid = $1 AND d.datname = $2")
        );
        let v12 = statements_sql("public", caps("1.12"), "");
        assert!(
            v12.contains("s.parallel_workers_to_launch, s.parallel_workers_launched"),
            "{v12}"
        );
        assert!(
            v12.contains("s.wal_bytes::bigint"),
            "numeric приводится к bigint"
        );
        assert!(
            info_sql("public", caps("1.8"))
                .starts_with("/* glafira */ SELECT NULL::bigint, NULL::bigint,")
        );
        // Периодические запросы Глафиры — с меткой: `pg_stat_statements` хранит её в тексте строки.
        for sql in [
            statements_sql("public", caps("1.8"), ""),
            texts_sql("public", caps("1.9")),
            DETECT_SQL.to_string(),
        ] {
            assert!(crate::product::is_marked_sql(&sql), "{sql}");
        }
        assert!(info_sql("public", caps("1.9")).contains("FROM public.pg_stat_statements_info"));
        assert!(
            texts_sql("public", caps("1.9"))
                .contains("pg_stat_statements(true) s WHERE s.queryid = ANY($1)")
        );
        assert!(texts_sql("public", caps("1.8")).contains("s.userid, true, left(s.query"));
    }

    #[test]
    fn since_reset_filters_sorts_and_sums() {
        let rows = vec![
            cumulative(1, true, "perf", 10, 100.0),
            cumulative(2, false, "perf", 10, 900.0),
            cumulative(3, true, "other", 10, 500.0),
            cumulative(4, true, "perf", 10, 300.0),
        ];
        let (out, t) = since_reset(&rows, &params(QueriesMode::SinceReset));
        // Только toplevel, обе базы: 1, 3, 4 — по убыванию времени.
        assert_eq!(
            out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [3, 4, 1]
        );
        assert_eq!((t.calls, t.total_exec_ms), (30, 900.0));
        assert_eq!(out[0].share_pct, 500.0 / 900.0 * 100.0);
        assert_eq!(
            out[0].stddev_exec_ms,
            Some(0.0),
            "накопительные данные несут min/max/stddev"
        );
        let mut p = params(QueriesMode::SinceReset);
        p.database = Some("perf".into());
        p.toplevel_only = false;
        p.limit = 50;
        let (out, t) = since_reset(&rows, &p);
        assert_eq!(
            out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [2, 4, 1]
        );
        assert_eq!(t.total_exec_ms, 1300.0);
    }

    /// Свои запросы Глафиры: в таблице с признаком `own`, но не в итогах — доли остальных считаются без них.
    #[test]
    fn own_rows_are_flagged_and_left_out_of_the_totals() {
        let rows = vec![
            cumulative(1, true, "perf", 3000, 30.0),
            cumulative(2, true, "perf", 1000, 6000.0),
            cumulative(3, true, "perf", 1000, 4000.0),
        ];
        let p = params(QueriesMode::SinceReset);
        let entries = entries_since_reset(&rows, &p);
        let own = HashSet::from([rows[0].key]);
        let (out, t) = build(&entries, &own, None, &p);
        assert_eq!((t.calls, t.total_exec_ms), (2000, 10_000.0));
        assert_eq!(
            out.iter()
                .map(|r| (r.key.query_id, r.own, r.share_pct))
                .collect::<Vec<_>>(),
            [(2, false, 60.0), (3, false, 40.0), (1, true, 0.3)]
        );
        // Правила по этим строкам и итогам: N+1 своего запроса (3000 вызовов, 0,01 мс) нет.
        let f = rules::evaluate(None, &out, &t, &PgssState::default(), Mode::SinceReset);
        assert!(f.iter().all(|x| x.id != "query_n_plus_one"), "{f:?}");
        let (_, all) = build(&entries, &HashSet::new(), None, &p);
        let f = rules::evaluate(
            None,
            &since_reset(&rows, &p).0,
            &all,
            &PgssState::default(),
            Mode::SinceReset,
        );
        assert_eq!(
            f.iter().filter(|x| x.id == "query_n_plus_one").count(),
            1,
            "без метки — находка"
        );
    }

    /// Кандидаты в свои — строки, что прошли бы N+1 при итогах без уже известных своих, и только с неизвестным текстом.
    #[test]
    fn own_candidates_are_n_plus_one_rows_with_unknown_texts() {
        let p = params(QueriesMode::SinceReset);
        let rows = vec![
            cumulative(1, true, "perf", 3000, 30.0),     // 30 %
            cumulative(2, true, "perf", 1800, 18.0),     // 18 %, без ключа 1 — 25,7 %
            cumulative(3, true, "perf", 5200, 52_000.0), // долгий
        ];
        let entries = entries_since_reset(&rows, &p);
        let none = HashSet::new();
        assert_eq!(own_candidates(&entries, &none, |_| true), [rows[0].key]);
        assert!(
            own_candidates(&entries, &none, |_| false).is_empty(),
            "тексты известны"
        );
        let own = HashSet::from([rows[0].key]);
        assert_eq!(own_candidates(&entries, &own, |_| true), [rows[1].key]);
        // Свой — по метке в начале текста; ведущие пробелы не мешают.
        let texts = HashMap::from([
            (rows[0].key, Arc::<str>::from("  \n/* glafira */ SELECT $1")),
            (rows[1].key, Arc::<str>::from("SELECT 1 /* glafira */")),
        ]);
        assert_eq!(own_keys(&entries, |k| texts.get(k).cloned()), own);
    }

    /// Поиск ограничивает строки, но не итоги: доли — от всей нагрузки.
    #[test]
    fn search_restricts_rows_but_not_the_totals() {
        let rows = vec![
            cumulative(1, true, "perf", 10, 100.0),
            cumulative(2, true, "perf", 10, 300.0),
            cumulative(3, false, "perf", 10, 900.0),
        ];
        let p = params(QueriesMode::SinceReset);
        let entries = entries_since_reset(&rows, &p);
        let matched = HashSet::from([rows[0].key, rows[2].key]);
        let (out, t) = build(&entries, &HashSet::new(), Some(&matched), &p);
        assert_eq!(out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [1]);
        assert_eq!(
            t.total_exec_ms, 400.0,
            "итоги — по всем строкам после фильтра базы и уровня"
        );
        assert_eq!(out[0].share_pct, 25.0);
        let (out, _) = build(&entries, &HashSet::new(), Some(&HashSet::new()), &p);
        assert!(out.is_empty());
    }

    /// Окно: ключ ушёл из pgss (скан текстов его не найдёт), но его текст есть в кеше узла — он находится по кешу.
    #[test]
    fn cached_texts_find_keys_gone_from_pgss() {
        let rows = vec![
            cumulative(1, true, "perf", 10, 100.0),
            cumulative(2, true, "perf", 10, 300.0),
            cumulative(3, true, "perf", 10, 300.0),
        ];
        let entries = entries_since_reset(&rows, &params(QueriesMode::SinceReset));
        let texts: HashMap<QueryKey, Arc<str>> = HashMap::from([
            (rows[0].key, Arc::from("SELECT * FROM shop.ORDERS o")),
            (rows[1].key, Arc::from("SELECT 1")),
        ]);
        let found = cached_matches(&entries, "Orders o", |k| texts.get(k).cloned());
        assert_eq!(found, HashSet::from([rows[0].key]));
        assert!(cached_matches(&entries, "x%", |k| texts.get(k).cloned()).is_empty());
    }

    #[test]
    fn search_sql_and_escaping() {
        assert_eq!(like_escape("shop.customers"), "shop.customers");
        assert_eq!(like_escape(r"50%_a\b"), r"50\%\_a\\b");
        let sql = search_sql("public", caps("1.9"));
        assert!(
            sql.starts_with("/* glafira */ SELECT s.queryid, s.dbid, s.userid, s.toplevel ")
                && sql.ends_with(
                    "FROM public.pg_stat_statements(true) s WHERE s.query ILIKE '%' || $1 || '%'"
                ),
            "{sql}"
        );
        assert!(search_sql("public", caps("1.8")).contains("s.userid, true FROM"));
        let mut p = params(QueriesMode::SinceReset);
        assert_eq!(p.search(), None);
        p.search = Some("  ".into());
        assert_eq!(p.search(), None, "пробелы — не поиск");
        p.search = Some(" Orders".into());
        assert_eq!(p.search(), Some(" Orders"));
    }

    #[test]
    fn attach_texts_uses_the_lookup() {
        let rows = vec![
            cumulative(1, true, "perf", 1, 1.0),
            cumulative(2, true, "perf", 1, 1.0),
        ];
        let (mut out, _) = since_reset(&rows, &params(QueriesMode::SinceReset));
        attach_texts(&mut out, |k| {
            (k.query_id == 1).then(|| (std::sync::Arc::<str>::from("SELECT 1"), true))
        });
        assert_eq!(
            (out[0].text.as_deref(), out[0].text_truncated),
            (Some("SELECT 1"), true)
        );
        assert_eq!(
            (out[1].text.as_deref(), out[1].text_truncated),
            (None, false)
        );
    }

    #[test]
    fn params_are_validated() {
        assert!(validate(&params(QueriesMode::SinceReset)).is_ok());
        assert!(validate(&params(QueriesMode::Window { s: 900 })).is_ok());
        assert_eq!(
            validate(&params(QueriesMode::Window { s: 60 }))
                .unwrap_err()
                .key,
            "errors.diag.badWindow"
        );
        let mut p = params(QueriesMode::SinceReset);
        p.limit = 70;
        assert_eq!(validate(&p).unwrap_err().key, "errors.queries.badLimit");
        let between = params(QueriesMode::Between {
            from: "a".into(),
            to: "b".into(),
        });
        let e = validate(&between).unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (
                ErrorCode::FeatureUnavailable,
                "errors.queries.betweenUnsupported"
            )
        );
    }

    #[test]
    fn request_and_data_json() {
        let r: crate::diag::DiagRequest = serde_json::from_value(serde_json::json!({
            "tool": "queries", "member": null,
            "queries": {"mode": {"kind": "window", "s": 300}, "sort": "hitRatio", "limit": 100, "database": "perf", "toplevelOnly": false}
        }))
        .unwrap();
        let q = r.queries.unwrap();
        assert_eq!(q.search, None, "search необязателен");
        assert_eq!(
            (
                q.mode,
                q.sort,
                q.limit,
                q.database.as_deref(),
                q.toplevel_only
            ),
            (
                QueriesMode::Window { s: 300 },
                QuerySort::HitRatio,
                100,
                Some("perf"),
                false
            )
        );
        let r: crate::diag::DiagRequest = serde_json::from_value(serde_json::json!({"tool": "queries", "queries": {"mode": {"kind": "sinceReset"}, "sort": "calls", "limit": 50, "database": null, "search": "orders"}})).unwrap();
        assert_eq!(
            r.queries.as_ref().unwrap().search.as_deref(),
            Some("orders")
        );
        assert!(
            r.queries.unwrap().toplevel_only,
            "toplevelOnly по умолчанию включён"
        );
        let v = serde_json::to_value(crate::diag::DiagData::Queries(QueriesData::empty(
            QueriesMode::SinceReset,
        )))
        .unwrap();
        assert_eq!(
            (&v["kind"], &v["mode"]["kind"], &v["rows"], &v["pgss"]),
            (
                &"queries".into(),
                &"sinceReset".into(),
                &serde_json::json!([]),
                &serde_json::Value::Null
            )
        );
        assert_eq!(v["totals"]["calls"], 0);
        let pgss = PgssInstalled {
            schema: "public".into(),
            version: "1.8".into(),
            caps: caps("1.8"),
        };
        let meta = serde_json::to_value(PgssMeta::of(&pgss, &PgssInfo::default())).unwrap();
        assert_eq!(
            (
                &meta["hasInfo"],
                &meta["statsResetMs"],
                &meta["trackPlanning"]
            ),
            (&false.into(), &serde_json::Value::Null, &false.into())
        );
    }

    #[test]
    fn queries_tool_needs_params() {
        let req = crate::diag::DiagRequest {
            tool: crate::diag::Tool::Queries,
            member: None,
            window_s: None,
            queries: None,
        };
        assert_eq!(
            crate::diag::validate(&req, crate::conn::profile::ConnectionKind::Server)
                .unwrap_err()
                .key,
            "errors.queries.paramsRequired"
        );
        assert!(
            !crate::diag::Tool::Queries.is_cluster() && !crate::diag::Tool::Queries.patroni_only()
        );
    }
}
