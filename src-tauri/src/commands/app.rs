use std::collections::BTreeMap;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_updater::UpdaterExt;
use time::format_description::well_known::Rfc3339;

use super::blocking;
use crate::diagnostics::AppInfo;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::i18n::LocalizedText;
use crate::policy::PolicyStatus;
use crate::state::AppState;

fn dir(app: &AppHandle, which: &str) -> String {
    let p = match which {
        "data" => app.path().app_data_dir(),
        _ => app.path().app_log_dir(),
    };
    p.map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "—".into())
}

pub fn app_info(app: &AppHandle) -> AppInfo {
    AppInfo {
        name: crate::product::PRODUCT_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: env!("GIT_COMMIT").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_dir: dir(app, "data"),
        log_dir: dir(app, "log"),
    }
}

#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppResult<AppInfo> {
    Ok(app_info(&app))
}

#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<()> {
    if path.trim().is_empty() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.diagnostics.noFile",
            "No file selected for the archive"
        ));
    }
    let st = state.inner().clone();
    let info = app_info(&app);
    let log_dir = app
        .path()
        .app_log_dir()
        .map_err(|e| AppError::internal_with(e.to_string()))?;
    blocking(move || {
        // Логи приложения и процессов ssh (`-E`: диагностика ssh, секретов в ней нет).
        let mut logs = crate::logging::log_files(&log_dir);
        logs.extend(crate::logging::log_files_with_prefix(&log_dir, "ssh-"));
        crate::diagnostics::write_archive(
            std::path::Path::new(&path),
            &info,
            &st.connections.list(),
            &st.policy.status,
            &logs,
        )
    })
    .await
}

/// Заглушка открытого ключа updater в `tauri.conf.json`, пока владелец не создал ключ.
/// Плагин проверяет ключ только при установке, поэтому сборку с заглушкой отсекаем сами:
/// никаких запросов к серверу обновлений и понятная ошибка вместо ошибки подписи.
const PUBKEY_PLACEHOLDER: &str = "REPLACE_WITH_UPDATER_PUBKEY";

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdateCheck {
    Disabled {
        reason: LocalizedText,
    },
    UpToDate,
    Available {
        version: String,
        notes: Option<String>,
        /// Заметки на языках интерфейса (код языка → Markdown); у релизов до этого поля — пусто.
        #[serde(rename = "notesI18n")]
        notes_i18n: BTreeMap<String, String>,
        date: Option<String>,
    },
}

/// `notes_i18n` из latest.json — расширение формата Tauri: updater его пропускает, а приложение берёт из `raw_json`.
/// Остаются только непустые строки: испорченное поле не мешает показать обычные `notes`.
fn notes_i18n(raw: &serde_json::Value) -> BTreeMap<String, String> {
    raw.get("notes_i18n")
        .and_then(serde_json::Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(code, v)| {
                    let text = v.as_str().filter(|t| !t.trim().is_empty())?;
                    Some((code.clone(), text.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug)]
enum UpdatePlan {
    Disabled(LocalizedText),
    /// None — адрес из tauri.conf.json, Some — зеркало из политики.
    Check(Option<String>),
}

fn update_plan(policy: &PolicyStatus) -> UpdatePlan {
    if !policy.updates_enabled {
        return UpdatePlan::Disabled(ltext!(
            "errors.updates.managed",
            "Updates are managed by your organization"
        ));
    }
    UpdatePlan::Check(policy.update_url.clone())
}

fn pubkey_configured(pubkey: Option<&str>) -> bool {
    pubkey.is_some_and(|k| !k.trim().is_empty() && k.trim() != PUBKEY_PLACEHOLDER)
}

fn not_configured() -> LocalizedText {
    ltext!(
        "errors.updates.notConfigured",
        "Updates are not configured in this build"
    )
}

fn app_pubkey_configured(app: &AppHandle) -> bool {
    let pubkey = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|c| c.get("pubkey"))
        .and_then(|k| k.as_str());
    pubkey_configured(pubkey)
}

fn updater(app: &AppHandle, url: Option<String>) -> AppResult<tauri_plugin_updater::Updater> {
    if !app_pubkey_configured(app) {
        return Err(AppError::localized(
            ErrorCode::InvalidInput,
            not_configured(),
        ));
    }
    // На Windows `install` запускает установщик и вызывает `process::exit(0)`,
    // минуя `RunEvent::Exit`: без хука открытые подключения не закрылись бы штатно. Хук заменяет
    // стандартный хук плагина, поэтому `cleanup_before_exit` вызываем сами. На macOS/Linux хук
    // не используется: там после установки `app.restart()` проходит Exit.
    let handle = app.clone();
    let mut b = app.updater_builder().on_before_exit(move || {
        handle.state::<AppState>().runtimes.shutdown();
        handle.cleanup_before_exit();
    });
    if let Some(u) = url {
        let bad_url = || {
            app_err!(
                ErrorCode::InvalidInput,
                "errors.updates.badPolicyUrl",
                "Invalid update address in the policy: {{url}}",
                url = u
            )
        };
        let parsed = u.parse().map_err(|_| bad_url())?;
        b = b
            .endpoints(vec![parsed])
            .map_err(|e| bad_url().with_details(e.to_string()))?;
    }
    b.build().map_err(|e| {
        app_err!(
            ErrorCode::Internal,
            "errors.updates.unavailable",
            "Updates are unavailable"
        )
        .with_details(e.to_string())
    })
}

use tauri_plugin_updater::Error as UpdErr;

/// Класс ошибки проверки обновлений.
#[derive(Debug, PartialEq, Eq)]
enum CheckErrorKind {
    /// Сервер ответил не-2xx (плагин сводит это к `ReleaseNotFound`): версий ещё нет.
    NoRelease,
    Network,
}

fn classify_check_error(e: &UpdErr) -> CheckErrorKind {
    match e {
        UpdErr::ReleaseNotFound => CheckErrorKind::NoRelease,
        _ => CheckErrorKind::Network,
    }
}

fn net_err(e: UpdErr) -> AppError {
    let details = e.to_string();
    match classify_check_error(&e) {
        CheckErrorKind::NoRelease => app_err!(
            ErrorCode::NotFound,
            "errors.updates.noRelease",
            "The update server has no published versions (or it is temporarily unavailable)"
        ),
        CheckErrorKind::Network => app_err!(
            ErrorCode::ConnectionFailed,
            "errors.updates.checkFailed",
            "Failed to check for updates"
        ),
    }
    .with_details(details)
}

/// Класс ошибки скачивания/установки.
#[derive(Debug, PartialEq, Eq)]
enum DownloadErrorKind {
    /// Сетевой сбой: скачивание можно повторить.
    Transient,
    /// Подпись не прошла проверку: повтор бессмыслен, ошибка сразу.
    Signature,
    Other,
}

fn classify_download_error(e: &UpdErr) -> DownloadErrorKind {
    match e {
        UpdErr::Reqwest(_) | UpdErr::Network(_) => DownloadErrorKind::Transient,
        UpdErr::Minisign(_)
        | UpdErr::Base64(_)
        | UpdErr::SignatureUtf8(_)
        | UpdErr::SignedVersionMismatch { .. }
        | UpdErr::MissingSignedVersion => DownloadErrorKind::Signature,
        _ => DownloadErrorKind::Other,
    }
}

const DOWNLOAD_ATTEMPTS: u32 = 3;

fn backoff(attempt: u32) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(attempt))
}

fn download_failed(e: &UpdErr) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.updates.downloadInterrupted",
        "The update download was interrupted — check your network and try again"
    )
    .with_details(e.to_string())
}

fn signature_failed(e: &UpdErr) -> AppError {
    app_err!(
        ErrorCode::Internal,
        "errors.updates.badSignature",
        "The update signature failed verification — the update was not installed"
    )
    .with_details(e.to_string())
}

fn install_failed(e: &impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::Internal,
        "errors.updates.installFailed",
        "Failed to install the update"
    )
    .with_details(e.to_string())
}

/// Скачивает пакет с повторами; повторяется только сетевая ошибка.
async fn download_with_retry(update: &tauri_plugin_updater::Update) -> AppResult<Vec<u8>> {
    let mut attempt = 1;
    loop {
        match update.download(|_, _| {}, || {}).await {
            Ok(bytes) => return Ok(bytes),
            Err(e) => match classify_download_error(&e) {
                DownloadErrorKind::Signature => return Err(signature_failed(&e)),
                DownloadErrorKind::Other => return Err(install_failed(&e)),
                DownloadErrorKind::Transient if attempt >= DOWNLOAD_ATTEMPTS => {
                    return Err(download_failed(&e));
                }
                DownloadErrorKind::Transient => {
                    tracing::warn!(attempt, "update download failed, retrying");
                    tokio::time::sleep(backoff(attempt)).await;
                    attempt += 1;
                }
            },
        }
    }
}

#[tauri::command]
pub async fn check_update(app: AppHandle, state: State<'_, AppState>) -> AppResult<UpdateCheck> {
    // Политика проверяется до сборки updater: при запрете — ни одного запроса к серверу.
    let url = match update_plan(&state.policy.status) {
        UpdatePlan::Disabled(reason) => return Ok(UpdateCheck::Disabled { reason }),
        UpdatePlan::Check(url) => url,
    };
    // Сборка без ключа updater (dev, пробный прогон) — штатное состояние, а не ошибка.
    if !app_pubkey_configured(&app) {
        return Ok(UpdateCheck::Disabled {
            reason: not_configured(),
        });
    }
    match updater(&app, url)?.check().await.map_err(net_err)? {
        None => Ok(UpdateCheck::UpToDate),
        Some(u) => Ok(UpdateCheck::Available {
            version: u.version.clone(),
            notes: u.body.clone(),
            notes_i18n: notes_i18n(&u.raw_json),
            date: u.date.and_then(|d| d.format(&Rfc3339).ok()),
        }),
    }
}

#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    let url = match update_plan(&state.policy.status) {
        UpdatePlan::Disabled(reason) => {
            return Err(AppError::localized(ErrorCode::InvalidInput, reason));
        }
        UpdatePlan::Check(url) => url,
    };
    let Some(update) = updater(&app, url)?.check().await.map_err(net_err)? else {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.updates.none",
            "No updates available"
        ));
    };
    tracing::info!(version = %update.version, "installing update");
    let bytes = download_with_retry(&update).await?;
    // Установка синхронная (файлы, на Windows — запуск установщика): не блокируем рантайм.
    let installer = update.clone();
    tauri::async_runtime::spawn_blocking(move || installer.install(bytes))
        .await
        .map_err(|e| install_failed(&e))?
        .map_err(|e| match classify_download_error(&e) {
            DownloadErrorKind::Signature => signature_failed(&e),
            _ => install_failed(&e),
        })?;
    // RunEvent::Exit закрывает открытые подключения перед перезапуском.
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_disables_check_without_network() {
        let st = PolicyStatus {
            managed: true,
            updates_enabled: false,
            ..PolicyStatus::default()
        };
        match update_plan(&st) {
            UpdatePlan::Disabled(reason) => assert_eq!(reason.key, "errors.updates.managed"),
            p => panic!("ожидалось Disabled, получено {p:?}"),
        }
    }

    #[test]
    fn policy_url_overrides_endpoint() {
        let st = PolicyStatus {
            managed: true,
            update_url: Some("https://mirror.corp/latest.json".into()),
            ..PolicyStatus::default()
        };
        match update_plan(&st) {
            UpdatePlan::Check(Some(url)) => assert_eq!(url, "https://mirror.corp/latest.json"),
            p => panic!("ожидалось Check(Some), получено {p:?}"),
        }
        assert!(matches!(
            update_plan(&PolicyStatus::default()),
            UpdatePlan::Check(None)
        ));
    }

    #[test]
    fn update_check_json_shape() {
        let v = serde_json::to_value(UpdateCheck::Available {
            version: "0.1.0".into(),
            notes: None,
            notes_i18n: BTreeMap::from([("ru".into(), "- Исправления".into())]),
            date: None,
        })
        .unwrap();
        assert_eq!(v["status"], "available");
        assert_eq!(v["version"], "0.1.0");
        assert!(v["notes"].is_null());
        assert_eq!(v["notesI18n"]["ru"], "- Исправления");
        assert_eq!(
            serde_json::to_value(UpdateCheck::UpToDate).unwrap()["status"],
            "upToDate"
        );
        let d = serde_json::to_value(UpdateCheck::Disabled {
            reason: not_configured(),
        })
        .unwrap();
        assert_eq!(d["status"], "disabled");
        assert_eq!(d["reason"]["key"], "errors.updates.notConfigured");
        assert_eq!(
            d["reason"]["text"],
            "Updates are not configured in this build"
        );
    }

    #[test]
    fn notes_i18n_from_raw_json() {
        let raw = serde_json::json!({
            "version": "0.4.0",
            "notes": "- fixes",
            "notes_i18n": { "en": "- fixes", "ru": "- Исправления", "de": "  ", "fr": 1, "ja": null },
            "platforms": {}
        });
        assert_eq!(
            notes_i18n(&raw),
            BTreeMap::from([
                ("en".to_owned(), "- fixes".to_owned()),
                ("ru".to_owned(), "- Исправления".to_owned()),
            ])
        );
        // Релизы без поля (0.2.0 и раньше) и испорченное поле — пустой набор, остаются обычные notes.
        assert!(notes_i18n(&serde_json::json!({ "notes": "- x" })).is_empty());
        assert!(notes_i18n(&serde_json::json!({ "notes_i18n": "- x" })).is_empty());
    }

    #[test]
    fn updater_accepts_latest_json_with_notes_i18n() {
        // Так latest.json читают и уже установленные версии: лишнее поле не ломает разбор, notes — как раньше.
        let raw = serde_json::json!({
            "version": "0.4.0",
            "notes": "- fixes",
            "notes_i18n": { "en": "- fixes", "ru": "- Исправления" },
            "pub_date": "2026-11-01T00:00:00Z",
            "platforms": { "darwin-aarch64": { "signature": "S", "url": "https://x/a.app.tar.gz" } }
        });
        let release: tauri_plugin_updater::RemoteRelease = serde_json::from_value(raw).unwrap();
        assert_eq!(release.notes.as_deref(), Some("- fixes"));
        assert_eq!(release.version.to_string(), "0.4.0");
    }

    #[test]
    fn placeholder_pubkey_is_not_configured() {
        assert!(!pubkey_configured(None));
        assert!(!pubkey_configured(Some("")));
        assert!(!pubkey_configured(Some(PUBKEY_PLACEHOLDER)));
        assert!(pubkey_configured(Some(
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWdu"
        )));
    }

    #[test]
    fn check_error_classification() {
        assert_eq!(
            classify_check_error(&UpdErr::ReleaseNotFound),
            CheckErrorKind::NoRelease
        );
        assert_eq!(
            classify_check_error(&UpdErr::EmptyEndpoints),
            CheckErrorKind::Network
        );
        let e = net_err(UpdErr::ReleaseNotFound);
        assert_eq!(e.code, ErrorCode::NotFound);
        assert_eq!(e.key, "errors.updates.noRelease");
        assert_eq!(
            net_err(UpdErr::Network("x".into())).code,
            ErrorCode::ConnectionFailed
        );
    }

    #[test]
    fn download_error_classification() {
        assert_eq!(
            classify_download_error(&UpdErr::Network("status 502".into())),
            DownloadErrorKind::Transient
        );
        for e in [
            UpdErr::SignatureUtf8("s".into()),
            UpdErr::MissingSignedVersion,
            UpdErr::SignedVersionMismatch {
                signed: "1".into(),
                announced: "2".into(),
            },
        ] {
            assert_eq!(classify_download_error(&e), DownloadErrorKind::Signature);
        }
        assert_eq!(
            classify_download_error(&UpdErr::InvalidUpdaterFormat),
            DownloadErrorKind::Other
        );
        let e = signature_failed(&UpdErr::MissingSignedVersion);
        assert_eq!(e.code, ErrorCode::Internal);
        assert_eq!(e.key, "errors.updates.badSignature");
        let e = download_failed(&UpdErr::Network("x".into()));
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.key, "errors.updates.downloadInterrupted");
        assert_eq!(backoff(1).as_secs(), 1);
        assert_eq!(backoff(2).as_secs(), 2);
    }

    #[test]
    fn shipped_config_points_to_the_releases_repo() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let up = &conf["plugins"]["updater"];
        // Заглушка допустима до ключа владельца: релиз с ней не пройдёт scripts/check-release.mjs.
        assert!(up["pubkey"].as_str().is_some_and(|k| !k.trim().is_empty()));
        assert_eq!(
            up["endpoints"][0],
            "https://github.com/vvnovg/glafira-postgres-releases/releases/latest/download/latest.json"
        );
        assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
        assert_eq!(
            conf["bundle"]["windows"]["wix"]["upgradeCode"],
            "a5dce14e-dd43-4ce6-9144-8437d8df8476"
        );
    }
}
