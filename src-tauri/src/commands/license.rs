use tauri::State;

use super::blocking;
use crate::error::AppResult;
use crate::license::LicenseStatus;
use crate::policy::PolicyStatus;
use crate::state::AppState;

#[tauri::command]
pub fn get_policy_status(state: State<'_, AppState>) -> AppResult<PolicyStatus> {
    Ok(state.policy.status.clone())
}

#[tauri::command]
pub fn get_license_status(state: State<'_, AppState>) -> AppResult<LicenseStatus> {
    Ok(state.license.status())
}

#[tauri::command]
pub async fn set_license_key(state: State<'_, AppState>, key: String) -> AppResult<LicenseStatus> {
    let st = state.inner().clone();
    blocking(move || st.license.set_user_key(&key)).await
}

#[tauri::command]
pub async fn remove_license_key(state: State<'_, AppState>) -> AppResult<LicenseStatus> {
    let st = state.inner().clone();
    blocking(move || st.license.remove_user_key()).await
}
