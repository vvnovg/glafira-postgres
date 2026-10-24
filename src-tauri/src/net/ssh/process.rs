//! Процесс `ssh` (OpenSSH): поиск клиента, аргументы, запуск с askpass, готовность, разбор ошибок (спец. P1 §5).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

use crate::conn::profile::{SshAuth, SshSettings, validate_ssh};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sync::MutexExt;

/// Сколько последних строк настоящего stderr идёт в хвост ошибки (`details`).
const STDERR_LINES: usize = 10;
/// Сколько последних строк лога `-E` этого запуска идёт в хвост ошибки. У каждого источника свой запас: длинный
/// баннер сервера (ssh печатает его в stderr) не должен вытеснять итоговую ошибку из лога.
const LOG_LINES: usize = 20;
/// Сколько байт конца лога `-E` читается для хвоста ошибки: за долгую жизнь туннеля лог растёт.
const LOG_TAIL_BYTES: u64 = 64 * 1024;
/// Ожидание `ssh -V`.
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);
/// Одна проба порта SOCKS при ожидании готовности. Короткая, потому что на Windows соединение с портом loopback,
/// который ещё никто не слушает, отклоняется не сразу, а через ~2 с (стек повторяет SYN после RST); на Linux и
/// macOS — за микросекунды. Проба, начатая до открытия порта, бросается по сроку, и следующая (через
/// `PROBE_PAUSE`) уже застаёт слушателя: готовность видна в пределах ~300 мс после открытия порта на любой ОС, а
/// выход процесса замечается не позже чем через одну пробу.
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);
/// Пауза между пробами порта SOCKS.
const PROBE_PAUSE: Duration = Duration::from_millis(50);
/// Дочитать stderr после выхода процесса: последние строки (причина выхода) нужны для разбора ошибки.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Клиент OpenSSH в PATH; на Windows — ещё и штатное место компонента «Клиент OpenSSH». Проверяет файлы на
/// диске: вызывать из `spawn_blocking`.
pub fn find_ssh() -> AppResult<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        dirs.push(PathBuf::from(root).join("System32").join("OpenSSH"));
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| ssh_error(SshErrorKind::NotFound, "", ""))
}

/// Версия OpenSSH из `ssh -V` (пишется в stderr): `(major, minor)`.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.split("OpenSSH_").nth(1)?;
    let rest = rest.trim_start_matches("for_Windows_");
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// `SSH_ASKPASS_REQUIRE=force` появился в OpenSSH 8.4: без него пароль и пассфраза не спрашиваются через askpass.
pub fn askpass_supported(v: (u32, u32)) -> bool {
    v >= (8, 4)
}

/// Версия клиента из `ssh -V`. Не запустился, не ответил за 5 с или ответил не как OpenSSH — `notFound`
/// с причиной в `details`: пригодного клиента OpenSSH нет.
pub async fn ssh_version(bin: &Path) -> AppResult<(u32, u32)> {
    ssh_version_with_env(bin, &[]).await
}

/// То же с дополнительным окружением (как `SshArgs::extra_env`: тесты управляют поддельным ssh).
pub async fn ssh_version_with_env(
    bin: &Path,
    extra_env: &[(String, String)],
) -> AppResult<(u32, u32)> {
    let mut cmd = Command::new(bin);
    cmd.envs(extra_env.iter().map(|(k, v)| (k, v)));
    cmd.arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_window(&mut cmd);
    let not_openssh = |why: &str| {
        ssh_error(
            SshErrorKind::NotFound,
            "",
            &format!("{} -V: {why}", bin.display()),
        )
    };
    let out = tokio::time::timeout(VERSION_TIMEOUT, cmd.output())
        .await
        .map_err(|_| not_openssh("timed out"))?
        .map_err(|e| not_openssh(&e.to_string()))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    parse_version(&text).ok_or_else(|| not_openssh(text.trim()))
}

/// Свободный локальный порт: слушатель на `127.0.0.1:0` открывается и закрывается; при гонке
/// (`ExitOnForwardFailure` на занятом порту) супервизор делает одну повторную попытку с новым портом.
pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

#[derive(Debug, Clone)]
pub struct SshArgs {
    pub bin: PathBuf,
    /// Лог `ssh -E`: туда ssh пишет и свои ошибки (не в stderr), его путь в командной строке опознаёт сирот.
    pub log_file: PathBuf,
    pub socks_port: u16,
    pub settings: SshSettings,
    /// Только для тестов (`GLAFIRA_SSH_KNOWN_HOSTS`): свой known_hosts, чтобы не трогать файл пользователя.
    pub known_hosts: Option<PathBuf>,
    /// Дополнительное окружение процесса (тесты управляют поддельным ssh). В командную строку не попадает;
    /// переменные askpass приложения главнее.
    pub extra_env: Vec<(String, String)>,
}

impl SshArgs {
    /// Аргументы без имени программы. Значения проверены `validate_ssh` (без пробелов и ведущего «-»; `spawn`
    /// проверяет их ещё раз), цель идёт после `--`.
    pub fn command_line(&self) -> Vec<OsString> {
        let s = &self.settings;
        let mut a: Vec<OsString> = vec![
            "-N".into(),
            "-T".into(),
            "-E".into(),
            self.log_file.clone().into(),
        ];
        // `ControlMaster=no` и `ControlPath=none` изолируют туннель от разделения соединений в `~/.ssh/config`
        // (`ControlMaster auto` + `ControlPath`). Иначе `ssh -N -D` мог бы отдать проброс уже открытому общему
        // сеансу: порт SOCKS пережил бы приложение как прокси без аутентификации, а ключ хоста и вход не
        // проверялись бы вовсе. Или наш процесс сам стал бы мастером, и чужие сеансы пользователя умерли бы
        // вместе с ним. Опции командной строки главнее файла настроек.
        for o in [
            "ExitOnForwardFailure=yes",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "StrictHostKeyChecking=ask",
            "NumberOfPasswordPrompts=1",
            "BatchMode=no",
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            a.push("-o".into());
            a.push(o.into());
        }
        if let Some(kh) = &self.known_hosts {
            a.push("-o".into());
            let mut v = OsString::from("UserKnownHostsFile=");
            v.push(kh);
            a.push(v);
        }
        if let Some(p) = s.port {
            a.push("-p".into());
            a.push(p.to_string().into());
        }
        if let Some(u) = &s.user {
            a.push("-l".into());
            a.push(u.into());
        }
        match s.auth {
            SshAuth::Key => {
                if let Some(k) = &s.key_path {
                    a.push("-i".into());
                    a.push(k.into());
                    a.push("-o".into());
                    a.push("IdentitiesOnly=yes".into());
                }
                // Только ключ: без этого после отказа по ключу ssh перешёл бы к keyboard-interactive, где
                // текст запроса задаёт сервер, и askpass мог бы отдать пассфразу в ответ на чужой вопрос.
                a.push("-o".into());
                a.push("PreferredAuthentications=publickey".into());
            }
            SshAuth::Password => {
                a.push("-o".into());
                a.push("PreferredAuthentications=password".into());
            }
            SshAuth::Agent => {}
        }
        a.push("-D".into());
        a.push(format!("127.0.0.1:{}", self.socks_port).into());
        a.push("--".into());
        a.push(s.target.clone().into());
        a
    }
}

/// Окружение askpass (спец. P1 §5.1): секреты отдаёт сам бинарь приложения.
#[derive(Debug, Clone)]
pub struct AskpassEnv {
    pub connection_id: String,
    pub report_file: PathBuf,
    pub trust: Option<String>,
    /// Файл секретов 0600 вместо keychain (`GLAFIRA_ASKPASS_SECRET_FILE`): только тесты, где keychain нет.
    pub secret_file: Option<PathBuf>,
    /// Бинарь для `SSH_ASKPASS` вместо текущего процесса (интеграционные тесты запускаются бинарём libtest).
    pub askpass_bin: Option<PathBuf>,
}

pub struct SshChild {
    child: Child,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// Чтение stderr в `stderr`: после выхода процесса его дожидаются, чтобы хвост был полным.
    reader: Option<JoinHandle<()>>,
    log_file: PathBuf,
    /// Размер лога до запуска: ssh дописывает в конец, строки прошлых запусков в хвост не попадают.
    log_offset: u64,
}

impl SshChild {
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Хвост вывода этого запуска: последние `STDERR_LINES` строк stderr (баннер сервера, вывод ProxyCommand,
    /// то, что ssh успел вывести до открытия лога), затем последние `LOG_LINES` строк лога `-E` (с ним ssh пишет
    /// ошибки туда, а не в stderr; итоговая причина выхода — в конце). Источники не делят запас строк.
    pub fn stderr_tail(&self) -> String {
        let mut lines: Vec<String> = self.stderr.lock_safe().iter().cloned().collect();
        let log = log_lines_since(&self.log_file, self.log_offset);
        lines.extend(log[log.len().saturating_sub(LOG_LINES)..].iter().cloned());
        lines.join("\n")
    }

    /// Завершился ли процесс (без ожидания); `Some(code)` — код выхода (`None` — завершён сигналом).
    pub fn try_exit_code(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    /// Ждёт выхода процесса и дочитывает stderr.
    pub async fn wait(&mut self) -> Option<i32> {
        let code = self.child.wait().await.ok().and_then(|s| s.code());
        self.drain_stderr().await;
        code
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// Сигнал завершения без ожидания (вне async: закрытие туннеля, drop). Завершившийся процесс не трогается.
    pub fn start_kill(&mut self) {
        let _ = self.child.start_kill();
    }

    /// Туннель готов, когда порт SOCKS принимает TCP; процесс завершился раньше — `Err(код выхода)` (причина —
    /// в `stderr_tail`); к `deadline` не готов — процесс завершается, `Err(None)`.
    pub async fn wait_ready(
        &mut self,
        socks: SocketAddr,
        deadline: Instant,
    ) -> Result<(), Option<i32>> {
        loop {
            if let Some(code) = self.try_exit_code() {
                self.drain_stderr().await;
                return Err(code);
            }
            let probe =
                tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(socks)).await;
            if matches!(probe, Ok(Ok(_))) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.kill().await;
                self.drain_stderr().await;
                return Err(None);
            }
            tokio::time::sleep(PROBE_PAUSE).await;
        }
    }

    /// Процесс вышел — stderr закрыт, чтение дойдёт до конца; срок — на случай, если stderr унаследовал
    /// ещё живой потомок ssh.
    async fn drain_stderr(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, reader).await;
        }
    }
}

/// Строки лога, дописанные после `offset` (не больше последних `LOG_TAIL_BYTES`). Лог короче `offset` —
/// его заменили, читается с начала.
fn log_lines_since(path: &Path, offset: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = if len < offset { 0 } else { offset };
    let start = start.max(len.saturating_sub(LOG_TAIL_BYTES));
    if f.seek(SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(LOG_TAIL_BYTES).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// На Windows — без консольного окна (приложение с GUI, окно мелькало бы на каждый запуск).
fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

/// Запуск `ssh`: настройки проверяются ещё раз (профиль с диска мог быть изменён вручную — значение с «-» не
/// должно стать опцией ssh), stdin закрыт (пароли — только через askpass), stderr читается в хвост. Вызывать
/// внутри рантайма tokio.
pub fn spawn(args: &SshArgs, env: &AskpassEnv) -> AppResult<SshChild> {
    validate_ssh(Some(&args.settings))?;
    let target = &args.settings.target;
    let askpass = match &env.askpass_bin {
        Some(bin) => bin.clone(),
        None => std::env::current_exe()
            .map_err(|e| ssh_error(SshErrorKind::Exited, target, &e.to_string()))?,
    };
    let mut cmd = Command::new(&args.bin);
    cmd.args(args.command_line())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Сначала дополнительное окружение: переменные askpass ниже его перекрывают.
        .envs(args.extra_env.iter().map(|(k, v)| (k, v)))
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("GLAFIRA_ASKPASS", &env.connection_id)
        .env("GLAFIRA_ASKPASS_REPORT", &env.report_file);
    if std::env::var_os("DISPLAY").is_none() {
        cmd.env("DISPLAY", ":0");
    }
    match &env.trust {
        Some(fp) => {
            cmd.env("GLAFIRA_ASKPASS_TRUST", fp);
        }
        None => {
            cmd.env_remove("GLAFIRA_ASKPASS_TRUST");
        }
    }
    // Файл секретов — только явно (тесты); унаследованная из окружения приложения переменная не должна увести
    // askpass от keychain.
    match &env.secret_file {
        Some(f) => {
            cmd.env("GLAFIRA_ASKPASS_SECRET_FILE", f);
        }
        None => {
            cmd.env_remove("GLAFIRA_ASKPASS_SECRET_FILE");
        }
    }
    hide_window(&mut cmd);
    let log_offset = std::fs::metadata(&args.log_file)
        .map(|m| m.len())
        .unwrap_or(0);
    let mut child = cmd
        .spawn()
        .map_err(|e| ssh_error(SshErrorKind::Exited, target, &e.to_string()))?;
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
    let reader = child.stderr.take().map(|err| {
        let tail = tail.clone();
        tokio::spawn(async move {
            // Построчно по байтам, а не `lines()`: не-UTF-8 (вывод ProxyCommand, askpass) не должен обрывать чтение,
            // иначе канал закроется и дочерние процессы получат EPIPE/SIGPIPE.
            let mut reader = BufReader::new(err);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let text = String::from_utf8_lossy(&buf);
                let line = text.strip_suffix('\n').unwrap_or(&text);
                let line = line.strip_suffix('\r').unwrap_or(line);
                let mut t = tail.lock_safe();
                if t.len() == STDERR_LINES {
                    t.pop_front();
                }
                t.push_back(line.to_owned());
            }
        })
    });
    Ok(SshChild {
        child,
        stderr: tail,
        reader,
        log_file: args.log_file.clone(),
        log_offset,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshErrorKind {
    NotFound,
    TooOld,
    HostKeyUnknown {
        host: Option<String>,
        fingerprint: Option<String>,
    },
    HostKeyChanged,
    AuthFailed,
    /// Локальный порт SOCKS занят (гонка за портом): супервизор повторяет с новым портом.
    BindFailed,
    /// Бастион не достучался до узла `host:port` (сбой SOCKS после соединения с портом туннеля или обрыв потока сразу
    /// после успеха SOCKS, см. `socks::forward_failed`). Из вывода ssh не выводится: неудачный CONNECT через `-D`
    /// процесс не завершает.
    ForwardFailed {
        host: String,
        port: u16,
    },
    Exited,
}

impl SshErrorKind {
    /// Значение `params.kind` (спец. §5.2).
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "notFound",
            Self::TooOld => "tooOld",
            Self::HostKeyUnknown { .. } => "hostKeyUnknown",
            Self::HostKeyChanged => "hostKeyChanged",
            Self::AuthFailed => "authFailed",
            Self::BindFailed => "bindFailed",
            Self::ForwardFailed { .. } => "forwardFailed",
            Self::Exited => "exited",
        }
    }
}

/// Отчёт askpass о неизвестном ключе хоста: хост в кавычках и отпечаток `SHA256:…`.
pub fn parse_host_key_report(report: &str) -> (Option<String>, Option<String>) {
    let host = report
        .split_once("authenticity of host '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(h, _)| h.to_string());
    let fingerprint = report
        .split_whitespace()
        .find(|w| w.starts_with("SHA256:"))
        .map(|w| w.trim_end_matches('.').to_string());
    (host, fingerprint)
}

/// Вид ошибки по выводу ssh (`stderr_tail`) и отчёту askpass; код выхода у OpenSSH на все ошибки один (255),
/// поэтому решает текст. Неудачные CONNECT через `-D` («channel N: open failed») процесс не завершают, а в логе
/// остаются: процесс, завершившийся после них, завершился по другой причине — `exited` (обрыв или неизвестное),
/// а не сбой проброса. «Too many authentication failures» (агент с шестью и больше ключами, среди которых нет
/// принятого) — отказ во входе, хотя сервер при этом и разрывает соединение («Received disconnect … Disconnected
/// from», что похоже на обрыв): как обрыв (`exited`) он перезапускался бы, и супервизор повторял бы вход раз в 30 с.
pub fn classify_stderr(code: Option<i32>, stderr: &str, report: Option<&str>) -> SshErrorKind {
    let _ = code;
    let s = stderr;
    if let Some(r) = report
        && r.contains("authenticity of host")
    {
        let (host, fingerprint) = parse_host_key_report(r);
        return SshErrorKind::HostKeyUnknown { host, fingerprint };
    }
    if s.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        SshErrorKind::HostKeyChanged
    } else if s.contains("Host key verification failed") {
        SshErrorKind::HostKeyUnknown {
            host: None,
            fingerprint: None,
        }
    } else if s.contains("Permission denied") || s.contains("Too many authentication failures") {
        SshErrorKind::AuthFailed
    } else if s.contains("Address already in use")
        || s.contains("Could not request local forwarding")
    {
        SshErrorKind::BindFailed
    } else {
        SshErrorKind::Exited
    }
}

/// `SSH_ERROR` с видом в `params.kind` (спец. §5.2, отступление 3); `tail` — хвост вывода ssh в `details`.
pub fn ssh_error(kind: SshErrorKind, target: &str, tail: &str) -> AppError {
    let mut e = match &kind {
        SshErrorKind::NotFound => app_err!(
            ErrorCode::SshError,
            "errors.ssh.notFound",
            "OpenSSH client (ssh) was not found: install it (Windows: the “OpenSSH Client” optional feature)"
        ),
        SshErrorKind::TooOld => app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.ssh.tooOld",
            "OpenSSH 8.4 or newer is needed for password and passphrase prompts; use an agent or a key without a passphrase"
        ),
        SshErrorKind::HostKeyUnknown { host, fingerprint } => app_err!(
            ErrorCode::SshError,
            "errors.ssh.hostKeyUnknown",
            "The host key of {{host}} is not known yet ({{fingerprint}})",
            host = host.clone().unwrap_or_else(|| target.to_string()),
            fingerprint = fingerprint.clone().unwrap_or_default()
        ),
        SshErrorKind::HostKeyChanged => app_err!(
            ErrorCode::SshError,
            "errors.ssh.hostKeyChanged",
            "The host key of {{target}} has changed: check ~/.ssh/known_hosts yourself",
            target = target
        ),
        SshErrorKind::AuthFailed => app_err!(
            ErrorCode::SshError,
            "errors.ssh.authFailed",
            "SSH login to {{target}} was refused: check the user, the key or the password",
            target = target
        ),
        SshErrorKind::BindFailed => app_err!(
            ErrorCode::SshError,
            "errors.ssh.bindFailed",
            "Could not bind a local port for the SSH tunnel"
        ),
        SshErrorKind::ForwardFailed { host, port } => app_err!(
            ErrorCode::SshError,
            "errors.ssh.forwardFailed",
            "The bastion {{target}} could not reach {{host}}:{{port}}",
            target = target,
            host = host,
            port = port
        ),
        SshErrorKind::Exited => app_err!(
            ErrorCode::SshError,
            "errors.ssh.exited",
            "The ssh process to {{target}} exited",
            target = target
        ),
    };
    e.params.insert("kind".into(), kind.name().into());
    if !target.is_empty() {
        e.params.insert("target".into(), target.into());
    }
    if !tail.trim().is_empty() {
        e = e.with_details(tail.trim());
    }
    // Обрыв и сбой проброса проходят сами (перезапуск, повтор); ключ хоста, вход, клиент — нет.
    e.retryable = matches!(
        kind,
        SshErrorKind::Exited | SshErrorKind::ForwardFailed { .. } | SshErrorKind::BindFailed
    );
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> SshSettings {
        SshSettings {
            target: "bastion".into(),
            port: Some(2222),
            user: Some("tester".into()),
            auth: SshAuth::Key,
            key_path: Some("/keys/id_ed25519".into()),
        }
    }

    #[test]
    fn command_line_is_fixed_and_uses_dynamic_forward() {
        let args = SshArgs {
            bin: PathBuf::from("/usr/bin/ssh"),
            log_file: PathBuf::from("/logs/ssh-c1.log"),
            socks_port: 40123,
            settings: settings(),
            known_hosts: Some(PathBuf::from("/tmp/kh")),
            extra_env: vec![],
        };
        let line: Vec<String> = args
            .command_line()
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            line,
            [
                "-N",
                "-T",
                "-E",
                "/logs/ssh-c1.log",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "StrictHostKeyChecking=ask",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "BatchMode=no",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-o",
                "UserKnownHostsFile=/tmp/kh",
                "-p",
                "2222",
                "-l",
                "tester",
                "-i",
                "/keys/id_ed25519",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "PreferredAuthentications=publickey",
                "-D",
                "127.0.0.1:40123",
                "--",
                "bastion",
            ]
        );
        // Способ «агент»: без -i и без ограничения способов входа; «пароль» — тоже без -i, но с
        // PreferredAuthentications=password.
        let mut agent = settings();
        agent.auth = SshAuth::Agent;
        agent.key_path = None;
        let line: Vec<String> = SshArgs {
            settings: agent,
            ..args.clone()
        }
        .command_line()
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert!(!line.iter().any(|a| a == "-i"));
        assert!(
            !line
                .iter()
                .any(|a| a.starts_with("PreferredAuthentications"))
        );
        let mut pw = settings();
        pw.auth = SshAuth::Password;
        pw.key_path = None;
        let line: Vec<String> = SshArgs {
            settings: pw,
            ..args
        }
        .command_line()
        .into_iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        assert!(
            line.windows(2)
                .any(|w| w == ["-o", "PreferredAuthentications=password"])
        );
    }

    #[test]
    fn stderr_is_classified() {
        let c = |code: i32, text: &str| classify_stderr(Some(code), text, None);
        assert_eq!(
            c(
                255,
                "tester@bastion: Permission denied (publickey,password).\n"
            ),
            SshErrorKind::AuthFailed
        );
        assert!(matches!(
            c(
                255,
                "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\n"
            ),
            SshErrorKind::HostKeyChanged
        ));
        assert_eq!(
            c(255, "Host key verification failed.\n"),
            SshErrorKind::HostKeyUnknown {
                host: None,
                fingerprint: None
            }
        );
        assert!(matches!(
            c(
                255,
                "bind [127.0.0.1]:40123: Address already in use\nCould not request local forwarding.\n"
            ),
            SshErrorKind::BindFailed
        ));
        // Неудачный CONNECT через -D процесс не завершает: процесс, завершившийся после него, завершился по другой
        // причине — `exited`, а не сбой проброса (у того нет узла: он виден только соединению через SOCKS).
        assert_eq!(
            c(
                255,
                "channel 2: open failed: connect failed: Connection refused\n"
            ),
            SshErrorKind::Exited
        );
        assert!(matches!(
            c(
                255,
                "ssh: connect to host bastion port 22: Connection refused\n"
            ),
            SshErrorKind::Exited
        ));
        // Неудачный CONNECT через -D остался в логе, но процесс завершил обрыв связи.
        assert_eq!(
            c(
                255,
                "channel 2: open failed: connect failed: Connection refused\nTimeout, server bastion not responding.\n"
            ),
            SshErrorKind::Exited
        );
        // Обрыв по другим признакам: «Connection closed/reset by», «timed out», «Disconnected from».
        for line in [
            "Connection closed by 10.0.0.5 port 22",
            "Read from remote host bastion: Connection reset by peer",
            "Connection reset by 10.0.0.5 port 22",
            "ssh: connect to host bastion port 22: Operation timed out",
            "Disconnected from 10.0.0.5 port 22",
        ] {
            assert_eq!(
                c(
                    255,
                    &format!(
                        "channel 2: open failed: connect failed: Connection refused\n{line}\n"
                    )
                ),
                SshErrorKind::Exited,
                "{line}"
            );
        }
        // Агент с шестью и больше ключами, ни один не принят: сервер разрывает соединение, но это отказ во входе —
        // супервизор встаёт на стоянку, а не входит раз в 30 с.
        assert_eq!(
            c(
                255,
                "Received disconnect from 10.0.0.5 port 22:2: Too many authentication failures\nDisconnected from 10.0.0.5 port 22\n"
            ),
            SshErrorKind::AuthFailed
        );
        assert_eq!(
            c(
                255,
                "Received disconnect from 10.0.0.5 port 22:2: Too many authentication failures\r\nDisconnected from authenticating user tester 10.0.0.5 port 22 [preauth]\r\nConnection closed by 10.0.0.5 port 22\r\n"
            ),
            SshErrorKind::AuthFailed
        );
        // Отчёт askpass точнее stderr: в нём хост и отпечаток.
        let report = "The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:abcDEF+/=.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert_eq!(
            classify_stderr(Some(255), "Host key verification failed.\n", Some(report)),
            SshErrorKind::HostKeyUnknown {
                host: Some("bastion (10.0.0.5)".into()),
                fingerprint: Some("SHA256:abcDEF+/=".into())
            }
        );
    }

    #[test]
    fn version_is_parsed_from_ssh_v_output() {
        assert_eq!(
            parse_version("OpenSSH_9.6p1, LibreSSL 3.3.6\n"),
            Some((9, 6))
        );
        assert_eq!(
            parse_version("OpenSSH_8.4p1 Debian-5, OpenSSL 1.1.1k\n"),
            Some((8, 4))
        );
        assert_eq!(
            parse_version("OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2\n"),
            Some((9, 5))
        );
        assert_eq!(parse_version("ssh: command not found"), None);
        assert!(askpass_supported((8, 4)) && !askpass_supported((8, 3)));
    }

    /// Параллельные тесты (туннели с поддельным ssh) тоже берут свободные порты и могут занять тот же порт между
    /// `free_port` и повторной привязкой — та же гонка, ради которой супервизор повторяет запуск: несколько попыток.
    #[test]
    fn free_port_is_bindable_again() {
        let bound = (0..5).any(|_| {
            let p = free_port().unwrap();
            assert!(p > 0);
            std::net::TcpListener::bind(("127.0.0.1", p)).is_ok()
        });
        assert!(bound);
    }

    #[test]
    fn ssh_error_carries_kind_target_details_and_retry() {
        let e = ssh_error(SshErrorKind::AuthFailed, "bastion", "  line 1\nline 2 \n");
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::SshError, "errors.ssh.authFailed")
        );
        assert_eq!(e.params["kind"], "authFailed");
        assert_eq!(e.params["target"], "bastion");
        assert_eq!(e.details.as_deref(), Some("line 1\nline 2"));
        assert!(!e.retryable);
        // Порт так и не занят (после повтора с новым портом) — свой текст, а не «бастион не достучался».
        let e = ssh_error(SshErrorKind::BindFailed, "bastion", "");
        assert_eq!(
            (e.key.as_str(), e.params["kind"].as_str(), e.retryable),
            ("errors.ssh.bindFailed", "bindFailed", true)
        );
        assert_eq!(e.message, "Could not bind a local port for the SSH tunnel");
        assert_eq!(e.details, None);
        let e = ssh_error(
            SshErrorKind::ForwardFailed {
                host: "pg-1".into(),
                port: 5432,
            },
            "bastion",
            "early eof",
        );
        assert_eq!(
            (e.key.as_str(), e.params["kind"].as_str(), e.retryable),
            ("errors.ssh.forwardFailed", "forwardFailed", true)
        );
        assert_eq!(
            (
                e.params["target"].as_str(),
                e.params["host"].as_str(),
                e.params["port"].as_str()
            ),
            ("bastion", "pg-1", "5432")
        );
        assert_eq!(e.message, "The bastion bastion could not reach pg-1:5432");
        assert_eq!(e.details.as_deref(), Some("early eof"));
        assert!(ssh_error(SshErrorKind::Exited, "bastion", "").retryable);
        let e = ssh_error(
            SshErrorKind::HostKeyUnknown {
                host: Some("bastion (10.0.0.5)".into()),
                fingerprint: Some("SHA256:x".into()),
            },
            "bastion",
            "",
        );
        assert_eq!(e.params["host"], "bastion (10.0.0.5)");
        assert_eq!(e.params["fingerprint"], "SHA256:x");
        assert_eq!(
            e.message,
            "The host key of bastion (10.0.0.5) is not known yet (SHA256:x)"
        );
        let e = ssh_error(SshErrorKind::TooOld, "bastion", "");
        assert_eq!(
            (e.code, e.params["kind"].as_str()),
            (ErrorCode::FeatureUnavailable, "tooOld")
        );
        // У «нет клиента» цели нет — и параметра тоже.
        let e = ssh_error(SshErrorKind::NotFound, "", "");
        assert!(!e.params.contains_key("target"));
    }

    /// Профиль с диска не проверяется при чтении: значение с «-» из вручную изменённого файла не должно
    /// дойти до ssh как опция.
    #[tokio::test]
    async fn spawn_rechecks_settings_before_starting_ssh() {
        let mut bad = settings();
        bad.target = "-oProxyCommand=touch /tmp/pwned".into();
        let args = SshArgs {
            bin: PathBuf::from("/nonexistent/ssh"),
            log_file: PathBuf::from("/nonexistent/ssh-c1.log"),
            socks_port: 40123,
            settings: bad,
            known_hosts: None,
            extra_env: vec![],
        };
        let env = AskpassEnv {
            connection_id: "c1".into(),
            report_file: PathBuf::from("/nonexistent/report"),
            trust: None,
            secret_file: None,
            askpass_bin: None,
        };
        let e = spawn(&args, &env).err().expect("spawn must refuse");
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::InvalidInput, "errors.connections.sshTarget")
        );
    }

    /// Исполняемый скрипт вместо ssh: аргументы те же, `$4` — путь лога из `-E`.
    #[cfg(unix)]
    fn fake_ssh(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("ssh");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Только что записанный скрипт на Linux изредка не запускается («Text file busy»), если другой поток тестов
    /// в этот момент порождает процесс: такие запуски повторяются.
    #[cfg(unix)]
    fn text_file_busy(e: &AppError) -> bool {
        e.details
            .as_deref()
            .is_some_and(|d| d.contains("Text file busy"))
    }

    #[cfg(unix)]
    fn spawn_fake(args: &SshArgs, env: &AskpassEnv) -> SshChild {
        for _ in 0..20 {
            match spawn(args, env) {
                Ok(child) => return child,
                Err(e) if text_file_busy(&e) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => panic!("{e:?}"),
            }
        }
        panic!("the fake ssh stays busy");
    }

    #[cfg(unix)]
    async fn version_of_fake(bin: &Path) -> AppResult<(u32, u32)> {
        for _ in 0..20 {
            match ssh_version(bin).await {
                Err(e) if text_file_busy(&e) => tokio::time::sleep(Duration::from_millis(50)).await,
                r => return r,
            }
        }
        panic!("the fake ssh stays busy");
    }

    #[cfg(unix)]
    fn fake_args(dir: &Path, bin: PathBuf) -> (SshArgs, AskpassEnv) {
        let args = SshArgs {
            bin,
            log_file: dir.join("ssh-c1.log"),
            socks_port: 40123,
            settings: settings(),
            known_hosts: None,
            extra_env: vec![],
        };
        let env = AskpassEnv {
            connection_id: "c1".into(),
            report_file: dir.join("askpass-c1.txt"),
            trust: Some("SHA256:x".into()),
            secret_file: None,
            askpass_bin: None,
        };
        (args, env)
    }

    /// С `-E` ssh пишет ошибки в лог, а не в stderr: хвост берёт строки лога этого запуска (без прошлых) и stderr,
    /// дочитанный после выхода процесса.
    #[cfg(unix)]
    #[tokio::test]
    async fn exit_before_ready_reports_this_runs_log_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ssh-c1.log"),
            "old run: Host key verification failed.\n",
        )
        .unwrap();
        let bin = fake_ssh(
            dir.path(),
            "echo \"tester@bastion: Permission denied (publickey).\" >> \"$4\"\n\
             echo \"env $SSH_ASKPASS_REQUIRE $GLAFIRA_ASKPASS $GLAFIRA_ASKPASS_TRUST\" >&2\n\
             exit 255",
        );
        let (args, env) = fake_args(dir.path(), bin);
        let mut child = spawn_fake(&args, &env);
        let unused = SocketAddr::from(([127, 0, 0, 1], free_port().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(Some(255)));
        let tail = child.stderr_tail();
        assert!(tail.contains("Permission denied"), "{tail}");
        assert!(tail.contains("env force c1 SHA256:x"), "{tail}");
        assert!(!tail.contains("old run"), "{tail}");
        assert_eq!(
            classify_stderr(Some(255), &tail, None),
            SshErrorKind::AuthFailed
        );
    }

    /// Баннер сервера ssh печатает в настоящий stderr, а итоговую ошибку пишет в лог `-E`: длинный stderr не
    /// должен вытеснять строки лога, по которым разбирается ошибка. У каждого источника свой запас строк.
    #[cfg(unix)]
    #[tokio::test]
    async fn long_stderr_does_not_push_out_the_log_lines_classification_needs() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_ssh(
            dir.path(),
            "i=1\n\
             while [ $i -le 25 ]; do echo \"banner $i\" >&2; i=$((i+1)); done\n\
             i=1\n\
             while [ $i -le 30 ]; do echo \"log noise $i\" >> \"$4\"; i=$((i+1)); done\n\
             echo \"tester@bastion: Permission denied (publickey,password).\" >> \"$4\"\n\
             exit 255",
        );
        let (args, env) = fake_args(dir.path(), bin);
        let mut child = spawn_fake(&args, &env);
        let unused = SocketAddr::from(([127, 0, 0, 1], free_port().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(Some(255)));
        let tail = child.stderr_tail();
        assert!(tail.contains("Permission denied"), "{tail}");
        assert_eq!(
            classify_stderr(Some(255), &tail, None),
            SshErrorKind::AuthFailed
        );
        // Из stderr — последние 10 строк, из лога — последние 20, лог идёт последним.
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(lines.len(), 30, "{tail}");
        assert_eq!(lines[0], "banner 16", "{tail}");
        assert_eq!(lines[9], "banner 25", "{tail}");
        assert_eq!(lines[10], "log noise 12", "{tail}");
        assert!(lines[29].contains("Permission denied"), "{tail}");
    }

    /// Бинарь askpass, файл секретов и дополнительное окружение доходят до ssh; переменные askpass приложения
    /// главнее дополнительных, а унаследованный файл секретов без явного указания снимается.
    #[cfg(unix)]
    #[tokio::test]
    async fn askpass_binary_secret_file_and_extra_env_reach_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_ssh(
            dir.path(),
            "echo \"env [$SSH_ASKPASS] [$GLAFIRA_ASKPASS_SECRET_FILE] [$FAKE_X] [$GLAFIRA_ASKPASS]\" >&2\n\
             exit 255",
        );
        let (mut args, mut env) = fake_args(dir.path(), bin);
        args.extra_env = vec![
            ("FAKE_X".into(), "1".into()),
            ("GLAFIRA_ASKPASS".into(), "spoofed".into()),
        ];
        env.askpass_bin = Some(PathBuf::from("/opt/glafira-pg"));
        env.secret_file = Some(dir.path().join("askpass-c1.secrets"));
        let unused = SocketAddr::from(([127, 0, 0, 1], free_port().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut child = spawn_fake(&args, &env);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(Some(255)));
        let tail = child.stderr_tail();
        let secrets = dir.path().join("askpass-c1.secrets");
        assert!(
            tail.contains(&format!(
                "env [/opt/glafira-pg] [{}] [1] [c1]",
                secrets.display()
            )),
            "{tail}"
        );
        // Без указания: SSH_ASKPASS — текущий бинарь, файла секретов нет.
        env.askpass_bin = None;
        env.secret_file = None;
        args.extra_env = vec![(
            "GLAFIRA_ASKPASS_SECRET_FILE".into(),
            "/tmp/inherited".into(),
        )];
        let mut child = spawn_fake(&args, &env);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(Some(255)));
        let tail = child.stderr_tail();
        let me = std::env::current_exe().unwrap();
        assert!(
            tail.contains(&format!("env [{}] [] [] [c1]", me.display())),
            "{tail}"
        );
    }

    /// Не-UTF-8 в stderr (вывод ProxyCommand, askpass) не обрывает чтение: следующие строки доходят, а дочерний
    /// процесс не упирается в закрытый канал.
    #[cfg(unix)]
    #[tokio::test]
    async fn non_utf8_stderr_line_does_not_stop_the_reader() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_ssh(
            dir.path(),
            "printf 'before\\377after\\n' >&2\n\
             echo \"later line\" >&2\n\
             exit 255",
        );
        let (args, env) = fake_args(dir.path(), bin);
        let mut child = spawn_fake(&args, &env);
        let unused = SocketAddr::from(([127, 0, 0, 1], free_port().unwrap()));
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(Some(255)));
        let tail = child.stderr_tail();
        assert!(tail.contains("before\u{fffd}after"), "{tail}");
        assert!(tail.contains("later line"), "{tail}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ready_when_the_socks_port_accepts_and_killed_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let bin = fake_ssh(dir.path(), "exec sleep 30");
        let (args, env) = fake_args(dir.path(), bin);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut child = spawn_fake(&args, &env);
        assert!(child.pid().is_some());
        let deadline = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            child
                .wait_ready(listener.local_addr().unwrap(), deadline)
                .await,
            Ok(())
        );
        child.kill().await;
        // Порт так и не открылся: к сроку процесс завершается.
        let mut child = spawn_fake(&args, &env);
        let unused = SocketAddr::from(([127, 0, 0, 1], free_port().unwrap()));
        let deadline = Instant::now() + Duration::from_millis(300);
        assert_eq!(child.wait_ready(unused, deadline).await, Err(None));
        assert!(child.try_exit_code().is_some(), "процесс завершён");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn version_comes_from_ssh_v_and_a_foreign_client_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let ok = fake_ssh(dir.path(), "echo 'OpenSSH_9.6p1, LibreSSL 3.3.6' >&2");
        assert_eq!(version_of_fake(&ok).await.unwrap(), (9, 6));
        let other = tempfile::tempdir().unwrap();
        let foreign = fake_ssh(other.path(), "echo 'Dropbear v2022.83' >&2");
        let e = version_of_fake(&foreign).await.unwrap_err();
        assert_eq!(
            (e.code, e.params["kind"].as_str()),
            (ErrorCode::SshError, "notFound")
        );
        assert!(e.details.unwrap().contains("Dropbear"));
    }
}
