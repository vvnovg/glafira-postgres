//! Окружение логин-оболочки (спец. P4c §4.2). GUI-приложение macOS и Linux не наследует `PATH` и переменные профиля
//! оболочки, поэтому Glafira один раз за сеанс снимает окружение `$SHELL -ilc` (`env -0` между метками, срок 5 с), как
//! VS Code: так у Qwen Code находится `node`, а ключи API и прокси из профиля доходят до агента. Не удалось — окружение
//! Glafira, а `PATH` процесса агента дополняется каталогом программы и известными местами. На Windows оболочки нет:
//! окружение Glafira. Оболочка (и `--version` программы, `detect`) идёт в новой сессии без управляющего терминала
//! (`setsid`); по сроку останавливается вся её группа процессов — и фоновые процессы профиля.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::sync::MutexExt;

pub const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const BEGIN: &str = "__GLAFIRA_BEGIN_5c1e__";
const END: &str = "__GLAFIRA_END_5c1e__";
/// Переменные самой оболочки — не окружение пользователя.
const SKIP_VARS: [&str; 4] = ["_", "SHLVL", "PWD", "OLDPWD"];
/// Сколько ждать конца stdout и stderr после выхода процесса: фоновый процесс профиля может держать их открытыми, а
/// вывод самого процесса к этому времени уже прочитан.
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// Сколько stderr хранить (начало): он нужен только для текста ошибки (`detect::version_of`).
const STDERR_KEEP: usize = 16 * 1024;

/// Окружение процесса агента. `Debug` показывает только имена переменных: в значениях бывают ключи API и токены.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ShellEnv {
    pub vars: Vec<(String, String)>,
    /// Окружение снято с логин-оболочки; `false` — окружение Glafira.
    pub from_shell: bool,
}

impl std::fmt::Debug for ShellEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellEnv")
            .field("from_shell", &self.from_shell)
            .field(
                "vars",
                &self.vars.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Имена переменных на Windows не различают регистр (`Path` = `PATH`).
fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

impl ShellEnv {
    /// Окружение процесса Glafira (запасной путь и Windows).
    pub fn current_process() -> Self {
        Self {
            vars: std::env::vars_os()
                .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
                .collect(),
            from_shell: false,
        }
    }

    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| same_name(k, name))
            .map(|(_, v)| v.as_str())
    }

    /// Окружение процесса `program`. Снятое с оболочки — как есть; только если каталога программы в `PATH` нет, он
    /// добавляется в конец (программа, указанная руками, например из каталога `nvm`, находит свой `node`, а порядок
    /// поиска пользователя не меняется). Иначе `PATH` = каталог программы, известные места и прежний `PATH` (без
    /// повторов): так у `qwen` из `~/.npm-global/bin` находится `node`.
    pub fn for_program(&self, program: &Path) -> Vec<(String, String)> {
        let program_dir = program
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let old: Vec<PathBuf> = self
            .var("PATH")
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default();
        let dirs: Vec<PathBuf> = if self.from_shell {
            // `PATH` пользователя не переставляется и не чистится от повторов: только хвост.
            match program_dir {
                Some(d) if !old.contains(&d) => old.into_iter().chain([d]).collect(),
                _ => return self.vars.clone(),
            }
        } else {
            let mut unique: Vec<PathBuf> = Vec::new();
            let all = program_dir
                .into_iter()
                .chain(known_dirs(dirs::home_dir().as_deref()))
                .chain(old);
            for d in all {
                if !d.as_os_str().is_empty() && !unique.contains(&d) {
                    unique.push(d);
                }
            }
            unique
        };
        let mut vars = self.vars.clone();
        let Ok(path) = std::env::join_paths(&dirs) else {
            return vars;
        };
        let path = path.to_string_lossy().into_owned();
        match vars.iter_mut().find(|(k, _)| same_name(k, "PATH")) {
            Some((_, v)) => *v = path,
            None => vars.push(("PATH".into(), path)),
        }
        vars
    }
}

/// Известные места агентских CLI (спец. §4.2).
pub fn known_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            out.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(h) = home {
            out.push(h.join(".local").join("bin"));
        }
        return out;
    }
    if let Some(h) = home {
        out.push(h.join(".local/bin"));
    }
    out.push(PathBuf::from("/opt/homebrew/bin"));
    out.push(PathBuf::from("/usr/local/bin"));
    if let Some(h) = home {
        out.push(h.join(".npm-global/bin"));
        out.push(h.join(".claude/local"));
    }
    out
}

/// Оболочка пользователя: `$SHELL`, иначе zsh на macOS и sh на Linux.
pub fn login_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            })
        })
}

/// Окружение сеанса: снимается при первом обращении (до `SHELL_TIMEOUT`), дальше — из памяти. Блокирующий вызов.
pub fn shell_env() -> Arc<ShellEnv> {
    static ENV: OnceLock<Arc<ShellEnv>> = OnceLock::new();
    ENV.get_or_init(|| {
        Arc::new(if cfg!(unix) {
            capture_with(&login_shell(), SHELL_TIMEOUT)
        } else {
            ShellEnv::current_process()
        })
    })
    .clone()
}

/// `shell -ilc 'env -0'` между метками; без `PATH` в ответе, по сроку или с ошибкой — окружение Glafira.
pub fn capture_with(shell: &Path, timeout: Duration) -> ShellEnv {
    match run_marked(shell, "env -0", timeout) {
        Some(bytes) => {
            let vars = parse_env0(&bytes);
            if vars.iter().any(|(k, _)| same_name(k, "PATH")) {
                return ShellEnv {
                    vars,
                    from_shell: true,
                };
            }
            tracing::warn!("login shell {} returned no PATH", shell.display());
        }
        None => tracing::warn!(
            "login shell environment not captured: {} (Glafira environment is used)",
            shell.display()
        ),
    }
    ShellEnv::current_process()
}

/// `KEY=VALUE\0…` → пары; переменные самой оболочки и имена не из `[A-Za-z0-9_]` (функции bash) пропускаются.
pub(crate) fn parse_env0(bytes: &[u8]) -> Vec<(String, String)> {
    bytes
        .split(|b| *b == 0)
        .filter_map(|item| {
            let s = String::from_utf8_lossy(item);
            let (k, v) = s.split_once('=')?;
            let ok = !k.is_empty()
                && !SKIP_VARS.contains(&k)
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            ok.then(|| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// `shell -ilc '<script>'`; вывод между метками — то, что напечатал `script` (профиль оболочки печатает что угодно до и
/// после). Блокирующий вызов.
pub(crate) fn run_marked(shell: &Path, script: &str, timeout: Duration) -> Option<Vec<u8>> {
    let full = format!("printf '%s' '{BEGIN}'; {script}; printf '%s' '{END}'");
    let mut cmd = Command::new(shell);
    cmd.arg("-ilc").arg(full);
    let done = run_with_timeout(cmd, timeout).ok()??;
    between(&done.stdout, BEGIN.as_bytes(), END.as_bytes())
}

/// Процесс `run_with_timeout`, закончившийся сам.
#[derive(Debug)]
pub(crate) struct Finished {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    /// Начало stderr, до `STDERR_KEEP`.
    pub stderr: Vec<u8>,
}

/// Читатель потока процесса в своём потоке ОС: в `buf` — не больше `keep` байт (остальное читается и отбрасывается, чтобы
/// процесс не встал на полном pipe), в конце — сигнал `done`.
fn read_all(
    mut from: impl Read + Send + 'static,
    keep: usize,
    done: mpsc::Sender<()>,
) -> Arc<Mutex<Vec<u8>>> {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let sink = buf.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match from.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut b = sink.lock_safe();
                    let room = keep.saturating_sub(b.len()).min(n);
                    b.extend_from_slice(&chunk[..room]);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = done.send(());
    });
    buf
}

/// Процесс без stdin, stdout — в память, начало stderr — тоже; срок — на процесс. На Unix процесс — в новой сессии
/// (`setsid`): интерактивная оболочка (`-i`) не берёт терминал, из которого запущена Glafira (`pnpm tauri dev`, тесты), а
/// по сроку SIGKILL получает вся её группа (`killpg`), а не только сама оболочка. После выхода процесса конец stdout и
/// stderr ждётся не дольше `DRAIN_GRACE` (фоновый процесс профиля может их держать), отдаётся прочитанное. `Err` —
/// процесс не запустился; `Ok(None)` — не уложился в срок (убит). Блокирующий вызов.
pub(crate) fn run_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> io::Result<Option<Finished>> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: между fork и exec зовётся только `setsid` — async-signal-safe, без выделения памяти и блокировок.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn()?;
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        kill(&mut child);
        return Ok(None);
    };
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let stdout = read_all(out, usize::MAX, done_tx.clone());
    let stderr = read_all(err, STDERR_KEEP, done_tx);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                kill(&mut child);
                return Ok(None);
            }
        }
    };
    let drained = Instant::now() + DRAIN_GRACE;
    for _ in 0..2 {
        if done_rx
            .recv_timeout(drained.saturating_duration_since(Instant::now()))
            .is_err()
        {
            break;
        }
    }
    let take = |b: &Arc<Mutex<Vec<u8>>>| std::mem::take(&mut *b.lock_safe());
    Ok(Some(Finished {
        code: status.code(),
        stdout: take(&stdout),
        stderr: take(&stderr),
    }))
}

/// Остановка процесса `run_with_timeout` и (Unix) всей его группы: фоновые процессы профиля не переживают оболочку.
fn kill(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(pg) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `killpg` только посылает сигнал группе, которую создал `setsid` этого потомка (pgid = его pid). Потомок
        // ещё не прибран (`wait` — ниже), поэтому его pid не мог достаться другому процессу. ESRCH не важна.
        unsafe {
            libc::killpg(pg, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn between(hay: &[u8], begin: &[u8], end: &[u8]) -> Option<Vec<u8>> {
    let start = find(hay, begin)? + begin.len();
    let stop = start + find(&hay[start..], end)?;
    Some(hay[start..stop].to_vec())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::time::Duration;

    use super::*;

    /// Фальшивая логин-оболочка: шумит до меток, меняет окружение, как профиль, и выполняет `-c` (второй аргумент).
    #[cfg(unix)]
    fn fake_shell(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("fake-shell");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[cfg(unix)]
    #[test]
    fn the_environment_comes_from_the_login_shell_between_markers() {
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            dir.path(),
            "echo 'Welcome to a noisy profile'\n\
             export FROM_PROFILE=yes\n\
             export MULTI_LINE='a\nb=c'\n\
             PATH=\"/opt/profile/bin:$PATH\"; export PATH\n\
             [ \"$1\" = -ilc ] || exit 3\n\
             shift\n\
             eval \"$1\"\n\
             echo 'goodbye'",
        );
        let env = capture_with(&shell, Duration::from_secs(5));
        assert!(env.from_shell);
        assert_eq!(env.var("FROM_PROFILE"), Some("yes"));
        assert_eq!(
            env.var("MULTI_LINE"),
            Some("a\nb=c"),
            "env -0: перевод строки в значении"
        );
        assert!(env.var("PATH").unwrap().starts_with("/opt/profile/bin:"));
        for skipped in ["_", "SHLVL", "PWD", "OLDPWD"] {
            assert_eq!(env.var(skipped), None, "{skipped}");
        }
        // Окружение оболочки — как есть; каталог программы вне `PATH` — только в конец `PATH`. Сравнение без
        // `assert_eq!`: при падении он напечатал бы значения переменных (ключи API) в журнал тестов.
        let path = env.var("PATH").unwrap().to_string();
        let in_path = Path::new("/opt/profile/bin").join("qwen");
        assert!(
            env.for_program(&in_path) == env.vars,
            "каталог уже в PATH — без изменений"
        );
        let vars = env.for_program(Path::new("/opt/x/bin/qwen"));
        let without_path = |v: &[(String, String)]| -> Vec<(String, String)> {
            v.iter().filter(|(k, _)| k != "PATH").cloned().collect()
        };
        assert!(
            without_path(&vars) == without_path(&env.vars),
            "прочие переменные — как есть"
        );
        let new_path = vars
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str());
        assert_eq!(new_path, Some(format!("{path}:/opt/x/bin").as_str()));
        assert!(!format!("{env:?}").contains("yes"), "Debug без значений");
    }

    #[cfg(unix)]
    #[test]
    fn the_shell_path_keeps_its_order_and_repeats() {
        let env = ShellEnv {
            vars: vec![("PATH".into(), "/a:/b:/a".into())],
            from_shell: true,
        };
        let vars = env.for_program(Path::new("/c/qwen"));
        assert_eq!(vars, vec![("PATH".to_string(), "/a:/b:/a:/c".to_string())]);
        assert_eq!(env.for_program(Path::new("/b/qwen")), env.vars);
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_or_silent_shell_falls_back_to_the_environment_of_glafira() {
        let dir = tempfile::tempdir().unwrap();
        let slow = fake_shell(dir.path(), "sleep 5");
        let started = std::time::Instant::now();
        let env = capture_with(&slow, Duration::from_millis(300));
        assert!(started.elapsed() < Duration::from_secs(3), "срок соблюдён");
        assert!(!env.from_shell);
        assert!(env == ShellEnv::current_process(), "окружение Glafira");
        let silent = fake_shell(dir.path(), "echo 'no markers here'");
        assert!(!capture_with(&silent, Duration::from_secs(5)).from_shell);
        let no_path = fake_shell(dir.path(), "shift\nunset PATH\neval \"$1\"");
        assert!(
            !capture_with(&no_path, Duration::from_secs(5)).from_shell,
            "без PATH — окружение Glafira"
        );
        assert!(!capture_with(Path::new("/nonexistent/shell"), Duration::from_secs(1)).from_shell);
    }

    #[cfg(unix)]
    #[test]
    fn a_profile_that_keeps_stdout_open_does_not_hold_glafira() {
        // Фоновый процесс профиля держит stdout: вывод между метками уже есть, ждать его конца незачем. Срок — с
        // запасом на нагрузку параллельных тестов; фоновый `sleep` заметно дольше срока.
        let dir = tempfile::tempdir().unwrap();
        let shell = fake_shell(dir.path(), "(sleep 15 &)\nshift\neval \"$1\"");
        let started = std::time::Instant::now();
        let env = capture_with(&shell, Duration::from_secs(5));
        assert!(env.from_shell, "окружение снято, хотя stdout ещё открыт");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    /// Оболочка — в своей сессии (`setsid`: без управляющего терминала, группа = сессия = её pid); по сроку SIGKILL
    /// получает вся группа — фоновый процесс профиля тоже, а не только сама оболочка.
    #[cfg(unix)]
    #[test]
    fn the_shell_runs_in_its_own_session_and_its_whole_group_is_killed_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let (me, bg) = (dir.path().join("me"), dir.path().join("bg"));
        let shell = fake_shell(
            dir.path(),
            &format!(
                "sleep 30 &\necho $! > '{}'\necho $$ > '{}'\nwait",
                bg.display(),
                me.display()
            ),
        );
        let pid_in =
            |p: &Path| -> Option<i32> { std::fs::read_to_string(p).ok()?.trim().parse().ok() };
        let (me_file, bg_file) = (me.clone(), bg.clone());
        // Сессии смотрим, пока оболочка жива (до срока).
        let watcher = std::thread::spawn(move || {
            let until = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let (Some(sh), Some(bg)) = (pid_in(&me_file), pid_in(&bg_file)) {
                    // SAFETY: `getsid` только читает сессию процесса.
                    let sids = unsafe { (libc::getsid(sh), libc::getsid(bg)) };
                    return (sh, bg, sids);
                }
                assert!(
                    std::time::Instant::now() < until,
                    "оболочка не записала pid"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let started = std::time::Instant::now();
        let r = run_with_timeout(Command::new(&shell), Duration::from_secs(2)).unwrap();
        assert!(r.is_none(), "срок вышел");
        assert!(started.elapsed() < Duration::from_secs(5));
        let (sh, bg, (sh_sid, bg_sid)) = watcher.join().unwrap();
        assert_eq!(
            (sh_sid, bg_sid),
            (sh, sh),
            "своя сессия у оболочки и её фонового процесса"
        );
        // SAFETY: сигнал 0 только проверяет, что процесс есть (убитый осиротевший процесс сразу прибирает init).
        let alive = || unsafe { libc::kill(bg, 0) } == 0;
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while alive() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!alive(), "фоновый процесс оболочки остановлен вместе с ней");
    }

    #[test]
    fn env0_is_parsed_without_shell_variables() {
        let vars = parse_env0(b"A=1\0B=x=y\0_=/usr/bin/env\0SHLVL=2\0BAD\0\0BASH_FUNC_f%%=() {}\0");
        assert_eq!(
            vars,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "x=y".to_string())
            ]
        );
    }

    #[test]
    fn without_the_shell_path_gets_the_program_folder_and_known_places() {
        let old = std::env::join_paths([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
        let env = ShellEnv {
            vars: vec![
                ("PATH".into(), old.to_string_lossy().into_owned()),
                ("LANG".into(), "C".into()),
            ],
            from_shell: false,
        };
        let program = Path::new("/opt/x/bin").join("qwen");
        let vars = env.for_program(&program);
        let path = vars.iter().find(|(k, _)| k == "PATH").unwrap().1.clone();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
        assert_eq!(
            dirs[0],
            PathBuf::from("/opt/x/bin"),
            "каталог программы — первым"
        );
        assert!(dirs.contains(&PathBuf::from("/usr/bin")) && dirs.contains(&PathBuf::from("/bin")));
        for known in known_dirs(dirs::home_dir().as_deref()) {
            assert!(dirs.contains(&known), "{}", known.display());
        }
        let unique: std::collections::BTreeSet<&PathBuf> = dirs.iter().collect();
        assert_eq!(unique.len(), dirs.len(), "без повторов");
        assert!(vars.contains(&("LANG".into(), "C".into())));
        // Без PATH вовсе — переменная появляется.
        let bare = ShellEnv {
            vars: Vec::new(),
            from_shell: false,
        };
        let vars = bare.for_program(&program);
        assert_eq!(vars.len(), 1);
        assert_eq!(vars[0].0, "PATH");
    }

    #[cfg(unix)]
    #[test]
    fn known_places_of_agent_clis() {
        let home = Path::new("/Users/u");
        assert_eq!(
            known_dirs(Some(home)),
            vec![
                PathBuf::from("/Users/u/.local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/Users/u/.npm-global/bin"),
                PathBuf::from("/Users/u/.claude/local"),
            ]
        );
    }
}
