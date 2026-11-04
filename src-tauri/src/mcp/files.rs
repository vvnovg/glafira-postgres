//! `mcp.json` (спец. P4 §3.3): адрес и токен работающего MCP-сервера. Файл существует, только пока приложение
//! обслуживает MCP (удаляется при выключении и при выходе); его читает шим `glafira-pg mcp`.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use glafira_mcp_core::endpoint::Endpoint;
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::storage::fs::{read_json, write_json};

pub const FILE_NAME: &str = "mcp.json";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpFile {
    pub version: u32,
    /// Путь сокета или имя named pipe (в файле — строка).
    pub endpoint: Endpoint,
    /// 64 hex-символа; новый при каждом запуске сервера.
    pub token: String,
    pub pid: u32,
}

/// Атомарная запись с правами 0600 (`write_json`).
pub fn write_mcp_file(dir: &Path, f: &McpFile) -> AppResult<()> {
    write_json(&dir.join(FILE_NAME), f)
}

/// Нечитаемый файл или файл другой версии — как будто его нет.
pub fn read_mcp_file(dir: &Path) -> Option<McpFile> {
    read_json::<McpFile>(&dir.join(FILE_NAME))
        .ok()
        .flatten()
        .filter(|f| f.version == VERSION)
}

/// Удаляет `mcp.json`, только если в нём `pid` и `token` из `own`: файл, который успел записать другой экземпляр
/// Glafira, остаётся. Ошибки — в журнал.
pub fn remove_mcp_file(dir: &Path, own: &McpFile) {
    if !read_mcp_file(dir).is_some_and(|cur| cur.pid == own.pid && cur.token == own.token) {
        return;
    }
    if let Err(e) = std::fs::remove_file(dir.join(FILE_NAME))
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!("mcp.json not removed: {e}");
    }
}

/// Каталог данных без Tauri — так его находит шим. Совпадает с `app_data_dir()` Tauri (`<data_dir ОС>/<identifier>`);
/// приложение при запуске сверяет оба пути (`lib.rs`).
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join(crate::product::IDENTIFIER))
}

/// Токен рукопожатия: 32 байта из генератора ОС, строчный hex.
pub fn new_token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    let mut out = String::with_capacity(64);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(dir: &Path) -> McpFile {
        McpFile {
            version: VERSION,
            endpoint: Endpoint::for_app(dir, crate::product::MCP_SERVER_NAME),
            token: new_token().unwrap(),
            pid: 42,
        }
    }

    #[test]
    fn written_file_is_private_read_back_and_removed_only_by_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_mcp_file(dir.path()), None);
        let f = sample(dir.path());
        write_mcp_file(dir.path(), &f).unwrap();
        assert_eq!(read_mcp_file(dir.path()), Some(f.clone()));
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join(FILE_NAME)).unwrap()).unwrap();
        assert!(
            raw["endpoint"].is_string(),
            "шим читает адрес строкой: {raw}"
        );
        assert_eq!(
            (raw["version"].as_u64(), raw["pid"].as_u64()),
            (Some(1), Some(42))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(FILE_NAME))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
        let foreign = McpFile {
            token: new_token().unwrap(),
            ..f.clone()
        };
        remove_mcp_file(dir.path(), &foreign);
        assert_eq!(
            read_mcp_file(dir.path()),
            Some(f.clone()),
            "файл другого экземпляра остаётся"
        );
        remove_mcp_file(dir.path(), &f);
        assert_eq!(read_mcp_file(dir.path()), None);
        remove_mcp_file(dir.path(), &f);
    }

    #[test]
    fn other_version_or_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = serde_json::to_value(sample(dir.path())).unwrap();
        v["version"] = 2.into();
        std::fs::write(dir.path().join(FILE_NAME), v.to_string()).unwrap();
        assert_eq!(read_mcp_file(dir.path()), None);
        std::fs::write(dir.path().join(FILE_NAME), b"{").unwrap();
        assert_eq!(read_mcp_file(dir.path()), None);
    }

    #[test]
    fn tokens_are_32_random_bytes_in_hex() {
        let (a, b) = (new_token().unwrap(), new_token().unwrap());
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, b);
    }

    #[test]
    fn shim_data_dir_ends_with_identifier() {
        assert!(data_dir().unwrap().ends_with(crate::product::IDENTIFIER));
    }
}
