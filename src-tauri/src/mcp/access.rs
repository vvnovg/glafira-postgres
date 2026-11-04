//! Доступ модели к подключению (спец. P4 §4.1, §4.3 шаги 3–4): действующий уровень, проверка при каждом вызове,
//! открытие закрытого подключения тем же путём, что щелчок в дереве, и перевод ошибок приложения в ошибки
//! инструментов (§10.2). Шаги 1–2 (MCP включён, клиент подтверждён) проверяет ядро шлюза через `Gate`.

use glafira_mcp_core::error::ToolError;

use crate::actions::journal::code_name;
use crate::conn::profile::{ConnectionProfile, LlmAccess};
use crate::error::{AppError, ErrorCode};
use crate::policy::{McpMaxAccess, PolicyStatus};
use crate::state::AppState;
use crate::topology::runtime::ConnState;

/// Текст внутренней ошибки для модели: подробности (пути, паники) остаются в логе приложения.
const INTERNAL_MESSAGE: &str = "Internal error in Glafira; retry the call, and if it repeats ask the user to check the Glafira log";
/// Ошибки файлов настроек подключения (сертификаты, ключи) и файлов данных Glafira: путь модели не нужен.
const SETTINGS_MESSAGE: &str = "The settings or certificate files of this connection cannot be used; ask the user to check the connection in Glafira";
/// Ошибки туннеля ssh: цель бастиона, адрес узла за ним и вывод ssh модели не нужны (§6.2: настройки SSH не
/// отдаются никогда) — подробности пользователь видит в Glafira.
const SSH_MESSAGE: &str = "The SSH tunnel to the server failed; see Glafira for details";
const PATH_ERROR_PREFIXES: [&str; 3] =
    ["errors.tls.", "errors.storage.", "errors.connections.path"];
const FILE_ERROR_KEYS: [&str; 2] = ["errors.io", "errors.json"];
/// Параметры ошибки, которых модель не получает ни у одной ошибки: пути файлов и цель ssh.
const HIDDEN_PARAMS: [&str; 3] = ["path", "file", "target"];

/// Действующий уровень: меньшее из уровня подключения и потолка политики `mcpMaxAccess` (§4.2). Потолок `data` выше
/// любого уровня P4a (`off`, `diagnostics`) и ничего не сужает; P4b сопоставит его с `LlmAccess::Data`.
pub fn effective_access(profile: &ConnectionProfile, policy: &PolicyStatus) -> LlmAccess {
    let ceiling = match policy.mcp_max_access {
        McpMaxAccess::Diagnostics => LlmAccess::Diagnostics,
        McpMaxAccess::Data => LlmAccess::Diagnostics,
    };
    profile.llm_access.min(ceiling)
}

/// Уровень `diagnostics`: литералы маскируются, статистика колонок — без значений (§4.1).
pub fn masks(access: LlmAccess) -> bool {
    matches!(access, LlmAccess::Diagnostics)
}

/// Шаг 3 §4.3: подключение есть, и его действующий уровень не `off`. Отсутствующее подключение и закрытое для модели
/// неразличимы (`not_allowed`).
pub fn check_connection(
    st: &AppState,
    connection_id: &str,
) -> Result<(ConnectionProfile, LlmAccess), ToolError> {
    let profile = st
        .profile(connection_id)
        .map_err(|_| ToolError::not_allowed())?;
    let access = effective_access(&profile, &st.policy.status);
    if access.is_off() {
        return Err(ToolError::not_allowed());
    }
    Ok((profile, access))
}

/// Открывает подключение, если оно закрыто, — как щелчок в дереве (`commands::topology::connect`): keychain, туннель
/// ssh, первый опрос топологии; интерфейс видит открытие по событиям `connection://state`. Открытое — как есть.
///
/// Открытие идёт отдельной задачей tokio: закрытие сессии MCP бросает тело вызова на любом `await`, а брошенное на
/// полпути `Runtimes::connect` оставило бы подключение в состоянии «Подключение…» без опроса. Задача доводит открытие
/// до конца (`Connected` или `Failed`) и без вызова.
pub async fn ensure_open(st: &AppState, connection_id: &str) -> Result<(), ToolError> {
    if matches!(
        st.runtimes.view(connection_id).state,
        ConnState::Connected | ConnState::Degraded
    ) {
        return Ok(());
    }
    let profile = st
        .profile(connection_id)
        .map_err(|_| ToolError::not_allowed())?;
    let runtimes = st.runtimes.clone();
    tokio::spawn(async move { runtimes.connect(profile).await })
        .await
        .map_err(|e| tool_error(AppError::internal_with(format!("connect task: {e}"))))?
        .map(|_| ())
        .map_err(tool_error)
}

/// Проверка доступа, затем открытие: закрытое для модели подключение не открывается.
pub async fn open_for_llm(st: &AppState, connection_id: &str) -> Result<LlmAccess, ToolError> {
    let (_, access) = check_connection(st, connection_id)?;
    ensure_open(st, connection_id).await?;
    Ok(access)
}

/// Ошибка инструмента с кодом и английским текстом.
pub fn err(code: &str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.to_string(),
        message: message.into(),
    }
}

pub fn invalid_argument(message: impl Into<String>) -> ToolError {
    err("invalid_argument", message)
}

/// Ошибка, текст которой модель получает общим: внутренняя (пути, паники), файлов настроек и данных (пути), туннеля
/// ssh (цель бастиона, адрес узла, вывод ssh).
fn hidden_message(e: &AppError) -> Option<&'static str> {
    let file_error = FILE_ERROR_KEYS.contains(&e.key.as_str())
        || PATH_ERROR_PREFIXES.iter().any(|p| e.key.starts_with(p));
    if e.code == ErrorCode::Internal {
        Some(INTERNAL_MESSAGE)
    } else if file_error {
        Some(SETTINGS_MESSAGE)
    } else if e.code == ErrorCode::SshError {
        Some(SSH_MESSAGE)
    } else {
        None
    }
}

/// Ошибка приложения → ошибка инструмента (§10.2): код `ErrorCode` строкой (`NODE_UNAVAILABLE`, `TIMEOUT`…) и
/// английский текст `AppError`; ввод — `invalid_argument`, «нет объекта» — `not_found`; внутренняя ошибка, ошибки
/// файлов и туннеля ssh — общим текстом (пути, цель ssh и подробности уходят в лог приложения).
pub fn tool_error(e: AppError) -> ToolError {
    if let Some(message) = hidden_message(&e) {
        tracing::warn!(
            key = %e.key,
            details = e.details.as_deref().unwrap_or_default(),
            "MCP tool error hidden from the model: {}",
            e.message
        );
        let code = if e.code == ErrorCode::Internal {
            "internal".to_string()
        } else {
            code_name(e.code)
        };
        return err(&code, message);
    }
    match e.code {
        ErrorCode::InvalidInput => invalid_argument(e.message),
        ErrorCode::NotFound => err("not_found", e.message),
        code => err(&code_name(code), e.message),
    }
}

/// Ошибка приложения внутри данных отчёта для модели (`PulseNode.error`, `HealthCell.error`): код и ключ — как есть,
/// без `details`; у скрытых ошибок (`tool_error`: внутренняя, файлов, ssh) текст — общий, а из параметров остаётся
/// только вид (`kind`), у остальных из параметров уходят пути и цель ssh. Интерфейс получает исходную ошибку: замена —
/// только в ответе модели.
pub fn model_app_error(e: &AppError) -> AppError {
    let mut out = e.clone();
    out.details = None;
    match hidden_message(e) {
        Some(message) => {
            out.message = message.to_string();
            out.params.retain(|k, _| k == "kind");
        }
        None => out
            .params
            .retain(|k, _| !HIDDEN_PARAMS.contains(&k.as_str())),
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::secrets::MemoryStore;

    fn state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        (dir, st)
    }

    #[test]
    fn unknown_and_closed_for_llm_connections_are_indistinguishable() {
        let (_dir, st) = state();
        st.connections.upsert(sample_profile("off")).unwrap();
        let mut on = sample_profile("on");
        on.llm_access = LlmAccess::Diagnostics;
        st.connections.upsert(on).unwrap();
        let off = check_connection(&st, "off").expect_err("off закрыт");
        let unknown = check_connection(&st, "nope").expect_err("нет подключения");
        assert_eq!(off.code, "not_allowed");
        assert_eq!(
            (off.code.as_str(), off.message.as_str()),
            (unknown.code.as_str(), unknown.message.as_str())
        );
        let (profile, access) = check_connection(&st, "on").expect("открыт для LLM");
        assert_eq!(profile.id, "on");
        assert!(matches!(access, LlmAccess::Diagnostics));
        assert!(masks(access) && !masks(LlmAccess::Off));
        assert!(matches!(
            effective_access(&profile, &st.policy.status),
            LlmAccess::Diagnostics
        ));
    }

    #[tokio::test]
    async fn closed_for_llm_connection_is_not_opened() {
        let (_dir, st) = state();
        st.connections.upsert(sample_profile("off")).unwrap();
        let e = open_for_llm(&st, "off").await.expect_err("off закрыт");
        assert_eq!(e.code, "not_allowed");
        assert_eq!(
            st.runtimes.view("off").state,
            ConnState::Disconnected,
            "проверка — до открытия"
        );
    }

    #[test]
    fn app_errors_become_tool_errors_without_internal_details_and_paths() {
        let e = tool_error(AppError::internal_with("panic at src/x.rs:1"));
        assert_eq!(e.code, "internal");
        assert!(!e.message.contains("src/x.rs"), "{}", e.message);
        let e = tool_error(app_err!(
            ErrorCode::NodeUnavailable,
            "errors.topology.notConnected",
            "The connection is not open"
        ));
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("NODE_UNAVAILABLE", "The connection is not open")
        );
        let e = tool_error(app_err!(
            ErrorCode::InvalidInput,
            "errors.diag.badWindow",
            "The window must be 300, 900 or 3600 seconds"
        ));
        assert_eq!(e.code, "invalid_argument");
        let e = tool_error(app_err!(
            ErrorCode::InvalidInput,
            "errors.tls.readFailed",
            "Cannot read {{path}}",
            path = "/Users/me/ca.pem"
        ));
        assert!(!e.message.contains("/Users/me"), "{}", e.message);
        let e = tool_error(app_err!(
            ErrorCode::NotFound,
            "errors.catalog.objectNotFound",
            "Object {{schema}}.{{name}} was not found (dropped or renamed?)",
            schema = "s",
            name = "t"
        ));
        assert_eq!(e.code, "not_found");
    }

    /// Цель бастиона, адрес узла за ним и вывод ssh модели не уходят: код `SSH_ERROR` и общий текст (§6.2, §10.2).
    #[test]
    fn ssh_errors_reach_the_model_without_the_bastion() {
        use crate::net::ssh::process::{SshErrorKind, ssh_error};
        use crate::net::ssh::socks::{forward_failed, tunnel_down};
        for e in [
            ssh_error(
                SshErrorKind::AuthFailed,
                "admin@bastion.corp.example",
                "admin@bastion.corp.example: Permission denied (publickey)",
            ),
            ssh_error(SshErrorKind::Exited, "admin@bastion.corp.example", ""),
            forward_failed(
                "admin@bastion.corp.example",
                "pg-hidden.internal",
                5432,
                "early eof",
            ),
            tunnel_down("admin@bastion.corp.example", 40123, "connection refused"),
        ] {
            let t = tool_error(e.clone());
            assert_eq!(
                (t.code.as_str(), t.message.as_str()),
                ("SSH_ERROR", SSH_MESSAGE),
                "{e:?}"
            );
            let m = serde_json::to_string(&model_app_error(&e)).unwrap();
            assert!(
                !m.contains("bastion") && !m.contains("pg-hidden") && !m.contains("Permission"),
                "{m}"
            );
            assert!(m.contains("SSH_ERROR") && m.contains(SSH_MESSAGE), "{m}");
        }
        let kind = model_app_error(&ssh_error(SshErrorKind::AuthFailed, "bastion", ""));
        assert_eq!(
            kind.params.get("kind").map(String::as_str),
            Some("authFailed")
        );
    }

    /// Ошибка внутри данных (`PulseNode.error`, `HealthCell.error`): без `details`, путей и цели ssh; остальное — как
    /// у интерфейса.
    #[test]
    fn embedded_errors_lose_details_and_paths() {
        let e = app_err!(
            ErrorCode::NodeUnavailable,
            "errors.pg.tcpFailed",
            "Could not reach {{host}}:{{port}}",
            host = "pg-1",
            port = 5432
        )
        .with_details("tcp connect error: /var/run/secret.sock");
        let m = model_app_error(&e);
        assert_eq!(m.details, None);
        assert_eq!(
            (m.code, m.key.as_str(), m.message.as_str()),
            (e.code, e.key.as_str(), e.message.as_str())
        );
        assert_eq!(m.params, e.params);
        let e = app_err!(
            ErrorCode::TlsError,
            "errors.tls.readFailed",
            "Cannot read {{path}}",
            path = "/Users/me/ca.pem"
        );
        let m = serde_json::to_string(&model_app_error(&e)).unwrap();
        assert!(!m.contains("/Users/me"), "{m}");
        assert!(
            m.contains(SETTINGS_MESSAGE) && m.contains("TLS_ERROR"),
            "{m}"
        );
        let m = serde_json::to_string(&model_app_error(&AppError::internal_with(
            "panic at src/x.rs:1",
        )))
        .unwrap();
        assert!(
            !m.contains("src/x.rs") && m.contains(INTERNAL_MESSAGE),
            "{m}"
        );
    }
}
