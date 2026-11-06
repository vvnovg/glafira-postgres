//! Наблюдение MCP (спец. P4 §5.3): `observe_activity` подписывается на сэмплер активности узла на окно (тот же сэмплер,
//! что у вкладки: нагрузка не удваивается) и сводит его буфер за окно; `observe_top_queries` читает
//! `pg_stat_statements` (`showtext := false`) в начале и в конце окна и отдаёт разницу в форме топа. Прогресс — после
//! каждого тика сэмплера (у pgss — раз в секунду) через `ObserveCtx::progress`; отмена клиентом (`ObserveCtx::cancel`)
//! завершает окно досрочно с уже собранным. Одну длительную операцию на сессию и предел длительности держит крейт шлюза.
//!
//! Сроки. Ядро обрывает вызов через `durationSeconds + 6` единиц от создания `ObserveCtx` (`ObserveCtx::deadline`), и
//! собранное тогда теряется. Поэтому окно отсчитывается от начала вызова (`ObserveCtx::started`), а не от конца
//! подготовки: открытие подключения (туннель ssh, keychain) и первое чтение pgss входят в `durationSeconds`, остаток окна
//! сокращается, а последнему чтению pgss оставляется время до срока ядра (`window_end`). Окно короче запрошенного на
//! 1/10 и больше — ответ с собранным и подсказкой (как при отмене); окна не осталось вовсе (собирать нечего) — `TIMEOUT`
//! сразу (`rest_of_window`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use glafira_mcp_core::error::ToolError;
use glafira_pg_mcp::model::{ObserveCtx, ObservedActivity, PgAnswer, PgOutput};
use glafira_pg_mcp::params::{ObserveActivityParams, ObserveQueriesParams};
use pg_advisor::ash::Sample;
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;

use super::access::{check_connection, ensure_open, err, masks, open_for_llm, tool_error};
use super::mask::{MAX_QUERY_BYTES, cut_text, mask_text};
use super::read::{TOP_LIMIT, add_hint, mask_statements, member_or_leader, parse_sort, report_out};
use crate::conn::run_blocking;
use crate::diag::queries::{
    Detect, Observed, PgssInstalled, QueriesMode, QueriesParams, Snapshot, detect,
    ensure_observed_sort, read_cumulative, report_observed,
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
/// Имена инструментов в сообщениях модели.
const ACTIVITY_TOOL: &str = "observe_activity";
const STATEMENTS_TOOL: &str = "observe_top_queries";
/// Подсказка о коротком окне — если оно короче запрошенного на 1/`SHORTENED_SHARE` и больше.
const SHORTENED_SHARE: u32 = 10;

/// Тип и событие ожидания; `(None, None)` — CPU.
type WaitKey<'s> = (Option<&'s str>, Option<&'s str>);

/// Ключ запроса в топе: `query_id`, если он есть и не 0, иначе номер группы текста (`summarize`: на уровне
/// `diagnostics` — по замаскированному тексту).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum QueryKey {
    Id(i64),
    Text(usize),
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
/// выдавал бы сравнение литералов. Без `query_id` (PG 13, `compute_query_id = off`) запросы с маскированием сводятся по
/// замаскированному тексту: одна инструкция с разными литералами — одна строка (сэмплы и ожидания сложены), а не
/// строки с одинаковым текстом, которые занимали бы топ и выдавали число вариантов литералов. Второе значение — число
/// замаскированных литералов у отданных запросов.
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
    let mut queries: HashMap<QueryKey, QueryAcc<'_>> = HashMap::new();
    // Группы текстов без `query_id`: исходный текст → группа, ключ группы (замаскированный или исходный) → номер.
    let mut group_of: HashMap<&str, usize> = HashMap::new();
    let mut groups: HashMap<String, usize> = HashMap::new();
    for s in &samples {
        let key = match s.query_id {
            Some(id) if id != 0 => QueryKey::Id(id),
            _ => QueryKey::Text(*group_of.entry(s.query).or_insert_with(|| {
                let mut text = s.query.to_string();
                if mask {
                    mask_text(&mut text);
                }
                let next = groups.len();
                *groups.entry(text).or_insert(next)
            })),
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

/// Ход для клиента: секунд от начала вызова из `durationSeconds`.
fn report_progress(ctx: &ObserveCtx) {
    let total = u32::try_from(ctx.duration.as_secs()).unwrap_or(u32::MAX);
    let done = u32::try_from(ctx.started.elapsed().as_secs()).unwrap_or(u32::MAX);
    (ctx.progress)(done.min(total), total);
}

/// Конец окна наблюдения (спец. §5.3: вызов — не дольше `durationSeconds` + 6 с): `duration` от начала вызова
/// (`ObserveCtx::started`; подготовка — открытие подключения, туннель ssh, keychain, первое чтение pgss — входит в
/// окно), но не позже страховочного срока ядра (`ObserveCtx::deadline`) за вычетом `reserve` — времени шага после окна.
fn window_end(ctx: &ObserveCtx, reserve: Duration) -> Instant {
    let end = ctx.started + ctx.duration;
    match ctx.deadline.checked_sub(reserve) {
        Some(latest) => end.min(latest),
        None => ctx.started,
    }
}

/// Конец окна после подготовки (`window_end`); окна не осталось — `TIMEOUT` сразу: собранного нет, а ожидание до срока
/// ядра лишь отняло бы время. Подключение к этому моменту открыто — повторный вызов не тратит окно на открытие.
fn rest_of_window(ctx: &ObserveCtx, reserve: Duration, tool: &str) -> Result<Instant, ToolError> {
    let end = window_end(ctx, reserve);
    if end > Instant::now() {
        return Ok(end);
    }
    Err(err(
        "TIMEOUT",
        format!(
            "Preparing the observation (opening the connection, the first read) took {:.1} s and left no time of the {:.1} s window; call {tool} again: the connection is open now",
            ctx.started.elapsed().as_secs_f64(),
            ctx.duration.as_secs_f64()
        ),
    ))
}

/// Подсказка, если окно `window_start`…`end` короче запрошенного на 1/`SHORTENED_SHARE` и больше: часть
/// `durationSeconds` ушла на подготовку и запас для последнего чтения.
fn shortened_hint(ctx: &ObserveCtx, window_start: Instant, end: Instant) -> Option<String> {
    let lost = ctx
        .duration
        .saturating_sub(end.saturating_duration_since(window_start));
    (!lost.is_zero() && lost * SHORTENED_SHARE >= ctx.duration).then(|| {
        format!(
            "The observation window is {:.1} s shorter than requested: preparing it (opening the connection, the first read) and the final read had to fit into durationSeconds",
            lost.as_secs_f64()
        )
    })
}

/// Окно наблюдения активности: до `end` (`rest_of_window`), отмены клиентом или остановки сэмплера; прогресс — на
/// каждый тик.
async fn watch(rx: &mut UnboundedReceiver<SamplerEvent>, ctx: &ObserveCtx, end: Instant) -> Watch {
    let mut w = Watch::default();
    loop {
        tokio::select! {
            _ = ctx.cancel.cancelled() => {
                w.cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(end) => break,
            e = rx.recv() => match e {
                Some(SamplerEvent::Tick { .. }) => {
                    w.ticks += 1;
                    report_progress(ctx);
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

/// `observe_activity`: подписка на сэмплер узла на остаток окна от начала вызова (открытие подключения уже заняло его
/// часть; после окна — только сводка буфера в памяти, запас до срока ядра ей не нужен), затем сводка его буфера за
/// окно; тексты на уровне `diagnostics` маскируются (`mask::mask_text`, без чтения `standard_conforming_strings`).
pub async fn observe_activity(
    st: &AppState,
    p: ObserveActivityParams,
    ctx: ObserveCtx,
) -> Result<PgAnswer, ToolError> {
    let access = open_for_llm(st, &p.connection_id).await?;
    let mask = masks(access);
    let id = p.connection_id.as_str();
    let member = member_or_leader(st, id, p.member.as_deref())?;
    let end = rest_of_window(&ctx, Duration::ZERO, ACTIVITY_TOOL)?;
    let (tx, mut rx) = unbounded_channel();
    let window_start = Instant::now();
    let started_at_ms = now_ms();
    let subscription = Subscription {
        sessions: &st.sessions,
        id: crate::sampler::start(st, id, Some(&member), Arc::new(ChannelSink(tx)))
            .map_err(tool_error)?,
    };
    let w = watch(&mut rx, &ctx, end).await;
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
    if let Some(h) = shortened_hint(&ctx, window_start, end) {
        out.summary["hint"] = json!(h);
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

/// Окно между чтениями pgss до `end`: прогресс раз в секунду; `true` — клиент отменил вызов.
async fn wait_window(ctx: &ObserveCtx, end: Instant) -> bool {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.tick().await;
    loop {
        tokio::select! {
            _ = ctx.cancel.cancelled() => return true,
            _ = tokio::time::sleep_until(end) => return false,
            _ = tick.tick() => report_progress(ctx),
        }
    }
}

/// Два чтения вокруг окна (`around_window`).
#[derive(Debug)]
struct Around<S> {
    from: S,
    to: S,
    /// От конца первого чтения до конца второго — покрытие разницы.
    between: Duration,
    cancelled: bool,
    /// Окно заметно короче запрошенного (`shortened_hint`).
    hint: Option<String>,
}

/// Первое чтение; проверка по нему (`check`: сортировка — сразу, а не после окна); окно до `rest_of_window` с запасом
/// на второе чтение вдвое больше первого (на нагруженном сервере второе может идти дольше; обычно запас тонет в
/// страховочных 6 с ядра и окно не сокращает); второе чтение. Окна не осталось — `TIMEOUT` без второго чтения.
async fn around_window<S, F, Fut>(
    ctx: &ObserveCtx,
    mut read: F,
    check: impl FnOnce(&S) -> Result<(), ToolError>,
) -> Result<Around<S>, ToolError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<S, ToolError>>,
{
    let t = Instant::now();
    let from = read().await?;
    check(&from)?;
    let first = Instant::now();
    let end = rest_of_window(ctx, (first - t) * 2, STATEMENTS_TOOL)?;
    let cancelled = wait_window(ctx, end).await;
    let to = read().await?;
    Ok(Around {
        from,
        to,
        between: first.elapsed(),
        cancelled,
        hint: shortened_hint(ctx, first, end),
    })
}

/// `observe_top_queries`: два чтения pgss (`showtext := false`) вокруг остатка окна от начала вызова (`around_window`)
/// и разница в форме топа окна; тексты — только у строк ответа (кеш текстов узла), на уровне `diagnostics`
/// маскируются, в журнал — литералы отданных строк (`read::mask_statements`, как у `get_top_queries`; поиска у
/// наблюдения нет). Доступ и имя сортировки проверяются до открытия подключения, её доступность на версии pgss — сразу
/// после первого чтения (`ensure_observed_sort`).
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
    let around = around_window(
        &ctx,
        || read_statements(st, id, &target),
        |(_, pgss, _)| ensure_observed_sort(sort, pgss).map_err(tool_error),
    )
    .await?;
    let ((_, pgss, from), (head, _, to)) = (around.from, around.to);
    // Секунды окна — до ближайшей: окно от начала вызова короче `durationSeconds` на подготовку (миллисекунды при
    // открытом подключении), и отбрасывание дробной части занижало бы его на секунду.
    let duration_s = u32::try_from((around.between.as_millis() + 500) / 1000)
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
    for hint in [
        around.cancelled.then_some(CANCELLED_HINT),
        around.hint.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        add_hint(&mut out.hint, hint);
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

    use glafira_pg_mcp::params::OBSERVE_GRACE_S;
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

    /// Без `query_id` (PG 13, `compute_query_id = off`) запросы на уровне `diagnostics` сводятся по замаскированному
    /// тексту: одна инструкция с разными литералами — одна строка топа (сэмплы сложены), а не строки с одинаковым
    /// текстом, выдающие число вариантов литералов.
    #[test]
    fn queries_without_an_id_are_grouped_by_the_masked_text() {
        let no_id = |at_ms, pid, query| Sample {
            query_id: None,
            ..sample(at_ms, pid, None, 0, query)
        };
        let samples = [
            no_id(1, 1, "SELECT * FROM t WHERE email = 'ann@example.com'"),
            no_id(1, 2, "SELECT * FROM t WHERE email = 'bob@example.com'"),
            no_id(2, 1, "SELECT * FROM t WHERE email = 'cid@example.com'"),
            no_id(2, 3, "SELECT now()"),
        ];
        let (o, masked) = summarize(&samples, &[1, 2], 0, 10, true);
        assert_eq!(o.top_queries.len(), 2, "{:?}", o.top_queries);
        assert_eq!(
            (&o.top_queries[0]["query"], &o.top_queries[0]["samples"]),
            (&json!("SELECT * FROM t WHERE email = $1"), &json!(3))
        );
        assert_eq!(o.top_queries[0]["queryId"], Value::Null);
        assert_eq!(masked, 1, "литералы — у отданного текста строки");
        // Без маскирования (уровень выше `diagnostics`) — по исходному тексту, как раньше.
        let (o, masked) = summarize(&samples, &[1, 2], 0, 10, false);
        assert_eq!((o.top_queries.len(), masked), (4, 0));
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
        let w = watch(&mut rx, &ctx, window_end(&ctx, Duration::ZERO)).await;
        let last = w.last_error.unwrap();
        assert!(
            last.starts_with("SSH_ERROR: ") && !last.contains("bastion"),
            "{last}"
        );
    }

    fn ctx_with(progress: ProgressCalls) -> ObserveCtx {
        ObserveCtx {
            progress: Arc::new(move |done: u32, total: u32| {
                progress.lock().unwrap().push((done, total))
            }),
            ..ObserveCtx::new(Duration::from_secs(30))
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
        let w = watch(&mut rx, &ctx, window_end(&ctx, Duration::ZERO)).await;
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
        let w = watch(&mut rx, &ctx, window_end(&ctx, Duration::ZERO)).await;
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
            wait_window(&ctx, window_end(&ctx, Duration::ZERO)).await,
            "отмена завершила окно досрочно"
        );
        assert!(!calls.lock().unwrap().is_empty(), "прогресс раз в секунду");
    }

    // Окно от начала вызова (страховочный срок ядра — `durationSeconds + 6` единиц от создания `ObserveCtx`).

    /// Единица времени тестов, как `with_time_unit` ядра: окно — 30 единиц, срок ядра — ещё `OBSERVE_GRACE_S`.
    const UNIT: Duration = Duration::from_millis(10);
    const WINDOW_UNITS: u32 = 30;

    /// Контекст, каким его создаёт ядро шлюза в тестовой единице времени.
    fn unit_ctx() -> ObserveCtx {
        let started = Instant::now();
        ObserveCtx {
            duration: UNIT * WINDOW_UNITS,
            started,
            deadline: started + UNIT * (WINDOW_UNITS + OBSERVE_GRACE_S),
            cancel: CancellationToken::new(),
            progress: Arc::new(|_, _| {}),
        }
    }

    /// Медленная подготовка (холодное открытие: туннель ssh, keychain) входит в окно: оно кончается через `duration`
    /// от начала вызова, а не после подготовки, и вызов укладывается в срок ядра.
    #[tokio::test(start_paused = true)]
    async fn activity_window_is_counted_from_the_start_of_the_call() {
        let ctx = unit_ctx();
        tokio::time::sleep(UNIT * 25).await;
        let end = rest_of_window(&ctx, Duration::ZERO, ACTIVITY_TOOL).unwrap();
        assert_eq!(end, ctx.started + ctx.duration);
        let (_tx, mut rx) = unbounded_channel();
        let w = watch(&mut rx, &ctx, end).await;
        assert!(!w.cancelled && !w.stopped);
        assert_eq!(ctx.started.elapsed(), UNIT * WINDOW_UNITS);
        assert!(Instant::now() < ctx.deadline);
        let hint = shortened_hint(&ctx, ctx.started + UNIT * 25, end).unwrap();
        assert!(hint.contains("shorter than requested"), "{hint}");
        assert_eq!(
            shortened_hint(&ctx, ctx.started + UNIT, end),
            None,
            "почти полное окно — без подсказки"
        );
    }

    /// Подготовка заняла всё окно: собранного нет — `TIMEOUT` сразу, без ожидания до срока ядра.
    #[tokio::test(start_paused = true)]
    async fn preparation_that_took_the_whole_window_is_a_timeout() {
        let ctx = unit_ctx();
        tokio::time::sleep(UNIT * (WINDOW_UNITS + 1)).await;
        let e = rest_of_window(&ctx, Duration::ZERO, ACTIVITY_TOOL).unwrap_err();
        assert_eq!(e.code, "TIMEOUT");
        assert!(
            e.message.contains("observe_activity again"),
            "{}",
            e.message
        );
    }

    /// Чтение pgss: `delay` единиц, итог — номер чтения.
    fn slow_read(
        calls: Arc<Mutex<u32>>,
        delay: u32,
    ) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<u32, ToolError>>>>
    {
        move || {
            let calls = calls.clone();
            Box::pin(async move {
                tokio::time::sleep(UNIT * delay).await;
                let mut n = calls.lock().unwrap();
                *n += 1;
                Ok(*n)
            })
        }
    }

    /// `observe_top_queries` после медленного открытия: окно — остаток от начала вызова, второе чтение — до срока ядра
    /// (раньше: открытие + первое чтение + полное окно + второе чтение — дальше срока, и собранное терялось).
    #[tokio::test(start_paused = true)]
    async fn statements_window_after_a_slow_open_ends_before_the_deadline() {
        let ctx = unit_ctx();
        tokio::time::sleep(UNIT * 20).await;
        let calls = Arc::new(Mutex::new(0));
        let r = around_window(&ctx, slow_read(calls.clone(), 2), |_| Ok(()))
            .await
            .unwrap();
        assert_eq!((r.from, r.to, r.cancelled), (1, 2, false));
        assert_eq!(ctx.started.elapsed(), UNIT * (WINDOW_UNITS + 2));
        assert!(Instant::now() <= ctx.deadline);
        // Между чтениями: от конца первого (22) до конца второго (32).
        assert_eq!(r.between, UNIT * 10);
        assert!(r.hint.is_some(), "окно короче запрошенного — с подсказкой");
    }

    /// Чтение pgss дольше страховочного запаса: окно кончается раньше, чтобы второе чтение успело до срока ядра.
    #[tokio::test(start_paused = true)]
    async fn slow_reads_shorten_the_window_to_leave_time_for_the_final_read() {
        let ctx = unit_ctx();
        let calls = Arc::new(Mutex::new(0));
        let r = around_window(&ctx, slow_read(calls, 8), |_| Ok(()))
            .await
            .unwrap();
        assert_eq!(r.to, 2);
        assert!(
            Instant::now() <= ctx.deadline,
            "второе чтение — до срока ядра: {:?} > {:?}",
            ctx.started.elapsed(),
            ctx.deadline - ctx.started
        );
        assert!(r.hint.is_some());
    }

    /// Недоступная сортировка — ошибка сразу после первого чтения, а не после окна.
    #[tokio::test(start_paused = true)]
    async fn unavailable_sort_fails_right_after_the_first_read() {
        let ctx = unit_ctx();
        let calls = Arc::new(Mutex::new(0));
        let e = around_window(&ctx, slow_read(calls.clone(), 2), |_| {
            Err(crate::mcp::access::invalid_argument("no such sort"))
        })
        .await
        .unwrap_err();
        assert_eq!(e.code, "invalid_argument");
        assert_eq!(ctx.started.elapsed(), UNIT * 2);
        assert_eq!(*calls.lock().unwrap(), 1, "второго чтения нет");
    }

    /// Первое чтение съело всё окно: `TIMEOUT` без второго чтения.
    #[tokio::test(start_paused = true)]
    async fn first_read_that_took_the_whole_window_is_a_timeout() {
        let ctx = unit_ctx();
        tokio::time::sleep(UNIT * 25).await;
        let calls = Arc::new(Mutex::new(0));
        let e = around_window(&ctx, slow_read(calls.clone(), 6), |_| Ok(()))
            .await
            .unwrap_err();
        assert_eq!(e.code, "TIMEOUT");
        assert!(
            e.message.contains("observe_top_queries again"),
            "{}",
            e.message
        );
        assert_eq!(*calls.lock().unwrap(), 1);
    }
}
