//! Команды подраздела «Фоновый ассистент» (спец. P4c §8.3, §8.4). Логика — `agent::*`; настройки читаются и пишутся в
//! фоновом потоке (запись `settings.json` с fsync держит блокировку), поиск программы и модели — тоже (логин-оболочка,
//! `--version`, чтение файлов).

use std::path::Path;

use tauri::State;

use super::blocking;
use crate::agent::detect::{self, Detected};
use crate::agent::models::{self, AgentModels};
use crate::agent::settings::{AgentSettings, Preset, disabled_by_policy};
use crate::agent::shell_env::shell_env;
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn get_agent_settings(state: State<'_, AppState>) -> AppResult<AgentSettings> {
    let st = state.inner().clone();
    blocking(move || Ok(st.settings.agent())).await
}

/// Проверка значений и политики, запись `settings.json`. Включить ассистента при запрете (`mcpAgentEnabled = false`
/// или `mcpEnabled = false`) нельзя; включённый до запрета выключатель не мешает править остальные поля — запуски
/// запрещает сама политика. Блокирующий вызов.
pub(crate) fn apply_agent_settings(
    st: &AppState,
    settings: AgentSettings,
) -> AppResult<AgentSettings> {
    let s = settings.validated()?;
    if s.enabled && !st.policy.status.agent_allowed() && !st.settings.agent().enabled {
        return Err(disabled_by_policy());
    }
    st.settings.set_agent(s.clone())?;
    Ok(s)
}

#[tauri::command]
pub async fn set_agent_settings(
    state: State<'_, AppState>,
    settings: AgentSettings,
) -> AppResult<AgentSettings> {
    let st = state.inner().clone();
    let saved = blocking(move || apply_agent_settings(&st, settings)).await?;
    // Подраздел и индикатор читают `McpStatus.agent` (задача 6).
    state.mcp.emit_state();
    Ok(saved)
}

/// «Найти»: логин-оболочка, `PATH`, известные места. С `path` — только версия этой программы (путь, исправленный руками).
#[tauri::command]
pub async fn detect_agent_cli(preset: Preset, path: Option<String>) -> AppResult<Detected> {
    blocking(move || {
        let env = shell_env();
        Ok(
            match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
                Some(p) => detect::with_version(preset, Path::new(&p), &env),
                None => detect::detect(preset, &env),
            },
        )
    })
    .await
}

/// Модели для поля «Модель»: у Qwen Code — из его настроек; у Claude Code — свободный ввод (Ollama — задача 7).
#[tauri::command]
pub async fn list_agent_models(preset: Preset) -> AppResult<AgentModels> {
    blocking(move || {
        Ok(match (preset, dirs::home_dir()) {
            (Preset::Qwen, Some(home)) => AgentModels::from_qwen(models::qwen_models(&home)),
            _ => AgentModels::none(),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::policy::Policy;
    use crate::secrets::MemoryStore;

    fn on() -> AgentSettings {
        AgentSettings {
            enabled: true,
            ..AgentSettings::default()
        }
    }

    #[test]
    fn settings_are_validated_and_saved() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let mut s = on();
        s.qwen.model = " qwen3.6-64k:35b-a3b ".into();
        let saved = apply_agent_settings(&st, s).unwrap();
        assert_eq!(saved.qwen.model, "qwen3.6-64k:35b-a3b");
        assert_eq!(st.settings.agent(), saved);
        let mut bad = on();
        bad.timeout_min = 1;
        assert_eq!(
            apply_agent_settings(&st, bad).unwrap_err().key,
            "errors.agent.badTimeout"
        );
        assert_eq!(
            st.settings.agent(),
            saved,
            "неверные значения не записываются"
        );
    }

    #[test]
    fn policy_forbids_turning_the_assistant_on() {
        for status in [
            crate::policy::PolicyStatus {
                mcp_agent_enabled: false,
                ..Default::default()
            },
            crate::policy::PolicyStatus {
                mcp_enabled: false,
                ..Default::default()
            },
        ] {
            let dir = tempfile::tempdir().unwrap();
            let st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
                .unwrap()
                .with_policy(Policy { status });
            assert_eq!(
                apply_agent_settings(&st, on()).unwrap_err().key,
                "errors.agent.disabledByPolicy"
            );
            assert!(!st.settings.agent().enabled);
            // Выключенного ассистента сохранить можно: путь и модель на будущее.
            apply_agent_settings(
                &st,
                AgentSettings {
                    enabled: false,
                    ..on()
                },
            )
            .unwrap();
        }
    }

    #[test]
    fn a_switch_saved_before_the_policy_does_not_block_other_fields() {
        // Ассистента включили до запрета: выключатель неактивен, но путь и модель править можно (запуски запрещает
        // сама политика).
        let dir = tempfile::tempdir().unwrap();
        AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .settings
            .set_agent(on())
            .unwrap();
        let mut policy = Policy::default();
        policy.status.mcp_agent_enabled = false;
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default()))
            .unwrap()
            .with_policy(policy);
        let mut s = on();
        s.qwen.model = "qwen3.8:27b".into();
        assert_eq!(
            apply_agent_settings(&st, s).unwrap().qwen.model,
            "qwen3.8:27b"
        );
        apply_agent_settings(
            &st,
            AgentSettings {
                enabled: false,
                ..on()
            },
        )
        .unwrap();
        assert_eq!(
            apply_agent_settings(&st, on()).unwrap_err().key,
            "errors.agent.disabledByPolicy",
            "выключенного снова не включить"
        );
    }
}
