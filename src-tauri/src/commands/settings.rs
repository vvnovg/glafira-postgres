use tauri::State;

use super::blocking;
use crate::error::{AppResult, ErrorCode};
use crate::policy::PolicyStatus;
use crate::settings::{LanguageSettings, language_settings, system_locale};
use crate::state::AppState;

/// Состояние политики администратора: прочитано один раз при запуске (`lib.rs`).
#[tauri::command]
pub fn get_policy_status(state: State<'_, AppState>) -> AppResult<PolicyStatus> {
    Ok(state.policy.status.clone())
}

fn current(st: &AppState) -> LanguageSettings {
    language_settings(
        system_locale(),
        st.settings.language(),
        st.policy.status.language.clone(),
    )
}

/// Блокирующая часть `set_language` (запись файла). Все языки доступны всем.
fn apply_language(st: &AppState, lang: Option<&str>) -> AppResult<LanguageSettings> {
    if st.policy.status.language.is_some() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.settings.languageByPolicy",
            "The language is set by your organization"
        ));
    }
    st.settings.set_language(lang)?;
    Ok(current(st))
}

/// Настройки читаются под блокировкой, которую держит запись файла (с fsync): асинхронная команда ждёт её
/// в фоновом потоке — синхронная ждала бы в главном и замораживала окно.
#[tauri::command]
pub async fn get_language_settings(state: State<'_, AppState>) -> AppResult<LanguageSettings> {
    let st = state.inner().clone();
    blocking(move || Ok(current(&st))).await
}

#[tauri::command]
pub async fn set_language(
    state: State<'_, AppState>,
    lang: Option<String>,
) -> AppResult<LanguageSettings> {
    let st = state.inner().clone();
    blocking(move || apply_language(&st, lang.as_deref())).await
}

#[tauri::command]
pub async fn get_ui_settings(state: State<'_, AppState>) -> AppResult<crate::settings::UiSettings> {
    let st = state.inner().clone();
    blocking(move || Ok(st.settings.ui())).await
}

#[tauri::command]
pub async fn set_ui_settings(
    state: State<'_, AppState>,
    settings: crate::settings::UiSettings,
) -> AppResult<crate::settings::UiSettings> {
    let st = state.inner().clone();
    blocking(move || {
        st.settings.set_ui(settings.clone())?;
        Ok(settings)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;
    use crate::settings::LanguageReason;
    use std::sync::Arc;

    fn state(dir: &std::path::Path) -> AppState {
        AppState::new(dir, Arc::new(MemoryStore::default())).unwrap()
    }

    #[test]
    fn user_can_pick_any_language() {
        let dir = tempfile::tempdir().unwrap();
        let st = state(dir.path());
        let r = apply_language(&st, Some("ru")).unwrap();
        assert_eq!(
            (r.effective.as_str(), r.reason),
            ("ru", LanguageReason::User)
        );
        let r = apply_language(&st, Some("en")).unwrap();
        assert_eq!(
            (r.effective.as_str(), r.reason),
            ("en", LanguageReason::User)
        );
        assert_eq!(st.settings.language().as_deref(), Some("en"));
        let r = apply_language(&st, None).unwrap();
        assert_eq!(r.chosen, None);
    }

    #[test]
    fn policy_language_blocks_change() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = state(dir.path());
        let mut policy = crate::policy::Policy::default();
        policy.status.language = Some("en".into());
        st = st.with_policy(policy);
        assert_eq!(
            apply_language(&st, Some("ru")).unwrap_err().code,
            ErrorCode::InvalidInput
        );
        assert_eq!(st.settings.language(), None);
        let s = current(&st);
        assert_eq!(
            (s.effective.as_str(), s.reason),
            ("en", LanguageReason::Policy)
        );
    }

    #[test]
    fn with_policy_keeps_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = crate::policy::Policy::default();
        policy.status.managed = true;
        policy.status.source = Some("src".into());
        let st = state(dir.path()).with_policy(policy);
        assert!(st.policy.status.managed);
        assert_eq!(st.policy.status.source.as_deref(), Some("src"));
    }
}
