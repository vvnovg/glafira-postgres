//! Архив диагностики: пользователь сам прикладывает его к обращению. Наружу ничего не отправляется.
//! Секретов в источниках нет: профили не содержат паролей (они в keychain; `user:password@` из адресов
//! Patroni вырезается при записи архива), лицензия — без ключа.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use zip::write::SimpleFileOptions;

use crate::conn::profile::{ConnectionProfile, strip_patroni_url_credentials};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::license::LicenseStatus;
use crate::policy::PolicyStatus;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub commit: String,
    pub os: String,
    pub arch: String,
    pub data_dir: String,
    pub log_dir: String,
}

#[derive(Serialize)]
struct InfoFile<'a> {
    app: &'a AppInfo,
    policy: &'a PolicyStatus,
    license: &'a LicenseStatus,
}

fn zip_err(e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::Internal,
        "errors.diagnostics.writeFailed",
        "Failed to write the diagnostics archive"
    )
    .with_details(e.to_string())
}

/// Профили для архива: профили с диска не проходят проверку, и адрес Patroni с `user:password@`
/// мог остаться от старой версии; пароль в архив попасть не должен.
fn without_url_credentials(profiles: &[ConnectionProfile]) -> Vec<ConnectionProfile> {
    profiles
        .iter()
        .cloned()
        .map(|mut p| {
            strip_patroni_url_credentials(&mut p);
            p
        })
        .collect()
}

pub fn write_archive(
    out: &Path,
    info: &AppInfo,
    profiles: &[ConnectionProfile],
    policy: &PolicyStatus,
    license: &LicenseStatus,
    log_files: &[PathBuf],
) -> AppResult<()> {
    let file = std::fs::File::create(out)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default();
    zip.start_file("info.json", opts).map_err(zip_err)?;
    zip.write_all(&serde_json::to_vec_pretty(&InfoFile {
        app: info,
        policy,
        license,
    })?)?;
    zip.start_file("connections.json", opts).map_err(zip_err)?;
    zip.write_all(&serde_json::to_vec_pretty(&without_url_credentials(
        profiles,
    ))?)?;
    for path in log_files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            // Файл успел уйти при ротации между перечислением и чтением.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        zip.start_file(format!("logs/{name}"), opts)
            .map_err(zip_err)?;
        zip.write_all(&bytes)?;
    }
    zip.finish().map_err(zip_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{ConnectionKind, HostOverride, PatroniSettings, sample_profile};
    use crate::license::{LicenseManager, LicenseState, LicenseStatus, Tier};
    use crate::policy::PolicyStatus;
    use std::io::Read;

    fn info() -> AppInfo {
        AppInfo {
            name: "X".into(),
            version: "0.2.0".into(),
            commit: "abc".into(),
            os: "macos".into(),
            arch: "aarch64".into(),
            data_dir: "/d".into(),
            log_dir: "/l".into(),
        }
    }

    #[test]
    fn archive_contains_info_profiles_and_logs_without_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("app.log");
        std::fs::write(&log, "запуск\n").unwrap();
        let license = LicenseStatus {
            tier: Tier::Pro,
            state: LicenseState::Valid,
            source: None,
            licensee: Some("a@b".into()),
            kind: None,
            expires: None,
            days_left: None,
            error: None,
        };
        let out = dir.path().join("diag.zip");
        write_archive(
            &out,
            &info(),
            &[sample_profile("a")],
            &PolicyStatus::default(),
            &license,
            &[log],
        )
        .unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut names: Vec<_> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["connections.json", "info.json", "logs/app.log"]);
        let mut info_json = String::new();
        zip.by_name("info.json")
            .unwrap()
            .read_to_string(&mut info_json)
            .unwrap();
        assert!(
            info_json.contains("\"version\": \"0.2.0\"") && info_json.contains("\"tier\": \"pro\"")
        );
        let mut connections_json = String::new();
        zip.by_name("connections.json")
            .unwrap()
            .read_to_string(&mut connections_json)
            .unwrap();
        let profiles: Vec<ConnectionProfile> = serde_json::from_str(&connections_json).unwrap();
        assert_eq!(profiles, [sample_profile("a")]);
    }

    /// Читает `connections.json` из архива как текст.
    fn archived_connections(out: &Path) -> String {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(out).unwrap()).unwrap();
        let mut text = String::new();
        zip.by_name("connections.json")
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    }

    #[test]
    fn archive_strips_credentials_from_patroni_urls_of_legacy_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = sample_profile("p");
        legacy.kind = ConnectionKind::Patroni;
        legacy.hosts.clear();
        legacy.patroni = Some(PatroniSettings {
            urls: vec![
                "https://admin:s3cr3t@pg-1:8008".into(),
                "http://pg-2:8008/health?user=a@b".into(),
            ],
            username: Some("patroni".into()),
            tls: None,
            host_overrides: vec![HostOverride {
                member: "pg-1".into(),
                host: None,
                port: None,
                api_url: Some("https://admin:s3cr3t@10.0.0.1:8008".into()),
            }],
        });
        let out = dir.path().join("diag.zip");
        write_archive(
            &out,
            &info(),
            &[legacy.clone(), sample_profile("a")],
            &PolicyStatus::default(),
            &LicenseStatus {
                tier: Tier::Free,
                state: LicenseState::None,
                source: None,
                licensee: None,
                kind: None,
                expires: None,
                days_left: None,
                error: None,
            },
            &[],
        )
        .unwrap();

        let text = archived_connections(&out);
        assert!(
            !text.contains("s3cr3t") && !text.contains("admin:"),
            "{text}"
        );
        let profiles: Vec<ConnectionProfile> = serde_json::from_str(&text).unwrap();
        let pt = profiles[0].patroni.as_ref().unwrap();
        assert_eq!(
            pt.urls,
            ["https://pg-1:8008", "http://pg-2:8008/health?user=a@b"]
        );
        assert_eq!(
            pt.host_overrides[0].api_url.as_deref(),
            Some("https://10.0.0.1:8008")
        );
        // Остальное в профиле как было, второй профиль не тронут.
        assert_eq!(pt.username.as_deref(), Some("patroni"));
        assert_eq!(profiles[1], sample_profile("a"));
        // Исходные профили (хранилище) не изменились.
        assert!(legacy.patroni.unwrap().urls[0].contains("s3cr3t"));
    }

    #[test]
    fn missing_log_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("app.log");
        std::fs::write(&log, "x").unwrap();
        let out = dir.path().join("diag.zip");
        write_archive(
            &out,
            &info(),
            &[],
            &PolicyStatus::default(),
            &LicenseStatus {
                tier: Tier::Free,
                state: LicenseState::None,
                source: None,
                licensee: None,
                kind: None,
                expires: None,
                days_left: None,
                error: None,
            },
            &[dir.path().join("app.1.log"), log],
        )
        .unwrap();
        let zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        assert_eq!(zip.len(), 3);
    }

    #[test]
    fn archive_does_not_contain_license_token() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir(&data).unwrap();
        let token = license_format::dev::pro_token("a@b", None);
        let mgr = LicenseManager::open(&data, license_format::dev::verifying_key());
        mgr.set_user_key(&token).unwrap();
        let policy = PolicyStatus {
            has_license_key: true,
            ..PolicyStatus::default()
        };
        let out = dir.path().join("diag.zip");
        write_archive(
            &out,
            &info(),
            &[sample_profile("a")],
            &policy,
            &mgr.status(),
            &[],
        )
        .unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        for i in 0..zip.len() {
            let mut text = String::new();
            zip.by_index(i).unwrap().read_to_string(&mut text).unwrap();
            assert!(!text.contains(&token), "токен лицензии попал в архив");
            assert!(!text.to_lowercase().contains("password"));
        }
    }
}
