//! Режим `glafira-pg mcp` (спец. P4 §3.3): тот же бинарь без Tauri и окна. Находит запущенную Glafira по `mcp.json`
//! (при необходимости запускает её), проходит рукопожатие и перекачивает байты между stdin/stdout клиента MCP и
//! сокетом (named pipe) приложения. В stdout — только протокол MCP, диагностика — в stderr.
//! P4c §3.3: шим фонового запуска (переменная `GLAFIRA_AGENT_TICKET`) передаёт билет в рукопожатии и Glafira не
//! запускает.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use glafira_mcp_core::endpoint::{self, BoxStream};
use glafira_mcp_core::handshake::{self, HandshakeError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Сколько шим ждёт, пока Glafira запустит MCP-сервер: у Qwen Code таймаут рукопожатия stdio — 30 с.
pub const WAIT: Duration = Duration::from_secs(25);
const POLL: Duration = Duration::from_millis(250);
/// Переменная окружения с билетом фонового запуска (спец. P4c §3.3): её ставит `env` MCP-конфига запуска.
pub const AGENT_TICKET_ENV: &str = "GLAFIRA_AGENT_TICKET";

/// Окружение шима; в тестах подменяется.
pub trait ShimEnv {
    /// Каталог данных Glafira (`mcp.json`, `settings.json`).
    fn data_dir(&self) -> Option<PathBuf>;
    /// Политика администратора разрешает MCP.
    fn policy_allows(&self) -> bool;
    /// Запускает Glafira отвязанным процессом.
    fn launch_app(&self) -> io::Result<()>;
    /// Билет фонового запуска (`GLAFIRA_AGENT_TICKET`, без пробелов по краям); пустая или не заданная переменная —
    /// `None`. Форму проверяет `connect`.
    fn agent_ticket(&self) -> Option<String> {
        None
    }
}

#[derive(Debug)]
pub enum ShimError {
    NoDataDir,
    DisabledByPolicy,
    DisabledInSettings,
    Launch(io::Error),
    NotStarted,
    Handshake(HandshakeError),
    /// `GLAFIRA_AGENT_TICKET` задан, но это не билет (P4c §3.3).
    BadTicket,
    /// Шим фонового запуска не нашёл работающую Glafira: запуска уже нет.
    RunOver,
}

impl fmt::Display for ShimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDataDir => write!(f, "Cannot find the Glafira data directory"),
            Self::DisabledByPolicy => write!(f, "MCP is disabled by your organization"),
            Self::DisabledInSettings => write!(
                f,
                "MCP is disabled in Glafira: turn it on in Settings → LLM assistants (MCP)"
            ),
            Self::Launch(e) => write!(f, "Cannot start Glafira: {e}"),
            Self::NotStarted => write!(
                f,
                "Glafira did not start the MCP server within {} s",
                WAIT.as_secs()
            ),
            Self::Handshake(e) => write!(f, "{e}"),
            Self::BadTicket => write!(
                f,
                "{AGENT_TICKET_ENV} is not a ticket of a background run: start the run again in Glafira"
            ),
            Self::RunOver => write!(f, "{}", handshake::BAD_TICKET_MESSAGE),
        }
    }
}

/// Проверки по порядку спец. P4 §3.3 (каталог данных, политика, выключатель) и форма билета фонового запуска (P4c).
/// Затем — соединение с запущенной Glafira. Нет `mcp.json` или соединение не удаётся — Glafira запускается один раз,
/// и шим пробует снова каждые 250 мс до `wait`. Шим фонового запуска Glafira не запускает: запуск живёт только в
/// работающей Glafira (`RunOver`). Отказ рукопожатия (`ERR bad_token …`, `ERR bad_ticket …`) — ошибка сразу, без
/// повторов.
pub async fn connect(env: &dyn ShimEnv, wait: Duration) -> Result<BoxStream, ShimError> {
    let dir = env.data_dir().ok_or(ShimError::NoDataDir)?;
    if !env.policy_allows() {
        return Err(ShimError::DisabledByPolicy);
    }
    if !crate::settings::mcp_enabled_in(&dir) {
        return Err(ShimError::DisabledInSettings);
    }
    // Билет не той формы — отказ, а не обычная сессия: иначе фоновый агент получил бы все инструменты клиента,
    // которого раньше подтвердили в чате.
    let ticket = env.agent_ticket();
    if ticket
        .as_deref()
        .is_some_and(|t| !handshake::valid_ticket(t))
    {
        return Err(ShimError::BadTicket);
    }
    let deadline = tokio::time::Instant::now() + wait;
    let mut launched = false;
    loop {
        if let Some(f) = super::files::read_mcp_file(&dir)
            && let Ok(mut s) = endpoint::connect(&f.endpoint).await
        {
            handshake::client_with_run(&mut s, &f.token, ticket.as_deref())
                .await
                .map_err(ShimError::Handshake)?;
            return Ok(s);
        }
        if ticket.is_some() {
            return Err(ShimError::RunOver);
        }
        if !launched {
            env.launch_app().map_err(ShimError::Launch)?;
            launched = true;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(ShimError::NotStarted);
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Копирует до конца потока, сбрасывая буфер после каждой порции: клиент MCP ждёт ответ построчно.
async fn copy_flushing<R, W>(mut r: R, mut w: W) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            let _ = w.shutdown().await;
            return Ok(());
        }
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
    }
}

/// Перекачивает байты в обе стороны, пока одна из сторон не закончится.
pub async fn pump<I, O>(stream: BoxStream, input: I, output: O)
where
    I: AsyncRead + Unpin,
    O: AsyncWrite + Unpin,
{
    let (mut sr, mut sw) = tokio::io::split(stream);
    tokio::select! {
        _ = copy_flushing(input, &mut sw) => {}
        _ = copy_flushing(&mut sr, output) => {}
    }
}

/// `…/X.app/Contents/MacOS/<бинарь>` → `…/X.app`.
pub fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let ok = macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    ok.then(|| bundle.to_path_buf())
}

/// Запуск Glafira отдельным процессом, не наследующим stdio шима: иначе он держал бы pipe клиента, и тот не увидел
/// бы конец потока. macOS — `open -a <бандл>`; Windows — `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`; Linux —
/// `$APPIMAGE` или сам бинарь в своей группе процессов.
fn launch(exe: &Path) -> io::Result<()> {
    if cfg!(target_os = "macos")
        && let Some(bundle) = app_bundle(exe)
    {
        Command::new("open")
            .arg("-a")
            .arg(bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        return Ok(());
    }
    // В AppImage `current_exe()` — во временной точке монтирования, которая исчезнет вместе с шимом.
    let target = super::launch_path(exe, std::env::var_os("APPIMAGE"));
    let mut cmd = Command::new(target);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}

struct RealEnv;

impl ShimEnv for RealEnv {
    fn data_dir(&self) -> Option<PathBuf> {
        super::files::data_dir()
    }

    fn policy_allows(&self) -> bool {
        crate::policy::sources::load().status.mcp_allowed()
    }

    fn agent_ticket(&self) -> Option<String> {
        // `var_os`: значение не в UTF-8 — тоже «задано», и `connect` отвергнет его как не билет.
        std::env::var_os(AGENT_TICKET_ENV)
            .map(|v| v.to_string_lossy().trim().to_string())
            .filter(|v| !v.is_empty())
    }

    fn launch_app(&self) -> io::Result<()> {
        // Сборка разработки без dev-сервера Vite открыла бы пустое окно: её запускают вручную (`pnpm tauri dev`).
        if cfg!(debug_assertions) {
            eprintln!(
                "glafira-pg mcp: development build: start Glafira with `pnpm tauri dev`, waiting for it"
            );
            return Ok(());
        }
        launch(&std::env::current_exe()?)
    }
}

/// Точка входа режима `mcp` (из `main` через `glafira_pg_lib::mcp_main`). Код возврата процесса: 0 — клиент или
/// приложение закрыли поток, 1 — ошибка (текст в stderr).
pub fn run() -> i32 {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("glafira-pg mcp: {e}");
            return 1;
        }
    };
    let code = rt.block_on(async {
        match connect(&RealEnv, WAIT).await {
            Ok(stream) => {
                pump(stream, tokio::io::stdin(), tokio::io::stdout()).await;
                0
            }
            Err(e) => {
                eprintln!("glafira-pg mcp: {e}");
                1
            }
        }
    });
    // Чтение stdin в tokio занимает блокирующий поток: не ждём его при выходе.
    rt.shutdown_background();
    code
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU32, Ordering};

    use glafira_mcp_core::endpoint::{Endpoint, Listener};

    use super::*;
    use crate::mcp::files::{self, McpFile};
    use crate::settings::SettingsStore;

    struct FakeEnv {
        dir: Option<PathBuf>,
        policy: bool,
        launches: AtomicU32,
        on_launch: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        ticket: Option<String>,
    }

    impl ShimEnv for FakeEnv {
        fn data_dir(&self) -> Option<PathBuf> {
            self.dir.clone()
        }
        fn policy_allows(&self) -> bool {
            self.policy
        }
        fn launch_app(&self) -> io::Result<()> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            if let Some(f) = self.on_launch.lock().unwrap().take() {
                f();
            }
            Ok(())
        }
        fn agent_ticket(&self) -> Option<String> {
            self.ticket.clone()
        }
    }

    fn env(dir: &Path, policy: bool, mcp_enabled: bool) -> FakeEnv {
        SettingsStore::open(dir)
            .set_mcp_enabled(mcp_enabled)
            .unwrap();
        FakeEnv {
            dir: Some(dir.to_path_buf()),
            policy,
            launches: AtomicU32::new(0),
            on_launch: Mutex::new(None),
            ticket: None,
        }
    }

    fn no_runs(_: &str) -> Option<String> {
        None
    }

    fn ticket() -> String {
        "ab".repeat(32)
    }

    /// Как запущенная Glafira: слушает endpoint и пишет `mcp.json` с `token_in_file`.
    async fn listen(dir: &Path, token_in_file: &str) -> Listener {
        let ep = Endpoint::for_app(dir, "shimtest");
        let l = Listener::bind(&ep).await.unwrap();
        files::write_mcp_file(
            dir,
            &McpFile {
                version: files::VERSION,
                endpoint: ep,
                token: token_in_file.into(),
                pid: 1,
            },
        )
        .unwrap();
        l
    }

    /// Принимает шим: рукопожатие со своим `server_token` и, если оно удалось, `from-app`.
    async fn serve_one(mut l: Listener, server_token: &'static str) {
        let mut s = l.accept().await.unwrap();
        if handshake::server(&mut s, server_token, &no_runs)
            .await
            .is_ok()
        {
            s.write_all(b"from-app").await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    #[tokio::test]
    async fn checks_run_in_order_and_disabled_mcp_does_not_launch_glafira() {
        let e = FakeEnv {
            dir: None,
            policy: true,
            launches: AtomicU32::new(0),
            on_launch: Mutex::new(None),
            ticket: None,
        };
        assert!(matches!(connect(&e, WAIT).await, Err(ShimError::NoDataDir)));

        let dir = tempfile::tempdir().unwrap();
        // Политика проверяется раньше настройки: MCP включён в «Настройках», но запрещён организацией.
        let e = env(dir.path(), false, true);
        assert!(matches!(
            connect(&e, WAIT).await,
            Err(ShimError::DisabledByPolicy)
        ));
        let e = env(dir.path(), true, false);
        assert!(matches!(
            connect(&e, WAIT).await,
            Err(ShimError::DisabledInSettings)
        ));
        assert_eq!(
            e.launches.load(Ordering::SeqCst),
            0,
            "выключенный MCP не запускает Glafira"
        );
    }

    #[test]
    fn messages_match_the_spec() {
        assert_eq!(
            ShimError::DisabledByPolicy.to_string(),
            "MCP is disabled by your organization"
        );
        assert_eq!(
            ShimError::DisabledInSettings.to_string(),
            "MCP is disabled in Glafira: turn it on in Settings → LLM assistants (MCP)"
        );
        assert_eq!(
            ShimError::NotStarted.to_string(),
            "Glafira did not start the MCP server within 25 s"
        );
        assert_eq!(WAIT, Duration::from_secs(25));
        assert_eq!(
            ShimError::RunOver.to_string(),
            "The background run is over: start it again in Glafira"
        );
        assert_eq!(
            ShimError::BadTicket.to_string(),
            "GLAFIRA_AGENT_TICKET is not a ticket of a background run: start the run again in Glafira"
        );
    }

    #[tokio::test]
    async fn launches_glafira_once_and_waits_for_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(dir.path(), true, true);
        let app_dir = dir.path().to_path_buf();
        *e.on_launch.lock().unwrap() = Some(Box::new(move || {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let l = listen(&app_dir, "tok").await;
                serve_one(l, "tok").await;
            });
        }));
        let mut s = connect(&e, Duration::from_secs(5)).await.unwrap();
        assert_eq!(e.launches.load(Ordering::SeqCst), 1, "запуск — один раз");
        let mut buf = [0u8; 8];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"from-app");
    }

    #[tokio::test]
    async fn running_glafira_is_used_without_launching() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(dir.path(), true, true);
        // Сервер и `mcp.json` готовы до шима: запускать нечего.
        let l = listen(dir.path(), "tok").await;
        tokio::spawn(serve_one(l, "tok"));
        let mut s = connect(&e, Duration::from_secs(5)).await.unwrap();
        let mut buf = [0u8; 8];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"from-app");
        assert_eq!(e.launches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rejected_token_is_an_error_not_a_retry() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(dir.path(), true, true);
        // В файле — один токен, сервер ждёт другой (файл чужого запуска).
        let l = listen(dir.path(), "tok").await;
        tokio::spawn(serve_one(l, "other"));
        let r = connect(&e, Duration::from_secs(5)).await;
        assert!(matches!(r, Err(ShimError::Handshake(_))), "{:?}", r.err());
        assert_eq!(e.launches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn gives_up_when_glafira_does_not_start() {
        let dir = tempfile::tempdir().unwrap();
        let e = env(dir.path(), true, true);
        let r = connect(&e, Duration::from_millis(600)).await;
        assert!(matches!(r, Err(ShimError::NotStarted)));
        assert_eq!(e.launches.load(Ordering::SeqCst), 1);
    }

    /// Шим фонового запуска (P4c §3.3): билет — в строке рукопожатия, приложение узнаёт запуск.
    #[tokio::test]
    async fn agent_ticket_goes_into_the_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path(), true, true);
        e.ticket = Some(ticket());
        let mut l = listen(dir.path(), "tok").await;
        let app = tokio::spawn(async move {
            let mut s = l.accept().await.unwrap();
            let check = |t: &str| (t == ticket()).then(|| "a1b2c3d".to_string());
            let run = handshake::server(&mut s, "tok", &check).await.unwrap();
            s.write_all(b"from-app").await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            run
        });
        let mut s = connect(&e, Duration::from_secs(5)).await.unwrap();
        let mut buf = [0u8; 8];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"from-app");
        assert_eq!(app.await.unwrap().as_deref(), Some("a1b2c3d"));
    }

    /// Билет не той формы — отказ сразу: обычной сессией с полным набором инструментов шим запуска не становится.
    #[tokio::test]
    async fn malformed_ticket_is_an_error_before_connecting() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path(), true, true);
        e.ticket = Some("not-a-ticket run=x".into());
        assert!(matches!(connect(&e, WAIT).await, Err(ShimError::BadTicket)));
        assert_eq!(e.launches.load(Ordering::SeqCst), 0);
    }

    /// Фоновый запуск живёт только в работающей Glafira: шим запуска её не запускает и не ждёт.
    #[tokio::test]
    async fn background_run_does_not_launch_glafira() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path(), true, true);
        e.ticket = Some(ticket());
        assert!(matches!(connect(&e, WAIT).await, Err(ShimError::RunOver)));
        assert_eq!(e.launches.load(Ordering::SeqCst), 0);
    }

    /// Отказ `bad_ticket` приложения — ошибка шима с текстом сервера, без повторов.
    #[tokio::test]
    async fn bad_ticket_from_the_app_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = env(dir.path(), true, true);
        e.ticket = Some(ticket());
        let l = listen(dir.path(), "tok").await;
        tokio::spawn(serve_one(l, "tok"));
        let Err(err) = connect(&e, Duration::from_secs(5)).await else {
            panic!("ожидался отказ рукопожатия");
        };
        assert!(
            matches!(&err, ShimError::Handshake(HandshakeError::Rejected { code, .. }) if code == "bad_ticket"),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "The background run is over: start it again in Glafira"
        );
        assert_eq!(e.launches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pump_carries_bytes_both_ways() {
        let (app_side, shim_side) = tokio::io::duplex(1024);
        let (mut stdin_writer, stdin) = tokio::io::duplex(1024);
        let app = tokio::spawn(async move {
            let mut app_side = app_side;
            let mut buf = [0u8; 5];
            app_side.read_exact(&mut buf).await.unwrap();
            app_side.write_all(b"HELLO").await.unwrap();
            buf
        });
        stdin_writer.write_all(b"hello").await.unwrap();
        let mut out = Vec::new();
        pump(Box::new(shim_side), stdin, &mut out).await;
        assert_eq!(&app.await.unwrap(), b"hello");
        assert_eq!(out, b"HELLO");
    }

    #[test]
    fn app_bundle_is_found_only_inside_a_bundle() {
        assert_eq!(
            app_bundle(Path::new(
                "/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg"
            )),
            Some(PathBuf::from("/Applications/Glafira for PostgreSQL.app"))
        );
        assert_eq!(app_bundle(Path::new("/usr/bin/glafira-pg")), None);
        assert_eq!(
            app_bundle(Path::new("/opt/Glafira.app/Contents/Resources/glafira-pg")),
            None
        );
    }
}
