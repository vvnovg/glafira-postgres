//! Очередь фонового ассистента (спец. P4c §3.2, §4.1, §4.4, §7): запуски по одному в порядке очереди (не больше
//! `MAX_QUEUE` ожидающих), перепроверки перед стартом, билет и MCP-конфиг, процесс агента со сроком и остановкой, проверка
//! инструментов модели по событию `init` до первого хода, итог по коду выхода, вызовам и предложениям запуска; событие
//! `agent://runs`, записи журнала MCP `agentRunStarted` / `agentRunFinished`, `McpStatus.agent`. Процессы — через
//! `AgentSpawner`, модель Ollama — через `ModelWarmth`: unit-тесты не запускают ни CLI, ни модель. Методы, кроме
//! `shutdown`, синхронны (пишут `agent-runs.json`) и вызываются внутри runtime tokio: сам запуск идёт задачей tokio.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use glafira_mcp_core::audit::AuditEvent;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Emitter};
use tokio::sync::{oneshot, watch};

use super::models::effective_model;
use super::presets::{self, RunInputs};
use super::process::{self, AgentProcess, AgentSpawner, TokioSpawner};
use super::prompt;
pub use super::runs::RunFilter;
use super::runs::{self, AgentRun, FailReason, NewRun, RunKind, RunStatus, RunStore};
use super::settings::{AgentSettings, Preset, disabled_by_policy, not_configured};
use super::tickets::{MAX_RUN_CALLS, RunObserver, RunScope, Tickets};
use super::warmth::{ModelWarmth, NoWarmth};
use crate::conn::profile::LlmAccess;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::mcp::McpController;
use crate::mcp::access::effective_access;
use crate::mcp::llm::proposals::ProposalFilter;
use crate::mcp::llm::requests::{RequestStatus, request_not_found};
use crate::mcp::llm::{LlmStores, off_thread};
use crate::policy::{Policy, PolicyStatus};
use crate::settings::SettingsStore;
use crate::storage::connections::ConnectionStore;
use crate::sync::MutexExt;
use crate::topology::model::now_ms;

/// Ожидающих запусков — не больше (спец. §3.2); одиннадцатый — ошибка.
pub const MAX_QUEUE: usize = 10;
/// Событие для консоли, карточки и «Настроек»: `AgentRun` целиком.
pub const RUNS_EVENT: &str = "agent://runs";
/// Ход работы уходит в интерфейс не чаще (§5.4); смена статуса — сразу.
const PROGRESS_EVERY: Duration = Duration::from_secs(1);
/// Предупреждение запуска: агент не сообщил список своих инструментов (§4.1 «Проверка инструментов»).
const UNSAFE_NO_INIT: &str = "unsafe no-init";

fn agent_off() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.off",
        "The background assistant is off: turn it on in Settings → LLM assistants (MCP)"
    )
}

fn queue_full() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.queueFull",
        "The background assistant queue is full"
    )
}

fn not_waiting(id: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.requestNotWaiting",
        "Optimization request {{id}} is not waiting for an answer",
        id = id
    )
}

pub fn log_missing(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.agent.logMissing",
        "The log of background run {{id}} is not available",
        id = id
    )
}

/// Идущий запуск в `McpStatus.agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningRef {
    pub run_id: String,
    pub request_id: Option<String>,
}

/// `McpStatus.agent` (спец. §8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    /// Политика: `mcpEnabled` и `mcpAgentEnabled`.
    pub policy_enabled: bool,
    pub enabled: bool,
    pub configured: bool,
    pub preset: Preset,
    pub running: Option<RunningRef>,
    pub queued: u32,
}

impl AgentStatus {
    /// Без очереди (запусков нет): только настройки и политика.
    pub fn idle(s: &AgentSettings, policy: &PolicyStatus) -> Self {
        Self {
            policy_enabled: policy.agent_allowed(),
            enabled: s.enabled,
            configured: s.configured(),
            preset: s.preset,
            running: None,
            queued: 0,
        }
    }
}

/// Получатель `agent://runs`. В приложении — окно Tauri, в тестах — запись событий.
pub trait RunsSink: Send + Sync {
    fn run_changed(&self, run: &AgentRun);
}

struct TauriRunsSink(AppHandle);

impl RunsSink for TauriRunsSink {
    fn run_changed(&self, run: &AgentRun) {
        if let Err(e) = self.0.emit(RUNS_EVENT, run) {
            tracing::warn!("emit {RUNS_EVENT}: {e}");
        }
    }
}

/// Чем очередь запускает агента: процессы, тёплая модель, путь шима, срок (тесты).
#[derive(Clone)]
pub struct AgentRuntime {
    pub spawner: Arc<dyn AgentSpawner>,
    pub warmth: Arc<dyn ModelWarmth>,
    /// Команда сервера `glafira-pg` в MCP-конфиге запуска — шим.
    pub shim_exe: PathBuf,
    /// Срок запуска вместо «Срок запуска, мин» — только тесты (минимум настройки — 2 мин).
    pub run_timeout: Option<Duration>,
}

impl AgentRuntime {
    /// Как в приложении: `TokioSpawner`, без тёплой модели (её ставит `lib.rs`, задача 7), шим — этот же бинарь
    /// (`mcp::launch_path`: в AppImage — `$APPIMAGE`).
    pub fn app() -> io::Result<Self> {
        Ok(Self {
            spawner: Arc::new(TokioSpawner::default()),
            warmth: Arc::new(NoWarmth),
            shim_exe: crate::mcp::launch_path(
                &std::env::current_exe()?,
                std::env::var_os("APPIMAGE"),
            ),
            run_timeout: None,
        })
    }
}

/// Части приложения, которые нужны очереди (без `AppState`: очередь — его поле).
#[derive(Clone)]
pub struct AgentDeps {
    pub data_dir: PathBuf,
    pub settings: Arc<SettingsStore>,
    pub policy: Arc<Policy>,
    pub connections: Arc<ConnectionStore>,
    pub llm: Arc<LlmStores>,
    pub mcp: Arc<McpController>,
    pub tickets: Arc<Tickets>,
    pub runtime: AgentRuntime,
}

/// Почему процесс остановлен до своего конца.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stopped {
    Timeout,
    /// «Остановить» в списке запусков.
    Stop,
    /// «Отменить запрос».
    RequestCancelled,
    /// Выход из Glafira.
    Shutdown,
}

/// Что известно о конце запуска (`outcome`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct EndFacts {
    pub kind: RunKind,
    pub stopped: Option<Stopped>,
    /// Проверка инструментов не прошла: процесс остановлен до первого хода (§4.1).
    pub unsafe_tools: bool,
    pub code: Option<i32>,
    /// Сессия с билетом хоть раз подключилась.
    pub connected: bool,
    /// Вызовов запуска; больше `MAX_RUN_CALLS` — шлюз отказал по пределу.
    pub calls: u32,
    /// CLI сообщил о своём пределе (`process::cli_limit`).
    pub cli_limit: bool,
    pub proposals: usize,
    pub mcp_running: bool,
    /// Действующий уровень подключения запроса — не `off`.
    pub access_ok: bool,
}

/// Итог запуска (спец. §3.2, §7; уточнение плана 13).
pub(crate) fn outcome(f: &EndFacts) -> (RunStatus, Option<FailReason>) {
    use FailReason as R;
    use RunStatus as S;
    match f.stopped {
        Some(Stopped::Timeout) => return (S::Failed, Some(R::Timeout)),
        Some(Stopped::Stop | Stopped::RequestCancelled) => return (S::Cancelled, None),
        Some(Stopped::Shutdown) => return (S::Interrupted, None),
        None => {}
    }
    if f.unsafe_tools {
        return (S::Failed, Some(R::UnsafeTools));
    }
    if !f.connected {
        return (
            S::Failed,
            Some(if f.mcp_running {
                R::NotConnected
            } else {
                R::McpOff
            }),
        );
    }
    let ok_exit = f.code == Some(0);
    if ok_exit && f.proposals > 0 {
        return (S::Done, None);
    }
    if !f.mcp_running {
        return (S::Failed, Some(R::McpOff));
    }
    if !f.access_ok {
        return (S::Failed, Some(R::AccessOff));
    }
    let limit = f.kind == RunKind::Optimize && (f.cli_limit || f.calls > MAX_RUN_CALLS);
    if limit {
        return if f.proposals > 0 {
            (S::Done, None)
        } else {
            (S::Failed, Some(R::Limit))
        };
    }
    if ok_exit {
        (S::Done, None)
    } else {
        (S::Failed, Some(R::Exit))
    }
}

/// Итог ожидания события `init` (§4.1 «Проверка инструментов»).
enum InitCheck {
    /// Только инструменты Glafira: запуск идёт дальше.
    Safe,
    /// Лишние инструменты или списка нет: предупреждение запуска (`unsafe …`).
    Unsafe(String),
    /// Пока ждали, запуск остановили или вышел срок.
    Stopped(Stopped),
}

/// Ход работы идущего запуска — в памяти; в файл — в конце.
#[derive(Debug, Clone, Default)]
struct Live {
    status: Option<RunStatus>,
    connected: bool,
    calls: u32,
    last_tool: Option<String>,
    last_call_at_ms: Option<i64>,
    last_emit: Option<tokio::time::Instant>,
    emit_pending: bool,
}

struct Current {
    run_id: String,
    request_id: Option<String>,
    stop: Option<oneshot::Sender<Stopped>>,
    done: watch::Receiver<bool>,
    live: Live,
}

#[derive(Default)]
struct QState {
    queued: VecDeque<String>,
    current: Option<Current>,
    /// Выход из Glafira: новые запуски не начинаются.
    closed: bool,
}

/// Что записать в запуск в его конце.
struct Final {
    status: RunStatus,
    reason: Option<FailReason>,
    code: Option<i32>,
    summary: Option<String>,
    cost: Option<f64>,
    proposals: Vec<String>,
    /// Дописываются к предупреждениям запуска (`unsafe …`).
    warnings: Vec<String>,
}

impl Final {
    fn early(status: RunStatus, reason: Option<FailReason>) -> Self {
        Self {
            status,
            reason,
            code: None,
            summary: None,
            cost: None,
            proposals: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Наблюдатель билетов (задача 4) без цикла `Arc`: билеты — поле приложения, очередь — тоже.
struct ObserverRef(Weak<AgentQueue>);

impl RunObserver for ObserverRef {
    fn run_session(&self, run_id: &str, connected: bool) {
        if let Some(q) = self.0.upgrade() {
            q.run_session(run_id, connected);
        }
    }

    fn run_call(&self, run_id: &str, tool: &str, calls: u32) {
        if let Some(q) = self.0.upgrade() {
            q.run_call(run_id, tool, calls);
        }
    }
}

pub struct AgentQueue {
    d: AgentDeps,
    /// `~/.qwen/settings.json` — модель Qwen Code по умолчанию.
    home: Option<PathBuf>,
    runs: RunStore,
    runs_dir: PathBuf,
    work_dir: PathBuf,
    state: Mutex<QState>,
    /// `start` и `check` не пересекаются: проверка повтора и предела очереди — вместе с записью.
    enqueue: Mutex<()>,
    sink: OnceLock<Arc<dyn RunsSink>>,
    me: Weak<AgentQueue>,
}

impl AgentQueue {
    /// Открывает `agent-runs.json`: запуски прошлого сеанса — `interrupted`, их MCP-конфиги удаляются, журналы — только
    /// последних `MAX_LOGS`. Очередь становится наблюдателем билетов и источником `McpStatus.agent`.
    pub fn open(d: AgentDeps) -> Arc<Self> {
        let runs_dir = d.data_dir.join(runs::RUNS_DIR);
        let work_dir = d.data_dir.join(runs::WORK_DIR);
        for dir in [&runs_dir, &work_dir] {
            if let Err(e) = runs::ensure_private_dir(dir) {
                tracing::warn!("{} not created: {e}", dir.display());
            }
        }
        let store = RunStore::open(&d.data_dir);
        match store.mark_interrupted() {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                runs = n,
                "background runs of the previous session are interrupted"
            ),
            Err(e) => tracing::warn!("background runs not marked interrupted: {}", e.message),
        }
        runs::remove_stale_configs(&runs_dir);
        runs::prune_logs(&runs_dir, &store.recent_ids(runs::MAX_LOGS));
        let q = Arc::new_cyclic(|me| Self {
            d,
            home: dirs::home_dir(),
            runs: store,
            runs_dir,
            work_dir,
            state: Mutex::default(),
            enqueue: Mutex::new(()),
            sink: OnceLock::new(),
            me: me.clone(),
        });
        q.d.tickets
            .set_observer(Arc::new(ObserverRef(Arc::downgrade(&q))));
        q.d.mcp.set_agent(Arc::downgrade(&q));
        q
    }

    pub fn deps(&self) -> AgentDeps {
        self.d.clone()
    }

    /// Окно приложения — получатель `agent://runs` (вызывается один раз в `setup`).
    pub fn set_app_handle(&self, h: AppHandle) {
        self.set_sink(Arc::new(TauriRunsSink(h)));
    }

    pub(crate) fn set_sink(&self, sink: Arc<dyn RunsSink>) {
        let _ = self.sink.set(sink);
    }

    /// «Передать LLM-ассистенту» в режиме фона и «Повторить»: запуск в очередь. Повторное нажатие для запроса, у которого
    /// запуск уже ждёт или идёт, возвращает его. Блокирующий вызов (запись файла).
    pub fn start(&self, request_id: &str) -> AppResult<AgentRun> {
        let _one = self.enqueue.lock_safe();
        if !self.d.policy.status.agent_allowed() {
            return Err(disabled_by_policy());
        }
        let s = self.d.settings.agent();
        if !s.enabled {
            return Err(agent_off());
        }
        if !s.configured() {
            return Err(not_configured());
        }
        let req = self
            .d
            .llm
            .requests
            .get(request_id)
            .ok_or_else(|| request_not_found(request_id))?;
        if req.status != RequestStatus::Waiting {
            return Err(not_waiting(request_id));
        }
        if let Some(active) = self.active(|r| r.request_id.as_deref() == Some(request_id)) {
            return Ok(active);
        }
        self.enqueue_run(NewRun {
            kind: RunKind::Optimize,
            request_id: Some(req.id),
            connection_id: Some(req.connection_id),
            preset: s.preset,
            model: effective_model(&s, self.home.as_deref()),
        })
    }

    /// «Проверить» (спец. §8.3): запуск без подключения и запроса; работает и при выключенном режиме. Блокирующий вызов.
    pub fn check(&self) -> AppResult<AgentRun> {
        let _one = self.enqueue.lock_safe();
        if !self.d.policy.status.agent_allowed() {
            return Err(disabled_by_policy());
        }
        let s = self.d.settings.agent();
        if !s.configured() {
            return Err(not_configured());
        }
        if let Some(active) = self.active(|r| r.kind == RunKind::Check) {
            return Ok(active);
        }
        self.enqueue_run(NewRun {
            kind: RunKind::Check,
            request_id: None,
            connection_id: None,
            preset: s.preset,
            model: effective_model(&s, self.home.as_deref()),
        })
    }

    fn enqueue_run(&self, n: NewRun) -> AppResult<AgentRun> {
        if self.state.lock_safe().queued.len() >= MAX_QUEUE {
            return Err(queue_full());
        }
        let run = self.runs.create(n)?;
        self.state.lock_safe().queued.push_back(run.id.clone());
        self.emit(&run);
        self.d.mcp.emit_state();
        self.pump();
        Ok(run)
    }

    /// Ожидающий или идущий запуск, подходящий под `pred`.
    fn active(&self, pred: impl Fn(&AgentRun) -> bool) -> Option<AgentRun> {
        let ids: Vec<String> = {
            let s = self.state.lock_safe();
            s.current
                .iter()
                .map(|c| c.run_id.clone())
                .chain(s.queued.iter().cloned())
                .collect()
        };
        ids.iter().filter_map(|id| self.get(id)).find(pred)
    }

    /// Следующий запуск из очереди, если ничего не идёт.
    fn pump(&self) {
        let Some(me) = self.me.upgrade() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("background run not started: no tokio runtime");
            return;
        };
        let (id, stop_rx, done_tx) = {
            let mut s = self.state.lock_safe();
            if s.closed || s.current.is_some() {
                return;
            }
            let Some(id) = s.queued.pop_front() else {
                return;
            };
            let (stop_tx, stop_rx) = oneshot::channel();
            let (done_tx, done_rx) = watch::channel(false);
            let request_id = self.runs.get(&id).and_then(|r| r.request_id);
            s.current = Some(Current {
                run_id: id.clone(),
                request_id,
                stop: Some(stop_tx),
                done: done_rx,
                live: Live::default(),
            });
            (id, stop_rx, done_tx)
        };
        rt.spawn(async move {
            me.clone().run_one(id, stop_rx).await;
            me.state.lock_safe().current = None;
            let _ = done_tx.send(true);
            me.d.mcp.emit_state();
            me.pump();
        });
    }

    /// Один запуск от перепроверок до итога.
    async fn run_one(self: Arc<Self>, id: String, mut stop_rx: oneshot::Receiver<Stopped>) {
        let Some(run) = self.runs.get(&id) else {
            return;
        };
        let s = self.d.settings.agent();
        if let Some((status, reason)) = self.precheck(&run, &s) {
            self.finish(&id, Final::early(status, reason)).await;
            return;
        }
        // starting: билет, MCP-конфиг, командная строка; срок отсчитывается отсюда (§4.4).
        let timeout = self
            .d
            .runtime
            .run_timeout
            .unwrap_or_else(|| Duration::from_secs(u64::from(s.timeout_min) * 60));
        let deadline = tokio::time::Instant::now() + timeout;
        let ticket = self.d.tickets.issue(RunScope {
            run_id: id.clone(),
            connection_id: run.connection_id.clone(),
            request_id: run.request_id.clone(),
        });
        let config = match runs::write_mcp_config(
            &self.runs_dir,
            &id,
            &self.d.runtime.shim_exe,
            &ticket,
            s.preset,
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(run = %id, "mcp config of the run not written: {e}");
                self.cleanup(&id);
                self.finish(
                    &id,
                    Final::early(RunStatus::Failed, Some(FailReason::SpawnFailed)),
                )
                .await;
                return;
            }
        };
        let inputs = RunInputs {
            mcp_config: config,
            cwd: self.work_dir.clone(),
            model: Some(s.model().trim().to_string()).filter(|m| !m.is_empty()),
            timeout_s: timeout.as_secs().max(1),
            ollama_url: s.ollama_url.clone(),
        };
        let spec = match presets::build(&s, &inputs) {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!(run = %id, "command line not built: {}", e.message);
                self.cleanup(&id);
                self.finish(
                    &id,
                    Final::early(RunStatus::Failed, Some(FailReason::NotConfigured)),
                )
                .await;
                return;
            }
        };
        let path = spec.program.display().to_string();
        if let Some(r) = self
            .update(&id, move |r| {
                r.status = RunStatus::Starting;
                r.started_at_ms = Some(now_ms());
                r.path = Some(path);
            })
            .await
        {
            self.set_live_status(&id, RunStatus::Starting);
            self.emit(&r);
            self.audit_started(&r);
        }
        self.d.mcp.emit_state();
        // Предзагрузка модели (в фоне, §6.2) и версия программы — пока агент ещё не запущен.
        let warnings = self.d.runtime.warmth.before_run(run.model.as_deref()).await;
        let (spawner, program) = (self.d.runtime.spawner.clone(), spec.program.clone());
        let version = tokio::task::spawn_blocking(move || spawner.version(&program))
            .await
            .ok()
            .flatten();
        if let Some(r) = self
            .update(&id, move |r| {
                r.cli_version = version;
                r.warnings = warnings;
            })
            .await
        {
            self.emit(&r);
        }
        if let Ok(why) = stop_rx.try_recv() {
            self.cleanup(&id);
            let (status, reason) = outcome(&EndFacts {
                stopped: Some(why),
                ..EndFacts::default()
            });
            self.finish(&id, Final::early(status, reason)).await;
            return;
        }
        let text = match (run.kind, run.request_id.as_deref()) {
            (RunKind::Optimize, Some(request)) => prompt::run_prompt(request, &self.ui_language()),
            _ => prompt::check_prompt(),
        };
        let mut proc =
            match self
                .d
                .runtime
                .spawner
                .spawn(&spec, text, runs::log_path(&self.runs_dir, &id))
            {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(run = %id, "agent not started: {e}");
                    let reason = if e.kind() == io::ErrorKind::NotFound {
                        FailReason::CliNotFound
                    } else {
                        FailReason::SpawnFailed
                    };
                    self.cleanup(&id);
                    self.finish(&id, Final::early(RunStatus::Failed, Some(reason)))
                        .await;
                    return;
                }
            };
        // До первого хода: модели доступны только инструменты Glafira (§4.1 «Проверка инструментов»).
        let (exit, stopped, unsafe_warning) = match Self::check_tools(
            proc.as_mut(),
            &mut stop_rx,
            deadline,
        )
        .await
        {
            InitCheck::Safe => {
                let ended = {
                    let mut wait = proc.wait();
                    let stop = async {
                        match (&mut stop_rx).await {
                            Ok(why) => why,
                            Err(_) => std::future::pending().await,
                        }
                    };
                    tokio::select! {
                        exit = &mut wait => Ok(exit),
                        () = tokio::time::sleep_until(deadline) => Err(Stopped::Timeout),
                        why = stop => Err(why),
                    }
                };
                match ended {
                    Ok(exit) => (exit, None, None),
                    Err(why) => {
                        proc.stop().await;
                        (proc.wait().await, Some(why), None)
                    }
                }
            }
            InitCheck::Unsafe(warning) => {
                tracing::warn!(run = %id, "{warning}: the agent is stopped before its first turn");
                proc.stop().await;
                (proc.wait().await, None, Some(warning))
            }
            InitCheck::Stopped(why) => {
                proc.stop().await;
                (proc.wait().await, Some(why), None)
            }
        };
        self.cleanup(&id);
        if stopped != Some(Stopped::Shutdown) {
            let (warmth, model) = (self.d.runtime.warmth.clone(), run.model.clone());
            tokio::spawn(async move { warmth.after_run(model.as_deref()).await });
        }
        let proposals = self.run_proposals(&run);
        let live = self.live(&id).unwrap_or_default();
        let unsafe_tools = unsafe_warning.is_some();
        let (status, reason) = outcome(&EndFacts {
            kind: run.kind,
            stopped,
            unsafe_tools,
            code: exit.code,
            connected: live.connected,
            calls: live.calls,
            cli_limit: process::cli_limit(s.preset, exit.code, &exit.stdout_tail),
            proposals: proposals.len(),
            mcp_running: self.d.mcp.is_running(),
            access_ok: run.kind == RunKind::Check || self.access_ok(&run),
        });
        // Остановленный до первого хода агент итога не писал: в хвосте — только служебные события.
        let (summary, cost) = if unsafe_tools {
            (None, None)
        } else {
            process::final_text(spec.output, &exit.stdout_tail)
        };
        // Стоимость — только у Claude Code без Ollama: через Ollama она выдуманная (§4.1, §4.5).
        let cost = cost.filter(|_| s.preset == Preset::Claude && !s.claude.via_ollama);
        self.finish(
            &id,
            Final {
                status,
                reason,
                code: exit.code,
                summary,
                cost,
                proposals,
                warnings: unsafe_warning.into_iter().collect(),
            },
        )
        .await;
    }

    /// Событие `init` — не дольше `INIT_TIMEOUT` (и не дольше срока запуска); остановка запуска прерывает ожидание. Имя
    /// без префикса `mcp__glafira-pg__` — `unsafe <имена через запятую>`; нет события — `unsafe no-init`.
    async fn check_tools(
        proc: &mut dyn AgentProcess,
        stop_rx: &mut oneshot::Receiver<Stopped>,
        deadline: tokio::time::Instant,
    ) -> InitCheck {
        let init_deadline = (tokio::time::Instant::now() + process::INIT_TIMEOUT).min(deadline);
        let stop = async {
            match stop_rx.await {
                Ok(why) => why,
                Err(_) => std::future::pending().await,
            }
        };
        tokio::select! {
            tools = tokio::time::timeout_at(init_deadline, proc.init_tools()) => match tools {
                Ok(Some(tools)) => {
                    let foreign = process::foreign_tools(&tools);
                    if foreign.is_empty() {
                        InitCheck::Safe
                    } else {
                        InitCheck::Unsafe(format!("unsafe {}", foreign.join(",")))
                    }
                }
                // Срок запуска короче минуты (тесты) и вышел раньше.
                Err(_) if tokio::time::Instant::now() >= deadline => {
                    InitCheck::Stopped(Stopped::Timeout)
                }
                Ok(None) | Err(_) => InitCheck::Unsafe(UNSAFE_NO_INIT.to_string()),
            },
            why = stop => InitCheck::Stopped(why),
        }
    }

    /// Перед стартом (§3.2): политика, MCP-сервер этого окна, режим и настройка, запрос ещё ждёт, уровень подключения.
    /// Запрос уже получил ответ или отменён (или режим выключен) — `skipped`; остальное — `failed` с причиной.
    fn precheck(
        &self,
        run: &AgentRun,
        s: &AgentSettings,
    ) -> Option<(RunStatus, Option<FailReason>)> {
        if !self.d.policy.status.agent_allowed() {
            return Some((RunStatus::Failed, Some(FailReason::Policy)));
        }
        if !self.d.mcp.is_running() {
            return Some((RunStatus::Failed, Some(FailReason::McpOff)));
        }
        if run.kind == RunKind::Optimize && !s.enabled {
            return Some((RunStatus::Skipped, None));
        }
        if !s.configured() {
            return Some((RunStatus::Failed, Some(FailReason::NotConfigured)));
        }
        if run.kind == RunKind::Optimize {
            let waiting = run
                .request_id
                .as_deref()
                .and_then(|id| self.d.llm.requests.get(id))
                .is_some_and(|r| r.status == RequestStatus::Waiting);
            if !waiting {
                return Some((RunStatus::Skipped, None));
            }
            if !self.access_ok(run) {
                return Some((RunStatus::Failed, Some(FailReason::AccessOff)));
            }
        }
        None
    }

    fn access_ok(&self, run: &AgentRun) -> bool {
        run.connection_id
            .as_deref()
            .and_then(|id| self.d.connections.get(id).ok())
            .is_some_and(|p| effective_access(&p, &self.d.policy.status) != LlmAccess::Off)
    }

    /// Предложения этого запуска (`runId`), по порядку. Повтор чужого предложения (дедупликация P4b отдаёт уже
    /// сохранённое — с чужим `runId` или без него) запуску не засчитывается.
    fn run_proposals(&self, run: &AgentRun) -> Vec<String> {
        let Some(request) = &run.request_id else {
            return Vec::new();
        };
        let filter = ProposalFilter {
            connection_id: run.connection_id.clone(),
            request_ids: Some(vec![request.clone()]),
            ..ProposalFilter::default()
        };
        let mut ids: Vec<String> = self
            .d
            .llm
            .proposals
            .list(&filter)
            .into_iter()
            .filter(|p| p.run_id.as_deref() == Some(run.id.as_str()))
            .map(|p| p.id)
            .collect();
        ids.reverse();
        ids
    }

    /// Конец запуска: билет отозван, сессии запуска закрыты, MCP-конфиг удалён (и при `cancelled`, и при `failed`).
    fn cleanup(&self, id: &str) {
        self.d.tickets.revoke(id);
        self.d.mcp.disconnect_run(id);
        runs::remove_mcp_config(&self.runs_dir, id);
    }

    async fn finish(&self, id: &str, f: Final) {
        let live = self.live(id);
        let ended = self
            .update(id, move |r| {
                r.status = f.status;
                r.reason = f.reason;
                r.exit_code = f.code;
                r.ended_at_ms = Some(now_ms());
                if let Some(l) = live {
                    r.calls = l.calls;
                    r.last_tool = l.last_tool;
                    r.last_call_at_ms = l.last_call_at_ms;
                }
                r.proposal_ids = f.proposals;
                r.summary = f.summary;
                r.cost_usd = f.cost;
                r.warnings.extend(f.warnings);
            })
            .await;
        if let Some(r) = ended {
            self.emit(&r);
            if r.started_at_ms.is_some() {
                self.audit_finished(&r);
            }
        }
        runs::prune_logs(&self.runs_dir, &self.runs.recent_ids(runs::MAX_LOGS));
    }

    /// Запись запуска из задачи запуска — вне потоков async.
    async fn update(
        &self,
        id: &str,
        f: impl FnOnce(&mut AgentRun) + Send + 'static,
    ) -> Option<AgentRun> {
        let me = self.me.upgrade()?;
        let id = id.to_string();
        match off_thread(move || me.runs.update(&id, f)).await {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("background run not saved: {}", e.message);
                None
            }
        }
    }

    /// Ожидающие запуски `ids` — сразу в итог (`cancelled`, `skipped`, `interrupted`). Блокирующий вызов.
    fn close_runs(&self, ids: &[String], status: RunStatus) {
        for id in ids {
            match self.runs.update(id, |r| {
                r.status = status;
                r.ended_at_ms = Some(now_ms());
            }) {
                Ok(r) => self.emit(&r),
                Err(e) => tracing::warn!("background run not saved: {}", e.message),
            }
        }
    }

    fn take_queued(&self, pred: impl Fn(&AgentRun) -> bool) -> Vec<String> {
        let mut s = self.state.lock_safe();
        let (taken, kept): (Vec<String>, Vec<String>) = s
            .queued
            .drain(..)
            .partition(|id| self.runs.get(id).is_some_and(|r| pred(&r)));
        s.queued = kept.into();
        taken
    }

    /// Остановка идущего запуска, если он подходит под `pred`.
    fn signal_current(&self, pred: impl Fn(&Current) -> bool, why: Stopped) -> bool {
        let mut s = self.state.lock_safe();
        let Some(c) = s.current.as_mut().filter(|c| pred(c)) else {
            return false;
        };
        if let Some(tx) = c.stop.take() {
            let _ = tx.send(why);
        }
        true
    }

    /// «Остановить» в списке запусков: из очереди — сразу `cancelled`; идущий останавливается, итог `cancelled` приходит
    /// событием. Запрос остаётся `waiting`. Блокирующий вызов.
    pub fn stop(&self, run_id: &str) -> AppResult<AgentRun> {
        let run = self
            .get(run_id)
            .ok_or_else(|| runs::run_not_found(run_id))?;
        if !run.status.is_active() {
            return Ok(run);
        }
        let taken = self.take_queued(|r| r.id == run_id);
        if taken.is_empty() {
            self.signal_current(|c| c.run_id == run_id, Stopped::Stop);
        } else {
            self.close_runs(&taken, RunStatus::Cancelled);
            self.d.mcp.emit_state();
        }
        self.get(run_id).ok_or_else(|| runs::run_not_found(run_id))
    }

    /// «Отменить запрос» (`mcp::llm::cancel_request`): запуск запроса снимается с очереди или останавливается. Блокирующий
    /// вызов.
    pub fn request_cancelled(&self, request_id: &str) {
        let taken = self.take_queued(|r| r.request_id.as_deref() == Some(request_id));
        self.close_runs(&taken, RunStatus::Cancelled);
        self.signal_current(
            |c| c.request_id.as_deref() == Some(request_id),
            Stopped::RequestCancelled,
        );
        self.d.mcp.emit_state();
    }

    /// После «Настроек»: выключенный режим снимает очередь (`skipped`, «Проверка» остаётся), текущий запуск доводится до
    /// конца (§3.2). Блокирующий вызов.
    pub fn settings_changed(&self) {
        if !self.d.settings.agent().enabled {
            let taken = self.take_queued(|r| r.kind == RunKind::Optimize);
            self.close_runs(&taken, RunStatus::Skipped);
        }
        self.d.mcp.emit_state();
    }

    /// Выход из Glafira (§4.4): очередь и текущий запуск — `interrupted`, процесс агента останавливается.
    pub async fn shutdown(&self) {
        let (queued, done) = {
            let mut s = self.state.lock_safe();
            s.closed = true;
            let queued: Vec<String> = s.queued.drain(..).collect();
            let done = s.current.as_mut().map(|c| {
                if let Some(tx) = c.stop.take() {
                    let _ = tx.send(Stopped::Shutdown);
                }
                c.done.clone()
            });
            (queued, done)
        };
        self.close_runs(&queued, RunStatus::Interrupted);
        if let Some(mut d) = done {
            let _ = d.wait_for(|x| *x).await;
        }
    }

    /// Запуск с ходом работы идущего запуска.
    pub fn get(&self, id: &str) -> Option<AgentRun> {
        self.runs.get(id).map(|r| self.overlay(r))
    }

    /// Сначала новые.
    pub fn list(&self, f: &RunFilter) -> Vec<AgentRun> {
        self.runs
            .list(f)
            .into_iter()
            .map(|r| self.overlay(r))
            .collect()
    }

    fn overlay(&self, mut r: AgentRun) -> AgentRun {
        if !r.status.is_active() {
            return r;
        }
        let s = self.state.lock_safe();
        if let Some(c) = s.current.as_ref().filter(|c| c.run_id == r.id) {
            if let Some(status) = c.live.status {
                r.status = status;
            }
            r.calls = c.live.calls;
            r.last_tool = c.live.last_tool.clone();
            r.last_call_at_ms = c.live.last_call_at_ms;
        }
        r
    }

    pub fn status(&self) -> AgentStatus {
        let mut status = AgentStatus::idle(&self.d.settings.agent(), &self.d.policy.status);
        let s = self.state.lock_safe();
        status.running = s.current.as_ref().map(|c| RunningRef {
            run_id: c.run_id.clone(),
            request_id: c.request_id.clone(),
        });
        status.queued = s.queued.len() as u32;
        status
    }

    /// Идёт запуск: «Выгрузить модель» неактивна (§6.2).
    pub fn is_busy(&self) -> bool {
        self.state.lock_safe().current.is_some()
    }

    pub fn log_file(&self, run_id: &str) -> AppResult<PathBuf> {
        self.runs
            .get(run_id)
            .ok_or_else(|| runs::run_not_found(run_id))?;
        let p = runs::log_path(&self.runs_dir, run_id);
        if p.is_file() {
            Ok(p)
        } else {
            Err(log_missing(run_id))
        }
    }

    /// «Показать журнал»: хвост до 256 КБ (у идущего запуска — то, что уже записано). Блокирующий вызов.
    pub fn read_log(&self, run_id: &str) -> AppResult<String> {
        let p = self.log_file(run_id)?;
        runs::read_log_tail(&p, runs::LOG_TAIL_BYTES).map_err(|_| log_missing(run_id))
    }

    fn live(&self, id: &str) -> Option<Live> {
        self.state
            .lock_safe()
            .current
            .as_ref()
            .filter(|c| c.run_id == id)
            .map(|c| c.live.clone())
    }

    fn set_live_status(&self, id: &str, status: RunStatus) {
        if let Some(c) = self
            .state
            .lock_safe()
            .current
            .as_mut()
            .filter(|c| c.run_id == id)
        {
            c.live.status = Some(status);
        }
    }

    /// Язык итога агента — язык интерфейса Glafira (политика → выбор → ОС).
    fn ui_language(&self) -> String {
        crate::settings::resolve(
            crate::settings::system_locale().as_deref(),
            self.d.settings.language().as_deref(),
            self.d.policy.status.language.as_deref(),
        )
        .0
    }

    fn emit(&self, r: &AgentRun) {
        if let Some(s) = self.sink.get() {
            s.run_changed(r);
        }
    }

    fn emit_live(&self, id: &str) {
        if let Some(r) = self.get(id) {
            self.emit(&r);
        }
    }

    /// Отложенное событие хода работы (не чаще `PROGRESS_EVERY`).
    fn flush_progress(&self, id: &str) {
        if let Some(c) = self
            .state
            .lock_safe()
            .current
            .as_mut()
            .filter(|c| c.run_id == id)
        {
            c.live.emit_pending = false;
            c.live.last_emit = Some(tokio::time::Instant::now());
        }
        self.emit_live(id);
    }

    fn connection_field(&self, id: Option<&str>) -> Value {
        id.map_or(Value::Null, |id| {
            let name = self
                .d
                .connections
                .get(id)
                .map(|p| p.name)
                .unwrap_or_default();
            json!({ "id": id, "name": name })
        })
    }

    /// `agentRunStarted` (спец. §9.2); промпт и вывод агента в журнал не пишутся.
    fn audit_started(&self, r: &AgentRun) {
        let mut f = Map::new();
        f.insert("runId".into(), r.id.clone().into());
        f.insert("requestId".into(), json!(r.request_id));
        f.insert(
            "connection".into(),
            self.connection_field(r.connection_id.as_deref()),
        );
        f.insert("preset".into(), r.preset.as_str().into());
        f.insert("model".into(), json!(r.model));
        self.d.mcp.record_audit(AuditEvent::App {
            kind: "agentRunStarted".into(),
            fields: f,
        });
    }

    /// `agentRunFinished` (спец. §9.2); итоговый текст в журнал не пишется.
    fn audit_finished(&self, r: &AgentRun) {
        let mut f = Map::new();
        f.insert("runId".into(), r.id.clone().into());
        f.insert("requestId".into(), json!(r.request_id));
        f.insert(
            "connection".into(),
            self.connection_field(r.connection_id.as_deref()),
        );
        f.insert("status".into(), r.status.as_str().into());
        if let Some(reason) = r.reason {
            f.insert("reason".into(), reason.as_str().into());
        }
        if let Some(code) = r.exit_code {
            f.insert("exitCode".into(), code.into());
        }
        let ms = r
            .ended_at_ms
            .zip(r.started_at_ms)
            .map_or(0, |(e, s)| u64::try_from(e - s).unwrap_or(0));
        f.insert("ms".into(), ms.into());
        f.insert("calls".into(), r.calls.into());
        f.insert("proposals".into(), r.proposal_ids.len().into());
        if let Some(c) = r.cost_usd {
            f.insert("costUsd".into(), json!(c));
        }
        self.d.mcp.record_audit(AuditEvent::App {
            kind: "agentRunFinished".into(),
            fields: f,
        });
    }
}

/// Ход работы от билетов (задача 4): подключение сессии и вызовы. Быстро, без ожиданий: вызывается из обвязки вызова.
/// Закрытие сессии (`connected = false`) очереди не нужно: после `Tickets::revoke` оно и не приходит.
impl RunObserver for AgentQueue {
    fn run_session(&self, run_id: &str, connected: bool) {
        if !connected {
            return;
        }
        if let Some(c) = self
            .state
            .lock_safe()
            .current
            .as_mut()
            .filter(|c| c.run_id == run_id)
        {
            c.live.connected = true;
        }
    }

    fn run_call(&self, run_id: &str, tool: &str, calls: u32) {
        enum Emit {
            Now,
            At(tokio::time::Instant),
            No,
        }
        let now = tokio::time::Instant::now();
        let emit = {
            let mut s = self.state.lock_safe();
            let Some(c) = s.current.as_mut().filter(|c| c.run_id == run_id) else {
                return;
            };
            let l = &mut c.live;
            l.connected = true;
            l.calls = l.calls.max(calls);
            l.last_tool = Some(tool.to_string());
            l.last_call_at_ms = Some(now_ms());
            if l.status != Some(RunStatus::Running) {
                // Первый вызов переводит запуск в `running` — событие сразу (§5.4).
                l.status = Some(RunStatus::Running);
                l.last_emit = Some(now);
                Emit::Now
            } else {
                match l.last_emit {
                    Some(t) if now < t + PROGRESS_EVERY => {
                        if l.emit_pending {
                            Emit::No
                        } else {
                            l.emit_pending = true;
                            Emit::At(t + PROGRESS_EVERY)
                        }
                    }
                    _ => {
                        l.last_emit = Some(now);
                        Emit::Now
                    }
                }
            }
        };
        match emit {
            Emit::Now => self.emit_live(run_id),
            Emit::At(at) => {
                if let (Some(me), Ok(rt)) =
                    (self.me.upgrade(), tokio::runtime::Handle::try_current())
                {
                    let id = run_id.to_string();
                    rt.spawn(async move {
                        tokio::time::sleep_until(at).await;
                        me.flush_progress(&id);
                    });
                }
            }
            Emit::No => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use futures_util::future::BoxFuture;
    use glafira_pg_mcp::server::RUN_TOOL_NAMES;
    use tokio::task::JoinHandle;

    use super::*;
    use crate::agent::presets::CommandSpec;
    use crate::agent::process::{AgentProcess, ExitInfo, InitWatch};
    use crate::agent::settings::{ClaudeSettings, QwenSettings};
    use crate::conn::profile::sample_profile;
    use crate::mcp::McpServerState;
    use crate::mcp::llm::proposals::{self, NewProposal};
    use crate::mcp::llm::requests;
    use crate::secrets::MemoryStore;
    use crate::state::AppState;

    const MODEL: &str = "qwen3.6-64k:35b-a3b";
    const RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"No better plan: the index on orders(status) is already used","total_cost_usd":0.0123}"#;

    /// Шаг сценария фальшивого агента.
    #[derive(Debug, Clone)]
    enum Step {
        /// Событие `system`/`init` потока: инструменты модели. Сценарий без `Init` и `NoInit` начинается с `init`
        /// девяти инструментов Glafira.
        Init(Vec<String>),
        /// Агент не сообщает список инструментов: событие `init` не приходит.
        NoInit,
        /// Сессия с билетом подтверждена.
        Connect,
        Call(&'static str),
        /// Столько вызовов подряд.
        Calls(u32),
        /// `propose_query_rewrite`: предложение с `runId` этого запуска, запрос — `answered`.
        Propose,
        Out(&'static str),
        Sleep(u64),
        Exit(i32),
        Hang,
    }

    /// Инструменты сессии запуска, как их называет CLI.
    fn glafira_tools() -> Vec<String> {
        RUN_TOOL_NAMES
            .iter()
            .map(|t| format!("mcp__glafira-pg__{t}"))
            .collect()
    }

    /// `init` с инструментами Glafira и лишними `extra`.
    fn init(extra: &[&str]) -> Step {
        let mut tools = glafira_tools();
        tools.extend(extra.iter().map(|s| s.to_string()));
        Step::Init(tools)
    }

    #[derive(Clone)]
    struct Spawned {
        run_id: String,
        spec: CommandSpec,
        stdin: String,
        config: PathBuf,
        command: String,
        ticket: String,
        /// Билет при запуске опознаётся как билет этого запуска.
        ticket_live: bool,
        /// Область запуска при запуске: подключение и запрос.
        scope: Option<(Option<String>, Option<String>)>,
    }

    #[derive(Default)]
    struct FakeSpawner {
        scripts: Mutex<VecDeque<Result<Vec<Step>, io::ErrorKind>>>,
        queue: OnceLock<Weak<AgentQueue>>,
        llm: OnceLock<Arc<LlmStores>>,
        tickets: OnceLock<Arc<Tickets>>,
        spawned: Mutex<Vec<Spawned>>,
        alive: Arc<AtomicU32>,
        max_alive: Arc<AtomicU32>,
        stops: Arc<AtomicU32>,
    }

    impl FakeSpawner {
        fn script(&self, steps: Vec<Step>) {
            self.scripts.lock().unwrap().push_back(Ok(steps));
        }

        fn fail(&self, kind: io::ErrorKind) {
            self.scripts.lock().unwrap().push_back(Err(kind));
        }

        fn attach(&self, st: &AppState) {
            let _ = self.queue.set(Arc::downgrade(&st.agent));
            let _ = self.llm.set(st.llm.clone());
            let _ = self.tickets.set(st.tickets.clone());
        }

        fn spawned(&self) -> Vec<Spawned> {
            self.spawned.lock().unwrap().clone()
        }
    }

    struct AliveGuard(Arc<AtomicU32>);

    impl Drop for AliveGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl AgentSpawner for FakeSpawner {
        fn spawn(
            &self,
            spec: &CommandSpec,
            stdin: String,
            log: PathBuf,
        ) -> io::Result<Box<dyn AgentProcess>> {
            let run_id = log.file_stem().unwrap().to_string_lossy().into_owned();
            let config = spec
                .args
                .iter()
                .position(|a| a == "--mcp-config")
                .map(|i| PathBuf::from(&spec.args[i + 1]))
                .expect("--mcp-config");
            let v: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
            let server = &v["mcpServers"]["glafira-pg"];
            let ticket = server["env"]["GLAFIRA_AGENT_TICKET"]
                .as_str()
                .unwrap()
                .to_string();
            let tickets = self.tickets.get().unwrap();
            self.spawned.lock().unwrap().push(Spawned {
                run_id: run_id.clone(),
                spec: spec.clone(),
                stdin,
                config: config.clone(),
                command: server["command"].as_str().unwrap_or_default().to_string(),
                ticket_live: tickets.resolve(&ticket).as_deref() == Some(run_id.as_str()),
                scope: tickets
                    .scope(&run_id)
                    .map(|s| (s.connection_id, s.request_id)),
                ticket,
            });
            let next = self.scripts.lock().unwrap().pop_front();
            let mut steps = match next {
                Some(Ok(s)) => s,
                Some(Err(kind)) => return Err(io::Error::from(kind)),
                None => vec![
                    Step::Connect,
                    Step::Call("get_optimization_request"),
                    Step::Exit(0),
                ],
            };
            if !steps
                .iter()
                .any(|s| matches!(s, Step::Init(_) | Step::NoInit))
            {
                steps.insert(0, init(&[]));
            }
            let n = self.alive.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_alive.fetch_max(n, Ordering::SeqCst);
            let guard = AliveGuard(self.alive.clone());
            let (q, llm) = (
                self.queue.get().unwrap().clone(),
                self.llm.get().unwrap().clone(),
            );
            let (init_tx, init) = InitWatch::channel();
            let task = tokio::spawn(async move {
                let _guard = guard;
                play(steps, run_id, q, llm, init_tx).await
            });
            Ok(Box::new(FakeProcess {
                task,
                result: None,
                init,
                stops: self.stops.clone(),
            }))
        }
    }

    async fn play(
        steps: Vec<Step>,
        run_id: String,
        q: Weak<AgentQueue>,
        llm: Arc<LlmStores>,
        init_tx: oneshot::Sender<Vec<String>>,
    ) -> ExitInfo {
        // Без события `init` отправитель живёт, пока идёт сценарий: «поток кончился» — конец сценария.
        let mut init_tx = Some(init_tx);
        let mut out = String::new();
        let mut calls = 0;
        for step in steps {
            match step {
                Step::Init(tools) => {
                    if let Some(tx) = init_tx.take() {
                        let _ = tx.send(tools);
                    }
                }
                Step::NoInit => {}
                Step::Connect => {
                    if let Some(q) = q.upgrade() {
                        q.run_session(&run_id, true);
                    }
                }
                Step::Call(tool) => {
                    calls += 1;
                    if let Some(q) = q.upgrade() {
                        q.run_call(&run_id, tool, calls);
                    }
                }
                Step::Calls(n) => {
                    for _ in 0..n {
                        calls += 1;
                        if let Some(q) = q.upgrade() {
                            q.run_call(&run_id, "explain_query", calls);
                        }
                    }
                }
                Step::Propose => {
                    let q = q.upgrade().unwrap();
                    let req = q.get(&run_id).and_then(|r| r.request_id).unwrap();
                    let (p, _) = llm
                        .proposals
                        .add_or_existing(NewProposal {
                            run_id: Some(run_id.clone()),
                            ..proposals::tests::new(Some(&req), "SELECT 2", &[])
                        })
                        .unwrap();
                    llm.requests.attach_proposal(&req, &p.id).unwrap();
                }
                Step::Out(s) => out.push_str(s),
                Step::Sleep(s) => tokio::time::sleep(Duration::from_secs(s)).await,
                Step::Exit(code) => {
                    return ExitInfo {
                        code: Some(code),
                        stdout_tail: out,
                    };
                }
                Step::Hang => std::future::pending::<()>().await,
            }
        }
        ExitInfo {
            code: Some(0),
            stdout_tail: out,
        }
    }

    struct FakeProcess {
        task: JoinHandle<ExitInfo>,
        result: Option<ExitInfo>,
        init: InitWatch,
        stops: Arc<AtomicU32>,
    }

    impl AgentProcess for FakeProcess {
        fn wait(&mut self) -> BoxFuture<'_, ExitInfo> {
            Box::pin(async move {
                if let Some(r) = &self.result {
                    return r.clone();
                }
                // Прерванная задача — «убит сигналом»: кода нет.
                let r = (&mut self.task).await.unwrap_or_default();
                self.result = Some(r.clone());
                r
            })
        }

        fn stop(&mut self) -> BoxFuture<'_, ()> {
            Box::pin(async move {
                self.stops.fetch_add(1, Ordering::SeqCst);
                self.task.abort();
            })
        }

        fn init_tools(&mut self) -> BoxFuture<'_, Option<Vec<String>>> {
            Box::pin(self.init.get())
        }
    }

    #[derive(Default)]
    struct FakeWarmth {
        warnings: Vec<String>,
        before: Mutex<Vec<Option<String>>>,
        after: Mutex<Vec<Option<String>>>,
    }

    impl ModelWarmth for FakeWarmth {
        fn before_run(&self, model: Option<&str>) -> BoxFuture<'_, Vec<String>> {
            self.before.lock().unwrap().push(model.map(String::from));
            let w = self.warnings.clone();
            Box::pin(async move { w })
        }

        fn after_run(&self, model: Option<&str>) -> BoxFuture<'_, ()> {
            self.after.lock().unwrap().push(model.map(String::from));
            Box::pin(async {})
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<AgentRun>>);

    impl RunsSink for Events {
        fn run_changed(&self, run: &AgentRun) {
            self.0.lock().unwrap().push(run.clone());
        }
    }

    struct Harness {
        dir: tempfile::TempDir,
        st: AppState,
        fake: Arc<FakeSpawner>,
        events: Arc<Events>,
    }

    impl Harness {
        fn q(&self) -> &Arc<AgentQueue> {
            &self.st.agent
        }
    }

    /// Приложение с подключением `c1` (уровень «Диагностика»), MCP-сервером «работает», Qwen Code с моделью и фальшивым
    /// агентом.
    fn harness_full(
        warmth: Arc<dyn ModelWarmth>,
        run_timeout: Option<Duration>,
        edit: impl FnOnce(&mut AgentSettings),
    ) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let fake = Arc::new(FakeSpawner::default());
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .with_agent(AgentRuntime {
                spawner: fake.clone(),
                warmth,
                shim_exe: PathBuf::from("/opt/glafira/glafira-pg"),
                run_timeout,
            });
        let mut p = sample_profile("c1");
        p.llm_access = LlmAccess::Diagnostics;
        st.connections.upsert(p).unwrap();
        let mut a = AgentSettings {
            enabled: true,
            qwen: QwenSettings {
                path: "/opt/agents/qwen".into(),
                model: MODEL.into(),
            },
            ..AgentSettings::default()
        };
        edit(&mut a);
        st.settings.set_agent(a).unwrap();
        st.settings.set_language(Some("en")).unwrap();
        st.mcp.force_server(McpServerState::Running);
        fake.attach(&st);
        let events = Arc::new(Events::default());
        st.agent.set_sink(events.clone());
        Harness {
            dir,
            st,
            fake,
            events,
        }
    }

    fn harness_with(
        warmth: Arc<dyn ModelWarmth>,
        edit: impl FnOnce(&mut AgentSettings),
    ) -> Harness {
        harness_full(warmth, None, edit)
    }

    fn harness() -> Harness {
        harness_with(Arc::new(NoWarmth), |_| {})
    }

    fn request(h: &Harness) -> String {
        h.st.llm
            .requests
            .create(requests::tests::new("c1"))
            .unwrap()
            .id
    }

    fn done(r: &AgentRun) -> bool {
        !r.status.is_active()
    }

    /// Ждёт условия по запуску до 2000 с остановленного времени.
    async fn until(h: &Harness, id: &str, what: impl Fn(&AgentRun) -> bool) -> AgentRun {
        for _ in 0..4000 {
            if let Some(r) = h.q().get(id)
                && what(&r)
            {
                return r;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        panic!("не дождались запуска {id}: {:?}", h.q().get(id));
    }

    fn talk(seconds: u64) -> Vec<Step> {
        vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Sleep(seconds),
            Step::Out(RESULT),
            Step::Exit(0),
        ]
    }

    #[tokio::test(start_paused = true)]
    async fn runs_go_one_at_a_time_in_fifo_order() {
        let h = harness();
        for _ in 0..3 {
            h.fake.script(talk(10));
        }
        let (ra, rb, rc) = (request(&h), request(&h), request(&h));
        let a = h.q().start(&ra).unwrap();
        let b = h.q().start(&rb).unwrap();
        let c = h.q().start(&rc).unwrap();
        assert_eq!(
            (a.status, a.kind, a.preset, a.model.as_deref()),
            (
                RunStatus::Queued,
                RunKind::Optimize,
                Preset::Qwen,
                Some(MODEL)
            )
        );
        until(&h, &a.id, |r| r.status == RunStatus::Running).await;
        assert_eq!(h.q().get(&b.id).unwrap().status, RunStatus::Queued);
        assert_eq!(h.q().get(&c.id).unwrap().status, RunStatus::Queued);
        let status = h.st.mcp.status().agent;
        assert_eq!(
            status.running,
            Some(RunningRef {
                run_id: a.id.clone(),
                request_id: Some(ra.clone())
            })
        );
        assert_eq!(status.queued, 2);
        assert!(status.policy_enabled && status.enabled && status.configured);
        let last = until(&h, &c.id, done).await;
        assert_eq!(last.status, RunStatus::Done);
        let a = h.q().get(&a.id).unwrap();
        assert_eq!(
            (a.status, a.reason, a.exit_code),
            (RunStatus::Done, None, Some(0))
        );
        assert_eq!(
            a.summary.as_deref(),
            Some("No better plan: the index on orders(status) is already used")
        );
        assert_eq!(a.cost_usd, None, "у Qwen Code стоимости нет");
        assert_eq!(
            (a.calls, a.last_tool.as_deref()),
            (1, Some("get_optimization_request"))
        );
        assert!(a.proposal_ids.is_empty() && a.started_at_ms.is_some() && a.ended_at_ms.is_some());
        assert!(a.warnings.is_empty(), "{:?}", a.warnings);
        let order: Vec<String> = h.fake.spawned().into_iter().map(|s| s.run_id).collect();
        assert_eq!(order, [a.id.clone(), b.id.clone(), c.id.clone()]);
        assert_eq!(
            h.fake.max_alive.load(Ordering::SeqCst),
            1,
            "один запуск за раз"
        );
        assert!(
            h.fake.spawned()[0]
                .spec
                .args
                .windows(2)
                .any(|w| w == ["--max-wall-time", "930s"]),
            "срок из настроек (15 мин) + 30 с"
        );
        assert_eq!(
            h.st.llm.requests.get(&ra).unwrap().status,
            RequestStatus::Waiting,
            "без предложений запрос ждёт"
        );
        // Фильтр списка: по запросу и по подключению, сначала новые.
        let mine = h.q().list(&RunFilter {
            request_ids: Some(vec![rb.clone()]),
            ..RunFilter::default()
        });
        assert_eq!(
            mine.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            [b.id.as_str()]
        );
        let all = h.q().list(&RunFilter {
            connection_id: Some("c1".into()),
            ..RunFilter::default()
        });
        assert_eq!(
            all.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            [last.id, b.id, a.id]
        );
        assert_eq!(h.st.mcp.status().agent.running, None);
    }

    #[tokio::test(start_paused = true)]
    async fn the_cost_is_kept_only_for_claude_code_without_ollama() {
        for (via_ollama, cost) in [(false, Some(0.0123)), (true, None)] {
            let h = harness_with(Arc::new(NoWarmth), |a| {
                a.preset = Preset::Claude;
                a.claude = ClaudeSettings {
                    path: "/opt/agents/claude".into(),
                    model: MODEL.into(),
                    via_ollama,
                    budget_usd: Some(1.0),
                };
            });
            h.fake.script(talk(1));
            let run = h.q().start(&request(&h)).unwrap();
            let run = until(&h, &run.id, done).await;
            assert_eq!(
                (run.status, run.preset, run.cost_usd),
                (RunStatus::Done, Preset::Claude, cost),
                "через Ollama стоимость выдуманная: {via_ollama}"
            );
            let finished =
                h.st.mcp
                    .audit_tail(20)
                    .into_iter()
                    .find(|e| e["event"] == "agentRunFinished")
                    .unwrap();
            assert_eq!(finished.get("costUsd").and_then(Value::as_f64), cost);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_queue_holds_ten_runs_and_shutdown_interrupts_everything() {
        let h = harness();
        h.fake.script(vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Hang,
        ]);
        let first = h.q().start(&request(&h)).unwrap();
        until(&h, &first.id, |r| r.status == RunStatus::Running).await;
        let mut queued = Vec::new();
        for _ in 0..MAX_QUEUE {
            queued.push(h.q().start(&request(&h)).unwrap());
        }
        assert_eq!(
            h.q().start(&request(&h)).unwrap_err().key,
            "errors.agent.queueFull"
        );
        let again = h
            .q()
            .start(queued[3].request_id.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            again.id, queued[3].id,
            "повторное нажатие не добавляет запуск"
        );
        assert_eq!(h.q().status().queued, MAX_QUEUE as u32);
        assert!(h.q().is_busy());
        h.q().shutdown().await;
        assert_eq!(h.q().get(&first.id).unwrap().status, RunStatus::Interrupted);
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1, "процесс остановлен");
        assert!(
            queued
                .iter()
                .all(|r| h.q().get(&r.id).unwrap().status == RunStatus::Interrupted)
        );
        assert!(!h.q().is_busy());
    }

    #[tokio::test(start_paused = true)]
    async fn a_hanging_agent_is_stopped_at_the_deadline() {
        let h = harness_with(Arc::new(NoWarmth), |a| a.timeout_min = 2);
        h.fake
            .script(vec![Step::Connect, Step::Call("explain_query"), Step::Hang]);
        let req = request(&h);
        let run = h.q().start(&req).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(
            (run.status, run.reason),
            (RunStatus::Failed, Some(FailReason::Timeout))
        );
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1);
        assert_eq!(h.fake.alive.load(Ordering::SeqCst), 0, "процесс остановлен");
        assert_eq!(
            (run.calls, run.last_tool.as_deref()),
            (1, Some("explain_query"))
        );
        let s = &h.fake.spawned()[0];
        assert!(s.ticket_live, "на время запуска билет действует");
        assert!(
            glafira_mcp_core::handshake::valid_ticket(&s.ticket),
            "в конфиге — ровно 64 hex: {:?}",
            s.ticket
        );
        assert_eq!(
            h.st.tickets.resolve(&s.ticket),
            None,
            "после запуска билет отозван"
        );
        assert!(!s.config.exists(), "MCP-конфиг запуска удалён");
        assert!(
            s.spec
                .args
                .windows(2)
                .any(|w| w == ["--max-wall-time", "150s"]),
            "запасной предел CLI — срок + 30 с: {:?}",
            s.spec.args
        );
        assert_eq!(
            h.st.llm.requests.get(&req).unwrap().status,
            RequestStatus::Waiting
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_test_run_timeout_replaces_the_setting() {
        let h = harness_full(Arc::new(NoWarmth), Some(Duration::from_secs(45)), |_| {});
        h.fake.script(vec![Step::Connect, Step::Hang]);
        let run = h.q().start(&request(&h)).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(run.reason, Some(FailReason::Timeout));
        assert!(
            h.fake.spawned()[0]
                .spec
                .args
                .windows(2)
                .any(|w| w == ["--max-wall-time", "75s"])
        );
    }

    #[tokio::test(start_paused = true)]
    async fn tools_beyond_glafira_stop_the_run_before_the_first_turn() {
        let h = harness();
        h.fake.script(vec![
            init(&["run_shell_command", "web_fetch"]),
            Step::Sleep(1),
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Exit(0),
        ]);
        let req = request(&h);
        let run = h.q().start(&req).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(
            (run.status, run.reason),
            (RunStatus::Failed, Some(FailReason::UnsafeTools))
        );
        assert_eq!(run.warnings, ["unsafe run_shell_command,web_fetch"]);
        assert_eq!(
            (run.calls, run.last_tool.as_deref()),
            (0, None),
            "вызовов нет"
        );
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1, "процесс остановлен");
        assert_eq!(h.fake.alive.load(Ordering::SeqCst), 0);
        let s = &h.fake.spawned()[0];
        assert_eq!(h.st.tickets.resolve(&s.ticket), None, "билет отозван");
        assert!(!s.config.exists());
        assert_eq!(
            h.st.llm.requests.get(&req).unwrap().status,
            RequestStatus::Waiting
        );
        let finished =
            h.st.mcp
                .audit_tail(20)
                .into_iter()
                .find(|e| e["event"] == "agentRunFinished")
                .unwrap();
        assert_eq!(finished["reason"], "unsafeTools");
    }

    #[tokio::test(start_paused = true)]
    async fn only_glafira_tools_let_the_run_go_on() {
        let h = harness();
        h.fake.script(vec![
            Step::Init(glafira_tools()),
            Step::Sleep(1),
            Step::Connect,
            Step::Call("propose_query_rewrite"),
            Step::Propose,
            Step::Out(RESULT),
            Step::Exit(0),
        ]);
        let run = h.q().start(&request(&h)).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!((run.status, run.reason), (RunStatus::Done, None));
        assert_eq!((run.calls, run.proposal_ids.len()), (1, 1));
        assert!(run.warnings.is_empty());
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn no_init_event_within_a_minute_is_unsafe() {
        let h = harness();
        h.fake.script(vec![Step::NoInit, Step::Hang]);
        let started = tokio::time::Instant::now();
        let run = h.q().start(&request(&h)).unwrap();
        until(&h, &run.id, |r| r.status == RunStatus::Starting).await;
        tokio::time::sleep(Duration::from_secs(59)).await;
        assert_eq!(
            h.q().get(&run.id).unwrap().status,
            RunStatus::Starting,
            "минуту запуск ждёт"
        );
        let run = until(&h, &run.id, done).await;
        let waited = started.elapsed();
        assert!(
            waited >= process::INIT_TIMEOUT
                && waited < process::INIT_TIMEOUT + Duration::from_secs(2),
            "{waited:?}"
        );
        assert_eq!(
            (run.status, run.reason),
            (RunStatus::Failed, Some(FailReason::UnsafeTools))
        );
        assert_eq!(run.warnings, ["unsafe no-init"]);
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1);
        // Поток кончился без события `init` — то же сразу.
        h.fake.script(vec![
            Step::NoInit,
            Step::Connect,
            Step::Call("list_connections"),
            Step::Exit(0),
        ]);
        let run = h.q().start(&request(&h)).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(
            (run.status, run.reason, run.warnings.as_slice()),
            (
                RunStatus::Failed,
                Some(FailReason::UnsafeTools),
                &["unsafe no-init".to_string()][..]
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stopping_while_the_tools_are_awaited_cancels_the_run() {
        let h = harness();
        h.fake.script(vec![Step::NoInit, Step::Hang]);
        let run = h.q().start(&request(&h)).unwrap();
        until(&h, &run.id, |r| r.status == RunStatus::Starting).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        h.q().stop(&run.id).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!((run.status, run.reason), (RunStatus::Cancelled, None));
        assert!(run.warnings.is_empty());
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_the_request_dequeues_or_stops_its_run() {
        let h = harness();
        h.fake.script(vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Hang,
        ]);
        let (ra, rb) = (request(&h), request(&h));
        let a = h.q().start(&ra).unwrap();
        let b = h.q().start(&rb).unwrap();
        until(&h, &a.id, |r| r.status == RunStatus::Running).await;
        crate::mcp::llm::cancel_request(&h.st, &rb).await.unwrap();
        assert_eq!(h.q().get(&b.id).unwrap().status, RunStatus::Cancelled);
        crate::mcp::llm::cancel_request(&h.st, &ra).await.unwrap();
        let a = until(&h, &a.id, done).await;
        assert_eq!((a.status, a.reason), (RunStatus::Cancelled, None));
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1);
        assert_eq!(
            h.fake.spawned().len(),
            1,
            "снятый с очереди запуск не стартовал"
        );
        assert_eq!(
            h.st.llm.requests.get(&ra).unwrap().status,
            RequestStatus::Cancelled
        );
    }

    #[tokio::test(start_paused = true)]
    async fn stop_cancels_the_run_and_keeps_the_request_waiting() {
        let h = harness();
        h.fake.script(vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Hang,
        ]);
        let (ra, rb) = (request(&h), request(&h));
        let a = h.q().start(&ra).unwrap();
        let b = h.q().start(&rb).unwrap();
        until(&h, &a.id, |r| r.status == RunStatus::Running).await;
        assert_eq!(
            h.q().stop(&b.id).unwrap().status,
            RunStatus::Cancelled,
            "из очереди — сразу"
        );
        assert!(
            h.q().stop(&a.id).unwrap().status.is_active(),
            "идущий останавливается, итог придёт событием"
        );
        let a = until(&h, &a.id, done).await;
        assert_eq!(a.status, RunStatus::Cancelled);
        assert_eq!(h.fake.stops.load(Ordering::SeqCst), 1);
        assert_eq!(
            h.st.llm.requests.get(&ra).unwrap().status,
            RequestStatus::Waiting
        );
        assert_eq!(
            h.q().stop(&a.id).unwrap().status,
            RunStatus::Cancelled,
            "повторная остановка ничего не меняет"
        );
        assert_eq!(
            h.q().stop("a000000").unwrap_err().key,
            "errors.agent.runNotFound"
        );
        // «Повторить» — новый запуск того же запроса.
        let again = h.q().start(&ra).unwrap();
        assert_ne!(again.id, a.id);
        assert_eq!(until(&h, &again.id, done).await.status, RunStatus::Done);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_answered_before_its_turn_is_skipped() {
        let h = harness();
        h.fake.script(talk(30));
        let (ra, rb) = (request(&h), request(&h));
        let a = h.q().start(&ra).unwrap();
        let b = h.q().start(&rb).unwrap();
        until(&h, &a.id, |r| r.status == RunStatus::Running).await;
        // Ответ из чата пришёл, пока запуск ждал очереди.
        h.st.llm.requests.attach_proposal(&rb, "p000001").unwrap();
        let b = until(&h, &b.id, done).await;
        assert_eq!(
            (b.status, b.reason, b.started_at_ms),
            (RunStatus::Skipped, None, None)
        );
        assert_eq!(h.fake.spawned().len(), 1);
        assert!(
            !h.st
                .mcp
                .audit_tail(50)
                .iter()
                .any(|e| e["event"] == "agentRunStarted" && e["runId"] == b.id.as_str()),
            "пропущенный запуск не начинался"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_repeat_of_a_chat_proposal_is_not_counted_for_the_run() {
        // Дедупликация P4b отдаёт уже сохранённое такое же предложение (здесь — чата, `runId: null`): запуску оно не
        // засчитывается (уточнение 13 задач 5–7).
        let h = harness();
        let req = request(&h);
        h.fake.script(vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Sleep(5),
            Step::Call("propose_query_rewrite"),
            Step::Propose,
            Step::Out(RESULT),
            Step::Exit(0),
        ]);
        let run = h.q().start(&req).unwrap();
        until(&h, &run.id, |r| r.status == RunStatus::Running).await;
        // Чат ответил тем же предложением, пока запуск работал.
        let (chat, created) =
            h.st.llm
                .proposals
                .add_or_existing(proposals::tests::new(Some(&req), "SELECT 2", &[]))
                .unwrap();
        assert!(created && chat.run_id.is_none());
        h.st.llm.requests.attach_proposal(&req, &chat.id).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(
            (run.status, run.reason, run.proposal_ids.len()),
            (RunStatus::Done, None, 0),
            "код 0 без своих предложений — done с пустыми proposalIds"
        );
        assert_eq!(
            h.st.llm.proposals.get(&chat.id).unwrap().run_id,
            None,
            "предложение чата не стало предложением запуска"
        );
        assert_eq!(
            h.st.llm.requests.get(&req).unwrap().status,
            RequestStatus::Answered
        );
    }

    #[test]
    fn runs_left_by_a_closed_glafira_are_interrupted_on_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::open(dir.path());
        let make = |status: RunStatus| {
            let r = store
                .create(NewRun {
                    kind: RunKind::Optimize,
                    request_id: Some("r1".into()),
                    connection_id: Some("c1".into()),
                    preset: Preset::Qwen,
                    model: None,
                })
                .unwrap();
            store.update(&r.id, |x| x.status = status).unwrap()
        };
        let ids: Vec<String> = [RunStatus::Queued, RunStatus::Starting, RunStatus::Running]
            .into_iter()
            .map(|s| make(s).id)
            .collect();
        let finished = make(RunStatus::Done);
        let runs_dir = dir.path().join(runs::RUNS_DIR);
        runs::ensure_private_dir(&runs_dir).unwrap();
        std::fs::write(runs::mcp_config_path(&runs_dir, &ids[2]), "{}").unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        for id in &ids {
            let got = st.agent.get(id).unwrap();
            assert_eq!(got.status, RunStatus::Interrupted, "{id}");
            assert!(got.ended_at_ms.is_some());
        }
        assert_eq!(st.agent.get(&finished.id).unwrap().status, RunStatus::Done);
        assert!(
            !runs::mcp_config_path(&runs_dir, &ids[2]).exists(),
            "конфиг с билетом прошлого сеанса удалён"
        );
        assert_eq!(st.agent.status().queued, 0, "очередь не восстанавливается");
    }

    #[tokio::test(start_paused = true)]
    async fn turning_the_assistant_off_skips_the_queue_but_finishes_the_current_run() {
        let h = harness();
        h.fake.script(talk(30));
        let ids: Vec<String> = (0..3)
            .map(|_| h.q().start(&request(&h)).unwrap().id)
            .collect();
        until(&h, &ids[0], |r| r.status == RunStatus::Running).await;
        let mut off = h.st.settings.agent();
        off.enabled = false;
        h.st.settings.set_agent(off).unwrap();
        h.q().settings_changed();
        for id in &ids[1..] {
            assert_eq!(h.q().get(id).unwrap().status, RunStatus::Skipped);
        }
        assert_eq!(until(&h, &ids[0], done).await.status, RunStatus::Done);
        assert_eq!(h.fake.spawned().len(), 1);
        assert_eq!(
            h.q().start(&request(&h)).unwrap_err().key,
            "errors.agent.off"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_that_meets_the_switched_off_mode_is_skipped() {
        // Режим выключили в файле настроек, а `settings_changed` не позвали: перепроверка перед стартом — тоже `skipped`.
        let h = harness();
        h.fake.script(talk(30));
        let (ra, rb) = (request(&h), request(&h));
        let a = h.q().start(&ra).unwrap();
        let b = h.q().start(&rb).unwrap();
        until(&h, &a.id, |r| r.status == RunStatus::Running).await;
        let mut off = h.st.settings.agent();
        off.enabled = false;
        h.st.settings.set_agent(off).unwrap();
        let b = until(&h, &b.id, done).await;
        assert_eq!(
            (b.status, b.reason, b.started_at_ms),
            (RunStatus::Skipped, None, None)
        );
    }

    /// Сценарий (или ошибка запуска) и ожидаемый итог.
    type Case = (
        Result<Vec<Step>, io::ErrorKind>,
        RunStatus,
        Option<FailReason>,
    );

    #[tokio::test(start_paused = true)]
    async fn finished_processes_get_their_outcome() {
        let h = harness();
        let cases: Vec<Case> = vec![
            (
                Ok(vec![
                    Step::Connect,
                    Step::Call("propose_query_rewrite"),
                    Step::Propose,
                    Step::Exit(0),
                ]),
                RunStatus::Done,
                None,
            ),
            (
                Ok(vec![
                    Step::Connect,
                    Step::Call("explain_query"),
                    Step::Exit(2),
                ]),
                RunStatus::Failed,
                Some(FailReason::Exit),
            ),
            (
                Ok(vec![Step::Exit(0)]),
                RunStatus::Failed,
                Some(FailReason::NotConnected),
            ),
            (
                Ok(vec![
                    Step::Connect,
                    Step::Calls(3),
                    Step::Exit(QWEN_EXIT_LIMIT),
                ]),
                RunStatus::Failed,
                Some(FailReason::Limit),
            ),
            (
                Ok(vec![
                    Step::Connect,
                    Step::Calls(MAX_RUN_CALLS + 1),
                    Step::Exit(0),
                ]),
                RunStatus::Failed,
                Some(FailReason::Limit),
            ),
            (
                Ok(vec![
                    Step::Connect,
                    Step::Calls(MAX_RUN_CALLS + 1),
                    Step::Propose,
                    Step::Exit(1),
                ]),
                RunStatus::Done,
                None,
            ),
            (
                Err(io::ErrorKind::NotFound),
                RunStatus::Failed,
                Some(FailReason::CliNotFound),
            ),
            (
                Err(io::ErrorKind::PermissionDenied),
                RunStatus::Failed,
                Some(FailReason::SpawnFailed),
            ),
        ];
        let mut started = Vec::new();
        for (script, ..) in &cases {
            match script {
                Ok(s) => h.fake.script(s.clone()),
                Err(k) => h.fake.fail(*k),
            }
            started.push(h.q().start(&request(&h)).unwrap());
        }
        for (run, (_, status, reason)) in started.iter().zip(&cases) {
            let got = until(&h, &run.id, done).await;
            assert_eq!((got.status, got.reason), (*status, *reason), "{got:?}");
        }
        let proposed = h.q().get(&started[0].id).unwrap();
        assert_eq!(proposed.proposal_ids.len(), 1);
        let p = h.st.llm.proposals.get(&proposed.proposal_ids[0]).unwrap();
        assert_eq!(p.run_id.as_deref(), Some(proposed.id.as_str()));
        assert_eq!(
            h.st.llm
                .requests
                .get(started[0].request_id.as_deref().unwrap())
                .unwrap()
                .status,
            RequestStatus::Answered
        );
        assert_eq!(h.q().get(&started[1].id).unwrap().exit_code, Some(2));
        let missing = h.q().get(&started[6].id).unwrap();
        assert_eq!(
            (missing.exit_code, missing.path.as_deref()),
            (None, Some("/opt/agents/qwen"))
        );
        assert!(missing.started_at_ms.is_some());
    }

    /// Код выхода Qwen Code при пределе CLI.
    const QWEN_EXIT_LIMIT: i32 = process::QWEN_LIMIT_EXIT;

    #[test]
    fn the_outcome_table() {
        use FailReason as R;
        use RunStatus as S;
        let base = EndFacts {
            kind: RunKind::Optimize,
            stopped: None,
            unsafe_tools: false,
            code: Some(0),
            connected: true,
            calls: 5,
            cli_limit: false,
            proposals: 0,
            mcp_running: true,
            access_ok: true,
        };
        let f = |patch: &dyn Fn(&mut EndFacts)| {
            let mut x = base.clone();
            patch(&mut x);
            outcome(&x)
        };
        assert_eq!(f(&|_| {}), (S::Done, None));
        assert_eq!(f(&|x| x.proposals = 2), (S::Done, None));
        assert_eq!(
            f(&|x| x.stopped = Some(Stopped::Timeout)),
            (S::Failed, Some(R::Timeout))
        );
        assert_eq!(
            f(&|x| {
                x.stopped = Some(Stopped::Timeout);
                x.proposals = 1;
            }),
            (S::Failed, Some(R::Timeout))
        );
        assert_eq!(
            f(&|x| x.stopped = Some(Stopped::Stop)),
            (S::Cancelled, None)
        );
        assert_eq!(
            f(&|x| x.stopped = Some(Stopped::RequestCancelled)),
            (S::Cancelled, None)
        );
        assert_eq!(
            f(&|x| x.stopped = Some(Stopped::Shutdown)),
            (S::Interrupted, None)
        );
        assert_eq!(
            f(&|x| {
                x.unsafe_tools = true;
                x.connected = false;
                x.code = None;
            }),
            (S::Failed, Some(R::UnsafeTools))
        );
        assert_eq!(
            f(&|x| {
                x.unsafe_tools = true;
                x.stopped = Some(Stopped::Stop);
            }),
            (S::Cancelled, None),
            "остановка пользователя важнее"
        );
        assert_eq!(
            f(&|x| x.connected = false),
            (S::Failed, Some(R::NotConnected))
        );
        assert_eq!(
            f(&|x| {
                x.connected = false;
                x.mcp_running = false;
            }),
            (S::Failed, Some(R::McpOff))
        );
        assert_eq!(f(&|x| x.mcp_running = false), (S::Failed, Some(R::McpOff)));
        assert_eq!(
            f(&|x| {
                x.mcp_running = false;
                x.proposals = 1;
            }),
            (S::Done, None),
            "предложение пришло до выключения"
        );
        assert_eq!(
            f(&|x| {
                x.code = Some(1);
                x.access_ok = false;
            }),
            (S::Failed, Some(R::AccessOff))
        );
        assert_eq!(f(&|x| x.code = Some(1)), (S::Failed, Some(R::Exit)));
        assert_eq!(
            f(&|x| x.code = None),
            (S::Failed, Some(R::Exit)),
            "убит сигналом извне"
        );
        assert_eq!(
            f(&|x| {
                x.code = Some(1);
                x.proposals = 1;
            }),
            (S::Failed, Some(R::Exit)),
            "запрос всё равно получил ответ"
        );
        assert_eq!(
            f(&|x| x.calls = MAX_RUN_CALLS),
            (S::Done, None),
            "ровно 40 вызовов — не предел"
        );
        assert_eq!(
            f(&|x| x.calls = MAX_RUN_CALLS + 1),
            (S::Failed, Some(R::Limit))
        );
        assert_eq!(
            f(&|x| {
                x.cli_limit = true;
                x.code = Some(55);
            }),
            (S::Failed, Some(R::Limit))
        );
        assert_eq!(
            f(&|x| {
                x.cli_limit = true;
                x.code = Some(55);
                x.proposals = 1;
            }),
            (S::Done, None)
        );
        assert_eq!(
            f(&|x| {
                x.kind = RunKind::Check;
                x.calls = MAX_RUN_CALLS + 1;
            }),
            (S::Done, None),
            "у проверки предложений не бывает"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn mcp_off_and_access_off_fail_the_run_before_start() {
        let h = harness();
        h.st.mcp.force_server(McpServerState::Stopped);
        let a = h.q().start(&request(&h)).unwrap();
        let a = until(&h, &a.id, done).await;
        assert_eq!(
            (a.status, a.reason, a.started_at_ms),
            (RunStatus::Failed, Some(FailReason::McpOff), None)
        );
        h.st.mcp.force_server(McpServerState::Running);
        let req = request(&h);
        let mut p = h.st.connections.get("c1").unwrap();
        p.llm_access = LlmAccess::Off;
        h.st.connections.upsert(p).unwrap();
        let b = h.q().start(&req).unwrap();
        let b = until(&h, &b.id, done).await;
        assert_eq!(
            (b.status, b.reason),
            (RunStatus::Failed, Some(FailReason::AccessOff))
        );
        assert!(h.fake.spawned().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn progress_events_are_throttled_and_the_journal_has_both_records() {
        let h = harness();
        h.fake.script(vec![
            Step::Connect,
            Step::Call("get_optimization_request"),
            Step::Call("explain_query"),
            Step::Call("explain_query"),
            Step::Call("hypothetical_index"),
            Step::Sleep(5),
            Step::Out(RESULT),
            Step::Exit(0),
        ]);
        let run = h.q().start(&request(&h)).unwrap();
        let run = until(&h, &run.id, done).await;
        let events: Vec<AgentRun> = h
            .events
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.id == run.id)
            .cloned()
            .collect();
        let statuses: Vec<RunStatus> = events.iter().map(|e| e.status).collect();
        assert_eq!(statuses.first(), Some(&RunStatus::Queued));
        assert_eq!(statuses.last(), Some(&RunStatus::Done));
        let running: Vec<&AgentRun> = events
            .iter()
            .filter(|e| e.status == RunStatus::Running)
            .collect();
        assert_eq!(
            running.len(),
            2,
            "первый вызов — сразу, остальные — одним событием через секунду: {statuses:?}"
        );
        assert_eq!((running[0].calls, running[1].calls), (1, 4));
        assert_eq!(running[1].last_tool.as_deref(), Some("hypothetical_index"));
        let journal = h.st.mcp.audit_tail(20);
        let started = journal
            .iter()
            .find(|e| e["event"] == "agentRunStarted")
            .expect("agentRunStarted");
        assert_eq!(started["runId"], run.id.as_str());
        assert_eq!(started["preset"], "qwen");
        assert_eq!(started["model"], MODEL);
        assert_eq!(started["connection"]["id"], "c1");
        let finished = journal
            .iter()
            .find(|e| e["event"] == "agentRunFinished")
            .expect("agentRunFinished");
        assert_eq!(
            (
                finished["status"].as_str(),
                finished["calls"].as_u64(),
                finished["proposals"].as_u64(),
                finished["exitCode"].as_i64()
            ),
            (Some("done"), Some(4), Some(0), Some(0))
        );
        assert!(finished.get("reason").is_none() && finished["ms"].is_u64());
        assert!(
            finished.get("costUsd").is_none(),
            "у Qwen Code стоимости нет"
        );
        let line = serde_json::to_string(finished).unwrap();
        assert!(
            !line.contains("No better plan"),
            "итоговый текст в журнал не пишется"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn check_runs_without_a_request_and_with_its_own_prompt() {
        let h = harness_with(Arc::new(NoWarmth), |a| a.enabled = false);
        h.fake.script(vec![
            Step::Connect,
            Step::Call("list_connections"),
            Step::Out("OK"),
            Step::Exit(0),
        ]);
        let run = h.q().check().unwrap();
        assert_eq!(
            (
                run.kind,
                run.request_id.as_deref(),
                run.connection_id.as_deref()
            ),
            (RunKind::Check, None, None)
        );
        assert_eq!(h.q().check().unwrap().id, run.id, "вторая проверка — та же");
        let run = until(&h, &run.id, done).await;
        assert_eq!(
            (run.status, run.summary.as_deref()),
            (RunStatus::Done, Some("OK"))
        );
        let s = &h.fake.spawned()[0];
        assert_eq!(s.stdin, prompt::check_prompt());
        assert_eq!(
            s.scope,
            Some((None, None)),
            "билет проверки — без подключения и запроса"
        );
        assert_eq!(
            h.q()
                .list(&RunFilter::default())
                .iter()
                .filter(|r| r.kind == RunKind::Check)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn policy_forbids_runs_and_checks() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = Policy::default();
        policy.status.mcp_agent_enabled = false;
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        st.settings
            .set_agent(AgentSettings {
                enabled: true,
                qwen: QwenSettings {
                    path: "/opt/agents/qwen".into(),
                    model: MODEL.into(),
                },
                ..AgentSettings::default()
            })
            .unwrap();
        let req = st.llm.requests.create(requests::tests::new("c1")).unwrap();
        assert_eq!(
            st.agent.start(&req.id).unwrap_err().key,
            "errors.agent.disabledByPolicy"
        );
        assert_eq!(
            st.agent.check().unwrap_err().key,
            "errors.agent.disabledByPolicy"
        );
        assert!(!st.agent.status().policy_enabled);
        assert!(!st.mcp.status().agent.policy_enabled);
    }

    #[tokio::test]
    async fn start_checks_the_settings_and_the_request() {
        let h = harness_with(Arc::new(NoWarmth), |a| a.qwen.path.clear());
        let req = request(&h);
        assert_eq!(
            h.q().start(&req).unwrap_err().key,
            "errors.agent.notConfigured"
        );
        assert_eq!(h.q().check().unwrap_err().key, "errors.agent.notConfigured");
        let mut a = h.st.settings.agent();
        a.qwen.path = "/opt/agents/qwen".into();
        h.st.settings.set_agent(a).unwrap();
        assert_eq!(
            h.q().start("r000000").unwrap_err().key,
            "errors.llm.requestNotFound"
        );
        h.st.llm.requests.cancel(&req).unwrap();
        assert_eq!(
            h.q().start(&req).unwrap_err().key,
            "errors.agent.requestNotWaiting"
        );
        assert!(h.fake.spawned().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_run_gets_its_prompt_config_and_working_folder() {
        let h = harness();
        let req = request(&h);
        let run = h.q().start(&req).unwrap();
        let run = until(&h, &run.id, done).await;
        let s = &h.fake.spawned()[0];
        assert_eq!(s.stdin, prompt::run_prompt(&req, "en"));
        assert_eq!(s.spec.program, PathBuf::from("/opt/agents/qwen"));
        assert_eq!(s.spec.cwd, h.dir.path().join(runs::WORK_DIR));
        assert_eq!(
            s.config,
            runs::mcp_config_path(&h.dir.path().join(runs::RUNS_DIR), &run.id)
        );
        assert_eq!(s.command, "/opt/glafira/glafira-pg", "сервер конфига — шим");
        assert_eq!(s.scope, Some((Some("c1".to_string()), Some(req.clone()))));
        assert!(s.spec.args.windows(2).any(|w| w == ["-m", MODEL]));
        assert!(
            !s.spec.args.iter().any(|a| a.contains(&req)),
            "id запроса — только в промпте"
        );
        assert_eq!(run.path.as_deref(), Some("/opt/agents/qwen"));
        assert!(
            runs::log_path(&h.dir.path().join(runs::RUNS_DIR), &run.id)
                .ends_with(format!("{}.log", run.id))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(h.dir.path().join(runs::WORK_DIR))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_model_is_warmed_before_and_kept_after_the_run() {
        let warmth = Arc::new(FakeWarmth {
            warnings: vec![format!("evict qwen3.8:27b {MODEL}")],
            ..FakeWarmth::default()
        });
        let h = harness_with(warmth.clone(), |_| {});
        let run = h.q().start(&request(&h)).unwrap();
        let run = until(&h, &run.id, done).await;
        assert_eq!(run.warnings, warmth.warnings);
        assert_eq!(*warmth.before.lock().unwrap(), [Some(MODEL.to_string())]);
        for _ in 0..100 {
            if !warmth.after.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(*warmth.after.lock().unwrap(), [Some(MODEL.to_string())]);
    }
}
