//! Помощники unit-тестов туннеля: поддельный `ssh` (`examples/fake_ssh.rs`).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::SystemTime;

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Бинарь не старше исходника.
fn fresh(bin: &Path, source: Option<SystemTime>) -> bool {
    matches!((modified(bin), source), (Some(b), Some(s)) if b >= s)
}

/// Путь к поддельному ssh. `cargo test` собирает примеры рядом с каталогом тестовых бинарей
/// (`target/debug/examples`); `cargo test --lib` (так идут проверки задач и CI на macOS и Windows) — нет, и
/// тогда файл (только `std`, один исходник) компилируется здесь же через `rustc` — один раз на процесс тестов
/// и заново, только если исходник новее.
pub(crate) fn fake_ssh_bin() -> PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(build).clone()
}

fn build() -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("fake_ssh.rs");
    let source_time = modified(&source);
    let exe = std::env::current_exe().expect("test binary path");
    // target/<профиль>/deps/<тест> → target/<профиль>
    let profile_dir = exe
        .parent()
        .and_then(Path::parent)
        .expect("target profile dir");
    let name = format!("fake_ssh{}", std::env::consts::EXE_SUFFIX);
    let example = profile_dir.join("examples").join(&name);
    if fresh(&example, source_time) {
        return example;
    }
    let dir = profile_dir.join("fake-ssh");
    let out = dir.join(&name);
    if fresh(&out, source_time) {
        return out;
    }
    std::fs::create_dir_all(&dir).expect("fake ssh dir");
    // Свой временный файл и атомарная замена: тесты из разных процессов не пишут в один файл.
    let tmp = dir.join(format!(
        "fake_ssh-{}{}",
        std::process::id(),
        std::env::consts::EXE_SUFFIX
    ));
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = Command::new(rustc)
        .args(["--edition", "2024", "--crate-name", "fake_ssh", "-o"])
        .arg(&tmp)
        .arg(&source)
        .status()
        .expect("rustc for fake_ssh");
    assert!(status.success(), "fake_ssh did not compile");
    // На Windows заменить запущенный бинарь нельзя: тогда этот процесс пользуется своим временным файлом.
    match std::fs::rename(&tmp, &out) {
        Ok(()) => out,
        Err(_) => tmp,
    }
}
