//! Фоновый ассистент целиком, без настоящих CLI и модели (спец. P4c §12 «Интеграция»): очередь Glafira запускает фальшивый
//! агент `glafira-fake-agent` (`tests/bin/fake_agent.rs`) настоящим `TokioSpawner` — своя группа процессов на Unix, Job
//! Object на Windows; агент читает промпт из stdin и запускает из MCP-конфига запуска настоящий шим `glafira-pg mcp` с
//! `env` конфига (билет); шим проходит рукопожатие с билетом на настоящем сокете (named pipe) MCP-сервера приложения в
//! каталоге данных теста. Агент печатает поток JSON, как CLI пресетов (`stream-json`): первым — событие `init` с
//! инструментами от шлюза, последним — `result`; очередь проверяет инструменты по `init` (спец. §4.1) и берёт итог из
//! `result`. Бэкенд — настоящий `AppBackend` (подтверждение билетом, область запуска, видимость инструментов, предел
//! вызовов, чтение, запросы на оптимизацию), кроме проверки предложения на сервере PostgreSQL: её заменяет обёртка
//! `NoServerCheck` — стенда в CI на трёх ОС нет. Предложение с настоящей проверкой и «Сравнить планы» —
//! `propose_on_the_perf_stand` (`GPG_IT_PERF`, workflow `perf`).
//!
//! Режим агента и каталог данных шима задаёт `TestSpawner` поверх `TokioSpawner` (`FAKE_AGENT_MODE`,
//! `GLAFIRA_TEST_DATA_DIR` в окружении процесса). Окружение агента — окружение процесса теста, а не логин-оболочки
//! разработчика: профиль оболочки тест не запускает. Что агент сделал, видно по строкам `FAKE_AGENT ключ=значение`: агент
//! пишет их в stderr (журнал запуска `agent-runs/<id>.log`) и копией в `fake-notes/<id>.txt` каталога данных — pid агента
//! и шима нужны тесту во время запуска. Запуск: `cargo test --manifest-path src-tauri/Cargo.toml --features fake-agent
//! --test it_agent`.

mod common;
mod mcp_harness;

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_pg_lib::agent::presets::CommandSpec;
use glafira_pg_lib::agent::process::{AgentProcess, AgentSpawner, TokioSpawner};
use glafira_pg_lib::agent::queue::{AgentRuntime, RunFilter};
use glafira_pg_lib::agent::runs::{
    AgentRun, FailReason, MAX_LOG_BYTES, MAX_SUMMARY_BYTES, RunKind, RunStatus,
};
use glafira_pg_lib::agent::settings::{AgentSettings, ClaudeSettings, Preset, QwenSettings};
use glafira_pg_lib::agent::shell_env::ShellEnv;
use glafira_pg_lib::agent::warmth::NoWarmth;
use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::mcp::backend::AppBackend;
use glafira_pg_lib::mcp::controller::McpServerState;
use glafira_pg_lib::mcp::llm::proposals::{NewProposal, ProposalSource};
use glafira_pg_lib::mcp::llm::requests::{
    NewRequest, OptimizationRequest, RequestSource, RequestStatus,
};
use glafira_pg_lib::mcp::llm::tools::store_proposal_unchecked;
use glafira_pg_lib::mcp::llm::{
    CreateRequestArgs, cancel_request, compare_proposal, create_request,
};
use glafira_pg_lib::mcp::shim::TEST_DATA_DIR_ENV;
use glafira_pg_lib::secrets::MemoryStore;
use glafira_pg_lib::state::AppState;
use glafira_pg_mcp::model::{
    LlmOp, ObserveCtx, PgAnswer, PgBackend, PgObserve, PgOutput, PgRead, SelectOp,
};
use glafira_pg_mcp::server::RUN_TOOL_NAMES;
use mcp_harness::{audit_events, member, set_access};
use serde_json::{Value, json};

const FAKE_AGENT: &str = env!("CARGO_BIN_EXE_glafira-fake-agent");
const SHIM: &str = env!("CARGO_BIN_EXE_glafira-pg");
/// Предел ожидания исхода запуска: отладочные сборки на раннерах Windows запускаются медленно.
const WAIT: Duration = Duration::from_secs(120);
/// Итоги и стоимость фальшивого агента (те же строки — в `tests/bin/fake_agent.rs`).
const PROPOSED: &str = "Fake agent summary: one rewrite proposed.";
const NO_PROPOSALS: &str = "Fake agent summary: no proposals, the plan already uses an index.";
const COST: f64 = 0.0042;
/// Запрос и индекс для pg-perf (как в `it_mcp_proposals`).
const PERF_SQL: &str = "SELECT count(*) FROM shop.orders WHERE status = 'disputed'";
const PERF_INDEX: &str = "CREATE INDEX ON shop.orders (status)";

/// `TokioSpawner` с окружением теста: каталог данных шима и режим фальшивого агента с параметрами — по очереди на каждый
/// запуск (последний повторяется). Запоминает командные строки запусков.
struct TestSpawner {
    inner: TokioSpawner,
    data_dir: PathBuf,
    plans: Mutex<VecDeque<Vec<(String, String)>>>,
    last: Mutex<Vec<(String, String)>>,
    specs: Mutex<Vec<CommandSpec>>,
}

impl TestSpawner {
    fn new(data_dir: &Path) -> Arc<Self> {
        Arc::new(Self {
            // Окружение процесса теста вместо логин-оболочки разработчика (её профиль тесту не нужен).
            inner: TokioSpawner::with_env(Arc::new(ShellEnv::current_process())),
            data_dir: data_dir.to_path_buf(),
            plans: Mutex::new(VecDeque::new()),
            last: Mutex::new(vec![("FAKE_AGENT_MODE".into(), "none".into())]),
            specs: Mutex::new(Vec::new()),
        })
    }

    /// Режим следующего запуска и его параметры (`FAKE_AGENT_*`).
    fn push(&self, mode: &str, extra: &[(&str, &str)]) {
        let mut plan = vec![("FAKE_AGENT_MODE".to_string(), mode.to_string())];
        plan.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        self.plans.lock().unwrap().push_back(plan);
    }

    fn spawned(&self) -> usize {
        self.specs.lock().unwrap().len()
    }

    fn last_spec(&self) -> CommandSpec {
        self.specs
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("запуска не было")
    }
}

impl AgentSpawner for TestSpawner {
    fn spawn(
        &self,
        spec: &CommandSpec,
        stdin: String,
        log: PathBuf,
    ) -> io::Result<Box<dyn AgentProcess>> {
        let next = self.plans.lock().unwrap().pop_front();
        let plan = match next {
            Some(p) => {
                *self.last.lock().unwrap() = p.clone();
                p
            }
            None => self.last.lock().unwrap().clone(),
        };
        let mut spec = spec.clone();
        spec.env_set.push((
            TEST_DATA_DIR_ENV.to_string(),
            self.data_dir.display().to_string(),
        ));
        spec.env_set.extend(plan);
        // Копия строк `FAKE_AGENT …` — в свой файл на запуск: pid агента и шима нужны тесту во время запуска.
        let notes = self.data_dir.join("fake-notes");
        std::fs::create_dir_all(&notes)?;
        let run = log
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        spec.env_set.push((
            "FAKE_AGENT_NOTES".to_string(),
            notes.join(format!("{run}.txt")).display().to_string(),
        ));
        self.specs.lock().unwrap().push(spec.clone());
        self.inner.spawn(&spec, stdin, log)
    }
}

/// `AppBackend` без проверки предложения на сервере PostgreSQL. Всё остальное — настоящее: `Gate` (подтверждение билетом,
/// видимость инструментов, подключение запуска, предел вызовов), чтение и запросы на оптимизацию. Предложение на запрос
/// своего запуска сохраняется тем же `store_proposal`, что после проверки (`store_proposal_unchecked`) (привязка к запросу, событие, `runId`); прямая
/// форма, чужой запрос и обычная сессия идут в приложение и получают его ответ.
struct NoServerCheck {
    app: AppBackend,
    st: AppState,
}

impl NoServerCheck {
    /// `request_id` — запрос запуска `run`.
    fn own_request(&self, run: Option<&str>, request_id: Option<&str>) -> bool {
        let own = run
            .and_then(|r| self.st.tickets.scope(r))
            .and_then(|s| s.request_id);
        own.is_some() && own.as_deref() == request_id.map(str::trim)
    }
}

impl Gate for NoServerCheck {
    fn access(&self, s: &SessionView) -> SessionAccess {
        self.app.access(s)
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        self.app.connection_name(id)
    }

    fn session_closed(&self, session_id: &str) {
        self.app.session_closed(session_id)
    }

    fn sessions_changed(&self) {
        self.app.sessions_changed()
    }

    fn run_for_ticket(&self, ticket: &str) -> Option<String> {
        self.app.run_for_ticket(ticket)
    }

    fn check_call(
        &self,
        session: &SessionView,
        tool: &str,
        connection_id: Option<&str>,
    ) -> Result<(), ToolError> {
        self.app.check_call(session, tool, connection_id)
    }

    fn tool_visible(&self, session: &SessionView, tool: &str) -> bool {
        self.app.tool_visible(session, tool)
    }

    fn prompts_visible(&self, session: &SessionView) -> bool {
        self.app.prompts_visible(session)
    }
}

impl PgBackend for NoServerCheck {
    fn read(&self, op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.app.read(op)
    }

    fn observe(
        &self,
        op: PgObserve,
        ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.app.observe(op, ctx)
    }

    fn select(&self, op: SelectOp, ctx: ObserveCtx) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        self.app.select(op, ctx)
    }

    fn llm(&self, op: LlmOp) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        match op {
            LlmOp::Propose {
                params,
                client,
                run,
            } if self.own_request(run.as_deref(), params.request_id.as_deref()) => {
                Box::pin(async move {
                    let id = params
                        .request_id
                        .as_deref()
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    let r = self
                        .st
                        .llm
                        .requests
                        .get(&id)
                        .ok_or_else(|| ToolError::not_found(format!("request {id}")))?;
                    let new = NewProposal {
                        source: ProposalSource::Mcp,
                        client,
                        request_id: Some(r.id.clone()),
                        connection_id: r.connection_id.clone(),
                        database: r.database.clone(),
                        member: Some(r.member.clone()),
                        original_sql: r.sql.clone(),
                        original_query_id: None,
                        rewritten_sql: params.rewritten_sql,
                        indexes: params.indexes.unwrap_or_default(),
                        rationale: params.rationale,
                        expected_effect: params.expected_effect,
                        run_id: run,
                    };
                    let out = store_proposal_unchecked(&self.st, new).await?;
                    Ok(PgAnswer::new(PgOutput::Proposed(out)))
                })
            }
            op => self.app.llm(op),
        }
    }
}

/// Каталог данных, приложение с MCP-сервером, тестовый спавнер. Упавший тест (`assert`) при сбросе снимает процессы своих
/// запусков, пережившие его: зависший агент не должен остаться на машине разработчика.
struct Env {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    st: AppState,
    spawner: Arc<TestSpawner>,
}

impl Env {
    async fn finish(self) {
        self.st.agent.shutdown().await;
        self.st.mcp.shutdown().await;
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        // Тест прошёл — его процессы уже закончились (`finish`, `wait_gone`), а их номера могли достаться другим
        // процессам: снимать нечего.
        if !std::thread::panicking() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(self.dir.join("fake-notes")) else {
            return;
        };
        for e in entries.flatten() {
            let notes = std::fs::read_to_string(e.path()).unwrap_or_default();
            for key in ["pid", "shim_pid", "shim2_pid"] {
                // Только процесс этого теста: программа — ровно фальшивый агент или шим этой сборки. Не подстрока «glafira»:
                // под неё попали бы установленная Glafira владельца и бинари других worktree.
                if let Some(pid) = noted(&notes, key).and_then(|p| p.parse::<u32>().ok())
                    && program_of(pid).is_some_and(|p| ours(&p))
                {
                    force_kill(pid);
                }
            }
        }
    }
}

/// Программа — фальшивый агент или шим этой сборки теста: путь целиком, после разрешения ссылок (так его отдаёт ОС); на
/// Windows — без учёта регистра.
fn ours(program: &Path) -> bool {
    [FAKE_AGENT, SHIM].iter().any(|ours| {
        let ours = std::fs::canonicalize(ours).unwrap_or_else(|_| PathBuf::from(ours));
        if cfg!(windows) {
            program
                .to_string_lossy()
                .eq_ignore_ascii_case(&ours.to_string_lossy())
        } else {
            program == ours
        }
    })
}

/// Путь программы живого процесса `pid`: на macOS — `proc_pidpath` (у `ps -o comm=` — `argv[0]`, его задаёт сам процесс).
#[cfg(target_os = "macos")]
fn program_of(pid: u32) -> Option<PathBuf> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: буфер размера `PROC_PIDPATHINFO_MAXSIZE` — столько `proc_pidpath` и пишет; ответ — длина пути.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let n = usize::try_from(n).ok().filter(|n| *n > 0)?;
    buf.truncate(n);
    Some(PathBuf::from(String::from_utf8(buf).ok()?))
}

/// Путь программы живого процесса `pid`: на Linux — `/proc/<pid>/exe`.
#[cfg(all(unix, not(target_os = "macos")))]
fn program_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// Путь программы живого процесса `pid`: на Windows — `Get-Process` (у `tasklist` — только имя образа).
#[cfg(windows)]
fn program_of(pid: u32) -> Option<PathBuf> {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).Path"),
        ])
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// Каталог данных теста: на Unix — короткий путь в `/tmp` (в нём `mcp.sock`, предел пути сокета — 100 байт), на Windows —
/// обычный временный каталог (сервер слушает named pipe).
fn data_dir() -> tempfile::TempDir {
    if cfg!(unix) {
        tempfile::Builder::new()
            .prefix("gpa")
            .tempdir_in("/tmp")
            .unwrap()
    } else {
        tempfile::tempdir().unwrap()
    }
}

/// Фоновый ассистент включён, программа обоих пресетов — фальшивый агент, Claude Code — без Ollama с пределом расходов.
fn agent_settings(preset: Preset) -> AgentSettings {
    AgentSettings {
        enabled: true,
        preset,
        qwen: QwenSettings {
            path: FAKE_AGENT.into(),
            model: "fake-model".into(),
        },
        claude: ClaudeSettings {
            path: FAKE_AGENT.into(),
            model: "fake-model".into(),
            via_ollama: false,
            budget_usd: Some(1.0),
        },
        // Тёплой моделью тест не управляет: `NoWarmth`, и настройка тоже не зовёт Ollama.
        keep_warm_min: 0,
        ..AgentSettings::default()
    }
}

fn runtime(spawner: &Arc<TestSpawner>, run_timeout: Option<Duration>) -> AgentRuntime {
    AgentRuntime {
        spawner: spawner.clone(),
        warmth: Arc::new(NoWarmth),
        shim_exe: PathBuf::from(SHIM),
        run_timeout,
    }
}

/// Приложение с подключениями `it` и `other` (уровень «Диагностика», соединения не открываются), MCP-сервером на обёртке
/// `NoServerCheck` и фоновым ассистентом на фальшивом агенте.
async fn env(preset: Preset, run_timeout: Option<Duration>) -> Env {
    let tmp = data_dir();
    let dir = tmp.path().to_path_buf();
    let spawner = TestSpawner::new(&dir);
    let st = AppState::new(&dir, Arc::new(MemoryStore::default()))
        .unwrap()
        .with_agent(runtime(&spawner, run_timeout));
    for id in ["it", "other"] {
        // Порт 9: соединение не открывается — тест не трогает базу.
        let mut p = common::server_profile(id, "127.0.0.1:9");
        p.llm_access = LlmAccess::Diagnostics;
        st.connections.upsert(p).unwrap();
    }
    st.settings.set_mcp_enabled(true).unwrap();
    st.settings.set_agent(agent_settings(preset)).unwrap();
    let wrapped = st.clone();
    let status = st
        .mcp
        .start_wrapped(&st, move |app| NoServerCheck { app, st: wrapped })
        .await;
    assert_eq!(status.server, McpServerState::Running, "{:?}", status.error);
    Env {
        _tmp: tmp,
        dir,
        st,
        spawner,
    }
}

/// Ожидающий запрос «из консоли» без стенда: снимок задаёт тест (`create_request` строит его по живому подключению).
fn new_request(st: &AppState, connection_id: &str) -> OptimizationRequest {
    st.llm
        .requests
        .create(NewRequest {
            connection_id: connection_id.into(),
            member: format!("{connection_id}-1"),
            database: "postgres".into(),
            source: RequestSource::Console,
            sql: "SELECT o.id FROM shop.orders o WHERE lower(o.status) = 'new'".into(),
            params: vec![],
            bundle: json!({ "planText": "Seq Scan on orders o  (cost=0.00..431.00 rows=50 width=4)", "tables": [] }),
        })
        .unwrap()
}

fn request_status(st: &AppState, id: &str) -> RequestStatus {
    st.llm.requests.get(id).unwrap().status
}

fn run_of(st: &AppState, id: &str) -> AgentRun {
    st.agent
        .list(&RunFilter {
            connection_id: None,
            request_ids: None,
        })
        .into_iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("запуска {id} нет"))
}

/// Ждёт, пока запуск `id` не станет таким, как нужно (`what` — для сообщения о сбое).
async fn wait_run(
    st: &AppState,
    id: &str,
    what: &str,
    done: impl Fn(&AgentRun) -> bool,
) -> AgentRun {
    let deadline = Instant::now() + WAIT;
    loop {
        let r = run_of(st, id);
        if done(&r) {
            return r;
        }
        assert!(Instant::now() < deadline, "{what}: {r:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Исход запуска.
async fn wait_end(st: &AppState, id: &str) -> AgentRun {
    wait_run(st, id, "исход запуска", |r| {
        !r.status.is_active()
    })
    .await
}

fn log_path(dir: &Path, run: &str) -> PathBuf {
    dir.join("agent-runs").join(format!("{run}.log"))
}

fn read_log(dir: &Path, run: &str) -> String {
    std::fs::read(log_path(dir, run))
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}

/// Строки `FAKE_AGENT …` запуска — копия из `FAKE_AGENT_NOTES`.
fn read_notes(dir: &Path, run: &str) -> String {
    std::fs::read_to_string(dir.join("fake-notes").join(format!("{run}.txt"))).unwrap_or_default()
}

/// Значение `ключ=…` из строк `FAKE_AGENT …` (первое).
fn noted(notes: &str, key: &str) -> Option<String> {
    notes
        .lines()
        .filter(|l| l.contains("FAKE_AGENT "))
        .flat_map(str::split_whitespace)
        .find_map(|t| t.strip_prefix(key)?.strip_prefix('=').map(str::to_string))
}

/// Ждёт строку `ключ=…` агента.
async fn wait_noted(dir: &Path, run: &str, key: &str) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(v) = noted(&read_notes(dir, run), key) {
            return v;
        }
        assert!(
            Instant::now() < deadline,
            "{key} нет у агента запуска {run}:\n{}\nжурнал:\n{}",
            read_notes(dir, run),
            read_log(dir, run)
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Процессы запуска: агент и запущенный им шим.
async fn run_pids(dir: &Path, run: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    for key in ["pid", "shim_pid"] {
        pids.push(wait_noted(dir, run, key).await.parse().expect("pid"));
    }
    pids
}

/// Живой процесс `pid`: строка `ps` (состояние и программа) или `tasklist` (программа первой). Зомби — не жив: его уже
/// нет, осталось забрать код выхода.
#[cfg(unix)]
fn process_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-o", "comm=", "-p", &pid.to_string()])
        .output()
        .expect("ps");
    let line = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!line.is_empty() && !line.starts_with('Z')).then_some(line)
}

#[cfg(windows)]
fn process_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|l| l.contains(&format!("\"{pid}\"")))
        .map(str::to_string)
}

fn alive(pid: u32) -> bool {
    process_line(pid).is_some()
}

fn force_kill(pid: u32) {
    #[cfg(unix)]
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status();
}

/// Ждёт, пока процессы не закончатся (не дольше 20 с).
async fn wait_gone(pids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while pids.iter().any(|&p| alive(p)) {
        assert!(Instant::now() < deadline, "процессы запуска живы: {pids:?}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 64 hex — так выглядит билет запуска.
fn is_ticket(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(unix)]
fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Записи журнала MCP о вызовах запуска `run`.
fn run_calls(st: &AppState, run: &str) -> Vec<Value> {
    audit_events(st, "call")
        .into_iter()
        .filter(|x| x["run"] == run)
        .collect()
}

/// Запись `agentRunFinished` запуска `run`. Очередь пишет её сразу после итога в `agent-runs.json`, и тест может увидеть
/// итог раньше записи журнала: ждём её (не дольше 10 с).
async fn finished_record(st: &AppState, run: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(f) = audit_events(st, "agentRunFinished")
            .into_iter()
            .find(|x| x["runId"] == run)
        {
            return f;
        }
        assert!(
            Instant::now() < deadline,
            "нет agentRunFinished запуска {run}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Полный путь (спец. §3, §4.1, §5, §7, §9): очередь → `TokioSpawner` → агент → шим с билетом из `env` конфига → сессия
/// запуска без диалога → девять инструментов и событие `init` только с ними → предложение с `runId` → итог из события
/// `result` → `done`. Билета нет ни в аргументах, ни в окружении процесса агента; MCP-конфиг удалён, рабочая папка пуста;
/// журнал MCP — с `run`, без промпта и итога агента.
#[tokio::test]
async fn propose_run_stores_a_proposal_of_the_run() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("propose", &[]);
    let queued = e.st.agent.start(&req.id).unwrap();
    assert_eq!(queued.kind, RunKind::Optimize);
    assert_eq!(queued.status, RunStatus::Queued);
    let run = wait_end(&e.st, &queued.id).await;
    let notes = read_notes(&e.dir, &run.id);
    assert_eq!(run.status, RunStatus::Done, "{run:?}\n{notes}");
    assert_eq!(run.request_id.as_deref(), Some(req.id.as_str()));
    assert_eq!(run.connection_id.as_deref(), Some("it"));
    assert_eq!(run.preset, Preset::Claude);
    assert_eq!(run.model.as_deref(), Some("fake-model"));
    assert_eq!(run.path.as_deref(), Some(FAKE_AGENT));
    assert!(
        run.started_at_ms.is_some() && run.ended_at_ms >= run.started_at_ms,
        "{run:?}"
    );
    assert!(run.calls >= 2, "{run:?}");
    assert_eq!(run.last_tool.as_deref(), Some("propose_query_rewrite"));
    assert_eq!(run.summary.as_deref(), Some(PROPOSED));
    assert_eq!(run.cost_usd, Some(COST));
    assert_eq!(run.exit_code, Some(0));
    assert!(run.warnings.is_empty(), "{run:?}");

    // Предложение запуска: в хранилище с runId, запрос отвечен.
    assert_eq!(run.proposal_ids.len(), 1, "{run:?}");
    let p =
        e.st.llm
            .proposals
            .get(&run.proposal_ids[0])
            .expect("предложение");
    assert_eq!(p.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(p.request_id.as_deref(), Some(req.id.as_str()));
    assert_eq!(p.client.as_deref(), Some("glafira-fake-agent"));
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Answered);
    assert_eq!(
        noted(&notes, "proposal").as_deref(),
        Some(p.id.as_str()),
        "ответ propose_query_rewrite — JSON в тексте: {notes}"
    );

    // Агент: промпт из stdin с id запроса, конфиг с билетом, девять инструментов, пустая рабочая папка.
    assert_eq!(
        noted(&notes, "prompt_request").as_deref(),
        Some(req.id.as_str()),
        "{notes}"
    );
    assert_eq!(
        noted(&notes, "request").as_deref(),
        Some(req.id.as_str()),
        "без requestId — запрос своего запуска: {notes}"
    );
    assert_eq!(
        noted(&notes, "ticket_len").as_deref(),
        Some("64"),
        "{notes}"
    );
    let mut names = RUN_TOOL_NAMES.to_vec();
    names.sort_unstable();
    assert_eq!(noted(&notes, "tools").as_deref(), Some("9"), "{notes}");
    assert_eq!(noted(&notes, "names"), Some(names.join(",")), "{notes}");
    assert!(
        noted(&notes, "cwd").is_some_and(|c| c.ends_with("agent-work")),
        "{notes}"
    );
    let spec = e.spawner.last_spec();
    assert!(
        spec.args.iter().any(|a| a == "--mcp-config"),
        "{:?}",
        spec.args
    );
    assert!(
        spec.args
            .windows(2)
            .any(|w| w[0] == "--output-format" && w[1] == "stream-json"),
        "{:?}",
        spec.args
    );
    assert!(
        spec.args.iter().all(|a| !is_ticket(a)),
        "билет — не в аргументах: {:?}",
        spec.args
    );
    assert!(
        spec.env_set
            .iter()
            .all(|(k, v)| k != "GLAFIRA_AGENT_TICKET" && !is_ticket(v)),
        "билет — только в MCP-конфиге"
    );
    assert!(spec.cwd.ends_with("agent-work"), "{:?}", spec.cwd);

    // Поток JSON агента — в журнале запуска: `init` первым событием stdout, с инструментами Glafira (спец. §4.1, §4.5).
    let log = read_log(&e.dir, &run.id);
    let first_out = log
        .lines()
        .find_map(|l| l.split_once(" out] ").map(|(_, rest)| rest))
        .expect("stdout агента в журнале");
    let init: Value = serde_json::from_str(first_out).expect("первая строка stdout — JSON");
    assert_eq!(
        (init["type"].as_str(), init["subtype"].as_str()),
        (Some("system"), Some("init")),
        "{init}"
    );
    let mut init_tools: Vec<&str> = init["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    init_tools.sort_unstable();
    let expected: Vec<String> = names
        .iter()
        .map(|n| format!("mcp__glafira-pg__{n}"))
        .collect();
    assert_eq!(init_tools, expected, "{init}");
    assert!(log.contains("\"type\":\"result\""), "{log}");

    // Файлы запуска (спец. §3.3, §4.5, §9.3).
    assert!(
        !e.dir
            .join("agent-runs")
            .join(format!("{}.mcp.json", run.id))
            .exists(),
        "MCP-конфиг удалён в конце запуска"
    );
    assert_eq!(
        std::fs::read_dir(e.dir.join("agent-work")).unwrap().count(),
        0,
        "рабочая папка пуста"
    );
    #[cfg(unix)]
    {
        assert_eq!(mode(&log_path(&e.dir, &run.id)), 0o600);
        assert_eq!(mode(&e.dir.join("agent-runs")), 0o700);
        assert_eq!(mode(&e.dir.join("agent-work")), 0o700);
    }

    // Подтверждение билетом: без диалога, клиент не запоминается (спец. §5.1); журнал MCP (§9.2).
    assert!(
        e.st.mcp.status().pending.is_empty(),
        "диалога подтверждения нет"
    );
    assert!(
        e.st.settings.mcp().approved_clients.is_empty(),
        "клиент фонового запуска не записывается"
    );
    let approved = audit_events(&e.st, "approved");
    assert!(
        approved
            .iter()
            .any(|x| x["run"] == run.id.as_str() && x["by"] == "run"),
        "{approved:?}"
    );
    let sessions = audit_events(&e.st, "sessionStarted");
    assert!(
        sessions.iter().any(|x| x["run"] == run.id.as_str()),
        "{sessions:?}"
    );
    let calls = run_calls(&e.st, &run.id);
    assert!(
        calls.iter().any(|x| x["tool"] == "propose_query_rewrite"),
        "{calls:?}"
    );
    let started = audit_events(&e.st, "agentRunStarted");
    assert!(
        started
            .iter()
            .any(|x| x["runId"] == run.id.as_str() && x["requestId"] == req.id.as_str()),
        "{started:?}"
    );
    let f = finished_record(&e.st, &run.id).await;
    assert_eq!(f["status"], "done", "{f}");
    let journal: String =
        e.st.mcp
            .audit_tail(1000)
            .iter()
            .map(Value::to_string)
            .collect();
    assert!(
        !journal.contains(PROPOSED),
        "итог агента в журнал MCP не пишется"
    );
    assert!(
        !journal.contains("Send to LLM assistant")
            && !journal.contains("started you in the background"),
        "промпт в журнал MCP не пишется"
    );
    e.finish().await;
}

/// Агент закончил без предложений (спец. §7): запуск `done` с пустым `proposalIds` и итогом, запрос ждёт дальше.
#[tokio::test]
async fn run_without_proposals_is_done_and_the_request_keeps_waiting() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("none", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    assert_eq!(run.status, RunStatus::Done, "{run:?}");
    assert!(run.proposal_ids.is_empty(), "{run:?}");
    assert_eq!(run.summary.as_deref(), Some(NO_PROPOSALS));
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Waiting);
    e.finish().await;
}

/// Запуски по одному (спец. §3.2); «Остановить» (§7) снимает всю группу процессов — и агента, который не отвечает на
/// SIGTERM, и шим (§4.4); запрос остаётся ждать, MCP-конфиг удалён, очередь идёт дальше.
#[tokio::test]
async fn stop_kills_the_process_group_and_the_queue_moves_on() {
    let e = env(Preset::Claude, None).await;
    let first = new_request(&e.st, "it");
    let second = new_request(&e.st, "it");
    e.spawner.push("hang", &[]);
    e.spawner.push("none", &[]);
    let a = e.st.agent.start(&first.id).unwrap().id;
    let b = e.st.agent.start(&second.id).unwrap().id;
    wait_run(
        &e.st,
        &a,
        "первый запуск работает",
        |r| r.status == RunStatus::Running,
    )
    .await;
    assert_eq!(
        run_of(&e.st, &b).status,
        RunStatus::Queued,
        "запуски идут по одному"
    );
    let cfg = e.dir.join("agent-runs").join(format!("{a}.mcp.json"));
    assert!(cfg.exists(), "MCP-конфиг — на время запуска");
    #[cfg(unix)]
    assert_eq!(mode(&cfg), 0o600);
    let pids = run_pids(&e.dir, &a).await;
    // Агент перехватил SIGTERM (Unix) и ждёт.
    wait_noted(&e.dir, &a, "hanging").await;
    assert!(pids.iter().all(|&p| alive(p)), "{pids:?}");

    let stopped_at = Instant::now();
    e.st.agent.stop(&a).unwrap();
    wait_gone(&pids).await;
    let took = stopped_at.elapsed();
    assert!(took < Duration::from_secs(20), "{took:?}");
    // Агент `hang` перехватывает SIGTERM: группу добивает SIGKILL через 5 с. На Windows Job Object закрывается сразу.
    #[cfg(unix)]
    assert!(
        took >= Duration::from_secs(4),
        "SIGKILL — через 5 с после SIGTERM: {took:?}"
    );
    let a_run = wait_end(&e.st, &a).await;
    assert_eq!(a_run.status, RunStatus::Cancelled, "{a_run:?}");
    assert_eq!(
        request_status(&e.st, &first.id),
        RequestStatus::Waiting,
        "«Остановить» запрос не трогает"
    );
    assert!(!cfg.exists(), "MCP-конфиг удалён");
    let b_run = wait_end(&e.st, &b).await;
    assert_eq!(
        b_run.status,
        RunStatus::Done,
        "очередь пошла дальше: {b_run:?}"
    );
    e.finish().await;
}

/// Сколько приложение ждёт остановки фонового ассистента при выходе (`AGENT_EXIT_GRACE` в `lib.rs`).
const AGENT_EXIT_GRACE: Duration = Duration::from_secs(8);

/// Выход из Glafira при живом агенте (спец. §4.4, §17.4): `shutdown` очереди, как при выходе приложения (`shutdown_agent`
/// до `shutdown_mcp`), — процессов агента и шима нет (агент `hang` перехватывает SIGTERM: группу добивает SIGKILL; на
/// Windows закрывается Job Object), запуск и запуск в очереди — `interrupted`, запрос ждёт, MCP-конфиг удалён, билет
/// отозван, сессий запуска нет.
#[tokio::test]
async fn shutdown_stops_the_running_agent() {
    let e = env(Preset::Qwen, None).await;
    let req = new_request(&e.st, "it");
    let next = new_request(&e.st, "it");
    e.spawner.push("hang", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let queued = e.st.agent.start(&next.id).unwrap().id;
    wait_run(&e.st, &id, "запуск работает", |r| {
        r.status == RunStatus::Running
    })
    .await;
    let pids = run_pids(&e.dir, &id).await;
    wait_noted(&e.dir, &id, "hanging").await;
    let cfg = e.dir.join("agent-runs").join(format!("{id}.mcp.json"));
    let config: Value = serde_json::from_slice(&std::fs::read(&cfg).unwrap()).unwrap();
    let ticket = config["mcpServers"]["glafira-pg"]["env"]["GLAFIRA_AGENT_TICKET"]
        .as_str()
        .expect("билет в MCP-конфиге")
        .to_string();
    assert_eq!(e.st.tickets.resolve(&ticket).as_deref(), Some(id.as_str()));
    assert!(
        e.st.mcp
            .status()
            .sessions
            .iter()
            .any(|s| s.run_id.as_deref() == Some(id.as_str())),
        "сессия запуска подключена"
    );
    assert!(pids.iter().all(|&p| alive(p)), "{pids:?}");

    let started = Instant::now();
    let stopped = tokio::time::timeout(AGENT_EXIT_GRACE, e.st.agent.shutdown()).await;
    let took = started.elapsed();
    e.st.mcp.shutdown().await;
    wait_gone(&pids).await;
    assert!(
        stopped.is_ok(),
        "очередь остановилась за {AGENT_EXIT_GRACE:?}, как ждёт приложение: {took:?}"
    );
    let run = run_of(&e.st, &id);
    assert_eq!(run.status, RunStatus::Interrupted, "{run:?}");
    assert_eq!(
        (run.reason, run.summary.as_deref()),
        (None, None),
        "{run:?}"
    );
    assert!(
        run.started_at_ms.is_some() && run.ended_at_ms.is_some(),
        "{run:?}"
    );
    let q = run_of(&e.st, &queued);
    assert_eq!(q.status, RunStatus::Interrupted, "{q:?}");
    assert_eq!(e.spawner.spawned(), 1, "запуск из очереди не стартовал");
    for r in [&req, &next] {
        assert_eq!(request_status(&e.st, &r.id), RequestStatus::Waiting);
    }
    assert!(!cfg.exists(), "MCP-конфиг удалён");
    assert_eq!(e.st.tickets.resolve(&ticket), None, "билет отозван");
    assert!(e.st.tickets.scope(&id).is_none());
    let f = finished_record(&e.st, &id).await;
    assert_eq!(f["status"].as_str(), Some("interrupted"), "{f}");
    common::wait_until(
        "сессий запуска после выхода нет",
        Duration::from_secs(10),
        async || e.st.mcp.status().sessions.is_empty(),
    )
    .await;
    e.finish().await;
}

/// Срок запуска (спец. §4.4): по сроку — остановка группы и `failed` / `timeout`; запрос ждёт дальше. Срок теста — 8 с
/// (`AgentRuntime.run_timeout`): минимум настройки — 2 минуты.
#[tokio::test]
async fn timeout_stops_the_run() {
    let e = env(Preset::Claude, Some(Duration::from_secs(8))).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("hang", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let pids = run_pids(&e.dir, &id).await;
    let run = wait_end(&e.st, &id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert_eq!(run.reason, Some(FailReason::Timeout), "{run:?}");
    assert!(
        run.ended_at_ms.unwrap() - run.started_at_ms.unwrap() >= 8000,
        "срок — от starting: {run:?}"
    );
    wait_gone(&pids).await;
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Waiting);
    e.finish().await;
}

/// «Отменить запрос» (спец. §7): в очереди — запуск снят без процесса; во время работы — группа остановлена; оба запроса и
/// оба запуска — `cancelled`.
#[tokio::test]
async fn cancelling_the_request_stops_its_run() {
    let e = env(Preset::Claude, None).await;
    let working = new_request(&e.st, "it");
    let queued = new_request(&e.st, "it");
    e.spawner.push("hang", &[]);
    let a = e.st.agent.start(&working.id).unwrap().id;
    let b = e.st.agent.start(&queued.id).unwrap().id;
    wait_run(&e.st, &a, "запуск работает", |r| {
        r.status == RunStatus::Running
    })
    .await;
    let pids = run_pids(&e.dir, &a).await;

    cancel_request(&e.st, &queued.id).await.unwrap();
    let b_run = wait_end(&e.st, &b).await;
    assert_eq!(b_run.status, RunStatus::Cancelled, "{b_run:?}");
    assert!(
        b_run.started_at_ms.is_none(),
        "снят с очереди до старта: {b_run:?}"
    );
    assert_eq!(e.spawner.spawned(), 1, "второй агент не запускался");

    cancel_request(&e.st, &working.id).await.unwrap();
    wait_gone(&pids).await;
    let a_run = wait_end(&e.st, &a).await;
    assert_eq!(a_run.status, RunStatus::Cancelled, "{a_run:?}");
    for r in [&working, &queued] {
        assert_eq!(request_status(&e.st, &r.id), RequestStatus::Cancelled);
    }
    e.finish().await;
}

/// Код выхода ≠ 0 после подключения (спец. §3.2): `failed` / `exit` с кодом; без события `result` итога нет.
#[tokio::test]
async fn crash_is_a_failed_run_with_the_exit_code() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("crash", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert_eq!(run.reason, Some(FailReason::Exit), "{run:?}");
    assert_eq!(run.exit_code, Some(3), "{run:?}");
    assert_eq!(run.summary, None, "{run:?}");
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Waiting);
    e.finish().await;
}

/// Вывод больше 1 МБ (спец. §4.5): журнал запуска — до 1 МБ и пометка `…[truncated]`, итоговый текст — до 4 КБ.
#[tokio::test]
async fn flood_is_cut_in_the_run_log() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("flood", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    assert_eq!(run.status, RunStatus::Done, "{run:?}");
    let len = std::fs::metadata(log_path(&e.dir, &id)).unwrap().len();
    assert!(len <= MAX_LOG_BYTES as u64 + 4096, "журнал {len} байт");
    assert!(read_log(&e.dir, &id).contains("…[truncated]"));
    let summary = run.summary.as_deref().unwrap_or_default();
    assert!(
        summary.starts_with("Fake agent summary: flooded the output.")
            && summary.len() <= MAX_SUMMARY_BYTES
            && summary.len() > MAX_SUMMARY_BYTES - 8,
        "итог агента длиннее 10 КБ обрезан до 4 КБ: {}",
        summary.len()
    );
    e.finish().await;
}

/// Предел 40 вызовов за запуск (спец. §5.2): 41-й — `run_limit`; запуск без предложений — `failed` / `limit`.
#[tokio::test]
async fn forty_one_calls_hit_the_run_limit() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("limit", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert!(
        notes.contains("call=40 tool=list_connections ok"),
        "{notes}"
    );
    assert!(
        notes.contains("call=41 tool=list_connections error=run_limit"),
        "{notes}"
    );
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert_eq!(run.reason, Some(FailReason::Limit), "{run:?}");
    assert!(run.calls >= 40, "{run:?}");
    e.finish().await;
}

/// Область сессии запуска (спец. §5.2): девять инструментов, prompts нет, `list_connections` — только подключение запроса;
/// чужое подключение, чужой запрос, `run_select`, прямая форма предложения и предложение на чужой запрос — `not_allowed`.
#[tokio::test]
async fn foreign_connection_request_and_tools_are_not_allowed() {
    let e = env(Preset::Claude, None).await;
    let own = new_request(&e.st, "it");
    let foreign = new_request(&e.st, "other");
    e.spawner.push(
        "foreign",
        &[
            ("FAKE_AGENT_FOREIGN_CONNECTION", "other"),
            ("FAKE_AGENT_FOREIGN_REQUEST", foreign.id.as_str()),
        ],
    );
    let id = e.st.agent.start(&own.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert_eq!(run.status, RunStatus::Done, "{run:?}\n{notes}");
    assert_eq!(noted(&notes, "tools").as_deref(), Some("9"), "{notes}");
    assert_eq!(noted(&notes, "prompts").as_deref(), Some("0"), "{notes}");
    assert_eq!(
        noted(&notes, "connections").as_deref(),
        Some("it"),
        "{notes}"
    );
    for tool in ["explain_query", "get_optimization_request", "run_select"] {
        assert!(
            notes.contains(&format!("tool={tool} error=not_allowed")),
            "{tool}: {notes}"
        );
    }
    assert_eq!(
        notes
            .matches("tool=propose_query_rewrite error=not_allowed")
            .count(),
        2,
        "прямая форма и чужой запрос: {notes}"
    );
    assert!(run.proposal_ids.is_empty());
    for r in [&own, &foreign] {
        assert!(
            e.st.llm
                .requests
                .get(&r.id)
                .unwrap()
                .proposal_ids
                .is_empty()
        );
    }
    e.finish().await;
}

/// «Проверить» (спец. §8.3) с пресетом Qwen Code: запуск `check` без подключения и запроса, `list_connections` пуст, итог
/// `OK` из события `result`; промпт — только в stdin; стоимость Qwen Code не пишется.
#[tokio::test]
async fn check_run_with_the_qwen_preset_answers_ok() {
    let e = env(Preset::Qwen, None).await;
    e.spawner.push("check", &[]);
    let id = e.st.agent.check().unwrap().id;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert_eq!(run.kind, RunKind::Check);
    assert_eq!(run.status, RunStatus::Done, "{run:?}\n{notes}");
    assert_eq!(run.preset, Preset::Qwen);
    assert!(
        run.request_id.is_none() && run.connection_id.is_none(),
        "{run:?}"
    );
    assert_eq!(run.summary.as_deref(), Some("OK"), "{run:?}");
    assert_eq!(run.cost_usd, None, "{run:?}");
    assert_eq!(
        noted(&notes, "connections").as_deref(),
        Some("0"),
        "{notes}"
    );
    assert_eq!(
        noted(&notes, "prompt_request").as_deref(),
        Some("-"),
        "{notes}"
    );
    assert!(
        noted(&notes, "prompt_bytes").is_some_and(|n| n != "0"),
        "промпт пришёл через stdin: {notes}"
    );
    let spec = e.spawner.last_spec();
    assert!(
        spec.args
            .windows(2)
            .any(|w| w[0] == "-o" && w[1] == "stream-json"),
        "{:?}",
        spec.args
    );
    e.finish().await;
}

/// Программы нет (спец. §3.2): `failed` / `cliNotFound`, запрос ждёт дальше.
#[tokio::test]
async fn missing_program_is_cli_not_found() {
    let e = env(Preset::Claude, None).await;
    let mut a = e.st.settings.agent();
    a.claude.path = e.dir.join("no-such-agent").display().to_string();
    e.st.settings.set_agent(a).unwrap();
    let req = new_request(&e.st, "it");
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    assert_eq!(run.status, RunStatus::Failed, "{run:?}");
    assert_eq!(run.reason, Some(FailReason::CliNotFound), "{run:?}");
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Waiting);
    e.finish().await;
}

/// Сброс упавшего теста снимает только процессы этой сборки (ревью задачи 8, замечание 1): программа процесса сверяется
/// путём целиком, а не подстрокой «glafira» (под неё попали бы Glafira владельца и бинари других worktree).
#[cfg(unix)]
#[test]
fn only_programs_of_this_test_build_are_ours() {
    // Фальшивый агент ждёт конца stdin; без каталога данных теста шим он не запускает.
    let mut agent = std::process::Command::new(FAKE_AGENT)
        .env_remove(TEST_DATA_DIR_ENV)
        .env_remove("FAKE_AGENT_NOTES")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let program = program_of(agent.id());
    drop(agent.stdin.take());
    let _ = agent.wait();
    let program = program.expect("путь программы живого процесса");
    assert!(ours(&program), "{}", program.display());
    let me = program_of(std::process::id()).unwrap();
    assert!(!ours(&me), "{}", me.display());
    assert!(!ours(Path::new(
        "/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg"
    )));
}

/// Страховка фальшивого агента (ревью задачи 8, замечание 2): сервер MCP-конфига он запускает только как шим — с
/// `args: ["mcp"]`. Без `mcp` бинарь Glafira был бы приложением с настоящим каталогом данных владельца, поэтому вместо него
/// здесь — скрипт, который только оставляет метку.
#[cfg(unix)]
#[test]
fn the_fake_agent_starts_the_server_of_the_config_only_as_the_shim() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let started = dir.path().join("started");
    let server = dir.path().join("server");
    std::fs::write(&server, format!("#!/bin/sh\n: > '{}'\n", started.display())).unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    let agent = |args: Value| {
        let config = dir.path().join("config.json");
        let v = json!({ "mcpServers": { "glafira-pg": { "command": server, "args": args } } });
        std::fs::write(&config, v.to_string()).unwrap();
        std::process::Command::new(FAKE_AGENT)
            .args([
                "--mcp-config",
                config.to_str().unwrap(),
                "-o",
                "stream-json",
            ])
            .env(TEST_DATA_DIR_ENV, dir.path())
            .env("FAKE_AGENT_MODE", "none")
            .env_remove("FAKE_AGENT_NOTES")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    };
    for args in [json!([]), json!(["mcp", "--x"]), json!(["serve"])] {
        let out = agent(args.clone());
        assert_eq!(out.status.code(), Some(2), "{args}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("must be [\"mcp\"]"), "{args}: {err}");
        assert!(!started.exists(), "{args}: сервер запущен");
    }
    // С `mcp` сервер запускается (здесь он сразу выходит — сессии нет, агент кончается ошибкой).
    agent(json!(["mcp"]));
    assert!(started.exists());
}

/// Проверка инструментов (спец. §4.1, итог задачи 1): в событии `init` кроме инструментов Glafira есть встроенный
/// инструмент CLI — группа процессов останавливается до первого хода, `failed` / `unsafeTools` с предупреждением
/// `unsafe <имя>`, ни одного вызова в журнале шлюза. Агент сделал бы вызов через 5 с после `init`.
#[tokio::test]
async fn tools_beyond_glafira_stop_the_run_before_the_first_turn() {
    let e = env(Preset::Qwen, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("unsafe", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let pids = run_pids(&e.dir, &id).await;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert_eq!(run.status, RunStatus::Failed, "{run:?}\n{notes}");
    assert_eq!(run.reason, Some(FailReason::UnsafeTools), "{run:?}");
    assert_eq!(
        run.warnings,
        vec!["unsafe run_shell_command".to_string()],
        "{run:?}"
    );
    assert_eq!(run.calls, 0, "{run:?}");
    assert_eq!(run.summary, None, "{run:?}");
    wait_gone(&pids).await;
    assert!(
        !notes.contains("call="),
        "агент не сделал ни одного хода: {notes}"
    );
    let calls = run_calls(&e.st, &id);
    assert!(calls.is_empty(), "в журнале шлюза нет вызовов: {calls:?}");
    assert_eq!(request_status(&e.st, &req.id), RequestStatus::Waiting);
    assert!(
        !e.dir
            .join("agent-runs")
            .join(format!("{id}.mcp.json"))
            .exists(),
        "MCP-конфиг удалён"
    );
    let f = finished_record(&e.st, &id).await;
    assert_eq!(
        (f["status"].as_str(), f["reason"].as_str()),
        (Some("failed"), Some("unsafeTools")),
        "{f}"
    );
    e.finish().await;
}

/// Одна живая сессия на запуск (решения плана, п. 7; ревью задачи 4): вторая сессия с тем же билетом — второй шим — через
/// настоящий сервер сменяет первую; Glafira закрывает первую сессию, её шим заканчивается сам, вызов через неё до шлюза не
/// доходит, вторая работает дальше. Обе сессии подтверждены билетом, первая закрыта до конца запуска.
#[tokio::test]
async fn a_new_session_of_the_run_closes_the_previous_one() {
    let e = env(Preset::Claude, None).await;
    let req = new_request(&e.st, "it");
    e.spawner.push("twice", &[]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert_eq!(run.status, RunStatus::Done, "{run:?}\n{notes}");
    assert!(notes.contains("call=1 tool=list_connections ok"), "{notes}");
    assert_eq!(
        noted(&notes, "first_shim_exit").as_deref(),
        Some("0"),
        "Glafira закрыла первую сессию — её шим закончился сам: {notes}"
    );
    assert!(
        notes.contains("call=2 tool=list_connections error=protocol"),
        "первая сессия закрыта: {notes}"
    );
    assert!(
        notes.contains("call=3 tool=list_connections ok"),
        "вторая сессия работает: {notes}"
    );
    assert_eq!(run.calls, 2, "до шлюза дошли два вызова: {run:?}");
    let approved: Vec<String> = audit_events(&e.st, "approved")
        .into_iter()
        .filter(|x| x["run"] == id.as_str() && x["by"] == "run")
        .filter_map(|x| x["session"].as_str().map(str::to_string))
        .collect();
    assert_eq!(approved.len(), 2, "{approved:?}");
    let started: Vec<Value> = audit_events(&e.st, "sessionStarted")
        .into_iter()
        .filter(|x| x["run"] == id.as_str())
        .collect();
    assert_eq!(started.len(), 2, "{started:?}");
    let ended = audit_events(&e.st, "sessionEnded");
    assert!(
        ended.iter().any(|x| x["session"] == approved[0].as_str()),
        "первая сессия закрыта: {ended:?}"
    );
    let calls = run_calls(&e.st, &id);
    assert_eq!(
        calls
            .iter()
            .map(|x| x["session"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        approved,
        "по вызову в каждой сессии: {calls:?}"
    );
    // Сессии запуска очередь закрывает в его конце; закрытие идёт в задачах сервера.
    common::wait_until(
        "сессий запуска после конца нет",
        Duration::from_secs(10),
        async || e.st.mcp.status().sessions.is_empty(),
    )
    .await;
    e.finish().await;
}

/// Стенд (спец. §12 «Стенд», `GPG_IT_PERF`): фальшивый агент на настоящем запросе pg-perf через настоящий `AppBackend` —
/// предложение проходит проверки Glafira (`prepare`, проба плана), получает `runId`, «Сравнить планы» работает с hypopg.
#[tokio::test]
async fn propose_on_the_perf_stand() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (tmp, st) = common::open_server_in(&perf, "perf").await;
    let dir = tmp.path().to_path_buf();
    let spawner = TestSpawner::new(&dir);
    let st = st.with_agent(runtime(&spawner, None));
    set_access(&st, "it", LlmAccess::Diagnostics);
    st.settings
        .set_agent(agent_settings(Preset::Claude))
        .unwrap();
    let status = st.mcp.set_enabled(st.clone(), true).await.unwrap();
    assert_eq!(status.server, McpServerState::Running, "{:?}", status.error);
    let e = Env {
        _tmp: tmp,
        dir,
        st,
        spawner,
    };
    let req = create_request(
        &e.st,
        "it",
        CreateRequestArgs {
            member: member(&e.st),
            database: "perf".into(),
            source: RequestSource::Console,
            sql: PERF_SQL.into(),
            params: vec![],
            plan: None,
        },
    )
    .await
    .unwrap();
    e.spawner
        .push("propose", &[("FAKE_AGENT_INDEXES", PERF_INDEX)]);
    let id = e.st.agent.start(&req.id).unwrap().id;
    let run = wait_end(&e.st, &id).await;
    let notes = read_notes(&e.dir, &id);
    assert_eq!(run.status, RunStatus::Done, "{run:?}\n{notes}");
    assert_eq!(run.proposal_ids.len(), 1, "{run:?}\n{notes}");
    let p = compare_proposal(&e.st, &run.proposal_ids[0]).await.unwrap();
    assert_eq!(p.run_id.as_deref(), Some(run.id.as_str()));
    assert!(
        p.rewritten_sql.contains("fake_agent_rewrite"),
        "{}",
        p.rewritten_sql
    );
    let c = p.comparison.expect("сравнение планов");
    assert!(c.cost_before > 0.0 && c.cost_after > 0.0, "{c:?}");
    assert_eq!(c.indexes_used.len(), 1, "{c:?}");
    e.finish().await;
}
