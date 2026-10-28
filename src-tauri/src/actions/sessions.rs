//! Отмена запроса и завершение сессий (спец. P2 §8.3): предпросмотр (задача 3) и выполнение (задача 4).

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;
use tokio_postgres::error::SqlState;

use crate::actions::Confirmation;
use crate::actions::journal::{self, ActionRecord, RecordConnection, SessionRecord};
use crate::conn::profile::Environment;
use crate::diag::caps::{Caps, V14};
use crate::diag::sql::BACKEND_START_US;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::guard::{ensure_mutation_allowed, ensure_name_confirmed};
use crate::state::AppState;

/// Больше сессий за одно действие не принимается.
pub const MAX_BATCH: usize = 500;
/// Общий предел времени пакета: оставшиеся сессии получают `failed` (`errors.actions.batchTimeout`).
pub const BATCH_LIMIT: Duration = Duration::from_secs(60);
/// Сколько после `pg_terminate_backend` ждать исчезновения сессии.
pub const GONE_WAIT: Duration = Duration::from_secs(5);
/// Как часто в это время проверять, что сессия исчезла.
pub const GONE_POLL: Duration = Duration::from_millis(250);
/// Клиентский срок ответа на один запрос к серверу: при обрыве сети запрос иначе ждал бы TCP keepalive, а
/// серверный `statement_timeout` соединения действия (5 с) тут не поможет. Больше него на запас сети.
pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignalKind {
    Cancel,
    Terminate,
}

/// Сессия адресуется парой `(pid, backend_start)`: PID переиспользуется, время запуска — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub pid: i32,
    pub backend_start_us: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionTarget {
    Selected {
        sessions: Vec<SessionRef>,
    },
    #[serde(rename_all = "camelCase")]
    IdleInTransaction {
        older_than_s: f64,
    },
    #[serde(rename_all = "camelCase")]
    RootBlocker {
        pid: i32,
        backend_start_us: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionRequest {
    pub member: String,
    pub kind: SignalKind,
    pub target: SessionTarget,
}

/// Почему сессию нельзя отменить или завершить (видно в строке предпросмотра и подсказке кнопки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DenyReason {
    /// Сессия суперпользователя: нужна роль суперпользователя.
    SuperuserSession,
    /// Чужая роль и нет `pg_signal_backend`.
    OtherRole,
    NotClientBackend,
    /// Соединение самого приложения.
    OwnConnection,
    /// Отмена сессии, которая не выполняет запрос: сервер её молча игнорирует, транзакция остаётся.
    NotRunning,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSession {
    pub pid: i32,
    pub backend_start_us: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub state: Option<String>,
    pub backend_type: Option<String>,
    pub xact_age_s: Option<f64>,
    pub state_age_s: Option<f64>,
    /// Первые 500 символов текста запроса.
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub session: PreviewSession,
    pub allowed: bool,
    pub reason: Option<DenyReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionPreview {
    /// Узел, к которому относится предпросмотр: выполнение принимает именно его.
    pub member: String,
    pub rows: Vec<PreviewRow>,
}

/// Факты прав для строки предпросмотра (из того же запроса).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivFacts {
    pub client: bool,
    pub own: bool,
    pub session_super: bool,
    pub same_role: bool,
    pub me_super: bool,
    pub signal: bool,
}

/// Правило PostgreSQL для `pg_cancel_backend` / `pg_terminate_backend`: суперпользователь — любую клиентскую сессию;
/// член роли сессии — её; член `pg_signal_backend` — любую сессию не суперпользователя.
pub fn deny_reason(f: &PrivFacts) -> Option<DenyReason> {
    if !f.client {
        return Some(DenyReason::NotClientBackend);
    }
    if f.own {
        return Some(DenyReason::OwnConnection);
    }
    if f.me_super || f.same_role {
        return None;
    }
    if f.session_super {
        return Some(DenyReason::SuperuserSession);
    }
    if f.signal {
        None
    } else {
        Some(DenyReason::OtherRole)
    }
}

/// Сессия выполняет запрос — отменять есть что. `pg_cancel_backend` у сессии, ждущей команду клиента (`idle`,
/// `idle in transaction`), сервер молча игнорирует. То же условие повторяет `signal_sql` для отмены.
pub fn is_running(state: Option<&str>) -> bool {
    matches!(state, Some("active" | "fastpath function call"))
}

/// Причина отказа строки предпросмотра: сначала права, затем — для отмены — выполняется ли запрос.
pub fn row_reason(kind: SignalKind, f: &PrivFacts, state: Option<&str>) -> Option<DenyReason> {
    deny_reason(f).or_else(|| {
        (kind == SignalKind::Cancel && !is_running(state)).then_some(DenyReason::NotRunning)
    })
}

/// Условие выбора строк по цели: `$1` — массив pid (`selected`), порог в секундах (`idleInTransaction`) или pid.
fn target_filter(target: &SessionTarget) -> &'static str {
    match target {
        SessionTarget::Selected { .. } => "a.pid = ANY($1)",
        SessionTarget::IdleInTransaction { .. } => {
            "a.backend_type = 'client backend' AND a.state IN ('idle in transaction', 'idle in transaction (aborted)') \
             AND a.state_change <= now() - make_interval(secs => $1)"
        }
        SessionTarget::RootBlocker { .. } => "a.pid = $1",
    }
}

pub fn preview_sql(target: &SessionTarget) -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.datname::text, a.application_name, host(a.client_addr), \
                a.state, a.backend_type, extract(epoch FROM now() - a.xact_start)::float8, \
                extract(epoch FROM now() - a.state_change)::float8, left(a.query, 500), \
                a.backend_type = 'client backend', a.pid = pg_backend_pid(), COALESCE(r.rolsuper, false), \
                COALESCE(pg_has_role(current_user, a.usesysid, 'MEMBER'), false), \
                (SELECT rolsuper FROM pg_roles WHERE rolname = current_user), \
                pg_has_role(current_user, 'pg_signal_backend', 'MEMBER') \
         FROM pg_stat_activity a LEFT JOIN pg_roles r ON r.oid = a.usesysid \
         WHERE {} ORDER BY a.pid",
        target_filter(target)
    )
}

async fn query_preview(
    c: &Client,
    kind: SignalKind,
    target: &SessionTarget,
) -> AppResult<Vec<PreviewRow>> {
    let sql = preview_sql(target);
    let rows = match target {
        SessionTarget::Selected { sessions } => {
            let pids: Vec<i32> = sessions.iter().map(|s| s.pid).collect();
            c.query(&sql, &[&pids]).await
        }
        SessionTarget::IdleInTransaction { older_than_s } => c.query(&sql, &[older_than_s]).await,
        SessionTarget::RootBlocker { pid, .. } => c.query(&sql, &[pid]).await,
    }
    .map_err(|e| crate::conn::errors::from_pg(&e))?;
    let wanted = |pid: i32, start: i64| match target {
        SessionTarget::Selected { sessions } => sessions
            .iter()
            .any(|s| s.pid == pid && s.backend_start_us == start),
        SessionTarget::IdleInTransaction { .. } => true,
        SessionTarget::RootBlocker {
            pid: p,
            backend_start_us,
        } => *p == pid && *backend_start_us == start,
    };
    Ok(rows
        .iter()
        .filter_map(|r| {
            // Чужую сессию без права её видеть (нет `pg_read_all_stats`) сервер отдаёт с пустым `backend_start`:
            // адресовать её парой (pid, backend_start) нельзя, в предпросмотр она не попадает.
            let start = r.get::<_, Option<i64>>(1)?;
            if !wanted(r.get(0), start) {
                return None;
            }
            let f = PrivFacts {
                client: r.get(11),
                own: r.get(12),
                session_super: r.get(13),
                same_role: r.get(14),
                me_super: r.get::<_, Option<bool>>(15).unwrap_or(false),
                signal: r.get(16),
            };
            let reason = row_reason(kind, &f, r.get::<_, Option<&str>>(6));
            Some(PreviewRow {
                session: PreviewSession {
                    pid: r.get(0),
                    backend_start_us: start,
                    user: r.get(2),
                    database: r.get(3),
                    application: r.get(4),
                    client: r.get(5),
                    state: r.get(6),
                    backend_type: r.get(7),
                    xact_age_s: r.get(8),
                    state_age_s: r.get(9),
                    query: r.get(10),
                },
                allowed: reason.is_none(),
                reason,
            })
        })
        .collect())
}

/// Заново читает сессии цели на узле `member` (соединение каталога: предпросмотр ничего не меняет).
pub async fn preview(
    st: &AppState,
    id: &str,
    req: &SessionActionRequest,
) -> AppResult<SessionActionPreview> {
    ensure_valid_target(&req.target)?;
    let target = st
        .runtimes
        .diag_target(id, Some(req.member.as_str()), false)?;
    let (kind, t) = (req.kind, req.target.clone());
    let rows = crate::diag::on_member(st, id, &target, move |c| {
        Box::pin(async move { query_preview(c, kind, &t).await })
    })
    .await?;
    Ok(SessionActionPreview {
        member: req.member.clone(),
        rows,
    })
}

/// Запрос выполнения: сессии, показанные в предпросмотре и разрешённые пользователем, и критерий цели для проверки.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionExecute {
    pub member: String,
    pub kind: SignalKind,
    pub target: SessionTarget,
    /// Сессии, показанные в предпросмотре и разрешённые пользователем.
    pub sessions: Vec<SessionRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum SessionOutcome {
    /// После terminate сессия с тем же `backend_start` исчезла за 5 с.
    Terminated,
    /// Отмена отправлена, или после terminate сессия ещё жива через 5 с.
    SignalSent,
    /// Сессии с таким `backend_start` уже нет.
    Gone,
    /// Сессия есть, но критерий цели больше не выполняется (проснулась, отпустила блокировки).
    NoLongerMatches,
    /// 42501: прав на сигнал нет.
    NotAllowed,
    Failed {
        error: AppError,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub pid: i32,
    pub backend_start_us: i64,
    #[serde(flatten)]
    pub outcome: SessionOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionResult {
    pub member: String,
    pub results: Vec<SessionResult>,
    pub outcome: journal::Outcome,
    /// `false` — действие выполнено, но запись в журнал не удалась.
    pub journal_written: bool,
}

/// Критерий цели, который проверяется заново в том же запросе, что и сигнал (`$3` — порог в секундах).
fn criterion(target: &SessionTarget) -> &'static str {
    match target {
        SessionTarget::Selected { .. } => "TRUE",
        SessionTarget::IdleInTransaction { .. } => {
            "a.state IN ('idle in transaction', 'idle in transaction (aborted)') \
             AND a.state_change <= now() - make_interval(secs => $3)"
        }
        SessionTarget::RootBlocker { .. } => {
            "EXISTS (SELECT 1 FROM pg_stat_activity w WHERE w.wait_event_type = 'Lock' \
             AND a.pid = ANY(pg_blocking_pids(w.pid)))"
        }
    }
}

/// Сигнал сессии одним запросом с повторной проверкой критерия (спец. §8.3): строка есть — сигнал отправлен.
/// `$1` — pid, `$2` — `backend_start` в микросекундах, `$3` — порог (только для `idleInTransaction`).
/// Отмена ещё раз проверяет, что запрос выполняется (`is_running`): закончившийся — `noLongerMatches`.
pub fn signal_sql(kind: SignalKind, target: &SessionTarget, caps: Caps) -> String {
    let (func, running) = match kind {
        SignalKind::Cancel => (
            "pg_cancel_backend",
            " AND a.state IN ('active', 'fastpath function call')",
        ),
        SignalKind::Terminate => ("pg_terminate_backend", ""),
    };
    let query_id = caps.col(V14, "a.query_id::text", "text");
    format!(
        "SELECT a.usename::text, a.datname::text, a.application_name, host(a.client_addr), a.state, {query_id}, \
                left(a.query, 500), extract(epoch FROM now() - a.xact_start)::float8, {func}(a.pid) \
         FROM pg_stat_activity a \
         WHERE a.pid = $1 AND {BACKEND_START_US} = $2 AND a.backend_type = 'client backend' \
           AND a.pid <> pg_backend_pid() AND {}{running}",
        criterion(target)
    )
}

fn exists_sql() -> String {
    format!("SELECT 1 FROM pg_stat_activity a WHERE a.pid = $1 AND {BACKEND_START_US} = $2")
}

/// Итог пакета для журнала: отказ в правах и сбой считаются неудачей сессии; `gone` и `noLongerMatches` — нет.
pub fn batch_outcome(results: &[SessionResult]) -> journal::Outcome {
    let failed = results
        .iter()
        .filter(|r| {
            matches!(
                r.outcome,
                SessionOutcome::NotAllowed | SessionOutcome::Failed { .. }
            )
        })
        .count();
    match failed {
        0 => journal::Outcome::Ok,
        n if n == results.len() => journal::Outcome::Error,
        _ => journal::Outcome::Partial,
    }
}

fn result_name(o: &SessionOutcome) -> &'static str {
    match o {
        SessionOutcome::Terminated => "terminated",
        SessionOutcome::SignalSent => "signalSent",
        SessionOutcome::Gone => "gone",
        SessionOutcome::NoLongerMatches => "noLongerMatches",
        SessionOutcome::NotAllowed => "notAllowed",
        SessionOutcome::Failed { .. } => "failed",
    }
}

/// Сессия с той же парой ещё есть. Ошибка — сервера или истёкший клиентский срок (`timed`).
async fn still_there(c: &Client, s: SessionRef) -> AppResult<bool> {
    let rows = timed(c.query(&exists_sql(), &[&s.pid, &s.backend_start_us]))
        .await?
        .map_err(|e| crate::conn::errors::from_pg(&e))?;
    Ok(!rows.is_empty())
}

fn failed(e: &tokio_postgres::Error) -> SessionOutcome {
    SessionOutcome::Failed {
        error: crate::conn::errors::from_pg(e),
    }
}

/// Пустая строка журнала для сессии, по которой не прочитан снимок (сигнала не было): только пара и итог.
fn blank_record(s: SessionRef) -> SessionRecord {
    SessionRecord {
        pid: s.pid,
        backend_start_us: s.backend_start_us,
        user: None,
        database: None,
        application: None,
        client: None,
        state: None,
        query_id: None,
        query: None,
        xact_age_s: None,
        result: String::new(),
    }
}

/// Итог сразу после сигнала. `false` от `pg_cancel_backend` / `pg_terminate_backend` — процесса уже нет, сигнал
/// не доставлен (`gone`); доставленная отмена — `signalSent`; после доставленного завершения нужно дождаться
/// исчезновения сессии (`None`).
fn outcome_after_signal(kind: SignalKind, delivered: bool) -> Option<SessionOutcome> {
    match (delivered, kind) {
        (false, _) => Some(SessionOutcome::Gone),
        (true, SignalKind::Cancel) => Some(SessionOutcome::SignalSent),
        (true, SignalKind::Terminate) => None,
    }
}

/// Ждёт после terminate исчезновения сессии с тем же `backend_start`: до `GONE_WAIT`, опрос раз в `GONE_POLL`.
/// Ошибка сервера при опросе — `signalSent` (сигнал доставлен); опрос без ответа в срок — `failed`: соединение
/// непригодно, и пакет останавливается.
async fn wait_gone(c: &Client, s: SessionRef) -> SessionOutcome {
    let deadline = Instant::now() + GONE_WAIT;
    loop {
        match still_there(c, s).await {
            Ok(false) => break SessionOutcome::Terminated,
            Ok(true) if Instant::now() >= deadline => break SessionOutcome::SignalSent,
            Ok(true) => tokio::time::sleep(GONE_POLL).await,
            Err(error) if timed_out(&error) => break SessionOutcome::Failed { error },
            Err(_) => break SessionOutcome::SignalSent,
        }
    }
}

/// Сигнал одной сессии с перепроверкой критерия; итог и снимок сессии (если строка была прочитана).
async fn attempt(
    c: &Client,
    sql: &str,
    kind: SignalKind,
    target: &SessionTarget,
    s: SessionRef,
) -> (SessionOutcome, Option<SessionRecord>) {
    let query = async {
        match target {
            SessionTarget::IdleInTransaction { older_than_s } => {
                c.query(sql, &[&s.pid, &s.backend_start_us, older_than_s])
                    .await
            }
            _ => c.query(sql, &[&s.pid, &s.backend_start_us]).await,
        }
    };
    let rows = match timed(query).await {
        Ok(rows) => rows,
        Err(error) => return (SessionOutcome::Failed { error }, None),
    };
    let row = match rows {
        Ok(rows) => rows.into_iter().next(),
        Err(e) if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) => {
            return (SessionOutcome::NotAllowed, None);
        }
        Err(e) => return (failed(&e), None),
    };
    let Some(row) = row else {
        // Сигнал не отправлен: сессии нет или она больше не подходит под цель.
        return match still_there(c, s).await {
            Ok(true) => (SessionOutcome::NoLongerMatches, None),
            Ok(false) => (SessionOutcome::Gone, None),
            Err(error) => (SessionOutcome::Failed { error }, None),
        };
    };
    let record = SessionRecord {
        user: row.get(0),
        database: row.get(1),
        application: row.get(2),
        client: row.get(3),
        state: row.get(4),
        query_id: row.get(5),
        query: row
            .get::<_, Option<String>>(6)
            .map(|q| journal::cut(&q, journal::QUERY_CHARS)),
        xact_age_s: row.get(7),
        ..blank_record(s)
    };
    let delivered = row.get::<_, Option<bool>>(8).unwrap_or(false);
    let outcome = match outcome_after_signal(kind, delivered) {
        Some(o) => o,
        None => wait_gone(c, s).await,
    };
    (outcome, Some(record))
}

/// Одна сессия пакета: итог и строка журнала. Строка есть у каждой сессии; без снимка (сигнала не было, строка
/// не прочитана) в ней только пара и итог.
async fn one(
    c: &Client,
    sql: &str,
    kind: SignalKind,
    target: &SessionTarget,
    s: SessionRef,
) -> (SessionOutcome, SessionRecord) {
    let (outcome, snapshot) = attempt(c, sql, kind, target, s).await;
    let mut record = snapshot.unwrap_or_else(|| blank_record(s));
    record.result = result_name(&outcome).to_string();
    (outcome, record)
}

/// Порог возраста должен быть конечным неотрицательным числом секунд: отрицательный дал бы «будущее» время,
/// `NaN` и бесконечность сервер отвергает ошибкой на каждой сессии.
fn ensure_valid_target(target: &SessionTarget) -> AppResult<()> {
    match target {
        SessionTarget::IdleInTransaction { older_than_s }
            if !older_than_s.is_finite() || *older_than_s < 0.0 =>
        {
            Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.actions.badThreshold",
                "The age threshold must be a non-negative number of seconds"
            ))
        }
        _ => Ok(()),
    }
}

/// Сессии запроса: не пусто; для `rootBlocker` — ровно пара самой цели, для `selected` — только пары из цели (иначе
/// сигнал ушёл бы чужим сессиям).
fn ensure_sessions_match(target: &SessionTarget, sessions: &[SessionRef]) -> AppResult<()> {
    if sessions.is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.noSessions",
            "No sessions to act on"
        ));
    }
    let matches = match target {
        SessionTarget::RootBlocker {
            pid,
            backend_start_us,
        } => {
            sessions
                == [SessionRef {
                    pid: *pid,
                    backend_start_us: *backend_start_us,
                }]
        }
        SessionTarget::Selected { sessions: chosen } => sessions.iter().all(|s| chosen.contains(s)),
        SessionTarget::IdleInTransaction { .. } => true,
    };
    if matches {
        Ok(())
    } else {
        Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.badTarget",
            "The sessions do not match the action target"
        ))
    }
}

/// Убирает повторы пары `(pid, backend_start)`; порядок первых вхождений сохраняется.
pub fn dedup_sessions(sessions: &[SessionRef]) -> Vec<SessionRef> {
    let mut seen = HashSet::new();
    sessions
        .iter()
        .copied()
        .filter(|s| seen.insert(*s))
        .collect()
}

/// Код ошибки действия для журнала при `outcome: error`: код первой сессии с отказом (`NOT_AUTHORIZED`) или сбоем.
pub fn batch_code(results: &[SessionResult], outcome: journal::Outcome) -> Option<String> {
    if outcome != journal::Outcome::Error {
        return None;
    }
    results
        .iter()
        .find_map(|r| match &r.outcome {
            SessionOutcome::NotAllowed => Some(ErrorCode::NotAuthorized),
            SessionOutcome::Failed { error } => Some(error.code),
            _ => None,
        })
        .map(journal::code_name)
}

fn statement_timeout() -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.actions.statementTimeout",
        "The server did not answer within {{secs}} s",
        secs = STATEMENT_TIMEOUT.as_secs()
    )
}

fn batch_timeout() -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.actions.batchTimeout",
        "The batch did not finish within {{secs}} s",
        secs = BATCH_LIMIT.as_secs()
    )
}

fn batch_stopped() -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.actions.batchStopped",
        "Not attempted: the batch was stopped after a timeout"
    )
}

/// Обращение к серверу под клиентским сроком `STATEMENT_TIMEOUT`: не ответил — `errors.actions.statementTimeout`.
async fn timed<T>(fut: impl Future<Output = T>) -> Result<T, AppError> {
    tokio::time::timeout(STATEMENT_TIMEOUT, fut)
        .await
        .map_err(|_| statement_timeout())
}

/// Сервер не ответил в срок: соединение действия после зависшего запроса непригодно.
fn timed_out(e: &AppError) -> bool {
    e.key == "errors.actions.statementTimeout"
}

/// Сессии пакета по одной: `run_one` — сигнал одной сессии. Перед каждой сессией по порядку: подключение
/// закрыто или открыто заново (`still_open` — `false`) — эта и оставшиеся `errors.topology.notConnected`; вышел
/// `BATCH_LIMIT` — `batchTimeout`; прошлая сессия не дождалась ответа сервера — `batchStopped`. Такая сессия
/// получает `failed` и строку журнала без снимка, запроса к серверу по ней нет. `run_one` — замыкание,
/// возвращающее future, а не `AsyncFnMut`: future асинхронного замыкания компилятор не может доказать `Send`, а
/// его требует команда Tauri.
async fn run_batch<F, Fut>(
    sessions: &[SessionRef],
    started: Instant,
    still_open: impl Fn() -> bool,
    mut run_one: F,
) -> (Vec<SessionResult>, Vec<SessionRecord>)
where
    F: FnMut(SessionRef) -> Fut,
    Fut: Future<Output = (SessionOutcome, SessionRecord)>,
{
    let mut results = Vec::with_capacity(sessions.len());
    let mut records = Vec::with_capacity(sessions.len());
    let (mut closed, mut stopped) = (false, false);
    for s in sessions {
        closed = closed || !still_open();
        let skipped = if closed {
            Some(crate::topology::runtime::not_connected())
        } else if started.elapsed() >= BATCH_LIMIT {
            Some(batch_timeout())
        } else if stopped {
            Some(batch_stopped())
        } else {
            None
        };
        let (outcome, record) = match skipped {
            Some(error) => {
                let outcome = SessionOutcome::Failed { error };
                let mut record = blank_record(*s);
                record.result = result_name(&outcome).to_string();
                (outcome, record)
            }
            None => run_one(*s).await,
        };
        stopped =
            stopped || matches!(&outcome, SessionOutcome::Failed { error } if timed_out(error));
        records.push(record);
        results.push(SessionResult {
            pid: s.pid,
            backend_start_us: s.backend_start_us,
            outcome,
        });
    }
    (results, records)
}

/// Выполняет отмену или завершение сессий, разрешённых в предпросмотре (спец. §8.3). Защита — до обращения к серверу.
pub async fn execute(
    st: &AppState,
    id: &str,
    req: &SessionActionExecute,
    confirmation: &Confirmation,
) -> AppResult<SessionActionResult> {
    let profile = st.profile(id)?;
    // Номер открытия подключения: закрытое или открытое заново за время пакета останавливает его.
    let generation = st.runtimes.generation(id);
    ensure_mutation_allowed(&profile, confirmation.confirm.as_deref())?;
    // Повтор пары — одна сессия: число для подтверждения и пакет считаются без повторов.
    let sessions = dedup_sessions(&req.sessions);
    ensure_valid_target(&req.target)?;
    ensure_sessions_match(&req.target, &sessions)?;
    // Пакет — несколько сессий или любой выбор по критерию: на prod нужно ещё ввести число сессий.
    let batch = sessions.len() > 1 || matches!(req.target, SessionTarget::IdleInTransaction { .. });
    if batch && profile.environment == Environment::Prod {
        ensure_name_confirmed(
            &sessions.len().to_string(),
            confirmation.typed_name.as_deref(),
        )?;
    }
    if sessions.len() > MAX_BATCH {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.batchTooLarge",
            "At most {{max}} sessions per action",
            max = MAX_BATCH
        ));
    }
    let started = Instant::now();
    // Ошибка соединения или чтения версии — ошибка команды: действие не началось, строки журнала нет.
    let c = crate::actions::conn::open(st, id, &req.member, None).await?;
    let caps = Caps::new(crate::diag::head(&c.client).await?.version_num);
    let sql = signal_sql(req.kind, &req.target, caps);
    let (results, records) = run_batch(
        &sessions,
        started,
        || st.runtimes.generation(id) == generation,
        |s| one(&c.client, &sql, req.kind, &req.target, s),
    )
    .await;
    let outcome = batch_outcome(&results);
    let record = ActionRecord {
        ts: journal::now_ms(),
        action: match req.kind {
            SignalKind::Cancel => "cancel",
            SignalKind::Terminate => "terminate",
        }
        .into(),
        connection: RecordConnection::of(&profile),
        member: Some(req.member.clone()),
        prod_confirmed: profile.environment == Environment::Prod,
        outcome,
        code: batch_code(&results, outcome),
        ms: started.elapsed().as_millis() as u64,
        sessions: records,
        slot: None,
        patroni: None,
    };
    let journal = st.actions.clone();
    let journal_written = crate::conn::run_blocking(move || Ok(journal.append(&record)))
        .await
        .unwrap_or(false);
    Ok(SessionActionResult {
        member: req.member.clone(),
        results,
        outcome,
        journal_written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        client: bool,
        own: bool,
        session_super: bool,
        same_role: bool,
        me_super: bool,
        signal: bool,
    ) -> PrivFacts {
        PrivFacts {
            client,
            own,
            session_super,
            same_role,
            me_super,
            signal,
        }
    }

    #[test]
    fn privilege_rules_follow_postgres() {
        // Суперпользователь может всё, кроме чужого фонового процесса и своего соединения.
        assert_eq!(
            deny_reason(&facts(true, false, true, false, true, false)),
            None
        );
        // Своя роль — можно и без pg_signal_backend.
        assert_eq!(
            deny_reason(&facts(true, false, false, true, false, false)),
            None
        );
        // pg_signal_backend — только сессии не суперпользователя.
        assert_eq!(
            deny_reason(&facts(true, false, false, false, false, true)),
            None
        );
        assert_eq!(
            deny_reason(&facts(true, false, true, false, false, true)),
            Some(DenyReason::SuperuserSession)
        );
        assert_eq!(
            deny_reason(&facts(true, false, false, false, false, false)),
            Some(DenyReason::OtherRole)
        );
        assert_eq!(
            deny_reason(&facts(false, false, false, false, true, true)),
            Some(DenyReason::NotClientBackend)
        );
        assert_eq!(
            deny_reason(&facts(true, true, false, true, true, true)),
            Some(DenyReason::OwnConnection)
        );
    }

    #[test]
    fn cancel_needs_a_running_query_and_rights_come_first() {
        let ok = facts(true, false, false, true, false, false);
        let cancel = |state| row_reason(SignalKind::Cancel, &ok, state);
        assert_eq!(cancel(Some("active")), None);
        assert_eq!(cancel(Some("fastpath function call")), None);
        for state in [
            Some("idle"),
            Some("idle in transaction"),
            Some("idle in transaction (aborted)"),
            None,
        ] {
            assert_eq!(cancel(state), Some(DenyReason::NotRunning), "{state:?}");
        }
        // Завершение не зависит от состояния.
        assert_eq!(
            row_reason(SignalKind::Terminate, &ok, Some("idle in transaction")),
            None
        );
        // Без прав причина — права, а не состояние.
        assert_eq!(
            row_reason(
                SignalKind::Cancel,
                &facts(true, false, false, false, false, false),
                Some("idle")
            ),
            Some(DenyReason::OtherRole)
        );
    }

    #[test]
    fn preview_sql_filters_by_target() {
        let sel = preview_sql(&SessionTarget::Selected { sessions: vec![] });
        assert!(sel.contains("a.pid = ANY($1)"));
        let idle = preview_sql(&SessionTarget::IdleInTransaction {
            older_than_s: 300.0,
        });
        assert!(
            idle.contains("a.state IN ('idle in transaction', 'idle in transaction (aborted)')")
        );
        assert!(idle.contains("make_interval(secs => $1)"));
        assert!(idle.contains("a.backend_type = 'client backend'"));
        let root = preview_sql(&SessionTarget::RootBlocker {
            pid: 1,
            backend_start_us: 2,
        });
        assert!(root.contains("a.pid = $1"));
        for s in [&sel, &idle, &root] {
            assert!(
                s.contains(crate::diag::sql::BACKEND_START_US),
                "одно выражение backend_start во всех запросах"
            );
            assert!(s.contains("pg_has_role(current_user, 'pg_signal_backend', 'MEMBER')"));
        }
    }

    #[test]
    fn request_json_is_camel_case() {
        let r: SessionActionRequest = serde_json::from_value(serde_json::json!({
            "member": "pg-1", "kind": "terminate",
            "target": {"kind": "rootBlocker", "pid": 42, "backendStartUs": 1_761_299_000_000_000_i64}
        }))
        .unwrap();
        assert_eq!(r.kind, SignalKind::Terminate);
        assert_eq!(
            r.target,
            SessionTarget::RootBlocker {
                pid: 42,
                backend_start_us: 1_761_299_000_000_000
            }
        );
        let idle: SessionTarget = serde_json::from_value(
            serde_json::json!({"kind": "idleInTransaction", "olderThanS": 300}),
        )
        .unwrap();
        assert_eq!(
            idle,
            SessionTarget::IdleInTransaction {
                older_than_s: 300.0
            }
        );
    }

    #[test]
    fn signal_sql_rechecks_the_criterion_in_the_same_statement() {
        let caps = Caps::new(170000);
        let t = signal_sql(
            SignalKind::Terminate,
            &SessionTarget::IdleInTransaction {
                older_than_s: 300.0,
            },
            caps,
        );
        assert!(t.contains("pg_terminate_backend(a.pid)"));
        assert!(t.contains("make_interval(secs => $3)"));
        assert!(t.contains(&format!("{} = $2", crate::diag::sql::BACKEND_START_US)));
        assert!(t.contains("a.backend_type = 'client backend' AND a.pid <> pg_backend_pid()"));
        let c = signal_sql(
            SignalKind::Cancel,
            &SessionTarget::Selected { sessions: vec![] },
            caps,
        );
        assert!(c.contains("pg_cancel_backend(a.pid)") && !c.contains("$3"));
        assert!(c.contains("AND a.state IN ('active', 'fastpath function call')"));
        assert!(!t.contains("a.state IN ('active'"));
        let r = signal_sql(
            SignalKind::Terminate,
            &SessionTarget::RootBlocker {
                pid: 1,
                backend_start_us: 2,
            },
            caps,
        );
        assert!(r.contains("a.pid = ANY(pg_blocking_pids(w.pid))"));
        // query_id — с 14; на 13 типизированный NULL.
        assert!(t.contains("a.query_id::text"));
        assert!(
            signal_sql(
                SignalKind::Terminate,
                &SessionTarget::Selected { sessions: vec![] },
                Caps::new(130000)
            )
            .contains("NULL::text")
        );
    }

    #[test]
    fn batch_outcome_counts_failures() {
        let r = |outcome| SessionResult {
            pid: 1,
            backend_start_us: 1,
            outcome,
        };
        let err = || {
            crate::error::AppError::localized(
                crate::error::ErrorCode::Internal,
                crate::i18n::LocalizedText {
                    key: "k".into(),
                    params: Default::default(),
                    text: "t".into(),
                },
            )
        };
        use journal::Outcome::*;
        assert_eq!(
            batch_outcome(&[r(SessionOutcome::Terminated), r(SessionOutcome::Gone)]),
            Ok
        );
        assert_eq!(batch_outcome(&[r(SessionOutcome::NoLongerMatches)]), Ok);
        assert_eq!(
            batch_outcome(&[r(SessionOutcome::Terminated), r(SessionOutcome::NotAllowed)]),
            Partial
        );
        assert_eq!(
            batch_outcome(&[r(SessionOutcome::Failed { error: err() })]),
            Error
        );
        assert_eq!(batch_outcome(&[r(SessionOutcome::NotAllowed)]), Error);
    }

    #[test]
    fn result_json_carries_the_result_tag() {
        let v = serde_json::to_value(SessionResult {
            pid: 7,
            backend_start_us: 9,
            outcome: SessionOutcome::SignalSent,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"pid": 7, "backendStartUs": 9, "result": "signalSent"})
        );
    }

    /// Состояние с одним сохранённым профилем и без открытого подключения: до сервера дело не доходит,
    /// а всё, что проверка пропускает дальше, упирается в «подключение не открыто».
    fn state_with(
        profile: crate::conn::profile::ConnectionProfile,
    ) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(
            dir.path(),
            std::sync::Arc::new(crate::secrets::MemoryStore::default()),
        )
        .unwrap();
        st.connections.upsert(profile).unwrap();
        (dir, st)
    }

    /// Запрос на `sessions` сессий; у цели `selected` выбор — сами эти сессии (как присылает интерфейс).
    fn execute_req(target: SessionTarget, sessions: usize) -> SessionActionExecute {
        let sessions: Vec<SessionRef> = (0..sessions as i32)
            .map(|pid| SessionRef {
                pid,
                backend_start_us: 1,
            })
            .collect();
        let target = match target {
            SessionTarget::Selected { .. } => SessionTarget::Selected {
                sessions: sessions.clone(),
            },
            other => other,
        };
        SessionActionExecute {
            member: "m".into(),
            kind: SignalKind::Terminate,
            target,
            sessions,
        }
    }

    async fn rejection(
        st: &AppState,
        req: &SessionActionExecute,
        c: &Confirmation,
    ) -> crate::error::AppError {
        match execute(st, "a", req, c).await {
            Ok(r) => panic!("действие не должно выполняться: {r:?}"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn bad_threshold_is_rejected_before_connecting() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        for bad in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let req = execute_req(SessionTarget::IdleInTransaction { older_than_s: bad }, 1);
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(e.code, crate::error::ErrorCode::InvalidInput, "{bad}");
            assert_eq!(e.key, "errors.actions.badThreshold", "{bad}");
            assert_eq!(
                e.message,
                "The age threshold must be a non-negative number of seconds"
            );
        }
        // Ноль — допустимый порог: проверка пройдена, дальше нет открытого подключения.
        let ok = execute_req(SessionTarget::IdleInTransaction { older_than_s: 0.0 }, 1);
        let e = rejection(&st, &ok, &Confirmation::default()).await;
        assert_eq!(e.key, "errors.topology.notConnected");
    }

    #[tokio::test]
    async fn guard_runs_before_any_other_check() {
        let mut p = crate::conn::profile::sample_profile("a");
        p.read_only = true;
        let (_dir, st) = state_with(p);
        let req = execute_req(
            SessionTarget::IdleInTransaction { older_than_s: -1.0 },
            MAX_BATCH + 1,
        );
        let e = rejection(&st, &req, &Confirmation::default()).await;
        assert_eq!(e.code, crate::error::ErrorCode::ReadOnly);
    }

    #[tokio::test]
    async fn oversized_batch_is_rejected_before_connecting() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        let req = execute_req(SessionTarget::Selected { sessions: vec![] }, MAX_BATCH + 1);
        let e = rejection(&st, &req, &Confirmation::default()).await;
        assert_eq!(e.code, crate::error::ErrorCode::InvalidInput);
        assert_eq!(e.key, "errors.actions.batchTooLarge");
        assert_eq!(e.message, "At most 500 sessions per action");
        // Ровно предел — проходит дальше проверок.
        let req = execute_req(SessionTarget::Selected { sessions: vec![] }, MAX_BATCH);
        let e = rejection(&st, &req, &Confirmation::default()).await;
        assert_eq!(e.key, "errors.topology.notConnected");
    }

    #[tokio::test]
    async fn prod_batch_needs_the_session_count_typed() {
        let mut p = crate::conn::profile::sample_profile("a");
        p.environment = crate::conn::profile::Environment::Prod;
        let name = p.name.clone();
        let (_dir, st) = state_with(p);
        let confirm = |typed: Option<&str>| Confirmation {
            confirm: Some(name.clone()),
            typed_name: typed.map(str::to_string),
        };
        let batch = execute_req(SessionTarget::Selected { sessions: vec![] }, 3);
        for typed in [None, Some("2"), Some("три")] {
            let e = rejection(&st, &batch, &confirm(typed)).await;
            assert_eq!(e.code, crate::error::ErrorCode::ConfirmationRequired);
            assert_eq!(e.key, "errors.guard.typeToConfirm", "{typed:?}");
            assert_eq!(e.params["expected"], "3");
        }
        let e = rejection(&st, &batch, &confirm(Some("3"))).await;
        assert_eq!(e.key, "errors.topology.notConnected");
        // Одна сессия — не пакет: число вводить не нужно, достаточно имени подключения.
        let one = execute_req(SessionTarget::Selected { sessions: vec![] }, 1);
        let e = rejection(&st, &one, &confirm(None)).await;
        assert_eq!(e.key, "errors.topology.notConnected");
        // Пакет по критерию — пакет даже из одной сессии.
        let idle = execute_req(SessionTarget::IdleInTransaction { older_than_s: 60.0 }, 1);
        let e = rejection(&st, &idle, &confirm(None)).await;
        assert_eq!(e.key, "errors.guard.typeToConfirm");
    }

    fn pair(pid: i32, start: i64) -> SessionRef {
        SessionRef {
            pid,
            backend_start_us: start,
        }
    }

    fn execute_with(target: SessionTarget, sessions: Vec<SessionRef>) -> SessionActionExecute {
        SessionActionExecute {
            member: "m".into(),
            kind: SignalKind::Terminate,
            target,
            sessions,
        }
    }

    #[tokio::test]
    async fn empty_session_list_is_rejected_before_connecting() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        for target in [
            SessionTarget::Selected { sessions: vec![] },
            SessionTarget::IdleInTransaction { older_than_s: 60.0 },
            SessionTarget::RootBlocker {
                pid: 5,
                backend_start_us: 7,
            },
        ] {
            let req = execute_with(target, vec![]);
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(e.code, crate::error::ErrorCode::InvalidInput);
            assert_eq!(e.key, "errors.actions.noSessions");
            assert_eq!(e.message, "No sessions to act on");
        }
    }

    #[tokio::test]
    async fn root_blocker_sessions_must_be_exactly_the_target() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        let target = || SessionTarget::RootBlocker {
            pid: 5,
            backend_start_us: 7,
        };
        for sessions in [
            vec![pair(6, 7)],
            vec![pair(5, 8)],
            vec![pair(5, 7), pair(6, 7)],
            vec![pair(6, 7), pair(5, 7)],
        ] {
            let req = execute_with(target(), sessions.clone());
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(
                e.code,
                crate::error::ErrorCode::InvalidInput,
                "{sessions:?}"
            );
            assert_eq!(e.key, "errors.actions.badTarget", "{sessions:?}");
            assert_eq!(e.message, "The sessions do not match the action target");
        }
        // Ровно пара цели проходит проверки; повтор той же пары — та же одна сессия.
        for sessions in [vec![pair(5, 7)], vec![pair(5, 7), pair(5, 7)]] {
            let req = execute_with(target(), sessions);
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(e.key, "errors.topology.notConnected");
        }
    }

    /// `selected`: выполняются только пары из цели — пара не из выбора, даже вместе с выбранными, отклоняется до
    /// подключения; подмножество выбора и повтор пары проходят.
    #[tokio::test]
    async fn selected_sessions_must_come_from_the_target() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        let target = || SessionTarget::Selected {
            sessions: vec![pair(5, 7), pair(6, 8)],
        };
        for sessions in [
            vec![pair(9, 9)],
            vec![pair(5, 8)],
            vec![pair(5, 7), pair(9, 9)],
            vec![pair(6, 8), pair(6, 7)],
        ] {
            let req = execute_with(target(), sessions.clone());
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(
                (e.code, e.key.as_str()),
                (
                    crate::error::ErrorCode::InvalidInput,
                    "errors.actions.badTarget"
                ),
                "{sessions:?}"
            );
        }
        for sessions in [
            vec![pair(5, 7)],
            vec![pair(6, 8), pair(5, 7)],
            vec![pair(5, 7), pair(5, 7)],
        ] {
            let req = execute_with(target(), sessions.clone());
            let e = rejection(&st, &req, &Confirmation::default()).await;
            assert_eq!(e.key, "errors.topology.notConnected", "{sessions:?}");
        }
    }

    #[test]
    fn dedup_keeps_the_first_occurrences_in_order() {
        assert_eq!(
            dedup_sessions(&[pair(3, 1), pair(1, 1), pair(3, 1), pair(3, 2), pair(1, 1)]),
            [pair(3, 1), pair(1, 1), pair(3, 2)]
        );
        assert!(dedup_sessions(&[]).is_empty());
    }

    #[tokio::test]
    async fn prod_batch_counts_sessions_without_duplicates() {
        let mut p = crate::conn::profile::sample_profile("a");
        p.environment = crate::conn::profile::Environment::Prod;
        let name = p.name.clone();
        let (_dir, st) = state_with(p);
        let confirm = |typed: Option<&str>| Confirmation {
            confirm: Some(name.clone()),
            typed_name: typed.map(str::to_string),
        };
        let selected = || SessionTarget::Selected {
            sessions: vec![pair(1, 1), pair(2, 1)],
        };
        let batch = execute_with(selected(), vec![pair(1, 1), pair(1, 1), pair(2, 1)]);
        let e = rejection(&st, &batch, &confirm(Some("3"))).await;
        assert_eq!(e.key, "errors.guard.typeToConfirm");
        assert_eq!(e.params["expected"], "2");
        let e = rejection(&st, &batch, &confirm(Some("2"))).await;
        assert_eq!(e.key, "errors.topology.notConnected");
        // Два повтора одной сессии — не пакет: хватает имени подключения.
        let twice = execute_with(selected(), vec![pair(1, 1), pair(1, 1)]);
        let e = rejection(&st, &twice, &confirm(None)).await;
        assert_eq!(e.key, "errors.topology.notConnected");
    }

    #[test]
    fn false_from_the_signal_function_means_gone() {
        for kind in [SignalKind::Cancel, SignalKind::Terminate] {
            assert_eq!(
                outcome_after_signal(kind, false),
                Some(SessionOutcome::Gone)
            );
        }
        assert_eq!(
            outcome_after_signal(SignalKind::Cancel, true),
            Some(SessionOutcome::SignalSent)
        );
        // После доставленного завершения ещё нужно дождаться исчезновения сессии.
        assert_eq!(outcome_after_signal(SignalKind::Terminate, true), None);
    }

    #[test]
    fn error_batch_carries_the_first_failure_code() {
        let r = |outcome| SessionResult {
            pid: 1,
            backend_start_us: 1,
            outcome,
        };
        let timeout = || SessionOutcome::Failed {
            error: app_err!(
                ErrorCode::Timeout,
                "errors.actions.batchTimeout",
                "The batch did not finish within {{secs}} s",
                secs = 60
            ),
        };
        use journal::Outcome::*;
        let code = |results: &[SessionResult]| batch_code(results, batch_outcome(results));
        assert_eq!(
            code(&[r(SessionOutcome::NotAllowed)]).as_deref(),
            Some("NOT_AUTHORIZED")
        );
        assert_eq!(code(&[r(timeout())]).as_deref(), Some("TIMEOUT"));
        // Первая из неудачных сессий, а не последняя.
        assert_eq!(
            code(&[r(SessionOutcome::NotAllowed), r(timeout())]).as_deref(),
            Some("NOT_AUTHORIZED")
        );
        // Код пишется только при итоге `error`.
        let mixed = [r(SessionOutcome::Terminated), r(timeout())];
        assert_eq!(batch_outcome(&mixed), Partial);
        assert_eq!(batch_code(&mixed, Partial), None);
        let gone = [r(SessionOutcome::Gone)];
        assert_eq!(batch_code(&gone, batch_outcome(&gone)), None);
        // Остановленный пакет — неудача, как прочие ошибки; код — по коду ошибки.
        for error in [statement_timeout(), batch_stopped()] {
            let stopped = [r(SessionOutcome::Failed { error })];
            assert_eq!(batch_outcome(&stopped), Error);
            assert_eq!(code(&stopped).as_deref(), Some("TIMEOUT"));
        }
        let closed = [r(SessionOutcome::Failed {
            error: crate::topology::runtime::not_connected(),
        })];
        assert_eq!(code(&closed).as_deref(), Some("NODE_UNAVAILABLE"));
    }

    /// Итог сессии без снимка: строка журнала — только пара и итог (как у сессии, по которой сигнала не было).
    fn ran(s: SessionRef, outcome: SessionOutcome) -> (SessionOutcome, SessionRecord) {
        let mut record = blank_record(s);
        record.result = result_name(&outcome).to_string();
        (outcome, record)
    }

    fn failure(r: &SessionResult) -> Option<&crate::error::AppError> {
        match &r.outcome {
            SessionOutcome::Failed { error } => Some(error),
            _ => None,
        }
    }

    fn three() -> Vec<SessionRef> {
        vec![pair(1, 1), pair(2, 1), pair(3, 1)]
    }

    /// Строки журнала идут по сессиям пакета; у невыполненных — только пара и `failed`.
    fn assert_not_attempted(records: &[SessionRecord], sessions: &[SessionRef]) {
        assert_eq!(records.len(), sessions.len());
        for (r, s) in records.iter().zip(sessions) {
            assert_eq!((r.pid, r.backend_start_us), (s.pid, s.backend_start_us));
            assert_eq!(r.result, "failed");
            assert!(r.user.is_none() && r.query.is_none());
        }
    }

    #[tokio::test]
    async fn closed_connection_stops_the_batch() {
        let sessions = three();
        let calls = std::cell::Cell::new(0);
        let (results, records) = run_batch(
            &sessions,
            Instant::now(),
            || calls.get() == 0,
            |s| {
                calls.set(calls.get() + 1);
                std::future::ready(ran(s, SessionOutcome::Terminated))
            },
        )
        .await;
        assert_eq!(calls.get(), 1, "после закрытия запросов к серверу нет");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].outcome, SessionOutcome::Terminated);
        assert_eq!(records[0].result, "terminated");
        for r in &results[1..] {
            let e = failure(r).expect("failed");
            assert_eq!(e.key, "errors.topology.notConnected");
            assert_eq!(e.code, ErrorCode::NodeUnavailable);
        }
        assert_eq!(results.iter().map(|r| r.pid).collect::<Vec<_>>(), [1, 2, 3]);
        assert_not_attempted(&records[1..], &sessions[1..]);
    }

    #[tokio::test]
    async fn statement_timeout_stops_the_batch() {
        let sessions = three();
        let calls = std::cell::Cell::new(0);
        let (results, records) = run_batch(
            &sessions,
            Instant::now(),
            || true,
            |s| {
                calls.set(calls.get() + 1);
                std::future::ready(ran(
                    s,
                    SessionOutcome::Failed {
                        error: statement_timeout(),
                    },
                ))
            },
        )
        .await;
        assert_eq!(
            calls.get(),
            1,
            "соединение после зависшего запроса не используется"
        );
        assert_eq!(
            failure(&results[0]).map(|e| e.key.as_str()),
            Some("errors.actions.statementTimeout")
        );
        for r in &results[1..] {
            let e = failure(r).expect("failed");
            assert_eq!(e.key, "errors.actions.batchStopped");
            assert_eq!(e.code, ErrorCode::Timeout);
            assert_eq!(
                e.message,
                "Not attempted: the batch was stopped after a timeout"
            );
        }
        assert_not_attempted(&records, &sessions);
    }

    #[tokio::test]
    async fn batch_limit_fails_the_rest_without_signals() {
        let sessions = three();
        let calls = std::cell::Cell::new(0);
        let run = async |still_open: bool| {
            run_batch(
                &sessions,
                Instant::now() - BATCH_LIMIT,
                || still_open,
                |s| {
                    calls.set(calls.get() + 1);
                    std::future::ready(ran(s, SessionOutcome::Terminated))
                },
            )
            .await
        };
        let (results, records) = run(true).await;
        assert_eq!(calls.get(), 0);
        for r in &results {
            let e = failure(r).expect("failed");
            assert_eq!(e.key, "errors.actions.batchTimeout");
            assert_eq!(e.message, "The batch did not finish within 60 s");
        }
        assert_not_attempted(&records, &sessions);
        // Закрытие подключения проверяется раньше предела пакета.
        let (results, _) = run(false).await;
        assert_eq!(calls.get(), 0);
        for r in &results {
            assert_eq!(
                failure(r).map(|e| e.key.as_str()),
                Some("errors.topology.notConnected")
            );
        }
    }

    /// Прочие неудачи сессии пакет не останавливают: соединение после них пригодно.
    #[tokio::test]
    async fn ordinary_batch_runs_every_session() {
        let sessions = three();
        let mut outcomes = vec![
            SessionOutcome::Failed {
                error: crate::error::AppError::localized(
                    ErrorCode::ConnectionFailed,
                    crate::i18n::LocalizedText::raw("server closed the connection"),
                ),
            },
            SessionOutcome::NotAllowed,
            SessionOutcome::Gone,
        ]
        .into_iter();
        let mut seen = Vec::new();
        let (results, records) = run_batch(
            &sessions,
            Instant::now(),
            || true,
            |s| {
                seen.push(s);
                std::future::ready(ran(s, outcomes.next().expect("итог сессии")))
            },
        )
        .await;
        assert_eq!(seen, sessions);
        assert_eq!(results[1].outcome, SessionOutcome::NotAllowed);
        assert_eq!(results[2].outcome, SessionOutcome::Gone);
        assert_eq!(
            records
                .iter()
                .map(|r| r.result.as_str())
                .collect::<Vec<_>>(),
            ["failed", "notAllowed", "gone"]
        );
    }

    /// Клиентский срок: запрос, на который сервер не отвечает, кончается ошибкой ровно через `STATEMENT_TIMEOUT`.
    #[tokio::test(start_paused = true)]
    async fn a_statement_without_an_answer_times_out() {
        let begun = tokio::time::Instant::now();
        let e = timed(std::future::pending::<()>()).await.unwrap_err();
        assert_eq!(begun.elapsed(), STATEMENT_TIMEOUT);
        assert_eq!(e.code, ErrorCode::Timeout);
        assert_eq!(e.key, "errors.actions.statementTimeout");
        assert_eq!(e.message, "The server did not answer within 10 s");
        assert_eq!(e.params["secs"], "10");
        // Ответ в срок проходит как есть.
        assert_eq!(timed(async { 7 }).await.unwrap(), 7);
    }

    #[test]
    fn blank_record_has_only_the_pair() {
        let r = blank_record(pair(4, 9));
        assert_eq!((r.pid, r.backend_start_us), (4, 9));
        assert!(r.user.is_none() && r.query.is_none() && r.xact_age_s.is_none());
        assert!(r.result.is_empty());
    }

    #[tokio::test]
    async fn preview_rejects_a_bad_threshold_before_connecting() {
        let (_dir, st) = state_with(crate::conn::profile::sample_profile("a"));
        let preview_of = |older_than_s| SessionActionRequest {
            member: "m".into(),
            kind: SignalKind::Terminate,
            target: SessionTarget::IdleInTransaction { older_than_s },
        };
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            let e = preview(&st, "a", &preview_of(bad)).await.unwrap_err();
            assert_eq!(e.key, "errors.actions.badThreshold", "{bad}");
        }
        // Ноль допустим: проверка пройдена, дальше нет открытого подключения.
        let e = preview(&st, "a", &preview_of(0.0)).await.unwrap_err();
        assert_eq!(e.key, "errors.topology.notConnected");
    }
}
