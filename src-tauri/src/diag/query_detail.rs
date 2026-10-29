//! Карточка запроса (спец. P3 §7.2): накопительные строки pgss по ролям и `toplevel`, окна и история из буфера
//! сэмплера pgss, выполняющиеся экземпляры из `pg_stat_activity` (14+), ожидания из буфера сэмплера активности.

use std::collections::HashMap;

use pg_advisor::ash::Sample;
use pg_advisor::findings::Finding;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat, totals};
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::queries::{
    Detect, PgssInfo, PgssInstalled, detect, info_sql, missing, row_from, statements_sql, too_old,
};
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::sql::BACKEND_START_US;
use crate::diag::{Head, Unavailable, WINDOWS_S, head, on_member};
use crate::error::{AppResult, ErrorCode};
use crate::sampler::statements::{HistoryPoint, WindowSum};
use crate::state::AppState;
use crate::sync::MutexExt;

/// История и ожидания — за последний час.
const LOOKBACK_MS: i64 = 3_600_000;

/// Выполняющийся экземпляр запроса: сессия с этим `query_id` в `pg_stat_activity` (14+).
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
    /// Текст упёрся в `track_activity_query_size`: литералы могут быть обрезаны, для плана такой текст не годится.
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
    /// `None` — буфера сэмплера активности на узле нет (вкладка «Сессии» не открывалась).
    pub waits: Option<QueryWaits>,
    pub findings: Vec<Finding>,
    pub unavailable: Vec<Unavailable>,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub pgss_version: Option<String>,
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

/// Строка окна по ключам запроса; доля — от всего окна. `None` — дельт у этих ключей в окне нет.
pub fn window_stat(w: &WindowSum, keys: &[QueryKey]) -> Option<QueryStat> {
    let mut sum = Counters::default();
    let mut any = false;
    for k in keys {
        if let Some(c) = w.sums.get(k) {
            sum.add(c);
            any = true;
        }
    }
    if !any {
        return None;
    }
    let t = totals(w.sums.values());
    let key = keys[0];
    Some(stat(key, None, None, &sum, None, &t))
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
    items.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.event_type.cmp(&b.event_type))
            .then_with(|| a.event.cmp(&b.event))
    });
    Some(QueryWaits {
        source: WaitsSource::Sampler,
        since_ms,
        total,
        items,
    })
}

/// Выполняющиеся экземпляры (14+): `$1` — `query_id`, `$2` — база. Признак обрезки: сервер режет текст до
/// `track_activity_query_size - 1` байт по границе символа (в UTF-8 — не более 3 байт недобора); предел берётся из
/// `pg_settings` (байты), а не из `current_setting` (строка с единицей, например `1kB`).
pub fn running_sql() -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.state, (extract(epoch FROM a.query_start) * 1000)::bigint, \
         extract(epoch FROM now() - a.query_start)::float8, a.wait_event_type, a.wait_event, a.usename::text, \
         a.application_name, host(a.client_addr), left(a.query, {QUERY_LIMIT}), \
         octet_length(a.query) >= (SELECT setting::int FROM pg_settings WHERE name = 'track_activity_query_size') - 4 \
         FROM pg_stat_activity a \
         WHERE a.query_id = $1 AND a.datname = $2 AND a.pid <> pg_backend_pid() ORDER BY a.query_start"
    )
}

fn text_sql(ns: &str) -> String {
    format!(
        "SELECT left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} FROM {ns}.pg_stat_statements(true) s \
         JOIN pg_database d ON d.oid = s.dbid WHERE s.queryid = $1 AND d.datname = $2 LIMIT 1"
    )
}

fn totals_sql(ns: &str) -> String {
    format!(
        "SELECT COALESCE(sum(calls), 0)::bigint, COALESCE(sum(total_exec_time), 0)::float8, COALESCE(sum(rows), 0)::bigint \
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
    unavailable: Vec<Unavailable>,
}

async fn fetch(c: &Client, database: &str, query_id: i64) -> AppResult<Fetched> {
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
                        " WHERE s.queryid = $1 AND d.datname = $2",
                    ),
                    &[&query_id, &database],
                )
                .await
                .map_err(pg)?;
            let rows: Vec<Cumulative> = raw.iter().filter_map(row_from).collect();
            let text = c
                .query_opt(&text_sql(&pgss.schema), &[&query_id, &database])
                .await
                .map_err(pg)?
                .map(|r| {
                    (
                        r.get::<_, String>(0),
                        r.get::<_, Option<bool>>(1).unwrap_or(false),
                    )
                });
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
    Ok(Fetched {
        head: h,
        pgss,
        rows,
        text,
        totals: t,
        info,
        running,
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
    // Замыкание отдаёт будущее на любое время жизни соединения, поэтому база уходит в него владеемой строкой.
    let database = database.to_string();
    let f = on_member(st, id, &target, |c| {
        Box::pin(async move { fetch(c, &database, query_id).await })
    })
    .await?;
    let mut d = QueryDetail::empty(&f.head);
    d.unavailable = f.unavailable;
    d.running = f.running;
    d.totals = f.totals;
    d.pgss_version = f.pgss.as_ref().map(|p| p.version.clone());
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
                stat: b.window(*w).and_then(|sum| window_stat(&sum, &keys)),
            })
            .collect();
        d.history = b.history(&keys, now_ms - LOOKBACK_MS);
        d.gaps = b
            .gaps(now_ms - LOOKBACK_MS)
            .into_iter()
            .map(|(from_ms, to_ms)| Gap { from_ms, to_ms })
            .collect();
    }
    if let Some(b) = st.samplers.buffer(id, member) {
        let b = b.lock_safe();
        d.waits = waits_of(
            &b.samples(now_ms - LOOKBACK_MS),
            query_id,
            now_ms - LOOKBACK_MS,
        );
    }
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
    fn window_stat_sums_only_the_query_keys() {
        let mut sums = std::collections::HashMap::new();
        sums.insert(
            key(7, 1, true),
            Counters {
                calls: 3,
                total_exec_ms: 30.0,
                ..Counters::default()
            },
        );
        sums.insert(
            key(7, 2, true),
            Counters {
                calls: 1,
                total_exec_ms: 10.0,
                ..Counters::default()
            },
        );
        sums.insert(
            key(8, 1, true),
            Counters {
                calls: 100,
                total_exec_ms: 1000.0,
                ..Counters::default()
            },
        );
        let w = crate::sampler::statements::WindowSum {
            since_ms: 0,
            until_ms: 60_000,
            ticks: 1,
            coverage_s: 60,
            dealloc_grew: false,
            sums,
        };
        let s = window_stat(&w, &[key(7, 1, true), key(7, 2, true)]).unwrap();
        assert_eq!((s.calls, s.total_exec_ms, s.mean_exec_ms), (4, 40.0, 10.0));
        assert_eq!(s.stddev_exec_ms, None);
        assert!(
            (s.share_pct - 40.0 / 1040.0 * 100.0).abs() < 1e-9,
            "доля — от всего окна"
        );
        assert!(window_stat(&w, &[key(9, 1, true)]).is_none());
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
            (&v["text"], &v["sinceReset"], &v["waits"]),
            (
                &serde_json::Value::Null,
                &serde_json::Value::Null,
                &serde_json::Value::Null
            )
        );
    }
}
