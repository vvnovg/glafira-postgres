//! Команды фонового ассистента (спец. P4c §8.3, §8.4): подраздел «Фоновый ассистент» и запуски. Логика — `agent::*`;
//! настройки читаются и пишутся в фоновом потоке (запись `settings.json` с fsync держит блокировку), поиск программы и
//! модели — тоже (логин-оболочка, `--version`, чтение файлов); очередь пишет `agent-runs.json` — тоже в фоновом потоке, а
//! сам запуск идёт задачей tokio.

use std::path::Path;

use tauri::State;

use super::blocking;
use crate::agent::detect::{self, Detected};
use crate::agent::models::{self, AgentModels};
use crate::agent::ollama;
use crate::agent::queue::{RunFilter, log_missing};
use crate::agent::runs::AgentRun;
use crate::agent::settings::{AgentSettings, Preset, check_path, disabled_by_policy};
use crate::agent::shell_env::{ShellEnv, shell_env};
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
    blocking(move || {
        let saved = apply_agent_settings(&st, settings)?;
        // Выключенный режим снимает очередь (`skipped`); `McpStatus.agent` уходит в интерфейс (`mcp://state`).
        st.agent.settings_changed();
        Ok(saved)
    })
    .await
}

/// «Найти»: логин-оболочка, `PATH`, известные места. С `path` — только версия этой программы (путь, исправленный руками).
#[tauri::command]
pub async fn detect_agent_cli(preset: Preset, path: Option<String>) -> AppResult<Detected> {
    blocking(move || detect_cli(preset, path, &shell_env())).await
}

/// `detect_agent_cli` в окружении `env`. Путь, указанный руками, — как в «Настройках» (`settings::check_path`): не
/// абсолютный — отказ `pathNotAbsolute`, программа не запускается (иначе имя нашлось бы по `PATH` окружения агента и
/// показало версию, а «Сохранить» потом отказало бы). Блокирующий вызов.
pub(crate) fn detect_cli(
    preset: Preset,
    path: Option<String>,
    env: &ShellEnv,
) -> AppResult<Detected> {
    match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(p) => {
            check_path(&p)?;
            Ok(detect::with_version(preset, Path::new(&p), env))
        }
        None => Ok(detect::detect(preset, env)),
    }
}

/// Модели для поля «Модель»: у Qwen Code — из его настроек; у Claude Code «Через Ollama» — `/api/tags` локальной
/// Ollama (по сохранённым «Через Ollama» и адресу); иначе — свободный ввод.
#[tauri::command]
pub async fn list_agent_models(
    state: State<'_, AppState>,
    preset: Preset,
) -> AppResult<AgentModels> {
    match preset {
        Preset::Qwen => {
            blocking(|| {
                Ok(dirs::home_dir().map_or_else(AgentModels::none, |home| {
                    AgentModels::from_qwen(models::qwen_models(&home))
                }))
            })
            .await
        }
        Preset::Claude => {
            let st = state.inner().clone();
            let s = blocking(move || Ok(st.settings.agent())).await?;
            Ok(ollama::list_models(&s).await)
        }
    }
}

/// «Выгрузить модель» в подразделе «Фоновый ассистент» (спец. §6.2): модель сохранённых настроек, только у локальной
/// Ollama; пока идёт запуск — ошибка.
#[tauri::command]
pub async fn unload_agent_model(state: State<'_, AppState>) -> AppResult<()> {
    let st = state.inner().clone();
    let s = {
        let st = st.clone();
        blocking(move || Ok(st.settings.agent())).await?
    };
    ollama::unload_model(&st, &s, dirs::home_dir().as_deref()).await
}

/// «Передать LLM-ассистенту» в режиме фона и «Повторить».
#[tauri::command]
pub async fn start_agent_run(
    state: State<'_, AppState>,
    request_id: String,
) -> AppResult<AgentRun> {
    let st = state.inner().clone();
    blocking(move || st.agent.start(&request_id)).await
}

/// «Остановить»: итог `cancelled` приходит событием `agent://runs`.
#[tauri::command]
pub async fn stop_agent_run(state: State<'_, AppState>, run_id: String) -> AppResult<AgentRun> {
    let st = state.inner().clone();
    blocking(move || st.agent.stop(&run_id)).await
}

/// Запуски для строки статуса и «Настроек», сначала новые; пустой фильтр — все, вместе с «Проверкой».
#[tauri::command]
pub async fn list_agent_runs(
    state: State<'_, AppState>,
    filter: RunFilter,
) -> AppResult<Vec<AgentRun>> {
    let st = state.inner().clone();
    blocking(move || Ok(st.agent.list(&filter))).await
}

/// «Показать журнал»: хвост до 256 КБ; у идущего запуска — то, что уже записано.
#[tauri::command]
pub async fn read_agent_log(state: State<'_, AppState>, run_id: String) -> AppResult<String> {
    let st = state.inner().clone();
    blocking(move || st.agent.read_log(&run_id)).await
}

/// «Показать файл» журнала запуска.
#[tauri::command]
pub fn reveal_agent_log(state: State<'_, AppState>, run_id: String) -> AppResult<()> {
    let path = state.agent.log_file(&run_id)?;
    tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| {
        tracing::warn!("reveal agent log: {e}");
        log_missing(&run_id)
    })
}

/// «Проверить» в подразделе «Фоновый ассистент».
#[tauri::command]
pub async fn check_agent(state: State<'_, AppState>) -> AppResult<AgentRun> {
    let st = state.inner().clone();
    blocking(move || st.agent.check()).await
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

    /// Путь, указанный руками, — только абсолютный, как в «Настройках»: относительное имя не ищется по `PATH` окружения
    /// агента и не запускается (ревью задачи 5, замечание 3).
    #[cfg(unix)]
    #[test]
    fn detect_with_a_relative_path_is_refused_without_starting_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("started");
        let qwen = dir.path().join("qwen");
        std::fs::write(
            &qwen,
            format!("#!/bin/sh\n: > '{}'\necho 0.26.5\n", started.display()),
        )
        .unwrap();
        std::fs::set_permissions(&qwen, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = ShellEnv {
            vars: vec![("PATH".into(), dir.path().display().to_string())],
            from_shell: true,
        };
        for p in ["qwen", " qwen ", "./qwen", "bin/qwen"] {
            assert_eq!(
                detect_cli(Preset::Qwen, Some(p.into()), &env)
                    .unwrap_err()
                    .key,
                "errors.agent.pathNotAbsolute",
                "{p}"
            );
        }
        assert!(!started.exists(), "программа не запускалась");
        let d = detect_cli(Preset::Qwen, Some(format!(" {} ", qwen.display())), &env).unwrap();
        assert_eq!(
            (d.path.as_deref(), d.version.as_deref()),
            (qwen.to_str(), Some("0.26.5"))
        );
        assert!(started.exists());
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
