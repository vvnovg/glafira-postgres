# Этап «Открытый код» для Glafira for PostgreSQL (Apache 2.0, без ключей, «Поддержать проект», автообновление, первый релиз) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Перевести Glafira for PostgreSQL на политику Apache 2.0 без лицензионных ключей, добавить пункт «Поддержать проект» и автообновление, принести релизную обвязку из Glafira for Apache Kafka — так, чтобы после слияния ветки `feature/apache-2` можно было опубликовать репозиторий и выпустить подписанный релиз v0.1.0.

**Architecture:** Спека — `docs/superpowers/specs/2026-10-22-glafira-postgres-apache-2-design.md`. Подсистема ключей удаляется из Rust и фронтенда целиком; политика администратора остаётся (`updatesEnabled`, `updateUrl`, `language`). Updater переносится из `~/kafka-explorer` (`tauri-plugin-updater`, команды `check_update`/`install_update`, раздел «Обновления» в «О программе», уведомление над вкладками) без подтверждения сессий. Релизная обвязка (workflow `release.yml`, скрипты `check-release`, `make-latest-json`, `make-pkg`, `guide-publish`, `check-dist-compat`, `third-party`, чек-лист, документы репозитория релизов) копируется из Kafka и адаптируется. Образцы читаются по абсолютным путям из `/Users/vvnovg/kafka-explorer` (только чтение: ничего там не менять).

**Tech Stack:** Tauri 2, Rust (edition 2024, `cargo` в PATH через `source ~/.cargo/env`), React 19 + TypeScript, vitest, i18next (словари en и ru), node:test для `scripts/*.test.mjs`, GitHub Actions, cargo-deny 0.20.2, gitleaks.

## Global Constraints

- Рабочий каталог — `/Users/vvnovg/postgres-patroni-diag`, ветка `feature/apache-2` (от `main` 56e2485). Коммиты — на русском, с трейлером `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`. Коммитить только свои файлы (`git add <пути>`, не `git add -A`); никогда не коммитить `docs/licensing-review-2026-10-19.md` (в `.gitignore`), `.superpowers/` (в `.gitignore`), `*.glafira-postgres-connections.json`.
- Образцы Kafka: `/Users/vvnovg/kafka-explorer` — только чтение. Стенд Docker (`gp-*`) не останавливать (`docker compose down` запрещён), контейнер `ke-kafka` не трогать.
- Репозитории: код — `https://github.com/vvnovg/glafira-postgres`, релизы — `https://github.com/vvnovg/glafira-postgres-releases`. `SUPPORT_URL = "https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md"`, `LICENSE_URL = "https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE"`, адрес updater `https://github.com/vvnovg/glafira-postgres-releases/releases/latest/download/latest.json`. Рублёвый способ поддержки — `https://pay.cloudtips.ru/p/0eee5125`, GitHub Sponsors — `https://github.com/sponsors/vvnovg`.
- Версия остаётся `0.1.0` во всех трёх манифестах; `mainBinaryName` `glafira-pg`, `identifier` `io.github.vvnovg.glafira.postgres` не меняются. `bundle.windows.wix.upgradeCode = "a5dce14e-dd43-4ce6-9144-8437d8df8476"`. Заглушка ключа updater — строка `REPLACE_WITH_UPDATER_PUBKEY`.
- Правообладатель — «Viacheslav Novgorodtsev», копирайт `© 2026 Viacheslav Novgorodtsev`, `Copyright 2026 Viacheslav Novgorodtsev` в `NOTICE`.
- Языков два: en и ru. Каждый новый ключ словаря добавляется в оба `src/locales/{en,ru}.json`; `pnpm i18n:check` должен проходить после каждой задачи, где менялись словари, Rust-тексты или руководство.
- Тесты Rust не используют Unix-пути вида `/nonexistent/…` (на Windows не абсолютны): только `tempfile::tempdir()`.
- `cargo-deny` в CI закреплён: `cargo deny --version 2>/dev/null | grep -q ' 0\.20\.2$' || cargo install cargo-deny --locked --version 0.20.2 --force`. Локально установлена 0.20.2.
- Перед коммитом Rust-задач: `cargo fmt --manifest-path src-tauri/Cargo.toml` (и для `crates/pg-advisor`, если менялся), `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo test --manifest-path src-tauri/Cargo.toml --lib`. Перед коммитом фронтенд-задач: `pnpm test`, `npx tsc --noEmit`, `pnpm i18n:check`.
- Файлы в репозитории — с LF; Markdown на русском (кроме файлов для публичного GitHub: `LICENSE`, `NOTICE`, `TRADEMARK.md`, `CONTRIBUTING.md`, `SECURITY.md`, `THIRD_PARTY.md`, `docs/releases-repo/*` и английская часть `SUPPORT.md`, `docs/guide/en/*`).
- Комментарии в коде — по-русски; строки интерфейса — только через словарь (`t("…")`, `ltext!`, `app_err!`).

---

### Task 1: Rust без подсистемы ключей

**Files:**
- Delete: `src-tauri/src/license/mod.rs` (каталог `src-tauri/src/license/`), `src-tauri/src/commands/license.rs`, `src-tauri/license-public.key`, `src-tauri/license-dev-public.key`, `.cargo/config.toml`
- Create: `src-tauri/src/migrate.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/state.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/settings.rs`, `src-tauri/src/commands/app.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/commands/folders.rs`, `src-tauri/src/storage/exchange.rs`, `src-tauri/src/diagnostics.rs`, `src-tauri/src/error.rs`, `src-tauri/src/settings.rs`
- Test: тесты внутри тех же файлов (`#[cfg(test)]`)

**Interfaces:**
- Consumes: ничего из других задач.
- Produces: `crate::migrate::remove_legacy_license_file(data_dir: &Path)`; `commands::settings::get_policy_status(state) -> AppResult<PolicyStatus>`; `AppState` без поля `license`; `diagnostics::write_archive(out, info, profiles, policy, log_files)` (5 аргументов); `ImportOptions { target, update_existing }` (без `is_pro`); `ErrorCode` без `ProRequired`. Поле `Policy.license_key` и `KEY_LICENSE_KEY` в `policy/mod.rs` остаются до задачи 2 (их никто больше не читает).

- [ ] **Step 1: Убрать зависимость `license-format`, ключи и `.cargo/config.toml`**

```bash
cd /Users/vvnovg/postgres-patroni-diag
sed -i '' '/^license-format = /d' src-tauri/Cargo.toml
grep -c "license-format" src-tauri/Cargo.toml   # ожидается 0
git rm -q -r src-tauri/src/license src-tauri/src/commands/license.rs src-tauri/license-public.key src-tauri/license-dev-public.key .cargo/config.toml
ls .cargo 2>/dev/null || echo "каталог .cargo удалён"
```

- [ ] **Step 2: `src-tauri/build.rs` — только номер коммита**

Заменить содержимое файла целиком:

```rust
fn git_commit() {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_COMMIT={commit}");
    println!("cargo:rerun-if-changed=../.git/HEAD");
}

fn main() {
    git_commit();
    tauri_build::build()
}
```

- [ ] **Step 3: `src-tauri/src/migrate.rs` — удаление `license.json` при запуске (сначала тест)**

Создать файл:

```rust
//! Остатки прежних сборок в каталоге данных. Значения файлов в лог не пишутся.

use std::path::Path;

/// Файл лицензионного ключа сборок с подсистемой ключей: ключи больше не используются,
/// файл удаляется при запуске.
const LEGACY_LICENSE_FILE: &str = "license.json";

/// Удаляет `license.json` из каталога данных, если он остался от прежних сборок. Ошибка удаления
/// не мешает запуску: файл больше ни на что не влияет.
pub fn remove_legacy_license_file(data_dir: &Path) {
    let path = data_dir.join(LEGACY_LICENSE_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(path = %path.display(), "legacy license file removed"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), "legacy license file not removed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_license_file_is_removed_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.json");
        std::fs::write(&path, "{\"token\":\"a.b\"}").unwrap();
        remove_legacy_license_file(dir.path());
        assert!(!path.exists());
        // Повторный вызов без файла — тихий.
        remove_legacy_license_file(dir.path());
        assert!(!path.exists());
    }

    #[test]
    fn failure_to_remove_does_not_panic() {
        // Каталог на месте файла: remove_file даёт ошибку, отличную от NotFound; запуск продолжается.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.json");
        std::fs::create_dir(&path).unwrap();
        remove_legacy_license_file(dir.path());
        assert!(path.is_dir());
    }
}
```

- [ ] **Step 4: `src-tauri/src/lib.rs` — без модуля `license`, с `migrate`, без команд лицензии**

Заменить содержимое файла целиком:

```rust
#[macro_use]
pub mod i18n;
pub mod catalog;
pub mod commands;
pub mod conn;
pub mod diagnostics;
pub mod error;
pub mod logging;
pub mod migrate;
pub mod policy;
pub mod product;
pub mod readiness;
pub mod secrets;
pub mod settings;
pub mod state;
pub mod storage;
pub mod sync;
pub mod topology;

use std::sync::Arc;

use tauri::Manager;

use crate::secrets::KeyringStore;
use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            crate::logging::init(&app.path().app_log_dir()?);
            tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");
            let dir = app.path().app_data_dir()?;
            let policy = crate::policy::sources::load();
            tracing::info!(managed = policy.status.managed, "policy");
            let st = AppState::new(&dir, Arc::new(KeyringStore::default()))?.with_policy(policy);
            st.runtimes
                .set_sink(Arc::new(crate::commands::topology::TauriSink(
                    app.handle().clone(),
                )));
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::app::export_diagnostics,
            commands::catalog::list_databases,
            commands::catalog::list_schemas,
            commands::catalog::list_objects,
            commands::catalog::list_partitions,
            commands::catalog::list_extensions,
            commands::catalog::list_roles,
            commands::connections::list_connections,
            commands::connections::save_connection,
            commands::connections::delete_connection,
            commands::connections::parse_connection_string,
            commands::connections::test_connection,
            commands::folders::list_folders,
            commands::folders::create_folder,
            commands::folders::rename_folder,
            commands::folders::delete_folder,
            commands::folders::move_items,
            commands::folders::export_connections,
            commands::folders::preview_import,
            commands::folders::apply_import,
            commands::settings::get_policy_status,
            commands::settings::get_language_settings,
            commands::settings::set_language,
            commands::settings::get_ui_settings,
            commands::settings::set_ui_settings,
            commands::topology::connect,
            commands::topology::disconnect,
            commands::topology::get_topology,
            commands::topology::list_topology_events,
            commands::topology::set_catalog_member,
            commands::topology::get_member_facts,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event
                && let Some(st) = app.try_state::<AppState>()
            {
                st.runtimes.shutdown();
            }
        });
}
```

- [ ] **Step 5: `src-tauri/src/commands/mod.rs` и `commands/settings.rs` — `get_policy_status` переезжает в settings**

В `src-tauri/src/commands/mod.rs` удалить строку `pub mod license;`.

В `src-tauri/src/commands/settings.rs`:

1. Импорты в начале файла заменить на:

```rust
use tauri::State;

use super::blocking;
use crate::error::{AppResult, ErrorCode};
use crate::policy::PolicyStatus;
use crate::settings::{LanguageSettings, language_settings, system_locale};
use crate::state::AppState;
```

2. Перед `fn current(st: &AppState)` добавить команду:

```rust
/// Состояние политики администратора: прочитано один раз при запуске (`lib.rs`).
#[tauri::command]
pub fn get_policy_status(state: State<'_, AppState>) -> AppResult<PolicyStatus> {
    Ok(state.policy.status.clone())
}
```

3. В тестах удалить целиком тест `pro_can_pick_any_language` (от `#[test]` до закрывающей `}` включительно) и переименовать `fn free_can_pick_any_language()` в `fn user_can_pick_any_language()`. Добавить тест команды политики после `policy_language_blocks_change`:

```rust
    #[test]
    fn policy_status_is_the_one_from_startup() {
        let dir = tempfile::tempdir().unwrap();
        let mut policy = crate::policy::Policy::default();
        policy.status.managed = true;
        policy.status.source = Some("src".into());
        let st = state(dir.path()).with_policy(policy);
        assert!(st.policy.status.managed);
        assert_eq!(st.policy.status.source.as_deref(), Some("src"));
    }
```

- [ ] **Step 6: `src-tauri/src/state.rs` — без `LicenseManager`, с удалением `license.json`**

Заменить всё до `#[cfg(test)]` на:

```rust
use std::path::Path;
use std::sync::Arc;

use crate::conn::pool::{PoolRegistry, PooledClient};
use crate::error::AppResult;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;
use crate::storage::connections::{ConnectionProfile, ConnectionStore};
use crate::topology::runtime::{LiveSource, Runtimes};

#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<ConnectionStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub settings: Arc<SettingsStore>,
    /// Пулы соединений с узлами открытых подключений.
    pub pools: Arc<PoolRegistry>,
    /// Открытые подключения: топология, журнал событий, узел каталога.
    pub runtimes: Arc<Runtimes>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        crate::migrate::remove_legacy_license_file(data_dir);
        let pools = Arc::new(PoolRegistry::default());
        let runtimes = Runtimes::new(
            Arc::new(LiveSource::new(secrets.clone(), pools.clone())),
            pools.clone(),
        );
        Ok(Self {
            connections: Arc::new(ConnectionStore::open(data_dir)?),
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
            settings: Arc::new(SettingsStore::open(data_dir)),
            pools,
            runtimes,
        })
    }

    pub fn profile(&self, id: &str) -> AppResult<ConnectionProfile> {
        self.connections.get(id)
    }

    /// Соединение с узлом открытого подключения: `member = None` — узел каталога,
    /// `database = None` — БД по умолчанию профиля. Если запрос на этом соединении не удался из-за
    /// таймаута или обрыва, вызывающий закрывает его `PooledClient::discard`, а не возвращает в пул.
    pub async fn client(
        &self,
        connection_id: &str,
        member: Option<&str>,
        database: Option<&str>,
    ) -> AppResult<PooledClient> {
        let (m, profile) = self.runtimes.target(connection_id, member)?;
        let db = database
            .map(str::to_string)
            .unwrap_or_else(|| profile.pg.database.clone());
        crate::topology::connector::pooled_client(
            &self.pools,
            &profile,
            self.secrets.clone(),
            &m.name,
            &m.host,
            m.port,
            &db,
        )
        .await
    }

    /// Удаляет подключение и все его секреты (блокирующий вызов: keychain). Сначала секреты, потом профиль:
    /// если keychain откажет, подключение останется в дереве и удаление можно повторить, а не останутся
    /// «ничьи» пароли, которые подхватило бы подключение с тем же id из импортируемого файла. Отсутствующая
    /// запись keychain ошибкой не считается.
    pub fn delete_connection_full(&self, id: &str) -> AppResult<()> {
        self.runtimes.disconnect(id);
        for field in crate::secrets::CONNECTION_SECRET_FIELDS {
            self.secrets
                .delete(&crate::secrets::secret_key(id, field))?;
        }
        self.connections.delete(id)
    }

    /// Политика администратора (читается при запуске в lib.rs).
    pub fn with_policy(mut self, policy: crate::policy::Policy) -> Self {
        self.policy = Arc::new(policy);
        self
    }
}
```

В тестах (`pub(crate) mod tests`) удалить целиком тест `policy_license_key_reaches_manager` и на его место поставить:

```rust
    #[test]
    fn legacy_license_file_is_removed_on_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("license.json"), "{}").unwrap();
        AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        assert!(!dir.path().join("license.json").exists());
    }
```

- [ ] **Step 7: `src-tauri/src/commands/app.rs` — архив без лицензии**

В вызове `crate::diagnostics::write_archive(...)` удалить строку `&st.license.status(),`. Итоговый вызов:

```rust
    blocking(move || {
        crate::diagnostics::write_archive(
            std::path::Path::new(&path),
            &info,
            &st.connections.list(),
            &st.policy.status,
            &crate::logging::log_files(&log_dir),
        )
    })
    .await
```

- [ ] **Step 8: `src-tauri/src/diagnostics.rs` — `info.json` без `license`**

Заменить содержимое файла целиком:

```rust
//! Архив диагностики: пользователь сам прикладывает его к обращению. Наружу ничего не отправляется.
//! Секретов в источниках нет: профили не содержат паролей (они в keychain; `user:password@` из адресов
//! Patroni вырезается при записи архива).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use zip::write::SimpleFileOptions;

use crate::conn::profile::{ConnectionProfile, strip_patroni_url_credentials};
use crate::error::{AppError, AppResult, ErrorCode};
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
    log_files: &[PathBuf],
) -> AppResult<()> {
    let file = std::fs::File::create(out)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default();
    zip.start_file("info.json", opts).map_err(zip_err)?;
    zip.write_all(&serde_json::to_vec_pretty(&InfoFile { app: info, policy })?)?;
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
        let out = dir.path().join("diag.zip");
        write_archive(
            &out,
            &info(),
            &[sample_profile("a")],
            &PolicyStatus::default(),
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
        assert!(info_json.contains("\"version\": \"0.2.0\"") && info_json.contains("\"policy\""));
        // Подсистемы ключей нет: в info.json только app и policy.
        assert!(!info_json.to_lowercase().contains("license"), "{info_json}");
        let mut connections_json = String::new();
        zip.by_name("connections.json")
            .unwrap()
            .read_to_string(&mut connections_json)
            .unwrap();
        let profiles: Vec<ConnectionProfile> = serde_json::from_str(&connections_json).unwrap();
        assert_eq!(profiles, [sample_profile("a")]);
        for i in 0..zip.len() {
            let mut text = String::new();
            zip.by_index(i).unwrap().read_to_string(&mut text).unwrap();
            assert!(!text.to_lowercase().contains("password"), "{text}");
        }
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
                // Пароль с неэкранированным `/`: по RFC это путь, но адрес всё равно очищается.
                "http://admin:s3/cr3t@pg-2:8008".into(),
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
        assert_eq!(pt.urls, ["https://pg-1:8008", "http://pg-2:8008"]);
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
            &[dir.path().join("app.1.log"), log],
        )
        .unwrap();
        let zip = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        assert_eq!(zip.len(), 3);
    }
}
```

- [ ] **Step 9: `src-tauri/src/error.rs` — без `ProRequired`**

Удалить строку `    ProRequired,` из `enum ErrorCode`. Других упоминаний в файле нет.

- [ ] **Step 10: `src-tauri/src/commands/connections.rs` — prod без ключа**

1. Импорт в начале файла:

```rust
use crate::conn::profile::{ConnectionProfile, normalize_profile, same_connection_settings};
```

2. В `save_connection_blocking` удалить блок от комментария `// Перевод в prod и снятие read-only с prod — функции Pro (как в Glafira for Kafka).` до `st.license.require_pro()?;\n    }` включительно, чтобы осталось:

```rust
    let previous = st.connections.get(&profile.id).ok();
    let saved = st.connections.upsert(profile)?;
```

3. В тестах: импорт `use crate::conn::profile::{CertFormat, ClientCert, sample_profile};` заменить на `use crate::conn::profile::{CertFormat, ClientCert, Environment, sample_profile};`. Тест `prod_requires_pro` заменить на:

```rust
    #[test]
    fn prod_is_saved_and_made_writable_without_a_key() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("a");
        p.environment = Environment::Prod;
        p.read_only = true;
        let saved = save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap();
        assert_eq!(saved.environment, Environment::Prod);
        assert!(saved.read_only);
        // Снять «только чтение» с prod можно: тарифов нет.
        let mut writable = saved;
        writable.read_only = false;
        let saved = save_connection_blocking(&st, input(writable, SecretsInput::default())).unwrap();
        assert!(!saved.read_only);
        assert!(!st.connections.get("a").unwrap().read_only);
    }
```

- [ ] **Step 11: `src-tauri/src/storage/exchange.rs` — импорт сохраняет `readOnly` из файла**

1. Импорт в начале файла:

```rust
use crate::conn::profile::{
    ConnectionProfile, normalize_profile, same_connection_settings, strip_patroni_url_credentials,
    validate_profile,
};
```

2. `pub struct ImportOptions<'a>` — удалить поле `pub is_pro: bool,`:

```rust
pub struct ImportOptions<'a> {
    /// Папка, под которую попадают корневые элементы файла; `None` — корень.
    pub target: Option<&'a str>,
    pub update_existing: bool,
}
```

3. В `plan_import` (цикл по подключениям): удалить `let mut prod_read_only = 0usize;`; удалить строки `let forced_read_only = !opts.is_pro && profile.environment == Environment::Prod;` и `if forced_read_only { profile.read_only = true; }`; две строки комментария `// Во Free совпадение с файлом «как есть» — тоже «без изменений»: иначе локальный` / `// prod без readOnly выглядел бы обновлённым при каждом импорте того же файла.` и строку `let unchanged_settings = same_settings(slot, incoming) || same_settings(slot, &profile);` заменить на `let unchanged_settings = same_settings(slot, &profile);`; удалить обе строки `prod_read_only += usize::from(forced_read_only);`; удалить блок

```rust
    if prod_read_only > 0 {
        preview.warnings.push(ltext!(
            "warnings.import.prodReadOnly",
            "{{count}} prod connections will be read-only: changing prod connections requires Pro",
            count = prod_read_only
        ));
    }
```

4. В тестах: импорт `use crate::conn::profile::{ApiTls, CertFormat, ClientCert, ConnectionKind, HostOverride, PatroniSettings, sample_profile};` дополнить `Environment` (между `ConnectionKind` и `HostOverride`). Помощник `opts`:

```rust
    fn opts(target: Option<&str>) -> ImportOptions<'_> {
        ImportOptions {
            target,
            update_existing: true,
        }
    }
```

Три теста `prod_becomes_read_only_in_free`, `prod_update_in_free_is_forced_read_only`, `identical_prod_in_free_is_unchanged_and_keeps_read_only` заменить на:

```rust
    #[test]
    fn prod_read_only_comes_from_file() {
        let mut prod = conn_json("p", "P", None);
        prod["environment"] = "prod".into();
        let mut ro = conn_json("r", "R", None);
        ro["environment"] = "prod".into();
        ro["readOnly"] = true.into();
        let bytes = file(vec![], vec![prod, ro, conn_json("d", "D", None)]);
        let p = plan(&StoreData::default(), &bytes, &opts(None));
        assert!(!conn(&p.next, "p").read_only);
        assert!(conn(&p.next, "r").read_only);
        assert!(!conn(&p.next, "d").read_only);
        assert!(p.preview.warnings.is_empty());
    }

    #[test]
    fn prod_update_keeps_read_only_from_file() {
        let mut local = sample_profile("p");
        local.environment = Environment::Prod;
        local.read_only = true;
        let data = StoreData {
            connections: vec![local],
            ..Default::default()
        };
        let mut incoming = conn_json("p", "renamed", None);
        incoming["environment"] = "prod".into();
        let p = plan(&data, &file(vec![], vec![incoming]), &opts(None));
        assert_eq!(preview_ids(&p.preview.updated), ["p"]);
        assert!(
            !conn(&p.next, "p").read_only,
            "readOnly берётся из файла, а не из локального профиля"
        );
        assert!(p.preview.warnings.is_empty());
    }

    #[test]
    fn identical_prod_is_unchanged() {
        let mut local = sample_profile("p");
        local.environment = Environment::Prod;
        local.read_only = false;
        let data = StoreData {
            connections: vec![local.clone()],
            ..Default::default()
        };
        let mut same = conn_json("p", "conn-p", None);
        same["environment"] = "prod".into();
        let p = plan(&data, &file(vec![], vec![same]), &opts(None));
        assert_eq!(preview_ids(&p.preview.unchanged), ["p"]);
        assert!(p.preview.updated.is_empty());
        assert!(p.preview.warnings.is_empty());
        assert_eq!(conn(&p.next, "p"), &local);
    }
```

Проверить, что в файле не осталось `is_pro`, `prod_read_only`, `forced_read_only`, `prodReadOnly`:

```bash
grep -n "is_pro\|prod_read_only\|forced_read_only\|prodReadOnly" src-tauri/src/storage/exchange.rs || echo "чисто"
```

- [ ] **Step 12: `src-tauri/src/commands/folders.rs` — импорт без тарифа**

1. Удалить строку `use crate::license::Tier;`.
2. В `import_from_file` блок `let opts = ImportOptions { ... };` заменить на:

```rust
    let opts = ImportOptions {
        target,
        update_existing,
    };
```

3. В тестах удалить помощник `pro_state` вместе с doc-комментарием (`/// Состояние с бессрочной Pro-лицензией …` до закрывающей `}`). В тесте `preview_does_not_write_and_apply_writes_once`: комментарий `// Free: prod-подключение станет только для чтения; у `a` меняется адрес.` заменить на `// У `a` меняется адрес: пароль получателя будет удалён.`; ожидание ключей предупреждений заменить на

```rust
        let keys: Vec<_> = preview.warnings.iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, ["warnings.import.passwordsReset"]);
```

и строку `assert!(st.connections.get("n").unwrap().read_only);` заменить на `assert!(!st.connections.get("n").unwrap().read_only, "readOnly как в файле");`.

4. Тесты `pro_keeps_prod_writable_on_import` и `free_cannot_lift_read_only_of_imported_prod_profile` заменить на один:

```rust
    #[test]
    fn imported_prod_profile_keeps_read_only_from_file_and_can_change_it() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let input = tempfile::tempdir().unwrap();
        let path = write_file(input.path(), "changed");
        let preview = import_from_file(&st, &path, None, true, true).unwrap();
        assert!(
            preview
                .warnings
                .iter()
                .all(|w| w.key != "warnings.import.prodReadOnly")
        );
        let mut imported = st.connections.get("n").unwrap();
        assert!(!imported.read_only, "readOnly как в файле");
        // Флажок можно поставить и снять: тарифов нет.
        imported.read_only = true;
        crate::commands::connections::save_connection_blocking(
            &st,
            crate::commands::connections::ConnectionInput {
                profile: imported,
                secrets: Default::default(),
            },
        )
        .unwrap();
        assert!(st.connections.get("n").unwrap().read_only);
        let mut back = st.connections.get("n").unwrap();
        back.read_only = false;
        crate::commands::connections::save_connection_blocking(
            &st,
            crate::commands::connections::ConnectionInput {
                profile: back,
                secrets: Default::default(),
            },
        )
        .unwrap();
        assert!(!st.connections.get("n").unwrap().read_only);
    }
```

- [ ] **Step 13: `src-tauri/src/settings.rs` — имя теста без лицензии**

Переименовать `fn priority_without_license()` в `fn language_priority()`.

- [ ] **Step 14: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
grep -rn "license\|ProRequired\|is_pro\|require_pro" src-tauri/src --include=*.rs | grep -v "migrate.rs\|legacy_license\|license.json\|policy/mod.rs\|policy/sources.rs" || echo "в Rust остались только migrate и policy (задача 2)"
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml --tests --no-run
```

Ожидается: clippy без предупреждений; `test result: ok` (около 325 тестов, 0 failed); интеграционные тесты компилируются. `Cargo.lock` изменился (ушли `license-format` и его зависимости) — это ожидаемо.

- [ ] **Step 15: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/build.rs src-tauri/src
git commit -q -F - <<'MSG'
refactor(license): подсистема лицензионных ключей удалена из Rust

Модуль license, команды ключа, зависимость license-format и .cargo/config.toml, открытые ключи и проверка
ключа в build.rs удалены. prod-подключения сохраняются и импортируются без тарифа, readOnly берётся из файла.
Архив диагностики без поля license. Оставшийся license.json удаляется при запуске (migrate.rs).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 2: Политика без `licenseKey`

**Files:**
- Modify: `src-tauri/src/policy/mod.rs`, `src-tauri/src/policy/sources.rs`, `docs/admin/policy.json`, `docs/admin/policy.reg`, `docs/admin/policy.mobileconfig`
- Test: тесты внутри `policy/mod.rs` и `policy/sources.rs`

**Interfaces:**
- Consumes: `AppState::with_policy(Policy)` из задачи 1 (только `status`).
- Produces: `Policy { status: PolicyStatus }`; `PolicyStatus` без `has_license_key` (сериализуется в `PolicyStatus` фронтенда без `hasLicenseKey` — задача 3); примеры политики с `updatesEnabled`, `updateUrl`, `language` (их читает тест `documented_examples_parse` и документация задачи 11).

- [ ] **Step 1: Тесты политики — сначала ожидания**

В `src-tauri/src/policy/mod.rs`, модуль `tests`:

1. В `absent_policy_is_unmanaged_with_updates` удалить строку `assert_eq!(p.license_key, None);`.
2. `values_are_applied` заменить на:

```rust
    #[test]
    fn values_are_applied() {
        let p = resolve(
            "src",
            values(&[
                ("updatesEnabled", PolicyValue::Bool(false)),
                (
                    "updateUrl",
                    PolicyValue::Str("https://mirror.corp/latest.json".into()),
                ),
                ("language", PolicyValue::Str("en".into())),
            ]),
        );
        assert!(p.status.managed);
        assert_eq!(p.status.source.as_deref(), Some("src"));
        assert!(!p.status.updates_enabled);
        assert_eq!(
            p.status.update_url.as_deref(),
            Some("https://mirror.corp/latest.json")
        );
        assert_eq!(p.status.language.as_deref(), Some("en"));
        assert!(p.status.errors.is_empty());
    }
```

3. Тест `license_key_is_not_serialized` заменить на:

```rust
    #[test]
    fn legacy_license_key_is_ignored() {
        // Ключ политик прежних сборок: неизвестный ключ — предупреждение в лог; соседние ключи применяются,
        // обновления не выключаются, в интерфейс значение не уходит.
        let p = resolve(
            "src",
            values(&[
                ("licenseKey", PolicyValue::Str("secret.key".into())),
                ("language", PolicyValue::Str("en".into())),
            ]),
        );
        assert!(p.status.managed);
        assert!(p.status.updates_enabled);
        assert!(p.status.errors.is_empty());
        assert_eq!(p.status.language.as_deref(), Some("en"));
        let json = serde_json::to_string(&p.status).unwrap();
        assert!(!json.contains("secret.key"));
        assert!(!json.to_lowercase().contains("license"));
        // Нестроковое значение — тоже просто неизвестный ключ, а не ошибка политики.
        let p = resolve("src", values(&[("licenseKey", PolicyValue::Bool(true))]));
        assert!(p.status.updates_enabled);
        assert!(p.status.errors.is_empty());
    }
```

В `src-tauri/src/policy/sources.rs`, тест `plist_file`: строку `  <key>licenseKey</key><string>a.b</string>` заменить на `  <key>updateUrl</key><string>https://mirror.corp/latest.json</string>`, а `assert_eq!(v["licenseKey"], PolicyValue::Str("a.b".into()));` — на `assert_eq!(v["updateUrl"], PolicyValue::Str("https://mirror.corp/latest.json".into()));`. В `documented_examples_parse`: строку `assert!(p.status.has_license_key);` заменить на

```rust
        assert!(!p.status.updates_enabled);
        assert_eq!(
            p.status.update_url.as_deref(),
            Some("https://mirror.example.corp/glafira-postgres/latest.json")
        );
```

цикл `for key in ["licenseKey", "language"]` заменить на `for key in ["updatesEnabled", "updateUrl", "language"]`, а после проверки `reg.contains(...)` раздела реестра добавить:

```rust
        for line in [
            "\"updatesEnabled\"=dword:00000000",
            "\"updateUrl\"=\"https://mirror.example.corp/glafira-postgres/latest.json\"",
            "\"language\"=\"en\"",
        ] {
            assert!(reg.contains(line), "{line}");
        }
```

- [ ] **Step 2: Запустить тесты — должны упасть**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
cargo test --manifest-path src-tauri/Cargo.toml --lib policy 2>&1 | tail -20
```

Ожидается: компиляция проходит, падают `legacy_license_key_is_ignored` (в JSON есть `hasLicenseKey`) и `documented_examples_parse` (примеры ещё старые).

- [ ] **Step 3: `src-tauri/src/policy/mod.rs` — удалить ключ**

1. Удалить строку `pub const KEY_LICENSE_KEY: &str = "licenseKey";`.
2. В `PolicyStatus` удалить поле `pub has_license_key: bool,` и в `impl Default for PolicyStatus` — строку `has_license_key: false,`.
3. `Policy` заменить на:

```rust
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub status: PolicyStatus,
}
```

4. В `resolve`: в ветке `RawPolicy::Unreadable(e)` удалить `license_key: None,` (остаётся `return Policy { status: PolicyStatus { ... } };`); удалить `let mut license_key = None;`; удалить ветку

```rust
            (KEY_LICENSE_KEY, PolicyValue::Str(s)) if !s.trim().is_empty() => {
                license_key = Some(s.trim().to_string());
                status.has_license_key = true;
            }
```

ветку `(KEY_UPDATES_ENABLED | KEY_UPDATE_URL | KEY_LICENSE_KEY, other) => {` заменить на `(KEY_UPDATES_ENABLED | KEY_UPDATE_URL, other) => {`; завершение функции заменить на

```rust
    if broken {
        status.updates_enabled = false;
    }
    Policy { status }
```

- [ ] **Step 4: Примеры политики**

`docs/admin/policy.json`:

```json
{
  "updatesEnabled": false,
  "updateUrl": "https://mirror.example.corp/glafira-postgres/latest.json",
  "language": "en"
}
```

`docs/admin/policy.reg` (файл с CRLF; записать через `printf`, чтобы не зависеть от редактора):

```bash
printf 'Windows Registry Editor Version 5.00\r\n\r\n[HKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\Glafira\\PostgreSQL]\r\n"updatesEnabled"=dword:00000000\r\n"updateUrl"="https://mirror.example.corp/glafira-postgres/latest.json"\r\n"language"="en"\r\n' > docs/admin/policy.reg
```

`docs/admin/policy.mobileconfig`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>PayloadContent</key>
  <array>
    <dict>
      <key>PayloadType</key><string>io.github.vvnovg.glafira.postgres</string>
      <key>PayloadIdentifier</key><string>io.github.vvnovg.glafira.postgres.policy</string>
      <key>PayloadUUID</key><string>00000000-0000-0000-0000-000000000011</string>
      <key>PayloadVersion</key><integer>1</integer>
      <key>updatesEnabled</key><false/>
      <key>updateUrl</key><string>https://mirror.example.corp/glafira-postgres/latest.json</string>
      <key>language</key><string>en</string>
    </dict>
  </array>
  <key>PayloadDisplayName</key><string>Glafira for PostgreSQL policy</string>
  <key>PayloadIdentifier</key><string>io.github.vvnovg.glafira.postgres.profile</string>
  <key>PayloadScope</key><string>System</string>
  <key>PayloadType</key><string>Configuration</string>
  <key>PayloadUUID</key><string>00000000-0000-0000-0000-000000000012</string>
  <key>PayloadVersion</key><integer>1</integer>
</dict>
</plist>
```

- [ ] **Step 5: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
grep -rn "licenseKey\|license_key\|has_license_key\|KEY_LICENSE_KEY" src-tauri/src docs/admin | grep -v "legacy_license_key_is_ignored\|unknown policy key\|(\"licenseKey\"" || echo "чисто"
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib
```

Ожидается: grep находит `licenseKey` только в тесте `legacy_license_key_is_ignored` (две строки `("licenseKey", …)`); все тесты зелёные.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/policy docs/admin
git commit -q -F - <<'MSG'
feat(policy): ключ licenseKey больше не разбирается — неизвестный ключ с предупреждением в логе

Примеры политики для трёх ОС показывают updatesEnabled, updateUrl и language.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 3: Фронтенд без подсистемы ключей и замков Pro

**Files:**
- Delete: `src/components/license/LicensePanel.tsx`, `src/components/license/LicensePanel.test.tsx`, `src/components/license/LicenseNotice.tsx` (весь каталог `src/components/license/`), `src/components/common/ProLock.tsx`, `src/lib/useLicense.ts`
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/state/workspace.ts`, `src/components/Layout.tsx`, `src/components/TabContent.tsx`, `src/components/Sidebar.tsx`, `src/components/about/AboutPanel.tsx`, `src/components/common/ErrorBanner.tsx`, `src/components/connections/ConnectionForm.tsx`, `src/styles.css`, `src/locales/en.json`, `src/locales/ru.json`, `docs/i18n/same-as-en.json`
- Test: `src/api/client.test.ts`, `src/state/workspace.test.ts`, `src/components/Sidebar.test.tsx`, `src/components/about/AboutPanel.test.tsx`, `src/components/common/ErrorBanner.test.tsx`, `src/components/connections/ConnectionForm.test.tsx`, `src/components/connections/ConnectionFormTab.test.tsx`, `src/components/connections/tree/ConnectionTree.test.tsx`, `src/components/connections/tree/ImportDialog.test.tsx`

**Interfaces:**
- Consumes: Rust без команд лицензии (задача 1) и `PolicyStatus` без `hasLicenseKey` (задача 2).
- Produces: `TabSpec` без `license`; `PolicyStatus` фронтенда `{ managed, source, updatesEnabled, updateUrl, language, errors }`; `ErrorCode` без `PRO_REQUIRED`; `Sidebar` с тремя ссылками (Настройки, Справка, О программе — «Поддержать проект» добавит задача 7); `AboutPanel` без строки «Тариф» (строку «Лицензия» и раздел «Обновления» добавят задачи 6–7); словари без ключей лицензии.

- [ ] **Step 1: Тесты — сначала ожидания**

`src/components/common/ErrorBanner.test.tsx` — последний тест (`ошибка без ключа (старый формат) — message`) заменить на:

```tsx
  it("ошибка без ключа (старый формат) — message, кнопок нет", () => {
    render(<ErrorBanner error={{ code: "INVALID_INPUT", message: "Нужно имя", retryable: false }} />);
    expect(screen.getByText("Нужно имя")).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
```

`src/components/Sidebar.test.tsx`: удалить строку `getLicenseStatus: vi.fn(() => new Promise(() => {})),` из мока; в тест добавить после `expect(screen.getByText("Подключения")).toBeInTheDocument();`:

```tsx
    expect(screen.queryByRole("button", { name: /^Лицензия/ })).not.toBeInTheDocument();
```

`src/components/about/AboutPanel.test.tsx`: удалить строку мока `getLicenseStatus: vi.fn().mockResolvedValue({ tier: "free", … })`, из обоих объектов `getPolicyStatus` убрать поле `hasLicenseKey: false,`; в первом тесте удалить строку `expect(screen.getByText("Free")).toBeInTheDocument();` и добавить на её место `expect(screen.queryByText("Тариф")).not.toBeInTheDocument();`.

`src/components/connections/ConnectionForm.test.tsx`:

1. В строке 4 убрать `LicenseStatus, ` из импорта типов.
2. Из мока `api` убрать строку `getLicenseStatus: vi.fn(),`.
3. Удалить строки `const FREE = { tier: "free", state: "none" } as LicenseStatus;` и `const PRO = { tier: "pro", state: "valid" } as LicenseStatus;`.
4. В `beforeEach` удалить `vi.mocked(api.getLicenseStatus).mockResolvedValue(FREE);`.
5. Тест `prod недоступен во Free` заменить на:

```tsx
  it("prod доступен новому подключению и подписан просто «prod»", async () => {
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    const prod = await screen.findByRole("option", { name: "prod" });
    expect(prod).toBeEnabled();
    expect(screen.queryByRole("option", { name: /Pro/ })).not.toBeInTheDocument();
    expect(screen.queryByText("🔒 Pro")).not.toBeInTheDocument();
  });
```

6. Тест `prod, только чтение и Free: флаг заблокирован, пока окружение остаётся prod` заменить на:

```tsx
  it("у существующего prod-подключения «Только чтение» доступно и снимается", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), id: "c1", name: "p", environment: "prod", readOnly: true }} tabId="t" />);
    const ro = screen.getByLabelText("Только чтение");
    expect(ro).toBeChecked();
    expect(ro).toBeEnabled();
    await user.click(ro);
    expect(ro).not.toBeChecked();
  });
```

7. Блок `describe("Pro и prod", …)` заменить целиком на:

```tsx
  describe("prod", () => {
    it("при выборе prod для нового подключения «только чтение» включается, но его можно снять", async () => {
      const user = userEvent.setup();
      vi.mocked(api.saveConnection).mockResolvedValue(saved("n"));
      renderWithClient(<ConnectionForm initial={{ ...newProfile(), name: "n" }} tabId="t" />);
      expect(screen.getByRole("option", { name: "prod" })).toBeEnabled();
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      const ro = screen.getByLabelText("Только чтение");
      expect(ro).toBeChecked();
      expect(ro).toBeEnabled();
      await user.click(ro);
      expect(ro).not.toBeChecked();
      await user.click(screen.getByRole("button", { name: "Сохранить" }));
      const profile = vi.mocked(api.saveConnection).mock.calls[0][0];
      expect(profile).toMatchObject({ environment: "prod", readOnly: false });
    });

    it("у существующего подключения переход в prod не навязывает «только чтение»", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      expect(screen.getByLabelText("Только чтение")).not.toBeChecked();
    });

    it("у существующего подключения возврат в prod не трогает снятый флажок", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ environment: "prod", readOnly: true })} tabId="t" />);
      const ro = screen.getByLabelText("Только чтение");
      await user.selectOptions(screen.getByLabelText("Окружение"), "dev");
      await user.click(ro);
      expect(ro).not.toBeChecked();
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      expect(ro).not.toBeChecked();
      expect(ro).toBeEnabled();
    });
  });
```

`src/components/connections/ConnectionFormTab.test.tsx`: из мока удалить строку `getLicenseStatus: vi.fn(() => Promise.resolve({ tier: "pro", state: "valid" })),`; тест `копия prod-подключения во Free: prod выбрать нельзя (создание prod — Pro)` заменить на:

```tsx
  it("копия prod-подключения: prod доступен, «Только чтение» как у источника", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([{ ...newProfile(), id: "p1", name: "billing", environment: "prod", readOnly: true }]);
    renderWithClient(<ConnectionFormTab connectionId={null} copyOf="p1" tabId="connection-form:copy:p1" />);
    expect(await screen.findByRole("heading", { name: "Копия подключения «billing»" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "prod" })).toBeEnabled();
    expect(screen.getByLabelText("Только чтение")).toBeChecked();
  });
```

`src/components/connections/tree/ConnectionTree.test.tsx`: удалить строку `getLicenseStatus: vi.fn(() => new Promise(() => {})),`.

`src/components/connections/tree/ImportDialog.test.tsx`: в `PREVIEW` строку `warnings: [{ key: "warnings.import.prodReadOnly", … }],` заменить на

```tsx
  warnings: [{ key: "warnings.import.missingFiles", params: { count: "2" }, text: "Certificate files of 2 connections are not found on this computer — check the paths" }],
```

и ожидание `expect(await screen.findByText(/2 prod-подключения будут только для чтения/)).toBeInTheDocument();` — на `expect(await screen.findByText(/Файлы сертификатов 2 подключений не найдены/)).toBeInTheDocument();`.

`src/api/client.test.ts`: удалить тест `setLicenseKey передаёт ключ` целиком; в тесте `команды без аргументов вызываются по имени` удалить четыре строки про `getLicenseStatus` и `removeLicenseKey`.

`src/state/workspace.test.ts`: удалить строку `expect(tabId({ kind: "license" })).toBe("license");`.

- [ ] **Step 2: Запустить тесты — должны упасть на отсутствующих ожиданиях**

```bash
cd /Users/vvnovg/postgres-patroni-diag && pnpm test 2>&1 | tail -15
```

Ожидается: падают тесты `ConnectionForm`/`ConnectionFormTab` (prod ещё отключён без статуса лицензии), `ErrorBanner` (кнопка «Открыть «Лицензия»» есть), `AboutPanel` («Тариф» есть).

- [ ] **Step 3: Удалить файлы подсистемы**

```bash
git rm -q -r src/components/license src/components/common/ProLock.tsx src/lib/useLicense.ts
```

- [ ] **Step 4: Типы, клиент, вкладки**

`src/api/types.ts`: удалить строку `  | "PRO_REQUIRED"` из `ErrorCode`; удалить блок

```ts
export type Tier = "free" | "pro";
export type LicenseState = "none" | "valid" | "expiring" | "expired" | "invalid";
export interface LicenseStatus {
  tier: Tier;
  state: LicenseState;
  source: "policy" | "user" | null;
  licensee: string | null;
  kind: "personal" | "organization" | null;
  expires: string | null;
  daysLeft: number | null;
  /** Почему ключ недействителен — полная фраза. */
  error: LocalizedText | null;
}
```

и строку `  hasLicenseKey: boolean;` из `PolicyStatus`.

`src/api/client.ts`: удалить `  LicenseStatus,` из импорта типов и три строки

```ts
  getLicenseStatus: () => call<LicenseStatus>("get_license_status"),
  setLicenseKey: (key: string) => call<LicenseStatus>("set_license_key", { key }),
  removeLicenseKey: () => call<LicenseStatus>("remove_license_key"),
```

`src/state/workspace.ts`: удалить `  | { kind: "license" }` из `TabSpec` и две строки `    case "license":` / `      return "license";` из `tabId`.

`src/components/Layout.tsx`: удалить `import { LicenseNotice } from "./license/LicenseNotice";`, две строки `    case "license":` / `      return i18n.t("tabs.license");` в `tabTitle` и строку `        <LicenseNotice />`.

`src/components/TabContent.tsx`: удалить `import { LicensePanel } from "./license/LicensePanel";` и две строки `    case "license":` / `      return <LicensePanel />;`.

- [ ] **Step 5: `ErrorBanner.tsx`, `Sidebar.tsx`, `AboutPanel.tsx`, `ConnectionForm.tsx`, `styles.css`**

`src/components/common/ErrorBanner.tsx` целиком:

```tsx
import { useTranslation } from "react-i18next";
import { toAppError } from "../../api/client";
import { tr } from "../../i18n/text";

export function ErrorBanner({ error, onRetry }: { error: unknown; onRetry?: () => void }) {
  const { t } = useTranslation();
  const e = toAppError(error);
  return (
    <div className="error-banner" role="alert">
      <div className="error-title">
        <strong>{tr(e)}</strong> <span className="muted">({e.code})</span>
        {onRetry && (
          <button type="button" onClick={onRetry}>
            {t("common.retry")}
          </button>
        )}
      </div>
      {e.details && (
        <details>
          <summary>{t("common.details")}</summary>
          <pre>{e.details}</pre>
        </details>
      )}
    </div>
  );
}
```

`src/components/Sidebar.tsx` целиком:

```tsx
import { useTranslation } from "react-i18next";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);

  return (
    <aside className="sidebar">
      <ConnectionTree />
      <div className="sidebar-footer">
        <button type="button" className="link" onClick={() => open({ kind: "settings" })}>
          {t("sidebar.settings")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "help" })}>
          {t("sidebar.help")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "about" })}>
          {t("sidebar.about")}
        </button>
      </div>
    </aside>
  );
}
```

`src/components/about/AboutPanel.tsx`: удалить строку `  const license = useQuery({ queryKey: ["license"], queryFn: api.getLicenseStatus });` и строку `        <div><dt>{t("about.tier")}</dt><dd>{license.data ? (license.data.tier === "pro" ? "Pro" : "Free") : "…"}</dd></div>`.

`src/components/connections/ConnectionForm.tsx`:

1. Удалить импорты `import { proHint, useLicense } from "../../lib/useLicense";` и `import { ProLock } from "../common/ProLock";`.
2. Удалить строки `  const { locked } = useLicense();` и `  const readOnlyLocked = locked && initial.environment === "prod" && initial.readOnly && profile.environment === "prod";`.
3. Элемент `<select id={`${uid}-env`} …>…</select>` заменить на:

```tsx
        <select
          id={`${uid}-env`}
          value={profile.environment}
          onChange={(e) => {
            const environment = e.target.value as Environment;
            // Новое подключение (и копия) в prod по умолчанию только для чтения; флажок можно снять до сохранения.
            setProfile({ ...profile, environment, readOnly: environment === "prod" && isNew ? true : profile.readOnly });
          }}
        >
          {ENVIRONMENTS.map((env) => (
            <option key={env} value={env}>
              {env}
            </option>
          ))}
        </select>
```

4. Блок `<span>` с флажком «Только чтение» заменить на:

```tsx
        <span>
          <input
            id={`${uid}-ro`}
            type="checkbox"
            checked={profile.readOnly}
            onChange={(e) => setProfile({ ...profile, readOnly: e.target.checked })}
          />
        </span>
```

`src/styles.css`: удалить строку `.pro-lock { font-size: 11px; color: var(--muted); white-space: nowrap; }`.

- [ ] **Step 6: Словари и `same-as-en.json`**

Из корня репозитория:

```bash
node - <<'JS'
const fs = require("node:fs");
const remove = [
  "common.openLicense", "common.proHint", "errors.license", "connections.form.prodPro",
  "sidebar.license", "tabs.license", "license", "about.tier",
  "warnings.import.prodReadOnly_one", "warnings.import.prodReadOnly_few",
  "warnings.import.prodReadOnly_many", "warnings.import.prodReadOnly_other",
];
const texts = {
  en: {
    "about.summary": "Glafira for PostgreSQL is a desktop app for understanding what is going on with PostgreSQL and a Patroni cluster: which nodes are alive, who the leader is, how far replicas lag and how the databases are laid out.",
    "hints.connections.environment": "Marks the connection with a color. prod connections ask you to type the connection name before any change.",
  },
  ru: {
    "about.summary": "Glafira for PostgreSQL — настольное приложение, которое помогает разобраться, что происходит с PostgreSQL и кластером Patroni: какие узлы живы, кто лидер, насколько отстают реплики и как устроены базы.",
    "hints.connections.environment": "Помечает подключение цветом. Для prod-подключений любое изменение нужно подтвердить вводом имени подключения.",
  },
};
const at = (o, parts) => parts.reduce((x, p) => (x && typeof x === "object" ? x[p] : undefined), o);
for (const lang of ["en", "ru"]) {
  const path = `src/locales/${lang}.json`;
  const dict = JSON.parse(fs.readFileSync(path, "utf8"));
  for (const key of remove) {
    const parts = key.split(".");
    const parent = at(dict, parts.slice(0, -1));
    if (parent && typeof parent === "object") delete parent[parts[parts.length - 1]];
  }
  for (const [key, value] of Object.entries(texts[lang])) {
    const parts = key.split(".");
    const parent = at(dict, parts.slice(0, -1));
    if (!parent || !(parts[parts.length - 1] in parent)) throw new Error(`${lang}: нет ключа ${key}`);
    parent[parts[parts.length - 1]] = value;
  }
  fs.writeFileSync(path, JSON.stringify(dict, null, 2) + "\n");
}
console.log("словари обновлены");
JS
sed -i '' '/"connections.form.prodPro",/d' docs/i18n/same-as-en.json
grep -c "icen\|Pro\b" src/locales/en.json src/locales/ru.json   # ожидается 0 и 0
```

- [ ] **Step 7: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag
grep -rn "license\|License\|ProLock\|proHint\|PRO_REQUIRED\|hasLicenseKey\|\bPro\b" src --include=*.ts --include=*.tsx --include=*.css --include=*.json || echo "чисто"
pnpm test
npx tsc --noEmit
pnpm i18n:check
pnpm build
```

Ожидается: grep ничего не находит; vitest — все файлы зелёные (около 400 тестов; `LicensePanel.test.tsx` больше нет); tsc и i18n:check без ошибок; сборка проходит.

- [ ] **Step 8: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -q -F - <<'MSG'
refactor(ui): экран «Лицензия», замки Pro и статус лицензии удалены из фронтенда

prod доступен любому подключению, «Только чтение» снимается; ключи лицензии и Pro убраны из словарей en и ru.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 4: Лицензия в манифестах, `deny.toml`, `cargo deny` в CI, копирайт, `.gitattributes`

**Files:**
- Create: `deny.toml`, `.gitattributes`
- Modify: `src-tauri/Cargo.toml`, `crates/pg-advisor/Cargo.toml`, `package.json`, `src-tauri/tauri.conf.json`, `.gitignore`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: Rust без `license-format` (задача 1).
- Produces: `deny.toml` для задач 9 и 11 (`CONTRIBUTING.md`, `THIRD_PARTY.md`); `ci.yml` с шагом `cargo deny` и триггером по тегам (задача 10 добавит `release.yml` рядом).

- [ ] **Step 1: Поля лицензии в манифестах и копирайт**

`src-tauri/Cargo.toml` — после строки `edition = "2024"` вставить:

```toml
license = "Apache-2.0"
repository = "https://github.com/vvnovg/glafira-postgres"
```

`crates/pg-advisor/Cargo.toml` — после строки `description = …` вставить те же две строки (`publish = false` остаётся).

`package.json` — после `"version": "0.1.0",` вставить `"license": "Apache-2.0",`.

`src-tauri/tauri.conf.json` — строку `"copyright": "© 2026 Glafira for PostgreSQL",` заменить на `"copyright": "© 2026 Viacheslav Novgorodtsev",`.

- [ ] **Step 2: `deny.toml`**

```toml
# Лицензии зависимостей: `cargo deny --manifest-path <крейт>/Cargo.toml --config deny.toml check licenses`
# для src-tauri и crates/pg-advisor (так же в CI). Белый список — только разрешительные лицензии;
# MPL-2.0 — копилефт на уровне файла, совместима, пока файлы этих крейтов не изменяются.
# C-библиотеки, которые собирают openssl-src (OpenSSL 3, feature vendored у openssl/postgres-openssl)
# и aws-lc-sys (AWS-LC для reqwest/rustls), cargo deny не видит — они перечислены в THIRD_PARTY.md вручную.
[licenses]
confidence-threshold = 0.8
unused-allowed-license = "allow"
allow = [
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Zlib",
  "0BSD",
  "CC0-1.0",
  "Unlicense",
  "BSL-1.0",
  "MIT-0",
  "Unicode-DFS-2016",
  "Unicode-3.0",
  "MPL-2.0",
  "CDLA-Permissive-2.0",
]

[licenses.private]
ignore = true
```

- [ ] **Step 3: `.gitattributes` и `.gitignore`**

`.gitattributes`:

```
# Все текстовые файлы — с LF при любом checkout: скрипты .mjs и .sh ломаются от CRLF
# (vitest не разбирает hashbang с CRLF на Windows-раннере).
* text=auto eol=lf
# Файл реестра Windows читается regedit — оставляем CRLF.
*.reg text eol=crlf
```

В конец `.gitignore` добавить:

```
# Экспорты подключений (без паролей, но с адресами реальных кластеров)
*.glafira-postgres-connections.json
```

Затем перенормализовать индекс и убедиться, что ничего неожиданного не изменилось:

```bash
git add --renormalize .
git status --short
```

Ожидается: в списке только намеренно изменённые файлы (`.gitattributes`, `.gitignore`, манифесты, `deny.toml`, `ci.yml`); если `docs/admin/policy.reg` показан как изменённый — это нормализация CRLF, оставить.

- [ ] **Step 4: `.github/workflows/ci.yml`**

Заменить содержимое целиком:

```yaml
name: ci

on:
  push:
    branches: [main]
    tags: ["v*"]
  pull_request:
  workflow_dispatch:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read

jobs:
  linux:
    runs-on: ubuntu-22.04
    timeout-minutes: 60
    env:
      # Полная отладочная информация в тестовых бинарниках (каждый с Tauri) занимает много места
      # на диске раннера; номеров строк в backtrace достаточно.
      CARGO_PROFILE_DEV_DEBUG: line-tables-only
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
      - uses: pnpm/action-setup@b906affcce14559ad1aafd4ab0e942779e9f58b1 # v4
        with:
          version: 11
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
          cache: pnpm
      - uses: dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec # ветка stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2
        with:
          workspaces: |
            src-tauri -> target
            crates/pg-advisor -> target
      - name: Системные зависимости Tauri
        # cmake не нужен: aws-lc-sys собирается крейтом cc, а vendored OpenSSL — Perl и make из образа раннера.
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libsecret-1-dev
      - run: pnpm install --frozen-lockfile
      - run: pnpm test
      - run: pnpm test:scripts
      - run: pnpm i18n:check
      - run: pnpm build
      - name: pg-advisor
        run: |
          cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check
          cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings
          cargo test --manifest-path crates/pg-advisor/Cargo.toml
      - run: cargo fmt --manifest-path src-tauri/Cargo.toml --check
      - run: cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
      - name: Лицензии зависимостей (cargo deny, белый список — deny.toml)
        run: |
          cargo deny --version 2>/dev/null | grep -q ' 0\.20\.2$' || cargo install cargo-deny --locked --version 0.20.2 --force
          cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses
          cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
      - name: Стенд
        run: |
          docker compose -f dev/docker-compose.yml up -d --build
          scripts/wait-stand.sh
      - name: Тесты Rust (unit + интеграционные)
        run: cargo test --manifest-path src-tauri/Cargo.toml --lib --tests -- --test-threads=1
        env:
          GPG_IT_SINGLE: localhost:5460
          GPG_IT_PRIMARY: localhost:5451
          GPG_IT_REPLICA: localhost:5452
          GPG_IT_PATRONI: http://localhost:8011,http://localhost:8012,http://localhost:8013
          GPG_IT_DOCKER: "1"
      - name: Логи стенда при сбое
        if: failure()
        run: docker compose -f dev/docker-compose.yml logs --tail=200

  platforms:
    strategy:
      fail-fast: false
      matrix:
        os: [macos-14, windows-2022]
    runs-on: ${{ matrix.os }}
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
      - uses: pnpm/action-setup@b906affcce14559ad1aafd4ab0e942779e9f58b1 # v4
        with:
          version: 11
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
          cache: pnpm
      # Отдельных шагов установки cmake и NASM нет намеренно (aws-lc-sys 0.46, зависимость reqwest/rustls):
      # - cmake не нужен: для macOS и Windows x64 есть готовые привязки, и aws-lc-sys собирается крейтом cc;
      # - NASM на Windows x64 не нужен: rustls включает feature prebuilt-nasm, без nasm в PATH берутся
      #   готовые объектные файлы, а vendored OpenSSL (openssl-src) без nasm собирается с no-asm;
      # - vendored OpenSSL нужен Perl (Strawberry Perl есть в образе windows-2022, perl — в macOS) и nmake из Visual Studio.
      - uses: dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec # ветка stable
      - uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6 # v2.9.2
        with:
          workspaces: src-tauri -> target
      - run: pnpm install --frozen-lockfile
      - run: pnpm test
      - run: pnpm build
      - name: Unit-тесты Rust
        run: cargo test --manifest-path src-tauri/Cargo.toml --lib
      - run: cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

- [ ] **Step 5: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
cargo test --manifest-path src-tauri/Cargo.toml --lib product
grep -n "KAFKA_EXPLORER_TOKEN\|license-signing\|CARGO_NET_GIT_FETCH_WITH_CLI" .github/workflows/ci.yml || echo "ci.yml чист"
node -e 'JSON.parse(require("node:fs").readFileSync("package.json","utf8")); JSON.parse(require("node:fs").readFileSync("src-tauri/tauri.conf.json","utf8")); console.log("json ok")'
```

Ожидается: `licenses ok` дважды (у `glafira-pg` теперь есть поле `license`); тест `matches_tauri_conf` зелёный; grep ничего не находит; JSON валиден.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml crates/pg-advisor/Cargo.toml package.json src-tauri/tauri.conf.json deny.toml .gitattributes .gitignore .github/workflows/ci.yml
git add --renormalize .
git commit -q -F - <<'MSG'
build: Apache-2.0 в манифестах, deny.toml и cargo deny в CI, копирайт автора, .gitattributes с LF

CI без доступа к kafka-explorer и охраны ключей; триггер по тегам v*; экспорты подключений в .gitignore.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 5: Updater — Rust, конфигурация Tauri, тексты ошибок

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `src-tauri/src/lib.rs`, `src-tauri/src/commands/app.rs`, `src/locales/en.json`, `src/locales/ru.json`
- Test: тесты внутри `src-tauri/src/commands/app.rs`

**Interfaces:**
- Consumes: `PolicyStatus { updates_enabled, update_url }` (задача 2), `AppState.runtimes.shutdown()`.
- Produces: команды `check_update(app, state) -> AppResult<UpdateCheck>` и `install_update(app, state) -> AppResult<()>`; JSON `UpdateCheck`: `{ "status": "disabled", "reason": LocalizedText } | { "status": "upToDate" } | { "status": "available", "version", "notes": string|null, "date": string|null }`; ключи словаря `errors.updates.*`; `tauri.conf.json` с `plugins.updater`, `createUpdaterArtifacts`, `upgradeCode` (их читают `scripts/check-release.mjs` задачи 9 и тест ниже).

- [ ] **Step 1: Зависимость и конфигурация**

В `src-tauri/Cargo.toml` после строки `tauri-plugin-dialog = "2"` добавить:

```toml
tauri-plugin-updater = { version = "2.14", default-features = false, features = ["rustls-tls", "system-proxy"] }
```

`src-tauri/tauri.conf.json` заменить целиком:

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Glafira for PostgreSQL",
  "mainBinaryName": "glafira-pg",
  "version": "0.1.0",
  "identifier": "io.github.vvnovg.glafira.postgres",
  "build": {
    "beforeDevCommand": "pnpm dev",
    "devUrl": "http://localhost:1420",
    "beforeBuildCommand": "pnpm build",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      {
        "title": "Glafira for PostgreSQL",
        "width": 1400,
        "height": 900,
        "minWidth": 900,
        "minHeight": 600,
        "dragDropEnabled": false
      }
    ],
    "security": {
      "csp": "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src ipc: http://ipc.localhost"
    }
  },
  "bundle": {
    "active": true,
    "targets": ["app", "dmg", "msi", "deb", "rpm", "appimage"],
    "icon": ["icons/32x32.png", "icons/128x128.png", "icons/128x128@2x.png", "icons/icon.icns", "icons/icon.ico"],
    "createUpdaterArtifacts": true,
    "publisher": "Glafira for PostgreSQL",
    "category": "DeveloperTool",
    "shortDescription": "Glafira for PostgreSQL — diagnostics for PostgreSQL and Patroni clusters",
    "copyright": "© 2026 Viacheslav Novgorodtsev",
    "linux": { "deb": { "depends": ["libsecret-1-0"] }, "rpm": { "depends": ["libsecret"] } },
    "macOS": { "minimumSystemVersion": "11.0" },
    "windows": { "wix": { "upgradeCode": "a5dce14e-dd43-4ce6-9144-8437d8df8476", "language": ["en-US"] } }
  },
  "plugins": {
    "updater": {
      "pubkey": "REPLACE_WITH_UPDATER_PUBKEY",
      "endpoints": ["https://github.com/vvnovg/glafira-postgres-releases/releases/latest/download/latest.json"]
    }
  }
}
```

- [ ] **Step 2: `src-tauri/src/commands/app.rs` — команды updater с тестами**

Заменить содержимое файла целиком:

```rust
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
        crate::diagnostics::write_archive(
            std::path::Path::new(&path),
            &info,
            &st.connections.list(),
            &st.policy.status,
            &crate::logging::log_files(&log_dir),
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
        date: Option<String>,
    },
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
    // На Windows `download_and_install` запускает установщик и вызывает `process::exit(0)`,
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
                    let pause = backoff(attempt);
                    tauri::async_runtime::spawn_blocking(move || std::thread::sleep(pause))
                        .await
                        .ok();
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
            date: None,
        })
        .unwrap();
        assert_eq!(v["status"], "available");
        assert_eq!(v["version"], "0.1.0");
        assert!(v["notes"].is_null());
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
```

- [ ] **Step 3: `src-tauri/src/lib.rs` — плагин, команды, лог политики**

1. После `.plugin(tauri_plugin_dialog::init())` добавить строку `.plugin(tauri_plugin_updater::Builder::new().build())`.
2. Строку `tracing::info!(managed = policy.status.managed, "policy");` заменить на:

```rust
            tracing::info!(
                managed = policy.status.managed,
                updates = policy.status.updates_enabled,
                "policy"
            );
```

3. В `generate_handler!` после `commands::app::export_diagnostics,` добавить:

```rust
            commands::app::check_update,
            commands::app::install_update,
```

- [ ] **Step 4: Тексты ошибок updater в словарях**

Из корня репозитория (вставка `errors.updates` сразу после `errors.policy` в обоих словарях):

```bash
node - <<'JS'
const fs = require("node:fs");
const updates = {
  en: {
    managed: "Updates are managed by your organization",
    notConfigured: "Updates are not configured in this build",
    badPolicyUrl: "Invalid update address in the policy: {{url}}",
    unavailable: "Updates are unavailable",
    noRelease: "The update server has no published versions (or it is temporarily unavailable)",
    checkFailed: "Failed to check for updates",
    downloadInterrupted: "The update download was interrupted — check your network and try again",
    badSignature: "The update signature failed verification — the update was not installed",
    installFailed: "Failed to install the update",
    none: "No updates available",
  },
  ru: {
    managed: "Обновления управляются вашей организацией",
    notConfigured: "Обновления не настроены в этой сборке",
    badPolicyUrl: "Неверный адрес обновлений в политике: {{url}}",
    unavailable: "Обновления недоступны",
    noRelease: "На сервере обновлений нет опубликованных версий (или он временно недоступен)",
    checkFailed: "Не удалось проверить обновления",
    downloadInterrupted: "Загрузка обновления прервалась — проверьте сеть и повторите",
    badSignature: "Подпись обновления не прошла проверку — обновление не установлено",
    installFailed: "Не удалось установить обновление",
    none: "Обновлений нет",
  },
};
/** Новый объект с `key` сразу после `after` (порядок ключей в JSON сохраняется). */
const insertAfter = (obj, after, key, value) =>
  Object.fromEntries(Object.entries(obj).flatMap(([k, v]) => (k === after ? [[k, v], [key, value]] : [[k, v]])));
for (const lang of ["en", "ru"]) {
  const path = `src/locales/${lang}.json`;
  const dict = JSON.parse(fs.readFileSync(path, "utf8"));
  if (!dict.errors.policy) throw new Error(`${lang}: нет errors.policy`);
  dict.errors = insertAfter(dict.errors, "policy", "updates", updates[lang]);
  fs.writeFileSync(path, JSON.stringify(dict, null, 2) + "\n");
}
console.log("errors.updates добавлены");
JS
```

- [ ] **Step 5: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib commands::app
cargo test --manifest-path src-tauri/Cargo.toml --lib product
pnpm i18n:check
```

Ожидается: первый `cargo` скачивает `tauri-plugin-updater` (нужна сеть), `Cargo.lock` меняется; clippy чист; 7 тестов `commands::app` и тест `product::tests::matches_tauri_conf` зелёные; `pnpm i18n:check` находит ключи `errors.updates.*` из `ltext!`/`app_err!` в словарях.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json src-tauri/src/lib.rs src-tauri/src/commands/app.rs src/locales/en.json src/locales/ru.json
git commit -q -F - <<'MSG'
feat(updater): команды check_update и install_update, плагин updater, артефакты updater в сборке

Проверка подчиняется политике (updatesEnabled, updateUrl); сборка с заглушкой ключа честно сообщает
«не настроены». upgradeCode WiX зафиксирован до первого релиза.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 6: Updater — фронтенд: уведомление и раздел «Обновления»

**Files:**
- Create: `src/components/about/UpdateNotice.tsx`, `src/components/about/UpdateNotice.test.tsx`
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/components/Layout.tsx`, `src/components/about/AboutPanel.tsx`, `src/locales/en.json`, `src/locales/ru.json`
- Test: `src/api/client.test.ts`, `src/components/about/AboutPanel.test.tsx`, `src/components/about/UpdateNotice.test.tsx`

**Interfaces:**
- Consumes: команды `check_update`/`install_update` и форму JSON `UpdateCheck` (задача 5).
- Produces: `api.checkUpdate(): Promise<UpdateCheck>`, `api.installUpdate(): Promise<void>`; компонент `UpdateNotice`; раздел `UpdatesSection` внутри `AboutPanel`; ключи `about.updates.*`, `about.notice.*`.

- [ ] **Step 1: Тесты — ожидания**

Создать `src/components/about/UpdateNotice.test.tsx`:

```tsx
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { checkUpdate: vi.fn() } };
});

import { api } from "../../api/client";
import { UpdateNotice } from "./UpdateNotice";

describe("UpdateNotice", () => {
  beforeEach(() => {
    vi.mocked(api.checkUpdate).mockReset();
    useWorkspace.setState({ tabs: [], activeId: null });
  });

  it("показывает доступную версию и открывает «О программе»", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValue({ status: "available", version: "0.2.0", notes: null, date: null });
    const user = userEvent.setup();
    renderWithClient(<UpdateNotice />);
    expect(await screen.findByText("Доступна версия 0.2.0.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Подробнее" }));
    expect(useWorkspace.getState().tabs.some((t) => t.kind === "about")).toBe(true);
    await user.click(screen.getByRole("button", { name: "Скрыть уведомление" }));
    expect(screen.queryByText("Доступна версия 0.2.0.")).not.toBeInTheDocument();
  });

  it("«обновления выключены» и ошибка проверки ничего не показывают", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValue({
      status: "disabled",
      reason: { key: "errors.updates.notConfigured", params: {}, text: "Updates are not configured in this build" },
    });
    const first = renderWithClient(<UpdateNotice />);
    await vi.waitFor(() => expect(api.checkUpdate).toHaveBeenCalled());
    expect(first.container).toBeEmptyDOMElement();
    first.unmount();

    vi.mocked(api.checkUpdate).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Не удалось проверить обновления", retryable: true });
    const second = renderWithClient(<UpdateNotice />);
    await vi.waitFor(() => expect(api.checkUpdate).toHaveBeenCalledTimes(2));
    expect(second.container).toBeEmptyDOMElement();
  });
});
```

В `src/components/about/AboutPanel.test.tsx`: в мок `api` добавить две строки после `exportDiagnostics`:

```tsx
      checkUpdate: vi.fn().mockResolvedValue({ status: "upToDate" }),
      installUpdate: vi.fn().mockResolvedValue(undefined),
```

и добавить тесты перед тестом `ошибки политики видны в «О программе»`:

```tsx
  it("обновление доступно: описание и установка", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: "Исправления", date: null });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Доступна версия 0.2.0")).toBeInTheDocument();
    expect(screen.getByText("Исправления")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Установить и перезапустить" }));
    expect(api.installUpdate).toHaveBeenCalledTimes(1);
  });

  it("политика выключила обновления", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({
      status: "disabled",
      reason: { key: "errors.updates.managed", params: {}, text: "Updates are managed by your organization" },
    });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Обновления управляются вашей организацией")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Установить и перезапустить" })).not.toBeInTheDocument();
  });

  it("последняя версия; ошибка сети — баннер с повтором", async () => {
    vi.mocked(api.checkUpdate)
      .mockRejectedValueOnce({ code: "CONNECTION_FAILED", message: "Не удалось проверить обновления", retryable: true })
      .mockResolvedValueOnce({ status: "upToDate" });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Не удалось проверить обновления")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("Установлена последняя версия")).toBeInTheDocument();
  });

  it("ошибка установки — баннер", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: null, date: null });
    vi.mocked(api.installUpdate).mockRejectedValueOnce({ code: "INTERNAL", message: "Не удалось установить обновление", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    await user.click(await screen.findByRole("button", { name: "Установить и перезапустить" }));
    expect(await screen.findByText("Не удалось установить обновление")).toBeInTheDocument();
  });
```

В `src/api/client.test.ts` добавить тест:

```ts
  it("команды updater вызываются по имени", async () => {
    invoke.mockResolvedValue({ status: "upToDate" });
    await api.checkUpdate();
    expect(invoke).toHaveBeenLastCalledWith("check_update", undefined);
    await api.installUpdate();
    expect(invoke).toHaveBeenLastCalledWith("install_update", undefined);
  });
```

- [ ] **Step 2: Запустить тесты — должны упасть**

```bash
pnpm vitest run src/components/about src/api 2>&1 | tail -12
```

Ожидается: `UpdateNotice.test.tsx` не находит модуль; `AboutPanel` не находит кнопку «Проверить обновления»; `client.test.ts` — `api.checkUpdate is not a function`.

- [ ] **Step 3: Типы и клиент**

`src/api/types.ts` — после `export interface PolicyStatus { … }` добавить:

```ts
/** Результат `check_update`. */
export type UpdateCheck =
  | { status: "disabled"; reason: LocalizedText }
  | { status: "upToDate" }
  | { status: "available"; version: string; notes: string | null; date: string | null };
```

`src/api/client.ts` — добавить `UpdateCheck,` в импорт типов (по алфавиту, после `TreeItemRef,`) и после строки `exportDiagnostics: …` вставить:

```ts
  checkUpdate: () => call<UpdateCheck>("check_update"),
  installUpdate: () => call<void>("install_update"),
```

- [ ] **Step 4: `UpdateNotice.tsx` и `Layout.tsx`**

Создать `src/components/about/UpdateNotice.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import { useWorkspace } from "../../state/workspace";

/** Проверка обновлений один раз при запуске. Ошибки и «выключено» молча игнорируются — подробности в «О программе». */
export function UpdateNotice() {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["update-check"], queryFn: api.checkUpdate, staleTime: Infinity, retry: false });
  const open = useWorkspace((s) => s.open);
  const [hidden, setHidden] = useState(false);
  if (hidden || q.data?.status !== "available") return null;
  return (
    <div className="notice" role="status">
      <span>{t("about.notice.available", { version: q.data.version })}</span>
      <button type="button" className="link" onClick={() => open({ kind: "about" })}>
        {t("about.notice.more")}
      </button>
      <button type="button" className="icon" aria-label={t("about.notice.hideAria")} onClick={() => setHidden(true)}>
        ×
      </button>
    </div>
  );
}
```

`src/components/Layout.tsx`: добавить импорт `import { UpdateNotice } from "./about/UpdateNotice";` (после импорта `EnvBadge`) и вставить `<UpdateNotice />` сразу после строки `<div className="env-stripe" … />` (там, где раньше стоял `LicenseNotice`).

- [ ] **Step 5: `AboutPanel.tsx` с разделом «Обновления»**

Заменить содержимое файла целиком:

```tsx
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../../api/client";
import { tr } from "../../i18n/text";
import { useDiagnosticsRequest } from "../../state/diagnosticsRequest";
import { ErrorBanner } from "../common/ErrorBanner";

export function AboutPanel() {
  const { t } = useTranslation();
  const info = useQuery({ queryKey: ["app-info"], queryFn: api.getAppInfo });
  const policy = useQuery({ queryKey: ["policy"], queryFn: api.getPolicyStatus });
  const [saved, setSaved] = useState<string | null>(null);
  const diag = useMutation({
    mutationFn: async () => {
      setSaved(null);
      const date = new Date().toISOString().slice(0, 10);
      const path = await save({ defaultPath: `diagnostics-${date}.zip`, filters: [{ name: "ZIP", extensions: ["zip"] }] });
      if (!path) return null;
      await api.exportDiagnostics(path);
      return path;
    },
    onSuccess: (path) => {
      if (path) setSaved(path);
    },
  });
  // «Собрать диагностику…» из меню Help: тот же сбор, что и по кнопке; запрос забирается один раз.
  // Пока сбор идёт, запрос снимается без второго сбора и второго диалога сохранения.
  const diagRequested = useDiagnosticsRequest((s) => s.pending);
  const { mutate: collect, isPending: collecting } = diag;
  useEffect(() => {
    if (diagRequested && useDiagnosticsRequest.getState().consume() && !collecting) collect();
  }, [diagRequested, collect, collecting]);

  if (info.error) return <ErrorBanner error={info.error} onRetry={() => info.refetch()} />;
  const i = info.data;
  if (!i) return <p className="empty">{t("common.loading")}</p>;
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{i.name}</h2>
      </div>
      <p className="about-summary">{t("about.summary")}</p>
      <dl className="stats">
        <div><dt>{t("about.version")}</dt><dd>{i.version}</dd></div>
        <div><dt>{t("about.build")}</dt><dd>{i.commit}</dd></div>
        <div><dt>{t("about.system")}</dt><dd>{i.os} / {i.arch}</dd></div>
        <div><dt>{t("about.policy")}</dt><dd>{policy.data ? (policy.data.managed ? t("about.policySet") : t("about.policyNone")) : "…"}</dd></div>
      </dl>
      {policy.data?.errors.map((e, i) => (
        <p key={i} className="warn">
          {t("about.policyError", { error: tr(e) })}
        </p>
      ))}
      <p className="muted">{t("about.dataDir")} <code>{i.dataDir}</code></p>
      <p className="muted">{t("about.logDir")} <code>{i.logDir}</code></p>
      <UpdatesSection />
      <section className="panel">
        <h3>{t("about.diagnostics.title")}</h3>
        <p className="muted">{t("about.diagnostics.hint")}</p>
        <div className="toolbar">
          <button type="button" disabled={diag.isPending} onClick={() => diag.mutate()}>
            {t("about.diagnostics.collect")}
          </button>
        </div>
        {saved && <p role="status">{t("about.diagnostics.saved", { path: saved })}</p>}
        {diag.error && <ErrorBanner error={diag.error} />}
      </section>
    </div>
  );
}

/** Ручная проверка и установка обновлений; подтверждения нет — длительных операций у приложения нет. */
function UpdatesSection() {
  const { t } = useTranslation();
  const check = useMutation({ mutationFn: api.checkUpdate });
  const install = useMutation({ mutationFn: api.installUpdate });
  const r = check.data;
  return (
    <section className="panel">
      <h3>{t("about.updates.title")}</h3>
      <div className="toolbar">
        <button
          type="button"
          disabled={check.isPending || install.isPending}
          onClick={() => {
            install.reset();
            check.mutate();
          }}
        >
          {t("about.updates.check")}
        </button>
      </div>
      {check.error && <ErrorBanner error={check.error} onRetry={() => check.mutate()} />}
      {r?.status === "disabled" && <p className="muted">{tr(r.reason)}</p>}
      {r?.status === "upToDate" && <p role="status">{t("about.updates.upToDate")}</p>}
      {r?.status === "available" && (
        <>
          <p role="status">{t("about.updates.available", { version: r.version })}</p>
          {r.notes && <pre>{r.notes}</pre>}
          <div className="toolbar">
            <button type="button" disabled={install.isPending} onClick={() => install.mutate()}>
              {t("about.updates.install")}
            </button>
          </div>
          {install.isPending && <p className="muted">{t("about.updates.installing")}</p>}
          {install.error && <ErrorBanner error={install.error} />}
        </>
      )}
    </section>
  );
}
```

- [ ] **Step 6: Словари — `about.updates` и `about.notice`**

```bash
node - <<'JS'
const fs = require("node:fs");
const add = {
  en: {
    updates: {
      title: "Updates",
      check: "Check for updates",
      upToDate: "You have the latest version",
      available: "Version {{version}} is available",
      install: "Install and restart",
      installing: "Downloading and installing the update…",
    },
    notice: { available: "Version {{version}} is available.", more: "Details", hideAria: "Dismiss notification" },
  },
  ru: {
    updates: {
      title: "Обновления",
      check: "Проверить обновления",
      upToDate: "Установлена последняя версия",
      available: "Доступна версия {{version}}",
      install: "Установить и перезапустить",
      installing: "Загрузка и установка обновления…",
    },
    notice: { available: "Доступна версия {{version}}.", more: "Подробнее", hideAria: "Скрыть уведомление" },
  },
};
for (const lang of ["en", "ru"]) {
  const path = `src/locales/${lang}.json`;
  const dict = JSON.parse(fs.readFileSync(path, "utf8"));
  if (!dict.about || !dict.about.diagnostics) throw new Error(`${lang}: нет about.diagnostics`);
  dict.about.updates = add[lang].updates;
  dict.about.notice = add[lang].notice;
  fs.writeFileSync(path, JSON.stringify(dict, null, 2) + "\n");
}
console.log("about.updates и about.notice добавлены");
JS
```

- [ ] **Step 7: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag
pnpm test
npx tsc --noEmit
pnpm i18n:check
```

Ожидается: все тесты зелёные, включая 4 новых в `AboutPanel.test.tsx`, 2 в `UpdateNotice.test.tsx`, 1 в `client.test.ts`.

- [ ] **Step 8: Commit**

```bash
git add src/api src/components/Layout.tsx src/components/about src/locales
git commit -q -F - <<'MSG'
feat(ui): раздел «Обновления» в «О программе» и уведомление о новой версии над вкладками

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 7: «Поддержать проект»: низ панели, «О программе», меню Help; строка «Лицензия»

**Files:**
- Create: `src/lib/links.ts`
- Modify: `src/components/Sidebar.tsx`, `src/components/about/AboutPanel.tsx`, `src/lib/useAppMenu.ts`, `src/locales/en.json`, `src/locales/ru.json`
- Test: `src/components/Sidebar.test.tsx`, `src/components/about/AboutPanel.test.tsx`, `src/lib/useAppMenu.test.ts`

**Interfaces:**
- Consumes: `AboutPanel` с `UpdatesSection` (задача 6).
- Produces: `SUPPORT_URL`, `LICENSE_URL`, `openSupport()`, `openLicense()` в `src/lib/links.ts`; ключи `support.title`, `support.hint`, `about.license` (их используют глава руководства задачи 8 и документация).

- [ ] **Step 1: Тесты — ожидания**

`src/components/Sidebar.test.tsx` заменить целиком:

```tsx
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { newProfile } from "./connections/profile";
import { renderWithClient } from "../test/render";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return {
    ...orig,
    api: {
      getTopology: vi.fn(() => new Promise(() => {})),
      listFolders: vi.fn(() => Promise.resolve([])),
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing" }])),
    },
  };
});

import { openUrl } from "@tauri-apps/plugin-opener";
import { SUPPORT_URL } from "../lib/links";
import { useWorkspace } from "../state/workspace";
import { Sidebar } from "./Sidebar";

beforeEach(() => {
  localStorage.clear();
  useWorkspace.setState({ tabs: [], activeId: null });
  vi.mocked(openUrl).mockClear();
});

describe("Sidebar", () => {
  it("показывает дерево подключений и ссылки внизу в заданном порядке", async () => {
    const user = userEvent.setup();
    const { container } = renderWithClient(<Sidebar />);
    expect(await screen.findByRole("button", { name: "billing" })).toBeInTheDocument();
    expect(screen.getByText("Подключения")).toBeInTheDocument();
    const footer = [...container.querySelectorAll<HTMLButtonElement>(".sidebar-footer button")].map((b) => b.textContent);
    expect(footer).toEqual(["Настройки", "Поддержать проект", "Справка", "О программе"]);
    expect(screen.queryByRole("button", { name: /^Лицензия/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Настройки" }));
    expect(useWorkspace.getState().activeId).toBe("settings");
  });

  it("«Поддержать проект» открывает страницу поддержки в браузере, вкладку не открывает", async () => {
    const user = userEvent.setup();
    renderWithClient(<Sidebar />);
    await user.click(await screen.findByRole("button", { name: "Поддержать проект" }));
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(useWorkspace.getState().tabs).toEqual([]);
  });
});
```

`src/components/about/AboutPanel.test.tsx`: после строки `vi.mock("@tauri-apps/plugin-dialog", …)` добавить `vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));`; к импортам после `import { save } from "@tauri-apps/plugin-dialog";` добавить

```tsx
import { openUrl } from "@tauri-apps/plugin-opener";
import { LICENSE_URL, SUPPORT_URL } from "../../lib/links";
```

и добавить тест после первого:

```tsx
  it("строка лицензии и кнопка поддержки открывают страницы в браузере", async () => {
    vi.mocked(openUrl).mockClear();
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText("Лицензия")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Apache 2.0" }));
    expect(openUrl).toHaveBeenCalledWith(LICENSE_URL);
    await user.click(screen.getByRole("button", { name: "Поддержать проект" }));
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(screen.getByText(/пожертвования помогают/)).toBeInTheDocument();
    expect(screen.queryByText("Тариф")).not.toBeInTheDocument();
  });
```

`src/lib/useAppMenu.test.ts`: после блока `vi.mock("@tauri-apps/api/menu", …)` добавить `vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));`; импорты заменить на

```ts
import { openUrl } from "@tauri-apps/plugin-opener";
import { i18n } from "../i18n";
import { useDiagnosticsRequest } from "../state/diagnosticsRequest";
import { useWorkspace } from "../state/workspace";
import { SUPPORT_URL } from "./links";
import { useAppMenu } from "./useAppMenu";
```

в `beforeEach` добавить `vi.mocked(openUrl).mockClear();`; ожидания подписей заменить: ru — `["Руководство Glafira", "—", "Поддержать проект", "О программе", "Собрать диагностику…"]`, en — `["Glafira Guide", "—", "Support the project", "About Glafira", "Collect Diagnostics…"]`; в тесте `пункты открывают справку, «О программе» и сбор диагностики` после `click("about"); …` добавить

```ts
    click("support");
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(useWorkspace.getState().activeId).toBe("about");
```

- [ ] **Step 2: Запустить тесты — должны упасть**

```bash
pnpm vitest run src/components/Sidebar.test.tsx src/components/about/AboutPanel.test.tsx src/lib/useAppMenu.test.ts 2>&1 | tail -12
```

Ожидается: не найден модуль `../lib/links`, нет кнопки «Поддержать проект», подписи меню без «Поддержать проект».

- [ ] **Step 3: `src/lib/links.ts`**

```ts
import { openUrl } from "@tauri-apps/plugin-opener";

/** Страница поддержки проекта — один адрес на все входы: низ левой панели, «О программе», меню Help. */
export const SUPPORT_URL = "https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md";
/** Текст лицензии Apache 2.0 в репозитории исходного кода. */
export const LICENSE_URL = "https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE";

/** Открывает страницу поддержки в системном браузере. */
export function openSupport(): void {
  void openUrl(SUPPORT_URL);
}

/** Открывает текст лицензии в системном браузере. */
export function openLicense(): void {
  void openUrl(LICENSE_URL);
}
```

- [ ] **Step 4: `Sidebar.tsx`, `AboutPanel.tsx`, `useAppMenu.ts`**

`src/components/Sidebar.tsx` целиком:

```tsx
import { useTranslation } from "react-i18next";
import { openSupport } from "../lib/links";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);

  return (
    <aside className="sidebar">
      <ConnectionTree />
      <div className="sidebar-footer">
        <button type="button" className="link" onClick={() => open({ kind: "settings" })}>
          {t("sidebar.settings")}
        </button>
        <button type="button" className="link" onClick={openSupport}>
          {t("support.title")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "help" })}>
          {t("sidebar.help")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "about" })}>
          {t("sidebar.about")}
        </button>
      </div>
    </aside>
  );
}
```

`src/components/about/AboutPanel.tsx`: добавить импорт `import { openLicense, openSupport } from "../../lib/links";` (после импорта `tr`); в `<dl className="stats">` после строки «Система» вставить

```tsx
        <div>
          <dt>{t("about.license")}</dt>
          <dd>
            <button type="button" className="link" onClick={openLicense}>
              Apache 2.0
            </button>
          </dd>
        </div>
```

(строка «Политика» остаётся после неё); после блока `{policy.data?.errors.map(…)}` и перед `<p className="muted">{t("about.dataDir")}…` вставить

```tsx
      <p>
        <button type="button" className="link" onClick={openSupport}>
          {t("support.title")}
        </button>{" "}
        <span className="muted">{t("support.hint")}</span>
      </p>
```

`src/lib/useAppMenu.ts`: добавить импорт `import { openSupport } from "./links";` (после `import { i18n } from "../i18n";`); массив `items` заменить на

```ts
  const items = [
    // ⌘? — на раскладке US это ⇧⌘/; символ «?» как клавишу Tauri не принимает.
    await MenuItem.new({ id: id("guide"), text: t("menu.guide"), accelerator: "CmdOrCtrl+Shift+/", action: () => open({ kind: "help" }) }),
    await PredefinedMenuItem.new({ item: "Separator" }),
    await MenuItem.new({ id: id("support"), text: t("support.title"), action: openSupport }),
    await MenuItem.new({ id: id("about"), text: t("menu.about"), action: () => open({ kind: "about" }) }),
    await MenuItem.new({
      id: id("diagnostics"),
      text: t("menu.diagnostics"),
      action: () => {
        useDiagnosticsRequest.getState().request();
        open({ kind: "about" });
      },
    }),
  ];
```

- [ ] **Step 5: Словари — `support.*` и `about.license`**

```bash
node - <<'JS'
const fs = require("node:fs");
const add = {
  en: { support: { title: "Support the project", hint: "Glafira is free and open source; donations keep it going." }, license: "License" },
  ru: { support: { title: "Поддержать проект", hint: "Glafira бесплатна, её исходный код открыт; пожертвования помогают её развивать." }, license: "Лицензия" },
};
const insertAfter = (obj, after, key, value) =>
  Object.fromEntries(Object.entries(obj).flatMap(([k, v]) => (k === after ? [[k, v], [key, value]] : [[k, v]])));
for (const lang of ["en", "ru"]) {
  const path = `src/locales/${lang}.json`;
  const dict = JSON.parse(fs.readFileSync(path, "utf8"));
  if (!dict.menu || !dict.about.system) throw new Error(`${lang}: нет menu или about.system`);
  const withSupport = insertAfter(dict, "menu", "support", add[lang].support);
  withSupport.about = insertAfter(withSupport.about, "system", "license", add[lang].license);
  fs.writeFileSync(path, JSON.stringify(withSupport, null, 2) + "\n");
}
console.log("support и about.license добавлены");
JS
```

- [ ] **Step 6: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag
pnpm test
npx tsc --noEmit
pnpm i18n:check
pnpm build
```

Ожидается: всё зелёное; `grep -rn "SUPPORT_URL\|LICENSE_URL" src --include=*.ts --include=*.tsx | grep -v "lib/links.ts\|test"` показывает только импорты из `links`.

- [ ] **Step 7: Commit**

```bash
git add src/lib/links.ts src/components/Sidebar.tsx src/components/Sidebar.test.tsx src/components/about src/lib/useAppMenu.ts src/lib/useAppMenu.test.ts src/locales
git commit -q -F - <<'MSG'
feat(ui): «Поддержать проект» внизу панели, в «О программе» и в меню Help; строка «Лицензия: Apache 2.0»

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 8: Руководство: глава «Обновления и диагностика» (en, ru), F1 на «О программе», документация i18n

**Files:**
- Create: `docs/guide/en/updates.md`, `docs/guide/ru/updates.md`
- Modify: `src/guide/chapters.ts`, `src/components/help/helpChapterFor.ts`, `scripts/i18n-check.mjs`, `docs/i18n/README.md`, `docs/i18n/glossary.md`
- Test: `src/components/help/helpChapterFor.test.ts`, `scripts/i18n-check.test.mjs`

**Interfaces:**
- Consumes: ключи словаря `about.updates.*`, `about.notice.more`, `errors.updates.*`, `support.title`, `about.license` (задачи 5–7) — метки `[[ui:…]]` в главе.
- Produces: `GUIDE_CHAPTERS = ["getting-started", "updates"]` в `src/guide/chapters.ts` и `scripts/i18n-check.mjs` (их читает `scripts/guide-publish.mjs` задачи 9); `helpChapterFor({ kind: "about" }) === "updates"`.

- [ ] **Step 1: Тесты — ожидания**

`src/components/help/helpChapterFor.test.ts` заменить целиком:

```ts
import { describe, expect, it } from "vitest";
import { helpChapterFor } from "./helpChapterFor";

describe("helpChapterFor", () => {
  it("для null, настроек и подключений — «Начало работы»", () => {
    expect(helpChapterFor(null)).toBe("getting-started");
    expect(helpChapterFor({ kind: "settings" })).toBe("getting-started");
    expect(helpChapterFor({ kind: "connection-form", connectionId: null })).toBe("getting-started");
  });

  it("для «О программе» — «Обновления и диагностика»", () => {
    expect(helpChapterFor({ kind: "about" })).toBe("updates");
  });
});
```

В `scripts/i18n-check.test.mjs` строку `assert.deepEqual(GUIDE_CHAPTERS, ["getting-started"]);` заменить на `assert.deepEqual(GUIDE_CHAPTERS, ["getting-started", "updates"]);`.

- [ ] **Step 2: Запустить тесты — должны упасть**

```bash
pnpm vitest run src/components/help/helpChapterFor.test.ts 2>&1 | tail -6
node --test scripts/i18n-check.test.mjs 2>&1 | tail -6
```

- [ ] **Step 3: Список глав и выбор главы по F1**

`src/guide/chapters.ts`: строку `export const GUIDE_CHAPTERS = ["getting-started"] as const;` заменить на `export const GUIDE_CHAPTERS = ["getting-started", "updates"] as const;`.

`scripts/i18n-check.mjs`: строку `export const GUIDE_CHAPTERS = ["getting-started"];` заменить на `export const GUIDE_CHAPTERS = ["getting-started", "updates"];`.

`src/components/help/helpChapterFor.ts` целиком:

```ts
import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/** Глава руководства для экрана (F1): «О программе» → «Обновления и диагностика», остальное — «Начало работы». */
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  return tab?.kind === "about" ? "updates" : "getting-started";
}
```

- [ ] **Step 4: Глава на английском — `docs/guide/en/updates.md`**

```markdown
# Updates and diagnostics

This chapter covers how to update the app, the [[ui:sidebar.about]] screen, the diagnostics archive and how to support the project.

## Updates {#updates}

When it starts, the app checks once whether a new version is available. If there is one, a notice above the tabs gives its version. [[ui:about.notice.more]] opens the [[ui:sidebar.about]] screen. If the check fails, for example without internet access, no message is shown.

To check manually:

1. Open [[ui:sidebar.about]] at the bottom of the left panel.
2. In [[ui:about.updates.title]], click [[ui:about.updates.check]].
3. If a new version is available, the screen shows its number and release notes. Click [[ui:about.updates.install]].

The app downloads the update, checks its signature, installs it and restarts. Open connections are closed before the restart; connection profiles, settings and other data are kept. A download interrupted by a network problem is retried automatically, up to three attempts in total. If all three attempts fail, the app reports [[ui:errors.updates.downloadInterrupted]]. If the signature check fails, the app reports [[ui:errors.updates.badSignature]].

Installing may ask for an administrator password, because the app is installed for all users of the computer: by the `.pkg` installer on macOS, the `.msi` installer on Windows, or a `.deb` or `.rpm` package on Linux. Users without administrator rights cannot update such an installation.

Updates are downloaded from the releases page on GitHub. If the check fails with [[ui:errors.updates.checkFailed]], check your network or proxy. [[ui:errors.updates.notConfigured]] means that this build was made without the updater key, for example a development or test build. In that case, download new versions from the releases page: https://github.com/vvnovg/glafira-postgres-releases/releases

An organization can turn off update checks through a policy, or point them to an internal mirror. When checks are turned off, the [[ui:about.updates.title]] section says [[ui:errors.updates.managed]] after you click [[ui:about.updates.check]]. Like [[ui:errors.updates.notConfigured]], this message appears only after the click; the check at startup shows nothing in these cases.

## About {#about}

Open [[ui:sidebar.about]] at the bottom of the left panel. On macOS, it is also in the Help menu as [[ui:menu.about]].

The screen shows a short description of the app and:

- [[ui:about.version]] — the version number, and [[ui:about.build]] — the identifier of the source code it was built from. Include both in a support request.
- [[ui:about.system]] — the operating system and processor architecture.
- [[ui:about.license]] — Apache 2.0; the link opens the license text in the browser.
- [[ui:about.policy]] — [[ui:about.policySet]] or [[ui:about.policyNone]]. Errors in the policy are listed below.
- [[ui:about.dataDir]] — the folder with connection profiles and settings.
- [[ui:about.logDir]] — the folder with log files.

The [[ui:about.updates.title]] and [[ui:about.diagnostics.title]] sections are also on this screen.

## Diagnostics archive {#diagnostics}

The diagnostics archive collects what support needs to understand a problem. To create it:

1. Open [[ui:sidebar.about]].
2. In [[ui:about.diagnostics.title]], click [[ui:about.diagnostics.collect]]. On macOS, you can also choose [[ui:menu.diagnostics]] in the Help menu.
3. Choose where to save the file. The suggested name is `diagnostics-<date>.zip`.

When the archive is saved, the screen shows its path. The ZIP file contains:

- `info.json` — the app version and build, the operating system, the data and log folder paths and the policy status;
- `connections.json` — connection profiles: names, environments, connection kinds, node and Patroni REST API addresses, user names, databases, certificate file paths and member address overrides;
- `logs/` — the app log files.

The archive contains no passwords: they are kept in the system secret store, and passwords inside Patroni API addresses are removed. Certificate files are not included either. Host names, user names and file paths are stored as plain text, and the logs contain node addresses from PostgreSQL and Patroni error messages. Look through the archive before you send it outside your company.

The app sends nothing by itself. Attach the archive to your support request yourself.

## Support the project {#support}

Glafira is free and open source under the Apache License 2.0. Donations keep it going. [[ui:support.title]] at the bottom of the left panel, on the [[ui:sidebar.about]] screen and, on macOS, in the Help menu opens the support page in the browser: https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md
```

- [ ] **Step 5: Глава на русском — `docs/guide/ru/updates.md`**

```markdown
# Обновления и диагностика

В этой главе — как обновить приложение, экран [[ui:sidebar.about]], архив диагностики и как поддержать проект.

## Обновления {#updates}

При запуске приложение один раз проверяет, есть ли новая версия. Если есть, над вкладками появляется уведомление с номером версии. Ссылка [[ui:about.notice.more]] открывает экран [[ui:sidebar.about]]. Если проверка не удалась, например без доступа в интернет, ничего не показывается.

Чтобы проверить вручную:

1. Откройте [[ui:sidebar.about]] внизу левой панели.
2. В разделе [[ui:about.updates.title]] нажмите [[ui:about.updates.check]].
3. Если доступна новая версия, на экране видны её номер и описание изменений. Нажмите [[ui:about.updates.install]].

Приложение скачивает обновление, проверяет его подпись, устанавливает и перезапускается. Открытые подключения закрываются перед перезапуском; профили подключений, настройки и другие данные сохраняются. Если загрузку прервал сбой сети, она повторяется автоматически — всего до трёх попыток. Если все три попытки не удались, приложение сообщает: «[[ui:errors.updates.downloadInterrupted]]». Если подпись не прошла проверку, приложение сообщает: «[[ui:errors.updates.badSignature]]».

Установка может запросить пароль администратора, потому что приложение установлено для всех пользователей компьютера: установщиком `.pkg` на macOS, установщиком `.msi` на Windows или пакетом `.deb` или `.rpm` на Linux. Пользователи без прав администратора такую установку обновить не могут.

Обновления загружаются со страницы релизов на GitHub. Если проверка завершилась ошибкой «[[ui:errors.updates.checkFailed]]», проверьте сеть или прокси. «[[ui:errors.updates.notConfigured]]» означает, что сборка сделана без ключа обновлений — например, это сборка для разработки или тестирования. Тогда скачивайте новые версии со страницы релизов: https://github.com/vvnovg/glafira-postgres-releases/releases

Организация может выключить проверку обновлений через политику или направить её на внутреннее зеркало. Когда проверка выключена, в разделе [[ui:about.updates.title]] после нажатия [[ui:about.updates.check]] появляется «[[ui:errors.updates.managed]]». Как и «[[ui:errors.updates.notConfigured]]», это сообщение видно только после нажатия: проверка при запуске в таких случаях ничего не показывает.

## О программе {#about}

Откройте [[ui:sidebar.about]] внизу левой панели. На macOS этот экран есть и в меню Help — пункт [[ui:menu.about]].

На экране — краткое описание приложения и строки:

- [[ui:about.version]] — номер версии, и [[ui:about.build]] — идентификатор исходного кода, из которого собрано приложение. Указывайте оба значения в обращении.
- [[ui:about.system]] — операционная система и архитектура процессора.
- [[ui:about.license]] — Apache 2.0; ссылка открывает текст лицензии в браузере.
- [[ui:about.policy]] — «[[ui:about.policySet]]» или «[[ui:about.policyNone]]». Ошибки политики перечислены ниже.
- [[ui:about.dataDir]] — папка с профилями подключений и настройками.
- [[ui:about.logDir]] — папка с файлами логов.

Здесь же — разделы [[ui:about.updates.title]] и [[ui:about.diagnostics.title]].

## Архив диагностики {#diagnostics}

Архив диагностики собирает то, что нужно поддержке, чтобы разобраться в проблеме. Чтобы создать его:

1. Откройте [[ui:sidebar.about]].
2. В разделе [[ui:about.diagnostics.title]] нажмите [[ui:about.diagnostics.collect]]. На macOS можно также выбрать [[ui:menu.diagnostics]] в меню Help.
3. Выберите, куда сохранить файл. Предлагаемое имя — `diagnostics-<дата>.zip`.

Когда архив сохранён, на экране показывается путь к нему. В ZIP-файле:

- `info.json` — версия и сборка приложения, операционная система, пути к папкам данных и логов, а также состояние политики;
- `connections.json` — профили подключений: имена, окружения, виды подключений, адреса узлов и REST API Patroni, имена пользователей, базы, пути к файлам сертификатов и подмены адресов участников;
- `logs/` — файлы логов приложения.

Паролей в архиве нет: они хранятся в системном хранилище секретов, а пароли внутри адресов API Patroni вырезаются. Файлов сертификатов в архиве тоже нет. Имена хостов, имена пользователей и пути к файлам записаны открытым текстом, а в логах есть адреса узлов из сообщений об ошибках PostgreSQL и Patroni. Просмотрите архив, прежде чем отправлять его за пределы компании.

Само приложение ничего не отправляет. Приложите архив к обращению сами.

## Поддержать проект {#support}

Glafira бесплатна, её исходный код открыт под Apache License 2.0. Проект живёт на пожертвования. Пункт [[ui:support.title]] внизу левой панели, на экране [[ui:sidebar.about]] и, на macOS, в меню Help открывает страницу поддержки в браузере: https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md
```

- [ ] **Step 6: Документация i18n без Pro и лицензии**

`docs/i18n/README.md`: строку `Все языки бесплатны, лицензия на выбор языка не влияет. Термины PostgreSQL и Patroni и стиль переводов — [glossary.md](glossary.md).` заменить на `Термины PostgreSQL и Patroni и стиль переводов — [glossary.md](glossary.md).`; в разделе «Плюралы» `Пример: `license.expiring_*`.` заменить на `Пример: `sidebar.partitions_*`.`.

`docs/i18n/glossary.md`: в разделе «Не переводится» строку `Glafira, Pro, Free, PostgreSQL, Patroni, WAL, LSN, DCS, REST API, DNS, TCP, TLS, SSL, mTLS, PEM, JSON, CSV, SQL, ID;` заменить на `Glafira, PostgreSQL, Patroni, WAL, LSN, DCS, REST API, DNS, TCP, TLS, SSL, mTLS, PEM, JSON, CSV, SQL, ID;`; в таблице «Термины приложения» удалить строку `| plan | тариф | Free / Pro |` (строка `| license | лицензия | |` остаётся: это строка «Лицензия» в «О программе»).

- [ ] **Step 7: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag
pnpm i18n:check
pnpm test
pnpm test:scripts
grep -rn "\bPro\b\|Free\b\|license-updates\|tabs.license" docs/guide docs/i18n || echo "чисто"
```

Ожидается: `pnpm i18n:check` проходит (метки главы найдены в словарях, якоря у каждого `##`, ru отличается от en); vitest и тесты скриптов зелёные; grep ничего не находит.

- [ ] **Step 8: Commit**

```bash
git add docs/guide src/guide/chapters.ts src/components/help scripts/i18n-check.mjs scripts/i18n-check.test.mjs docs/i18n/README.md docs/i18n/glossary.md
git commit -q -F - <<'MSG'
docs(guide): глава «Обновления и диагностика» на en и ru; F1 на «О программе» открывает её

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 9: Скрипты релиза из Kafka, `THIRD_PARTY.md`, проверка совместимости dist

**Files:**
- Create: `scripts/check-release.mjs`, `scripts/check-release.test.mjs`, `scripts/make-latest-json.mjs`, `scripts/make-latest-json.test.mjs`, `scripts/make-pkg.sh`, `scripts/check-dist-compat.mjs`, `scripts/check-dist-compat.test.mjs`, `scripts/guide-publish.mjs`, `scripts/guide-publish.test.mjs`, `scripts/third-party.mjs`, `scripts/third-party.test.mjs`, `THIRD_PARTY.md`
- Modify: `package.json`

**Interfaces:**
- Consumes: `GUIDE_CHAPTERS` и `flatten` из `scripts/i18n-check.mjs` (задача 8), `plugins.updater.pubkey` и версии манифестов (задача 5), `deny.toml`-совместимые лицензии (задача 4).
- Produces: `node scripts/check-release.mjs [--dry-run] vX.Y.Z`, `node scripts/make-latest-json.mjs …`, `scripts/make-pkg.sh <App.app> <out.pkg> [identity]`, `node scripts/guide-publish.mjs <каталог>`, `node scripts/check-dist-compat.mjs dist/assets` (в `pnpm build`), `node scripts/third-party.mjs` → `THIRD_PARTY.md`; `pnpm guide:publish`. Их вызывает `release.yml` задачи 10 и чек-лист.

- [ ] **Step 1: Скопировать скрипты, которые не зависят от продукта**

```bash
cd /Users/vvnovg/postgres-patroni-diag
K=/Users/vvnovg/kafka-explorer/scripts
cp $K/check-release.mjs $K/check-release.test.mjs $K/make-latest-json.mjs $K/make-latest-json.test.mjs $K/make-pkg.sh $K/check-dist-compat.mjs $K/check-dist-compat.test.mjs scripts/
chmod +x scripts/make-pkg.sh
grep -il "kafka" scripts/check-release.mjs scripts/make-latest-json.mjs scripts/make-pkg.sh scripts/check-dist-compat.mjs || echo "упоминаний Kafka нет"
node --test scripts/check-release.test.mjs scripts/make-latest-json.test.mjs scripts/check-dist-compat.test.mjs
```

Ожидается: grep ничего не находит; три файла тестов зелёные.

- [ ] **Step 2: `scripts/guide-publish.mjs` с тестом**

```bash
cp /Users/vvnovg/kafka-explorer/scripts/guide-publish.mjs /Users/vvnovg/kafka-explorer/scripts/guide-publish.test.mjs scripts/
sed -i '' 's/"# Glafira for Apache Kafka — User guide"/"# Glafira for PostgreSQL — User guide"/' scripts/guide-publish.mjs
grep -n "User guide" scripts/guide-publish.mjs   # ожидается строка с «Glafira for PostgreSQL — User guide»
```

В `scripts/guide-publish.test.mjs` последний тест (`publishGuide на реальном репозитории: главы без меток и {#`) заменить на:

```js
test("publishGuide на реальном репозитории: главы без меток и {#", () => {
  const out = mkdtempSync(join(tmpdir(), "guide-real-"));
  try {
    publishGuide(process.cwd(), out);
    const readme = readFileSync(join(out, "README.md"), "utf8");
    assert.ok(readme.startsWith("# Glafira for PostgreSQL — User guide"), readme);
    for (const n of ["English", "Русский"]) {
      assert.ok(readme.includes(n), n);
    }
    for (const c of ["en", "ru"]) {
      const files = readdirSync(join(out, c));
      assert.equal(files.length, GUIDE_CHAPTERS.length, c);
      for (const f of files) {
        const t = readFileSync(join(out, c, f), "utf8");
        assert.ok(!t.includes("[[ui:") && !t.includes("{#"), `${c}/${f}`);
      }
    }
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});
```

Проверка:

```bash
node --test scripts/guide-publish.test.mjs
node scripts/guide-publish.mjs /private/tmp/claude-501/-Users-vvnovg-kafka-explorer/ab751a67-d55c-4003-af6a-202ce9560f1e/scratchpad/guide-out && ls /private/tmp/claude-501/-Users-vvnovg-kafka-explorer/ab751a67-d55c-4003-af6a-202ce9560f1e/scratchpad/guide-out/*
```

Ожидается: тесты зелёные; в каталоге `README.md`, `en/getting-started.md`, `en/updates.md`, `ru/getting-started.md`, `ru/updates.md`.

- [ ] **Step 3: `scripts/third-party.mjs`**

```js
#!/usr/bin/env node
// THIRD_PARTY.md — лицензии стороннего кода в сборке: крейты Rust (cargo metadata, без dev-зависимостей),
// пакеты npm, попадающие во фронтенд (pnpm licenses --prod), и C-библиотеки, которые собирают
// openssl-src и aws-lc-sys и которых cargo не видит. Запуск из корня репозитория: `node scripts/third-party.mjs`.
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** C-библиотеки внутри бинаря: собираются из исходников при `cargo build`. */
export const C_LIBRARIES = [
  {
    name: "OpenSSL 3",
    license: "Apache-2.0",
    url: "https://www.openssl.org/",
    note: "openssl-src, feature vendored (openssl, postgres-openssl)",
  },
  {
    name: "AWS-LC",
    license: "ISC, Apache-2.0, MIT, BSD-3-Clause (see the license expression of aws-lc-sys)",
    url: "https://github.com/aws/aws-lc",
    note: "aws-lc-sys (reqwest → rustls → aws-lc-rs)",
  },
];

// Сравнение по кодовым точкам, а не localeCompare: порядок не зависит от локали и версии ICU.
const cmp = (x, y) => (x < y ? -1 : x > y ? 1 : 0);
const byNameVersion = (a, b) => cmp(a.name, b.name) || cmp(a.version, b.version);

/**
 * Крейты, которые попадают в сборку: обход графа `resolve` от корня по обычным и build-зависимостям
 * (dev-зависимости не входят в бинарь). Локальные крейты (без `source`) пропускаются.
 */
export function rustRows(metadata) {
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]));
  const packages = new Map(metadata.packages.map((p) => [p.id, p]));
  const seen = new Set();
  const queue = [metadata.resolve.root];
  while (queue.length) {
    const id = queue.shift();
    if (seen.has(id)) continue;
    seen.add(id);
    for (const dep of nodes.get(id)?.deps ?? []) {
      if (dep.dep_kinds.some((k) => k.kind !== "dev")) queue.push(dep.pkg);
    }
  }
  return [...seen]
    .map((id) => packages.get(id))
    .filter((p) => p && p.source !== null)
    .map((p) => ({
      name: p.name,
      version: p.version,
      license: p.license ?? (p.license_file ? `see ${p.license_file} in the crate` : "unknown"),
      url: p.repository ?? "",
    }))
    .sort(byNameVersion);
}

/** Пакеты из вывода `pnpm licenses list --prod --json` ({ лицензия: [{ name, versions, homepage }] }). */
export function npmRows(byLicense) {
  const rows = [];
  for (const [license, pkgs] of Object.entries(byLicense)) {
    for (const p of pkgs) for (const version of p.versions) rows.push({ name: p.name, version, license, url: p.homepage ?? "" });
  }
  return rows.sort(byNameVersion);
}

const cell = (s) => String(s).replace(/\|/g, "\\|");
const table = (header, rows) => [`| ${header.join(" | ")} |`, `|${header.map(() => "---").join("|")}|`, ...rows.map((r) => `| ${r.map(cell).join(" | ")} |`)];

/** Markdown THIRD_PARTY.md. Без дат и путей, порядок строк не зависит от локали — файл детерминирован и пригоден для diff. */
export function render({ rust, npm, cLibraries = C_LIBRARIES }) {
  return [
    "# Third-party software",
    "",
    "Glafira for PostgreSQL is licensed under the Apache License 2.0 (see `LICENSE`). The application also contains",
    "the third-party software listed below, each under its own license. The list is generated by",
    "`node scripts/third-party.mjs` from `cargo metadata` and `pnpm licenses`; the C libraries are listed by hand,",
    "because they are compiled from source by the Rust `-sys` crates and are invisible to Cargo's license data.",
    "",
    "## C libraries compiled into the binary",
    "",
    ...table(["Library", "License", "Source", "Built by"], cLibraries.map((l) => [l.name, l.license, l.url, l.note])),
    "",
    `## Rust crates (${rust.length})`,
    "",
    ...table(["Crate", "Version", "License", "Repository"], rust.map((r) => [r.name, r.version, r.license, r.url])),
    "",
    `## npm packages in the frontend (${npm.length})`,
    "",
    ...table(["Package", "Version", "License", "Homepage"], npm.map((r) => [r.name, r.version, r.license, r.url])),
    "",
  ].join("\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const run = (cmd, args) => JSON.parse(execFileSync(cmd, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 }));
  const metadata = run("cargo", ["metadata", "--manifest-path", "src-tauri/Cargo.toml", "--format-version", "1", "--locked"]);
  const npm = run("pnpm", ["licenses", "list", "--prod", "--json"]);
  const rust = rustRows(metadata);
  const npmList = npmRows(npm);
  writeFileSync("THIRD_PARTY.md", render({ rust, npm: npmList }));
  console.log(`THIRD_PARTY.md: ${rust.length} крейтов Rust, ${npmList.length} пакетов npm, ${C_LIBRARIES.length} C-библиотек`);
}
```

- [ ] **Step 4: `scripts/third-party.test.mjs`**

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { C_LIBRARIES, npmRows, render, rustRows } from "./third-party.mjs";

const pkg = (id, name, version, extra = {}) => ({ id, name, version, source: "registry+https://github.com/rust-lang/crates.io-index", license: "MIT", license_file: null, repository: `https://example.org/${name}`, ...extra });
const metadata = {
  packages: [
    pkg("app", "glafira-pg", "0.1.0", { source: null, license: "Apache-2.0" }),
    pkg("a", "alpha", "1.0.0"),
    pkg("b", "beta", "2.0.0", { license: "MIT OR Apache-2.0" }),
    pkg("c", "gamma", "0.1.0", { license: null, license_file: "LICENSE.txt" }),
    pkg("d", "devonly", "9.9.9"),
    pkg("e", "unused", "0.0.1"),
    pkg("f", "serde_json", "1.0.0"),
    pkg("g", "serde-untagged", "0.1.0"),
    pkg("h", "Inflector", "0.11.0"),
    pkg("l", "pg-advisor", "0.1.0", { source: null, license: "Apache-2.0" }),
  ],
  resolve: {
    root: "app",
    nodes: [
      { id: "app", deps: [{ pkg: "a", dep_kinds: [{ kind: null }] }, { pkg: "b", dep_kinds: [{ kind: "build" }] }, { pkg: "d", dep_kinds: [{ kind: "dev" }] }, { pkg: "f", dep_kinds: [{ kind: null }] }, { pkg: "g", dep_kinds: [{ kind: null }] }, { pkg: "h", dep_kinds: [{ kind: null }] }, { pkg: "l", dep_kinds: [{ kind: null }] }] },
      { id: "a", deps: [{ pkg: "c", dep_kinds: [{ kind: null }] }] },
      { id: "b", deps: [] },
      { id: "c", deps: [] },
      { id: "d", deps: [{ pkg: "e", dep_kinds: [{ kind: null }] }] },
      { id: "e", deps: [] },
      { id: "f", deps: [] },
      { id: "g", deps: [] },
      { id: "h", deps: [] },
      { id: "l", deps: [] },
    ],
  },
};

test("rustRows: обычные и build-зависимости от корня, без dev, без локальных крейтов", () => {
  const rows = rustRows(metadata);
  assert.deepEqual(
    rows.filter((r) => ["alpha", "beta", "gamma"].includes(r.name)).map((r) => `${r.name} ${r.version} ${r.license}`),
    ["alpha 1.0.0 MIT", "beta 2.0.0 MIT OR Apache-2.0", "gamma 0.1.0 see LICENSE.txt in the crate"],
  );
  assert.equal(rows.find((r) => r.name === "alpha").url, "https://example.org/alpha");
  assert.ok(!rows.some((r) => ["glafira-pg", "pg-advisor", "devonly", "unused"].includes(r.name)));
});

test("rustRows: порядок по кодовым точкам, а не по локали («-» раньше «_», заглавные раньше строчных)", () => {
  // localeCompare дал бы alpha, beta, gamma, Inflector, serde_json, serde-untagged.
  assert.deepEqual(
    rustRows(metadata).map((r) => r.name),
    ["Inflector", "alpha", "beta", "gamma", "serde-untagged", "serde_json"],
  );
});

test("npmRows: группы по лицензии разворачиваются в строки по версиям", () => {
  const rows = npmRows({
    MIT: [{ name: "react", versions: ["19.1.0"], homepage: "https://react.dev" }, { name: "i18next", versions: ["25.0.0", "25.1.0"] }],
    "Apache-2.0": [{ name: "zustand", versions: ["5.0.0"] }],
  });
  assert.deepEqual(
    rows.map((r) => `${r.name}@${r.version}:${r.license}`),
    ["i18next@25.0.0:MIT", "i18next@25.1.0:MIT", "react@19.1.0:MIT", "zustand@5.0.0:Apache-2.0"],
  );
  assert.equal(rows[2].url, "https://react.dev");
  assert.equal(rows[0].url, "");
});

test("render: три раздела, счётчики, C-библиотеки и экранирование «|»", () => {
  const md = render({
    rust: [{ name: "a|b", version: "1.0.0", license: "MIT", url: "" }],
    npm: [{ name: "react", version: "19.1.0", license: "MIT", url: "https://react.dev" }],
  });
  assert.match(md, /^# Third-party software/);
  assert.match(md, /Glafira for PostgreSQL is licensed under the Apache License 2\.0/);
  assert.match(md, /## C libraries compiled into the binary/);
  for (const lib of C_LIBRARIES) assert.ok(md.includes(`| ${lib.name} | ${lib.license} |`), lib.name);
  assert.ok(C_LIBRARIES.some((l) => l.name === "OpenSSL 3") && C_LIBRARIES.some((l) => l.name === "AWS-LC"));
  assert.match(md, /## Rust crates \(1\)/);
  assert.ok(md.includes("| a\\|b | 1.0.0 | MIT |"));
  assert.match(md, /## npm packages in the frontend \(1\)/);
  assert.ok(md.includes("| react | 19.1.0 | MIT | https://react.dev |"));
  assert.ok(md.endsWith("\n"));
});
```

- [ ] **Step 5: `package.json` — сборка с проверкой dist и публикация руководства**

В `scripts` строку `"build": "tsc && vite build",` заменить на `"build": "tsc && vite build && node scripts/check-dist-compat.mjs dist/assets",`; после `"i18n:check": …` добавить `"guide:publish": "node scripts/guide-publish.mjs dist/guide"` (с запятой у предыдущей строки).

- [ ] **Step 6: Сгенерировать `THIRD_PARTY.md` и проверить детерминизм**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
cargo update -w --offline --manifest-path src-tauri/Cargo.toml
node scripts/third-party.mjs
cp THIRD_PARTY.md /private/tmp/claude-501/-Users-vvnovg-kafka-explorer/ab751a67-d55c-4003-af6a-202ce9560f1e/scratchpad/third-party-first.md
node scripts/third-party.mjs
cmp THIRD_PARTY.md /private/tmp/claude-501/-Users-vvnovg-kafka-explorer/ab751a67-d55c-4003-af6a-202ce9560f1e/scratchpad/third-party-first.md && echo "детерминирован"
grep -c "^| " THIRD_PARTY.md
grep -n "aws-lc-sys\|openssl-src\|tauri-plugin-updater\|tokio-postgres" THIRD_PARTY.md | head
```

Ожидается: «детерминирован»; в таблице Rust есть `aws-lc-sys`, `openssl-src`, `tauri-plugin-updater`, `tokio-postgres`; нет `license-format`. Если `cargo metadata --locked` падает из-за рассинхронизированного `Cargo.lock` — выполнить `cargo build --manifest-path src-tauri/Cargo.toml` и повторить.

- [ ] **Step 7: Проверки**

```bash
pnpm test:scripts
pnpm build
node scripts/check-release.mjs --dry-run v0.1.0; echo "exit $?"
```

Ожидается: все тесты скриптов зелёные (включая `third-party`, `guide-publish`, `check-dist-compat`); `pnpm build` печатает `✓ dist/assets: чанки совместимы с WebKit macOS 11+`; `check-release.mjs --dry-run` завершается с кодом 1 и ошибкой «CHANGELOG.md: нет раздела «## 0.1.0»» (файл появится в задаче 10) плюс предупреждение о заглушке ключа — это ожидаемо.

- [ ] **Step 8: Commit**

```bash
git add scripts package.json THIRD_PARTY.md
git commit -q -F - <<'MSG'
build(release): скрипты релиза из Kafka — проверка релиза, latest.json, .pkg, руководство, совместимость dist; THIRD_PARTY.md

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 10: `release.yml`, `CHANGELOG.md`, чек-лист релиза, документы репозитория релизов, `FUNDING.yml`

**Files:**
- Create: `.github/workflows/release.yml`, `CHANGELOG.md`, `docs/release-checklist.md`, `docs/releases-repo/README.md`, `docs/releases-repo/SUPPORT.md`, `.github/FUNDING.yml`

**Interfaces:**
- Consumes: скрипты задачи 9 (`check-release.mjs`, `make-latest-json.mjs`, `make-pkg.sh`, `guide-publish.mjs`), `CHANGELOG.md` с разделом `## 0.1.0` (описание релиза), `LICENSE`/`NOTICE` (задача 11 — workflow копирует их в репозиторий релизов при публикации).
- Produces: workflow `release` (по тегу `v*` и `workflow_dispatch` с `dry_run`), документы для репозитория `vvnovg/glafira-postgres-releases`.

- [ ] **Step 1: `CHANGELOG.md`**

```markdown
# Changelog

## 0.1.0
- Первый выпуск Glafira for PostgreSQL: диагностика PostgreSQL и кластеров Patroni.
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API). Вставка строки подключения (JDBC, `postgresql://…`, `host=… port=…`); пароли в системном хранилище секретов, иначе `~/.pgpass`.
- Проверка подключения по шагам и чек-лист готовности к диагностике: роль `glafira_diag`, `pg_stat_statements`, параметры сервера.
- Обзор кластера: участники с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, материализованные представления, функции, последовательности; партиции, роли, расширения.
- Папки подключений, экспорт и импорт подключений без паролей, дублирование подключения.
- Интерфейс на английском и русском, подсказки «?», встроенное руководство: вкладка «Справка», F1, меню Help на macOS.
- Политика администратора (`updatesEnabled`, `updateUrl`, `language`): профиль MDM на macOS, реестр на Windows, `/etc` на Linux.
- Автообновление: проверка при запуске и в «О программе» → «Проверить обновления»; организация может выключить обновления или указать зеркало.
- «О программе»: версия, сборка, политика, архив диагностики (версия, логи, профили без паролей).
- Исходный код открыт под Apache License 2.0, все функции бесплатны. Пункт «Поддержать проект» — внизу левой панели, в «О программе» и в меню Help (macOS).
- Установщики: macOS (`.dmg`, `.pkg`; подписаны Developer ID и нотаризованы), Windows (`.msi`), Linux (`.deb`, `.rpm`, `.AppImage`).
```

- [ ] **Step 2: `.github/workflows/release.yml`**

```yaml
name: release

on:
  push:
    tags: ["v*"]
  # Пробный прогон: сборка всех платформ и latest.json без публикации.
  # Запуск вручную возможен, только когда release.yml уже в ветке по умолчанию (main).
  workflow_dispatch:
    inputs:
      dry_run:
        description: "Пробный прогон (без публикации; публикация — только по тегу v*)"
        type: boolean
        default: true

permissions:
  contents: read

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

env:
  RELEASES_REPO: vvnovg/glafira-postgres-releases

jobs:
  check:
    runs-on: ubuntu-22.04
    outputs:
      version: ${{ steps.v.outputs.version }}
      tag: ${{ steps.v.outputs.tag }}
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
        with:
          persist-credentials: false
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
      - run: node --test "scripts/*.test.mjs"
      # Руководство собирается и в пробном прогоне: ошибка в главах видна до релиза.
      - name: Сборка руководства (проверка)
        run: node scripts/guide-publish.mjs guide-out
      # По тегу — версия из тега и строгая проверка; вручную — версия из package.json,
      # а проблема с ключом updater (заглушка в tauri.conf.json) — только предупреждение.
      - id: v
        env:
          EVENT: ${{ github.event_name }}
          DRY_RUN: ${{ inputs.dry_run }}
        run: |
          if [ "$EVENT" = "push" ]; then
            tag="$GITHUB_REF_NAME"
            node scripts/check-release.mjs "$tag"
          else
            if [ "$DRY_RUN" != "true" ]; then
              echo "::error::вручную доступен только пробный прогон; релиз публикуется пушем тега v*"; exit 1
            fi
            tag="v$(node -p 'require("./package.json").version')"
            node scripts/check-release.mjs --dry-run "$tag"
          fi
          echo "tag=$tag" >> "$GITHUB_OUTPUT"
          echo "version=${tag#v}" >> "$GITHUB_OUTPUT"
      - name: Секрет подписи updater задан
        env:
          KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
        run: |
          if [ -z "$KEY" ]; then echo "::error::нет секрета TAURI_SIGNING_PRIVATE_KEY"; exit 1; fi

  build:
    needs: check
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-14
            target: aarch64-apple-darwin
            arch: aarch64
          # macos-13 выведен из эксплуатации; Intel-раннер — macos-15-intel.
          - os: macos-15-intel
            target: x86_64-apple-darwin
            arch: x64
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            arch: x64
          - os: ubuntu-22.04
            target: x86_64-unknown-linux-gnu
            arch: amd64
    runs-on: ${{ matrix.os }}
    env:
      BUNDLE: src-tauri/target/${{ matrix.target }}/release/bundle
      VERSION: ${{ needs.check.outputs.version }}
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
        with:
          persist-credentials: false
      - uses: pnpm/action-setup@b906affcce14559ad1aafd4ab0e942779e9f58b1 # v4
        with:
          version: 11
      # Без кэшей (ни pnpm, ни rust-cache): в этом job есть ключ подписи updater и секреты Apple,
      # а кэш, записанный другим запуском (ci.yml на main), мог бы подменить зависимости или
      # артефакты сборки. Релиз собирается с нуля — дольше, но только из lock-файлов.
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
      - uses: dtolnay/rust-toolchain@b725eda525362e7cf34af9a7243cac1e3f01a7ec # ветка stable
        with:
          toolchain: stable
          targets: ${{ matrix.target }}
      - name: Системные зависимости (Linux)
        if: runner.os == 'Linux'
        # cmake и NASM не нужны: aws-lc-sys собирается крейтом cc, vendored OpenSSL — Perl и make из образа раннера.
        run: |
          sudo apt-get update
          sudo apt-get install -y libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf libsecret-1-dev rpm
      - run: pnpm install --frozen-lockfile
      # Секреты Apple — только в env шага tauri-action (не в GITHUB_ENV: иначе их видели бы все
      # следующие шаги). Пустые APPLE_* ломают сборку (bundler импортирует пустой сертификат),
      # поэтому шагов два: с подписью — когда заданы все секреты, без подписи — когда не задан ни один.
      - name: Подпись Apple задана?
        id: apple
        if: runner.os == 'macOS'
        env:
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
        run: |
          set=0; missing=""
          for name in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
            if [ -n "${!name}" ]; then set=$((set + 1)); else missing="$missing $name"; fi
          done
          if [ "$set" -eq 0 ]; then
            echo "::warning::macOS-сборка без подписи и нотаризации"
            echo "signing=false" >> "$GITHUB_OUTPUT"
          elif [ -n "$missing" ]; then
            echo "::error::секреты Apple заданы не все, нет:$missing"; exit 1
          else
            echo "signing=true" >> "$GITHUB_OUTPUT"
          fi
      - name: Сборка (с подписью Apple)
        if: steps.apple.outputs.signing == 'true'
        uses: tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5 # action-v0.6.2
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
          APPLE_CERTIFICATE: ${{ secrets.APPLE_CERTIFICATE }}
          APPLE_CERTIFICATE_PASSWORD: ${{ secrets.APPLE_CERTIFICATE_PASSWORD }}
          APPLE_SIGNING_IDENTITY: ${{ secrets.APPLE_SIGNING_IDENTITY }}
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_PASSWORD: ${{ secrets.APPLE_PASSWORD }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
        with:
          args: --target ${{ matrix.target }}
      - name: Сборка
        if: steps.apple.outputs.signing != 'true'
        uses: tauri-apps/tauri-action@84b9d35b5fc46c1e45415bdb6144030364f7ebc5 # action-v0.6.2
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
        with:
          args: --target ${{ matrix.target }}
      # Tauri называет архив updater «<productName>.app.tar.gz» без версии и архитектуры:
      # обе macOS-сборки дали бы одно имя, а make-latest-json.mjs не смог бы различить платформы.
      - name: Архив updater с архитектурой в имени (macOS)
        if: runner.os == 'macOS'
        shell: bash
        run: |
          shopt -s nullglob
          archives=("$BUNDLE"/macos/*.app.tar.gz)
          if [ "${#archives[@]}" -ne 1 ]; then echo "::error::ожидался один .app.tar.gz, найдено ${#archives[@]}"; exit 1; fi
          src="${archives[0]}"
          name="$(basename "$src" .app.tar.gz)"
          dst="$BUNDLE/macos/${name}_${VERSION}_${{ matrix.arch }}.app.tar.gz"
          mv "$src" "$dst"
          mv "$src.sig" "$dst.sig"
      # .pkg подписывается Developer ID Installer (если задан APPLE_INSTALLER_IDENTITY), но отдельно
      # не нотаризуется (notarytool submit + stapler staple) — доработка на потом.
      - name: .pkg (macOS)
        if: runner.os == 'macOS'
        shell: bash
        env:
          INSTALLER_IDENTITY: ${{ secrets.APPLE_INSTALLER_IDENTITY }}
        run: |
          shopt -s nullglob
          apps=("$BUNDLE"/macos/*.app)
          if [ "${#apps[@]}" -ne 1 ]; then echo "::error::ожидался один .app, найдено ${#apps[@]}"; exit 1; fi
          app="${apps[0]}"
          name="$(basename "$app" .app)"
          scripts/make-pkg.sh "$app" "$BUNDLE/macos/${name}_${VERSION}_${{ matrix.arch }}.pkg" "$INSTALLER_IDENTITY"
      - name: Тихая установка MSI (проверка)
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $msi = Get-ChildItem "$env:BUNDLE/msi/*.msi" | Select-Object -First 1
          $p = Start-Process msiexec -ArgumentList "/i `"$($msi.FullName)`" /qn /l*v install.log" -Wait -PassThru
          if ($p.ExitCode -ne 0) { Get-Content install.log -Tail 50; exit 1 }
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2
        with:
          name: bundle-${{ matrix.target }}
          path: |
            ${{ env.BUNDLE }}/dmg/*.dmg
            ${{ env.BUNDLE }}/macos/*.pkg
            ${{ env.BUNDLE }}/macos/*.app.tar.gz
            ${{ env.BUNDLE }}/macos/*.app.tar.gz.sig
            ${{ env.BUNDLE }}/msi/*.msi
            ${{ env.BUNDLE }}/msi/*.msi.sig
            ${{ env.BUNDLE }}/deb/*.deb
            ${{ env.BUNDLE }}/deb/*.deb.sig
            ${{ env.BUNDLE }}/rpm/*.rpm
            ${{ env.BUNDLE }}/rpm/*.rpm.sig
            ${{ env.BUNDLE }}/appimage/*.AppImage
            ${{ env.BUNDLE }}/appimage/*.AppImage.sig
          if-no-files-found: error

  assemble:
    needs: [check, build]
    runs-on: ubuntu-22.04
    env:
      VERSION: ${{ needs.check.outputs.version }}
      TAG: ${{ needs.check.outputs.tag }}
    steps:
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
        with:
          persist-credentials: false
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
      - uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4.3.0
        with:
          pattern: bundle-*
          path: release/assets
          merge-multiple: true
      # upload-artifact сохраняет подкаталоги бандла (dmg/, macos/, msi/…): складываем всё в один каталог.
      - name: Плоский набор артефактов
        run: |
          before=$(find release/assets -type f | wc -l)
          find release/assets -mindepth 2 -type f -print0 | while IFS= read -r -d '' f; do
            dst="release/assets/$(basename "$f")"
            if [ -e "$dst" ]; then echo "::error::конфликт имён артефактов: $(basename "$f")"; exit 1; fi
            mv "$f" "$dst"
          done
          find release/assets -mindepth 1 -type d -empty -delete
          after=$(find release/assets -maxdepth 1 -type f | wc -l)
          if [ "$before" -ne "$after" ]; then echo "::error::после выравнивания $after файлов из $before"; exit 1; fi
      # GitHub сам заменяет пробелы в именах ассетов, и URL в latest.json перестали бы совпадать
      # с реальными. Переименовываем заранее (подпись .sig от имени файла не зависит).
      - name: Имена артефактов без пробелов
        run: |
          shopt -s nullglob
          for f in release/assets/*; do
            base="$(basename "$f")"
            new="${base// /_}"
            if [ "$base" != "$new" ]; then mv "$f" "release/assets/$new"; fi
          done
          ls -la release/assets
      - name: Описание и latest.json
        run: |
          awk -v v="## $VERSION" '$0==v{f=1;next} /^## /{f=0} f' CHANGELOG.md > release/notes.md
          if [ ! -s release/notes.md ]; then echo "::error::пустой раздел CHANGELOG для $VERSION"; exit 1; fi
          node scripts/make-latest-json.mjs --version "$VERSION" --notes-file release/notes.md \
            --base-url "https://github.com/${RELEASES_REPO}/releases/download/${TAG}" --dir release/assets \
            --require-all > latest.json
          mv latest.json release/assets/latest.json
          cat release/assets/latest.json
      # И для пробного прогона, и для публикации: готовый набор ассетов + описание.
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2
        with:
          name: release-${{ needs.check.outputs.tag }}
          path: release
          if-no-files-found: error

  publish:
    needs: [check, assemble]
    if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')
    runs-on: ubuntu-22.04
    env:
      VERSION: ${{ needs.check.outputs.version }}
      TAG: ${{ needs.check.outputs.tag }}
    steps:
      - name: Токен публикации задан
        env:
          RELEASES_TOKEN: ${{ secrets.RELEASES_TOKEN }}
        run: |
          if [ -z "$RELEASES_TOKEN" ]; then echo "::error::нет секрета RELEASES_TOKEN"; exit 1; fi
      - uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093 # v4.3.0
        with:
          name: release-${{ needs.check.outputs.tag }}
          path: release
      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4.4.0
        with:
          persist-credentials: false
          path: src
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: 22
      # Скрипт использует только модули node: — зависимости не нужны.
      - name: Сборка руководства
        working-directory: src
        run: node scripts/guide-publish.mjs ../guide-out
      # Атомарно: черновик со всеми ассетами, и только потом — публикация с пометкой latest.
      # Бета публикуется обычным релизом: updater берёт releases/latest/download/latest.json,
      # а без явного --latest GitHub может не пометить тег -beta как последний.
      - name: Публикация в репозиторий релизов
        env:
          GH_TOKEN: ${{ secrets.RELEASES_TOKEN }}
        run: |
          gh release create "$TAG" release/assets/* --repo "$RELEASES_REPO" --title "$TAG" --notes-file release/notes.md --draft
          gh release edit "$TAG" --repo "$RELEASES_REPO" --draft=false --latest
      # Каталог guide/, README, SUPPORT.md, LICENSE и NOTICE репозитория релизов — одним коммитом «guide: $TAG»,
      # только если что-то изменилось. Токен — только через GH_TOKEN и credential helper gh, в URL и логи не попадает.
      # После публикации релиза: руководство не опережает релиз, если тот не создался. Сборка руководства —
      # выше, до релиза, и ломает job заранее. Сбой самого push релиз не отменяет (continue-on-error):
      # повтор job упадёт на gh release create, поэтому руководство тогда докладывают вручную
      # (pnpm guide:publish и коммит в guide/ репозитория релизов).
      - name: Руководство, README, SUPPORT.md, LICENSE и NOTICE в репозиторий релизов
        continue-on-error: true
        env:
          GH_TOKEN: ${{ secrets.RELEASES_TOKEN }}
        run: |
          gh auth setup-git
          git clone --depth 1 "https://github.com/${RELEASES_REPO}.git" releases-repo
          rm -rf releases-repo/guide
          cp -R guide-out releases-repo/guide
          cp src/docs/releases-repo/README.md releases-repo/README.md
          cp src/docs/releases-repo/SUPPORT.md releases-repo/SUPPORT.md
          cp src/LICENSE releases-repo/LICENSE
          cp src/NOTICE releases-repo/NOTICE
          cd releases-repo
          git add -A guide README.md SUPPORT.md LICENSE NOTICE
          if git diff --cached --quiet; then echo "руководство, README, SUPPORT.md, LICENSE и NOTICE без изменений"; exit 0; fi
          git -c user.name="github-actions[bot]" -c user.email="41898282+github-actions[bot]@users.noreply.github.com" \
            commit -m "guide: $TAG"
          git push origin HEAD
```

- [ ] **Step 3: `docs/releases-repo/README.md` и `SUPPORT.md`, `.github/FUNDING.yml`**

`docs/releases-repo/README.md`:

```markdown
# Glafira for PostgreSQL — releases

Desktop app for diagnosing PostgreSQL servers and Patroni clusters. Download the installer for your OS from [Releases](../../releases/latest).

User guide: [English](guide/en/getting-started.md) · [Русский](guide/ru/getting-started.md)

## First launch
macOS builds are signed with an Apple Developer ID and notarized: the app opens without extra steps.

**Windows** — the installer is not signed yet, so SmartScreen warns on first launch: click "More info" → "Run anyway",
or unblock the installer in PowerShell:
```powershell
Unblock-File -Path "$env:USERPROFILE\Downloads\Glafira_for_PostgreSQL_*_x64_en-US.msi"
```
Updates installed from inside the app don't need this.

## Source code and support
Glafira is free and open source under the Apache License 2.0: https://github.com/vvnovg/glafira-postgres.
All features are available to everyone. If Glafira is useful to you, see [SUPPORT.md](SUPPORT.md).

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada. This project is not affiliated with it.
```

`docs/releases-repo/SUPPORT.md`:

```markdown
# Support the project

Glafira for PostgreSQL is free and open source (Apache License 2.0). All features are available to everyone;
there are no paid plans. Donations pay for the Apple Developer Program membership and for the time spent on support and
new releases.

- GitHub Sponsors: https://github.com/sponsors/vvnovg

Thank you!

---

# Поддержать проект

Glafira for PostgreSQL бесплатна и открыта (Apache License 2.0). Все функции доступны всем, платных тарифов нет.
Пожертвования идут на членство в Apple Developer Program и на время, которое уходит на поддержку и новые выпуски.

- GitHub Sponsors: https://github.com/sponsors/vvnovg
- В рублях: https://pay.cloudtips.ru/p/0eee5125

Спасибо!
```

`.github/FUNDING.yml`:

```yaml
github: vvnovg
custom: ["https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md"]
```

- [ ] **Step 4: `docs/release-checklist.md`**

```markdown
# Чек-лист релиза

## Один раз (владелец)
1. Ключ updater: `pnpm tauri signer generate -w ~/.tauri/glafira-postgres-updater.key`. Открытый ключ — в `src-tauri/tauri.conf.json` →
   `plugins.updater.pubkey` вместо заглушки `REPLACE_WITH_UPDATER_PUBKEY`, отдельным коммитом в `main`.
2. Публичный репозиторий `vvnovg/glafira-postgres-releases`: README из `docs/releases-repo/README.md`, `SUPPORT.md` из
   `docs/releases-repo/SUPPORT.md`, `LICENSE`, `NOTICE` и `.github/FUNDING.yml` (как в этом репозитории). Каждый релиз
   обновляет README, SUPPORT.md, LICENSE, NOTICE и `guide/` сам (`release.yml`).
3. Секреты репозитория `vvnovg/glafira-postgres` (Settings → Secrets and variables → Actions):
   `TAURI_SIGNING_PRIVATE_KEY` (содержимое файла ключа), `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
   `RELEASES_TOKEN` (fine-grained PAT: только `vvnovg/glafira-postgres-releases`, Contents: Read and write).
   Срок токена — 1 год; напоминание в календаре за 2 недели. Истёкший токен ломает только шаг publish:
   создать новый с теми же правами и `gh secret set RELEASES_TOKEN --repo vvnovg/glafira-postgres`, затем перезапустить job publish.
   Подпись Apple — все шесть или ни одного (с частью секретов сборка падает): `APPLE_CERTIFICATE` (.p12 в base64),
   `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` («Developer ID Application: Viacheslav Novgorodtsev (JHWW538CH9)»),
   `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID` (`JHWW538CH9`); при наличии — `APPLE_INSTALLER_IDENTITY`
   (Developer ID Installer) для подписи `.pkg`.
4. Резервная копия закрытого ключа updater вне компьютера разработки.

## Пробный прогон
Нужен `release.yml` в ветке `main`. `gh workflow run release -f dry_run=true` собирает все платформы и `latest.json`
без публикации. `node scripts/check-release.mjs --dry-run vX.Y.Z` превращает проблему с ключом updater в предупреждение;
сборка по тегу всегда строгая.

## Каждый релиз
1. Версия в `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`; раздел `## X.Y.Z` в `CHANGELOG.md`
   покрывает всё с прошлого тега — из него собирается описание релиза.
2. `cargo update -w --offline --manifest-path src-tauri/Cargo.toml` — новая версия попадает в `Cargo.lock`
   (без этого `cargo metadata --locked` в скрипте ниже падает), затем `node scripts/third-party.mjs` →
   обновлённый `THIRD_PARTY.md` (лицензии зависимостей); закоммитить.
3. `node scripts/check-release.mjs vX.Y.Z` → «можно выпускать».
4. `git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z` → workflow `release`. Релиз создаётся черновиком со всеми ассетами
   и публикуется атомарно (`--draft=false --latest`); updater читает `releases/latest/download/latest.json`
   (артефакты updater — формат v2).
5. Смоук на трёх ОС (ниже) по установщикам из релиза.

## Никогда не менять
- `bundle.windows.wix.upgradeCode` в `src-tauri/tauri.conf.json` (`a5dce14e-dd43-4ce6-9144-8437d8df8476`):
  по нему MSI новой версии обновляет установленную, а не ставится рядом. Без явного значения WiX выводит код из
  имени продукта, поэтому он задан явно и не меняется даже при переименовании продукта.
- `mainBinaryName` (`glafira-pg`) и `identifier` (`io.github.vvnovg.glafira.postgres`): от них зависят путь
  установки, имя процесса, данные, keychain и политика.
- Ключ updater (`plugins.updater.pubkey`): со сменой ключа все установленные версии перестанут принимать обновления.

## Известные ограничения
- `.pkg` подписывается Developer ID Installer, если задан `APPLE_INSTALLER_IDENTITY`, но отдельно не нотаризуется.
- Установщик Windows не подписан: SmartScreen предупреждает при первом запуске.

## Смоук установщиков
Стенд: `docker compose -f dev/docker-compose.yml up -d` и `scripts/wait-stand.sh` (README → «Стенд»).
- [ ] macOS (`.dmg` и `.pkg`): установка, запуск без предупреждений Gatekeeper (сборка подписана и нотаризована),
      подключение к серверу `localhost:5460` (`glafira_diag` / `glafira-secret`) и к кластеру Patroni (`http://localhost:8011`), «О программе» — версия.
- [ ] Windows (`.msi`, `msiexec /qn` из администраторской консоли): то же; в «Программах и компонентах» одна запись.
- [ ] Linux (`.deb`, `.rpm`, `.AppImage`): то же; бинарь `/usr/bin/glafira-pg`.
- [ ] Политика `updatesEnabled = false` (по `docs/admin.md`): «Обновления управляются вашей организацией», в логе нет запросов к серверу обновлений.
- [ ] Политика с `updateUrl: "http://…"`: в «О программе» ошибка политики, обновления выключены.
- [ ] «Собрать диагностику» → архив открывается, паролей нет.
- [ ] Пути данных и логов из `docs/admin.md` совпадают с «О программе» на каждой ОС.
- [ ] Обновление (начиная со второго релиза): установить предыдущую версию → «Проверить обновления» → «Установить и перезапустить» →
      новая версия, подключения и настройки на месте (для `.deb` и `.rpm` — отдельно: ставится пакет того же типа,
      запрашивается пароль администратора).

## Сквозная проверка обновления (по решению владельца; рекомендуется до первого релиза с updater или при изменениях updater)
Цель — до тега убедиться, что выпущенная версия сможет обновиться. Нужны боевой ключ updater (pubkey уже в
`tauri.conf.json`) и секреты `TAURI_SIGNING_PRIVATE_KEY*` — шаги выполняет владелец.

1. **Старая версия (A).** Ветка с версией выпуска, например `0.1.0`. Пробный прогон: `gh workflow run release -f dry_run=true`;
   скачать артефакт `release-v0.1.0` (установщики A).
2. **Новая версия (B).** Временная ветка от той же ревизии, версия `0.1.1` в `package.json`, `src-tauri/Cargo.toml`,
   `src-tauri/tauri.conf.json` и раздел `## 0.1.1` в `CHANGELOG.md` (ветку не сливать). Пробный прогон из неё
   (`gh workflow run release --ref <ветка> -f dry_run=true`, workflow должен быть в `main`) → артефакт `release-v0.1.1`.
3. **Тестовое зеркало (https).** Один из вариантов:
   - тестовый релиз в отдельном публичном репозитории (например, `vvnovg/glafira-postgres-updater-test`):
     `gh release create v0.1.1 <ассеты B> --repo …`, затем пересобрать `latest.json` с адресами этого репозитория:
     `node scripts/make-latest-json.mjs --version 0.1.1 --notes-file notes.md --base-url https://github.com/<repo>/releases/download/v0.1.1 --dir <ассеты B> --require-all`;
   - статический https-сервер в локальной сети (с доверенным сертификатом) с теми же файлами и `latest.json`,
     пересобранным с его `--base-url`.
   Проверить: `curl -fsSL <url>/latest.json` открывается, `curl -fsSI` по каждому `url` из него — 200.
4. **Политика с зеркалом** на каждой тестовой машине (`docs/admin.md` → «Политика»): `updateUrl` = адрес
   `latest.json` зеркала (только `https://`).
5. **На каждой ОС и каждом типе установки** — macOS `.dmg` и `.pkg` (aarch64; x64 — если есть машина),
   Windows `.msi`, Linux `.deb`, `.rpm`, `.AppImage`:
   - [ ] установить A, создать подключение с паролем и открыть вкладку «Обзор» (подключение открыто);
   - [ ] «О программе» → «Проверить обновления» → «Доступна версия 0.1.1»;
   - [ ] «Установить и перезапустить»;
   - [ ] после перезапуска «О программе» показывает 0.1.1; подключения, пароли и настройки на месте;
   - [ ] на сервере PostgreSQL после перезапуска нет «зависших» сессий `application_name = glafira-pg` от прежнего процесса
         (`SELECT pid, state FROM pg_stat_activity WHERE application_name = 'glafira-pg';`) — на **Windows** это проверка хука
         выхода (`on_before_exit` закрывает подключения перед запуском MSI);
   - [ ] Windows: в «Программах и компонентах» одна запись версии 0.1.1 (upgradeCode сработал), установка
         требует UAC; под пользователем без прав администратора — понятная ошибка, A продолжает работать;
   - [ ] macOS `.pkg` (приложение в `/Applications` принадлежит root): запрос пароля администратора, после
         установки владелец и права `.app` не изменились неожиданно (`ls -ld /Applications/Glafira*.app`);
         отказ от ввода пароля — ошибка, A продолжает работать. macOS `.dmg` (приложение перетащено
         пользователем): обновление без пароля;
   - [ ] Linux `.deb`/`.rpm`: запрос пароля (pkexec), `dpkg -l | grep glafira` / `rpm -q` показывает 0.1.1;
         `.AppImage`: файл заменён на месте, права на исполнение сохранены.
6. **Отрицательные проверки:** `latest.json` с испорченной подписью → ошибка установки, A работает;
   политика `updatesEnabled = false` → проверки нет.
7. Удалить временную ветку и тестовый релиз/зеркало. Только после этого ставить тег.
```

- [ ] **Step 5: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag
node scripts/check-release.mjs --dry-run v0.1.0; echo "exit $?"
awk -v v="## 0.1.0" '$0==v{f=1;next} /^## /{f=0} f' CHANGELOG.md | wc -l
node -e 'const y=require("node:fs").readFileSync(".github/workflows/release.yml","utf8"); for (const s of ["RELEASES_REPO: vvnovg/glafira-postgres-releases","cp src/LICENSE releases-repo/LICENSE","git add -A guide README.md SUPPORT.md LICENSE NOTICE","libsecret-1-dev rpm"]) if(!y.includes(s)) throw new Error("нет: "+s); console.log("release.yml ok")'
grep -rn "kafka\|Kafka" .github/workflows/release.yml docs/release-checklist.md docs/releases-repo CHANGELOG.md .github/FUNDING.yml || echo "упоминаний Kafka нет"
```

Ожидается: `check-release.mjs --dry-run` печатает `::warning::пробный прогон: tauri.conf.json: не задан открытый ключ updater …` и `✓ v0.1.0: пробный прогон разрешён`, exit 0; раздел CHANGELOG непустой (12 строк); `release.yml ok`; упоминаний Kafka нет.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml .github/FUNDING.yml CHANGELOG.md docs/release-checklist.md docs/releases-repo
git commit -q -F - <<'MSG'
ci(release): workflow релиза из Kafka — подпись Apple, latest.json, публикация в glafira-postgres-releases; CHANGELOG 0.1.0, чек-лист, SUPPORT.md

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 11: Корневые файлы лицензии, README, руководство администратора

**Files:**
- Create: `LICENSE`, `NOTICE`, `TRADEMARK.md`, `CONTRIBUTING.md`, `SECURITY.md`
- Modify: `README.md`, `docs/admin.md`

**Interfaces:**
- Consumes: `deny.toml` и команды проверок (задача 4), `THIRD_PARTY.md` (задача 9), чек-лист и репозиторий релизов (задача 10).
- Produces: `LICENSE` и `NOTICE` (их копирует `release.yml`), документация для публичного репозитория.

- [ ] **Step 1: `LICENSE` и `NOTICE`**

```bash
cd /Users/vvnovg/postgres-patroni-diag
cp /Users/vvnovg/kafka-explorer/LICENSE LICENSE
wc -l LICENSE   # ожидается 202
grep -n "Copyright \[yyyy\] \[name of copyright owner\]" LICENSE   # приложение с квадратными скобками — канон apache.org
```

`NOTICE`:

```
Glafira for PostgreSQL
Copyright 2026 Viacheslav Novgorodtsev

This product includes software developed by third parties under their own
licenses; see THIRD_PARTY.md for the list.

PostgreSQL is a registered trademark of the PostgreSQL Community Association
of Canada. This project is not affiliated with it.
```

- [ ] **Step 2: `TRADEMARK.md`, `CONTRIBUTING.md`, `SECURITY.md`**

`TRADEMARK.md`:

```markdown
# Trademarks

"Glafira", "Glafira for PostgreSQL" and the Glafira application icon are trademarks of Viacheslav Novgorodtsev.
The Apache License 2.0 that covers the source code does not grant any rights to these names or to the icon
(see section 6 of the license).

You may:

- say that your software works with Glafira or is compatible with it;
- link to this repository and to the official releases;
- use the name when describing or reviewing the application.

You may not, without written permission:

- distribute modified or rebuilt versions of the application under the name Glafira or with the Glafira icon —
  give your fork its own name and icon;
- use the name or the icon in a way that suggests the author endorses your product or service.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada. This project is not
affiliated with it.
```

`CONTRIBUTING.md`:

```markdown
# Contributing

Thank you for your interest in Glafira for PostgreSQL.

## Before you start

- Open an issue first for anything bigger than a small fix, so that the approach can be agreed on.
- The working language of the repository is Russian: comments, commit messages and `docs/` are written in Russian.
  Issues and pull requests in English are welcome.

## Building and testing

See `README.md` («Требования», «Запуск», «Тесты»). Before opening a pull request run:

```bash
pnpm test && pnpm test:scripts && pnpm i18n:check && pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check
cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/pg-advisor/Cargo.toml
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
```

Integration tests need the Docker stand described in `README.md` («Стенд»). `cargo-deny` can be installed with
`cargo install cargo-deny --locked --version 0.20.2`.

## Interface strings and the guide

Every new string goes to both dictionaries in `src/locales/` (`en.json` and `ru.json`) at once (see
`docs/i18n/README.md` and `docs/i18n/glossary.md`); `pnpm i18n:check` enforces it. Changes to the user guide go to
both languages in `docs/guide/`.

## Developer Certificate of Origin

Contributions are accepted under the Apache License 2.0 (`LICENSE`). Commits in pull requests from contributors
must be signed off (`git commit -s`), which certifies the
[Developer Certificate of Origin](https://developercertificate.org/). Pull requests whose commits lack
a `Signed-off-by` line are not merged.
```

`SECURITY.md`:

```markdown
# Security policy

## Supported versions

Only the latest release on https://github.com/vvnovg/glafira-postgres-releases/releases receives fixes.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's private vulnerability reporting: on the repository page open
the **Security** tab and click **Report a vulnerability**. Describe the problem, the app version and the steps
to reproduce it.

You will get a reply within 7 days. When a fix is released, the report is published as a security advisory with
credit to the reporter, unless you ask otherwise.

## What matters most

Glafira connects to PostgreSQL servers and Patroni REST APIs with the user's credentials. Reports about credential
handling (system secret store, `~/.pgpass`, logs, diagnostics archive), TLS verification, the administrator policy
and the updater are especially welcome.
```

- [ ] **Step 3: `README.md`**

Заменить содержимое целиком:

```markdown
# Glafira for PostgreSQL

Desktop-приложение (Tauri 2 + Rust + React) для диагностики PostgreSQL и кластеров Patroni.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada; this project is not affiliated with it.

Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`; этап P1: `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`.
Руководство администратора (установка, политика, роль для диагностики, сеть): `docs/admin.md`.

## Установка
Установщики (macOS `.dmg`/`.pkg`, Windows `.msi`, Linux `.deb`/`.rpm`/`.AppImage`) — в разделе релизов:
https://github.com/vvnovg/glafira-postgres-releases/releases

Сборки для macOS подписаны Developer ID и нотаризованы. Установщик Windows пока не подписан: SmartScreen → «Подробнее» → «Выполнить в любом случае».
Приложение проверяет обновления при запуске; вручную — «О программе» → «Проверить обновления».

Для администраторов (тихая установка, политика, зеркало обновлений): `docs/admin.md`.

## Что умеет (P1a)
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API).
- Вставка строки подключения: JDBC, `postgresql://…`, `host=… port=…`; пароль из `~/.pgpass` (права 0600: `chmod 0600 ~/.pgpass`, иначе файл игнорируется), если не сохранён.
- Топология с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Проверка подключения по шагам и чек-лист готовности к диагностике.
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, функции, последовательности; роли, расширения.
- Папки, экспорт и импорт подключений без секретов, дублирование подключения.
- Интерфейс на английском и русском, встроенное руководство (вкладка «Справка», F1, меню Help на macOS), автообновление.

## Лицензия и поддержка проекта
- Код открыт под [Apache License 2.0](LICENSE). Все функции приложения бесплатны и доступны всем: платных тарифов и лицензионных ключей нет.
- Имя Glafira и значок приложения лицензией не покрываются — см. `TRADEMARK.md`. Лицензии зависимостей — `THIRD_PARTY.md`.
- Проект живёт на пожертвования: пункт «Поддержать проект» в приложении (внизу левой панели, в «О программе» и в меню Help на macOS) ведёт на https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md.
- Как участвовать — `CONTRIBUTING.md`; об уязвимостях — `SECURITY.md`.

## Требования
- Rust stable (`rustup`), Xcode Command Line Tools (macOS)
- Node 22+, pnpm
- Docker — для стенда и интеграционных тестов
- Для сборки vendored OpenSSL — Perl и C-компилятор (macOS: Xcode Command Line Tools; Windows: Visual Studio Build Tools и Perl, например Strawberry Perl;
  Linux: пакеты Tauri из `.github/workflows/ci.yml`). cmake и NASM не нужны.
- `cargo-deny` 0.20.2 для проверки лицензий зависимостей: `cargo install cargo-deny --locked --version 0.20.2`

## Запуск
```bash
pnpm install
docker compose -f dev/docker-compose.yml up -d --build
scripts/wait-stand.sh
pnpm tauri dev
```

## Стенд (`dev/docker-compose.yml`)
| Что | Адрес |
|---|---|
| Кластер Patroni `demo` (PG 17, etcd) | PG `localhost:5441–5443`, REST API `localhost:8011–8013` |
| Primary и реплика без Patroni (PG 16) | `localhost:5451`, `localhost:5452` |
| Одиночный сервер без `pg_stat_statements` (PG 15) | `localhost:5460` |

Учётные данные: PG `glafira_diag` / `glafira-secret` (роль для диагностики), суперпользователь `postgres` / `postgres-secret`;
REST API Patroni `patroni` / `patroni-secret`. Демо-база `orders`, схема `shop`.
Участники Patroni отдают внутренние адреса `patroniN`: в профиле кластера задайте подмену адресов
(`patroni1 → localhost:5441`, API `http://localhost:8011` и т. д.).

## Тесты
```bash
pnpm test                                               # фронтенд
pnpm test:scripts && pnpm i18n:check                     # скрипты, словари и руководство
cargo test --manifest-path crates/pg-advisor/Cargo.toml  # правила
cargo test --manifest-path src-tauri/Cargo.toml --lib    # Rust unit
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses   # лицензии зависимостей
GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 \
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 \
  cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1   # интеграционные (нужен стенд)
```
Без переменной `GPG_IT_*` интеграционный тест пропускается. `GPG_IT_DOCKER=1` разрешает тестам останавливать контейнеры,
делать switchover и promote; после них стенд возвращается в исходное состояние.

## Релиз
`docs/release-checklist.md`: версии в трёх манифестах, раздел в `CHANGELOG.md`, `THIRD_PARTY.md`,
`node scripts/check-release.mjs vX.Y.Z`, тег `vX.Y.Z` → workflow `release` собирает установщики на трёх ОС,
подписывает macOS-сборки и публикует релиз с `latest.json` в `vvnovg/glafira-postgres-releases`.

## Где хранятся данные
- Профили: `connections.json` в каталоге данных (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres/`).
- Пароли — в системном хранилище секретов, сервис `glafira-postgres`.
- Настройки: `settings.json`.
- Логи: `RUST_LOG=debug pnpm tauri dev`.
```

- [ ] **Step 4: `docs/admin.md`**

Заменить содержимое целиком:

```markdown
# Glafira for PostgreSQL: руководство администратора

## Установка
| ОС | Файл | Тихая установка |
|---|---|---|
| macOS | `Glafira_for_PostgreSQL_<версия>_aarch64.pkg` / `_x64.pkg` | `sudo installer -pkg <файл>.pkg -target /` (Jamf, Intune, Kandji) |
| Windows | `Glafira_for_PostgreSQL_<версия>_x64_en-US.msi` | `msiexec /i <файл>.msi /qn` (SCCM, Intune) — установка на машину |
| Linux | `Glafira_for_PostgreSQL_<версия>_amd64.deb`, `Glafira_for_PostgreSQL-<версия>-1.x86_64.rpm`, `Glafira_for_PostgreSQL_<версия>_amd64.AppImage` | `sudo apt install ./<файл>.deb`, `sudo dnf install ./<файл>.rpm` |

Установщики — в разделе релизов `https://github.com/vvnovg/glafira-postgres-releases/releases`.
Пробелы в именах файлов заменены на `_` при публикации; точные имена смотрите в релизе.
Исполняемый файл приложения — `glafira-pg` (`glafira-pg.exe` на Windows, `/usr/bin/glafira-pg` на Linux).

### Обновление установленного приложения
Встроенное обновление («О программе» → «Проверить обновления») скачивает установщик того же типа, что установлен:

| Установлено из | Что делает обновление | Права |
|---|---|---|
| `.pkg` / `.dmg` (macOS) | заменяет `.app` на месте | если `.app` принадлежит root (установка из `.pkg` в `/Applications`) — запрос пароля администратора |
| `.msi` (Windows, на машину) | запускает новый MSI | UAC; у пользователя без прав администратора обновление не установится |
| `.deb` / `.rpm` (Linux) | ставит новый пакет через `dpkg -i` / `rpm -U` | пароль администратора (pkexec, иначе zenity/kdialog + sudo) |
| `.AppImage` | заменяет файл AppImage | запись в каталог с AppImage |

**Управляемые парки** (SCCM, Intune, Jamf, Kandji, репозиторий пакетов): обычные пользователи без прав
администратора не смогут установить обновление сами. Рекомендуется политика `updatesEnabled = false` и раздача
новых версий теми же средствами, что и установка (на Linux — через свой репозиторий apt/dnf или `apt install ./<файл>.deb`).

## Политика
Приложение читает политику при запуске. Пользователь не может её изменить.

| ОС | Где |
|---|---|
| macOS | профиль конфигурации MDM, домен `io.github.vvnovg.glafira.postgres` (файл попадает в `/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist`) — пример `admin/policy.mobileconfig` |
| Windows | `HKLM\SOFTWARE\Policies\Glafira\PostgreSQL` (GPO, Intune) — пример `admin/policy.reg` |
| Linux | `/etc/io.github.vvnovg.glafira.postgres/policy.json` — пример `admin/policy.json` |

| Ключ | Тип | По умолчанию | Смысл |
|---|---|---|---|
| `updatesEnabled` | bool (`dword` 0/1 в реестре) | `true` | `false` — приложение не обращается к серверу обновлений |
| `updateUrl` | строка, только `https://` | публичный `latest.json` | зеркало с одобренными версиями (формат — `latest.json` Tauri). Адрес `http://` считается ошибкой политики: обновления выключаются |
| `language` | строка: `en`, `ru` | язык ОС или выбор пользователя | язык интерфейса для всех; пользователь его не меняет. Неизвестный код — ошибка политики (показывается в «О программе»), язык не задаётся, обновления не выключаются |

Ключ `licenseKey` прежних сборок больше не используется: все функции бесплатны, ключ игнорируется, как любой
неизвестный, с предупреждением в логе.

Политика читается один раз при запуске: после изменения перезапустите приложение. Неизвестные ключи игнорируются.
На macOS читается только политика уровня устройства: профиль должен быть установлен на устройство (канал устройства MDM,
`PayloadScope` = `System`), тогда он окажется в `/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist`.
Профиль уровня пользователя (`/Library/Managed Preferences/<пользователь>/…`) приложение не читает.
Значения в реестре Windows — строки (`REG_SZ`), флаги — `dword` 0/1. Состояние политики и её ошибки
(нечитаемый файл, неверный тип значения, неизвестный язык) видны в окне «О программе».

### Проверка политики вручную (macOS)
`defaults write` в `/Library/Managed Preferences` на современной macOS не работает (туда пишет только MDM).
Для проверки соберите plist и скопируйте его от имени администратора:

```bash
rm -f /tmp/policy.plist && plutil -create xml1 /tmp/policy.plist
plutil -insert updatesEnabled -bool NO /tmp/policy.plist
sudo mkdir -p "/Library/Managed Preferences"
sudo cp /tmp/policy.plist "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"
sudo chmod 644 "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"
plutil -p "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"
```
Удалить: `sudo rm "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"`.

## Внутреннее зеркало обновлений
Скопируйте из релиза установщики, `*.sig` и `latest.json`, замените в `latest.json` адреса `url` на адреса зеркала
и укажите адрес `latest.json` в `updateUrl`. Подписи остаются прежними: приложение проверяет их встроенным ключом.
Зеркало должно отдаваться по `https://` (по `http` приложение обновления не загружает).

В `latest.json` по записи на каждый тип установки: `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64`,
`linux-x86_64-deb`, `linux-x86_64-rpm`, `linux-x86_64-appimage` и `linux-x86_64` (тоже AppImage — запасная запись).
Сохраните все записи: установка из `.deb` скачивает только `.deb`, из `.rpm` — только `.rpm`.

## Роль для диагностики
```sql
CREATE ROLE glafira_diag LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;   -- отмена и завершение сессий
GRANT pg_maintain TO glafira_diag;         -- PostgreSQL 17+: VACUUM и ANALYZE
-- в каждой БД, где нужен анализ запросов (и pg_stat_statements в shared_preload_libraries):
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
```
В `pg_hba.conf` нужна строка, разрешающая роли вход с машин пользователей (`scram-sha-256`).

Роль нужна для полной диагностики; без неё приложение подключается, но чек-лист готовности в проверке подключения
покажет, чего не хватает, и подскажет точную команду (`GRANT … TO "glafira_diag";`). В P1 приложение само ничего не изменяет:
`pg_signal_backend` и `pg_maintain` чек-лист проверяет заранее. `pg_maintain` есть только в PostgreSQL 17 и новее;
в более ранних версиях `VACUUM` и `ANALYZE` могут запускать владелец таблицы, владелец базы или суперпользователь.
`CREATE EXTENSION` выполняет суперпользователь; `shared_preload_libraries` меняется только с перезапуском сервера.

### Настройки сервера, которые проверяет чек-лист
| Параметр | Значение | Зачем |
|---|---|---|
| `shared_preload_libraries` | содержит `pg_stat_statements` | статистика запросов; на каждом узле |
| `compute_query_id` | `on` (PostgreSQL 14+; `auto` подходит, если `pg_stat_statements` загружен на всех узлах) | связь запросов в `pg_stat_activity` и `pg_stat_statements` |
| `track_io_timing` | `on` | время ввода-вывода запросов (предупреждение, если выключено) |
| `track_activity_query_size` | не меньше `4096` | длинные тексты запросов не обрезаются (предупреждение) |

Необязательные расширения, которые приложение использует, если они установлены в базе:
`hypopg`, `pgstattuple`, `pg_buffercache`, `pg_wait_sampling`, `pg_qualstats`, `pg_stat_kcache`
(`CREATE EXTENSION IF NOT EXISTS <имя>;`, если пакет расширения установлен на сервере).

### Patroni
Роль и расширения создаются на лидере и доезжают до реплик репликацией. Параметры PostgreSQL в кластере Patroni задаёт
Patroni (`patronictl edit-config`, затем `patronictl restart` для `shared_preload_libraries`), а не `postgresql.conf` на узле.
Если в конфигурации Patroni есть раздел `postgresql.pg_hba`, Patroni сам пишет `pg_hba.conf` — строку для роли добавляйте туда.

## Сеть
| Куда | Порт | Зачем |
|---|---|---|
| Каждый узел PostgreSQL | 5432 (или ваш) | каталог, топология списка узлов, проверка подключения |
| REST API Patroni каждого узла | 8008 (или ваш) | обнаружение участников (`GET /cluster`, `GET /patroni`) |
| Сервер обновлений | 443 | `github.com` и хосты, куда GitHub перенаправляет загрузку ассетов релизов (`release-assets.githubusercontent.com`, `objects.githubusercontent.com`), — или хост из `updateUrl`; только если обновления не выключены политикой |

Приложение вызывает только `GET /cluster` и `GET /patroni`; basic auth передаётся, если задан в профиле. В Patroni
(проверено на 4.1.5) `restapi.allowlist`, обязательная basic auth для небезопасных вызовов и `restapi.verify_client: optional`
действуют только на изменяющие запросы (PATCH, PUT, POST, DELETE), а чтение они не закрывают, поэтому для Glafira
`restapi.allowlist` менять не нужно. Не добавляйте в него адреса и подсети пользователей: он защищает switchover, restart и reinitialize.
Чтение закрывает только `restapi.verify_client: required` (TLS-рукопожатие требует клиентский сертификат): тогда выдайте
пользователям клиентский сертификат и укажите его в профиле кластера в разделе «TLS для API» — профиль поддерживает HTTPS,
свой CA и клиентский сертификат. Если Patroni отдаёт адреса, недоступные с машин пользователей (pod IP, NAT), в профиле
кластера задаётся подмена адресов участников.
SSH-туннели через бастион — в следующей версии.

Приложение не отправляет телеметрию и отчёты о сбоях и не загружает удалённое содержимое: сетевые соединения — только с серверами PostgreSQL
и REST API Patroni из профилей подключений и с сервером обновлений.

### Как приложение выглядит на сервере
- Соединения идут с `application_name = glafira-pg`: по нему их видно в `pg_stat_activity`.
- Сессии открываются только на чтение: `default_transaction_read_only = on`, `statement_timeout = 5s`, `lock_timeout = 1s`.
- Не больше 3 соединений на каждую связку «подключение, узел, база»; простаивающее закрывается через 5 минут. Учитывайте их в `max_connections` и в лимитах роли.
- Проверка подключения («Проверить») не использует этот пул: она открывает свои короткоживущие соединения к узлам (на каждом узле
  несколько за время проверки) и закрывает их по окончании. Запас в `max_connections` и в лимитах роли нужен и для них.
- Топология опрашивается раз в 10 секунд.

## Данные и логи
| | macOS | Windows | Linux |
|---|---|---|---|
| Данные | `~/Library/Application Support/io.github.vvnovg.glafira.postgres` | `%APPDATA%\io.github.vvnovg.glafira.postgres` | `~/.local/share/io.github.vvnovg.glafira.postgres` |
| Логи | `~/Library/Logs/io.github.vvnovg.glafira.postgres` | `%LOCALAPPDATA%\io.github.vvnovg.glafira.postgres\logs` | `~/.local/share/io.github.vvnovg.glafira.postgres/logs` |
| Пароли | Keychain, сервис `glafira-postgres` | Credential Manager | Secret Service |

В каталоге данных лежат `connections.json` (профили без паролей) и `settings.json`.
Если пароль в хранилище не сохранён, используется `~/.pgpass` (Windows: `%APPDATA%\postgresql\pgpass.conf`) или файл из `PGPASSFILE`.
На Unix у такого файла должны быть права 0600 (`chmod 0600 ~/.pgpass`): как в libpq, файл, доступный группе или всем
(`mode & 0o077`), игнорируется, и в журнале остаётся только предупреждение.
Окно «О программе» показывает эти каталоги и собирает диагностический архив (версия, логи, профили без паролей);
приложение ничего не отправляет — архив пользователь прикладывает к обращению сам.
```

- [ ] **Step 5: Проверки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
cmp LICENSE /Users/vvnovg/kafka-explorer/LICENSE && echo "LICENSE = канон"
grep -n "KAFKA_EXPLORER_TOKEN\|kafka-explorer\|license.json\|licenseKey\|Free\|\bPro\b" README.md docs/admin.md | grep -v "Ключ \`licenseKey\` прежних сборок" || echo "README и admin.md чисты"
cargo test --manifest-path src-tauri/Cargo.toml --lib policy::sources
pnpm i18n:check
```

Ожидается: `LICENSE = канон`; grep показывает только строку-примечание про `licenseKey` в `docs/admin.md`; тесты политики зелёные; `i18n:check` проходит.

- [ ] **Step 6: Commit**

```bash
git add LICENSE NOTICE TRADEMARK.md CONTRIBUTING.md SECURITY.md README.md docs/admin.md
git commit -q -F - <<'MSG'
docs: LICENSE, NOTICE, TRADEMARK, CONTRIBUTING (DCO), SECURITY; README и руководство администратора под Apache 2.0 и updater

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
```

---

### Task 12: Смоук «Открытый код» и финальная проверка ветки

**Files:**
- Create: `docs/smoke/apache-2.md`

**Interfaces:**
- Consumes: всё из задач 1–11.
- Produces: чек-лист GUI-смоука для владельца; зелёная ветка `feature/apache-2`.

- [ ] **Step 1: `docs/smoke/apache-2.md`**

```markdown
# Смоук «Открытый код» (Apache 2.0, без ключей, «Поддержать проект», автообновление)

Сборка: `pnpm tauri dev`. Стенд: `docker compose -f dev/docker-compose.yml up -d` и `scripts/wait-stand.sh`
(сервер `localhost:5460`, роль `glafira_diag` / `glafira-secret`).
Каталог данных — строка «Данные:» в «О программе» (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres`),
лог — последний файл в каталоге из строки «Логи:» (`~/Library/Logs/io.github.vvnovg.glafira.postgres`).
До публикации репозиториев адреса в пп. 4–5 отвечают 404: важно, что браузер открыл именно этот адрес.

1. **Старый ключ.** Закрыть приложение, положить в каталог данных файл `license.json`:
   `echo '{"token":"a.b"}' > "$HOME/Library/Application Support/io.github.vvnovg.glafira.postgres/license.json"`,
   запустить `pnpm tauri dev`: файла нет; в логе запись уровня INFO `legacy license file removed` и ни одной записи WARN про него.
2. **prod без ключа.** «+» над деревом (новое подключение) → поле «Окружение»: вариант `prod` доступен и подписан просто `prod`
   (без «Pro» и замка); после выбора флажок «Только чтение» установлен, его можно снять и поставить снова. Заполнить: хост `localhost`,
   порт `5460`, пользователь `glafira_diag`, пароль `glafira-secret`; сохранить с установленным флажком. У сохранённого подключения
   «Изменить подключение» → снять флажок → «Сохранить»: сохраняется без ошибки, значок подключения без пометки «только чтение».
   В меню подключения «Дублировать подключение»: в копии вариант `prod` доступен, флажок «Только чтение» как у источника.
3. **Импорт prod.** У prod-подключения без «Только чтение»: кнопка «Ещё действия» над деревом → «Экспортировать всё…» → сохранить файл;
   удалить подключение (форма → «Удалить»); «Ещё действия» → «Импортировать…» → выбрать файл: в окне «Импорт подключений» нет
   предупреждения про «только для чтения» → «Импортировать». У импортированного подключения открыть форму: флажок «Только чтение» снят.
4. **Три входа «Поддержать проект».** Внизу левой панели ссылки в порядке: Настройки, Поддержать проект, Справка, О программе.
   В «О программе» под строками сведений — кнопка «Поддержать проект» и подсказка «Glafira бесплатна, её исходный код открыт;
   пожертвования помогают её развивать.». На macOS в меню Help пункты: Руководство Glafira, разделитель, Поддержать проект, О программе,
   Собрать диагностику…. Каждый из трёх входов открывает в браузере https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md,
   новая вкладка в приложении не появляется.
5. **«О программе».** Строки: Версия `0.1.0`, Сборка (хеш коммита), Система, Лицензия — кнопка `Apache 2.0` открывает в браузере
   https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE, Политика организации; строки «Тариф» нет; внизу панели нет ссылки «Лицензия».
6. **Обновления в dev-сборке.** При запуске уведомления над вкладками нет. «О программе» → раздел «Обновления» → «Проверить обновления»:
   текст «Обновления не настроены в этой сборке», кнопки «Установить и перезапустить» нет; в логе нет записей с `github.com`.
7. **Политика** (macOS):
   ```bash
   rm -f /tmp/policy.plist && plutil -create xml1 /tmp/policy.plist
   plutil -insert updatesEnabled -bool NO /tmp/policy.plist
   plutil -insert licenseKey -string abc /tmp/policy.plist
   sudo mkdir -p "/Library/Managed Preferences"
   sudo cp /tmp/policy.plist "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"
   sudo chmod 644 "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"
   ```
   Перезапустить приложение. «О программе»: «Политика организации: задана», строк «Политика организации: …» с ошибками нет;
   «Проверить обновления» → «Обновления управляются вашей организацией»; в логе `unknown policy key “licenseKey” ignored`.
   Снять политику: `sudo rm "/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist"` и перезапустить.
8. **F1 и руководство.** На вкладке «О программе» нажать F1: открывается глава «Обновления и диагностика» с разделами
   «Обновления», «О программе», «Архив диагностики», «Поддержать проект»; в содержании справки две главы — «Начало работы»
   и «Обновления и диагностика»; в тексте нет слов «Pro», «Free» и экрана «Лицензия».
9. **Языки.** «Настройки» → «Язык интерфейса» → `English`: внизу панели «Support the project», в «О программе» подсказка
   «Glafira is free and open source; donations keep it going.», раздел «Updates» с кнопкой «Check for updates», строка «License»;
   на macOS в меню Help пункт «Support the project». Вернуть `Русский`.
10. **Архив диагностики.** «О программе» → «Собрать диагностику» → сохранить `diagnostics-<дата>.zip`; распаковать:
    в `info.json` есть объекты `app` и `policy`, слова `license` нет; в `connections.json` и в `logs/` нет пароля `glafira-secret`.
```

- [ ] **Step 2: Полная проверка ветки**

```bash
cd /Users/vvnovg/postgres-patroni-diag && source ~/.cargo/env
pnpm test
pnpm test:scripts
pnpm i18n:check
pnpm build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --lib
cargo test --manifest-path src-tauri/Cargo.toml --tests --no-run
cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check
cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/pg-advisor/Cargo.toml
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
node scripts/third-party.mjs && git diff --quiet THIRD_PARTY.md && echo "THIRD_PARTY.md актуален и детерминирован"
node scripts/check-release.mjs --dry-run v0.1.0
```

Ожидается: всё зелёное; `THIRD_PARTY.md` без изменений после повторной генерации; `check-release` — только предупреждение о заглушке ключа.

- [ ] **Step 3: Гарды — ничего лишнего не осталось**

```bash
cd /Users/vvnovg/postgres-patroni-diag
echo "--- license-format / ProRequired / ProLock / KAFKA_EXPLORER_TOKEN ---"
grep -rn "license-format\|ProRequired\|PRO_REQUIRED\|ProLock\|proHint\|useLicense\|KAFKA_EXPLORER_TOKEN\|license-signing\|GPG_ALLOW_DEV_LICENSE_KEY" \
  src src-tauri/src src-tauri/Cargo.toml src-tauri/build.rs scripts .github README.md docs/admin.md docs/guide docs/i18n CONTRIBUTING.md SECURITY.md package.json || echo "чисто"
echo "--- Pro / Free как тариф ---"
grep -rnw "Pro\|Free" src/locales docs/guide docs/i18n README.md docs/admin.md CHANGELOG.md || echo "чисто"
echo "--- license вне допустимых мест ---"
grep -rn -i "license" src src-tauri/src --include=*.ts --include=*.tsx --include=*.rs --include=*.json \
  | grep -v "src/lib/links.ts\|about.license\|LICENSE_URL\|openLicense\|migrate.rs\|legacy_license\|license.json\|licenseKey\|\"license\": \"License\"\|\"license\": \"Лицензия\"\|legacy license" || echo "чисто"
echo "--- файлы, которых не должно быть ---"
ls src-tauri/license-public.key src-tauri/license-dev-public.key .cargo/config.toml src-tauri/src/license src-tauri/src/commands/license.rs src/components/license src/components/common/ProLock.tsx src/lib/useLicense.ts 2>&1 | grep -v "No such file" || echo "удалены"
echo "--- внутренний обзор не в индексе ---"
git ls-files | grep -c "licensing-review" ; git check-ignore docs/licensing-review-2026-10-19.md
```

Ожидается: все четыре блока — «чисто»/«удалены»; обзор лицензирования не в индексе и игнорируется.

- [ ] **Step 4: Commit**

```bash
git add docs/smoke/apache-2.md
git commit -q -F - <<'MSG'
docs(smoke): чек-лист GUI-смоука этапа «Открытый код»

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
MSG
git log --oneline main..feature/apache-2
```

---

## После плана (контроллер, не субагенты)

1. Финальное ревью ветки (`git diff main...feature/apache-2`) свежим субагентом-ревьюером по спеке; волна исправлений при необходимости.
2. GUI-смоук владельца по `docs/smoke/apache-2.md`; слияние в `main` — по его решению (fast-forward).
3. Шаги публикации по `docs/release-checklist.md` («Один раз»): ключ updater (владелец), репозитории GitHub, секреты
   (владелец), публичность, пробный релиз `gh workflow run release -f dry_run=true`, тег `v0.1.0`, проверка релиза и `latest.json`.
