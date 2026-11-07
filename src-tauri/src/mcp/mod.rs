//! MCP-шлюз Glafira (спец. `docs/superpowers/specs/2026-11-04-glafira-postgres-p4-design.md`): связь сервера из
//! крейтов `glafira-mcp-core` и `glafira-pg-mcp` с приложением — контроллер и `mcp.json`, подтверждение клиентов,
//! право сессий и данные инструментов (`AppBackend`), фрагменты конфигов клиентов.

pub mod access;
pub mod approvals;
pub mod backend;
pub mod controller;
pub mod files;
pub mod glossary;
pub mod guard;
pub mod llm;
pub mod mask;
pub mod model_sql;
pub mod observe;
pub mod read;
pub mod secrets;
pub mod select;
pub mod shim;
pub mod texts;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

pub use approvals::PendingApproval;
pub use controller::{McpController, McpServerState, McpStatus, McpUi};

/// Событие статуса для интерфейса: `McpStatus` целиком при каждом изменении.
pub const STATE_EVENT: &str = "mcp://state";

/// Готовые фрагменты конфигов клиентов для «Настроек» (спец. P4 §11).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpClientConfigs {
    /// Команда любого stdio-клиента и её аргументы.
    pub command: String,
    pub args: Vec<String>,
    /// `qwen mcp add …`.
    pub qwen_code: String,
    /// Блок `mcpServers` для `~/.qwen/settings.json`.
    pub qwen_settings: String,
    /// `claude mcp add … -- …`.
    pub claude_code: String,
    /// Блок для `claude_desktop_config.json`.
    pub claude_desktop: String,
    /// Блок для `~/.cursor/mcp.json`.
    pub cursor: String,
}

/// Путь для командной строки: в одинарных кавычках на macOS/Linux (если в нём есть что-то кроме букв, цифр и
/// `/._-`), в двойных на Windows.
fn shell_quote(s: &str) -> String {
    if cfg!(windows) {
        return format!("\"{s}\"");
    }
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Путь, который запускают клиенты MCP и шим: в AppImage `current_exe()` — временная точка монтирования,
/// исчезающая после выхода, поэтому берётся `$APPIMAGE` (спец. P4 §3.3, §11).
pub fn launch_path(exe: &Path, appimage: Option<OsString>) -> PathBuf {
    match appimage {
        Some(a) if !a.is_empty() => PathBuf::from(a),
        _ => exe.to_path_buf(),
    }
}

pub fn client_configs(exe: &Path) -> McpClientConfigs {
    let command = exe.to_string_lossy().into_owned();
    let name = crate::product::MCP_SERVER_NAME;
    let quoted = shell_quote(&command);
    // Qwen Code, Claude Desktop и Cursor читают блок одной формы; файлы у них разные.
    let servers = serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": { name: { "command": command, "args": ["mcp"] } }
    }))
    .unwrap_or_default();
    McpClientConfigs {
        qwen_code: format!("qwen mcp add {name} {quoted} mcp"),
        qwen_settings: servers.clone(),
        claude_code: format!("claude mcp add {name} -- {quoted} mcp"),
        claude_desktop: servers.clone(),
        cursor: servers,
        args: vec!["mcp".into()],
        command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn configs_quote_paths_with_spaces() {
        let exe = Path::new("/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg");
        let c = client_configs(exe);
        let quoted = "'/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg'";
        assert_eq!(c.qwen_code, format!("qwen mcp add glafira-pg {quoted} mcp"));
        assert_eq!(
            c.claude_code,
            format!("claude mcp add glafira-pg -- {quoted} mcp")
        );
        assert_eq!(
            (c.command.as_str(), c.args.clone()),
            (exe.to_str().unwrap(), vec!["mcp".to_string()])
        );
        for block in [&c.qwen_settings, &c.claude_desktop, &c.cursor] {
            let v: serde_json::Value = serde_json::from_str(block).unwrap();
            assert_eq!(
                v["mcpServers"]["glafira-pg"]["command"],
                exe.to_str().unwrap()
            );
            assert_eq!(
                v["mcpServers"]["glafira-pg"]["args"],
                serde_json::json!(["mcp"])
            );
        }
        let plain = client_configs(Path::new("/usr/bin/glafira-pg"));
        assert_eq!(
            plain.qwen_code,
            "qwen mcp add glafira-pg /usr/bin/glafira-pg mcp"
        );
        assert_eq!(shell_quote("/a/it's"), r"'/a/it'\''s'");
        let v = serde_json::to_value(&plain).unwrap();
        for key in [
            "command",
            "args",
            "qwenCode",
            "qwenSettings",
            "claudeCode",
            "claudeDesktop",
            "cursor",
        ] {
            assert!(v.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn appimage_path_wins_over_the_mount_point() {
        let exe = Path::new("/tmp/.mount_abc/usr/bin/glafira-pg");
        assert_eq!(
            launch_path(exe, Some("/home/u/Glafira.AppImage".into())),
            PathBuf::from("/home/u/Glafira.AppImage")
        );
        assert_eq!(launch_path(exe, None), exe.to_path_buf());
        assert_eq!(launch_path(exe, Some("".into())), exe.to_path_buf());
    }
}
