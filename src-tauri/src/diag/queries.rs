//! Инструмент «Запросы» (спец. P3 §4): поиск `pg_stat_statements`, SQL по версии расширения, режим «с момента
//! сброса» и ленивые тексты. Режим «за окно» — из буфера сэмплера pgss (`sampler::statements`).

use std::sync::Arc;

use pg_advisor::pgss::PgssCaps;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{
    Counters, Cumulative, QueryKey, QuerySort, QueryStat, Totals, stat, top, totals,
};
use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, Row};

use crate::catalog::ddl::quote_ident;
use crate::conn::errors::from_pg;
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::{
    DiagData, DiagReport, Tool, Unavailable, WINDOWS_S, head, make_report, on_member,
};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
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

const DETECT_SQL: &str = "SELECT e.extversion::text, n.nspname::text FROM pg_extension e \
    JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_stat_statements'";

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
        "SELECT s.userid, s.dbid, s.queryid, {toplevel}, d.datname::text, r.rolname::text, \
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
        "SELECT {info}, current_setting('pg_stat_statements.max')::bigint, \
         current_setting('pg_stat_statements.track_planning', true)"
    )
}

/// Тексты по ключам (`showtext := true`, `$1` — массив `queryid`): не длиннее `QUERY_LIMIT` и признак обрезки.
pub fn texts_sql(ns: &str, caps: PgssCaps) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    format!(
        "SELECT s.queryid, s.dbid, s.userid, {toplevel}, left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} \
         FROM {ns}.pg_stat_statements(true) s WHERE s.queryid = ANY($1)"
    )
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
    pub max: i64,
    pub track_planning: bool,
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
}

impl PgssMeta {
    pub fn of(pgss: &PgssInstalled, info: &PgssInfo) -> Self {
        Self {
            version: pgss.version.clone(),
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

/// Фильтр строк по параметрам: база и верхний уровень.
pub fn keep(c: &Cumulative, p: &QueriesParams) -> bool {
    (!p.toplevel_only || c.key.toplevel)
        && p.database
            .as_deref()
            .is_none_or(|d| c.datname.as_deref() == Some(d))
}

/// Строки отчёта «с момента сброса»: фильтр, итоги по отфильтрованному набору, сортировка и обрезка.
pub fn stats_since_reset(rows: &[Cumulative], p: &QueriesParams) -> (Vec<QueryStat>, Totals) {
    let kept: Vec<&Cumulative> = rows.iter().filter(|c| keep(c, p)).collect();
    let t = totals(kept.iter().map(|c| &c.counters));
    let mut out: Vec<QueryStat> = kept
        .iter()
        .map(|c| {
            stat(
                c.key,
                c.datname.clone(),
                c.rolname.clone(),
                &c.counters,
                Some((c.min_exec_ms, c.max_exec_ms, c.stddev_exec_ms)),
                &t,
            )
        })
        .collect();
    top(&mut out, p.sort, p.limit);
    (out, t)
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
            "{{part}}: pg_stat_statements is not installed in this database",
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
    let (h, detected, snapshot) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let d = detect(c).await?;
            let snap = match &d {
                Detect::Installed(pgss) => Some(read_cumulative(c, pgss).await?),
                _ => None,
            };
            Ok((h, d, snap))
        })
    })
    .await?;
    let mut unavailable = Vec::new();
    let (data, findings) = match (detected, snapshot) {
        (Detect::Installed(pgss), Some(snap)) => {
            if !p.sort.available(&pgss.caps, false) {
                return Err(sort_unavailable());
            }
            let (mut rows, t) = stats_since_reset(&snap.rows, p);
            attach_texts(&mut rows, |k| cached_text(st, id, &member, k));
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

/// Режим окна: буфер сэмплера pgss появляется в задаче 6; без него — пустой отчёт с пометкой «сэмплер запускается».
async fn report_window(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
    p: &QueriesParams,
    _window_s: u32,
) -> AppResult<DiagReport> {
    let h = on_member(st, id, target, |c| Box::pin(head(c))).await?;
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(QueriesData::empty(p.mode.clone())),
        Vec::new(),
        vec![window_starting()],
    ))
}

/// Текст из кеша узла; кеш живёт в буфере сэмплера pgss (задача 6) — до него текстов в кеше нет.
pub fn cached_text(
    _st: &AppState,
    _id: &str,
    _member: &str,
    _key: &QueryKey,
) -> Option<(Arc<str>, bool)> {
    None
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
    let ids: Vec<i64> = keys.iter().map(|k| k.query_id).collect();
    let rows = on_member(st, id, &target, |c| {
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
        }
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
        assert!(info_sql("public", caps("1.8")).starts_with("SELECT NULL::bigint, NULL::bigint,"));
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
        let (out, t) = stats_since_reset(&rows, &params(QueriesMode::SinceReset));
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
        let (out, t) = stats_since_reset(&rows, &p);
        assert_eq!(
            out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [2, 4, 1]
        );
        assert_eq!(t.total_exec_ms, 1300.0);
    }

    #[test]
    fn attach_texts_uses_the_lookup() {
        let rows = vec![
            cumulative(1, true, "perf", 1, 1.0),
            cumulative(2, true, "perf", 1, 1.0),
        ];
        let (mut out, _) = stats_since_reset(&rows, &params(QueriesMode::SinceReset));
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
        let r: crate::diag::DiagRequest = serde_json::from_value(serde_json::json!({"tool": "queries", "queries": {"mode": {"kind": "sinceReset"}, "sort": "calls", "limit": 50, "database": null}})).unwrap();
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
