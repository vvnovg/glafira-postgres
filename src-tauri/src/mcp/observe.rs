//! Наблюдение MCP (спец. P4 §5.3): `observe_activity` подписывается на сэмплер активности узла на окно (тот же сэмплер,
//! что у вкладки: нагрузка не удваивается) и сводит его буфер за окно; `observe_top_queries` читает
//! `pg_stat_statements` (`showtext := false`) в начале и в конце окна и отдаёт разницу в форме топа. Прогресс — после
//! каждого тика сэмплера (у pgss — раз в секунду) через `ObserveCtx::progress`; отмена клиентом (`ObserveCtx::cancel`)
//! завершает окно досрочно с уже собранным. Одну длительную операцию на сессию и предел длительности держит крейт шлюза.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{ObserveCtx, ObservedActivity, PgAnswer, PgOutput};
use glafira_pg_mcp::params::{ObserveActivityParams, ObserveQueriesParams};
use pg_advisor::ash::Sample;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;

use super::access::{check_connection, ensure_open, masks, open_for_llm, tool_error};
use super::mask::{MAX_QUERY_BYTES, cut_text, mask_text};
use super::read::{TOP_LIMIT, mask_statements, member_or_leader, parse_sort, report_out};
use crate::conn::run_blocking;
use crate::diag::queries::{
    Detect, Observed, PgssInstalled, QueriesMode, QueriesParams, Snapshot, detect, read_cumulative,
    report_observed,
};
use crate::diag::{Head, head, on_member};
use crate::error::AppError;
use crate::sampler::registry::{SamplerEvent, SamplerSink};
use crate::sampler::statements::{pgss_missing, pgss_too_old};
use crate::session::SessionRegistry;
use crate::state::AppState;
use crate::sync::MutexExt;
use crate::topology::model::now_ms;
use crate::topology::runtime::DiagTarget;

/// «Нет ожидания»: сессия на CPU (или ожидание не инструментировано), как `CPU*` профиля ожиданий.
const CPU: &str = "CPU*";
/// Строк в топах ответа.
const TOP: usize = 10;
/// Сессий в списках «появились» и «пропали».
const MAX_SESSIONS: usize = 20;
const CANCELLED_HINT: &str = "The observation ended early (cancelled): the numbers cover only the elapsed part of the window";

/// Тип и событие ожидания; `(None, None)` — CPU.
type WaitKey<'s> = (Option<&'s str>, Option<&'s str>);

/// Ключ запроса в топе: `query_id`, если он есть и не 0, иначе текст.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum QueryKey<'s> {
    Id(i64),
    Text(&'s str),
}

/// Счётчик запроса окна: id, текст первого сэмпла, сэмплы и ожидания.
struct QueryAcc<'s> {
    query_id: Option<i64>,
    text: &'s str,
    samples: u64,
    waits: HashMap<WaitKey<'s>, u64>,
}

fn ranked<'s>(m: HashMap<WaitKey<'s>, u64>) -> Vec<(WaitKey<'s>, u64)> {
    let mut v: Vec<_> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v
}

fn label((t, e): WaitKey<'_>) -> String {
    match (t, e) {
        (Some(t), Some(e)) => format!("{t}:{e}"),
        (Some(t), None) => t.to_string(),
        _ => CPU.to_string(),
    }
}

fn wait_value((t, e): WaitKey<'_>, samples: u64, share: Option<f64>) -> Value {
    let mut v = json!({ "waitEventType": t.unwrap_or(CPU), "waitEvent": e, "samples": samples });
    if let Some(s) = share {
        v["share"] = json!(s);
    }
    v
}

/// Текст запроса для ответа: маскирование (уровень `diagnostics`), затем не длиннее `MAX_QUERY_BYTES`; второе
/// значение — замаскировано литералов.
fn shown_text(text: &str, mask: bool) -> (String, usize) {
    let mut text = text.to_string();
    let masked = if mask { mask_text(&mut text) } else { 0 };
    cut_text(&mut text, MAX_QUERY_BYTES);
    (text, masked)
}

/// Сводка окна `from_ms`…`to_ms` (часы приложения, как у сэмплов) по буферу активности: ожидания, топ запросов с их
/// ожиданиями, сессии, ставшие активными и переставшие быть активными (по pid лидера: параллельные воркеры несут его
/// запрос), пики по тикам. Тексты (`mask` — маскировать, уровень `diagnostics`) — не длиннее `MAX_QUERY_BYTES`.
/// Порядок запросов с равным числом сэмплов — по показанному (замаскированному) тексту: порядок исходных текстов
/// выдавал бы сравнение литералов. Второе значение — число замаскированных литералов у отданных запросов.
pub fn summarize(
    samples: &[Sample<'_>],
    ticks: &[i64],
    from_ms: i64,
    to_ms: i64,
    mask: bool,
) -> (ObservedActivity, usize) {
    let in_window = |at: i64| at >= from_ms && at <= to_ms;
    let samples: Vec<&Sample<'_>> = samples.iter().filter(|s| in_window(s.at_ms)).collect();
    let ticks: Vec<i64> = ticks.iter().copied().filter(|t| in_window(*t)).collect();
    let total = samples.len() as u64;
    let share = |n: u64| {
        if total == 0 {
            0.0
        } else {
            n as f64 / total as f64
        }
    };
    let mut per_tick: BTreeMap<i64, (u32, u32)> = ticks.iter().map(|t| (*t, (0, 0))).collect();
    for s in &samples {
        let e = per_tick.entry(s.at_ms).or_default();
        e.0 += 1;
        if s.wait_type == Some("Lock") {
            e.1 += 1;
        }
    }
    let peak_sessions = per_tick.values().map(|v| v.0).max().unwrap_or(0);
    let peak_lock_waits = per_tick.values().map(|v| v.1).max().unwrap_or(0);
    let mut waits: HashMap<WaitKey<'_>, u64> = HashMap::new();
    for s in &samples {
        *waits.entry((s.wait_type, s.wait_event)).or_default() += 1;
    }
    let waits = ranked(waits);
    let top_wait = waits.first().map(|(w, _)| label(*w));
    let mut queries: HashMap<QueryKey<'_>, QueryAcc<'_>> = HashMap::new();
    for s in &samples {
        let key = match s.query_id {
            Some(id) if id != 0 => QueryKey::Id(id),
            _ => QueryKey::Text(s.query),
        };
        let q = queries.entry(key).or_insert_with(|| QueryAcc {
            query_id: s.query_id.filter(|i| *i != 0),
            text: s.query,
            samples: 0,
            waits: HashMap::new(),
        });
        q.samples += 1;
        *q.waits.entry((s.wait_type, s.wait_event)).or_default() += 1;
    }
    let mut queries: Vec<(QueryAcc<'_>, String, usize)> = queries
        .into_values()
        .map(|q| {
            let (text, n) = shown_text(q.text, mask);
            (q, text, n)
        })
        .collect();
    queries.sort_by(|(a, at, _), (b, bt, _)| {
        b.samples
            .cmp(&a.samples)
            .then_with(|| at.cmp(bt))
            .then(a.query_id.cmp(&b.query_id))
    });
    let mut masked = 0;
    let top_queries: Vec<Value> = queries
        .into_iter()
        .take(TOP)
        .map(|(q, text, n)| {
            masked += n;
            let waits: Vec<Value> = ranked(q.waits)
                .into_iter()
                .take(3)
                .map(|(w, n)| wait_value(w, n, None))
                .collect();
            json!({
                "queryId": q.query_id.map(|i| i.to_string()),
                "query": text,
                "samples": q.samples,
                "share": share(q.samples),
                "waits": waits,
            })
        })
        .collect();
    // Сессии по pid лидера: первый и последний тик окна — кто был активен.
    let mut seen: BTreeMap<i32, (&Sample<'_>, i64, i64)> = BTreeMap::new();
    for s in samples
        .iter()
        .filter(|s| s.leader_pid.is_none_or(|l| l == s.pid))
    {
        let e = seen.entry(s.pid).or_insert((*s, s.at_ms, s.at_ms));
        e.1 = e.1.min(s.at_ms);
        e.2 = e.2.max(s.at_ms);
    }
    let active_at = |tick: Option<i64>| -> HashSet<i32> {
        samples
            .iter()
            .filter(|s| Some(s.at_ms) == tick)
            .map(|s| s.pid)
            .collect()
    };
    let (at_first, at_last) = (
        active_at(ticks.first().copied()),
        active_at(ticks.last().copied()),
    );
    let session = |s: &Sample<'_>| json!({ "pid": s.pid, "application": s.application, "user": s.user, "database": s.database });
    let (mut appeared, mut disappeared) = (Vec::new(), Vec::new());
    if ticks.len() >= 2 {
        let mut new: Vec<&(&Sample<'_>, i64, i64)> = seen
            .iter()
            .filter(|(pid, _)| !at_first.contains(*pid))
            .map(|(_, v)| v)
            .collect();
        new.sort_by_key(|&&(_, first, _)| first);
        appeared = new
            .into_iter()
            .take(MAX_SESSIONS)
            .map(|&(s, first, _)| {
                let mut v = session(s);
                v["firstSeenMs"] = json!(first);
                v
            })
            .collect();
        disappeared = seen
            .iter()
            .filter(|(pid, _)| at_first.contains(*pid) && !at_last.contains(*pid))
            .take(MAX_SESSIONS)
            .map(|(_, &(s, _, last))| {
                let mut v = session(s);
                v["lastSeenMs"] = json!(last);
                v
            })
            .collect();
    }
    let lock_samples = samples
        .iter()
        .filter(|s| s.wait_type == Some("Lock"))
        .count() as u64;
    let cpu_samples = samples.iter().filter(|s| s.wait_type.is_none()).count() as u64;
    let avg_active = if ticks.is_empty() {
        0.0
    } else {
        total as f64 / ticks.len() as f64
    };
    let summary = json!({
        "ticks": ticks.len(),
        "avgActiveSessions": avg_active,
        "lockWaitShare": share(lock_samples),
        "cpuShare": share(cpu_samples),
        "topWait": top_wait,
    });
    (
        ObservedActivity {
            started_at_ms: from_ms,
            duration_ms: u64::try_from(to_ms - from_ms).unwrap_or(0),
            samples: u32::try_from(ticks.len()).unwrap_or(u32::MAX),
            waits: waits
                .iter()
                .take(TOP)
                .map(|(w, n)| wait_value(*w, *n, Some(share(*n))))
                .collect(),
            top_queries,
            appeared,
            disappeared,
            peak_sessions,
            peak_lock_waits,
            summary,
        },
        masked,
    )
}

/// Канал подписки наблюдения: события сэмплера — в цикл окна. Закрытый приёмник (окно кончилось или тело вызова
/// брошено) снимает подписку при следующей рассылке.
struct ChannelSink(UnboundedSender<SamplerEvent>);

impl SamplerSink<SamplerEvent> for ChannelSink {
    fn send(&self, e: SamplerEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

/// Подписка окна: снимается при выходе из тела вызова любым путём, в том числе когда ядро шлюза бросает его на
/// `await` (закрылась сессия MCP).
struct Subscription<'a> {
    sessions: &'a SessionRegistry,
    id: String,
}

impl Drop for Subscription<'_> {
    fn drop(&mut self) {
        self.sessions.stop(&self.id);
    }
}

/// Что видел цикл окна.
#[derive(Debug, Default)]
struct Watch {
    ticks: u32,
    errors: u32,
    /// Последняя ошибка сэмплера в виде для модели (`code: message`, `access::tool_error`: без путей и цели ssh).
    last_error: Option<String>,
    /// Сэмплер остановлен не нами (подключение закрыто, участник исчез, отказ во входе).
    stopped: bool,
    /// Клиент отменил вызов.
    cancelled: bool,
}

fn model_error(e: AppError) -> String {
    let e = tool_error(e);
    format!("{}: {}", e.code, e.message)
}

/// Окно наблюдения активности: до `ctx.duration`, отмены клиентом или остановки сэмплера; прогресс — на каждый тик.
async fn watch(rx: &mut UnboundedReceiver<SamplerEvent>, ctx: &ObserveCtx) -> Watch {
    let total = u32::try_from(ctx.duration.as_secs()).unwrap_or(u32::MAX);
    let started = Instant::now();
    let deadline = started + ctx.duration;
    let mut w = Watch::default();
    loop {
        tokio::select! {
            _ = ctx.cancel.cancelled() => {
                w.cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(deadline) => break,
            e = rx.recv() => match e {
                Some(SamplerEvent::Tick { .. }) => {
                    w.ticks += 1;
                    let done = u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX);
                    (ctx.progress)(done.min(total), total);
                }
                Some(SamplerEvent::Error { error }) => {
                    w.errors += 1;
                    w.last_error = Some(model_error(error));
                }
                Some(SamplerEvent::Gap { .. }) => {}
                Some(SamplerEvent::Stopped) | None => {
                    w.stopped = true;
                    break;
                }
            },
        }
    }
    w
}

/// `observe_activity`: подписка на сэмплер узла на окно, затем сводка его буфера за окно; тексты на уровне
/// `diagnostics` маскируются (`mask::mask_text`, без чтения `standard_conforming_strings`).
pub async fn observe_activity(
    st: &AppState,
    p: ObserveActivityParams,
    ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    let access = open_for_llm(st, &p.connection_id).await?;
    let mask = masks(access);
    let id = p.connection_id.as_str();
    let member = member_or_leader(st, id, p.member.as_deref())?;
    let (tx, mut rx) = unbounded_channel();
    let started_at_ms = now_ms();
    let subscription = Subscription {
        sessions: &st.sessions,
        id: crate::sampler::start(st, id, Some(&member), Arc::new(ChannelSink(tx)))
            .map_err(tool_error)?,
    };
    let w = watch(&mut rx, &ctx).await;
    drop(subscription);
    let ended_at_ms = now_ms();
    let (mut out, masked) = match st.samplers.buffer(id, &member) {
        Some(b) => run_blocking(move || {
            let b = b.lock_safe();
            Ok(summarize(
                &b.samples(started_at_ms),
                &b.ticks(started_at_ms),
                started_at_ms,
                ended_at_ms,
                mask,
            ))
        })
        .await
        .map_err(tool_error)?,
        None => summarize(&[], &[], started_at_ms, ended_at_ms, mask),
    };
    out.summary["member"] = json!(member);
    out.summary["samplerErrors"] = json!(w.errors);
    if let Some(e) = w.last_error {
        out.summary["lastSamplerError"] = json!(e);
    }
    if w.cancelled || w.stopped {
        out.summary["endedEarly"] = json!(true);
    }
    Ok(PgAnswer {
        body: PgOutput::ObservedActivity(out),
        masked: mask.then_some(masked as u64),
        rows: None,
    })
}

/// Шапка узла и чтение pgss без текстов; нет расширения или оно старше 1.8 — `FEATURE_UNAVAILABLE`.
async fn read_statements(
    st: &AppState,
    id: &str,
    target: &DiagTarget,
) -> Result<(Head, PgssInstalled, Snapshot), ToolError> {
    on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            match detect(c, None).await? {
                Detect::Installed(pgss) => {
                    let snap = read_cumulative(c, &pgss).await?;
                    Ok((h, pgss, snap))
                }
                Detect::TooOld(v) => Err(pgss_too_old(&v)),
                Detect::Missing => Err(pgss_missing()),
            }
        })
    })
    .await
    .map_err(tool_error)
}

/// Окно между чтениями pgss: прогресс раз в секунду; `true` — клиент отменил вызов.
async fn wait_window(ctx: &ObserveCtx, started: Instant) -> bool {
    let total = u32::try_from(ctx.duration.as_secs()).unwrap_or(u32::MAX);
    let deadline = started + ctx.duration;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.tick().await;
    loop {
        tokio::select! {
            _ = ctx.cancel.cancelled() => return true,
            _ = tokio::time::sleep_until(deadline) => return false,
            _ = tick.tick() => {
                let done = u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX);
                (ctx.progress)(done.min(total), total);
            }
        }
    }
}

/// `observe_top_queries`: два чтения pgss (`showtext := false`) и разница в форме топа окна; тексты — только у строк
/// ответа (кеш текстов узла), на уровне `diagnostics` маскируются, в журнал — литералы отданных строк
/// (`read::mask_statements`, как у `get_top_queries`; поиска у наблюдения нет). Доступ и сортировка проверяются до
/// открытия подключения.
pub async fn observe_top_queries(
    st: &AppState,
    p: ObserveQueriesParams,
    ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    let (_, access) = check_connection(st, &p.connection_id)?;
    let sort = parse_sort(p.sort.as_deref())?;
    ensure_open(st, &p.connection_id).await?;
    let id = p.connection_id.as_str();
    let target = st
        .runtimes
        .diag_target(id, p.member.as_deref(), false)
        .map_err(tool_error)?;
    let (_, pgss, from) = read_statements(st, id, &target).await?;
    let started = Instant::now();
    let cancelled = wait_window(&ctx, started).await;
    let (head, _, to) = read_statements(st, id, &target).await?;
    let duration_s = u32::try_from(started.elapsed().as_secs())
        .unwrap_or(u32::MAX)
        .max(1);
    let observed = Observed {
        head,
        pgss,
        from,
        to,
        duration_s,
    };
    let limit = p.limit.unwrap_or(TOP_LIMIT) as usize;
    let q = QueriesParams {
        mode: QueriesMode::Window { s: duration_s },
        sort,
        limit,
        database: None,
        toplevel_only: true,
        search: None,
    };
    let mut report = report_observed(st, id, &target, &observed, &q)
        .await
        .map_err(tool_error)?;
    let masked = mask_statements(access, &mut report, None, Some(limit));
    let mut out = report_out(report);
    if cancelled {
        out.hint = Some(CANCELLED_HINT.to_string());
    }
    Ok(PgAnswer {
        body: PgOutput::ObservedQueries(out),
        masked,
        rows: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::error::AppError;

    fn sample(
        at_ms: i64,
        pid: i32,
        wait: Option<(&'static str, &'static str)>,
        query_id: i64,
        query: &'static str,
    ) -> Sample<'static> {
        Sample {
            at_ms,
            pid,
            leader_pid: None,
            wait_type: wait.map(|w| w.0),
            wait_event: wait.map(|w| w.1),
            query_id: Some(query_id),
            query,
            application: "app",
            user: "u",
            database: "orders",
            client: "",
        }
    }

    #[test]
    fn window_summary_counts_waits_queries_and_sessions_and_masks_texts() {
        const Q: &str = "SELECT * FROM t WHERE email = 'ann@example.com'";
        let lock = Some(("Lock", "relation"));
        let samples = [
            sample(500, 4, None, 10, "SELECT before_the_window()"),
            sample(1_000, 1, lock, 7, Q),
            sample(1_000, 3, lock, 9, "SELECT now()"),
            sample(2_000, 1, lock, 7, Q),
            sample(2_000, 2, None, 8, "SELECT clock_timestamp()"),
        ];
        let (o, masked) = summarize(&samples, &[500, 1_000, 2_000], 1_000, 2_000, true);
        assert_eq!(
            masked, 1,
            "текст маскируется один раз на запрос, а не на сэмпл"
        );
        assert_eq!(
            (
                o.samples,
                o.peak_sessions,
                o.peak_lock_waits,
                o.duration_ms,
                o.started_at_ms
            ),
            (2, 2, 2, 1_000, 1_000)
        );
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(
            v["waits"][0],
            json!({"waitEventType": "Lock", "waitEvent": "relation", "samples": 3, "share": 0.75})
        );
        assert_eq!(
            (
                &v["topQueries"][0]["queryId"],
                &v["topQueries"][0]["samples"]
            ),
            (&json!("7"), &json!(2))
        );
        let text = v["topQueries"][0]["query"].as_str().unwrap();
        assert!(
            !text.contains("ann@example.com") && text.contains("$1"),
            "{text}"
        );
        assert_eq!(
            v["topQueries"][0]["waits"][0],
            json!({"waitEventType": "Lock", "waitEvent": "relation", "samples": 2})
        );
        let pids = |k: &str| -> Vec<Value> {
            v[k].as_array()
                .unwrap()
                .iter()
                .map(|s| s["pid"].clone())
                .collect()
        };
        assert_eq!(pids("appeared"), [json!(2)]);
        assert_eq!(pids("disappeared"), [json!(3)]);
        assert_eq!(v["summary"]["topWait"], "Lock:relation");
        assert!(
            !v.to_string().contains("before_the_window"),
            "сэмпл до окна не считается"
        );
    }

    #[test]
    fn long_query_texts_are_cut() {
        let long: &'static str = Box::leak("x".repeat(MAX_QUERY_BYTES * 2).into_boxed_str());
        let (o, _) = summarize(&[sample(1, 1, None, 1, long)], &[1], 0, 10, false);
        let v = serde_json::to_value(&o).unwrap();
        assert_eq!(
            v["topQueries"][0]["query"].as_str().unwrap().len(),
            MAX_QUERY_BYTES
        );
    }

    // Сверх плана: порядок запросов не выдаёт литералы, ошибка сэмплера — без цели ssh.

    /// Запросы с равным числом сэмплов идут по показанному тексту: порядок исходных текстов сравнивал бы литералы
    /// (`'aaa'` < `'zzz'`), и модель узнала бы о них больше, чем `$1`.
    #[test]
    fn equal_queries_are_ordered_by_the_masked_text() {
        let samples = [
            sample(1, 1, None, 1, "SELECT * FROM t WHERE a = 'zzz' AND b = 1"),
            sample(1, 2, None, 2, "SELECT * FROM t WHERE a = 'aaa' AND c = 1"),
        ];
        let (o, masked) = summarize(&samples, &[1], 0, 10, true);
        let ids: Vec<&Value> = o.top_queries.iter().map(|q| &q["queryId"]).collect();
        assert_eq!(
            ids,
            [&json!("1"), &json!("2")],
            "«… b = $2» раньше «… c = $2»"
        );
        assert_eq!(masked, 4);
        assert_eq!(
            o.top_queries[0]["query"],
            "SELECT * FROM t WHERE a = $1 AND b = $2"
        );
    }

    #[tokio::test]
    async fn sampler_errors_reach_the_model_without_the_bastion() {
        use crate::net::ssh::process::{SshErrorKind, ssh_error};
        let (tx, mut rx) = unbounded_channel();
        let ctx = ctx_with(Arc::default());
        tx.send(SamplerEvent::Error {
            error: ssh_error(
                SshErrorKind::AuthFailed,
                "admin@bastion.corp.example",
                "admin@bastion.corp.example: Permission denied (publickey)",
            ),
        })
        .unwrap();
        tx.send(SamplerEvent::Stopped).unwrap();
        let w = watch(&mut rx, &ctx).await;
        let last = w.last_error.unwrap();
        assert!(
            last.starts_with("SSH_ERROR: ") && !last.contains("bastion"),
            "{last}"
        );
    }

    fn ctx_with(progress: ProgressCalls) -> ObserveCtx {
        ObserveCtx {
            duration: Duration::from_secs(30),
            cancel: CancellationToken::new(),
            progress: Arc::new(move |done: u32, total: u32| {
                progress.lock().unwrap().push((done, total))
            }),
        }
    }

    type ProgressCalls = Arc<Mutex<Vec<(u32, u32)>>>;

    #[tokio::test]
    async fn ticks_report_progress_and_cancel_ends_the_window_early() {
        let (tx, mut rx) = unbounded_channel();
        let calls: ProgressCalls = Arc::default();
        let ctx = ctx_with(calls.clone());
        let tick = |at_ms| SamplerEvent::Tick {
            at_ms,
            active: 1,
            waiting: 0,
        };
        tx.send(tick(1)).unwrap();
        tx.send(SamplerEvent::Error {
            error: AppError::internal_with("x"),
        })
        .unwrap();
        tx.send(tick(2)).unwrap();
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });
        let w = watch(&mut rx, &ctx).await;
        assert_eq!(
            (w.ticks, w.errors, w.cancelled, w.stopped),
            (2, 1, true, false)
        );
        // Ошибка сэмплера — в виде для модели (`access::tool_error`): подробности внутренней ошибки остаются в логе.
        let last = w.last_error.as_deref().unwrap();
        assert!(last.starts_with("internal: "), "{last}");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(_, total)| *total == 30));
    }

    #[tokio::test]
    async fn stopped_sampler_ends_the_window() {
        let (tx, mut rx) = unbounded_channel();
        let ctx = ctx_with(Arc::default());
        tx.send(SamplerEvent::Stopped).unwrap();
        let w = watch(&mut rx, &ctx).await;
        assert!(w.stopped && !w.cancelled);
    }

    #[tokio::test]
    async fn statements_window_reports_progress_and_stops_on_cancel() {
        let calls: ProgressCalls = Arc::default();
        let ctx = ctx_with(calls.clone());
        let cancel = ctx.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1_200)).await;
            cancel.cancel();
        });
        assert!(
            wait_window(&ctx, Instant::now()).await,
            "отмена завершила окно досрочно"
        );
        assert!(!calls.lock().unwrap().is_empty(), "прогресс раз в секунду");
    }
}
