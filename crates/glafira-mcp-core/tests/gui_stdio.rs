use std::io::Write;
use std::process::{Command, Stdio};

/// На Windows проверяет, что GUI-бинарь (как release-сборка Glafira) работает через stdio: так работает шим
/// `glafira-pg mcp` (спец. P4 §3.3). На macOS и Linux — обычный процесс, тест проходит так же.
#[test]
fn gui_subsystem_binary_talks_over_pipes() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_gui_stdio_probe"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"ping\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "echo:ping\n");
}
