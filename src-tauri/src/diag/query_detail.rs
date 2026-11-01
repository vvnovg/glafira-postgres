//! Карточка запроса (спец. P3 §7.2): накопительные строки pgss по ролям и `toplevel`, окна и история из буфера
//! сэмплера pgss, выполняющиеся экземпляры из `pg_stat_activity` (14+), ожидания из буфера сэмплера активности.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pg_advisor::ash::Sample;
use pg_advisor::findings::Finding;
use pg_advisor::pgss::PgssCaps;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat};
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
use crate::product::SQL_MARKER;
use crate::sampler::statements::{HistoryPoint, KeysWindow, StatementsBuffer};
use crate::state::AppState;
use crate::sync::MutexExt;

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
    /// `None` — буфера сэмплера активности на узле нет (вкладка «Профиль ожиданий» не открывалась).
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
    unavailable: Vec<Unavailable>,
}

/// `cache` — буфер сэмплера pgss узла (если сэмплер на узле запускался): из него берётся текст, в него же кладётся
/// прочитанный с сервера. Блокировка короткая и никогда не держится через `await`.
async fn fetch(
    c: &Client,
    database: &str,
    query_id: i64,
    cache: Option<&Mutex<StatementsBuffer>>,
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
    // Замыкание отдаёт будущее на любое время жизни соединения, поэтому база и буфер уходят в него владеемыми.
    let database = database.to_string();
    let cache = st.statements.buffer(id, member);
    let f = on_member(st, id, &target, |c| {
        Box::pin(async move { fetch(c, &database, query_id, cache.as_deref()).await })
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
