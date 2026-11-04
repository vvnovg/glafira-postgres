//! Шим `glafira-pg mcp` как процесс (спец. P4 §3.3): режим выбирается в `main` до Tauri, в stdout — только протокол.
//! Каталог данных шима — `dirs::data_dir()`; тест подменяет его через `HOME` и `XDG_DATA_HOME` (на Windows каталог
//! берётся из API оболочки — тест только для unix). Бинарь тестов — отладочная сборка: Glafira шим не запускает.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use glafira_mcp_core::endpoint::{Endpoint, Listener};
use glafira_mcp_core::handshake;
use glafira_pg_lib::mcp::files::{self, McpFile};
use glafira_pg_lib::settings::SettingsStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_glafira-pg");

/// Каталог данных, который шим найдёт при `HOME = home` (как `dirs::data_dir()` + идентификатор).
fn data_dir(home: &Path) -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".local/share")
    };
    base.join(glafira_pg_lib::product::IDENTIFIER)
}

fn shim(home: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.arg("mcp")
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    c
}

#[tokio::test]
async fn disabled_mcp_exits_with_a_message_and_a_clean_stdout() {
    let home = tempfile::tempdir().unwrap();
    let child = shim(home.path()).spawn().unwrap();
    let out = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("шим завершился сам, Tauri не запускался")
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "stdout — только протокол MCP");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("glafira-pg mcp: MCP is disabled in Glafira: turn it on in Settings"),
        "{err}"
    );
}

#[tokio::test]
async fn shim_pumps_bytes_between_stdio_and_the_app_socket() {
    let home = tempfile::tempdir().unwrap();
    let dir = data_dir(home.path());
    std::fs::create_dir_all(&dir).unwrap();
    SettingsStore::open(&dir).set_mcp_enabled(true).unwrap();
    // Короткий путь сокета: каталог данных внутри временного HOME мог бы превысить предел длины пути сокета.
    let sock_dir = tempfile::Builder::new()
        .prefix("gpm")
        .tempdir_in("/tmp")
        .unwrap();
    let ep = Endpoint::Unix(sock_dir.path().join("s.sock"));
    let mut listener = Listener::bind(&ep).await.unwrap();
    files::write_mcp_file(
        &dir,
        &McpFile {
            version: files::VERSION,
            endpoint: ep,
            token: "tok".into(),
            pid: 1,
        },
    )
    .unwrap();

    let mut child = shim(home.path()).spawn().unwrap();
    let mut app = tokio::time::timeout(Duration::from_secs(20), listener.accept())
        .await
        .expect("шим подключился")
        .unwrap();
    handshake::server(&mut app, "tok").await.unwrap();

    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"ping\n").await.unwrap();
    let mut got = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), app.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"ping\n");

    app.write_all(b"pong\n").await.unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut got = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), stdout.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&got, b"pong\n");

    // Приложение закрыло соединение — шим выходит с кодом 0.
    drop(app);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("шим вышел после закрытия сокета")
        .unwrap();
    assert_eq!(status.code(), Some(0));
}
