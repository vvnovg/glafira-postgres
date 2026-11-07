//! Процесс фонового агента (спец. P4c §4.1, §4.2, §4.4, §4.5): запуск без окна в своей группе процессов (Unix) или в Job
//! Object (Windows); промпт — в stdin, затем stdin закрывается; stdout и stderr — в журнал запуска с метками потока и
//! времени (до `MAX_LOG_BYTES`, сбрасывается после каждой порции), хвост stdout — в память для итогового текста. Из потока
//! stdout читается событие `system`/`init` — список инструментов модели до первого хода (проверка инструментов, §4.1).
//! Остановка — SIGTERM группе, через `STOP_GRACE` SIGKILL (Unix) или `TerminateJobObject` (Windows): так останавливаются и
//! дочерние процессы (шим, `node`). Когда процесс закончился сам, остатки группы (Job Object) тоже останавливаются: держать
//! вывод открытым им незачем.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::detect;
use super::presets::{CommandSpec, OutputFormat};
use super::runs::{MAX_LOG_BYTES, MAX_SUMMARY_BYTES, ensure_private_dir};
use super::settings::Preset;
use super::shell_env::{self, ShellEnv};
use crate::mcp::llm::plain_text;
use crate::product::MCP_SERVER_NAME;
use crate::sync::MutexExt;

/// SIGTERM → столько ждать → SIGKILL (спец. §4.4).
pub const STOP_GRACE: Duration = Duration::from_secs(5);
/// Хвост stdout в памяти: итог — последнее событие потока; запасной разбор `-o json` Qwen Code (весь разговор массивом)
/// тоже находит его в хвосте.
pub const STDOUT_TAIL_BYTES: usize = 64 * 1024;
/// Событие `init` приходит до первого хода модели; дольше очередь его не ждёт (§4.1 «Проверка инструментов»).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Строка потока длиннее не разбирается при поиске `init` (само событие — единицы килобайт).
const MAX_INIT_LINE: usize = 1 << 20;
/// Сколько ждать конца чтения вывода после конца процесса.
const READERS_GRACE: Duration = Duration::from_secs(2);
/// Qwen Code: предел `--max-wall-time` / `--max-tool-calls` (`FatalBudgetExceededError`).
pub const QWEN_LIMIT_EXIT: i32 = 55;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Итог процесса: код выхода (`None` — убит сигналом) и хвост stdout.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub stdout_tail: String,
}

/// Запущенный агент. `wait` и `init_tools` можно звать повторно и прерывать: итог запоминается.
pub trait AgentProcess: Send {
    fn wait(&mut self) -> BoxFuture<'_, ExitInfo>;
    /// Остановка всей группы процессов: SIGTERM, через паузу — SIGKILL; итог потом — `wait`.
    fn stop(&mut self) -> BoxFuture<'_, ()>;
    /// Остановка всей группы сразу, без паузы (SIGKILL группе, на Windows — `TerminateJobObject`): проверка инструментов
    /// не прошла, первый ход модели продолжаться не должен (§4.1 «сразу останавливает»). Итог потом — `wait`.
    fn kill(&mut self) -> BoxFuture<'_, ()>;
    /// Что поток stdout сказал об инструментах модели до первого хода (§4.1 «Проверка инструментов»): первое событие
    /// `{"type": "system", "subtype": "init", "tools": […]}`, событие без списка, ход модели (событие не `system`) или
    /// строка не JSON до него, конец потока.
    /// Срок ожидания задаёт вызывающий (`INIT_TIMEOUT`).
    fn init_tools(&mut self) -> BoxFuture<'_, InitTools>;
}

/// Итог поиска события `init` в потоке stdout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitTools {
    /// Событие `init` со списком инструментов.
    Listed(Vec<String>),
    /// Событие `init` без списка `tools` или событие не `system` до него (ход модели `assistant` / `user` и прочее):
    /// проверить нечего, а модель уже делает первый ход.
    NoList,
    /// До события `init` пришла строка не JSON, или вывод не `stream-json` вовсе: поток не тот, проверить нечем.
    NotStreamJson,
    /// Поток stdout кончился без события `init`: обычно процесс вышел сам (ранняя ошибка CLI).
    Ended,
}

/// Запуск процесса агента. В приложении — `TokioSpawner`, в тестах очереди — фальшивый агент.
pub trait AgentSpawner: Send + Sync {
    /// Версия программы для записи запуска (`--version`, до 10 с); по умолчанию неизвестна. Блокирующий вызов.
    fn version(&self, _program: &Path) -> Option<String> {
        None
    }

    /// Запуск: `stdin` — промпт, `log` — журнал запуска. Ошибка `NotFound` — программы нет. Блокирующий вызов: при
    /// первом запуске снимается окружение логин-оболочки (до 5 с).
    fn spawn(
        &self,
        spec: &CommandSpec,
        stdin: String,
        log: PathBuf,
    ) -> io::Result<Box<dyn AgentProcess>>;
}

/// Итог поиска события `init` (`AgentProcess::init_tools`): ожидание по каналу от читателя stdout, итог запоминается.
/// Общий для процессов приложения и фальшивых агентов тестов.
pub(crate) struct InitWatch {
    rx: Option<oneshot::Receiver<InitTools>>,
    got: InitTools,
}

impl InitWatch {
    /// Канал: отправитель — у читателя потока; брошенный без отправки отправитель — `InitTools::Ended`.
    pub(crate) fn channel() -> (oneshot::Sender<InitTools>, Self) {
        let (tx, rx) = oneshot::channel();
        (
            tx,
            Self {
                rx: Some(rx),
                got: InitTools::Ended,
            },
        )
    }

    pub(crate) async fn get(&mut self) -> InitTools {
        if let Some(rx) = self.rx.as_mut() {
            let got = rx.await.unwrap_or(InitTools::Ended);
            self.rx = None;
            self.got = got;
        }
        self.got.clone()
    }
}

/// Имена инструментов вне `glafira-pg` (`mcp__glafira-pg__<имя>`): с ними запуск останавливается до первого хода.
pub fn foreign_tools(tools: &[String]) -> Vec<String> {
    let prefix = format!("mcp__{MCP_SERVER_NAME}__");
    tools
        .iter()
        .filter(|t| t.strip_prefix(&prefix).is_none_or(str::is_empty))
        .cloned()
        .collect()
}

/// Поиск события `init` в порциях stdout: строки собираются через границы порций; пустая строка и слишком длинная
/// строка пропускаются.
#[derive(Default)]
struct InitScan {
    line: Vec<u8>,
    skipping: bool,
}

impl InitScan {
    /// Порция вывода; `Some` — найдено первое событие `init` или строка не JSON до него (дальше звать незачем).
    fn feed(&mut self, mut data: &[u8]) -> Option<InitTools> {
        while !data.is_empty() {
            let (part, rest, ends) = match data.iter().position(|b| *b == b'\n') {
                Some(i) => (&data[..i], &data[i + 1..], true),
                None => (data, &data[data.len()..], false),
            };
            data = rest;
            if !self.skipping {
                if self.line.len() + part.len() > MAX_INIT_LINE {
                    self.line.clear();
                    self.skipping = true;
                } else {
                    self.line.extend_from_slice(part);
                }
            }
            if ends {
                let found = (!self.skipping).then(|| init_of(&self.line)).flatten();
                self.line.clear();
                self.skipping = false;
                if found.is_some() {
                    return found;
                }
            }
        }
        None
    }
}

/// Строка потока до события `init`: само событие (имя инструмента — строка; объект с `name` — его имя; прочее — JSON
/// текстом, такое имя проверку не пройдёт), строка не JSON (`NotStreamJson`), другое системное событие или служебное
/// событие входа `auth_status` (`None`, у Claude Code до `init` идут `hook_started` и `hook_response`, при входе по
/// подписке — `auth_status`) или любое другое событие (`NoList`): ход модели (`assistant`, `user`) или незнакомое
/// событие до списка инструментов — модель уже работает, а проверить нечем (повторное ревью задачи 6, замечание 1).
/// Список допустимых — разрешающий: новое незнакомое событие останавливает запуск, а не пропускается.
fn init_of(line: &[u8]) -> Option<InitTools> {
    let t = line.trim_ascii();
    if t.is_empty() {
        return None;
    }
    let Ok(v) = serde_json::from_slice::<Value>(t) else {
        return Some(InitTools::NotStreamJson);
    };
    match v.get("type").and_then(Value::as_str) {
        Some("system") => {}
        Some("auth_status") => return None,
        _ => return Some(InitTools::NoList),
    }
    if v.get("subtype").and_then(Value::as_str) != Some("init") {
        return None;
    }
    Some(match v.get("tools").and_then(Value::as_array) {
        Some(items) => InitTools::Listed(
            items
                .iter()
                .map(|t| match t {
                    Value::String(s) => s.clone(),
                    _ => t
                        .get("name")
                        .and_then(Value::as_str)
                        .map_or_else(|| t.to_string(), String::from),
                })
                .collect(),
        ),
        None => InitTools::NoList,
    })
}

#[derive(Clone, Copy)]
enum Stream {
    Out = 0,
    Err = 1,
}

impl Stream {
    fn tag(self) -> &'static str {
        match self {
            Stream::Out => "out",
            Stream::Err => "err",
        }
    }
}

/// Журнал запуска: строки с меткой времени от старта и потока; после `MAX_LOG_BYTES` — пометка и только счётчик байт.
struct RunLog {
    file: io::BufWriter<std::fs::File>,
    started: Instant,
    written: usize,
    dropped: usize,
    line_start: [bool; 2],
    truncated: bool,
}

impl RunLog {
    fn create(path: &Path, header: &str) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            ensure_private_dir(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut log = Self {
            file: io::BufWriter::new(opts.open(path)?),
            started: Instant::now(),
            written: 0,
            dropped: 0,
            line_start: [true, true],
            truncated: false,
        };
        log.put(header.as_bytes());
        Ok(log)
    }

    fn put(&mut self, bytes: &[u8]) {
        let room = MAX_LOG_BYTES.saturating_sub(self.written);
        let n = bytes.len().min(room);
        if n > 0 {
            let _ = self.file.write_all(&bytes[..n]);
            self.written += n;
        }
        if n < bytes.len() {
            self.dropped += bytes.len() - n;
            if !self.truncated {
                self.truncated = true;
                let _ = self.file.write_all("\n…[truncated]\n".as_bytes());
            }
        }
    }

    fn write(&mut self, stream: Stream, mut data: &[u8]) {
        let i = stream as usize;
        while !data.is_empty() {
            if self.line_start[i] {
                let mark = format!(
                    "[{:>9.3}s {}] ",
                    self.started.elapsed().as_secs_f64(),
                    stream.tag()
                );
                self.put(mark.as_bytes());
                self.line_start[i] = false;
            }
            let end = data
                .iter()
                .position(|b| *b == b'\n')
                .map_or(data.len(), |p| p + 1);
            let (line, rest) = data.split_at(end);
            self.put(line);
            self.line_start[i] = line.ends_with(b"\n");
            data = rest;
        }
        // «Показать журнал» у идущего запуска читает файл: порция видна сразу, а не после `finish` (спека §4.5).
        let _ = self.file.flush();
    }

    fn finish(&mut self, code: Option<i32>) {
        if self.dropped > 0 {
            let _ = writeln!(
                self.file,
                "\n# …[truncated]: {} bytes not written",
                self.dropped
            );
        }
        let code = code.map_or("none (killed by a signal)".to_string(), |c| c.to_string());
        let _ = writeln!(self.file, "\n# exit code: {code}");
        let _ = self.file.flush();
    }
}

/// Чтение потока в журнал; у stdout — ещё хвост в памяти и поиск события `init` (`init` — его получатель).
fn read_stream<R>(
    mut r: R,
    stream: Stream,
    log: Arc<Mutex<RunLog>>,
    keep_tail: bool,
    mut init: Option<oneshot::Sender<InitTools>>,
) -> JoinHandle<Vec<u8>>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut tail: VecDeque<u8> = VecDeque::new();
        let mut scan = InitScan::default();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            log.lock_safe().write(stream, chunk);
            if let Some(found) = init.as_ref().and_then(|_| scan.feed(chunk))
                && let Some(tx) = init.take()
            {
                // Первое событие `init` или строка не JSON до него: дальше поток не разбирается.
                let _ = tx.send(found);
            }
            if keep_tail {
                tail.extend(chunk);
                let extra = tail.len().saturating_sub(STDOUT_TAIL_BYTES);
                tail.drain(..extra);
            }
        }
        tail.into_iter().collect()
    })
}

/// Чтение закончилось (поток закрыт) или не закончилось за `READERS_GRACE` — тогда прерывается.
async fn join_reader(h: &mut Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    let Some(handle) = h.as_mut() else {
        return Vec::new();
    };
    let out = match tokio::time::timeout(READERS_GRACE, &mut *handle).await {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => Vec::new(),
        Err(_) => {
            handle.abort();
            Vec::new()
        }
    };
    *h = None;
    out
}

/// Процессы агента на tokio: окружение логин-оболочки (`shell_env`, снимается при первом запуске) и переменные пресета.
pub struct TokioSpawner {
    env: Option<Arc<ShellEnv>>,
    grace: Duration,
}

impl TokioSpawner {
    pub fn new() -> Self {
        Self {
            env: None,
            grace: STOP_GRACE,
        }
    }

    /// Заданное окружение вместо логин-оболочки (тесты).
    pub fn with_env(env: Arc<ShellEnv>) -> Self {
        Self {
            env: Some(env),
            grace: STOP_GRACE,
        }
    }

    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    fn env(&self) -> Arc<ShellEnv> {
        self.env.clone().unwrap_or_else(shell_env::shell_env)
    }
}

impl Default for TokioSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentSpawner for TokioSpawner {
    fn version(&self, program: &Path) -> Option<String> {
        detect::version_of(program, &self.env(), detect::VERSION_TIMEOUT).ok()
    }

    fn spawn(
        &self,
        spec: &CommandSpec,
        stdin: String,
        log: PathBuf,
    ) -> io::Result<Box<dyn AgentProcess>> {
        // Аргументы без секретов: билет — в файле MCP-конфига, промпт — в stdin. Окружение в журнал не пишется: в нём
        // бывают ключи API из профиля оболочки.
        let header = format!(
            "# {} {:?}\n# cwd {}\n",
            spec.program.display(),
            spec.args,
            spec.cwd.display()
        );
        let mut run_log = RunLog::create(&log, &header)?;
        if !spec.program.is_file() {
            // Программы нет (или это каталог): `cliNotFound` с путём, не ошибка запуска ОС, у которой текст свой.
            run_log.put(b"# spawn failed: the program is not found\n");
            run_log.finish(None);
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} is not found", spec.program.display()),
            ));
        }
        let env = self.env();
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(env.for_program(&spec.program))
            .envs(spec.env_set.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        for k in &spec.env_remove {
            cmd.env_remove(k);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                run_log.put(format!("# spawn failed: {e}\n").as_bytes());
                run_log.finish(None);
                return Err(e);
            }
        };
        // `RawHandle` и `HANDLE` — один тип (`*mut c_void`).
        #[cfg(windows)]
        let job = match child.raw_handle().map(job::Job::for_process) {
            Some(Ok(j)) => Some(j),
            Some(Err(e)) => {
                tracing::warn!("agent job object not created: {e}");
                None
            }
            None => None,
        };
        #[cfg(unix)]
        let pgid = child.id().and_then(|id| i32::try_from(id).ok());
        if let Some(mut input) = child.stdin.take() {
            tokio::spawn(async move {
                let _ = input.write_all(stdin.as_bytes()).await;
                let _ = input.shutdown().await;
            });
        }
        let log = Arc::new(Mutex::new(run_log));
        let (init_tx, init) = InitWatch::channel();
        // Событие `init` есть только в потоке JSON; простой текст проверить нечем.
        let init_tx = if spec.output == OutputFormat::StreamJson {
            Some(init_tx)
        } else {
            let _ = init_tx.send(InitTools::NotStreamJson);
            None
        };
        let out = child
            .stdout
            .take()
            .map(|r| read_stream(r, Stream::Out, log.clone(), true, init_tx));
        let err = child
            .stderr
            .take()
            .map(|r| read_stream(r, Stream::Err, log.clone(), false, None));
        Ok(Box::new(TokioProcess {
            child,
            #[cfg(unix)]
            pgid,
            #[cfg(windows)]
            job,
            out,
            err,
            log,
            init,
            grace: self.grace,
            tail: None,
            exit: None,
        }))
    }
}

struct TokioProcess {
    child: tokio::process::Child,
    /// Группа процессов агента (= pid лидера, `process_group(0)`).
    #[cfg(unix)]
    pgid: Option<i32>,
    #[cfg(windows)]
    job: Option<job::Job>,
    out: Option<JoinHandle<Vec<u8>>>,
    err: Option<JoinHandle<Vec<u8>>>,
    log: Arc<Mutex<RunLog>>,
    init: InitWatch,
    grace: Duration,
    /// Хвост stdout, когда чтение уже закончилось (`wait` прервали после него).
    tail: Option<Vec<u8>>,
    exit: Option<ExitInfo>,
}

impl TokioProcess {
    #[cfg(unix)]
    fn signal(&self, sig: i32) {
        if let Some(pg) = self.pgid {
            // SAFETY: `killpg` только посылает сигнал своей группе процессов агента; ошибка (ESRCH — группы уже нет) не
            // важна.
            unsafe {
                libc::killpg(pg, sig);
            }
        }
    }

    /// Всё, что осталось от агента: шим, `node`, фоновые процессы, держащие вывод.
    fn kill_leftovers(&self) {
        #[cfg(unix)]
        {
            self.signal(libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            if let Some(j) = &self.job {
                j.terminate();
            }
        }
    }
}

impl AgentProcess for TokioProcess {
    fn wait(&mut self) -> BoxFuture<'_, ExitInfo> {
        Box::pin(async move {
            if let Some(e) = &self.exit {
                return e.clone();
            }
            // Прерывать можно на любом `await`: `Child::wait` и `join_reader` это допускают, а прочитанный хвост stdout
            // сразу запоминается (очередь ждёт конца процесса со сроком, пока проверяет событие `init`).
            let status = self.child.wait().await;
            self.kill_leftovers();
            let code = status.ok().and_then(|s| s.code());
            if self.tail.is_none() {
                let tail = join_reader(&mut self.out).await;
                self.tail = Some(tail);
            }
            join_reader(&mut self.err).await;
            self.log.lock_safe().finish(code);
            let info = ExitInfo {
                code,
                stdout_tail: String::from_utf8_lossy(self.tail.as_deref().unwrap_or_default())
                    .into_owned(),
            };
            self.exit = Some(info.clone());
            info
        })
    }

    fn stop(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.exit.is_some() {
                return;
            }
            #[cfg(unix)]
            {
                self.signal(libc::SIGTERM);
            }
            #[cfg(windows)]
            {
                match &self.job {
                    Some(j) => j.terminate(),
                    None => {
                        let _ = self.child.start_kill();
                    }
                }
            }
            if tokio::time::timeout(self.grace, self.child.wait())
                .await
                .is_err()
            {
                #[cfg(unix)]
                {
                    self.signal(libc::SIGKILL);
                }
                let _ = self.child.start_kill();
            }
        })
    }

    fn kill(&mut self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if self.exit.is_some() {
                return;
            }
            // SIGKILL группе (Windows — `TerminateJobObject`); лидер — ещё и напрямую, если группы или Job Object нет.
            self.kill_leftovers();
            let _ = self.child.start_kill();
        })
    }

    fn init_tools(&mut self) -> BoxFuture<'_, InitTools> {
        Box::pin(self.init.get())
    }
}

impl Drop for TokioProcess {
    /// Процесс бросили, не дождавшись (выход из Glafira): группа останавливается; на Windows Job Object закрывается сам.
    fn drop(&mut self) {
        if self.exit.is_none() {
            self.kill_leftovers();
        }
    }
}

#[cfg(windows)]
mod job {
    use std::io;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// Job Object процесса агента: `KILL_ON_JOB_CLOSE` — дочерние процессы останавливаются и при закрытии дескриптора
    /// (выход Glafira, в том числе аварийный).
    pub struct Job(HANDLE);

    // SAFETY: дескриптор Job Object — объект ядра; его можно использовать из любого потока.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn for_process(process: HANDLE) -> io::Result<Self> {
            // SAFETY: вызовы WinAPI с проверкой результата; структура ограничений — нули и один флаг.
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(h);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if AssignProcessToJobObject(h, process) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn terminate(&self) {
            // SAFETY: свой живой дескриптор; ошибка (процессов уже нет) не важна.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: дескриптор закрывается один раз.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

/// Итоговый текст агента и стоимость (спец. §4.5): поле `result` последнего события `"type": "result"` потока JSON по
/// строке (у обоих CLI) и `total_cost_usd`; запасной разбор — весь хвост JSON-массивом (`-o json` Qwen Code) или объектом
/// (`--output-format json` Claude Code). Нет события (срок, остановка, падение) — итога нет: сырой хвост потока JSON —
/// не ответ ассистента, вывод целиком — в «Показать журнал» (решение контроллера к задаче 6). У простого текста итог —
/// последние 4 КБ stdout. Управляющие последовательности терминала и символы убираются; до `MAX_SUMMARY_BYTES`.
/// Стоимость показывается только у Claude Code без Ollama — это решает очередь.
pub fn final_text(output: OutputFormat, stdout_tail: &str) -> (Option<String>, Option<f64>) {
    match output {
        OutputFormat::StreamJson => {
            let Some(obj) = result_event(stdout_tail) else {
                return (None, None);
            };
            let text = obj
                .get("result")
                .and_then(Value::as_str)
                .map(|s| clean(s, false))
                .filter(|s| !s.is_empty());
            let cost = obj.get("total_cost_usd").and_then(Value::as_f64);
            (text, cost)
        }
        OutputFormat::Text => {
            let text = clean(stdout_tail, true);
            ((!text.is_empty()).then_some(text), None)
        }
    }
}

/// Предел CLI сработал (спец. §3.2 `limit`, итог проверки CLI): Qwen Code — код 55 (`FatalBudgetExceededError`); Claude
/// Code — код ≠ 0 и `subtype` итога, начинающийся с `error_max` (`error_max_budget_usd`).
pub fn cli_limit(preset: Preset, code: Option<i32>, stdout_tail: &str) -> bool {
    match preset {
        Preset::Qwen => code == Some(QWEN_LIMIT_EXIT),
        Preset::Claude => {
            code != Some(0)
                && result_event(stdout_tail).is_some_and(|o| {
                    o.get("subtype")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.starts_with("error_max"))
                })
        }
    }
}

fn is_result(v: &Value) -> bool {
    v.get("type").and_then(Value::as_str) == Some("result")
}

/// Итог в хвосте вывода: последнее событие `"type": "result"` потока по строке (первая строка хвоста может быть обрезана —
/// она просто не разбирается), иначе запасной разбор `result_object`.
fn result_event(tail: &str) -> Option<Value> {
    tail.lines()
        .rev()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(is_result)
        .or_else(|| result_object(tail))
}

/// Запасной разбор: весь хвост — JSON (объект или массив) или, если начало массива не поместилось, последний объект с
/// `"type": "result"`.
fn result_object(tail: &str) -> Option<Value> {
    let t = tail.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return match v {
            Value::Array(items) => items.into_iter().rev().find(is_result),
            v if v.is_object() && (is_result(&v) || v.get("result").is_some()) => Some(v),
            _ => None,
        };
    }
    for pat in ["\"type\":\"result\"", "\"type\": \"result\""] {
        let Some(pos) = t.rfind(pat) else {
            continue;
        };
        let Some(start) = t[..pos].rfind('{') else {
            continue;
        };
        if let Some(Ok(v)) = serde_json::Deserializer::from_str(&t[start..])
            .into_iter::<Value>()
            .next()
        {
            return Some(v);
        }
    }
    None
}

/// Без управляющих последовательностей и символов (кроме перевода строки и табуляции), до `MAX_SUMMARY_BYTES`: у текста
/// stdout — конец, у поля `result` — начало.
fn clean(s: &str, keep_end: bool) -> String {
    cut_bytes(&plain_text(&strip_ansi(s)), MAX_SUMMARY_BYTES, keep_end)
}

fn cut_bytes(s: &str, max: usize, keep_end: bool) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    if keep_end {
        let mut start = s.len() - max;
        while !s.is_char_boundary(start) {
            start += 1;
        }
        s[start..].to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].to_string()
    }
}

/// Управляющие последовательности терминала: CSI (`ESC [ … буква`), OSC (`ESC ] … BEL` или `ESC \`) и прочие `ESC X`.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for n in chars.by_ref() {
                    if ('@'..='~').contains(&n) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(n) = chars.next() {
                    if n == '\u{7}' {
                        break;
                    }
                    if n == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawner() -> TokioSpawner {
        TokioSpawner::with_env(Arc::new(ShellEnv::current_process()))
            .with_grace(Duration::from_millis(300))
    }

    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/data/agent-work","tools":["mcp__glafira-pg__list_connections","mcp__glafira-pg__get_optimization_request"],"model":"qwen3.6-64k:35b-a3b"}"#;

    #[test]
    fn the_final_text_comes_from_the_last_result_event_of_the_stream() {
        // Claude Code `--output-format stream-json --verbose`: системные события, init, ходы, итог — по строке.
        let out = [
            r#"{"type":"system","subtype":"hook_started","hook_name":"SessionStart"}"#,
            INIT,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking at the plan"}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"num_turns":6,"result":"\u001b[1mNo proposals\u001b[0m: the plan already uses the index","total_cost_usd":0.0123}"#,
        ]
        .join("\n");
        let (text, cost) = final_text(OutputFormat::StreamJson, &format!("{out}\n"));
        assert_eq!(
            text.as_deref(),
            Some("No proposals: the plan already uses the index")
        );
        assert_eq!(cost, Some(0.0123));
        // Два события `result` (агент закончил дважды) — берётся последнее; Qwen Code стоимости не сообщает.
        let twice = format!(
            "{}\n{}\n",
            r#"{"type":"result","subtype":"success","result":"first"}"#,
            r#"{"type":"result","subtype":"success","result":"second","num_turns":3}"#
        );
        assert_eq!(
            final_text(OutputFormat::StreamJson, &twice),
            (Some("second".to_string()), None)
        );
        // Запасной разбор: один объект (`--output-format json` Claude Code).
        let object =
            r#"{"type":"result","subtype":"success","result":"Done","total_cost_usd":0.5}"#;
        assert_eq!(
            final_text(OutputFormat::StreamJson, object),
            (Some("Done".to_string()), Some(0.5))
        );
    }

    #[test]
    fn the_final_text_of_qwen_code_is_found_even_when_the_head_is_cut() {
        let big = "x".repeat(STDOUT_TAIL_BYTES);
        let messages = serde_json::json!([
            { "type": "system", "subtype": "session_start", "session_id": "s" },
            { "type": "assistant", "message": { "content": [{ "type": "text", "text": big }] } },
            { "type": "result", "subtype": "success", "is_error": false, "result": "Proposed an index on orders(status)." }
        ]);
        // Запасной разбор `-o json` Qwen Code: массив событий, целиком и с обрезанным началом.
        for full in [
            messages.to_string(),
            serde_json::to_string_pretty(&messages).unwrap(),
        ] {
            let (text, cost) = final_text(OutputFormat::StreamJson, &full);
            assert_eq!(
                text.as_deref(),
                Some("Proposed an index on orders(status).")
            );
            assert_eq!(cost, None, "Qwen Code стоимость не сообщает");
            let tail = &full[full.len() - 2000..];
            assert_eq!(
                final_text(OutputFormat::StreamJson, tail).0.as_deref(),
                Some("Proposed an index on orders(status)."),
                "начало массива не поместилось в хвост"
            );
        }
        // Поток `-o stream-json`: первая строка хвоста обрезана посередине.
        let stream = messages
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let tail = &stream[stream.len() - 3000..];
        assert!(serde_json::from_str::<Value>(tail.lines().next().unwrap()).is_err());
        assert_eq!(
            final_text(OutputFormat::StreamJson, tail).0.as_deref(),
            Some("Proposed an index on orders(status).")
        );
    }

    #[test]
    fn text_output_is_cleaned_and_cut_to_four_kilobytes() {
        let (text, cost) = final_text(
            OutputFormat::Text,
            "\u{1b}[32mOK\u{1b}[0m\r\n\u{1b}]0;title\u{7}",
        );
        assert_eq!((text.as_deref(), cost), (Some("OK"), None));
        let long = format!("{}END", "а".repeat(3000));
        let text = final_text(OutputFormat::Text, &long).0.unwrap();
        assert!(
            text.len() <= MAX_SUMMARY_BYTES && text.ends_with("END"),
            "{}",
            text.len()
        );
        // Поток JSON без события `result` (срок, остановка, падение) — итога нет: сырой хвост — не ответ ассистента.
        assert_eq!(
            final_text(OutputFormat::StreamJson, "plain words"),
            (None, None)
        );
        let no_result = format!(
            "{INIT}\n{}\n{}",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking"}]}}"#,
            "y".repeat(5000)
        );
        assert_eq!(
            final_text(OutputFormat::StreamJson, &no_result),
            (None, None)
        );
        assert_eq!(final_text(OutputFormat::StreamJson, "  \n"), (None, None));
        // У простого текста итог — конец хвоста.
        let text = final_text(OutputFormat::Text, &no_result).0.unwrap();
        assert!(
            text.len() == MAX_SUMMARY_BYTES && text.chars().all(|c| c == 'y'),
            "конец хвоста"
        );
        let json = serde_json::json!({ "type": "result", "result": "r".repeat(5000) }).to_string();
        assert_eq!(
            final_text(OutputFormat::StreamJson, &json).0.unwrap().len(),
            MAX_SUMMARY_BYTES,
            "у JSON — начало итога"
        );
        // Итог без поля `result` (предел расходов) — текста нет, хвост не подставляется.
        assert_eq!(
            final_text(
                OutputFormat::StreamJson,
                r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"total_cost_usd":1.02}"#
            ),
            (None, Some(1.02))
        );
        assert_eq!(
            strip_ansi("a\u{1b}[1;31mb\u{1b}[0mc\u{1b}]8;;http://x\u{1b}\\d"),
            "abcd"
        );
    }

    #[test]
    fn cli_limits_are_recognised() {
        assert!(cli_limit(Preset::Qwen, Some(QWEN_LIMIT_EXIT), ""));
        assert!(!cli_limit(Preset::Qwen, Some(1), ""));
        let budget = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true}"#;
        assert!(cli_limit(Preset::Claude, Some(1), budget));
        assert!(
            cli_limit(Preset::Claude, Some(1), &format!("{INIT}\n{budget}\n")),
            "итог — последнее событие потока"
        );
        assert!(
            !cli_limit(Preset::Claude, Some(0), budget),
            "Claude Code — только с кодом ≠ 0"
        );
        assert!(!cli_limit(
            Preset::Claude,
            Some(0),
            r#"{"type":"result","subtype":"success"}"#
        ));
        assert!(!cli_limit(
            Preset::Claude,
            Some(1),
            r#"{"type":"result","subtype":"error_during_execution"}"#
        ));
        assert!(
            !cli_limit(Preset::Claude, Some(55), ""),
            "код 55 — только у Qwen Code"
        );
    }

    #[test]
    fn the_init_event_lists_the_tools_of_the_model() {
        let mut scan = InitScan::default();
        // Порции рвут строки посередине; до `init` — пустые строки и другие системные события.
        let stream = format!(
            "\n  \r\n{}\n{INIT}\n{}\n",
            r#"{"type":"system","subtype":"hook_started"}"#,
            r#"{"type":"system","subtype":"init","tools":["run_shell_command"]}"#
        );
        // Как `read_stream`: после первого ответа поток не разбирается.
        let found = stream
            .as_bytes()
            .chunks(7)
            .find_map(|chunk| scan.feed(chunk));
        assert_eq!(
            found,
            Some(InitTools::Listed(vec![
                "mcp__glafira-pg__list_connections".into(),
                "mcp__glafira-pg__get_optimization_request".into()
            ]))
        );
        // Имена-объекты и прочее: объект с `name` — имя, остальное — JSON текстом (и не проходит проверку).
        let mut scan = InitScan::default();
        assert_eq!(
            scan.feed(
                br#"{"type":"system","subtype":"init","tools":[{"name":"mcp__glafira-pg__explain_query"},7]}
"#
            ),
            Some(InitTools::Listed(vec![
                "mcp__glafira-pg__explain_query".into(),
                "7".into()
            ]))
        );
        let mut scan = InitScan::default();
        assert_eq!(
            scan.feed(b"{\"type\":\"system\",\"subtype\":\"init\"}\n"),
            Some(InitTools::NoList),
            "init без списка инструментов"
        );
        // Строка не JSON до `init` — вывод не `stream-json`; после `init` поток уже не разбирается.
        let mut scan = InitScan::default();
        assert_eq!(
            scan.feed(b"{\"type\":\"system\",\"subtype\":\"hook_started\"}\nwarming up\n"),
            Some(InitTools::NotStreamJson)
        );
        let mut scan = InitScan::default();
        assert_eq!(
            scan.feed(b"\x1b[32m{\"type\":\"system\"}\n"),
            Some(InitTools::NotStreamJson)
        );
        // До `init` — только системные события: ход модели или другое событие — остановка сразу, без списка.
        for before in [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"[]"}]}}"#,
            r#"{"type":"result","subtype":"success","result":"done"}"#,
            r#"{"type":"stream_event","event":{}}"#,
            r#"{"subtype":"init","tools":[]}"#,
            "[]",
            "7",
        ] {
            let mut scan = InitScan::default();
            assert_eq!(
                scan.feed(
                    format!(
                        "{}\n{before}\n{INIT}\n",
                        r#"{"type":"system","subtype":"hook_response"}"#
                    )
                    .as_bytes()
                ),
                Some(InitTools::NoList),
                "{before}"
            );
        }
        // Служебное событие входа Claude Code до `init` — не ход модели: ждём `init`.
        let mut scan = InitScan::default();
        assert_eq!(
            scan.feed(
                format!(
                    "{}\n{INIT}\n",
                    r#"{"type":"auth_status","isAuthenticating":true,"output":[]}"#
                )
                .as_bytes()
            ),
            Some(InitTools::Listed(vec![
                "mcp__glafira-pg__list_connections".into(),
                "mcp__glafira-pg__get_optimization_request".into()
            ]))
        );
        // Слишком длинная строка пропускается целиком, следующая разбирается.
        let mut scan = InitScan::default();
        let long = format!("{{\"x\":\"{}\"}}", "z".repeat(MAX_INIT_LINE));
        assert_eq!(scan.feed(long.as_bytes()), None);
        assert_eq!(scan.feed(b"\n"), None);
        assert_eq!(
            scan.feed(format!("{INIT}\n").as_bytes()).map(|_| ()),
            Some(())
        );
        assert_eq!(
            foreign_tools(&[
                "mcp__glafira-pg__list_connections".into(),
                "run_shell_command".into(),
                "mcp__other__x".into(),
                "mcp__glafira-pg".into(),
            ]),
            ["run_shell_command", "mcp__other__x", "mcp__glafira-pg"]
        );
        assert!(foreign_tools(&["mcp__glafira-pg__explain_query".into()]).is_empty());
    }

    #[tokio::test]
    async fn a_missing_program_is_not_found_and_the_log_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("a.log");
        let spec = CommandSpec {
            program: dir.path().join("missing").join("qwen"),
            args: vec![],
            env_set: vec![],
            env_remove: vec![],
            cwd: dir.path().to_path_buf(),
            output: OutputFormat::StreamJson,
        };
        let e = spawner()
            .spawn(&spec, String::new(), log.clone())
            .err()
            .unwrap();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("spawn failed")
        );
    }

    #[cfg(unix)]
    fn sh(dir: &Path, script: &str, env: &[(&str, String)]) -> CommandSpec {
        CommandSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env_set: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            env_remove: vec![],
            cwd: dir.to_path_buf(),
            output: OutputFormat::Text,
        }
    }

    /// Процесс есть и он не зомби (убитый, но ещё не прибранный init — уже не живой).
    #[cfg(unix)]
    fn alive(pid: i32) -> bool {
        // SAFETY: сигнал 0 только проверяет, что процесс есть.
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| {
                s.rsplit_once(')')
                    .map(|(_, rest)| rest.trim_start().starts_with('Z'))
            })
            .unwrap_or(false);
        exists && !zombie
    }

    #[cfg(unix)]
    async fn pid_from(file: &Path) -> i32 {
        for _ in 0..300 {
            if let Ok(s) = std::fs::read_to_string(file)
                && let Ok(pid) = s.trim().parse()
            {
                return pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("нет файла {}", file.display());
    }

    #[cfg(unix)]
    async fn until_dead(pid: i32) -> bool {
        for _ in 0..300 {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_goes_to_the_log_with_marks_and_the_tail_keeps_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("agent-runs").join("a000001.log");
        let spec = sh(
            dir.path(),
            "cat; echo 'to stderr' >&2; printf 'last line'; exit 3",
            &[],
        );
        let mut p = spawner()
            .spawn(&spec, "prompt text\n".into(), log.clone())
            .unwrap();
        let exit = p.wait().await;
        assert_eq!(
            exit,
            ExitInfo {
                code: Some(3),
                stdout_tail: "prompt text\nlast line".into()
            }
        );
        assert_eq!(p.wait().await, exit, "повторное ожидание — тот же итог");
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.starts_with("# /bin/sh"), "{text}");
        assert!(text.contains(" out] prompt text\n"), "{text}");
        assert!(text.contains(" err] to stderr\n"), "{text}");
        assert!(text.contains("# exit code: 3"), "{text}");
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(log.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    /// «Показать журнал» у идущего запуска читает файл (спека §4.5): порция вывода видна в журнале сразу, а не в
    /// конце запуска.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_log_is_written_while_the_agent_runs() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("a.log");
        let spec = sh(dir.path(), "echo 'first chunk'; sleep 300", &[]);
        let mut p = spawner().spawn(&spec, String::new(), log.clone()).unwrap();
        let mut seen = false;
        for _ in 0..300 {
            if std::fs::read_to_string(&log).is_ok_and(|t| t.contains(" out] first chunk\n")) {
                seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            seen,
            "журнал: {:?}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(50), p.wait())
                .await
                .is_err(),
            "процесс ещё работает"
        );
        p.stop().await;
        assert_eq!(p.wait().await.code, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_tools_come_from_the_init_event_before_the_first_turn() {
        let dir = tempfile::tempdir().unwrap();
        // Скрипт печатает строки `$LINES` и работает дальше: ответ о списке приходит, пока процесс жив.
        let live = |name: &str, lines: &[&str]| {
            let mut spec = sh(
                dir.path(),
                "printf '%s\\n' \"$LINE1\" \"$LINE2\"; sleep 300",
                &[
                    ("LINE1", lines[0].to_string()),
                    ("LINE2", lines.get(1).copied().unwrap_or("").to_string()),
                ],
            );
            spec.output = OutputFormat::StreamJson;
            spawner()
                .spawn(&spec, String::new(), dir.path().join(name))
                .unwrap()
        };
        let mut p = live(
            "a.log",
            &[
                r#"{"type":"system","subtype":"hook_started"}"#,
                r#"{"type":"system","subtype":"init","tools":["mcp__glafira-pg__list_connections","run_shell_command"]}"#,
            ],
        );
        let tools = tokio::time::timeout(Duration::from_secs(5), p.init_tools())
            .await
            .expect("init пришёл, процесс работает дальше");
        assert_eq!(
            tools,
            InitTools::Listed(vec![
                "mcp__glafira-pg__list_connections".to_string(),
                "run_shell_command".to_string()
            ])
        );
        assert_eq!(
            p.init_tools().await,
            tools,
            "повторный вызов — тот же список"
        );
        p.kill().await;
        let exit = p.wait().await;
        assert!(
            exit.stdout_tail.contains("\"subtype\":\"init\""),
            "строки init остаются в выводе"
        );
        // Событие `init` без списка, строка не JSON и ход модели до него — ответ сразу, процесс ещё работает.
        for (name, lines, want) in [
            (
                "b.log",
                [r#"{"type":"system","subtype":"init","model":"m"}"#, ""],
                InitTools::NoList,
            ),
            (
                "c.log",
                [
                    "Loading settings…",
                    r#"{"type":"system","subtype":"init","tools":[]}"#,
                ],
                InitTools::NotStreamJson,
            ),
            (
                "f.log",
                [
                    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking"}]}}"#,
                    r#"{"type":"system","subtype":"init","tools":[]}"#,
                ],
                InitTools::NoList,
            ),
        ] {
            let mut p = live(name, &lines);
            let got = tokio::time::timeout(Duration::from_secs(5), p.init_tools())
                .await
                .expect("ответ — до конца процесса");
            assert_eq!(got, want, "{name}");
            p.kill().await;
            assert_eq!(p.wait().await.code, None);
        }
        // Поток кончился без события init.
        let mut spec = sh(
            dir.path(),
            "echo '{\"type\":\"system\",\"subtype\":\"hook_started\"}'",
            &[],
        );
        spec.output = OutputFormat::StreamJson;
        let mut p = spawner()
            .spawn(&spec, String::new(), dir.path().join("d.log"))
            .unwrap();
        assert_eq!(p.init_tools().await, InitTools::Ended);
        assert_eq!(p.wait().await.code, Some(0));
        // Простой текст — проверить нечем, ответ сразу.
        let mut p = spawner()
            .spawn(
                &sh(
                    dir.path(),
                    "echo \"$INIT\"; sleep 300",
                    &[("INIT", INIT.into())],
                ),
                String::new(),
                dir.path().join("e.log"),
            )
            .unwrap();
        assert_eq!(p.init_tools().await, InitTools::NotStreamJson);
        p.kill().await;
    }

    /// Проверка инструментов не прошла: группа останавливается сразу, без паузы SIGTERM → SIGKILL (§4.1).
    #[cfg(unix)]
    #[tokio::test]
    async fn kill_stops_the_whole_group_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        // TERM игнорируют и оболочка, и её потомок: обычная остановка ждала бы паузу (здесь — 5 с).
        let spec = sh(
            dir.path(),
            "trap '' TERM; sleep 300 & echo $! > \"$PIDFILE\"; wait",
            &[("PIDFILE", pidfile.display().to_string())],
        );
        let mut p = TokioSpawner::with_env(Arc::new(ShellEnv::current_process()))
            .spawn(&spec, String::new(), dir.path().join("a.log"))
            .unwrap();
        let child = pid_from(&pidfile).await;
        let started = std::time::Instant::now();
        p.kill().await;
        let exit = tokio::time::timeout(Duration::from_secs(3), p.wait())
            .await
            .expect("wait после kill не ждёт паузу");
        assert_eq!(exit.code, None);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        assert!(until_dead(child).await, "потомок группы остановлен");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_terminates_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        let spec = sh(
            dir.path(),
            "sleep 300 & echo $! > \"$PIDFILE\"; wait",
            &[("PIDFILE", pidfile.display().to_string())],
        );
        let mut p = spawner()
            .spawn(&spec, String::new(), dir.path().join("a.log"))
            .unwrap();
        let child = pid_from(&pidfile).await;
        assert!(alive(child));
        let started = std::time::Instant::now();
        p.stop().await;
        let exit = p.wait().await;
        assert_eq!(exit.code, None, "убит сигналом");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(
            until_dead(child).await,
            "дочерний процесс группы остановлен"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_that_ignores_sigterm_is_killed_after_the_grace() {
        let dir = tempfile::tempdir().unwrap();
        // Игнорирование TERM наследуют и дочерние `sleep`.
        let spec = sh(dir.path(), "trap '' TERM; while :; do sleep 1; done", &[]);
        let mut p = spawner()
            .spawn(&spec, String::new(), dir.path().join("a.log"))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let started = std::time::Instant::now();
        p.stop().await;
        let exit = p.wait().await;
        assert_eq!(exit.code, None);
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(300) && took < Duration::from_secs(3),
            "{took:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn leftovers_of_a_finished_leader_are_killed_and_wait_returns() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("child.pid");
        // Фоновый процесс держит stdout: без остановки группы чтение вывода ждало бы его 300 с.
        let spec = sh(
            dir.path(),
            "sleep 300 & echo $! > \"$PIDFILE\"; echo done",
            &[("PIDFILE", pidfile.display().to_string())],
        );
        let mut p = spawner()
            .spawn(&spec, String::new(), dir.path().join("a.log"))
            .unwrap();
        let exit = tokio::time::timeout(Duration::from_secs(5), p.wait())
            .await
            .expect("wait не зависает");
        assert_eq!(
            exit,
            ExitInfo {
                code: Some(0),
                stdout_tail: "done\n".into()
            }
        );
        assert!(until_dead(pid_from(&pidfile).await).await);
    }

    /// Очередь ждёт конца процесса со сроком (поток кончился без `init`): прерванное ожидание не теряет хвост stdout.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_interrupted_wait_keeps_the_exit_and_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let spec = sh(
            dir.path(),
            "printf 'tail of stdout'; echo 'to stderr' >&2; sleep 0.2; exit 4",
            &[],
        );
        let mut p = spawner()
            .spawn(&spec, String::new(), dir.path().join("a.log"))
            .unwrap();
        let mut interrupted = 0;
        let exit = loop {
            match tokio::time::timeout(Duration::from_micros(200), p.wait()).await {
                Ok(exit) => break exit,
                Err(_) => interrupted += 1,
            }
        };
        assert!(interrupted > 0);
        assert_eq!(
            exit,
            ExitInfo {
                code: Some(4),
                stdout_tail: "tail of stdout".into()
            }
        );
        assert_eq!(p.wait().await, exit);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_flood_is_cut_at_one_megabyte_and_the_tail_survives() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("a.log");
        let spec = sh(
            dir.path(),
            "head -c 3000000 /dev/zero | tr '\\000' 'x'; echo; echo tail-marker",
            &[],
        );
        let mut p = spawner().spawn(&spec, String::new(), log.clone()).unwrap();
        let exit = p.wait().await;
        assert_eq!(exit.code, Some(0));
        assert!(exit.stdout_tail.ends_with("\ntail-marker\n"));
        assert!(exit.stdout_tail.len() <= STDOUT_TAIL_BYTES);
        let size = std::fs::metadata(&log).unwrap().len() as usize;
        assert!(size <= MAX_LOG_BYTES + 512, "{size}");
        let text = String::from_utf8_lossy(&std::fs::read(&log).unwrap()).into_owned();
        assert!(text.contains("…[truncated]"));
        assert!(text.contains("bytes not written"));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_runs_without_a_window_and_stops_the_job() {
        let dir = tempfile::tempdir().unwrap();
        let cmd =
            PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
                .join(r"System32\cmd.exe");
        let spec = |line: &str| CommandSpec {
            program: cmd.clone(),
            args: vec!["/C".into(), line.into()],
            env_set: vec![],
            env_remove: vec![],
            cwd: dir.path().to_path_buf(),
            output: OutputFormat::Text,
        };
        let mut p = spawner()
            .spawn(
                &spec("echo out& exit /b 3"),
                String::new(),
                dir.path().join("a.log"),
            )
            .unwrap();
        let exit = p.wait().await;
        assert_eq!(exit.code, Some(3));
        assert!(exit.stdout_tail.contains("out"));
        let mut p = spawner()
            .spawn(
                &spec("ping -n 300 127.0.0.1 >NUL"),
                String::new(),
                dir.path().join("b.log"),
            )
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        p.stop().await;
        let exit = tokio::time::timeout(Duration::from_secs(5), p.wait())
            .await
            .expect("Job Object остановлен");
        assert_ne!(exit.code, Some(0));
    }
}
