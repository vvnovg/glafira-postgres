//! Сироты ssh после сбоя приложения (спец. P1 §5): PID и путь лога в `tunnels.json` (0600) в каталоге данных.
//! При старте процессы из файла завершаются, если в их командной строке есть путь их лога (чужой процесс с
//! тем же PID не пострадает). Все функции блокирующие (файл, `ps`/PowerShell): вызывать из `spawn_blocking`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sync::MutexExt;

const FILE: &str = "tunnels.json";
/// Чтение-изменение-запись файла из разных туннелей не перемешиваются.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    pids: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    pid: u32,
    log: PathBuf,
}

/// Нет файла или он повреждён — пустой реестр (сироты — best effort, запуск не должен падать).
fn read(dir: &Path) -> Registry {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(dir: &Path, r: &Registry) -> std::io::Result<()> {
    let data = serde_json::to_vec(r)?;
    crate::storage::fs::write_private(&dir.join(FILE), &data)
}

/// Запоминает процесс ssh туннеля — сразу после запуска, ещё до готовности порта SOCKS.
pub fn register(dir: &Path, pid: u32, log: &Path) -> std::io::Result<()> {
    let _g = LOCK.lock_safe();
    let mut r = read(dir);
    r.pids.retain(|e| e.pid != pid);
    r.pids.push(Entry {
        pid,
        log: log.to_path_buf(),
    });
    write(dir, &r)
}

/// Забывает процесс (туннель закрыт штатно).
pub fn unregister(dir: &Path, pid: u32) -> std::io::Result<()> {
    let _g = LOCK.lock_safe();
    let mut r = read(dir);
    r.pids.retain(|e| e.pid != pid);
    write(dir, &r)
}

/// На Windows — без консольного окна.
#[cfg(windows)]
fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// Командная строка процесса (пусто — процесса нет или узнать нельзя).
fn command_line(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        // `-ww`: строка целиком, без обрезки по ширине.
        std::process::Command::new("ps")
            .args(["-ww", "-o", "command=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        hidden(&mut std::process::Command::new("powershell"))
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\").CommandLine"),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = pid;
        String::new()
    }
}

fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = hidden(&mut std::process::Command::new("taskkill"))
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
}

/// Завершает сирот и очищает файл; возвращает число завершённых.
pub fn reap(dir: &Path) -> usize {
    let _g = LOCK.lock_safe();
    let r = read(dir);
    let mut killed = 0;
    for e in &r.pids {
        let log = e.log.to_string_lossy();
        if !log.is_empty() && command_line(e.pid).contains(log.as_ref()) {
            terminate(e.pid);
            killed += 1;
        }
    }
    if !r.pids.is_empty() {
        let _ = write(dir, &Registry::default());
    }
    killed
}

// Тест на `sh` и сигналах — только unix.
#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    #[test]
    fn reap_kills_only_processes_whose_command_line_names_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("ssh-c1.log");
        let marker = log.to_string_lossy().into_owned();
        // Командная строка содержит путь лога — как у настоящего ssh с «-E <лог>».
        // Цикл, а не просто `sleep`: одну простую команду sh заменяет exec, и командная строка теряет метку.
        let sh = |script: String| {
            std::process::Command::new("sh")
                .args(["-c", &script])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut ours = sh(format!("while :; do sleep 1; done # {marker}"));
        let mut foreign = sh("while :; do sleep 1; done # other".to_string());
        register(dir.path(), ours.id(), &log).unwrap();
        register(dir.path(), foreign.id(), &dir.path().join("ssh-other.log")).unwrap();
        let file = dir.path().join("tunnels.json");
        assert!(file.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(reap(dir.path()), 1);
        assert!(
            ours.wait().unwrap().code().is_none(),
            "наш процесс завершён сигналом"
        );
        assert!(
            foreign.try_wait().unwrap().is_none(),
            "чужой процесс с другой командной строкой не тронут"
        );
        foreign.kill().unwrap();
        let _ = foreign.wait();
        assert!(
            !file.exists()
                || std::fs::read_to_string(&file)
                    .unwrap()
                    .contains("\"pids\":[]")
        );
    }
}
