//! «Найти» в подразделе «Фоновый ассистент» (спец. P4c §4.2): логин-оболочка (`command -v` между метками, 5 с), `PATH`
//! её окружения, известные места; версия — `<путь> --version` (10 с) в окружении агента, ниже минимальной —
//! предупреждение. На Windows вместо `where.exe` — тот же поиск по `PATH` процесса с расширениями `.exe` и `.cmd` (без
//! запуска процесса), затем `%APPDATA%\npm` и `%USERPROFILE%\.local\bin`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Serialize;

use super::presets::min_version;
use super::settings::Preset;
use super::shell_env::{
    SHELL_TIMEOUT, ShellEnv, known_dirs, login_shell, run_marked, run_with_timeout,
};

pub const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    /// Найденный абсолютный путь; `None` — не нашлось.
    pub path: Option<String>,
    /// `0.26.5`.
    pub version: Option<String>,
    /// Версия ниже минимальной пресета (`presets::min_version`).
    pub below_minimum: bool,
    /// Почему версию не узнать (технический английский текст).
    pub error: Option<String>,
}

/// Поиск на этой машине: логин-оболочка (macOS, Linux), `PATH`, известные места. Блокирующий вызов.
pub fn detect(preset: Preset, env: &ShellEnv) -> Detected {
    let shell = cfg!(unix).then(login_shell);
    let places = known_places(preset, dirs::home_dir().as_deref());
    detect_with(preset, env, shell.as_deref(), &places)
}

/// То же с заданной оболочкой и известными местами (тесты не трогают программы машины).
pub fn detect_with(
    preset: Preset,
    env: &ShellEnv,
    shell: Option<&Path>,
    places: &[PathBuf],
) -> Detected {
    let name = preset.program_name();
    let found = shell
        .and_then(|sh| which_login(sh, name, SHELL_TIMEOUT))
        .or_else(|| env.var("PATH").and_then(|p| find_in_path(name, p)))
        .or_else(|| places.iter().find(|p| is_program(p)).cloned());
    match found {
        Some(p) => with_version(preset, &p, env),
        None => Detected::default(),
    }
}

/// Путь и версия программы (найденной или заданной руками).
pub fn with_version(preset: Preset, path: &Path, env: &ShellEnv) -> Detected {
    let mut d = Detected {
        path: Some(path.display().to_string()),
        ..Detected::default()
    };
    match version_of(path, env, VERSION_TIMEOUT) {
        Ok(v) => {
            d.below_minimum =
                parse_version(&v).is_some_and(|(a, b, _)| (a, b) < min_version(preset));
            d.version = Some(v);
        }
        Err(e) => d.error = Some(e),
    }
    d
}

/// `<program> --version` в окружении агента (`ShellEnv::for_program`); версия — `a.b.c`. Блокирующий вызов.
pub fn version_of(program: &Path, env: &ShellEnv, timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.arg("--version")
        .env_clear()
        .envs(env.for_program(program));
    match run_with_timeout(cmd, timeout) {
        Err(e) => Err(format!("{} cannot be started: {e}", program.display())),
        Ok(None) => Err(format!(
            "{} --version did not answer within {timeout:?}",
            program.display()
        )),
        Ok(Some((code, out))) => {
            let text = String::from_utf8_lossy(&out);
            match parse_version(&text) {
                Some((a, b, c)) if code == Some(0) => Ok(format!("{a}.{b}.{c}")),
                _ => Err(format!(
                    "{} --version: exit code {}, output “{}”",
                    program.display(),
                    code.map_or("none".to_string(), |c| c.to_string()),
                    text.trim()
                )),
            }
        }
    }
}

/// Первое `a.b[.c]` в тексте.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    for token in s.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() < 2 || parts.iter().any(|p| p.is_empty()) {
            continue;
        }
        let nums: Option<Vec<u32>> = parts.iter().take(3).map(|p| p.parse().ok()).collect();
        if let Some(n) = nums {
            return Some((n[0], n[1], n.get(2).copied().unwrap_or(0)));
        }
    }
    None
}

/// `command -v <name>` в логин-оболочке: только абсолютный путь к файлу (алиасы и функции не годятся). Блокирующий вызов.
pub(crate) fn which_login(shell: &Path, name: &str, timeout: Duration) -> Option<PathBuf> {
    let out = run_marked(shell, &format!("command -v {name}"), timeout)?;
    let text = String::from_utf8_lossy(&out);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let p = PathBuf::from(line);
    (p.is_absolute() && is_program(&p)).then_some(p)
}

/// Расширения программы: на Windows — `.exe`, затем `.cmd` (обёртка npm).
fn program_exts() -> &'static [&'static str] {
    if cfg!(windows) {
        &[".exe", ".cmd"]
    } else {
        &[""]
    }
}

/// Поиск по `PATH` (только абсолютные каталоги).
pub fn find_in_path(name: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .filter(|d| d.is_absolute())
        .flat_map(|d| {
            program_exts()
                .iter()
                .map(move |e| d.join(format!("{name}{e}")))
        })
        .find(|p| is_program(p))
}

/// Известные места программы пресета (спец. §4.2).
pub fn known_places(preset: Preset, home: Option<&Path>) -> Vec<PathBuf> {
    let name = preset.program_name();
    known_dirs(home)
        .into_iter()
        .flat_map(|d| {
            program_exts()
                .iter()
                .map(move |e| d.join(format!("{name}{e}")))
        })
        .collect()
}

/// Файл, который можно запустить (на Unix — с правом на выполнение).
fn is_program(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn versions_are_parsed_from_cli_output() {
        assert_eq!(parse_version("0.26.5"), Some((0, 26, 5)));
        assert_eq!(parse_version("2.1.319 (Claude Code)\n"), Some((2, 1, 319)));
        assert_eq!(parse_version("qwen v0.27"), Some((0, 27, 0)));
        assert_eq!(parse_version("Claude Code 2.1.4-beta.1"), Some((2, 1, 4)));
        assert_eq!(parse_version("no version"), None);
        assert_eq!(parse_version("..."), None);
        assert_eq!(
            parse_version("build 7 of 1.2."),
            None,
            "одно число и хвост с точкой — не версия"
        );
    }

    #[cfg(unix)]
    #[test]
    fn programs_are_found_in_path_only_when_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let qwen = script(dir.path(), "qwen", "exit 0");
        let claude = dir.path().join("claude");
        std::fs::write(&claude, "x").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = std::env::join_paths([Path::new("/nonexistent"), dir.path()]).unwrap();
        let path = path.to_string_lossy();
        assert_eq!(find_in_path("qwen", &path), Some(qwen));
        assert_eq!(
            find_in_path("claude", &path),
            None,
            "без права на выполнение"
        );
        assert_eq!(
            find_in_path("qwen", "relative/bin"),
            None,
            "относительные каталоги не берутся"
        );
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::create_dir(dir.path().join("sub").join("qwen")).unwrap();
        assert_eq!(
            find_in_path("qwen", &dir.path().join("sub").to_string_lossy()),
            None,
            "каталог с именем программы — не программа"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_programs_are_found_with_cmd_and_exe() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("qwen.cmd"), "@echo 0.26.5").unwrap();
        std::fs::write(dir.path().join("claude.exe"), "x").unwrap();
        let path = dir.path().to_string_lossy();
        assert_eq!(
            find_in_path("qwen", &path),
            Some(dir.path().join("qwen.cmd"))
        );
        assert_eq!(
            find_in_path("claude", &path),
            Some(dir.path().join("claude.exe"))
        );
        let home = Path::new(r"C:\Users\u");
        assert!(
            known_places(Preset::Claude, Some(home)).contains(&home.join(r".local\bin\claude.exe"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_login_shell_finds_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let qwen = script(&bin, "qwen", "echo 0.26.5");
        let shell = script(
            dir.path(),
            "fake-shell",
            &format!(
                "echo 'motd noise'\nPATH=\"{}:/usr/bin:/bin\"; export PATH\nshift\neval \"$1\"",
                bin.display()
            ),
        );
        assert_eq!(
            which_login(&shell, "qwen", SHELL_TIMEOUT),
            Some(qwen.clone())
        );
        // `PATH` фальшивой оболочки без каталогов пользователя: настоящий `claude` машины не находится.
        assert_eq!(which_login(&shell, "claude", SHELL_TIMEOUT), None);
        let d = detect_with(
            Preset::Qwen,
            &ShellEnv::current_process(),
            Some(&shell),
            &[],
        );
        assert_eq!(d.path.as_deref(), Some(qwen.to_str().unwrap()));
        assert_eq!(
            (d.version.as_deref(), d.below_minimum, d.error),
            (Some("0.26.5"), false, None)
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_alias_in_the_login_shell_is_not_a_path() {
        let dir = tempfile::tempdir().unwrap();
        let shell = script(
            dir.path(),
            "fake-shell",
            "shift\ncommand() { echo \"alias qwen='npx qwen'\"; }\neval \"$1\"",
        );
        assert_eq!(which_login(&shell, "qwen", SHELL_TIMEOUT), None);
    }

    #[cfg(unix)]
    #[test]
    fn known_places_are_tried_and_old_versions_are_flagged() {
        let home = tempfile::tempdir().unwrap();
        let claude = script(
            &home.path().join(".local/bin"),
            "claude",
            "echo '2.0.9 (Claude Code)'",
        );
        let env = ShellEnv {
            vars: vec![("PATH".into(), "/nonexistent".into())],
            from_shell: true,
        };
        // Известные места передаются явно: на машине разработчика есть настоящий `/opt/homebrew/bin/qwen`.
        let places = known_places(Preset::Claude, Some(home.path()))
            .into_iter()
            .filter(|p| p.starts_with(home.path()))
            .collect::<Vec<_>>();
        let d = detect_with(Preset::Claude, &env, None, &places);
        assert_eq!(d.path.as_deref(), Some(claude.to_str().unwrap()));
        assert_eq!(
            (d.version.as_deref(), d.below_minimum),
            (Some("2.0.9"), true)
        );
        assert_eq!(
            known_places(Preset::Qwen, Some(Path::new("/Users/u")))[0],
            PathBuf::from("/Users/u/.local/bin/qwen")
        );
        let none = detect_with(
            Preset::Qwen,
            &env,
            None,
            &[home.path().join("missing/qwen")],
        );
        assert_eq!(none, Detected::default(), "не нашлось — пусто, без ошибки");
    }

    #[cfg(unix)]
    #[test]
    fn the_path_of_the_shell_environment_is_searched_before_known_places() {
        let dir = tempfile::tempdir().unwrap();
        let in_path = script(&dir.path().join("p"), "qwen", "echo 0.27.1");
        let known = script(&dir.path().join("k"), "qwen", "echo 0.26.0");
        let env = ShellEnv {
            vars: vec![(
                "PATH".into(),
                dir.path().join("p").to_string_lossy().into_owned(),
            )],
            from_shell: true,
        };
        let d = detect_with(Preset::Qwen, &env, None, &[known]);
        assert_eq!(d.path.as_deref(), Some(in_path.to_str().unwrap()));
        assert_eq!(d.version.as_deref(), Some("0.27.1"));
    }

    #[cfg(unix)]
    #[test]
    fn version_errors_are_reported_not_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let broken = script(dir.path(), "qwen", "echo 'node: not found' >&2; exit 127");
        let d = with_version(Preset::Qwen, &broken, &ShellEnv::current_process());
        assert_eq!(d.path.as_deref(), Some(broken.to_str().unwrap()));
        assert_eq!(d.version, None);
        assert!(
            d.error.as_deref().is_some_and(|e| e.contains("127")),
            "{:?}",
            d.error
        );
        let missing = with_version(
            Preset::Qwen,
            Path::new("/nonexistent/qwen"),
            &ShellEnv::current_process(),
        );
        assert!(missing.error.is_some());
        let hang = script(dir.path(), "claude", "sleep 5");
        let started = std::time::Instant::now();
        let e = version_of(
            &hang,
            &ShellEnv::current_process(),
            std::time::Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(e.contains("did not answer"), "{e}");
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        let v = serde_json::to_value(&d).unwrap();
        assert!(v.get("belowMinimum").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn the_version_runs_in_the_environment_of_the_agent() {
        // `--version` идёт в окружении агента: без PATH оболочки `node` из каталога программы находится.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        script(&bin, "fake-node", "echo 0.26.5");
        let qwen = script(&bin, "qwen", "exec fake-node");
        let env = ShellEnv {
            vars: vec![("PATH".into(), "/usr/bin:/bin".into())],
            from_shell: false,
        };
        assert_eq!(
            version_of(&qwen, &env, VERSION_TIMEOUT).as_deref(),
            Ok("0.26.5")
        );
    }
}
