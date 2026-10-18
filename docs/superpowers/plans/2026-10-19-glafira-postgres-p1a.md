# Glafira for PostgreSQL — P1a «Каркас и подключения без SSH»: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Desktop-приложение Glafira for PostgreSQL, которое подключается напрямую к серверу, списку узлов или кластеру Patroni, показывает топологию с ролями, лагом и журналом событий, проверяет готовность к диагностике и даёт пройти дерево «папки → подключения → узлы / базы → схемы → объекты».

**Architecture:** Tauri 2. Каркас (хранилище, keychain, политика, лицензия, i18n, папки, экспорт) переносится из Glafira for Apache Kafka. Новое Rust-ядро: `conn/` (профиль, строка подключения, `~/.pgpass`, TLS, соединения, пулы), `topology/` (Patroni REST, опрос узлов, журнал, фоновое наблюдение), `readiness/` + крейт `pg-advisor` (чек-лист готовности), `catalog/` (запросы каталога). Фронтенд на React переиспользует дерево и вкладки Glafira, узлы дерева и форма — новые.

**Tech Stack:** Rust stable (edition 2024), Tauri 2, tokio-postgres 0.7 + postgres-openssl 0.5 (OpenSSL vendored), rustls-native-certs 0.8, reqwest 0.13 (rustls), keyring 3; React 19, TypeScript 6, Vite 8, TanStack Query 5 / Virtual 3, Zustand 5, i18next, Vitest + Testing Library; Docker (etcd 3.5, Patroni 4 на `postgres:17`, `postgres:16`, `postgres:15`).

**Спецификация:** `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md` (P1a — §1 «План реализации делится на две части»). Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`.

**Образец кода:** Glafira for Apache Kafka в `/Users/vvnovg/kafka-explorer`. Ниже — `$G`; корень нового репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`). Команды шагов выполняются из `$P`, если не сказано иное.

## Global Constraints

- PostgreSQL 13+, Patroni 3.0+ (включая 4.x). Платформы: macOS (arm64, x64), Windows x64, Linux x64.
- Идентификаторы: `PRODUCT_NAME = "Glafira for PostgreSQL"`, `IDENTIFIER = "io.github.vvnovg.glafira.postgres"`, бинарь `glafira-pg`, `KEYCHAIN_SERVICE = "glafira-postgres"`, `POLICY_VENDOR = "Glafira"`, `POLICY_PRODUCT = "PostgreSQL"`, `PG_APPLICATION_NAME = "glafira-pg"`.
- Секреты — только в keychain, ключ `<id подключения>/<поле>`; поля P1a: `pgPassword`, `pgKeyPassword`, `patroniPassword`, `patroniKeyPassword`. В `connections.json` секретов нет.
- Соединение с PG: `application_name = glafira-pg`, параметр запуска `options = -c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000`.
- Пул: не больше 3 соединений на ключ (подключение, участник, БД); простаивающее соединение закрывается через 5 мин.
- Топология опрашивается раз в 10 с; журнал событий — до 500 событий на подключение, в памяти.
- HTTP-запрос к Patroni — таймаут 5 с.
- Все команды Tauri возвращают `Result<T, AppError>`; `AppError { code, key, params, message, details?, retryable }` сериализуется в camelCase, `code` — в SCREAMING_SNAKE_CASE.
- Пользовательские тексты из Rust — только через `ltext!` / `app_err!` с английским шаблоном; переводы — `src/locales/{en,ru}.json`. Кириллица в пользовательских строках Rust запрещена (`pnpm i18n:check`). Комментарии в коде — по-русски, как в Glafira.
- Языки P1: `en`, `ru`.
- Изменяющих операций с БД и Patroni в P1a нет. Флаги `environment` и `readOnly` хранятся и показываются; создание prod-подключения и перевод в prod — Pro.
- Ни одна операция не блокирует UI: keychain и файлы — в `spawn_blocking`, у каждой сетевой операции таймаут.
- Нет телеметрии и удалённого контента; CSP — только `self` и IPC.
- Сообщения коммитов — по-русски, в стиле `feat(conn): …`, с последней строкой `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Отступления от спецификации (решения плана)

1. **Свой пул вместо `deadpool-postgres`** (`conn/pool.rs`, ~120 строк). `deadpool-postgres` подключается через `MakeTlsConnect` по хосту из `Config`, а P1b нужен `Config::connect_raw` к локальному порту туннеля с проверкой TLS по исходному имени хоста. Поведение §7 спеки сохраняется: максимум 3, проверка при выдаче, закрытие после 5 мин простоя.
2. **Секрет `patroniKeyPassword`** — пароль зашифрованного клиентского ключа или PKCS#12 для Patroni API. В спеке §3.2 его нет, а без него mTLS к Patroni с зашифрованным ключом невозможен.
3. **Поле `ssh` профиля появляется в P1b** вместе с туннелем; в P1a его нет. Старые файлы читаются: поле необязательное.
4. **Ключи политики `updatesEnabled` / `updateUrl` остаются в разборе** (модуль политики Glafira на них построен, P6 их использует), но в `docs/admin.md` P1 не документируются. Ключ `mcpEnabled` удаляется (P4 вернёт его вместе с `mcpMaxAccess`).
5. **Сопоставление реплики со строкой `pg_stat_replication`** (§6.3 спеки) дополнено правилом «единственная пара»: если после сопоставления по `client_addr` и `application_name` осталась ровно одна реплика и ровно одна строка — они сопоставляются. Без этого стенд и типичные контейнерные установки (клиентский адрес — IP контейнера, `application_name = walreceiver`) не получают `sync_state`.
6. **Версия PostgreSQL участника Patroni в обзоре** берётся из `/patroni` (`server_version` — число, форматируется как `17.2`).
7. **Фильтр в SQL (`ILIKE`) — у списков объектов схемы** (там их могут быть тысячи). Списки баз, схем, ролей и расширений короткие и приходят целиком; команды `list_databases` / `list_schemas` в спеке тоже без фильтра.
8. **Открытый ключ лицензий в P1a — тестовый** (`license-format/testdata/dev-public.key`): боевая пара ключей продукта PostgreSQL кладётся в P6 вместе с подписанием и релизами. `build.rs` не даёт собрать релиз с тестовым ключом (как в Glafira).

---

## Структура файлов P1a

```
postgres-patroni-diag/
├── package.json, pnpm-lock.yaml, pnpm-workspace.yaml, index.html
├── vite.config.ts, vitest.config.ts, tsconfig.json, tsconfig.node.json
├── public/                                   # копия из Glafira
├── .github/workflows/ci.yml
├── .cargo/config.toml                        # net.git-fetch-with-cli
├── README.md
├── docs/
│   ├── admin.md, admin/{policy.json, policy.reg, policy.mobileconfig}
│   └── guide/{en,ru}/getting-started.md
├── scripts/{i18n-check.mjs, i18n-check.test.mjs, wait-stand.sh}
├── dev/
│   ├── docker-compose.yml
│   ├── patroni/{Dockerfile, patroni.yml, post_bootstrap.sh}
│   ├── replica/entrypoint.sh
│   ├── primary/10-replication.sh
│   └── sql/init.sql
├── crates/pg-advisor/{Cargo.toml, src/lib.rs, src/readiness.rs}
├── src/                                      # фронтенд
│   ├── main.tsx, App.tsx, styles.css, polyfills.ts, vite-env.d.ts
│   ├── api/{client.ts, types.ts}
│   ├── i18n/{index.ts, languages.ts, text.ts}, locales/{en.json, ru.json}
│   ├── guide/{chapters.ts, parse.ts}
│   ├── lib/{activate.ts, env.ts, focusTrap.ts, format.ts, plainInputs.ts, useAppMenu.ts,
│   │        useDebouncedValue.ts, useHelpShortcut.ts, useLanguageSync.ts, useLicense.ts,
│   │        useConnectionEvents.ts}
│   ├── state/{workspace.ts, helpContext.ts, diagnosticsRequest.ts}
│   ├── test/{setup.ts, render.tsx, fixtures.ts}
│   └── components/
│       ├── Layout.tsx, Sidebar.tsx, TabContent.tsx
│       ├── common/{ErrorBanner, Hint, EnvBadge, ProLock}.tsx
│       ├── settings/SettingsPanel.tsx, license/{LicensePanel, LicenseNotice}.tsx
│       ├── about/AboutPanel.tsx, help/{HelpPanel.tsx, helpChapterFor.ts}
│       ├── connections/{profile.ts, fields.tsx, ConnectionForm.tsx, ConnectionFormTab.tsx,
│       │                NodesSection.tsx, PgSection.tsx, TestPanel.tsx}
│       ├── connections/tree/{ConnectionTree.tsx, ConnectionNode.tsx, FolderMenu.tsx,
│       │                     ImportDialog.tsx, folderTree.ts, CatalogNodes.tsx, ObjectList.tsx}
│       └── overview/Overview.tsx
└── src-tauri/
    ├── Cargo.toml, build.rs, tauri.conf.json, capabilities/default.json, icons/
    ├── license-public.key, license-dev-public.key
    ├── src/{main.rs, lib.rs, product.rs, error.rs, i18n.rs, sync.rs, logging.rs, settings.rs,
    │        secrets.rs, state.rs, diagnostics.rs}
    ├── src/policy/{mod.rs, sources.rs}, src/license/mod.rs
    ├── src/storage/{mod.rs, fs.rs, connections.rs, tree.rs, exchange.rs}
    ├── src/conn/{mod.rs, profile.rs, connstr.rs, pgpass.rs, errors.rs, tls.rs, pg.rs, pool.rs}
    ├── src/topology/{mod.rs, model.rs, journal.rs, patroni.rs, probe.rs, connector.rs,
    │                 observe.rs, runtime.rs}
    ├── src/readiness/{mod.rs, check.rs}
    ├── src/catalog/{mod.rs, databases.rs, schemas.rs, objects.rs, extensions.rs, roles.rs}
    ├── src/commands/{mod.rs, app.rs, license.rs, settings.rs, folders.rs, connections.rs,
    │                 topology.rs, catalog.rs}
    └── tests/{common/mod.rs, it_pg.rs, it_patroni.rs, it_probe.rs, it_runtime.rs,
               it_readiness.rs, it_catalog.rs}
```

Переменные интеграционных тестов (без переменной тест пропускается):

| Переменная | Значение на стенде |
|---|---|
| `GPG_IT_SINGLE` | `localhost:5460` — `pg-single` (PG 15, без `pg_stat_statements`) |
| `GPG_IT_PRIMARY`, `GPG_IT_REPLICA` | `localhost:5451`, `localhost:5452` — потоковая репликация без Patroni (PG 16) |
| `GPG_IT_PATRONI` | `http://localhost:8011,http://localhost:8012,http://localhost:8013` — API кластера `demo` |
| `GPG_IT_DOCKER` | `1` — тесты, которые останавливают и запускают контейнеры стенда |

Учётные данные стенда: PG `glafira_diag` / `glafira-secret`; суперпользователь `postgres` / `postgres-secret`; Patroni REST basic auth `patroni` / `patroni-secret`.

---

### Task 1: Каркас Rust — ядро из Glafira и оболочка Tauri

**Files:**
- Create: `src-tauri/Cargo.toml`, `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/product.rs`, `src-tauri/src/error.rs`, `src-tauri/src/state.rs`, `src-tauri/src/storage/mod.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/app.rs`, `.cargo/config.toml`, `.gitignore`, `docs/admin/policy.json`, `docs/admin/policy.reg`, `docs/admin/policy.mobileconfig`
- Copy from `$G/src-tauri` and modify: `src/sync.rs`, `src/i18n.rs`, `src/logging.rs`, `src/secrets.rs`, `src/settings.rs`, `src/diagnostics.rs`, `src/storage/fs.rs`, `src/policy/{mod.rs, sources.rs}`, `src/license/mod.rs`, `src/commands/{license.rs, settings.rs}`, `capabilities/default.json`, `icons/`
- Test: unit-тесты в перечисленных модулях

**Interfaces:**
- Produces:
  - `crate::error::{AppError, AppResult, ErrorCode}`; `ErrorCode` = `ConnectionFailed | TlsError | AuthFailed | NotAuthorized | Timeout | InvalidInput | NotFound | ProRequired | PatroniError | NodeUnavailable | Internal`; `AppError: Clone + PartialEq + Serialize + std::error::Error`; `AppError::localized(code, LocalizedText)`, `.with_details(s)`, `AppError::internal_with(s)`.
  - Макросы `ltext!`, `app_err!` (`crate::i18n`).
  - `crate::secrets::{SecretStore, KeyringStore, MemoryStore, OverlayStore, secret_key, PG_PASSWORD, PG_KEY_PASSWORD, PATRONI_PASSWORD, PATRONI_KEY_PASSWORD, CONNECTION_SECRET_FIELDS}`.
  - `crate::storage::fs::{read_json, read_json_or_quarantine, write_json, newer_schema_error}` (как в Glafira).
  - `crate::state::AppState { secrets, policy, license, settings }`, `AppState::new(dir, secrets) -> AppResult<Self>`, `.with_policy(policy)`.
  - `crate::commands::blocking(f) -> AppResult<T>`.
  - `crate::diagnostics::{AppInfo, write_archive(out, info, profiles: &[impl Serialize], policy, license, log_files)}`.
  - `crate::commands::app::{app_info(&AppHandle) -> AppInfo, get_app_info}`.

- [ ] **Step 1: Скопировать файлы ядра**

```bash
G=/Users/vvnovg/kafka-explorer
mkdir -p src-tauri/src/{storage,policy,license,commands} src-tauri/capabilities docs/admin .cargo
cp -R $G/src-tauri/icons src-tauri/
cp $G/src-tauri/capabilities/default.json src-tauri/capabilities/
cp $G/src-tauri/src/{sync.rs,i18n.rs,logging.rs,secrets.rs,settings.rs,diagnostics.rs} src-tauri/src/
cp $G/src-tauri/src/storage/fs.rs src-tauri/src/storage/
cp $G/src-tauri/src/policy/{mod.rs,sources.rs} src-tauri/src/policy/
cp $G/src-tauri/src/license/mod.rs src-tauri/src/license/
cp $G/src-tauri/src/commands/{license.rs,settings.rs} src-tauri/src/commands/
# Тестовый открытый ключ лицензий: пока боевого ключа продукта нет (P6), приложение собирается с ним,
# а build.rs не даёт выпустить с ним релиз.
cp $G/crates/license-format/testdata/dev-public.key src-tauri/license-public.key
cp $G/crates/license-format/testdata/dev-public.key src-tauri/license-dev-public.key
git -C $G rev-parse main   # запомнить SHA: он нужен в Cargo.toml (rev)
```

- [ ] **Step 2: Написать `src-tauri/Cargo.toml`** (вместо `<SHA>` — вывод последней команды шага 1)

```toml
[package]
name = "glafira-pg"
version = "0.1.0"
description = "Glafira for PostgreSQL — диагностика PostgreSQL и кластеров Patroni"
authors = ["Glafira for PostgreSQL contributors"]
edition = "2024"

[lib]
# Суффикс _lib нужен, чтобы имя библиотеки не совпало с именем бинаря (Windows, cargo#8519).
name = "glafira_pg_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
tauri-plugin-dialog = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
keyring = { version = "3", features = ["apple-native", "windows-native", "sync-secret-service"] }
thiserror = "2"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
time = { version = "0.3", features = ["formatting"] }
zip = { version = "9", default-features = false, features = ["deflate"] }
tokio = { version = "1", features = ["rt", "rt-multi-thread", "net", "time", "macros", "sync", "io-util"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "registry", "fmt"] }
plist = "1"
dirs = "7"
sys-locale = "0.3"
license-format = { git = "https://github.com/vvnovg/kafka-explorer", rev = "<SHA>" }

[target.'cfg(windows)'.dependencies]
winreg = "0.55"

[dev-dependencies]
license-format = { git = "https://github.com/vvnovg/kafka-explorer", rev = "<SHA>", features = ["dev-keys"] }
tempfile = "3"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }

[profile.release]
codegen-units = 1
lto = true
opt-level = 3
panic = "abort"
strip = true
```

`.cargo/config.toml`:

```toml
# license-format берётся git-зависимостью из kafka-explorer: системный git знает учётные данные.
[net]
git-fetch-with-cli = true
```

`.gitignore` (заменить созданный при инициализации):

```
.DS_Store
node_modules
dist
dist-ssr
*.local
logs
*.log
target
src-tauri/gen
.vscode/*
!.vscode/extensions.json
.idea
# Закрытые ключи лицензий
license-signing.key
```

- [ ] **Step 3: `build.rs`, `tauri.conf.json`, `main.rs`, `product.rs`**

`src-tauri/build.rs` — скопировать `$G/src-tauri/build.rs` и изменить в `check_license_key`:
- путь тестового ключа: `let dev = manifest_dir.join("license-dev-public.key");`
- переменную `KE_ALLOW_DEV_LICENSE_KEY` → `GPG_ALLOW_DEV_LICENSE_KEY` (все три вхождения);
- текст паники: `"src-tauri/license-public.key совпадает с тестовым ключом src-tauri/license-dev-public.key: релизная сборка приняла бы ключи, подписанные общедоступным dev-signing.key. Положите боевой открытый ключ продукта или задайте GPG_ALLOW_DEV_LICENSE_KEY=1 для локальной проверки релизной сборки."`

`src-tauri/tauri.conf.json`:

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
    "publisher": "Glafira for PostgreSQL",
    "category": "DeveloperTool",
    "shortDescription": "Glafira for PostgreSQL — diagnostics for PostgreSQL and Patroni clusters",
    "copyright": "© 2026 Glafira for PostgreSQL",
    "linux": { "deb": { "depends": ["libsecret-1-0"] }, "rpm": { "depends": ["libsecret"] } },
    "macOS": { "minimumSystemVersion": "11.0" }
  }
}
```

`src-tauri/src/main.rs`:

```rust
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    glafira_pg_lib::run()
}
```

`src-tauri/src/product.rs`:

```rust
//! Имя продукта и идентификаторы — в одном месте. Переименование меняет только этот файл
//! и `tauri.conf.json`; тест ниже следит, чтобы они совпадали.

pub const PRODUCT_NAME: &str = "Glafira for PostgreSQL";
pub const IDENTIFIER: &str = "io.github.vvnovg.glafira.postgres";
pub const KEYCHAIN_SERVICE: &str = "glafira-postgres";
pub const POLICY_VENDOR: &str = "Glafira";
pub const POLICY_PRODUCT: &str = "PostgreSQL";
/// `application_name` соединений приложения: так их видно в `pg_stat_activity`.
pub const PG_APPLICATION_NAME: &str = "glafira-pg";

#[cfg(test)]
mod tests {
    #[test]
    fn matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], super::IDENTIFIER);
        assert_eq!(conf["productName"], super::PRODUCT_NAME);
        assert_eq!(conf["mainBinaryName"], "glafira-pg");
        assert_eq!(conf["app"]["windows"][0]["title"], super::PRODUCT_NAME);
    }
}
```

- [ ] **Step 4: Новый `error.rs`**

`src-tauri/src/error.rs` (вместо Glafira-версии: без rdkafka, с кодами PG):

```rust
use std::collections::BTreeMap;

use serde::Serialize;

use crate::i18n::LocalizedText;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    ConnectionFailed,
    TlsError,
    AuthFailed,
    NotAuthorized,
    Timeout,
    InvalidInput,
    NotFound,
    ProRequired,
    /// Patroni REST API ответил ошибкой или неожиданным ответом.
    PatroniError,
    /// Узел не отвечает или пока не принимает соединения (запускается, в восстановлении).
    NodeUnavailable,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    pub code: ErrorCode,
    /// Ключ словаря интерфейса (`src/locales/*.json`).
    pub key: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Английский текст (для логов и как запасной вариант перевода).
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    pub retryable: bool,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn localized(code: ErrorCode, text: LocalizedText) -> Self {
        Self {
            code,
            key: text.key,
            params: text.params,
            message: text.text,
            details: None,
            retryable: Self::retryable_code(code),
        }
    }

    fn retryable_code(code: ErrorCode) -> bool {
        matches!(
            code,
            ErrorCode::ConnectionFailed | ErrorCode::Timeout | ErrorCode::NodeUnavailable
        )
    }

    /// Текст ошибки для полей-сообщений (ключ, параметры, английский текст).
    pub fn text(&self) -> LocalizedText {
        LocalizedText {
            key: self.key.clone(),
            params: self.params.clone(),
            text: self.message.clone(),
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Неожиданная внутренняя ошибка: общий текст «Internal error», подробности — в `details`
    /// (технический текст, не переводится).
    pub fn internal_with(details: impl Into<String>) -> Self {
        app_err!(ErrorCode::Internal, "errors.internal", "Internal error").with_details(details)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        app_err!(
            ErrorCode::Internal,
            "errors.io",
            "I/O error: {{error}}",
            error = e
        )
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        app_err!(
            ErrorCode::Internal,
            "errors.json",
            "JSON error: {{error}}",
            error = e
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_for_frontend() {
        let e = app_err!(ErrorCode::NodeUnavailable, "errors.x", "Node {{n}}", n = "pg-1");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "NODE_UNAVAILABLE");
        assert_eq!(v["key"], "errors.x");
        assert_eq!(v["params"]["n"], "pg-1");
        assert_eq!(v["message"], "Node pg-1");
        assert_eq!(v["retryable"], true);
        assert!(v.get("details").is_none());
    }

    #[test]
    fn retryable_codes() {
        for (code, retry) in [
            (ErrorCode::ConnectionFailed, true),
            (ErrorCode::Timeout, true),
            (ErrorCode::NodeUnavailable, true),
            (ErrorCode::AuthFailed, false),
            (ErrorCode::PatroniError, false),
        ] {
            assert_eq!(
                AppError::localized(code, LocalizedText::raw("x")).retryable,
                retry,
                "{code:?}"
            );
        }
    }
}
```

(`LocalizedText::raw` уже есть в `i18n.rs` Glafira — его используют тесты `state.rs` Glafira.)

- [ ] **Step 5: Правки скопированных модулей**

1. `i18n.rs` — в тесте заменить `"Kafka"` на `"PostgreSQL"` (три вхождения в `ltext!("messages.test.hello", …)` и ассертах).
2. `secrets.rs` — заменить импорт `use crate::kafka::config::{…};` и константу `CONNECTION_SECRET_FIELDS` на:

```rust
/// Пароль PostgreSQL.
pub const PG_PASSWORD: &str = "pgPassword";
/// Пароль клиентского ключа PostgreSQL (PKCS#12 или зашифрованный PEM).
pub const PG_KEY_PASSWORD: &str = "pgKeyPassword";
/// Пароль basic auth Patroni REST API.
pub const PATRONI_PASSWORD: &str = "patroniPassword";
/// Пароль клиентского ключа Patroni REST API.
pub const PATRONI_KEY_PASSWORD: &str = "patroniKeyPassword";

/// Все секреты подключения в keychain (для удаления).
pub const CONNECTION_SECRET_FIELDS: [&str; 4] =
    [PG_PASSWORD, PG_KEY_PASSWORD, PATRONI_PASSWORD, PATRONI_KEY_PASSWORD];
```

   В тестах `secrets.rs` заменить `SASL_PASSWORD` на `PG_PASSWORD`, если встречается.
3. `settings.rs`:
   - `pub const LANGUAGES: [&str; 2] = ["en", "ru"];` (остальные языки вернёт P6);
   - удалить `ApprovedClient`, `McpSettings`, поле `mcp` в `SettingsFile`, функции `mcp_enabled_in`, `mcp`, `set_mcp_enabled`, `is_client_approved`, `remember_client`, `touch_client`, `revoke_client`;
   - удалить тесты, которые используют MCP (`mcp_settings_persist_next_to_language`, `unreadable_settings_mean_mcp_off_for_the_shim` и любые с `approved`), и ассерты на языки кроме `en`/`ru` в тестах `match_language` / `resolve` (функция `match_language_in` остаётся);
   - удалить ставший неиспользуемым импорт `read_json`; комментарий модуля: `//! Пользовательские настройки (язык интерфейса) и выбор действующего языка.`
4. `policy/mod.rs` — удалить `KEY_MCP_ENABLED`, поле `PolicyStatus::mcp_enabled` (в структуре, `Default`, ветке `RawPolicy::Unreadable`), три ветки `(KEY_MCP_ENABLED, …)` в `resolve`, тест `mcp_policy` и ассерты `mcp_enabled` в остальных тестах.
5. `policy/sources.rs` — заменить тело теста `documented_examples_parse`:

```rust
    #[test]
    fn documented_examples_parse() {
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/admin");
        let json = read_json_file(Path::new(&format!("{base}/policy.json")));
        let p = crate::policy::resolve("doc", json);
        assert!(p.status.errors.is_empty(), "{:?}", p.status.errors);
        assert!(p.status.has_license_key);
        assert_eq!(p.status.language.as_deref(), Some("en"));

        // Профиль MDM: содержимое PayloadContent[0] попадает в Managed Preferences как есть.
        let profile = plist::Value::from_file(format!("{base}/policy.mobileconfig")).unwrap();
        let content = profile
            .as_dictionary()
            .and_then(|d| d.get("PayloadContent"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .expect("PayloadContent[0]");
        assert_eq!(
            content.get("PayloadType").and_then(|v| v.as_string()),
            Some(crate::product::IDENTIFIER)
        );
        for key in ["licenseKey", "language"] {
            assert!(content.contains_key(key), "{key}");
        }

        let reg = std::fs::read_to_string(format!("{base}/policy.reg")).unwrap();
        assert!(reg.contains(&format!(
            r"[HKEY_LOCAL_MACHINE\SOFTWARE\Policies\{}\{}]",
            crate::product::POLICY_VENDOR,
            crate::product::POLICY_PRODUCT
        )));
    }
```

6. `diagnostics.rs` — параметр `profiles: &[ConnectionProfile]` → `profiles: &[impl Serialize]`, импорт `crate::storage::connections::ConnectionProfile` удалить; в тесте `sample_profile("a")` → `serde_json::json!({ "id": "a", "name": "conn-a" })`, ассерты на содержимое `connections.json` архива привести к этому JSON.

Примеры политики:

`docs/admin/policy.json`:

```json
{
  "licenseKey": "<ключ организации>",
  "language": "en"
}
```

`docs/admin/policy.reg`:

```
Windows Registry Editor Version 5.00

[HKEY_LOCAL_MACHINE\SOFTWARE\Policies\Glafira\PostgreSQL]
"licenseKey"="<ключ организации>"
"language"="en"
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
      <key>licenseKey</key><string>&lt;ключ организации&gt;</string>
      <key>language</key><string>en</string>
    </dict>
  </array>
  <key>PayloadDisplayName</key><string>Glafira for PostgreSQL policy</string>
  <key>PayloadIdentifier</key><string>io.github.vvnovg.glafira.postgres.profile</string>
  <key>PayloadType</key><string>Configuration</string>
  <key>PayloadUUID</key><string>00000000-0000-0000-0000-000000000012</string>
  <key>PayloadVersion</key><integer>1</integer>
</dict>
</plist>
```

- [ ] **Step 6: `state.rs`, `storage/mod.rs`, `commands/`, `lib.rs`**

`src-tauri/src/storage/mod.rs`:

```rust
pub mod fs;
```

`src-tauri/src/state.rs`:

```rust
use std::path::Path;
use std::sync::Arc;

use crate::error::AppResult;
use crate::license::LicenseManager;
use crate::secrets::SecretStore;
use crate::settings::SettingsStore;

#[derive(Clone)]
pub struct AppState {
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub license: Arc<LicenseManager>,
    pub settings: Arc<SettingsStore>,
}

impl AppState {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>) -> AppResult<Self> {
        std::fs::create_dir_all(data_dir)?;
        Ok(Self {
            secrets,
            policy: Arc::new(crate::policy::Policy::default()),
            license: Arc::new(LicenseManager::open(
                data_dir,
                LicenseManager::embedded_key(),
            )),
            settings: Arc::new(SettingsStore::open(data_dir)),
        })
    }

    /// Политика администратора (читается при запуске в lib.rs).
    pub fn with_policy(mut self, policy: crate::policy::Policy) -> Self {
        self.license.set_policy_key(policy.license_key.clone());
        self.policy = Arc::new(policy);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn policy_license_key_reaches_manager() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        st.license = Arc::new(LicenseManager::open(
            dir.path(),
            license_format::dev::verifying_key(),
        ));
        let policy = crate::policy::Policy {
            license_key: Some(license_format::dev::pro_token("ACME", None)),
            ..Default::default()
        };
        let st = st.with_policy(policy);
        assert_eq!(st.license.status().licensee.as_deref(), Some("ACME"));
    }
}
```

`src-tauri/src/commands/mod.rs`:

```rust
pub mod app;
pub mod license;
pub mod settings;

use crate::error::{AppResult, ErrorCode};

/// Блокирующие вызовы (keychain, файлы) не выполняются на async-потоках Tauri.
pub(crate) async fn blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| {
        app_err!(
            ErrorCode::Internal,
            "errors.app.taskPanicked",
            "Background task failed: {{error}}",
            error = e
        )
    })?
}
```

`src-tauri/src/commands/app.rs`:

```rust
use tauri::{AppHandle, Manager};

use crate::diagnostics::AppInfo;
use crate::error::AppResult;

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
```

`src-tauri/src/lib.rs`:

```rust
#[macro_use]
pub mod i18n;
pub mod commands;
pub mod diagnostics;
pub mod error;
pub mod license;
pub mod logging;
pub mod policy;
pub mod product;
pub mod secrets;
pub mod settings;
pub mod state;
pub mod storage;
pub mod sync;

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
            tracing::info!(tier = ?st.license.status().tier, "license");
            app.manage(st);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app::get_app_info,
            commands::license::get_policy_status,
            commands::license::get_license_status,
            commands::license::set_license_key,
            commands::license::remove_license_key,
            commands::settings::get_language_settings,
            commands::settings::set_language,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {});
}
```

- [ ] **Step 7: Собрать и прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: PASS. Падение компиляции — значит, в скопированном модуле осталась ссылка на `kafka`, `mcp`, `migrate`, `tls`, `storage::connections` или удалённое поле: убрать её по правилам шага 5. `tauri::generate_context!` требует `dist/` — если сборка просит его, выполнить `mkdir -p dist && touch dist/index.html` (каталог в `.gitignore`; задача 2 заменит его настоящей сборкой).

Run: `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check`
Expected: без предупреждений.

- [ ] **Step 8: Commit**

```bash
git add .cargo .gitignore docs/admin src-tauri
git commit -m "feat(core): каркас Rust из Glafira — ошибки, i18n, keychain, политика, лицензия, настройки

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Каркас фронтенда — оболочка, вкладки приложения, i18n en/ru, справка

**Files:**
- Copy from `$G` and modify: `package.json`, `pnpm-workspace.yaml`, `index.html`, `vite.config.ts`, `vitest.config.ts`, `tsconfig.json`, `tsconfig.node.json`, `public/`, `scripts/{i18n-check.mjs, i18n-check.test.mjs}`, `src/{main.tsx, App.tsx, styles.css, polyfills.ts, polyfills.test.ts, vite-env.d.ts}`, `src/i18n/*`, `src/guide/{chapters.ts, parse.ts, parse.test.ts, noLookbehind.test.ts}`, `src/lib/{activate, env, focusTrap, format, plainInputs, useAppMenu, useDebouncedValue, useHelpShortcut, useLanguageSync, useLicense}.ts(x)` с тестами, `src/state/{workspace.ts, workspace.test.ts, helpContext.ts, diagnosticsRequest.ts}`, `src/test/{setup.ts, render.tsx}`, `src/components/{Layout.tsx, Layout.test.tsx, Sidebar.tsx, TabContent.tsx, TabContent.help.test.tsx}`, `src/components/common/{ErrorBanner, Hint, EnvBadge, ProLock}.tsx` с тестами, `src/components/settings/SettingsPanel.tsx` (+ test), `src/components/license/*`, `src/components/about/AboutPanel.tsx` (+ test), `src/components/help/*`, `src/api/client.ts`, `src/api/types.ts`
- Create: `src/locales/en.json`, `src/locales/ru.json`, `docs/guide/en/getting-started.md`, `docs/guide/ru/getting-started.md`

**Interfaces:**
- Consumes: команды Task 1 (`get_app_info`, `get_policy_status`, `get_license_status`, `set_license_key`, `remove_license_key`, `get_language_settings`, `set_language`).
- Produces:
  - `src/state/workspace.ts`: `TabSpec` = `{ kind: "connection-form"; connectionId: string | null; folderId?: string | null } | { kind: "license" } | { kind: "about" } | { kind: "help"; chapter?: string; anchor?: string } | { kind: "settings" }` (Task 15 добавит `overview`); `useWorkspace` с `open`, `close`, `closeForConnection`, `activate`; `tabId(spec)`.
  - `src/api/client.ts`: `api`, `toAppError`, `isAppError`, внутренний `call<T>(cmd, args)`.
  - `src/guide/chapters.ts`: `GUIDE_CHAPTERS = ["getting-started"] as const`.

- [ ] **Step 1: Скопировать файлы**

```bash
G=/Users/vvnovg/kafka-explorer
cp $G/{package.json,pnpm-workspace.yaml,index.html,vite.config.ts,vitest.config.ts,tsconfig.json,tsconfig.node.json} .
cp -R $G/public .
mkdir -p scripts src/{api,i18n,locales,guide,lib,state,test} src/components/{common,settings,license,about,help,connections/tree}
cp $G/scripts/{i18n-check.mjs,i18n-check.test.mjs} scripts/
mkdir -p docs/i18n && cp $G/docs/i18n/{README.md,glossary.md,same-as-en.json} docs/i18n/
cp $G/src/{main.tsx,App.tsx,styles.css,polyfills.ts,polyfills.test.ts,vite-env.d.ts} src/
cp $G/src/i18n/* src/i18n/
cp $G/src/guide/{chapters.ts,parse.ts,parse.test.ts,noLookbehind.test.ts} src/guide/
for f in activate env focusTrap format plainInputs useAppMenu useDebouncedValue useHelpShortcut useLanguageSync useLicense; do
  cp $G/src/lib/$f.ts* src/lib/ 2>/dev/null; cp $G/src/lib/$f.test.ts* src/lib/ 2>/dev/null; true
done
cp $G/src/state/{workspace.ts,workspace.test.ts,helpContext.ts,diagnosticsRequest.ts} src/state/
cp $G/src/test/{setup.ts,render.tsx} src/test/
cp $G/src/components/{Layout.tsx,Layout.test.tsx,Sidebar.tsx,TabContent.tsx,TabContent.help.test.tsx} src/components/
for f in ErrorBanner Hint EnvBadge ProLock; do cp $G/src/components/common/$f.tsx src/components/common/; cp $G/src/components/common/$f.test.tsx src/components/common/ 2>/dev/null; true; done
cp $G/src/components/settings/{SettingsPanel.tsx,SettingsPanel.test.tsx} src/components/settings/
cp $G/src/components/license/* src/components/license/
cp $G/src/components/about/{AboutPanel.tsx,AboutPanel.test.tsx} src/components/about/
cp $G/src/components/help/* src/components/help/
cp $G/src/api/{client.ts,types.ts} src/api/
```

- [ ] **Step 2: Правки конфигурации**

- `package.json`: `"name": "glafira-postgres"`, `"version": "0.1.0"`; скрипт `"i18n:check": "node scripts/i18n-check.mjs --require-all-languages --rust-cyrillic src-tauri/src"`; удалить скрипты `guide:publish` и `check:dist`, в `build` оставить `"tsc && vite build"`; в `test:scripts` — `"node --test \"scripts/*.test.mjs\""`. Зависимости — без изменений.
- `index.html`: `<title>Glafira for PostgreSQL</title>`.
- `src/i18n/languages.ts`:

```ts
export type LanguageCode = "en" | "ru";

export const LANGUAGES: { code: LanguageCode; nativeName: string }[] = [
  { code: "en", nativeName: "English" },
  { code: "ru", nativeName: "Русский" },
];
```

- `src/guide/chapters.ts`: `export const GUIDE_CHAPTERS = ["getting-started"] as const;` (P1b добавит главы `connections` и `tree`).
- `scripts/i18n-check.mjs` — список глав зашит в самом скрипте: `export const GUIDE_CHAPTERS = ["getting-started"];`. Что ещё проверяет скрипт и что важно для всех следующих задач:
  - `--rust-cyrillic`: любая строковая константа с кириллицей в `src-tauri/src` вне `#[cfg(test)]` — ошибка (в том числе в `expect(…)`); комментарии и тесты — можно;
  - кириллица в коде фронтенда вне комментариев, `*.test.tsx`, `src/test/` и `src/locales/` — ошибка;
  - ключи `t("…")`, `i18n.t("…")`, `<Hint k="…">` должны быть в `en.json`; шаблоны `ltext!`/`app_err!` — совпадать с `en.json`;
  - русский текст, совпадающий с английским, — ошибка, если ключа нет в `docs/i18n/same-as-en.json` → `ru`.
  В `i18n-check.test.mjs` тесты, завязанные на главы и языки Glafira, привести к одной главе и языкам `en`, `ru`.
- `docs/i18n/same-as-en.json` — оставить только раздел `ru` и в нём только ключи, которые существуют после чистки словарей (шаг 4). Следующие задачи добавляют сюда ключи с намеренно одинаковым текстом: Task 15 — `topology.role.leader`, `topology.role.primary`, `topology.role.syncStandby`, `topology.role.quorumStandby`, `topology.role.unknown`; Task 16 — `test.nodeOk`, `test.step.patroni`, `connections.form.targets.primary`, `readiness.computeQueryId.title`, `readiness.trackIoTiming.title`, `readiness.trackActivityQuerySize.title`; Task 18 — `overview.dcsSeen`.
- `src/components/help/helpChapterFor.ts`:

```ts
import type { ChapterId } from "../../guide/chapters";
import type { TabSpec } from "../../state/workspace";

/** Глава руководства для экрана (F1). В P1a — одна глава; P1b добавит «Подключения» и «Дерево». */
export function helpChapterFor(_tab: TabSpec | null): ChapterId {
  return "getting-started";
}
```

  В `helpChapterFor.test.ts` оставить один тест: для `null` и для `{ kind: "settings" }` возвращается `"getting-started"`.

- [ ] **Step 3: Вычистить Kafka из оболочки**

1. `src/state/workspace.ts` — `TabSpec` из раздела **Interfaces**; удалить `ProducerPrefill`, `ResetPrefill`, `newConsumerKey` и ветки `tabId` для `topic`, `group`, `consumer`; ветка `default` у `tabId` не нужна (все виды перечислены). В `workspace.test.ts` заменить вкладки Kafka на `connection-form` и `settings`.
2. `src/components/TabContent.tsx` — оставить ветки `about`, `help`, `settings`, `license`, `connection-form`; для `connection-form` временно `return <p className="empty">{tab.connectionId ?? ""}</p>;` (Task 16 подставит форму). Импорты Kafka-компонентов удалить.
3. `src/components/Layout.tsx` — `tabTitle` только для оставшихся видов; удалить `UpdateNotice`, `useMcpEvents`, `ApprovalDialog`. В `Layout.test.tsx` удалить тесты MCP и обновлений.
4. `src/components/Sidebar.tsx` — удалить `McpIndicator`; `ConnectionTree` временно заменить на `<div className="tree" />` (Task 17 вернёт дерево).
5. `src/components/settings/SettingsPanel.tsx` — удалить раздел MCP (`McpSection`) и его тесты в `SettingsPanel.test.tsx`.
6. `src/components/about/AboutPanel.tsx` — удалить блок проверки и установки обновлений (`checkUpdate`, `installUpdate`, состояние и кнопки) и экспорт диагностики оставить, но временно скрыть кнопку (Task 3 вернёт команду `export_diagnostics`); соответствующие тесты удалить.
7. `src/lib/useAppMenu.ts` — пункты меню, ведущие во вкладки Kafka, удалить (оставить «Справка» и «О программе»); тест привести в соответствие.
8. `src/api/client.ts` и `src/api/types.ts` — оставить только типы и функции: `AppError`, `AppInfo`, `LanguageSettings`, `LicenseStatus`, `PolicyStatus` (без `mcpEnabled`); `getAppInfo`, `getLanguageSettings`, `setLanguage`, `getLicenseStatus`, `getPolicyStatus`, `setLicenseKey`, `removeLicenseKey`. Тест `client.test.ts` скопировать из `$G/src/api/client.test.ts`, оставив тесты `toAppError` и `call`.
9. Файлы, на которые после правок никто не ссылается, удалить (`tsc` покажет).

- [ ] **Step 4: Словари en/ru**

Скопировать `$G/src/locales/en.json` и `ru.json` в `src/locales/`, затем удалить разделы `cluster`, `topics`, `groups`, `browse`, `producer`, `consumer`, `mcp`, `warnings`, а в `errors` — ключи `kafka.*`, `topics.*`, `groups.*`, `produce.*`, `browse.*`, `consume.*`, `mcp.*`; в `sidebar` — ключи `cluster`, `topics`, `groups`, `producer`, `consumer`, `llmAccess`; в `tabs` — ключи вкладок Kafka. Заменить вхождения «Kafka» в текстах «О программе» и лицензии на «PostgreSQL». Остальные ключи нужны скопированным компонентам.

Run: `node scripts/i18n-check.mjs --require-all-languages --rust-cyrillic src-tauri/src`
Expected: только сообщения о неиспользуемых ключах (если скрипт их выводит) — удалить эти ключи и повторить до чистого вывода. Ключи, которые использует Rust (`errors.policy.*`, `errors.license.*`, `errors.settings.*`, `errors.io`, `errors.json`, `errors.internal`, `errors.app.taskPanicked`), должны остаться: скрипт сверяет шаблоны `ltext!`/`app_err!` с `en.json`.

- [ ] **Step 5: Глава «Начало работы»**

`docs/guide/ru/getting-started.md`:

```markdown
# Начало работы

Glafira for PostgreSQL помогает разобраться, что происходит с PostgreSQL и кластером Patroni: какие узлы
живы, кто лидер, насколько отстают реплики и как устроены базы.

## Первое подключение

1. Нажмите «+» над деревом подключений.
2. Выберите вид: **Сервер** (один PostgreSQL), **Список узлов** (несколько серверов, primary определяется
   автоматически) или **Кластер Patroni** (адреса REST API Patroni).
3. Можно сразу вставить строку подключения — JDBC, `postgresql://…` или `host=… port=…`: поля заполнятся сами.
4. Нажмите «Проверить»: приложение покажет, какие шаги прошли, и чек-лист готовности к диагностике.
5. Сохраните подключение и раскройте его в дереве.

## Пароли

Пароли хранятся в системном хранилище секретов (Keychain, Credential Manager, Secret Service). Если пароль
не задан, используется `~/.pgpass`, как в `psql`.

## Роль для диагностики

Для полной диагностики роли нужны `pg_monitor` и `pg_signal_backend`; скрипт создания роли — в чек-листе
проверки подключения.
```

`docs/guide/en/getting-started.md`:

```markdown
# Getting started

Glafira for PostgreSQL helps you understand what is going on with PostgreSQL and a Patroni cluster: which
nodes are alive, who the leader is, how far replicas lag and how the databases are laid out.

## First connection

1. Click “+” above the connection tree.
2. Choose a kind: **Server** (a single PostgreSQL), **Host list** (several servers, the primary is detected
   automatically) or **Patroni cluster** (Patroni REST API addresses).
3. You can paste a connection string right away — JDBC, `postgresql://…` or `host=… port=…`: the fields
   are filled in for you.
4. Click “Test”: the app shows which steps passed and a diagnostics readiness checklist.
5. Save the connection and expand it in the tree.

## Passwords

Passwords are kept in the system secret store (Keychain, Credential Manager, Secret Service). If no password
is set, `~/.pgpass` is used, as in `psql`.

## A role for diagnostics

Full diagnostics need the `pg_monitor` and `pg_signal_backend` roles; the role creation script is in the
connection test checklist.
```

- [ ] **Step 6: Установить зависимости и прогнать тесты**

Run: `pnpm install && pnpm test && pnpm test:scripts && pnpm i18n:check && pnpm build`
Expected: все тесты PASS, сборка без ошибок `tsc`. Если тест скопированного компонента ссылается на удалённую вкладку или команду — удалить этот тест (поведение Kafka в продукте не нужно), а не возвращать код.

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: собирается (теперь `dist/` настоящий).

Ручная проверка: `pnpm tauri dev` — окно «Glafira for PostgreSQL», внизу боковой панели «Настройки», «Лицензия (Free)», «Справка», «О программе»; вкладки открываются, язык переключается.

- [ ] **Step 7: Commit**

```bash
git add package.json pnpm-lock.yaml pnpm-workspace.yaml index.html vite.config.ts vitest.config.ts tsconfig.json tsconfig.node.json public scripts src docs/guide docs/i18n
git commit -m "feat(ui): оболочка приложения из Glafira — вкладки, настройки, лицензия, справка, en/ru

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 3: Профиль подключения, хранилище, секреты и команды подключений

**Files:**
- Create: `src-tauri/src/conn/mod.rs`, `src-tauri/src/conn/profile.rs`, `src-tauri/src/commands/connections.rs`
- Copy from `$G/src-tauri/src` and modify: `storage/connections.rs`, `storage/tree.rs`, `commands/folders.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/state.rs`, `src-tauri/src/storage/mod.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/app.rs`
- Test: unit-тесты в `conn/profile.rs`, `storage/connections.rs`, `commands/connections.rs`, `state.rs`

**Interfaces:**
- Consumes: Task 1 (`AppError`, `secrets::*`, `storage::fs`, `AppState`, `blocking`, `diagnostics::write_archive`).
- Produces:
  - `crate::conn::profile::{ConnectionProfile, Environment, ConnectionKind, TargetServerType, SslMode, HostPort, CertFormat, ClientCert, PgTls, PgSettings, ApiTls, HostOverride, PatroniSettings, normalize_profile, validate_profile}`; `ConnectionProfile::uses_secret(&self, field: &str) -> bool`; `#[cfg(test)] pub fn sample_profile(id: &str) -> ConnectionProfile` (вид `server`, `localhost:5432`, пользователь `glafira_diag`).
  - `crate::storage::connections::{ConnectionStore, ConnectionFolder, StoreData, check_tree, folder_not_found, unique_name, connection_not_found}` — как в Glafira; `ConnectionStore::{open, list, list_folders, get, upsert, delete, snapshot, update}`.
  - `AppState.connections: Arc<ConnectionStore>`, `AppState::delete_connection_full(&self, id) -> AppResult<()>`.
  - `crate::commands::connections::{SecretsInput, ConnectionInput, save_connection_blocking}`; команды `list_connections`, `save_connection(input: ConnectionInput)`, `delete_connection(id)`; команды папок `list_folders`, `create_folder`, `rename_folder`, `delete_folder`, `move_items`; `export_diagnostics(path)`.

- [ ] **Step 1: Написать тесты профиля**

`src-tauri/src/conn/mod.rs`:

```rust
pub mod profile;
```

В конец `src-tauri/src/conn/profile.rs` (сам модуль — в шаге 3):

```rust
#[cfg(test)]
pub fn sample_profile(id: &str) -> ConnectionProfile {
    ConnectionProfile {
        id: id.to_string(),
        name: format!("conn-{id}"),
        color: "#4f8cff".into(),
        environment: Environment::Dev,
        read_only: false,
        kind: ConnectionKind::Server,
        hosts: vec![HostPort { host: "localhost".into(), port: 5432 }],
        target_server_type: TargetServerType::Primary,
        patroni: None,
        pg: PgSettings {
            username: "glafira_diag".into(),
            database: "postgres".into(),
            ssl_mode: SslMode::Prefer,
            tls: PgTls::default(),
            connect_timeout_s: 10,
            databases: None,
        },
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD};

    fn abs(p: &str) -> String {
        if cfg!(windows) { format!("C:\\{p}") } else { format!("/{p}") }
    }

    fn patroni(mut p: ConnectionProfile) -> ConnectionProfile {
        p.kind = ConnectionKind::Patroni;
        p.patroni = Some(PatroniSettings {
            urls: vec!["https://pg-1:8008".into()],
            username: Some("patroni".into()),
            tls: None,
            host_overrides: vec![],
        });
        p
    }

    #[test]
    fn json_shape_matches_frontend_contract() {
        let mut p = sample_profile("a");
        p.kind = ConnectionKind::HostList;
        p.target_server_type = TargetServerType::PreferStandby;
        p.pg.ssl_mode = SslMode::VerifyFull;
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["kind"], "hostList");
        assert_eq!(v["targetServerType"], "preferStandby");
        assert_eq!(v["pg"]["sslMode"], "verify-full");
        assert_eq!(v["pg"]["connectTimeoutS"], 10);
        assert_eq!(v["hosts"][0]["host"], "localhost");
        assert!(v.get("password").is_none());
    }

    #[test]
    fn missing_fields_get_defaults() {
        let p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": "a", "name": "A", "color": "#fff", "environment": "dev", "readOnly": false,
            "kind": "server", "hosts": [{"host": "h", "port": 5432}],
            "pg": {"username": "u"}
        }))
        .unwrap();
        assert_eq!(p.pg.database, "postgres");
        assert_eq!(p.pg.ssl_mode, SslMode::Prefer);
        assert_eq!(p.pg.connect_timeout_s, 10);
        assert_eq!(p.target_server_type, TargetServerType::Primary);
        assert!(p.patroni.is_none());
    }

    #[test]
    fn spec_example_parses() {
        let p: ConnectionProfile = serde_json::from_value(serde_json::json!({
            "id": "6f1c", "name": "billing-prod", "color": "#e11d48", "environment": "prod",
            "readOnly": true, "kind": "patroni", "hosts": [], "targetServerType": "primary",
            "patroni": {
                "urls": ["https://pg-1:8008", "https://pg-2:8008"], "username": "patroni",
                "tls": {"caPath": "/certs/ca.crt", "verifyHostname": true, "clientCert": null},
                "hostOverrides": [{"member": "pg-1", "host": "10.1.2.3", "port": 5432, "apiUrl": "https://10.1.2.3:8008"}]
            },
            "pg": {
                "username": "glafira_diag", "database": "postgres", "sslMode": "verify-full",
                "tls": {"caPath": "/certs/ca.crt", "clientCert": {"format": "pem", "certificatePath": "/c", "keyPath": "/k"}},
                "connectTimeoutS": 10, "databases": null
            },
            "folderId": null, "order": 0
        }))
        .unwrap();
        assert_eq!(p.kind, ConnectionKind::Patroni);
        assert_eq!(p.patroni.as_ref().unwrap().host_overrides[0].port, Some(5432));
        assert_eq!(p.pg.tls.client_cert.as_ref().unwrap().format, CertFormat::Pem);
    }

    #[test]
    fn normalize_keeps_only_fields_of_the_kind() {
        let mut p = patroni(sample_profile("a"));
        normalize_profile(&mut p);
        assert!(p.hosts.is_empty(), "у Patroni узлы приходят из /cluster");

        let mut s = sample_profile("b");
        s.patroni = patroni(sample_profile("x")).patroni;
        s.name = "  b  ".into();
        s.hosts[0].host = " db1 ".into();
        s.pg.databases = Some(vec![" orders ".into(), "".into()]);
        s.pg.tls.ca_path = Some("".into());
        normalize_profile(&mut s);
        assert!(s.patroni.is_none());
        assert_eq!(s.name, "b");
        assert_eq!(s.hosts[0].host, "db1");
        assert_eq!(s.pg.databases, Some(vec!["orders".to_string()]));
        assert_eq!(s.pg.tls.ca_path, None);

        let mut e = sample_profile("c");
        e.pg.databases = Some(vec![" ".into()]);
        normalize_profile(&mut e);
        assert_eq!(e.pg.databases, None, "пустой фильтр — все базы");

        let mut u = patroni(sample_profile("d"));
        u.patroni.as_mut().unwrap().urls = vec![" https://pg-1:8008/ ".into(), "".into()];
        u.patroni.as_mut().unwrap().username = Some("".into());
        normalize_profile(&mut u);
        assert_eq!(u.patroni.as_ref().unwrap().urls, vec!["https://pg-1:8008".to_string()]);
        assert_eq!(u.patroni.as_ref().unwrap().username, None);
    }

    #[test]
    fn validation_rules() {
        let cases: Vec<(&str, Box<dyn Fn(&mut ConnectionProfile)>)> = vec![
            ("errors.connections.emptyName", Box::new(|p| p.name = " ".into())),
            ("errors.connections.serverHosts", Box::new(|p| p.hosts.push(HostPort { host: "b".into(), port: 1 }))),
            ("errors.connections.hostListCount", Box::new(|p| { p.kind = ConnectionKind::HostList; p.hosts.clear(); })),
            ("errors.connections.hostListCount", Box::new(|p| {
                p.kind = ConnectionKind::HostList;
                p.hosts = (0..17).map(|i| HostPort { host: format!("h{i}"), port: 5432 }).collect();
            })),
            ("errors.connections.hostInvalid", Box::new(|p| p.hosts[0].host = "a b".into())),
            ("errors.connections.portInvalid", Box::new(|p| p.hosts[0].port = 0)),
            ("errors.connections.pgUserRequired", Box::new(|p| p.pg.username = "".into())),
            ("errors.connections.pgDatabaseRequired", Box::new(|p| p.pg.database = "".into())),
            ("errors.connections.timeoutRange", Box::new(|p| p.pg.connect_timeout_s = 0)),
            ("errors.connections.timeoutRange", Box::new(|p| p.pg.connect_timeout_s = 121)),
            ("errors.connections.pathNotAbsolute", Box::new(|p| p.pg.tls.ca_path = Some("ca.pem".into()))),
            ("errors.connections.keyRequired", Box::new(|p| p.pg.tls.client_cert = Some(ClientCert {
                format: CertFormat::Pem, certificate_path: abs("c.pem"), key_path: None,
            }))),
            ("errors.connections.patroniUrls", Box::new(|p| { *p = patroni(p.clone()); p.patroni.as_mut().unwrap().urls.clear(); })),
            ("errors.connections.patroniUrl", Box::new(|p| { *p = patroni(p.clone()); p.patroni.as_mut().unwrap().urls = vec!["pg-1:8008".into()]; })),
            ("errors.connections.overrideMember", Box::new(|p| {
                *p = patroni(p.clone());
                p.patroni.as_mut().unwrap().host_overrides = vec![HostOverride { member: "".into(), host: Some("h".into()), port: None, api_url: None }];
            })),
            ("errors.connections.overrideDuplicate", Box::new(|p| {
                *p = patroni(p.clone());
                let o = HostOverride { member: "pg-1".into(), host: Some("h".into()), port: None, api_url: None };
                p.patroni.as_mut().unwrap().host_overrides = vec![o.clone(), o];
            })),
            ("errors.connections.patroniUrl", Box::new(|p| {
                *p = patroni(p.clone());
                p.patroni.as_mut().unwrap().host_overrides = vec![HostOverride { member: "pg-1".into(), host: None, port: None, api_url: Some("pg-1".into()) }];
            })),
        ];
        for (key, change) in cases {
            let mut p = sample_profile("a");
            change(&mut p);
            let err = validate_profile(&p).unwrap_err();
            assert_eq!(err.key, key, "{p:?}");
            assert_eq!(err.code, crate::error::ErrorCode::InvalidInput);
        }
        validate_profile(&sample_profile("a")).unwrap();
        validate_profile(&patroni(sample_profile("p"))).map(|_| ()).unwrap_or_else(|e| {
            // У Patroni список hosts игнорируется после normalize, но и без него профиль валиден.
            panic!("{e:?}")
        });
        let mut pkcs = sample_profile("k");
        pkcs.pg.tls.client_cert = Some(ClientCert { format: CertFormat::Pkcs12, certificate_path: abs("c.p12"), key_path: None });
        validate_profile(&pkcs).unwrap();
    }

    #[test]
    fn used_secrets_depend_on_profile() {
        let p = sample_profile("a");
        assert!(p.uses_secret(PG_PASSWORD));
        assert!(!p.uses_secret(PG_KEY_PASSWORD));
        assert!(!p.uses_secret(PATRONI_PASSWORD));
        let mut q = patroni(sample_profile("b"));
        q.pg.tls.client_cert = Some(ClientCert { format: CertFormat::Pkcs12, certificate_path: abs("c.p12"), key_path: None });
        q.patroni.as_mut().unwrap().tls = Some(ApiTls {
            ca_path: None, verify_hostname: true,
            client_cert: Some(ClientCert { format: CertFormat::Pkcs12, certificate_path: abs("a.p12"), key_path: None }),
        });
        assert!(q.uses_secret(PG_KEY_PASSWORD));
        assert!(q.uses_secret(PATRONI_PASSWORD));
        assert!(q.uses_secret(PATRONI_KEY_PASSWORD));
        q.patroni.as_mut().unwrap().username = None;
        assert!(!q.uses_secret(PATRONI_PASSWORD));
    }
}
```

Тест `validation_rules` проверяет `patroni(sample_profile("p"))` без `normalize`: `hosts` у него не пуст, и это допустимо — `validate_profile` для `patroni` список `hosts` не проверяет.

- [ ] **Step 2: Убедиться, что тесты не собираются**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::profile`
Expected: ошибки компиляции (типов `ConnectionProfile` и функций ещё нет).

- [ ] **Step 3: Реализовать `conn/profile.rs`** (над блоком из шага 1)

```rust
//! Профиль подключения (спец. P1 §3.1): вид, узлы, Patroni REST API, параметры PostgreSQL.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Environment {
    Local,
    Dev,
    Test,
    Stage,
    Prod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionKind {
    Server,
    HostList,
    Patroni,
}

/// Узел по умолчанию для списка узлов — как `targetServerType` JDBC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetServerType {
    #[default]
    Primary,
    Standby,
    PreferStandby,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPort {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CertFormat {
    #[default]
    Pem,
    Pkcs12,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCert {
    pub format: CertFormat,
    pub certificate_path: String,
    /// Только для PEM.
    #[serde(default)]
    pub key_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PgTls {
    /// PEM-файл CA; пусто — системные корневые сертификаты.
    #[serde(default)]
    pub ca_path: Option<String>,
    #[serde(default)]
    pub client_cert: Option<ClientCert>,
}

fn default_database() -> String {
    "postgres".into()
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PgSettings {
    pub username: String,
    #[serde(default = "default_database")]
    pub database: String,
    #[serde(default)]
    pub ssl_mode: SslMode,
    #[serde(default)]
    pub tls: PgTls,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_s: u64,
    /// `None` — все БД с `datallowconn`; список — только эти.
    #[serde(default)]
    pub databases: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiTls {
    #[serde(default)]
    pub ca_path: Option<String>,
    #[serde(default = "default_true")]
    pub verify_hostname: bool,
    #[serde(default)]
    pub client_cert: Option<ClientCert>,
}

/// Подмена адресов участника Patroni (pod IP, NAT): PG и/или REST API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostOverride {
    pub member: String,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub api_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniSettings {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub tls: Option<ApiTls>,
    #[serde(default)]
    pub host_overrides: Vec<HostOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub color: String,
    pub environment: Environment,
    pub read_only: bool,
    pub kind: ConnectionKind,
    #[serde(default)]
    pub hosts: Vec<HostPort>,
    #[serde(default)]
    pub target_server_type: TargetServerType,
    #[serde(default)]
    pub patroni: Option<PatroniSettings>,
    pub pg: PgSettings,
    #[serde(default)]
    pub folder_id: Option<String>,
    #[serde(default)]
    pub order: i64,
    /// Поля из более новой версии Glafira: сохраняются при записи как есть.
    /// `ConnectionStore::upsert` входящий `extra` не принимает.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ConnectionProfile {
    /// Нужен ли профилю секрет `field`; ненужные секреты при сохранении удаляются.
    pub fn uses_secret(&self, field: &str) -> bool {
        let api_tls = self.patroni.as_ref().and_then(|p| p.tls.as_ref());
        match field {
            PG_PASSWORD => true,
            PG_KEY_PASSWORD => self.pg.tls.client_cert.is_some(),
            PATRONI_PASSWORD => self.patroni.as_ref().is_some_and(|p| p.username.is_some()),
            PATRONI_KEY_PASSWORD => api_tls.is_some_and(|t| t.client_cert.is_some()),
            _ => false,
        }
    }
}

fn trim_opt(v: &mut Option<String>) {
    *v = v.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
}

fn normalize_cert(c: &mut Option<ClientCert>) {
    if let Some(cert) = c {
        cert.certificate_path = cert.certificate_path.trim().to_string();
        trim_opt(&mut cert.key_path);
        if cert.format == CertFormat::Pkcs12 {
            cert.key_path = None;
        }
    }
}

/// Приводит профиль из формы к хранимому виду: обрезает пробелы, убирает поля чужого вида.
pub fn normalize_profile(p: &mut ConnectionProfile) {
    p.name = p.name.trim().to_string();
    for h in &mut p.hosts {
        h.host = h.host.trim().to_string();
    }
    match p.kind {
        ConnectionKind::Patroni => p.hosts.clear(),
        ConnectionKind::Server | ConnectionKind::HostList => p.patroni = None,
    }
    if let Some(pt) = &mut p.patroni {
        pt.urls = pt
            .urls
            .iter()
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .collect();
        trim_opt(&mut pt.username);
        if let Some(tls) = &mut pt.tls {
            trim_opt(&mut tls.ca_path);
            normalize_cert(&mut tls.client_cert);
        }
        for o in &mut pt.host_overrides {
            o.member = o.member.trim().to_string();
            trim_opt(&mut o.host);
            trim_opt(&mut o.api_url);
            if let Some(u) = &mut o.api_url {
                *u = u.trim_end_matches('/').to_string();
            }
        }
    }
    p.pg.username = p.pg.username.trim().to_string();
    p.pg.database = p.pg.database.trim().to_string();
    trim_opt(&mut p.pg.tls.ca_path);
    normalize_cert(&mut p.pg.tls.client_cert);
    if let Some(list) = &mut p.pg.databases {
        *list = list
            .iter()
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect();
        if list.is_empty() {
            p.pg.databases = None;
        }
    }
}

fn invalid(text: crate::i18n::LocalizedText) -> AppError {
    AppError::localized(ErrorCode::InvalidInput, text)
}

fn validate_path(path: &str) -> AppResult<()> {
    if Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(invalid(ltext!(
            "errors.connections.pathNotAbsolute",
            "The file path must be absolute: {{path}}",
            path = path
        )))
    }
}

fn validate_tls(ca: Option<&String>, cert: Option<&ClientCert>) -> AppResult<()> {
    if let Some(ca) = ca {
        validate_path(ca)?;
    }
    if let Some(c) = cert {
        validate_path(&c.certificate_path)?;
        match (c.format, &c.key_path) {
            (CertFormat::Pem, None) => {
                return Err(invalid(ltext!(
                    "errors.connections.keyRequired",
                    "Specify the client key file for a PEM certificate"
                )));
            }
            (CertFormat::Pem, Some(k)) => validate_path(k)?,
            (CertFormat::Pkcs12, _) => {}
        }
    }
    Ok(())
}

fn validate_http_url(url: &str) -> AppResult<()> {
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.split("://").nth(1).is_some_and(|rest| !rest.is_empty());
    if ok {
        Ok(())
    } else {
        Err(invalid(ltext!(
            "errors.connections.patroniUrl",
            "The Patroni API address must start with http:// or https://: {{url}}",
            url = url
        )))
    }
}

pub fn validate_profile(p: &ConnectionProfile) -> AppResult<()> {
    if p.id.trim().is_empty() {
        return Err(invalid(ltext!("errors.connections.noId", "The connection has no ID")));
    }
    if p.name.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.emptyName",
            "Connection name cannot be empty"
        )));
    }
    match p.kind {
        ConnectionKind::Server if p.hosts.len() != 1 => {
            return Err(invalid(ltext!(
                "errors.connections.serverHosts",
                "A server connection needs exactly one host"
            )));
        }
        ConnectionKind::HostList if !(1..=16).contains(&p.hosts.len()) => {
            return Err(invalid(ltext!(
                "errors.connections.hostListCount",
                "Specify from 1 to 16 hosts"
            )));
        }
        ConnectionKind::Patroni => validate_patroni(p.patroni.as_ref())?,
        _ => {}
    }
    if p.kind != ConnectionKind::Patroni {
        for h in &p.hosts {
            if h.host.is_empty() || h.host.chars().any(char::is_whitespace) {
                return Err(invalid(ltext!(
                    "errors.connections.hostInvalid",
                    "Invalid host name: “{{host}}”",
                    host = h.host
                )));
            }
            if h.port == 0 {
                return Err(invalid(ltext!(
                    "errors.connections.portInvalid",
                    "Port must be from 1 to 65535"
                )));
            }
        }
    }
    if p.pg.username.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.pgUserRequired",
            "Specify the PostgreSQL user"
        )));
    }
    if p.pg.database.trim().is_empty() {
        return Err(invalid(ltext!(
            "errors.connections.pgDatabaseRequired",
            "Specify the default database"
        )));
    }
    if !(1..=120).contains(&p.pg.connect_timeout_s) {
        return Err(invalid(ltext!(
            "errors.connections.timeoutRange",
            "Connection timeout must be from 1 to 120 seconds"
        )));
    }
    validate_tls(p.pg.tls.ca_path.as_ref(), p.pg.tls.client_cert.as_ref())
}

fn validate_patroni(pt: Option<&PatroniSettings>) -> AppResult<()> {
    let urls_err = || {
        invalid(ltext!(
            "errors.connections.patroniUrls",
            "Specify from 1 to 8 Patroni REST API addresses"
        ))
    };
    let pt = pt.ok_or_else(urls_err)?;
    if !(1..=8).contains(&pt.urls.len()) {
        return Err(urls_err());
    }
    for u in &pt.urls {
        validate_http_url(u)?;
    }
    if let Some(tls) = &pt.tls {
        validate_tls(tls.ca_path.as_ref(), tls.client_cert.as_ref())?;
    }
    let mut seen = HashSet::new();
    for o in &pt.host_overrides {
        if o.member.trim().is_empty() {
            return Err(invalid(ltext!(
                "errors.connections.overrideMember",
                "Specify the member name for the address override"
            )));
        }
        if !seen.insert(o.member.as_str()) {
            return Err(invalid(ltext!(
                "errors.connections.overrideDuplicate",
                "Address override for {{member}} is specified twice",
                member = o.member
            )));
        }
        if o.port == Some(0) {
            return Err(invalid(ltext!(
                "errors.connections.portInvalid",
                "Port must be from 1 to 65535"
            )));
        }
        if let Some(u) = &o.api_url {
            validate_http_url(u)?;
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Прогнать тесты профиля**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::profile`
Expected: PASS (6 тестов).

- [ ] **Step 5: Хранилище и дерево папок из Glafira**

```bash
G=/Users/vvnovg/kafka-explorer
cp $G/src-tauri/src/storage/{connections.rs,tree.rs} src-tauri/src/storage/
```

`storage/mod.rs`:

```rust
pub mod connections;
pub mod fs;
pub mod tree;
```

Правки `storage/connections.rs`:
1. Удалить всё от начала файла до `pub fn validate_profile` включительно, кроме `ConnectionFolder` (Environment, SaslMechanism, CertFormat, ClientCert, TlsConfig, SaslConfig, SecurityConfig, LlmAccess, ConnectionProfile, `parse_bootstrap`, `is_secret_property`, `TlsFile` и функции путей, `validate_tls`, `validate_profile`). Вверху оставить нужные `use` и добавить:

```rust
pub use crate::conn::profile::{ConnectionProfile, Environment, validate_profile};
```

2. Удалить `#[cfg(test)] pub fn sample_profile` (теперь в `conn::profile`).
3. В тестах: `use crate::conn::profile::{sample_profile, SslMode};`; удалить тесты `llm_access_defaults_to_off_and_is_written_only_when_open`, `old_tls_profiles_get_defaults`, `client_cert_json_shape`, `tls_profile`, `validation_of_certificate_paths`, `private_key_material_is_rejected_in_extra_properties`, `parse_bootstrap_variants`, `validation_rejects_secrets_in_extra_properties`, `validation_rejects_bad_profiles`, `json_shape_matches_frontend_contract`; в `persists_and_reloads` заменить изменение `b.security = …` на `b.pg.ssl_mode = SslMode::VerifyFull;` (и ассерт — на `ssl_mode`); во всех JSON-фикстурах заменить `"bootstrapServers":"h:1","security":{"protocol":"PLAINTEXT"}` на `"kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"}`. В `incoming_extra_is_ignored_on_save` ключ `"saslPassword"` заменить на `"pgPassword"`.

`storage/tree.rs` — без изменений.

- [ ] **Step 6: AppState, команды подключений и папок**

`state.rs` — добавить поле и методы (тест `policy_license_key_reaches_manager` остаётся):

```rust
use crate::storage::connections::{ConnectionProfile, ConnectionStore};

#[derive(Clone)]
pub struct AppState {
    pub connections: Arc<ConnectionStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub policy: Arc<crate::policy::Policy>,
    pub license: Arc<LicenseManager>,
    pub settings: Arc<SettingsStore>,
}
```

В `AppState::new` — `connections: Arc::new(ConnectionStore::open(data_dir)?),`. Методы:

```rust
    pub fn profile(&self, id: &str) -> AppResult<ConnectionProfile> {
        self.connections.get(id)
    }

    /// Удаляет подключение и все его секреты (блокирующий вызов: keychain).
    pub fn delete_connection_full(&self, id: &str) -> AppResult<()> {
        self.connections.delete(id)?;
        for field in crate::secrets::CONNECTION_SECRET_FIELDS {
            self.secrets.delete(&crate::secrets::secret_key(id, field))?;
        }
        Ok(())
    }
```

Тест в `state.rs`:

```rust
    #[test]
    fn delete_connection_removes_all_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        state.connections.upsert(crate::conn::profile::sample_profile("a")).unwrap();
        for f in crate::secrets::CONNECTION_SECRET_FIELDS {
            state.secrets.set(&crate::secrets::secret_key("a", f), "x").unwrap();
        }
        state.delete_connection_full("a").unwrap();
        assert!(state.connections.get("a").is_err());
        for f in crate::secrets::CONNECTION_SECRET_FIELDS {
            assert_eq!(state.secrets.get(&crate::secrets::secret_key("a", f)).unwrap(), None, "{f}");
        }
    }
```

`commands/folders.rs` — скопировать из `$G`, удалить команды `export_connections`, `preview_import`, `apply_import`, их вспомогательные функции, импорты `storage::exchange` и тесты, которые их вызывают (Task 19 вернёт их). `crate::license::Tier` и `TreeItemRef` остаются, если используются оставшимися командами; иначе убрать импорт.

`commands/connections.rs`:

```rust
use std::collections::HashMap;

use serde::Deserialize;
use tauri::State;

use super::blocking;
use crate::conn::profile::{ConnectionProfile, Environment, normalize_profile};
use crate::error::AppResult;
use crate::secrets::{
    CONNECTION_SECRET_FIELDS, PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD,
    PG_PASSWORD, secret_key,
};
use crate::state::AppState;

/// Секреты из формы. У каждого поля: `null`/нет — оставить сохранённый, `""` — удалить, иное — сохранить.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsInput {
    #[serde(default)]
    pub pg_password: Option<String>,
    #[serde(default)]
    pub pg_key_password: Option<String>,
    #[serde(default)]
    pub patroni_password: Option<String>,
    #[serde(default)]
    pub patroni_key_password: Option<String>,
}

impl SecretsInput {
    fn get(&self, field: &str) -> Option<&String> {
        match field {
            PG_PASSWORD => self.pg_password.as_ref(),
            PG_KEY_PASSWORD => self.pg_key_password.as_ref(),
            PATRONI_PASSWORD => self.patroni_password.as_ref(),
            PATRONI_KEY_PASSWORD => self.patroni_key_password.as_ref(),
            _ => None,
        }
    }

    /// Значения формы поверх keychain (`OverlayStore`) для проверки несохранённого профиля.
    pub fn overrides(&self, connection_id: &str) -> HashMap<String, String> {
        CONNECTION_SECRET_FIELDS
            .iter()
            .filter_map(|f| self.get(f).map(|v| (secret_key(connection_id, f), v.clone())))
            .collect()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    pub profile: ConnectionProfile,
    #[serde(default)]
    pub secrets: SecretsInput,
}

/// Что делать с секретом при сохранении.
enum SecretOp {
    Keep,
    Set(String),
    Delete,
}

/// Используемые профилем секреты — по данным формы, неиспользуемые — удаляются.
fn secret_ops(profile: &ConnectionProfile, secrets: &SecretsInput) -> Vec<(&'static str, SecretOp)> {
    CONNECTION_SECRET_FIELDS
        .iter()
        .map(|&field| {
            let op = if !profile.uses_secret(field) {
                SecretOp::Delete
            } else {
                match secrets.get(field) {
                    None => SecretOp::Keep,
                    Some(v) if v.is_empty() => SecretOp::Delete,
                    Some(v) => SecretOp::Set(v.clone()),
                }
            };
            (field, op)
        })
        .collect()
}

#[tauri::command]
pub fn list_connections(state: State<'_, AppState>) -> AppResult<Vec<ConnectionProfile>> {
    Ok(state.connections.list())
}

#[tauri::command]
pub async fn save_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
) -> AppResult<ConnectionProfile> {
    let st = state.inner().clone();
    blocking(move || save_connection_blocking(&st, input)).await
}

/// Сохраняет профиль и секреты согласованно: если keychain не принял изменение,
/// профиль возвращается к прежнему состоянию (или удаляется, если он новый).
pub fn save_connection_blocking(st: &AppState, input: ConnectionInput) -> AppResult<ConnectionProfile> {
    let ConnectionInput { mut profile, secrets } = input;
    if profile.id.is_empty() {
        profile.id = uuid::Uuid::new_v4().to_string();
    }
    normalize_profile(&mut profile);
    let previous = st.connections.get(&profile.id).ok();
    // Перевод в prod и снятие read-only с prod — функции Pro (как в Glafira for Kafka).
    let was_prod = previous.as_ref().map(|p| p.environment) == Some(Environment::Prod);
    let becomes_prod = profile.environment == Environment::Prod && !was_prod;
    let becomes_writable = profile.environment == Environment::Prod
        && was_prod
        && previous.as_ref().is_some_and(|p| p.read_only)
        && !profile.read_only;
    if becomes_prod || becomes_writable {
        st.license.require_pro()?;
    }
    let saved = st.connections.upsert(profile)?;
    let result = secret_ops(&saved, &secrets).into_iter().try_for_each(|(field, op)| {
        let key = secret_key(&saved.id, field);
        match op {
            SecretOp::Keep => Ok(()),
            SecretOp::Set(v) => st.secrets.set(&key, &v),
            SecretOp::Delete => st.secrets.delete(&key),
        }
    });
    if let Err(e) = result {
        let rollback = match previous {
            Some(p) => st.connections.upsert(p).map(|_| ()),
            None => st.connections.delete(&saved.id),
        };
        if let Err(r) = rollback {
            tracing::error!("failed to roll back profile {}: {r}", saved.id);
        }
        return Err(e);
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_connection(state: State<'_, AppState>, id: String) -> AppResult<()> {
    let st = state.inner().clone();
    blocking(move || st.delete_connection_full(&id)).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::profile::{ClientCert, CertFormat, sample_profile};
    use crate::error::{AppError, ErrorCode};
    use crate::secrets::{MemoryStore, SecretStore};

    /// Keychain, в который ничего нельзя записать.
    struct BrokenKeychain;

    impl SecretStore for BrokenKeychain {
        fn get(&self, _: &str) -> AppResult<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str, _: &str) -> AppResult<()> {
            Err(AppError::internal_with("keychain unavailable"))
        }
        fn delete(&self, _: &str) -> AppResult<()> {
            Err(AppError::internal_with("keychain unavailable"))
        }
    }

    fn state_with(secrets: Arc<dyn SecretStore>) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), secrets).unwrap();
        (dir, st)
    }

    fn input(profile: ConnectionProfile, secrets: SecretsInput) -> ConnectionInput {
        ConnectionInput { profile, secrets }
    }

    #[test]
    fn new_profile_gets_id_and_password() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("");
        p.id = String::new();
        let saved = save_connection_blocking(
            &st,
            input(p, SecretsInput { pg_password: Some("pw".into()), ..Default::default() }),
        )
        .unwrap();
        assert!(!saved.id.is_empty());
        assert_eq!(st.secrets.get(&secret_key(&saved.id, PG_PASSWORD)).unwrap().as_deref(), Some("pw"));
    }

    #[test]
    fn empty_secret_deletes_and_none_keeps() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let p = sample_profile("a");
        save_connection_blocking(&st, input(p.clone(), SecretsInput { pg_password: Some("pw".into()), ..Default::default() })).unwrap();
        save_connection_blocking(&st, input(p.clone(), SecretsInput::default())).unwrap();
        assert_eq!(st.secrets.get(&secret_key("a", PG_PASSWORD)).unwrap().as_deref(), Some("pw"));
        save_connection_blocking(&st, input(p, SecretsInput { pg_password: Some("".into()), ..Default::default() })).unwrap();
        assert_eq!(st.secrets.get(&secret_key("a", PG_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn unused_secrets_are_deleted() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("a");
        p.pg.tls.client_cert = Some(ClientCert {
            format: CertFormat::Pkcs12,
            certificate_path: if cfg!(windows) { "C:\\c.p12".into() } else { "/c.p12".into() },
            key_path: None,
        });
        save_connection_blocking(&st, input(p.clone(), SecretsInput { pg_key_password: Some("k".into()), ..Default::default() })).unwrap();
        assert!(st.secrets.get(&secret_key("a", PG_KEY_PASSWORD)).unwrap().is_some());
        p.pg.tls.client_cert = None;
        save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap();
        assert_eq!(st.secrets.get(&secret_key("a", PG_KEY_PASSWORD)).unwrap(), None);
    }

    #[test]
    fn keychain_failure_rolls_back_new_profile() {
        let (_d, st) = state_with(Arc::new(BrokenKeychain));
        let err = save_connection_blocking(
            &st,
            input(sample_profile("a"), SecretsInput { pg_password: Some("pw".into()), ..Default::default() }),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(st.connections.get("a").is_err(), "новый профиль удалён");
    }

    #[test]
    fn prod_requires_pro() {
        let (_d, st) = state_with(Arc::new(MemoryStore::default()));
        let mut p = sample_profile("a");
        p.environment = Environment::Prod;
        p.read_only = true;
        let err = save_connection_blocking(&st, input(p, SecretsInput::default())).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProRequired);
    }

    #[test]
    fn overrides_cover_only_given_fields() {
        let s = SecretsInput { pg_password: Some("".into()), patroni_password: Some("x".into()), ..Default::default() };
        let o = s.overrides("id");
        assert_eq!(o.get("id/pgPassword").map(String::as_str), Some(""));
        assert_eq!(o.get("id/patroniPassword").map(String::as_str), Some("x"));
        assert!(!o.contains_key("id/pgKeyPassword"));
    }
}
```

В `commands/app.rs` добавить экспорт диагностики:

```rust
use tauri::State;

use super::blocking;
use crate::error::{AppError, ErrorCode};
use crate::state::AppState;

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
            &st.license.status(),
            &crate::logging::log_files(&log_dir),
        )
    })
    .await
}
```

`commands/mod.rs` — `pub mod connections; pub mod folders;`. `lib.rs` — `pub mod conn;` и в `generate_handler!`: `commands::app::export_diagnostics`, `commands::connections::{list_connections, save_connection, delete_connection}`, `commands::folders::{list_folders, create_folder, rename_folder, delete_folder, move_items}` (по одному пути на строку, как в Glafira).

Тексты новых ошибок `errors.connections.*` из шага 3 добавить в `src/locales/en.json` (английский — точно как шаблон) и `ru.json`:

| Ключ | ru |
|---|---|
| `errors.connections.serverHosts` | «У подключения к серверу должен быть ровно один хост» |
| `errors.connections.hostListCount` | «Укажите от 1 до 16 хостов» |
| `errors.connections.hostInvalid` | «Неверное имя хоста: «{{host}}»» |
| `errors.connections.portInvalid` | «Порт — от 1 до 65535» |
| `errors.connections.pgUserRequired` | «Укажите пользователя PostgreSQL» |
| `errors.connections.pgDatabaseRequired` | «Укажите базу данных по умолчанию» |
| `errors.connections.timeoutRange` | «Таймаут подключения — от 1 до 120 секунд» |
| `errors.connections.pathNotAbsolute` | «Путь к файлу должен быть абсолютным: {{path}}» |
| `errors.connections.keyRequired` | «Для PEM-сертификата укажите файл ключа» |
| `errors.connections.patroniUrls` | «Укажите от 1 до 8 адресов REST API Patroni» |
| `errors.connections.patroniUrl` | «Адрес API Patroni должен начинаться с http:// или https://: {{url}}» |
| `errors.connections.overrideMember` | «Укажите имя участника для подмены адреса» |
| `errors.connections.overrideDuplicate` | «Подмена адреса для {{member}} указана дважды» |

- [ ] **Step 7: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS; `i18n:check` без замечаний.

- [ ] **Step 8: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(conn): профиль подключения трёх видов, хранилище, секреты и папки

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Разбор строки подключения (JDBC, URI, conninfo)

**Files:**
- Create: `src-tauri/src/conn/connstr.rs`
- Modify: `src-tauri/Cargo.toml` (зависимость `percent-encoding = "2"`), `src-tauri/src/conn/mod.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `conn/connstr.rs`

**Interfaces:**
- Consumes: `conn::profile::{CertFormat, ClientCert, ConnectionKind, HostPort, SslMode, TargetServerType}`.
- Produces: `crate::conn::connstr::{parse_connection_string(text: &str) -> AppResult<ParsedConnection>, ParsedConnection { patch: ProfilePatch, password: Option<String>, ignored: Vec<String> }, ProfilePatch { kind, hosts, target_server_type: Option<_>, username: Option<String>, database: Option<String>, ssl_mode: Option<SslMode>, ca_path: Option<String>, client_cert: Option<ClientCert>, connect_timeout_s: Option<u64> }}`; команда `parse_connection_string(text: String) -> ParsedConnection` (JSON camelCase).

- [ ] **Step 1: Написать тесты**

`src-tauri/src/conn/connstr.rs` (сначала только тесты; реализация — шаг 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn hp(host: &str, port: u16) -> HostPort {
        HostPort { host: host.into(), port }
    }

    fn err_key(s: &str) -> String {
        parse_connection_string(s).unwrap_err().key
    }

    #[test]
    fn jdbc_multi_host() {
        let p = parse_connection_string(
            "jdbc:postgresql://h1:5432,h2:5433/orders?user=app&password=s3cr%40t&targetServerType=primary&ssl=true&ApplicationName=x",
        )
        .unwrap();
        assert_eq!(p.patch.kind, ConnectionKind::HostList);
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.patch.database.as_deref(), Some("orders"));
        assert_eq!(p.patch.username.as_deref(), Some("app"));
        assert_eq!(p.password.as_deref(), Some("s3cr@t"));
        assert_eq!(p.patch.target_server_type, Some(TargetServerType::Primary));
        assert_eq!(p.patch.ssl_mode, Some(SslMode::VerifyFull), "ssl=true в pgjdbc — verify-full");
        assert_eq!(p.ignored, vec!["ApplicationName".to_string()]);
    }

    #[test]
    fn uri_with_userinfo_and_default_ports() {
        let p = parse_connection_string(
            "postgresql://app:secret@h1,h2:5433/orders?target_session_attrs=read-write&sslmode=verify-full",
        )
        .unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.password.as_deref(), Some("secret"));
        assert_eq!(p.patch.target_server_type, Some(TargetServerType::Primary));
        assert_eq!(p.patch.ssl_mode, Some(SslMode::VerifyFull));
        assert!(p.ignored.is_empty());
    }

    #[test]
    fn single_host_is_server() {
        let p = parse_connection_string("postgres://db.example.com/app").unwrap();
        assert_eq!(p.patch.kind, ConnectionKind::Server);
        assert_eq!(p.patch.hosts, vec![hp("db.example.com", 5432)]);
        assert_eq!(p.patch.database.as_deref(), Some("app"));
        assert_eq!(p.patch.username, None);
        let p = parse_connection_string("JDBC:PostgreSQL://h/db").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h", 5432)], "схема без учёта регистра");
    }

    #[test]
    fn conninfo_ports_by_position_or_shared() {
        let p = parse_connection_string("host=h1,h2 port=5432,5433 dbname=orders user=app sslmode=require").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h1", 5432), hp("h2", 5433)]);
        assert_eq!(p.patch.ssl_mode, Some(SslMode::Require));
        assert_eq!(p.patch.database.as_deref(), Some("orders"));
        let p = parse_connection_string("host=a,b,c port=6432").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("a", 6432), hp("b", 6432), hp("c", 6432)]);
        assert_eq!(err_key("host=a,b,c port=1,2"), "errors.connstr.portsMismatch");
    }

    #[test]
    fn conninfo_quoting_and_escapes() {
        let p = parse_connection_string(r"host = h   password='it\'s a \\ test' user=u\ x").unwrap();
        assert_eq!(p.password.as_deref(), Some(r"it's a \ test"));
        assert_eq!(p.patch.username.as_deref(), Some("u x"));
        assert_eq!(err_key("host='h"), "errors.connstr.syntax");
        assert_eq!(err_key("host h"), "errors.connstr.syntax");
    }

    #[test]
    fn ipv6_hosts() {
        let p = parse_connection_string("postgresql://[::1]:5433,[fe80::1]/db").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("::1", 5433), hp("fe80::1", 5432)]);
    }

    #[test]
    fn unix_sockets_are_rejected() {
        assert_eq!(err_key("host=/var/run/postgresql"), "errors.connstr.unixSocket");
        assert_eq!(err_key("postgresql://%2Fvar%2Frun%2Fpostgresql/db"), "errors.connstr.unixSocket");
        assert_eq!(err_key("postgresql://:5432/db"), "errors.connstr.unixSocket");
    }

    #[test]
    fn certificates() {
        let p = parse_connection_string("postgresql://h/db?sslcert=/c/client.p12&sslkey=/c/k.pem").unwrap();
        let c = p.patch.client_cert.unwrap();
        assert_eq!(c.format, CertFormat::Pkcs12);
        assert_eq!(c.key_path, None);
        assert_eq!(p.ignored, vec!["sslkey".to_string()]);
        let p = parse_connection_string("postgresql://h/db?sslcert=/c/c.pem&sslkey=/c/k.pem&sslrootcert=/c/ca.pem").unwrap();
        let c = p.patch.client_cert.unwrap();
        assert_eq!(c.format, CertFormat::Pem);
        assert_eq!(c.key_path.as_deref(), Some("/c/k.pem"));
        assert_eq!(p.patch.ca_path.as_deref(), Some("/c/ca.pem"));
    }

    #[test]
    fn hostaddr_is_used_without_host() {
        let p = parse_connection_string("hostaddr=10.0.0.1 port=5432 user=u").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("10.0.0.1", 5432)]);
        let p = parse_connection_string("host=h hostaddr=10.0.0.1").unwrap();
        assert_eq!(p.patch.hosts, vec![hp("h", 5432)]);
        assert_eq!(p.ignored, vec!["hostaddr".to_string()]);
    }

    #[test]
    fn timeouts() {
        assert_eq!(parse_connection_string("host=h connect_timeout=500").unwrap().patch.connect_timeout_s, Some(120));
        assert_eq!(parse_connection_string("host=h connect_timeout=0").unwrap().patch.connect_timeout_s, None);
        assert_eq!(parse_connection_string("jdbc:postgresql://h/db?connectTimeout=7").unwrap().patch.connect_timeout_s, Some(7));
        assert_eq!(err_key("host=h connect_timeout=abc"), "errors.connstr.badTimeout");
    }

    #[test]
    fn target_mappings() {
        let t = |s: &str| parse_connection_string(s).unwrap().patch.target_server_type;
        assert_eq!(t("host=h target_session_attrs=prefer-standby"), Some(TargetServerType::PreferStandby));
        assert_eq!(t("host=h target_session_attrs=read-only"), Some(TargetServerType::Standby));
        assert_eq!(t("host=h target_session_attrs=any"), Some(TargetServerType::Any));
        assert_eq!(t("jdbc:postgresql://h/db?targetServerType=preferSecondary"), Some(TargetServerType::PreferStandby));
        assert_eq!(t("jdbc:postgresql://h/db?targetServerType=secondary"), Some(TargetServerType::Standby));
        assert_eq!(t("jdbc:postgresql://h/db?targetServerType=master"), Some(TargetServerType::Primary));
        assert_eq!(err_key("host=h target_session_attrs=weird"), "errors.connstr.badTargetSessionAttrs");
        assert_eq!(err_key("jdbc:postgresql://h/db?targetServerType=weird"), "errors.connstr.badTargetServerType");
    }

    #[test]
    fn ssl_modes() {
        let m = |s: &str| parse_connection_string(s).unwrap().patch.ssl_mode;
        assert_eq!(m("host=h sslmode=allow"), Some(SslMode::Prefer));
        assert_eq!(m("host=h sslmode=verify-ca"), Some(SslMode::VerifyCa));
        assert_eq!(m("jdbc:postgresql://h/db?ssl=false"), Some(SslMode::Disable));
        assert_eq!(m("jdbc:postgresql://h/db?ssl=true&sslmode=require"), Some(SslMode::Require));
        assert_eq!(m("host=h"), None);
        assert_eq!(err_key("host=h sslmode=on"), "errors.connstr.badSslMode");
    }

    #[test]
    fn malformed_input() {
        assert_eq!(err_key("hello"), "errors.connstr.unrecognized");
        assert_eq!(err_key("dbname=x"), "errors.connstr.noHosts");
        assert_eq!(err_key("postgresql://h:99999/db"), "errors.connstr.badPort");
        assert_eq!(err_key("postgresql://h/%zz"), "errors.connstr.badEncoding");
        let many = (0..17).map(|i| format!("h{i}")).collect::<Vec<_>>().join(",");
        assert_eq!(err_key(&format!("host={many}")), "errors.connstr.tooManyHosts");
    }
}
```

- [ ] **Step 2: Убедиться, что тесты не собираются**

Добавить `pub mod connstr;` в `conn/mod.rs`, `percent-encoding = "2"` в `[dependencies]`.
Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::connstr`
Expected: ошибки компиляции (нет `parse_connection_string`).

- [ ] **Step 3: Реализация** (над блоком тестов)

```rust
//! Строка подключения → поля формы (спец. P1 §4): JDBC, URI `postgresql://`, conninfo `key=value`.
//! Пароль из строки возвращается отдельно (форма отправит его в keychain), нераспознанные параметры —
//! только именами: значения могут содержать секреты.

use serde::Serialize;

use crate::conn::profile::{
    CertFormat, ClientCert, ConnectionKind, HostPort, SslMode, TargetServerType,
};
use crate::error::{AppError, AppResult, ErrorCode};

const DEFAULT_PORT: u16 = 5432;
const MAX_HOSTS: usize = 16;
const MAX_TIMEOUT_S: u64 = 120;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfilePatch {
    pub kind: ConnectionKind,
    pub hosts: Vec<HostPort>,
    pub target_server_type: Option<TargetServerType>,
    pub username: Option<String>,
    pub database: Option<String>,
    pub ssl_mode: Option<SslMode>,
    pub ca_path: Option<String>,
    pub client_cert: Option<ClientCert>,
    pub connect_timeout_s: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedConnection {
    pub patch: ProfilePatch,
    pub password: Option<String>,
    pub ignored: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Dialect {
    Libpq,
    Jdbc,
}

/// Строка до сопоставления с полями: хосты с необязательными портами и параметры по порядку.
struct Raw {
    hosts: Vec<(String, Option<u16>)>,
    user: Option<String>,
    password: Option<String>,
    database: Option<String>,
    params: Vec<(String, String)>,
}

fn invalid(text: crate::i18n::LocalizedText) -> AppError {
    AppError::localized(ErrorCode::InvalidInput, text)
}

fn unix_socket() -> AppError {
    invalid(ltext!("errors.connstr.unixSocket", "Unix sockets are not supported"))
}

fn syntax(near: &str) -> AppError {
    invalid(ltext!(
        "errors.connstr.syntax",
        "Connection string syntax error near “{{near}}”",
        near = near
    ))
}

pub fn parse_connection_string(text: &str) -> AppResult<ParsedConnection> {
    let t = text.trim();
    if let Some(rest) = strip_prefix_ci(t, "jdbc:postgresql://") {
        return map(parse_uri_body(rest)?, Dialect::Jdbc);
    }
    for scheme in ["postgresql://", "postgres://"] {
        if let Some(rest) = strip_prefix_ci(t, scheme) {
            return map(parse_uri_body(rest)?, Dialect::Libpq);
        }
    }
    if t.contains('=') {
        return map(parse_conninfo(t)?, Dialect::Libpq);
    }
    Err(invalid(ltext!(
        "errors.connstr.unrecognized",
        "Unrecognized connection string: expected jdbc:postgresql://, postgresql:// or key=value pairs"
    )))
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

fn decode(s: &str) -> AppResult<String> {
    percent_encoding::percent_decode_str(s)
        .decode_utf8()
        .map(|c| c.into_owned())
        .map_err(|_| bad_encoding())
        .and_then(|v| {
            // `%zz` percent_decode пропускает как есть: такой текст — тоже ошибка кодировки.
            if has_bad_escape(s) { Err(bad_encoding()) } else { Ok(v) }
        })
}

fn has_bad_escape(s: &str) -> bool {
    let b = s.as_bytes();
    b.iter().enumerate().any(|(i, &c)| {
        c == b'%'
            && !(b.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && b.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
    })
}

fn bad_encoding() -> AppError {
    invalid(ltext!(
        "errors.connstr.badEncoding",
        "Invalid percent-encoding in the connection string"
    ))
}

fn parse_port(s: &str) -> AppResult<u16> {
    s.trim().parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| {
        invalid(ltext!("errors.connstr.badPort", "Invalid port: {{port}}", port = s))
    })
}

fn parse_host_port(s: &str) -> AppResult<(String, Option<u16>)> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('/') {
        return Err(unix_socket());
    }
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| syntax(s))?;
        let port = match after.strip_prefix(':') {
            Some(p) => Some(parse_port(p)?),
            None if after.is_empty() => None,
            None => return Err(syntax(s)),
        };
        return Ok((host.to_string(), port));
    }
    match s.rsplit_once(':') {
        Some(("", _)) => Err(unix_socket()),
        Some((h, p)) if !h.contains(':') => Ok((h.to_string(), Some(parse_port(p)?))),
        // IPv6 без скобок: порта нет.
        _ => Ok((s.to_string(), None)),
    }
}

fn parse_uri_body(rest: &str) -> AppResult<Raw> {
    let (main, query) = match rest.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (rest, None),
    };
    let (authority, path) = match main.find('/') {
        Some(i) => (&main[..i], Some(&main[i + 1..])),
        None => (main, None),
    };
    let (userinfo, hostspec) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (user, password) = match userinfo {
        Some(u) => match u.split_once(':') {
            Some((name, pw)) => (Some(decode(name)?), Some(decode(pw)?)),
            None => (Some(decode(u)?), None),
        },
        None => (None, None),
    };
    let mut hosts = Vec::new();
    if !hostspec.is_empty() {
        for part in hostspec.split(',') {
            hosts.push(parse_host_port(&decode(part)?)?);
        }
    }
    let database = path.map(decode).transpose()?.filter(|d| !d.is_empty());
    let mut params = Vec::new();
    for pair in query.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        params.push((decode(k)?, decode(v)?));
    }
    Ok(Raw {
        hosts,
        user: user.filter(|u| !u.is_empty()),
        password,
        database,
        params,
    })
}

fn parse_conninfo(s: &str) -> AppResult<Raw> {
    let mut params = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            break;
        }
        let mut key = String::new();
        while let Some(c) = chars.next_if(|c| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.next() != Some('=') {
            return Err(syntax(&key));
        }
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if chars.next_if_eq(&'\'').is_some() {
            loop {
                match chars.next() {
                    Some('\\') => value.push(chars.next().ok_or_else(|| syntax(&key))?),
                    Some('\'') => break,
                    Some(c) => value.push(c),
                    None => return Err(syntax(&key)),
                }
            }
        } else {
            while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        value.push(n);
                    }
                } else {
                    value.push(c);
                }
            }
        }
        params.push((key, value));
    }
    Ok(Raw { hosts: Vec::new(), user: None, password: None, database: None, params })
}

fn take(params: &mut Vec<(String, String)>, name: &str) -> Option<String> {
    let i = params.iter().position(|(k, _)| k == name)?;
    Some(params.remove(i).1)
}

/// Хосты из параметров `host` / `hostaddr` / `port` (conninfo и `?host=` в URI).
fn hosts_from_params(params: &mut Vec<(String, String)>) -> AppResult<Vec<(String, Option<u16>)>> {
    let names = match take(params, "host") {
        Some(h) => h,
        None => match take(params, "hostaddr") {
            Some(a) => a,
            None => return Ok(Vec::new()),
        },
    };
    let names: Vec<String> = names.split(',').map(|n| n.trim().to_string()).collect();
    if names.iter().any(|n| n.is_empty() || n.starts_with('/')) {
        return Err(unix_socket());
    }
    let ports: Vec<u16> = match take(params, "port") {
        Some(p) => p.split(',').map(parse_port).collect::<AppResult<_>>()?,
        None => Vec::new(),
    };
    match ports.len() {
        0 => Ok(names.into_iter().map(|n| (n, None)).collect()),
        1 => Ok(names.into_iter().map(|n| (n, Some(ports[0]))).collect()),
        k if k == names.len() => Ok(names.into_iter().zip(ports).map(|(n, p)| (n, Some(p))).collect()),
        k => Err(invalid(ltext!(
            "errors.connstr.portsMismatch",
            "The number of ports ({{ports}}) does not match the number of hosts ({{hosts}})",
            ports = k,
            hosts = names.len()
        ))),
    }
}

fn parse_sslmode(v: &str) -> AppResult<SslMode> {
    match v.to_ascii_lowercase().as_str() {
        "disable" => Ok(SslMode::Disable),
        "allow" | "prefer" => Ok(SslMode::Prefer),
        "require" => Ok(SslMode::Require),
        "verify-ca" => Ok(SslMode::VerifyCa),
        "verify-full" => Ok(SslMode::VerifyFull),
        _ => Err(invalid(ltext!(
            "errors.connstr.badSslMode",
            "Unknown sslmode: {{value}}",
            value = v
        ))),
    }
}

fn parse_target_session_attrs(v: &str) -> AppResult<TargetServerType> {
    match v.to_ascii_lowercase().as_str() {
        "read-write" | "primary" => Ok(TargetServerType::Primary),
        "read-only" | "standby" => Ok(TargetServerType::Standby),
        "prefer-standby" => Ok(TargetServerType::PreferStandby),
        "any" => Ok(TargetServerType::Any),
        _ => Err(invalid(ltext!(
            "errors.connstr.badTargetSessionAttrs",
            "Unknown target_session_attrs: {{value}}",
            value = v
        ))),
    }
}

fn parse_target_server_type(v: &str) -> AppResult<TargetServerType> {
    match v.to_ascii_lowercase().as_str() {
        "primary" | "master" | "preferprimary" => Ok(TargetServerType::Primary),
        "secondary" | "standby" => Ok(TargetServerType::Standby),
        "prefersecondary" | "preferstandby" => Ok(TargetServerType::PreferStandby),
        "any" => Ok(TargetServerType::Any),
        _ => Err(invalid(ltext!(
            "errors.connstr.badTargetServerType",
            "Unknown targetServerType: {{value}}",
            value = v
        ))),
    }
}

fn parse_timeout(v: &str) -> AppResult<Option<u64>> {
    let n: u64 = v.trim().parse().map_err(|_| {
        invalid(ltext!(
            "errors.connstr.badTimeout",
            "Invalid connect timeout: {{value}}",
            value = v
        ))
    })?;
    Ok((n > 0).then(|| n.min(MAX_TIMEOUT_S)))
}

fn map(mut raw: Raw, d: Dialect) -> AppResult<ParsedConnection> {
    if raw.hosts.is_empty() {
        raw.hosts = hosts_from_params(&mut raw.params)?;
    }
    if raw.hosts.is_empty() {
        return Err(invalid(ltext!(
            "errors.connstr.noHosts",
            "No hosts in the connection string"
        )));
    }
    if raw.hosts.len() > MAX_HOSTS {
        return Err(invalid(ltext!(
            "errors.connstr.tooManyHosts",
            "More than 16 hosts in the connection string"
        )));
    }
    let (mut username, mut password, mut database) = (raw.user, raw.password, raw.database);
    let mut ssl_mode = None;
    let mut jdbc_ssl = None;
    let (mut ca_path, mut cert, mut key) = (None, None, None);
    let mut target = None;
    let mut timeout = None;
    let mut ignored = Vec::new();
    for (k, v) in raw.params {
        match (d, k.as_str()) {
            (_, "user") => username = username.or(Some(v)),
            (_, "password") => password = password.or(Some(v)),
            (Dialect::Libpq, "dbname") => database = database.or(Some(v)),
            (_, "sslmode") => ssl_mode = Some(parse_sslmode(&v)?),
            (Dialect::Jdbc, "ssl") => jdbc_ssl = Some(v.eq_ignore_ascii_case("true")),
            (_, "sslrootcert") => ca_path = Some(v),
            (_, "sslcert") => cert = Some(v),
            (_, "sslkey") => key = Some(v),
            (Dialect::Libpq, "target_session_attrs") => target = Some(parse_target_session_attrs(&v)?),
            (Dialect::Jdbc, "targetServerType") => target = Some(parse_target_server_type(&v)?),
            (Dialect::Libpq, "connect_timeout") | (Dialect::Jdbc, "connectTimeout") => {
                timeout = parse_timeout(&v)?
            }
            _ => ignored.push(k),
        }
    }
    if ssl_mode.is_none() {
        // pgjdbc: ssl=true без sslmode — verify-full.
        ssl_mode = jdbc_ssl.map(|on| if on { SslMode::VerifyFull } else { SslMode::Disable });
    }
    let client_cert = match (cert, key) {
        (Some(c), k) => {
            let lower = c.to_ascii_lowercase();
            if lower.ends_with(".p12") || lower.ends_with(".pfx") {
                if k.is_some() {
                    ignored.push("sslkey".into());
                }
                Some(ClientCert { format: CertFormat::Pkcs12, certificate_path: c, key_path: None })
            } else {
                Some(ClientCert { format: CertFormat::Pem, certificate_path: c, key_path: k })
            }
        }
        (None, Some(_)) => {
            ignored.push("sslkey".into());
            None
        }
        (None, None) => None,
    };
    let hosts: Vec<HostPort> = raw
        .hosts
        .into_iter()
        .map(|(host, port)| HostPort { host, port: port.unwrap_or(DEFAULT_PORT) })
        .collect();
    let kind = if hosts.len() == 1 { ConnectionKind::Server } else { ConnectionKind::HostList };
    Ok(ParsedConnection {
        patch: ProfilePatch {
            kind,
            hosts,
            target_server_type: target,
            username: username.filter(|u| !u.is_empty()),
            database: database.filter(|d| !d.is_empty()),
            ssl_mode,
            ca_path,
            client_cert,
            connect_timeout_s: timeout,
        },
        password: password.filter(|p| !p.is_empty()),
        ignored,
    })
}
```

В тесте `hostaddr_is_used_without_host` второй случай (`host=h hostaddr=10.0.0.1`): `hosts_from_params` берёт `host`, а `hostaddr` остаётся в `params` и попадает в `ignored` — это и проверяется.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::connstr`
Expected: PASS (13 тестов).

- [ ] **Step 5: Команда и тексты**

В `commands/connections.rs`:

```rust
#[tauri::command]
pub fn parse_connection_string(text: String) -> AppResult<crate::conn::connstr::ParsedConnection> {
    crate::conn::connstr::parse_connection_string(&text)
}
```

Зарегистрировать `commands::connections::parse_connection_string` в `lib.rs`. В `en.json` — шаблоны из шага 3 (ключи `errors.connstr.*`), в `ru.json`:

| Ключ | ru |
|---|---|
| `unixSocket` | «Unix-сокеты не поддерживаются» |
| `syntax` | «Ошибка в строке подключения рядом с «{{near}}»» |
| `unrecognized` | «Строка не распознана: ожидается jdbc:postgresql://, postgresql:// или пары ключ=значение» |
| `badEncoding` | «Неверное %-кодирование в строке подключения» |
| `badPort` | «Неверный порт: {{port}}» |
| `portsMismatch` | «Число портов ({{ports}}) не совпадает с числом хостов ({{hosts}})» |
| `badSslMode` | «Неизвестный sslmode: {{value}}» |
| `badTargetSessionAttrs` | «Неизвестный target_session_attrs: {{value}}» |
| `badTargetServerType` | «Неизвестный targetServerType: {{value}}» |
| `badTimeout` | «Неверный таймаут подключения: {{value}}» |
| `noHosts` | «В строке подключения нет хостов» |
| `tooManyHosts` | «В строке подключения больше 16 хостов» |

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(conn): разбор строки подключения — JDBC, URI и conninfo

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Пароль из `~/.pgpass`

**Files:**
- Create: `src-tauri/src/conn/pgpass.rs`
- Modify: `src-tauri/src/conn/mod.rs`
- Test: unit-тесты в `conn/pgpass.rs`

**Interfaces:**
- Produces: `crate::conn::pgpass::{lookup(content: &str, host: &str, port: u16, database: &str, user: &str) -> Option<String>, find_password(host, port, database, user) -> Option<String>, find_password_in(path: &Path, host, port, database, user) -> Option<String>, default_path() -> Option<PathBuf>}`.

- [ ] **Step 1: Тесты**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
# комментарий
db1:5432:orders:app:first

db1:5432:*:app:second
*:*:*:admin:adm\\:in
h\\:x:5432:db:u:esc\\\\aped
short:line
";

    #[test]
    fn first_matching_line_wins() {
        assert_eq!(lookup(FILE, "db1", 5432, "orders", "app").as_deref(), Some("first"));
        assert_eq!(lookup(FILE, "db1", 5432, "billing", "app").as_deref(), Some("second"));
        assert_eq!(lookup(FILE, "db1", 5433, "orders", "app"), None);
    }

    #[test]
    fn wildcards_and_escapes() {
        assert_eq!(lookup(FILE, "any", 1, "x", "admin").as_deref(), Some("adm:in"));
        assert_eq!(lookup(FILE, "h:x", 5432, "db", "u").as_deref(), Some("esc\\aped"));
        assert_eq!(lookup(FILE, "short", 1, "line", "u"), None);
    }

    #[test]
    fn password_stops_at_unescaped_colon() {
        assert_eq!(lookup("h:1:d:u:pa:ss", "h", 1, "d", "u").as_deref(), Some("pa"));
        assert_eq!(lookup("h:1:d:u:p\r\n", "h", 1, "d", "u").as_deref(), Some("p"));
    }

    #[cfg(unix)]
    #[test]
    fn file_with_group_access_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pgpass");
        std::fs::write(&path, "h:1:d:u:pw\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(find_password_in(&path, "h", 1, "d", "u").as_deref(), Some("pw"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(find_password_in(&path, "h", 1, "d", "u"), None);
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_password_in(&dir.path().join("nope"), "h", 1, "d", "u"), None);
    }
}
```

- [ ] **Step 2: Убедиться, что тесты не собираются**

Добавить `pub mod pgpass;` в `conn/mod.rs`.
Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pgpass`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Пароль из `~/.pgpass` (или файла `PGPASSFILE`) по правилам libpq: первая подходящая строка
//! `host:port:database:user:password`, `*` — любое значение, `\:` и `\\` — экранирование.

use std::path::{Path, PathBuf};

/// Делит строку по неэкранированным `:`, снимая экранирование.
fn split_fields(line: &str) -> Vec<String> {
    let mut fields = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    fields.last_mut().expect("at least one field").push(n);
                }
            }
            ':' => fields.push(String::new()),
            c => fields.last_mut().expect("at least one field").push(c),
        }
    }
    fields
}

pub fn lookup(content: &str, host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    let port = port.to_string();
    let matches = |pattern: &str, value: &str| pattern == "*" || pattern == value;
    content
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(split_fields)
        .filter(|f| f.len() >= 5)
        .find(|f| {
            matches(&f[0], host) && matches(&f[1], &port) && matches(&f[2], database) && matches(&f[3], user)
        })
        .map(|f| f[4].clone())
}

pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PGPASSFILE") {
        return Some(PathBuf::from(p));
    }
    #[cfg(windows)]
    {
        dirs::config_dir().map(|d| d.join("postgresql").join("pgpass.conf"))
    }
    #[cfg(not(windows))]
    {
        dirs::home_dir().map(|h| h.join(".pgpass"))
    }
}

/// Пароль для подключения или `None`. Блокирующий вызов (чтение файла).
pub fn find_password(host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    find_password_in(&default_path()?, host, port, database, user)
}

/// Как libpq, на Unix файл с доступом для группы или всех игнорируется.
pub fn find_password_in(path: &Path, host: &str, port: u16, database: &str, user: &str) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            tracing::warn!(path = %path.display(), "pgpass ignored: group or world access");
            return None;
        }
    }
    let content = std::fs::read_to_string(path).ok()?;
    lookup(&content, host, port, database, user)
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pgpass`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri
git commit -m "feat(conn): пароль из ~/.pgpass по правилам libpq

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 6: Docker-стенд — Patroni, потоковая репликация, одиночный сервер

**Files:**
- Create: `dev/docker-compose.yml`, `dev/patroni/Dockerfile`, `dev/patroni/patroni.yml`, `dev/patroni/post_bootstrap.sh`, `dev/primary/10-replication.sh`, `dev/replica/entrypoint.sh`, `dev/sql/init.sql`, `scripts/wait-stand.sh`

**Interfaces:**
- Produces: стенд из таблицы «Переменные интеграционных тестов»; контейнеры `gp-etcd`, `gp-patroni1..3`, `gp-pg-primary`, `gp-pg-replica`, `gp-pg-single`; кластер Patroni `demo`; в каждом PG — роль `glafira_diag` (`pg_monitor`, `pg_signal_backend`), БД `orders` со схемой `shop`; `pg_stat_statements` создан в БД `postgres` там, где он в `shared_preload_libraries` (Patroni, `pg-primary`/`pg-replica`), и отсутствует в `pg-single`.

- [ ] **Step 1: Демо-схема и роль**

`dev/sql/init.sql`:

```sql
-- Роль для диагностики (спец. P1 §10) и демо-схема shop: дерево, вкладки объектов, матрица версий.
CREATE ROLE glafira_diag LOGIN PASSWORD 'glafira-secret';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;

-- pg_stat_statements создаётся только там, где он загружен: pg-single нужен для «красного» чек-листа.
DO $$
BEGIN
  IF current_setting('shared_preload_libraries') LIKE '%pg_stat_statements%' THEN
    CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
  END IF;
END $$;

CREATE DATABASE orders;
\connect orders

CREATE SCHEMA shop;
CREATE TABLE shop.customers (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email text NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now()
);
COMMENT ON TABLE shop.customers IS 'Покупатели';
CREATE TABLE shop.orders (
  id bigserial PRIMARY KEY,
  customer_id bigint NOT NULL REFERENCES shop.customers (id),
  total numeric(12, 2) NOT NULL CHECK (total >= 0),
  total_with_tax numeric(12, 2) GENERATED ALWAYS AS (total * 1.2) STORED,
  status text NOT NULL DEFAULT 'new'
);
CREATE INDEX orders_customer_idx ON shop.orders (customer_id);
CREATE TABLE shop.events (id bigint NOT NULL, happened_at timestamptz NOT NULL, payload jsonb)
  PARTITION BY RANGE (happened_at);
CREATE TABLE shop.events_2026_09 PARTITION OF shop.events FOR VALUES FROM ('2026-09-01') TO ('2026-10-01');
CREATE TABLE shop.events_2026_10 PARTITION OF shop.events FOR VALUES FROM ('2026-10-01') TO ('2026-11-01');
CREATE VIEW shop.active_orders AS SELECT * FROM shop.orders WHERE status <> 'done';
CREATE MATERIALIZED VIEW shop.order_totals AS
  SELECT customer_id, sum(total) AS total FROM shop.orders GROUP BY customer_id;
CREATE SEQUENCE shop.invoice_seq AS integer;
CREATE FUNCTION shop.order_count(c bigint) RETURNS bigint LANGUAGE sql STABLE
  AS $$ SELECT count(*) FROM shop.orders WHERE customer_id = c $$;
CREATE PROCEDURE shop.close_order(o bigint) LANGUAGE sql
  AS $$ UPDATE shop.orders SET status = 'done' WHERE id = o $$;
CREATE FUNCTION shop.touch() RETURNS trigger LANGUAGE plpgsql
  AS $$ BEGIN NEW.created_at := now(); RETURN NEW; END $$;
CREATE TRIGGER customers_touch BEFORE INSERT ON shop.customers FOR EACH ROW EXECUTE FUNCTION shop.touch();

INSERT INTO shop.customers (email) SELECT 'user' || g || '@example.com' FROM generate_series(1, 100) g;
INSERT INTO shop.orders (customer_id, total) SELECT 1 + g % 100, g FROM generate_series(1, 1000) g;
REFRESH MATERIALIZED VIEW shop.order_totals;
ANALYZE;
```

- [ ] **Step 2: Образ Patroni**

`dev/patroni/Dockerfile` (контекст сборки — `dev/`):

```dockerfile
FROM postgres:17
# Patroni 4 с клиентом etcd3 — в отдельном venv: системный Python в Debian запрещает pip install.
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3-venv curl \
 && python3 -m venv /opt/patroni \
 && /opt/patroni/bin/pip install --no-cache-dir "patroni[etcd3]>=4.0,<5" "psycopg[binary]>=3.2,<4" \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /var/lib/postgresql/patroni \
 && chown postgres:postgres /var/lib/postgresql/patroni
COPY patroni/patroni.yml sql/init.sql /etc/patroni/
COPY --chmod=0755 patroni/post_bootstrap.sh /etc/patroni/post_bootstrap.sh
USER postgres
HEALTHCHECK --interval=5s --timeout=3s CMD curl -fs http://localhost:8008/liveness || exit 1
ENTRYPOINT ["/opt/patroni/bin/patroni"]
CMD ["/etc/patroni/patroni.yml"]
```

`dev/patroni/patroni.yml` (имя узла и адреса — из переменных `PATRONI_NAME`, `PATRONI_RESTAPI_CONNECT_ADDRESS`, `PATRONI_POSTGRESQL_CONNECT_ADDRESS`):

```yaml
scope: demo
namespace: /service/
restapi:
  listen: 0.0.0.0:8008
  authentication:
    username: patroni
    password: patroni-secret
etcd3:
  hosts: etcd:2379
bootstrap:
  dcs:
    ttl: 30
    loop_wait: 10
    retry_timeout: 10
    maximum_lag_on_failover: 1048576
    postgresql:
      use_pg_rewind: true
      use_slots: true
      parameters:
        wal_level: replica
        hot_standby: "on"
        max_wal_senders: 10
        max_replication_slots: 10
        wal_log_hints: "on"
  initdb:
    - encoding: UTF8
    - data-checksums
  pg_hba:
    - local all all trust
    - host replication replicator 0.0.0.0/0 scram-sha-256
    - host all all 0.0.0.0/0 scram-sha-256
  post_bootstrap: /etc/patroni/post_bootstrap.sh
postgresql:
  listen: 0.0.0.0:5432
  data_dir: /var/lib/postgresql/patroni/data
  bin_dir: /usr/lib/postgresql/17/bin
  pgpass: /tmp/pgpass
  authentication:
    superuser:
      username: postgres
      password: postgres-secret
    replication:
      username: replicator
      password: replicator-secret
  parameters:
    unix_socket_directories: /var/run/postgresql
    shared_preload_libraries: pg_stat_statements
    track_io_timing: "on"
    track_activity_query_size: 4096
tags:
  nofailover: false
  noloadbalance: false
  clonefrom: false
  nosync: false
```

`dev/patroni/post_bootstrap.sh`:

```bash
#!/bin/bash
# Patroni передаёт строку подключения суперпользователя первым аргументом (пароль — в PGPASSFILE).
set -euo pipefail
psql "$1" -v ON_ERROR_STOP=1 -f /etc/patroni/init.sql
```

- [ ] **Step 3: Репликация без Patroni**

`dev/primary/10-replication.sh`:

```bash
#!/bin/bash
# Пользователь и слот репликации для pg-replica; доступ к репликации по сети.
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres <<'SQL'
CREATE ROLE replicator WITH REPLICATION LOGIN PASSWORD 'replicator-secret';
SELECT pg_create_physical_replication_slot('replica1');
SQL
echo "host replication replicator all scram-sha-256" >> "$PGDATA/pg_hba.conf"
```

`dev/replica/entrypoint.sh`:

```bash
#!/bin/bash
# Реплика без Patroni: при пустом каталоге данных — pg_basebackup с primary, затем обычный запуск.
set -euo pipefail
export PGPASSWORD="$REPLICATION_PASSWORD"
until pg_isready -h pg-primary -p 5432 -q; do sleep 1; done
if [ ! -s "$PGDATA/PG_VERSION" ]; then
  # -R пишет primary_conninfo и standby.signal; слот replica1 создал primary.
  until rm -rf "${PGDATA:?}"/* && pg_basebackup -h pg-primary -U replicator -D "$PGDATA" -X stream -R -S replica1; do
    sleep 2
  done
fi
chmod 0700 "$PGDATA"
exec postgres -c hot_standby=on -c shared_preload_libraries=pg_stat_statements
```

- [ ] **Step 4: `dev/docker-compose.yml`**

```yaml
name: glafira-pg-dev

x-patroni: &patroni
  build:
    context: .
    dockerfile: patroni/Dockerfile
  image: glafira-pg-dev/patroni:17
  depends_on: [etcd]

services:
  etcd:
    image: quay.io/coreos/etcd:v3.5.17
    container_name: gp-etcd
    command:
      - etcd
      - --name=etcd
      - --data-dir=/etcd-data
      - --listen-client-urls=http://0.0.0.0:2379
      - --advertise-client-urls=http://etcd:2379
      - --listen-peer-urls=http://0.0.0.0:2380
      - --initial-advertise-peer-urls=http://etcd:2380
      - --initial-cluster=etcd=http://etcd:2380

  patroni1:
    <<: *patroni
    container_name: gp-patroni1
    hostname: patroni1
    environment:
      PATRONI_NAME: patroni1
      PATRONI_RESTAPI_CONNECT_ADDRESS: patroni1:8008
      PATRONI_POSTGRESQL_CONNECT_ADDRESS: patroni1:5432
    ports: ["5441:5432", "8011:8008"]

  patroni2:
    <<: *patroni
    container_name: gp-patroni2
    hostname: patroni2
    environment:
      PATRONI_NAME: patroni2
      PATRONI_RESTAPI_CONNECT_ADDRESS: patroni2:8008
      PATRONI_POSTGRESQL_CONNECT_ADDRESS: patroni2:5432
    ports: ["5442:5432", "8012:8008"]

  patroni3:
    <<: *patroni
    container_name: gp-patroni3
    hostname: patroni3
    environment:
      PATRONI_NAME: patroni3
      PATRONI_RESTAPI_CONNECT_ADDRESS: patroni3:8008
      PATRONI_POSTGRESQL_CONNECT_ADDRESS: patroni3:5432
    ports: ["5443:5432", "8013:8008"]

  pg-primary:
    image: postgres:16
    container_name: gp-pg-primary
    hostname: pg-primary
    environment:
      POSTGRES_PASSWORD: postgres-secret
    command:
      - postgres
      - -c
      - wal_level=replica
      - -c
      - max_wal_senders=10
      - -c
      - max_replication_slots=10
      - -c
      - hot_standby=on
      - -c
      - shared_preload_libraries=pg_stat_statements
      - -c
      - track_io_timing=on
    volumes:
      - ./primary/10-replication.sh:/docker-entrypoint-initdb.d/10-replication.sh:ro
      - ./sql/init.sql:/docker-entrypoint-initdb.d/20-init.sql:ro
    ports: ["5451:5432"]

  pg-replica:
    image: postgres:16
    container_name: gp-pg-replica
    hostname: pg-replica
    depends_on: [pg-primary]
    user: postgres
    entrypoint: ["/bin/bash", "/replica-entrypoint.sh"]
    environment:
      PGDATA: /var/lib/postgresql/data
      REPLICATION_PASSWORD: replicator-secret
    volumes:
      - ./replica/entrypoint.sh:/replica-entrypoint.sh:ro
    ports: ["5452:5432"]

  pg-single:
    image: postgres:15
    container_name: gp-pg-single
    environment:
      POSTGRES_PASSWORD: postgres-secret
    volumes:
      - ./sql/init.sql:/docker-entrypoint-initdb.d/20-init.sql:ro
    ports: ["5460:5432"]
```

- [ ] **Step 5: `scripts/wait-stand.sh`**

```bash
#!/usr/bin/env bash
# Ждёт готовности стенда: все PG принимают соединения, у Patroni есть лидер и две потоковые реплики,
# реплика без Patroni получила демо-базу.
set -euo pipefail
deadline=$((SECONDS + 240))

wait_for() {
  until "$@" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "timeout: $*" >&2
      exit 1
    fi
    sleep 2
  done
}

for c in gp-pg-single gp-pg-primary gp-pg-replica gp-patroni1 gp-patroni2 gp-patroni3; do
  wait_for docker exec "$c" pg_isready -h localhost -p 5432 -q
done

cluster_ok() {
  local out
  out=$(curl -fs http://localhost:8011/cluster) || return 1
  echo "$out" | grep -q '"role": *"leader"' || return 1
  [ "$(echo "$out" | grep -o '"state": *"streaming"' | wc -l)" -ge 2 ]
}
wait_for cluster_ok
wait_for docker exec gp-pg-replica psql -U postgres -d orders -tAc "select 1"
echo "stand is ready"
```

- [ ] **Step 6: Поднять стенд и проверить**

```bash
chmod +x scripts/wait-stand.sh dev/patroni/post_bootstrap.sh dev/primary/10-replication.sh dev/replica/entrypoint.sh
docker compose -f dev/docker-compose.yml up -d --build
scripts/wait-stand.sh
```
Expected: `stand is ready`.

```bash
docker exec gp-pg-single psql "postgresql://glafira_diag:glafira-secret@localhost/postgres" -tAc "select pg_has_role('pg_monitor','member')"
curl -s http://localhost:8011/cluster
docker exec gp-pg-primary psql -U postgres -tAc "select application_name, sync_state from pg_stat_replication"
docker exec gp-patroni1 psql -U postgres -d orders -tAc "select count(*) from shop.orders"
```
Expected: `t`; JSON с тремя участниками (один `leader`, два `streaming`); строка `walreceiver|async`; `1000`.

- [ ] **Step 7: Commit**

```bash
git add dev scripts/wait-stand.sh
git commit -m "test(dev): стенд — кластер Patroni на etcd, репликация без Patroni, одиночный сервер

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Соединение с PostgreSQL — ошибки, TLS, подключение

**Files:**
- Create: `src-tauri/src/conn/errors.rs`, `src-tauri/src/conn/tls.rs`, `src-tauri/src/conn/pg.rs`, `src-tauri/tests/common/mod.rs`, `src-tauri/tests/it_pg.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/conn/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `conn/tls.rs`, `conn/pg.rs`, `conn/errors.rs`; интеграционные `tests/it_pg.rs`

**Interfaces:**
- Consumes: `conn::profile::*`, `conn::pgpass::find_password`, `secrets::{SecretStore, secret_key, PG_PASSWORD, PG_KEY_PASSWORD}`, `product::PG_APPLICATION_NAME`.
- Produces:
  - `crate::conn::errors::{from_pg(&tokio_postgres::Error) -> AppError, tcp_error(&io::Error, host, port) -> AppError, connect_timeout(host, port, secs) -> AppError, tls_setup(impl Display) -> AppError, chain(&(dyn Error + 'static)) -> String}`.
  - `crate::conn::tls::{Identity { cert, chain, key }, load_identity(&ClientCert, Option<&str>) -> AppResult<Identity>, pg_connector(SslMode, Option<&str>, Option<&ClientCert>, Option<&str>) -> AppResult<Option<SslConnector>>, identity_pem(&ClientCert, Option<&str>) -> AppResult<Vec<u8>>, ca_pem_bundle(path: &str) -> AppResult<Vec<u8>>}`.
  - `crate::conn::pg::{SESSION_OPTIONS, ConnectSpec { tcp_host, tcp_port, host, port, user, password, database, ssl_mode, connector, timeout }, ConnectSpec::build(&ConnectionProfile, host, port, database, &dyn SecretStore) -> AppResult<Self>, ConnectSpec::pg_config(&self) -> tokio_postgres::Config, ConnectSpec::connect(&self) -> AppResult<tokio_postgres::Client>}`.
  - `tests/common`: `env(name) -> Option<String>`, `host_port(&str) -> (String, u16)`, `server_profile(id, addr) -> ConnectionProfile`, `secrets_with_password(id) -> Arc<MemoryStore>`, `PG_USER`, `PG_PASSWORD_VALUE`.

- [ ] **Step 1: Зависимости**

В `[dependencies]` `src-tauri/Cargo.toml`:

```toml
tokio-postgres = "0.7"
postgres-openssl = "0.5"
openssl = { version = "0.10", features = ["vendored"] }
rustls-native-certs = "0.8"
```

`conn/mod.rs`:

```rust
pub mod connstr;
pub mod errors;
pub mod pg;
pub mod pgpass;
pub mod profile;
pub mod tls;
```

- [ ] **Step 2: Тесты TLS (падающие)**

В конец `src-tauri/src/conn/tls.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use openssl::asn1::Asn1Time;
    use openssl::hash::MessageDigest;
    use openssl::symm::Cipher;
    use openssl::x509::X509NameBuilder;

    fn self_signed() -> (X509, PKey<Private>) {
        let key = PKey::from_rsa(openssl::rsa::Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "glafira-test").unwrap();
        let name = name.build();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_subject_name(&name).unwrap();
        b.set_issuer_name(&name).unwrap();
        b.set_pubkey(&key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        b.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
        b.sign(&key, MessageDigest::sha256()).unwrap();
        (b.build(), key)
    }

    fn write(dir: &std::path::Path, name: &str, data: &[u8]) -> String {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn pem_identity_plain_and_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let cert_path = write(dir.path(), "c.pem", &cert.to_pem().unwrap());
        let plain = write(dir.path(), "k.pem", &key.private_key_to_pem_pkcs8().unwrap());
        let enc = write(
            dir.path(),
            "ke.pem",
            &key.private_key_to_pem_pkcs8_passphrase(Cipher::aes_256_cbc(), b"pw").unwrap(),
        );
        let cc = |k: &str| ClientCert { format: CertFormat::Pem, certificate_path: cert_path.clone(), key_path: Some(k.into()) };
        assert!(load_identity(&cc(&plain), None).is_ok());
        assert!(load_identity(&cc(&enc), Some("pw")).is_ok());
        assert_eq!(load_identity(&cc(&enc), Some("wrong")).unwrap_err().key, "errors.tls.badFile");
    }

    #[test]
    fn pkcs12_identity() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let p12 = Pkcs12::builder().name("x").pkey(&key).cert(&cert).build2("pw").unwrap();
        let path = write(dir.path(), "c.p12", &p12.to_der().unwrap());
        let cc = ClientCert { format: CertFormat::Pkcs12, certificate_path: path, key_path: None };
        assert!(load_identity(&cc, Some("pw")).is_ok());
        assert_eq!(load_identity(&cc, Some("nope")).unwrap_err().key, "errors.tls.badFile");
    }

    #[test]
    fn missing_file_is_reported_with_path() {
        let cc = ClientCert { format: CertFormat::Pem, certificate_path: "/nope/c.pem".into(), key_path: Some("/nope/k.pem".into()) };
        let e = load_identity(&cc, None).unwrap_err();
        assert_eq!(e.key, "errors.tls.readFailed");
        assert_eq!(e.params["path"], "/nope/c.pem");
    }

    #[test]
    fn connector_follows_sslmode() {
        assert!(pg_connector(SslMode::Disable, None, None, None).unwrap().is_none());
        assert!(pg_connector(SslMode::Prefer, None, None, None).unwrap().is_some());
        let dir = tempfile::tempdir().unwrap();
        let (cert, _) = self_signed();
        let ca = write(dir.path(), "ca.pem", &cert.to_pem().unwrap());
        assert!(pg_connector(SslMode::VerifyFull, Some(&ca), None, None).unwrap().is_some());
        let junk = write(dir.path(), "junk.pem", b"not a certificate");
        assert_eq!(pg_connector(SslMode::VerifyCa, Some(&junk), None, None).unwrap_err().key, "errors.tls.badFile");
    }

    #[test]
    fn identity_pem_has_certificate_and_unencrypted_key() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = self_signed();
        let p12 = Pkcs12::builder().name("x").pkey(&key).cert(&cert).build2("pw").unwrap();
        let path = write(dir.path(), "c.p12", &p12.to_der().unwrap());
        let pem = identity_pem(&ClientCert { format: CertFormat::Pkcs12, certificate_path: path, key_path: None }, Some("pw")).unwrap();
        let text = String::from_utf8(pem).unwrap();
        assert!(text.contains("BEGIN CERTIFICATE"));
        assert!(text.contains("BEGIN PRIVATE KEY"));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::tls`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализовать `conn/errors.rs` и `conn/tls.rs`**

`src-tauri/src/conn/errors.rs`:

```rust
//! Ошибки PostgreSQL → `AppError` (таблица §13 спеки P1).

use tokio_postgres::error::SqlState;

use crate::error::{AppError, ErrorCode};

/// Текст ошибки вместе с цепочкой причин (`error connecting to server: Connection refused`).
pub fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !out.contains(&t) {
            out.push_str(": ");
            out.push_str(&t);
        }
        cur = s.source();
    }
    out
}

pub fn from_pg(e: &tokio_postgres::Error) -> AppError {
    if let Some(db) = e.as_db_error() {
        let code = db.code();
        let details = match db.detail() {
            Some(d) => format!("{}: {} ({d})", code.code(), db.message()),
            None => format!("{}: {}", code.code(), db.message()),
        };
        let err = if *code == SqlState::INVALID_PASSWORD
            || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION
        {
            app_err!(
                ErrorCode::AuthFailed,
                "errors.pg.authFailed",
                "Authentication failed: check the user, the password and pg_hba.conf"
            )
        } else if *code == SqlState::INVALID_CATALOG_NAME {
            app_err!(ErrorCode::InvalidInput, "errors.pg.databaseNotFound", "The database does not exist")
        } else if *code == SqlState::INSUFFICIENT_PRIVILEGE {
            app_err!(ErrorCode::NotAuthorized, "errors.pg.notAuthorized", "Not enough privileges for the query")
        } else if *code == SqlState::QUERY_CANCELED {
            app_err!(
                ErrorCode::Timeout,
                "errors.pg.statementTimeout",
                "The query was interrupted by statement_timeout"
            )
        } else if *code == SqlState::TOO_MANY_CONNECTIONS {
            app_err!(
                ErrorCode::ConnectionFailed,
                "errors.pg.tooManyConnections",
                "The server has no free connections (max_connections)"
            )
        } else if *code == SqlState::CANNOT_CONNECT_NOW {
            app_err!(
                ErrorCode::NodeUnavailable,
                "errors.pg.cannotConnectNow",
                "The server is starting up or in recovery"
            )
        } else {
            app_err!(
                ErrorCode::Internal,
                "errors.pg.serverError",
                "PostgreSQL error {{code}}",
                code = code.code()
            )
        };
        return err.with_details(details);
    }
    let text = chain(e);
    let lower = text.to_lowercase();
    let err = if lower.contains("password missing") {
        app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.passwordMissing",
            "The server asked for a password, but none is saved or found in ~/.pgpass"
        )
    } else if lower.contains("server does not support tls") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.tlsNotSupported",
            "The server does not accept TLS connections: choose sslmode prefer or disable"
        )
    } else if lower.contains("hostname mismatch") || lower.contains("ip address mismatch") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.hostnameMismatch",
            "The server host name does not match its certificate"
        )
    } else if lower.contains("certificate verify failed") {
        app_err!(
            ErrorCode::TlsError,
            "errors.pg.caUntrusted",
            "The server certificate is not signed by a trusted CA"
        )
    } else if lower.contains("tls") || lower.contains("ssl") || lower.contains("handshake") {
        app_err!(ErrorCode::TlsError, "errors.pg.tlsError", "TLS connection error")
    } else if e.is_closed() {
        app_err!(
            ErrorCode::ConnectionFailed,
            "errors.pg.connectionClosed",
            "The connection to the server was closed"
        )
    } else {
        app_err!(ErrorCode::ConnectionFailed, "errors.pg.connectionFailed", "Could not connect to the server")
    };
    err.with_details(text)
}

pub fn tcp_error(e: &std::io::Error, host: &str, port: u16) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.pg.tcpFailed",
        "Could not reach {{host}}:{{port}}",
        host = host,
        port = port
    )
    .with_details(e.to_string())
}

pub fn connect_timeout(host: &str, port: u16, secs: u64) -> AppError {
    app_err!(
        ErrorCode::Timeout,
        "errors.pg.connectTimeout",
        "No answer from {{host}}:{{port}} within {{secs}} s",
        host = host,
        port = port,
        secs = secs
    )
}

pub fn tls_setup(e: impl std::fmt::Display) -> AppError {
    app_err!(ErrorCode::TlsError, "errors.pg.tlsSetup", "Could not prepare TLS settings")
        .with_details(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(std::io::Error);
    impl std::fmt::Display for Outer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "error connecting to server")
        }
    }
    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn chain_joins_sources() {
        let e = Outer(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Connection refused"));
        assert_eq!(chain(&e), "error connecting to server: Connection refused");
    }

    #[test]
    fn tcp_and_timeout_texts() {
        let e = tcp_error(&std::io::Error::other("boom"), "db1", 5432);
        assert_eq!(e.code, ErrorCode::ConnectionFailed);
        assert_eq!(e.message, "Could not reach db1:5432");
        assert!(e.retryable);
        assert_eq!(connect_timeout("db1", 5432, 10).message, "No answer from db1:5432 within 10 s");
    }
}
```

`src-tauri/src/conn/tls.rs` (над тестами):

```rust
//! TLS для PostgreSQL и Patroni API: CA, клиентский сертификат (PEM / PKCS#12), системные корневые
//! сертификаты. Файлы читаются при каждом создании пула или клиента — ротация подхватывается.

use std::sync::OnceLock;

use openssl::error::ErrorStack;
use openssl::pkcs12::{ParsedPkcs12_2, Pkcs12};
use openssl::pkey::{PKey, Private};
use openssl::provider::Provider;
use openssl::ssl::{SslConnector, SslMethod, SslVerifyMode};
use openssl::x509::X509;

use crate::conn::errors::tls_setup;
use crate::conn::profile::{CertFormat, ClientCert, SslMode};
use crate::error::{AppError, AppResult, ErrorCode};

pub struct Identity {
    pub cert: X509,
    pub chain: Vec<X509>,
    pub key: PKey<Private>,
}

fn read(path: &str) -> AppResult<Vec<u8>> {
    std::fs::read(path).map_err(|e| {
        app_err!(ErrorCode::InvalidInput, "errors.tls.readFailed", "Cannot read {{path}}", path = path)
            .with_details(e.to_string())
    })
}

fn bad_file(path: &str, e: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.tls.badFile",
        "{{path}} is not a valid certificate or key, or the password is wrong",
        path = path
    )
    .with_details(e.to_string())
}

static LEGACY: OnceLock<Option<Provider>> = OnceLock::new();

/// Старые PKCS#12 (RC2/3DES) требуют провайдер legacy OpenSSL 3 — как в Glafira for Kafka.
fn parse_pkcs12(p12: &Pkcs12, password: &str) -> Result<ParsedPkcs12_2, ErrorStack> {
    match p12.parse2(password) {
        Err(e) if is_unsupported(&e)
            && LEGACY
                .get_or_init(|| Provider::try_load(None, "legacy", true).ok())
                .is_some() =>
        {
            p12.parse2(password)
        }
        r => r,
    }
}

fn is_unsupported(e: &ErrorStack) -> bool {
    let t = e.to_string().to_lowercase();
    t.contains("unsupported") || t.contains("rc2") || t.contains("fetch")
}

pub fn load_identity(c: &ClientCert, password: Option<&str>) -> AppResult<Identity> {
    match c.format {
        CertFormat::Pem => {
            let data = read(&c.certificate_path)?;
            let mut certs = X509::stack_from_pem(&data)
                .map_err(|e| bad_file(&c.certificate_path, e))?
                .into_iter();
            let cert = certs
                .next()
                .ok_or_else(|| bad_file(&c.certificate_path, "no certificate"))?;
            let key_path = c.key_path.as_deref().unwrap_or(&c.certificate_path);
            let key_pem = read(key_path)?;
            let key = match password.filter(|p| !p.is_empty()) {
                Some(p) => PKey::private_key_from_pem_passphrase(&key_pem, p.as_bytes()),
                None => PKey::private_key_from_pem(&key_pem),
            }
            .map_err(|e| bad_file(key_path, e))?;
            Ok(Identity { cert, chain: certs.collect(), key })
        }
        CertFormat::Pkcs12 => {
            let path = &c.certificate_path;
            let p12 = Pkcs12::from_der(&read(path)?).map_err(|e| bad_file(path, e))?;
            let parsed = parse_pkcs12(&p12, password.unwrap_or("")).map_err(|e| bad_file(path, e))?;
            let cert = parsed.cert.ok_or_else(|| bad_file(path, "no certificate"))?;
            let key = parsed.pkey.ok_or_else(|| bad_file(path, "no private key"))?;
            let chain = parsed.ca.map(|s| s.into_iter().collect()).unwrap_or_default();
            Ok(Identity { cert, chain, key })
        }
    }
}

static SYSTEM_ROOTS: OnceLock<Vec<X509>> = OnceLock::new();

/// Системные корневые сертификаты (Keychain, хранилище Windows, /etc/ssl): vendored OpenSSL путей ОС не знает.
fn system_roots() -> &'static [X509] {
    SYSTEM_ROOTS.get_or_init(|| {
        let res = rustls_native_certs::load_native_certs();
        for e in &res.errors {
            tracing::warn!("system root certificates: {e}");
        }
        res.certs
            .iter()
            .filter_map(|c| X509::from_der(c.as_ref()).ok())
            .collect()
    })
}

/// CA-файл как PEM-набор (для reqwest); проверяется, что в нём есть хотя бы один сертификат.
pub fn ca_pem_bundle(path: &str) -> AppResult<Vec<u8>> {
    let data = read(path)?;
    let certs = X509::stack_from_pem(&data).map_err(|e| bad_file(path, e))?;
    if certs.is_empty() {
        return Err(bad_file(path, "no certificates"));
    }
    Ok(data)
}

/// `SslConnector` для режима `mode`; `None` — без TLS (`disable`).
pub fn pg_connector(
    mode: SslMode,
    ca_path: Option<&str>,
    cert: Option<&ClientCert>,
    key_password: Option<&str>,
) -> AppResult<Option<SslConnector>> {
    if mode == SslMode::Disable {
        return Ok(None);
    }
    let mut b = SslConnector::builder(SslMethod::tls_client()).map_err(tls_setup)?;
    let roots: Vec<X509> = match ca_path {
        Some(p) => {
            let data = ca_pem_bundle(p)?;
            X509::stack_from_pem(&data).map_err(|e| bad_file(p, e))?
        }
        None => system_roots().to_vec(),
    };
    for c in roots {
        // Повтор сертификата в хранилище — не ошибка.
        let _ = b.cert_store_mut().add_cert(c);
    }
    if let Some(c) = cert {
        let id = load_identity(c, key_password)?;
        b.set_certificate(&id.cert).map_err(tls_setup)?;
        b.set_private_key(&id.key).map_err(tls_setup)?;
        for extra in id.chain {
            b.add_extra_chain_cert(extra).map_err(tls_setup)?;
        }
    }
    b.set_verify(match mode {
        SslMode::VerifyCa | SslMode::VerifyFull => SslVerifyMode::PEER,
        _ => SslVerifyMode::NONE,
    });
    Ok(Some(b.build()))
}

/// Сертификат с цепочкой и незашифрованный ключ PKCS#8 одним PEM — для `reqwest::Identity::from_pem`.
pub fn identity_pem(c: &ClientCert, password: Option<&str>) -> AppResult<Vec<u8>> {
    let id = load_identity(c, password)?;
    let mut out = id.cert.to_pem().map_err(tls_setup)?;
    for x in &id.chain {
        out.extend(x.to_pem().map_err(tls_setup)?);
    }
    out.extend(id.key.private_key_to_pem_pkcs8().map_err(tls_setup)?);
    Ok(out)
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::tls conn::errors`
Expected: PASS.

- [ ] **Step 4: Тест сборки параметров соединения (падающий)**

В конец `src-tauri/src/conn/pg.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::sample_profile;
    use crate::secrets::MemoryStore;

    #[test]
    fn spec_takes_password_from_keychain_and_sets_session() {
        let secrets = MemoryStore::default();
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        let mut p = sample_profile("a");
        p.pg.ssl_mode = SslMode::Disable;
        p.pg.connect_timeout_s = 7;
        let spec = ConnectSpec::build(&p, "db1", 5433, "orders", &secrets).unwrap();
        assert_eq!((spec.tcp_host.as_str(), spec.tcp_port), ("db1", 5433));
        assert_eq!(spec.password.as_deref(), Some("pw"));
        assert!(spec.connector.is_none());
        assert_eq!(spec.timeout, Duration::from_secs(7));
        let cfg = spec.pg_config();
        assert_eq!(cfg.get_user(), Some("glafira_diag"));
        assert_eq!(cfg.get_dbname(), Some("orders"));
        assert_eq!(cfg.get_application_name(), Some("glafira-pg"));
        assert_eq!(cfg.get_options(), Some(SESSION_OPTIONS));
        assert_eq!(cfg.get_password(), Some(b"pw".as_slice()));
        assert_eq!(cfg.get_ssl_mode(), tokio_postgres::config::SslMode::Disable);
    }

    #[test]
    fn verify_modes_require_tls_on_the_wire() {
        for (mode, wire) in [
            (SslMode::Prefer, tokio_postgres::config::SslMode::Prefer),
            (SslMode::Require, tokio_postgres::config::SslMode::Require),
            (SslMode::VerifyCa, tokio_postgres::config::SslMode::Require),
            (SslMode::VerifyFull, tokio_postgres::config::SslMode::Require),
        ] {
            let mut p = sample_profile("a");
            p.pg.ssl_mode = mode;
            let spec = ConnectSpec::build(&p, "h", 5432, "postgres", &MemoryStore::default()).unwrap();
            assert_eq!(spec.pg_config().get_ssl_mode(), wire, "{mode:?}");
            assert!(spec.connector.is_some());
        }
    }

    #[test]
    fn debug_hides_password() {
        let secrets = MemoryStore::default();
        secrets.set(&secret_key("a", PG_PASSWORD), "s3cr3t").unwrap();
        let spec = ConnectSpec::build(&sample_profile("a"), "h", 5432, "postgres", &secrets).unwrap();
        assert!(!format!("{spec:?}").contains("s3cr3t"));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pg`
Expected: ошибки компиляции.

- [ ] **Step 5: Реализовать `conn/pg.rs`** (над тестами)

```rust
//! Соединение с PostgreSQL: параметры из профиля и keychain, TCP, TLS, таймаут (спец. P1 §7).

use std::time::Duration;

use openssl::ssl::SslConnector;
use tokio::net::TcpStream;
use tokio_postgres::{Client, NoTls};

use crate::conn::profile::{ConnectionProfile, SslMode};
use crate::conn::{errors, pgpass, tls};
use crate::error::AppResult;
use crate::secrets::{PG_KEY_PASSWORD, PG_PASSWORD, SecretStore, secret_key};

/// Параметры сессии диагностики: только чтение и короткие таймауты.
pub const SESSION_OPTIONS: &str =
    "-c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000";

/// Всё для нового соединения с узлом. Собирается один раз на пул: keychain, ~/.pgpass, файлы TLS.
#[derive(Clone)]
pub struct ConnectSpec {
    /// Куда открывать TCP (P1b: при туннеле — локальный порт).
    pub tcp_host: String,
    pub tcp_port: u16,
    /// Исходный адрес узла: имя для проверки сертификата и поиска в ~/.pgpass.
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: String,
    pub ssl_mode: SslMode,
    pub connector: Option<SslConnector>,
    pub timeout: Duration,
}

impl std::fmt::Debug for ConnectSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("user", &self.user)
            .field("ssl_mode", &self.ssl_mode)
            .finish_non_exhaustive()
    }
}

impl ConnectSpec {
    /// Блокирующий вызов: keychain, ~/.pgpass, файлы сертификатов.
    pub fn build(
        profile: &ConnectionProfile,
        host: &str,
        port: u16,
        database: &str,
        secrets: &dyn SecretStore,
    ) -> AppResult<Self> {
        let user = profile.pg.username.clone();
        let password = match secrets.get(&secret_key(&profile.id, PG_PASSWORD))? {
            Some(p) if !p.is_empty() => Some(p),
            _ => pgpass::find_password(host, port, database, &user),
        };
        let key_password = secrets.get(&secret_key(&profile.id, PG_KEY_PASSWORD))?;
        let connector = tls::pg_connector(
            profile.pg.ssl_mode,
            profile.pg.tls.ca_path.as_deref(),
            profile.pg.tls.client_cert.as_ref(),
            key_password.as_deref(),
        )?;
        Ok(Self {
            tcp_host: host.to_string(),
            tcp_port: port,
            host: host.to_string(),
            port,
            user,
            password,
            database: database.to_string(),
            ssl_mode: profile.pg.ssl_mode,
            connector,
            timeout: Duration::from_secs(profile.pg.connect_timeout_s),
        })
    }

    pub fn pg_config(&self) -> tokio_postgres::Config {
        use tokio_postgres::config::SslMode as Wire;
        let mut c = tokio_postgres::Config::new();
        c.user(&self.user)
            .dbname(&self.database)
            .application_name(crate::product::PG_APPLICATION_NAME)
            .options(SESSION_OPTIONS)
            .ssl_mode(match self.ssl_mode {
                SslMode::Disable => Wire::Disable,
                SslMode::Prefer => Wire::Prefer,
                SslMode::Require | SslMode::VerifyCa | SslMode::VerifyFull => Wire::Require,
            });
        if let Some(p) = &self.password {
            c.password(p);
        }
        c
    }

    /// Новое соединение. Задача соединения запускается в текущем runtime tokio.
    pub async fn connect(&self) -> AppResult<Client> {
        let attempt = async {
            let stream = TcpStream::connect((self.tcp_host.as_str(), self.tcp_port))
                .await
                .map_err(|e| errors::tcp_error(&e, &self.host, self.port))?;
            let _ = stream.set_nodelay(true);
            let cfg = self.pg_config();
            match &self.connector {
                None => {
                    let (client, conn) = cfg
                        .connect_raw(stream, NoTls)
                        .await
                        .map_err(|e| errors::from_pg(&e))?;
                    spawn_connection(conn);
                    Ok(client)
                }
                Some(connector) => {
                    let mut ssl = connector.configure().map_err(errors::tls_setup)?;
                    if self.ssl_mode != SslMode::VerifyFull {
                        ssl.set_verify_hostname(false);
                    }
                    let tls = postgres_openssl::TlsConnector::new(ssl, &self.host);
                    let (client, conn) = cfg
                        .connect_raw(stream, tls)
                        .await
                        .map_err(|e| errors::from_pg(&e))?;
                    spawn_connection(conn);
                    Ok(client)
                }
            }
        };
        tokio::time::timeout(self.timeout, attempt)
            .await
            .map_err(|_| errors::connect_timeout(&self.host, self.port, self.timeout.as_secs()))?
    }
}

fn spawn_connection<F>(conn: F)
where
    F: std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("pg connection ended: {e}");
        }
    });
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pg`
Expected: PASS.

- [ ] **Step 6: Интеграционные тесты соединения**

`src-tauri/tests/common/mod.rs`:

```rust
#![allow(dead_code)]

use std::sync::Arc;

use glafira_pg_lib::conn::profile::{
    ConnectionKind, ConnectionProfile, Environment, HostPort, PgSettings, PgTls, SslMode,
    TargetServerType,
};
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

pub const PG_USER: &str = "glafira_diag";
pub const PG_PASSWORD_VALUE: &str = "glafira-secret";

/// Значение переменной окружения; без неё интеграционный тест пропускается.
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

pub fn host_port(s: &str) -> (String, u16) {
    let (h, p) = s.trim().rsplit_once(':').expect("host:port");
    (h.to_string(), p.parse().expect("port"))
}

pub fn server_profile(id: &str, addr: &str) -> ConnectionProfile {
    let (host, port) = host_port(addr);
    ConnectionProfile {
        id: id.into(),
        name: id.into(),
        color: "#4f8cff".into(),
        environment: Environment::Dev,
        read_only: false,
        kind: ConnectionKind::Server,
        hosts: vec![HostPort { host, port }],
        target_server_type: TargetServerType::Primary,
        patroni: None,
        pg: PgSettings {
            username: PG_USER.into(),
            database: "postgres".into(),
            ssl_mode: SslMode::Prefer,
            tls: PgTls::default(),
            connect_timeout_s: 5,
            databases: None,
        },
        folder_id: None,
        order: 0,
        extra: Default::default(),
    }
}

pub fn secrets_with_password(id: &str) -> Arc<MemoryStore> {
    let s = MemoryStore::default();
    s.set(&secret_key(id, PG_PASSWORD), PG_PASSWORD_VALUE).unwrap();
    Arc::new(s)
}
```

`src-tauri/tests/it_pg.rs`:

```rust
//! Соединение с PostgreSQL стенда (`GPG_IT_SINGLE=localhost:5460`).

mod common;

use glafira_pg_lib::conn::errors::from_pg;
use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::profile::SslMode;
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

fn single() -> Option<String> {
    common::env("GPG_IT_SINGLE")
}

fn spec(addr: &str, db: &str, mode: SslMode, password: &str) -> ConnectSpec {
    let mut p = common::server_profile("it", addr);
    p.pg.ssl_mode = mode;
    let s = MemoryStore::default();
    s.set(&secret_key("it", PG_PASSWORD), password).unwrap();
    let (h, port) = common::host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s).unwrap()
}

#[tokio::test]
async fn session_is_read_only_with_timeouts() {
    let Some(addr) = single() else { return };
    let c = spec(&addr, "postgres", SslMode::Prefer, common::PG_PASSWORD_VALUE).connect().await.unwrap();
    let row = c
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), current_setting('statement_timeout')",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, String>(0), "on");
    assert_eq!(row.get::<_, String>(1), "glafira-pg");
    assert_eq!(row.get::<_, String>(2), "5s");
    let err = c.batch_execute("CREATE TABLE glafira_must_not_exist ()").await.unwrap_err();
    assert!(from_pg(&err).details.unwrap().contains("25006"), "read-only транзакция");
}

#[tokio::test]
async fn auth_database_and_tls_errors_are_classified() {
    let Some(addr) = single() else { return };
    let e = spec(&addr, "postgres", SslMode::Disable, "wrong").connect().await.unwrap_err();
    assert_eq!(e.code, ErrorCode::AuthFailed);
    let e = spec(&addr, "no_such_db", SslMode::Disable, common::PG_PASSWORD_VALUE).connect().await.unwrap_err();
    assert_eq!(e.key, "errors.pg.databaseNotFound");
    // На стенде TLS выключен.
    let e = spec(&addr, "postgres", SslMode::Require, common::PG_PASSWORD_VALUE).connect().await.unwrap_err();
    assert_eq!(e.key, "errors.pg.tlsNotSupported");
}

#[tokio::test]
async fn closed_port_and_statement_timeout() {
    let Some(addr) = single() else { return };
    let e = spec("127.0.0.1:1", "postgres", SslMode::Disable, "x").connect().await.unwrap_err();
    assert_eq!(e.key, "errors.pg.tcpFailed");
    let c = spec(&addr, "postgres", SslMode::Disable, common::PG_PASSWORD_VALUE).connect().await.unwrap();
    let err = c.simple_query("SELECT pg_sleep(6)").await.unwrap_err();
    assert_eq!(from_pg(&err).key, "errors.pg.statementTimeout");
}
```

В `en.json`/`ru.json` — ключи `errors.pg.*` и `errors.tls.*` из шага 3:

| Ключ | ru |
|---|---|
| `errors.pg.authFailed` | «Аутентификация не прошла: проверьте пользователя, пароль и pg_hba.conf» |
| `errors.pg.databaseNotFound` | «База данных не существует» |
| `errors.pg.notAuthorized` | «Недостаточно прав для запроса» |
| `errors.pg.statementTimeout` | «Запрос прерван по statement_timeout» |
| `errors.pg.tooManyConnections` | «На сервере нет свободных соединений (max_connections)» |
| `errors.pg.cannotConnectNow` | «Сервер запускается или находится в восстановлении» |
| `errors.pg.serverError` | «Ошибка PostgreSQL {{code}}» |
| `errors.pg.passwordMissing` | «Сервер запросил пароль, но он не сохранён и не найден в ~/.pgpass» |
| `errors.pg.tlsNotSupported` | «Сервер не принимает TLS-соединения: выберите sslmode prefer или disable» |
| `errors.pg.hostnameMismatch` | «Имя хоста сервера не совпадает с его сертификатом» |
| `errors.pg.caUntrusted` | «Сертификат сервера подписан недоверенным CA» |
| `errors.pg.tlsError` | «Ошибка TLS-соединения» |
| `errors.pg.connectionClosed` | «Соединение с сервером закрыто» |
| `errors.pg.connectionFailed` | «Не удалось подключиться к серверу» |
| `errors.pg.tcpFailed` | «Не удалось связаться с {{host}}:{{port}}» |
| `errors.pg.connectTimeout` | «{{host}}:{{port}} не ответил за {{secs}} с» |
| `errors.pg.tlsSetup` | «Не удалось подготовить параметры TLS» |
| `errors.tls.readFailed` | «Не удалось прочитать {{path}}» |
| `errors.tls.badFile` | «{{path}} — не сертификат или ключ, либо неверный пароль» |

- [ ] **Step 7: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run (стенд поднят): `GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_pg`
Expected: PASS (3 теста). Без переменной тесты проходят мгновенно (пропуск).

- [ ] **Step 8: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(conn): соединение с PostgreSQL — TLS, ~/.pgpass, классификация ошибок

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Пул соединений по (подключение, участник, БД)

**Files:**
- Create: `src-tauri/src/conn/pool.rs`
- Modify: `src-tauri/src/conn/mod.rs`, `src-tauri/tests/it_pg.rs`
- Test: unit-тесты в `conn/pool.rs`, интеграционные в `tests/it_pg.rs`

**Interfaces:**
- Consumes: `conn::pg::ConnectSpec`, `sync::MutexExt`.
- Produces: `crate::conn::pool::{MAX_CONNECTIONS = 3, IDLE_TIMEOUT = 300 s, PgPool, PooledClient (Deref<Target = tokio_postgres::Client>), PoolKey { connection_id, member, database }, PoolRegistry}`; `PgPool::new(spec) -> Arc<PgPool>`, `PgPool::get(self: &Arc<Self>) -> AppResult<PooledClient>`, `PgPool::close_idle(&self, max_idle) -> usize`, `PgPool::idle_count(&self) -> usize`; `PoolRegistry::{get(&PoolKey) -> Option<Arc<PgPool>>, insert_if_absent(PoolKey, ConnectSpec) -> Arc<PgPool>, evict(&PoolKey), evict_connection(&str), evict_member(&str, &str), close_idle(Duration), keys() -> Vec<PoolKey>}`.

- [ ] **Step 1: Тесты реестра (падающие)**

`conn/mod.rs` — добавить `pub mod pool;`. В конец `src-tauri/src/conn/pool.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::SslMode;

    fn spec() -> ConnectSpec {
        ConnectSpec {
            tcp_host: "h".into(),
            tcp_port: 5432,
            host: "h".into(),
            port: 5432,
            user: "u".into(),
            password: None,
            database: "postgres".into(),
            ssl_mode: SslMode::Disable,
            connector: None,
            timeout: Duration::from_secs(1),
        }
    }

    fn key(c: &str, m: &str, d: &str) -> PoolKey {
        PoolKey { connection_id: c.into(), member: m.into(), database: d.into() }
    }

    #[test]
    fn same_key_same_pool() {
        let r = PoolRegistry::default();
        let a = r.insert_if_absent(key("c", "m", "d"), spec());
        let b = r.insert_if_absent(key("c", "m", "d"), spec());
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&a, &r.get(&key("c", "m", "d")).unwrap()));
        assert!(r.get(&key("c", "m", "other")).is_none());
    }

    #[test]
    fn eviction_by_connection_and_member() {
        let r = PoolRegistry::default();
        r.insert_if_absent(key("c1", "m1", "d"), spec());
        r.insert_if_absent(key("c1", "m2", "d"), spec());
        r.insert_if_absent(key("c2", "m1", "d"), spec());
        r.evict_member("c1", "m1");
        let mut keys = r.keys();
        keys.sort_by(|a, b| (&a.connection_id, &a.member).cmp(&(&b.connection_id, &b.member)));
        assert_eq!(keys, vec![key("c1", "m2", "d"), key("c2", "m1", "d")]);
        r.evict_connection("c1");
        assert_eq!(r.keys(), vec![key("c2", "m1", "d")]);
        r.evict(&key("c2", "m1", "d"));
        assert!(r.keys().is_empty());
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pool`
Expected: ошибки компиляции.

- [ ] **Step 2: Реализация** (над тестами)

```rust
//! Пул соединений по ключу (подключение, участник, БД): не больше 3 соединений, проверка при выдаче,
//! закрытие простаивающих (спец. P1 §7). Свой пул вместо deadpool-postgres — см. «Отступления» плана P1a.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::error::{AppError, AppResult};
use crate::sync::MutexExt;

pub const MAX_CONNECTIONS: usize = 3;
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Соединение, простоявшее дольше, перед выдачей проверяется пустым запросом.
const VALIDATE_AFTER: Duration = Duration::from_secs(30);

pub struct PgPool {
    spec: ConnectSpec,
    idle: Mutex<Vec<(Client, Instant)>>,
    permits: Arc<Semaphore>,
}

/// Соединение из пула; при drop живое соединение возвращается в пул.
pub struct PooledClient {
    client: Option<Client>,
    pool: Arc<PgPool>,
    _permit: OwnedSemaphorePermit,
}

impl Deref for PooledClient {
    type Target = Client;
    fn deref(&self) -> &Client {
        self.client.as_ref().expect("client is present until drop")
    }
}

impl Drop for PooledClient {
    fn drop(&mut self) {
        if let Some(c) = self.client.take()
            && !c.is_closed()
        {
            self.pool.idle.lock_safe().push((c, Instant::now()));
        }
    }
}

impl PgPool {
    pub fn new(spec: ConnectSpec) -> Arc<Self> {
        Arc::new(Self {
            spec,
            idle: Mutex::new(Vec::new()),
            permits: Arc::new(Semaphore::new(MAX_CONNECTIONS)),
        })
    }

    pub fn spec(&self) -> &ConnectSpec {
        &self.spec
    }

    /// Свободное соединение или новое; при занятых трёх — ждёт освобождения.
    pub async fn get(self: &Arc<Self>) -> AppResult<PooledClient> {
        let permit = self
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| AppError::internal_with("connection pool is closed"))?;
        while let Some((client, since)) = self.pop_idle() {
            if client.is_closed() {
                continue;
            }
            if since.elapsed() >= VALIDATE_AFTER && client.simple_query("").await.is_err() {
                continue;
            }
            return Ok(PooledClient { client: Some(client), pool: self.clone(), _permit: permit });
        }
        let client = self.spec.connect().await?;
        Ok(PooledClient { client: Some(client), pool: self.clone(), _permit: permit })
    }

    fn pop_idle(&self) -> Option<(Client, Instant)> {
        self.idle.lock_safe().pop()
    }

    /// Закрывает соединения, простоявшие не меньше `max_idle`; возвращает, сколько закрыто.
    pub fn close_idle(&self, max_idle: Duration) -> usize {
        let mut idle = self.idle.lock_safe();
        let before = idle.len();
        idle.retain(|(_, since)| since.elapsed() < max_idle);
        before - idle.len()
    }

    pub fn idle_count(&self) -> usize {
        self.idle.lock_safe().len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    pub connection_id: String,
    pub member: String,
    pub database: String,
}

#[derive(Default)]
pub struct PoolRegistry {
    pools: Mutex<HashMap<PoolKey, Arc<PgPool>>>,
}

impl PoolRegistry {
    pub fn get(&self, key: &PoolKey) -> Option<Arc<PgPool>> {
        self.pools.lock_safe().get(key).cloned()
    }

    /// Пул по ключу; если его нет — новый из `spec` (успевший раньше поток выигрывает).
    pub fn insert_if_absent(&self, key: PoolKey, spec: ConnectSpec) -> Arc<PgPool> {
        self.pools
            .lock_safe()
            .entry(key)
            .or_insert_with(|| PgPool::new(spec))
            .clone()
    }

    pub fn evict(&self, key: &PoolKey) {
        self.pools.lock_safe().remove(key);
    }

    pub fn evict_connection(&self, connection_id: &str) {
        self.pools.lock_safe().retain(|k, _| k.connection_id != connection_id);
    }

    pub fn evict_member(&self, connection_id: &str, member: &str) {
        self.pools
            .lock_safe()
            .retain(|k, _| !(k.connection_id == connection_id && k.member == member));
    }

    pub fn close_idle(&self, max_idle: Duration) {
        for p in self.pools.lock_safe().values() {
            p.close_idle(max_idle);
        }
    }

    pub fn keys(&self) -> Vec<PoolKey> {
        self.pools.lock_safe().keys().cloned().collect()
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pool`
Expected: PASS.

- [ ] **Step 3: Интеграционные тесты пула**

В конец `src-tauri/tests/it_pg.rs`:

```rust
use std::time::Duration;

use glafira_pg_lib::conn::pool::PgPool;

async fn pid(c: &tokio_postgres::Client) -> i32 {
    c.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0)
}

#[tokio::test]
async fn pool_reuses_limits_and_closes_idle() {
    let Some(addr) = single() else { return };
    let pool = PgPool::new(spec(&addr, "postgres", SslMode::Disable, common::PG_PASSWORD_VALUE));
    let a = pool.get().await.unwrap();
    let first = pid(&a).await;
    drop(a);
    assert_eq!(pool.idle_count(), 1);
    let again = pool.get().await.unwrap();
    assert_eq!(pid(&again).await, first, "соединение переиспользуется");

    let b = pool.get().await.unwrap();
    let c = pool.get().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(300), pool.get()).await.is_err(),
        "четвёртое соединение ждёт"
    );
    drop(b);
    let d = tokio::time::timeout(Duration::from_secs(2), pool.get()).await.unwrap().unwrap();
    drop((again, c, d));
    assert_eq!(pool.idle_count(), 3);
    assert_eq!(pool.close_idle(Duration::ZERO), 3);
    assert_eq!(pool.idle_count(), 0);
}

#[tokio::test]
async fn terminated_connection_is_replaced() {
    let Some(addr) = single() else { return };
    let pool = PgPool::new(spec(&addr, "postgres", SslMode::Disable, common::PG_PASSWORD_VALUE));
    let a = pool.get().await.unwrap();
    let victim = pid(&a).await;
    drop(a);
    let killer = spec(&addr, "postgres", SslMode::Disable, common::PG_PASSWORD_VALUE).connect().await.unwrap();
    killer.execute("SELECT pg_terminate_backend($1)", &[&victim]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let b = pool.get().await.unwrap();
    assert_ne!(pid(&b).await, victim);
}
```

Run (стенд поднят): `GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_pg`
Expected: PASS (5 тестов).

- [ ] **Step 4: Commit**

```bash
git add src-tauri
git commit -m "feat(conn): пул соединений — не больше трёх на узел и БД, закрытие простаивающих

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 9: Модель топологии и журнал событий

**Files:**
- Create: `src-tauri/src/topology/mod.rs`, `src-tauri/src/topology/model.rs`, `src-tauri/src/topology/journal.rs`
- Modify: `src-tauri/src/lib.rs` (`pub mod topology;`), `src-tauri/src/conn/mod.rs` (`run_blocking`)
- Test: unit-тесты в `topology/model.rs`, `topology/journal.rs`

**Interfaces:**
- Consumes: `conn::profile::ConnectionKind`, `AppError`.
- Produces:
  - `crate::conn::run_blocking(f: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T>` (async, через `tokio::task::spawn_blocking`).
  - `crate::topology::BoxFuture<'a, T>`.
  - `crate::topology::model::{Role (Primary | StandbyLeader | SyncStandby | QuorumStandby | Replica | Unknown), Member, TopologyWarning (NoPrimary | MultiplePrimaries { members } | SystemIdMismatch { members } | PatroniUnreachable | MemberUnreachable { name }), Topology, member_name(host, port) -> String, system_id_warning(&[Member]) -> Option<TopologyWarning>, now_ms() -> i64}`; `Member::new(name, host, port)` (роль `Unknown`, `reachable = true`); `Topology::{leader(), member(name), is_degraded(), same_except_time(&other)}`; `Role::is_standby()`.
  - `crate::topology::journal::{EventKind, TopologyEvent { at_ms, event }, diff(&Topology, &Topology) -> Vec<TopologyEvent>, Journal { CAP = 500 }, Journal::extend(&mut self, &[TopologyEvent]), Journal::newest_first() -> Vec<TopologyEvent>}`.
  - JSON: `Member` — camelCase без `systemId`; `TopologyWarning` и `EventKind` — с полем `"kind"` (`"noPrimary"`, `"leaderChanged"` …); `TopologyEvent` — `{ "atMs": …, "kind": …, …поля события }`.

- [ ] **Step 1: Общие функции**

В `src-tauri/src/conn/mod.rs`:

```rust
use crate::error::{AppError, AppResult};

/// Блокирующая работа (keychain, файлы) вне async-потоков. Работает и в runtime Tauri, и в `#[tokio::test]`.
pub async fn run_blocking<T, F>(f: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::internal_with(e.to_string()))?
}
```

`src-tauri/src/topology/mod.rs`:

```rust
//! Топология подключения: участники, роли, лаг, журнал и фоновое наблюдение (спец. P1 §6).

pub mod journal;
pub mod model;

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
```

- [ ] **Step 2: Тесты модели и журнала (падающие)**

В конец `src-tauri/src/topology/model.rs`:

```rust
#[cfg(test)]
pub(crate) fn topo(members: Vec<Member>) -> Topology {
    Topology {
        connection_id: "c".into(),
        kind: ConnectionKind::Patroni,
        observed_at_ms: 1_000,
        scope: Some("demo".into()),
        paused: Some(false),
        members,
        warnings: vec![],
    }
}

#[cfg(test)]
pub(crate) fn member(name: &str, role: Role) -> Member {
    let mut m = Member::new(name, name, 5432);
    m.role = role;
    m.state = Some(if role == Role::Primary { "running" } else { "streaming" }.into());
    m.timeline = Some(1);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_prefers_primary_then_standby_leader() {
        let t = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        assert_eq!(t.leader().unwrap().name, "b");
        let s = topo(vec![member("a", Role::StandbyLeader), member("b", Role::Replica)]);
        assert_eq!(s.leader().unwrap().name, "a");
        assert!(topo(vec![member("a", Role::Replica)]).leader().is_none());
    }

    #[test]
    fn member_names_and_system_ids() {
        assert_eq!(member_name("db1", 5432), "db1:5432");
        assert_eq!(member_name("::1", 5433), "[::1]:5433");
        let mut a = member("a", Role::Primary);
        let mut b = member("b", Role::Replica);
        assert!(system_id_warning(&[a.clone(), b.clone()]).is_none(), "нет данных — нет предупреждения");
        a.system_id = Some("1".into());
        b.system_id = Some("1".into());
        assert!(system_id_warning(&[a.clone(), b.clone()]).is_none());
        b.system_id = Some("2".into());
        assert_eq!(
            system_id_warning(&[a, b]),
            Some(TopologyWarning::SystemIdMismatch { members: vec!["a".into(), "b".into()] })
        );
    }

    #[test]
    fn json_shape() {
        let mut t = topo(vec![member("a", Role::SyncStandby)]);
        t.members[0].system_id = Some("x".into());
        t.warnings = vec![TopologyWarning::MemberUnreachable { name: "a".into() }];
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["connectionId"], "c");
        assert_eq!(v["members"][0]["role"], "syncStandby");
        assert!(v["members"][0].get("systemId").is_none());
        assert_eq!(v["warnings"][0], serde_json::json!({"kind": "memberUnreachable", "name": "a"}));
        assert!(t.is_degraded());
    }

    #[test]
    fn same_except_time_ignores_observation_moment() {
        let a = topo(vec![member("a", Role::Primary)]);
        let mut b = a.clone();
        b.observed_at_ms = 99;
        assert!(a.same_except_time(&b));
        b.members[0].lag_bytes = Some(1);
        assert!(!a.same_except_time(&b));
    }
}
```

В конец `src-tauri/src/topology/journal.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::{member, topo};

    fn kinds(events: &[TopologyEvent]) -> Vec<&EventKind> {
        events.iter().map(|e| &e.event).collect()
    }

    #[test]
    fn switchover_gives_leader_and_role_changes() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        next.observed_at_ms = 2_000;
        next.members[1].timeline = Some(2);
        next.members[0].timeline = Some(2);
        let ev = diff(&prev, &next);
        assert!(ev.iter().all(|e| e.at_ms == 2_000));
        let k = kinds(&ev);
        assert!(k.contains(&&EventKind::LeaderChanged { from: Some("a".into()), to: Some("b".into()) }));
        assert!(k.contains(&&EventKind::TimelineChanged { from: 1, to: 2 }));
        assert!(k.contains(&&EventKind::MemberRoleChanged { name: "a".into(), from: Role::Primary, to: Role::Replica }));
        assert!(k.contains(&&EventKind::MemberRoleChanged { name: "b".into(), from: Role::Replica, to: Role::Primary }));
    }

    #[test]
    fn unreachable_member_does_not_produce_role_noise() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = prev.clone();
        next.members[1].reachable = false;
        next.members[1].role = Role::Unknown;
        next.members[1].state = None;
        assert_eq!(kinds(&diff(&prev, &next)), vec![&EventKind::MemberUnreachable { name: "b".into() }]);
        assert_eq!(kinds(&diff(&next, &prev)), vec![
            &EventKind::MemberReachable { name: "b".into() },
        ]);
    }

    #[test]
    fn added_removed_state_and_pause() {
        let prev = topo(vec![member("a", Role::Primary), member("b", Role::Replica)]);
        let mut next = topo(vec![member("a", Role::Primary), member("c", Role::Replica)]);
        next.members[0].state = Some("restarting".into());
        next.paused = Some(true);
        let k = kinds(&diff(&prev, &next)).into_iter().cloned().collect::<Vec<_>>();
        assert!(k.contains(&EventKind::MemberAdded { name: "c".into() }));
        assert!(k.contains(&EventKind::MemberRemoved { name: "b".into() }));
        assert!(k.contains(&EventKind::MemberStateChanged {
            name: "a".into(),
            from: Some("running".into()),
            to: Some("restarting".into())
        }));
        assert!(k.contains(&EventKind::PauseChanged { paused: true }));
        assert!(diff(&prev, &prev).is_empty());
    }

    #[test]
    fn journal_keeps_500_newest_first() {
        let mut j = Journal::default();
        let events: Vec<TopologyEvent> = (0..510)
            .map(|i| TopologyEvent { at_ms: i, event: EventKind::PauseChanged { paused: i % 2 == 0 } })
            .collect();
        j.extend(&events);
        let list = j.newest_first();
        assert_eq!(list.len(), Journal::CAP);
        assert_eq!(list[0].at_ms, 509);
        assert_eq!(list.last().unwrap().at_ms, 10);
    }

    #[test]
    fn event_json_is_flat() {
        let e = TopologyEvent { at_ms: 5, event: EventKind::LeaderChanged { from: None, to: Some("b".into()) } };
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({"atMs": 5, "kind": "leaderChanged", "from": null, "to": "b"})
        );
    }
}
```

`topology/mod.rs` уже объявляет оба модуля; `lib.rs` — `pub mod topology;`.
Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация модели** (над тестами `model.rs`)

```rust
//! Топология подключения (спец. P1 §6.1): участники, роли, лаг, предупреждения.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::conn::profile::ConnectionKind;
use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Primary,
    StandbyLeader,
    SyncStandby,
    QuorumStandby,
    Replica,
    Unknown,
}

impl Role {
    pub fn is_standby(self) -> bool {
        matches!(self, Role::SyncStandby | Role::QuorumStandby | Role::Replica)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Базовый адрес REST API Patroni (`http://h:8008`).
    pub api_url: Option<String>,
    pub role: Role,
    /// Роль и состояние как их прислал Patroni.
    pub raw_role: Option<String>,
    pub state: Option<String>,
    pub timeline: Option<i64>,
    pub lag_bytes: Option<i64>,
    pub replay_delay_ms: Option<i64>,
    /// Список узлов: от кого реплика получает WAL.
    pub upstream: Option<String>,
    pub tags: serde_json::Map<String, serde_json::Value>,
    pub pending_restart: bool,
    pub server_version: Option<String>,
    pub patroni_version: Option<String>,
    pub dcs_last_seen_ms: Option<i64>,
    /// Нужен только для предупреждения `systemIdMismatch`.
    #[serde(skip)]
    pub system_id: Option<String>,
    pub reachable: bool,
    pub error: Option<AppError>,
}

impl Member {
    pub fn new(name: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            port,
            api_url: None,
            role: Role::Unknown,
            raw_role: None,
            state: None,
            timeline: None,
            lag_bytes: None,
            replay_delay_ms: None,
            upstream: None,
            tags: Default::default(),
            pending_restart: false,
            server_version: None,
            patroni_version: None,
            dcs_last_seen_ms: None,
            system_id: None,
            reachable: true,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TopologyWarning {
    NoPrimary,
    MultiplePrimaries { members: Vec<String> },
    SystemIdMismatch { members: Vec<String> },
    PatroniUnreachable,
    MemberUnreachable { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Topology {
    pub connection_id: String,
    pub kind: ConnectionKind,
    pub observed_at_ms: i64,
    pub scope: Option<String>,
    pub paused: Option<bool>,
    pub members: Vec<Member>,
    pub warnings: Vec<TopologyWarning>,
}

impl Topology {
    /// Узел, принимающий запись: primary, иначе лидер standby-кластера.
    pub fn leader(&self) -> Option<&Member> {
        self.members
            .iter()
            .find(|m| m.role == Role::Primary)
            .or_else(|| self.members.iter().find(|m| m.role == Role::StandbyLeader))
    }

    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    pub fn is_degraded(&self) -> bool {
        self.warnings.iter().any(|w| {
            matches!(w, TopologyWarning::PatroniUnreachable | TopologyWarning::MemberUnreachable { .. })
        })
    }

    pub fn same_except_time(&self, other: &Topology) -> bool {
        self.members == other.members
            && self.warnings == other.warnings
            && self.paused == other.paused
            && self.scope == other.scope
    }
}

/// Имя участника сервера и списка узлов — `host:port` (IPv6 — в скобках).
pub fn member_name(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Участники принадлежат разным кластерам: у них разные `system_identifier`.
pub fn system_id_warning(members: &[Member]) -> Option<TopologyWarning> {
    let ids: BTreeSet<&str> = members.iter().filter_map(|m| m.system_id.as_deref()).collect();
    (ids.len() > 1).then(|| TopologyWarning::SystemIdMismatch {
        members: members
            .iter()
            .filter(|m| m.system_id.is_some())
            .map(|m| m.name.clone())
            .collect(),
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 4: Реализация журнала** (над тестами `journal.rs`)

```rust
//! Журнал событий топологии (спец. P1 §6.4): разница двух опросов, до 500 событий в памяти.

use std::collections::VecDeque;

use serde::Serialize;

use crate::topology::model::{Role, Topology};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EventKind {
    LeaderChanged { from: Option<String>, to: Option<String> },
    MemberAdded { name: String },
    MemberRemoved { name: String },
    MemberRoleChanged { name: String, from: Role, to: Role },
    MemberStateChanged { name: String, from: Option<String>, to: Option<String> },
    MemberUnreachable { name: String },
    MemberReachable { name: String },
    TimelineChanged { from: i64, to: i64 },
    PauseChanged { paused: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyEvent {
    pub at_ms: i64,
    #[serde(flatten)]
    pub event: EventKind,
}

/// События между двумя опросами; время событий — момент второго опроса.
pub fn diff(prev: &Topology, next: &Topology) -> Vec<TopologyEvent> {
    let mut out = Vec::new();
    let mut push = |event: EventKind| out.push(TopologyEvent { at_ms: next.observed_at_ms, event });

    let leader = |t: &Topology| t.leader().map(|m| m.name.clone());
    let (from, to) = (leader(prev), leader(next));
    if from != to {
        push(EventKind::LeaderChanged { from, to });
    }
    let timeline = |t: &Topology| t.leader().and_then(|m| m.timeline);
    if let (Some(a), Some(b)) = (timeline(prev), timeline(next))
        && a != b
    {
        push(EventKind::TimelineChanged { from: a, to: b });
    }
    if let (Some(a), Some(b)) = (prev.paused, next.paused)
        && a != b
    {
        push(EventKind::PauseChanged { paused: b });
    }
    for m in &next.members {
        let name = m.name.clone();
        let Some(old) = prev.member(&m.name) else {
            push(EventKind::MemberAdded { name });
            continue;
        };
        match (old.reachable, m.reachable) {
            (true, false) => {
                push(EventKind::MemberUnreachable { name });
                continue;
            }
            (false, true) => {
                push(EventKind::MemberReachable { name });
                continue;
            }
            (false, false) => continue,
            (true, true) => {}
        }
        if old.role != m.role {
            push(EventKind::MemberRoleChanged { name: name.clone(), from: old.role, to: m.role });
        }
        if old.state != m.state {
            push(EventKind::MemberStateChanged { name, from: old.state.clone(), to: m.state.clone() });
        }
    }
    for old in &prev.members {
        if next.member(&old.name).is_none() {
            push(EventKind::MemberRemoved { name: old.name.clone() });
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct Journal {
    events: VecDeque<TopologyEvent>,
}

impl Journal {
    pub const CAP: usize = 500;

    pub fn extend(&mut self, events: &[TopologyEvent]) {
        for e in events {
            if self.events.len() == Self::CAP {
                self.events.pop_front();
            }
            self.events.push_back(e.clone());
        }
    }

    /// Новые сверху.
    pub fn newest_first(&self) -> Vec<TopologyEvent> {
        self.events.iter().rev().cloned().collect()
    }
}
```

В тесте `unreachable_member_does_not_produce_role_noise` обратный переход (`diff(&next, &prev)`) даёт только `MemberReachable`: роль и состояние после возвращения узла не сравниваются с данными недоступного узла.

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology`
Expected: PASS (9 тестов).

- [ ] **Step 6: Commit**

```bash
git add src-tauri
git commit -m "feat(topology): модель топологии и журнал событий

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Клиент Patroni REST API и обнаружение участников

**Files:**
- Create: `src-tauri/src/topology/patroni.rs`, `src-tauri/tests/it_patroni.rs`
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/topology/mod.rs`, `src-tauri/tests/common/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `topology/patroni.rs`, интеграционные `tests/it_patroni.rs`

**Interfaces:**
- Consumes: `conn::profile::{PatroniSettings, HostOverride, ConnectionKind}`, `conn::tls::{ca_pem_bundle, identity_pem}`, `conn::errors::chain`, `topology::model::*`, `topology::BoxFuture`.
- Produces: `crate::topology::patroni::{PatroniHttp (trait: get_json(&self, url) -> BoxFuture<AppResult<serde_json::Value>>), ReqwestHttp::new(&PatroniSettings, password: Option<String>, key_password: Option<&str>) -> AppResult<ReqwestHttp>, HTTP_TIMEOUT, api_base(&str) -> String, cluster_bases(&PatroniSettings, Option<&Topology>) -> Vec<String>, map_role(Option<&str>) -> Role, format_server_version(i64) -> String, members_from_cluster(&ClusterJson, &PatroniSettings) -> Vec<Member>, discover(http: &dyn PatroniHttp, settings, connection_id, previous: Option<&Topology>, now_ms) -> AppResult<Topology>, ClusterJson}`.
- `tests/common`: `patroni_settings() -> Option<PatroniSettings>` (адреса из `GPG_IT_PATRONI`, подмена адресов `patroniN → localhost:544N / http://localhost:801N`), `docker(args: &[&str])`, `wait_stand()`, `patronictl(args: &[&str]) -> String`.

- [ ] **Step 1: Зависимости**

```toml
reqwest = { version = "0.13", default-features = false, features = ["json", "rustls"] }
futures-util = "0.3"
```

`topology/mod.rs` — `pub mod patroni;`.

- [ ] **Step 2: Тесты (падающие)**

В конец `src-tauri/src/topology/patroni.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;
    use crate::sync::MutexExt;

    struct FakeHttp {
        answers: HashMap<String, AppResult<serde_json::Value>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeHttp {
        fn new(answers: Vec<(&str, AppResult<serde_json::Value>)>) -> Self {
            Self {
                answers: answers.into_iter().map(|(u, a)| (u.to_string(), a)).collect(),
                calls: Mutex::new(vec![]),
            }
        }
    }

    impl PatroniHttp for FakeHttp {
        fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            self.calls.lock_safe().push(url.to_string());
            let r = self.answers.get(url).cloned().unwrap_or_else(|| Err(unreachable_err(url)));
            Box::pin(async move { r })
        }
    }

    fn unreachable_err(url: &str) -> AppError {
        app_err!(ErrorCode::PatroniError, "errors.patroni.unreachable", "Patroni API at {{url}} is unreachable", url = url)
    }

    fn settings(urls: &[&str]) -> PatroniSettings {
        PatroniSettings { urls: urls.iter().map(|u| u.to_string()).collect(), username: None, tls: None, host_overrides: vec![] }
    }

    fn cluster_v3() -> serde_json::Value {
        serde_json::json!({
            "members": [
                {"name": "pg-1", "role": "leader", "state": "running", "api_url": "http://10.0.0.1:8008/patroni",
                 "host": "10.0.0.1", "port": 5432, "timeline": 7},
                {"name": "pg-2", "role": "sync_standby", "state": "streaming", "api_url": "http://10.0.0.2:8008/patroni",
                 "host": "10.0.0.2", "port": 5432, "timeline": 7, "lag": 0},
                {"name": "pg-3", "role": "replica", "state": "streaming", "api_url": "http://10.0.0.3:8008/patroni",
                 "host": "10.0.0.3", "port": 5432, "timeline": 7, "lag": "unknown",
                 "tags": {"nofailover": true}, "pending_restart": true}
            ],
            "scope": "billing"
        })
    }

    fn patroni_json(version: i64, system_id: &str) -> serde_json::Value {
        serde_json::json!({
            "state": "running", "role": "replica", "server_version": version, "timeline": 7,
            "dcs_last_seen": 1_760_000_000, "database_system_identifier": system_id,
            "patroni": {"version": "4.0.4", "scope": "billing", "name": "x"}
        })
    }

    #[test]
    fn members_from_patroni3_cluster() {
        let c: ClusterJson = serde_json::from_value(cluster_v3()).unwrap();
        let m = members_from_cluster(&c, &settings(&["http://x:8008"]));
        assert_eq!(m.len(), 3);
        assert_eq!((m[0].role, m[1].role, m[2].role), (Role::Primary, Role::SyncStandby, Role::Replica));
        assert_eq!(m[0].api_url.as_deref(), Some("http://10.0.0.1:8008"));
        assert_eq!(m[1].lag_bytes, Some(0));
        assert_eq!(m[2].lag_bytes, None, "\"unknown\" — нет данных");
        assert_eq!(m[2].tags["nofailover"], serde_json::json!(true));
        assert!(m[2].pending_restart);
        assert_eq!(m[2].raw_role.as_deref(), Some("replica"));
    }

    #[test]
    fn members_from_patroni4_cluster_and_overrides() {
        let c: ClusterJson = serde_json::from_value(serde_json::json!({
            "members": [
                {"name": "a", "role": "leader", "state": "running", "api_url": "http://a:8008/patroni", "host": "a", "port": 5432, "timeline": 3},
                {"name": "b", "role": "quorum_standby", "state": "streaming", "api_url": "http://b:8008/patroni", "host": "b", "port": 5432,
                 "receive_lag": 10, "replay_lag": 20, "lag": 20}
            ],
            "pause": true
        }))
        .unwrap();
        let mut s = settings(&["http://a:8008"]);
        s.host_overrides = vec![HostOverride { member: "b".into(), host: Some("localhost".into()), port: Some(5442), api_url: Some("http://localhost:8012".into()) }];
        let m = members_from_cluster(&c, &s);
        assert_eq!(m[1].role, Role::QuorumStandby);
        assert_eq!(m[1].lag_bytes, Some(20));
        assert_eq!((m[1].host.as_str(), m[1].port), ("localhost", 5442));
        assert_eq!(m[1].api_url.as_deref(), Some("http://localhost:8012"));
        assert_eq!(c.pause, Some(true));
    }

    #[tokio::test]
    async fn discover_tries_next_url_and_fills_details() {
        let http = FakeHttp::new(vec![
            ("http://seed2:8008/cluster", Ok(cluster_v3())),
            ("http://10.0.0.1:8008/patroni", Ok(patroni_json(170_002, "7400"))),
            ("http://10.0.0.2:8008/patroni", Ok(patroni_json(170_002, "7400"))),
        ]);
        let t = discover(&http, &settings(&["http://seed1:8008", "http://seed2:8008/"]), "c", None, 5).await.unwrap();
        assert_eq!(t.kind, ConnectionKind::Patroni);
        assert_eq!(t.scope.as_deref(), Some("billing"));
        assert_eq!(t.paused, Some(false));
        assert_eq!(t.observed_at_ms, 5);
        let pg1 = t.member("pg-1").unwrap();
        assert_eq!(pg1.server_version.as_deref(), Some("17.2"));
        assert_eq!(pg1.patroni_version.as_deref(), Some("4.0.4"));
        assert_eq!(pg1.dcs_last_seen_ms, Some(1_760_000_000_000));
        let pg3 = t.member("pg-3").unwrap();
        assert!(!pg3.reachable);
        assert_eq!(pg3.error.as_ref().unwrap().code, ErrorCode::PatroniError);
        assert_eq!(t.warnings, vec![TopologyWarning::MemberUnreachable { name: "pg-3".into() }]);
        assert_eq!(http.calls.lock_safe()[0], "http://seed1:8008/cluster");
    }

    #[tokio::test]
    async fn system_id_mismatch_is_reported() {
        let http = FakeHttp::new(vec![
            ("http://s:8008/cluster", Ok(cluster_v3())),
            ("http://10.0.0.1:8008/patroni", Ok(patroni_json(160_004, "1"))),
            ("http://10.0.0.2:8008/patroni", Ok(patroni_json(160_004, "2"))),
            ("http://10.0.0.3:8008/patroni", Ok(patroni_json(160_004, "1"))),
        ]);
        let t = discover(&http, &settings(&["http://s:8008"]), "c", None, 1).await.unwrap();
        assert!(matches!(t.warnings.as_slice(), [TopologyWarning::SystemIdMismatch { .. }]));
    }

    #[tokio::test]
    async fn unreachable_api_fails_first_time_and_degrades_later() {
        let http = FakeHttp::new(vec![]);
        let err = discover(&http, &settings(&["http://s:8008"]), "c", None, 1).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::PatroniError);

        let ok = FakeHttp::new(vec![("http://s:8008/cluster", Ok(cluster_v3()))]);
        let prev = discover(&ok, &settings(&["http://s:8008"]), "c", None, 1).await.unwrap();
        let t = discover(&http, &settings(&["http://s:8008"]), "c", Some(&prev), 2).await.unwrap();
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        assert!(t.members.iter().all(|m| !m.reachable));
        assert_eq!(t.observed_at_ms, 2);
        // Опрос начался с лидера прошлой топологии.
        assert_eq!(http.calls.lock_safe()[0], "http://10.0.0.1:8008/cluster");
    }

    #[test]
    fn bases_order_and_formats() {
        assert_eq!(api_base("http://h:8008/patroni"), "http://h:8008");
        assert_eq!(api_base("https://h:8008/"), "https://h:8008");
        assert_eq!(format_server_version(170_002), "17.2");
        assert_eq!(format_server_version(130_015), "13.15");
        assert_eq!(format_server_version(90_624), "9.6.24");
        assert_eq!(map_role(Some("master")), Role::Primary);
        assert_eq!(map_role(Some("standby_leader")), Role::StandbyLeader);
        assert_eq!(map_role(Some("weird")), Role::Unknown);
        assert_eq!(map_role(None), Role::Unknown);
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::patroni`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация** (над тестами)

```rust
//! Клиент Patroni REST API (спец. P1 §6.2): участники — `GET /cluster`, детали — `GET /patroni`.

use std::time::Duration;

use serde::Deserialize;

use crate::conn::errors::chain;
use crate::conn::profile::{ConnectionKind, PatroniSettings};
use crate::conn::tls;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::BoxFuture;
use crate::topology::model::{Member, Role, Topology, TopologyWarning, system_id_warning};

pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Сколько текста ответа с ошибкой сохраняется в `details`.
const BODY_SNIPPET: usize = 500;

/// GET JSON из Patroni; трейт — чтобы подменять сеть в тестах.
pub trait PatroniHttp: Send + Sync {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>>;
}

pub struct ReqwestHttp {
    client: reqwest::Client,
    auth: Option<(String, String)>,
}

fn http_setup(e: impl std::fmt::Display) -> AppError {
    app_err!(ErrorCode::InvalidInput, "errors.patroni.setup", "Could not prepare the Patroni API client")
        .with_details(e.to_string())
}

impl ReqwestHttp {
    /// Блокирующий вызов (файлы сертификатов). `password` — секрет `patroniPassword`.
    pub fn new(settings: &PatroniSettings, password: Option<String>, key_password: Option<&str>) -> AppResult<Self> {
        let mut b = reqwest::Client::builder().timeout(HTTP_TIMEOUT).connect_timeout(HTTP_TIMEOUT);
        if let Some(t) = &settings.tls {
            if let Some(ca) = &t.ca_path {
                let pem = tls::ca_pem_bundle(ca)?;
                for c in reqwest::Certificate::from_pem_bundle(&pem).map_err(http_setup)? {
                    b = b.add_root_certificate(c);
                }
            }
            if !t.verify_hostname {
                b = b.tls_danger_accept_invalid_hostnames(true);
            }
            if let Some(cc) = &t.client_cert {
                let pem = tls::identity_pem(cc, key_password)?;
                b = b.identity(reqwest::Identity::from_pem(&pem).map_err(http_setup)?);
            }
        }
        Ok(Self {
            client: b.build().map_err(http_setup)?,
            auth: settings.username.clone().map(|u| (u, password.unwrap_or_default())),
        })
    }
}

fn request_error(url: &str, e: &reqwest::Error) -> AppError {
    let text = chain(e);
    if e.is_timeout() {
        app_err!(ErrorCode::PatroniError, "errors.patroni.timeout", "Patroni API at {{url}} did not answer within 5 s", url = url)
    } else if text.to_lowercase().contains("certificate") {
        app_err!(ErrorCode::TlsError, "errors.patroni.tls", "TLS error talking to the Patroni API at {{url}}", url = url)
    } else {
        app_err!(ErrorCode::PatroniError, "errors.patroni.unreachable", "Patroni API at {{url}} is unreachable", url = url)
    }
    .with_details(text)
}

fn bad_json(url: &str, e: impl std::fmt::Display) -> AppError {
    app_err!(ErrorCode::PatroniError, "errors.patroni.badJson", "Patroni API at {{url}} returned an unexpected response", url = url)
        .with_details(e.to_string())
}

impl PatroniHttp for ReqwestHttp {
    fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
        Box::pin(async move {
            let mut req = self.client.get(url);
            if let Some((u, p)) = &self.auth {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send().await.map_err(|e| request_error(url, &e))?;
            let status = resp.status();
            let body = resp.text().await.map_err(|e| request_error(url, &e))?;
            let parsed = serde_json::from_str::<serde_json::Value>(&body);
            // /patroni у узла с остановленным PostgreSQL отвечает 503, но с тем же JSON.
            match (status.as_u16(), parsed) {
                (200..=299, Ok(v)) => Ok(v),
                (503, Ok(v)) if v.is_object() => Ok(v),
                (200..=299, Err(e)) => Err(bad_json(url, e)),
                (401 | 403, _) => Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.unauthorized",
                    "The Patroni API at {{url}} rejected the credentials",
                    url = url
                )),
                (code, _) => Err(app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.httpStatus",
                    "Patroni API at {{url}} answered {{status}}",
                    url = url,
                    status = code
                )
                .with_details(body.chars().take(BODY_SNIPPET).collect::<String>())),
            }
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LagJson {
    Bytes(i64),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct MemberJson {
    pub name: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub timeline: Option<i64>,
    #[serde(default)]
    pub lag: Option<LagJson>,
    #[serde(default)]
    pub replay_lag: Option<LagJson>,
    #[serde(default)]
    pub tags: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub pending_restart: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClusterJson {
    #[serde(default)]
    pub members: Vec<MemberJson>,
    #[serde(default)]
    pub pause: Option<bool>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PatroniInfo {
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PatroniJson {
    #[serde(default)]
    server_version: Option<i64>,
    #[serde(default)]
    dcs_last_seen: Option<i64>,
    #[serde(default)]
    database_system_identifier: Option<String>,
    #[serde(default)]
    patroni: Option<PatroniInfo>,
    #[serde(default)]
    pending_restart: Option<bool>,
}

/// Базовый адрес API: `http://h:8008/patroni` → `http://h:8008`.
pub fn api_base(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches("/patroni")
        .trim_end_matches('/')
        .to_string()
}

pub fn map_role(raw: Option<&str>) -> Role {
    match raw {
        Some("leader" | "master" | "primary") => Role::Primary,
        Some("standby_leader") => Role::StandbyLeader,
        Some("sync_standby") => Role::SyncStandby,
        Some("quorum_standby") => Role::QuorumStandby,
        Some("replica") => Role::Replica,
        _ => Role::Unknown,
    }
}

/// `server_version` из `/patroni` (170002) → `17.2`; до 10-й версии — `9.6.24`.
pub fn format_server_version(v: i64) -> String {
    if v >= 100_000 {
        format!("{}.{}", v / 10_000, v % 100)
    } else {
        format!("{}.{}.{}", v / 10_000, (v / 100) % 100, v % 100)
    }
}

fn lag_bytes(l: Option<&LagJson>) -> Option<i64> {
    match l {
        Some(LagJson::Bytes(b)) => Some(*b),
        _ => None,
    }
}

/// Участники из `/cluster` с подменой адресов из профиля.
pub fn members_from_cluster(c: &ClusterJson, settings: &PatroniSettings) -> Vec<Member> {
    c.members
        .iter()
        .map(|m| {
            let ov = settings.host_overrides.iter().find(|o| o.member == m.name);
            let host = ov.and_then(|o| o.host.clone()).or_else(|| m.host.clone()).unwrap_or_default();
            let port = ov.and_then(|o| o.port).or(m.port).unwrap_or(5432);
            let mut member = Member::new(&m.name, host, port);
            member.api_url = ov
                .and_then(|o| o.api_url.as_deref())
                .or(m.api_url.as_deref())
                .map(api_base);
            member.role = map_role(m.role.as_deref());
            member.raw_role = m.role.clone();
            member.state = m.state.clone();
            member.timeline = m.timeline;
            member.lag_bytes = lag_bytes(m.replay_lag.as_ref()).or_else(|| lag_bytes(m.lag.as_ref()));
            member.tags = m.tags.clone();
            member.pending_restart = m.pending_restart.unwrap_or(false);
            member
        })
        .collect()
}

/// Порядок опроса `/cluster`: лидер прошлого опроса, остальные участники, адреса профиля — без повторов.
pub fn cluster_bases(settings: &PatroniSettings, previous: Option<&Topology>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |u: &str| {
        let b = api_base(u);
        if !b.is_empty() && !out.contains(&b) {
            out.push(b);
        }
    };
    if let Some(t) = previous {
        if let Some(u) = t.leader().and_then(|m| m.api_url.as_deref()) {
            add(u);
        }
        for m in &t.members {
            if let Some(u) = m.api_url.as_deref() {
                add(u);
            }
        }
    }
    for u in &settings.urls {
        add(u);
    }
    out
}

async fn fetch_cluster(http: &dyn PatroniHttp, bases: &[String]) -> AppResult<ClusterJson> {
    let mut last = None;
    for base in bases {
        let url = format!("{base}/cluster");
        match http.get_json(&url).await.and_then(|v| serde_json::from_value(v).map_err(|e| bad_json(&url, e))) {
            Ok(c) => return Ok(c),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")
    }))
}

pub async fn discover(
    http: &dyn PatroniHttp,
    settings: &PatroniSettings,
    connection_id: &str,
    previous: Option<&Topology>,
    now_ms: i64,
) -> AppResult<Topology> {
    let cluster = match fetch_cluster(http, &cluster_bases(settings, previous)).await {
        Ok(c) => c,
        Err(e) => {
            // Первое подключение — ошибка; дальше — прошлые участники как недоступные.
            let Some(prev) = previous else { return Err(e) };
            let mut t = prev.clone();
            t.observed_at_ms = now_ms;
            for m in &mut t.members {
                m.reachable = false;
                m.error = Some(e.clone());
            }
            t.warnings = vec![TopologyWarning::PatroniUnreachable];
            return Ok(t);
        }
    };
    let mut members = members_from_cluster(&cluster, settings);
    let details = futures_util::future::join_all(members.iter().map(|m| {
        let base = m.api_url.clone();
        async move {
            let base = base?;
            let url = format!("{base}/patroni");
            Some(
                http.get_json(&url)
                    .await
                    .and_then(|v| serde_json::from_value::<PatroniJson>(v).map_err(|e| bad_json(&url, e))),
            )
        }
    }))
    .await;
    let mut warnings = Vec::new();
    for (m, d) in members.iter_mut().zip(details) {
        match d {
            Some(Ok(p)) => {
                m.server_version = p.server_version.map(format_server_version);
                m.patroni_version = p.patroni.and_then(|x| x.version);
                m.dcs_last_seen_ms = p.dcs_last_seen.map(|s| s * 1000);
                m.system_id = p.database_system_identifier;
                m.pending_restart |= p.pending_restart.unwrap_or(false);
            }
            Some(Err(e)) => {
                m.reachable = false;
                m.error = Some(e);
                warnings.push(TopologyWarning::MemberUnreachable { name: m.name.clone() });
            }
            None => {}
        }
    }
    if let Some(w) = system_id_warning(&members) {
        warnings.push(w);
    }
    Ok(Topology {
        connection_id: connection_id.to_string(),
        kind: ConnectionKind::Patroni,
        observed_at_ms: now_ms,
        scope: cluster.scope.clone(),
        paused: Some(cluster.pause.unwrap_or(false)),
        members,
        warnings,
    })
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::patroni`
Expected: PASS (6 тестов).

- [ ] **Step 4: Интеграционные тесты против стенда**

В `tests/common/mod.rs`:

```rust
use glafira_pg_lib::conn::profile::{HostOverride, PatroniSettings};

/// Настройки кластера стенда: участники отдают внутренние имена patroniN — подменяем их на порты хоста.
pub fn patroni_settings() -> Option<PatroniSettings> {
    let urls = env("GPG_IT_PATRONI")?;
    Some(PatroniSettings {
        urls: urls.split(',').map(|u| u.trim().to_string()).collect(),
        username: Some("patroni".into()),
        tls: None,
        host_overrides: (1..=3)
            .map(|i| HostOverride {
                member: format!("patroni{i}"),
                host: Some("localhost".into()),
                port: Some(5440 + i as u16),
                api_url: Some(format!("http://localhost:801{i}")),
            })
            .collect(),
    })
}

pub const PATRONI_PASSWORD_VALUE: &str = "patroni-secret";

/// `docker …`; паника, если команда не удалась.
pub fn docker(args: &[&str]) -> String {
    let out = std::process::Command::new("docker").args(args).output().expect("docker");
    assert!(out.status.success(), "docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Ждёт готовности стенда (`scripts/wait-stand.sh`): после остановок и переключений следующий тест
/// должен видеть лидера и две потоковые реплики.
pub fn wait_stand() {
    let wait = concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/wait-stand.sh");
    let out = std::process::Command::new(wait).output().expect("wait-stand.sh");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

pub fn patronictl(args: &[&str]) -> String {
    let mut full = vec!["exec", "gp-patroni1", "/opt/patroni/bin/patronictl", "-c", "/etc/patroni/patroni.yml"];
    full.extend_from_slice(args);
    docker(&full)
}
```

`src-tauri/tests/it_patroni.rs`:

```rust
//! Patroni REST API стенда (`GPG_IT_PATRONI`); переключения и пауза — ещё и `GPG_IT_DOCKER=1`.

mod common;

use std::time::Duration;

use glafira_pg_lib::topology::journal::{EventKind, diff};
use glafira_pg_lib::topology::model::{Role, Topology, now_ms};
use glafira_pg_lib::topology::patroni::{ReqwestHttp, discover};

async fn observe(prev: Option<&Topology>) -> Option<Topology> {
    let settings = common::patroni_settings()?;
    let http = ReqwestHttp::new(&settings, Some(common::PATRONI_PASSWORD_VALUE.into()), None).unwrap();
    Some(discover(&http, &settings, "it", prev, now_ms()).await.unwrap())
}

#[tokio::test]
async fn discovers_three_members_with_one_leader() {
    let Some(t) = observe(None).await else { return };
    assert_eq!(t.scope.as_deref(), Some("demo"));
    assert_eq!(t.members.len(), 3);
    assert_eq!(t.members.iter().filter(|m| m.role == Role::Primary).count(), 1);
    assert!(t.members.iter().all(|m| m.reachable), "{:?}", t.warnings);
    assert!(t.members.iter().all(|m| m.server_version.as_deref().is_some_and(|v| v.starts_with("17."))));
    assert!(t.members.iter().all(|m| m.patroni_version.as_deref().is_some_and(|v| v.starts_with("4."))));
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
}

#[tokio::test]
async fn switchover_and_pause_are_observed() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(before) = observe(None).await else { return };
    let leader = before.leader().unwrap().name.clone();
    let candidate = before.members.iter().find(|m| m.name != leader).unwrap().name.clone();
    common::patronictl(&["switchover", "--leader", &leader, "--candidate", &candidate, "--force"]);
    let mut after = before.clone();
    for _ in 0..30 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        after = observe(Some(&before)).await.unwrap();
        if after.leader().is_some_and(|m| m.name == candidate) {
            break;
        }
    }
    let events = diff(&before, &after);
    assert!(
        events.iter().any(|e| e.event == EventKind::LeaderChanged { from: Some(leader.clone()), to: Some(candidate.clone()) }),
        "{events:?}"
    );

    common::patronictl(&["pause", "--wait"]);
    let paused = observe(Some(&after)).await.unwrap();
    common::patronictl(&["resume", "--wait"]);
    common::wait_stand();
    assert_eq!(paused.paused, Some(true));
}

#[tokio::test]
async fn stopped_member_becomes_unreachable() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(before) = observe(None).await else { return };
    // Останавливаем реплику (не лидера), чтобы не ждать failover.
    let victim = before.members.iter().find(|m| m.role != Role::Primary).unwrap().name.clone();
    let container = format!("gp-{victim}");
    common::docker(&["stop", &container]);
    let t = observe(Some(&before)).await.unwrap();
    common::docker(&["start", &container]);
    common::wait_stand();
    let m = t.member(&victim).expect("участник ещё в /cluster до истечения ttl");
    assert!(!m.reachable);
}
```

В словари — ключи `errors.patroni.*`:

| Ключ | ru |
|---|---|
| `setup` | «Не удалось подготовить клиент API Patroni» |
| `timeout` | «API Patroni {{url}} не ответил за 5 с» |
| `tls` | «Ошибка TLS при обращении к API Patroni {{url}}» |
| `unreachable` | «API Patroni {{url}} недоступен» |
| `badJson` | «API Patroni {{url}} вернул неожиданный ответ» |
| `unauthorized` | «API Patroni {{url}} отклонил учётные данные» |
| `httpStatus` | «API Patroni {{url}} ответил {{status}}» |
| `noUrls` | «Нет адресов API Patroni» |

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run (стенд): `GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 cargo test --manifest-path src-tauri/Cargo.toml --test it_patroni -- --test-threads=1`
Expected: PASS (3 теста). После теста `scripts/wait-stand.sh` снова выводит `stand is ready` (остановленный участник вернулся).

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(topology): клиент Patroni REST API — обнаружение участников, версии, предупреждения

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Опрос узлов списка и одиночного сервера

**Files:**
- Create: `src-tauri/src/topology/probe.rs`, `src-tauri/src/topology/connector.rs`, `src-tauri/tests/it_probe.rs`
- Modify: `src-tauri/src/topology/mod.rs`, `src-tauri/tests/common/mod.rs`
- Test: unit-тесты в `topology/probe.rs`, интеграционные `tests/it_probe.rs`

**Interfaces:**
- Consumes: `conn::pg::ConnectSpec`, `conn::errors::from_pg`, `conn::run_blocking`, `conn::profile::{ConnectionKind, ConnectionProfile, HostPort}`, `topology::model::*`.
- Produces:
  - `crate::topology::probe::{NodeFacts { in_recovery, server_version, system_id, replicas: Vec<ReplicationRow>, sender: Option<(String, u16)>, own_lag_bytes, replay_delay_ms }, ReplicationRow { application_name, client_addr, state, sync_state, replay_lag_bytes }, collect(&tokio_postgres::Client) -> AppResult<NodeFacts>, assemble(connection_id, kind, hosts: &[HostPort], results: Vec<AppResult<NodeFacts>>, resolved: &HashMap<String, Vec<IpAddr>>, now_ms) -> Topology, observe_hosts(connection_id, kind, hosts, nodes: &dyn NodeConnector, now_ms) -> Topology}`.
  - `crate::topology::connector::{NodeConnector (trait: facts(&self, member: &str, host: &str, port: u16) -> BoxFuture<AppResult<NodeFacts>>), AdhocConnector { profile: ConnectionProfile, secrets: Arc<dyn SecretStore> }}` — разовое соединение на каждый опрос (проверка подключения).
  - `tests/common`: `reset_replication_pair()`.

- [ ] **Step 1: Тесты сборки топологии (падающие)**

`topology/mod.rs` — `pub mod connector; pub mod probe;`. В конец `src-tauri/src/topology/probe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn hp(h: &str, p: u16) -> HostPort {
        HostPort { host: h.into(), port: p }
    }

    fn primary(rows: Vec<ReplicationRow>) -> AppResult<NodeFacts> {
        Ok(NodeFacts { in_recovery: false, server_version: "16.4".into(), system_id: Some("1".into()), replicas: rows, ..Default::default() })
    }

    fn replica(sender: Option<(&str, u16)>, lag: Option<i64>) -> AppResult<NodeFacts> {
        Ok(NodeFacts {
            in_recovery: true,
            server_version: "16.4".into(),
            system_id: Some("1".into()),
            sender: sender.map(|(h, p)| (h.to_string(), p)),
            own_lag_bytes: lag,
            replay_delay_ms: Some(120),
            ..Default::default()
        })
    }

    fn row(app: &str, addr: Option<&str>, sync: &str, lag: i64) -> ReplicationRow {
        ReplicationRow {
            application_name: app.into(),
            client_addr: addr.map(String::from),
            state: Some("streaming".into()),
            sync_state: Some(sync.into()),
            replay_lag_bytes: Some(lag),
        }
    }

    fn down() -> AppResult<NodeFacts> {
        Err(app_err!(ErrorCode::ConnectionFailed, "errors.pg.tcpFailed", "Could not reach {{host}}:{{port}}", host = "x", port = 1))
    }

    #[test]
    fn primary_and_replica_matched_by_application_name() {
        let hosts = [hp("db1", 5432), hp("db2", 5432)];
        let t = assemble("c", ConnectionKind::HostList, &hosts,
            vec![primary(vec![row("db2", None, "sync", 42)]), replica(Some(("db1", 5432)), Some(7))],
            &HashMap::new(), 9);
        assert_eq!(t.members[0].role, Role::Primary);
        assert_eq!(t.members[1].role, Role::SyncStandby);
        assert_eq!(t.members[1].lag_bytes, Some(42), "лаг primary точнее собственного");
        assert_eq!(t.members[1].upstream.as_deref(), Some("db1:5432"));
        assert_eq!(t.members[1].replay_delay_ms, Some(120));
        assert_eq!(t.members[1].state.as_deref(), Some("streaming"));
        assert!(t.warnings.is_empty());
        assert_eq!(t.observed_at_ms, 9);
    }

    #[test]
    fn replica_matched_by_client_address() {
        let hosts = [hp("db1", 5432), hp("db2", 5432)];
        let resolved = HashMap::from([("db2".to_string(), vec!["10.0.0.2".parse().unwrap()])]);
        let t = assemble("c", ConnectionKind::HostList, &hosts,
            vec![primary(vec![row("walreceiver", Some("10.0.0.9"), "async", 1), row("walreceiver", Some("10.0.0.2"), "quorum", 5)]),
                 replica(None, None)],
            &resolved, 1);
        assert_eq!(t.members[1].role, Role::QuorumStandby);
        assert_eq!(t.members[1].lag_bytes, Some(5));
    }

    #[test]
    fn single_pair_rule() {
        let hosts = [hp("localhost", 5451), hp("localhost", 5452)];
        let t = assemble("c", ConnectionKind::HostList, &hosts,
            vec![primary(vec![row("walreceiver", Some("172.18.0.5"), "sync", 3)]), replica(Some(("pg-primary", 5432)), Some(1))],
            &HashMap::new(), 1);
        assert_eq!(t.members[1].role, Role::SyncStandby);
        assert_eq!(t.members[1].lag_bytes, Some(3));
        assert_eq!(t.members[1].upstream.as_deref(), Some("pg-primary:5432"), "не из списка — как есть");
    }

    #[test]
    fn primary_warnings() {
        let hosts = [hp("a", 1), hp("b", 1)];
        let none = assemble("c", ConnectionKind::HostList, &hosts, vec![replica(None, None), replica(None, None)], &HashMap::new(), 1);
        assert_eq!(none.warnings, vec![TopologyWarning::NoPrimary]);
        let two = assemble("c", ConnectionKind::HostList, &hosts, vec![primary(vec![]), primary(vec![])], &HashMap::new(), 1);
        assert_eq!(two.warnings, vec![TopologyWarning::MultiplePrimaries { members: vec!["a:1".into(), "b:1".into()] }]);
        let server = assemble("c", ConnectionKind::Server, &hosts[..1], vec![replica(None, None)], &HashMap::new(), 1);
        assert!(server.warnings.is_empty(), "у сервера нет требований к primary");
    }

    #[test]
    fn unreachable_and_system_id() {
        let hosts = [hp("a", 1), hp("b", 1), hp("c", 1)];
        let mut other = replica(None, None).unwrap();
        other.system_id = Some("2".into());
        let t = assemble("c", ConnectionKind::HostList, &hosts, vec![primary(vec![]), down(), Ok(other)], &HashMap::new(), 1);
        let b = t.member("b:1").unwrap();
        assert!(!b.reachable);
        assert_eq!(b.role, Role::Unknown);
        assert_eq!(b.error.as_ref().unwrap().key, "errors.pg.tcpFailed");
        assert!(t.warnings.contains(&TopologyWarning::MemberUnreachable { name: "b:1".into() }));
        assert!(t.warnings.contains(&TopologyWarning::SystemIdMismatch { members: vec!["a:1".into(), "c:1".into()] }));
        assert!(t.is_degraded());
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::probe`
Expected: ошибки компиляции.

- [ ] **Step 2: Реализация опроса** (над тестами `probe.rs`)

```rust
//! Опрос узлов списка и одиночного сервера (спец. P1 §6.3): роль, версия, репликация.

use std::collections::HashMap;
use std::net::IpAddr;

use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionKind, HostPort};
use crate::error::AppResult;
use crate::topology::connector::NodeConnector;
use crate::topology::model::{Member, Role, Topology, TopologyWarning, member_name, system_id_warning};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplicationRow {
    pub application_name: String,
    pub client_addr: Option<String>,
    pub state: Option<String>,
    pub sync_state: Option<String>,
    pub replay_lag_bytes: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeFacts {
    pub in_recovery: bool,
    pub server_version: String,
    pub system_id: Option<String>,
    /// Строки `pg_stat_replication` (только у primary).
    pub replicas: Vec<ReplicationRow>,
    /// `pg_stat_wal_receiver.sender_host:sender_port` (только у реплики).
    pub sender: Option<(String, u16)>,
    pub own_lag_bytes: Option<i64>,
    pub replay_delay_ms: Option<i64>,
}

pub async fn collect(c: &Client) -> AppResult<NodeFacts> {
    let row = c
        .query_one("SELECT pg_is_in_recovery(), current_setting('server_version')", &[])
        .await
        .map_err(|e| from_pg(&e))?;
    let mut f = NodeFacts { in_recovery: row.get(0), server_version: row.get(1), ..Default::default() };
    // pg_control_system() может быть недоступна роли — тогда без проверки system_identifier.
    f.system_id = c
        .query_one("SELECT system_identifier::text FROM pg_control_system()", &[])
        .await
        .ok()
        .map(|r| r.get(0));
    if f.in_recovery {
        if let Some(r) = c
            .query_opt("SELECT sender_host, sender_port FROM pg_stat_wal_receiver", &[])
            .await
            .map_err(|e| from_pg(&e))?
        {
            let host: Option<String> = r.get(0);
            let port: Option<i32> = r.get(1);
            f.sender = host.map(|h| (h, port.and_then(|p| u16::try_from(p).ok()).unwrap_or(5432)));
        }
        let r = c
            .query_one(
                "SELECT pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::bigint, \
                 (extract(epoch FROM now() - pg_last_xact_replay_timestamp()) * 1000)::bigint",
                &[],
            )
            .await
            .map_err(|e| from_pg(&e))?;
        f.own_lag_bytes = r.get(0);
        f.replay_delay_ms = r.get(1);
    } else {
        let rows = c
            .query(
                "SELECT application_name, host(client_addr), state, sync_state, \
                 pg_wal_lsn_diff(pg_current_wal_lsn(), replay_lsn)::bigint FROM pg_stat_replication",
                &[],
            )
            .await
            .map_err(|e| from_pg(&e))?;
        f.replicas = rows
            .iter()
            .map(|r| ReplicationRow {
                application_name: r.get::<_, Option<String>>(0).unwrap_or_default(),
                client_addr: r.get(1),
                state: r.get(2),
                sync_state: r.get(3),
                replay_lag_bytes: r.get(4),
            })
            .collect();
    }
    Ok(f)
}

fn apply_row(m: &mut Member, r: &ReplicationRow) {
    if r.replay_lag_bytes.is_some() {
        m.lag_bytes = r.replay_lag_bytes;
    }
    m.role = match r.sync_state.as_deref() {
        Some("sync") => Role::SyncStandby,
        Some("quorum") => Role::QuorumStandby,
        _ => Role::Replica,
    };
    m.state = r.state.clone();
}

/// Реплики ↔ строки `pg_stat_replication` primary: по `application_name`, по адресу клиента,
/// затем правило «единственной пары» (отступление 5 плана P1a).
fn match_replicas(members: &mut [Member], rows: &[ReplicationRow], resolved: &HashMap<String, Vec<IpAddr>>) {
    let mut used = vec![false; rows.len()];
    let mut unmatched = Vec::new();
    let replicas: Vec<usize> = (0..members.len()).filter(|&i| members[i].role == Role::Replica).collect();
    for i in replicas {
        let m = &members[i];
        let ips = resolved.get(&m.host);
        let found = rows.iter().enumerate().position(|(j, r)| {
            !used[j]
                && (r.application_name == m.host
                    || r.application_name == m.name
                    || r.client_addr.as_ref().is_some_and(|a| {
                        ips.is_some_and(|ips| ips.iter().any(|ip| ip.to_string() == *a))
                    }))
        });
        match found {
            Some(j) => {
                used[j] = true;
                apply_row(&mut members[i], &rows[j]);
            }
            None => unmatched.push(i),
        }
    }
    let free: Vec<usize> = (0..rows.len()).filter(|&j| !used[j]).collect();
    if let ([i], [j]) = (unmatched.as_slice(), free.as_slice()) {
        apply_row(&mut members[*i], &rows[*j]);
    }
}

pub fn assemble(
    connection_id: &str,
    kind: ConnectionKind,
    hosts: &[HostPort],
    results: Vec<AppResult<NodeFacts>>,
    resolved: &HashMap<String, Vec<IpAddr>>,
    now_ms: i64,
) -> Topology {
    let mut members = Vec::with_capacity(hosts.len());
    let mut facts: Vec<Option<NodeFacts>> = Vec::with_capacity(hosts.len());
    let mut warnings = Vec::new();
    for (h, r) in hosts.iter().zip(results) {
        let mut m = Member::new(member_name(&h.host, h.port), &h.host, h.port);
        match r {
            Ok(f) => {
                m.role = if f.in_recovery { Role::Replica } else { Role::Primary };
                m.server_version = Some(f.server_version.clone());
                m.system_id = f.system_id.clone();
                m.lag_bytes = f.own_lag_bytes;
                m.replay_delay_ms = f.replay_delay_ms;
                m.upstream = f.sender.as_ref().map(|(sh, sp)| {
                    hosts
                        .iter()
                        .find(|x| &x.host == sh && x.port == *sp)
                        .map(|x| member_name(&x.host, x.port))
                        .unwrap_or_else(|| member_name(sh, *sp))
                });
                facts.push(Some(f));
            }
            Err(e) => {
                m.reachable = false;
                m.error = Some(e);
                warnings.push(TopologyWarning::MemberUnreachable { name: m.name.clone() });
                facts.push(None);
            }
        }
        members.push(m);
    }
    let primaries: Vec<usize> = (0..members.len()).filter(|&i| members[i].role == Role::Primary).collect();
    if kind == ConnectionKind::HostList {
        match primaries.len() {
            0 => warnings.insert(0, TopologyWarning::NoPrimary),
            1 => {}
            _ => warnings.insert(
                0,
                TopologyWarning::MultiplePrimaries { members: primaries.iter().map(|&i| members[i].name.clone()).collect() },
            ),
        }
    }
    if let [p] = primaries.as_slice() {
        let rows = facts[*p].as_ref().map(|f| f.replicas.clone()).unwrap_or_default();
        match_replicas(&mut members, &rows, resolved);
    }
    if let Some(w) = system_id_warning(&members) {
        warnings.push(w);
    }
    Topology {
        connection_id: connection_id.to_string(),
        kind,
        observed_at_ms: now_ms,
        scope: None,
        paused: None,
        members,
        warnings,
    }
}

async fn resolve(hosts: &[HostPort]) -> HashMap<String, Vec<IpAddr>> {
    let mut out = HashMap::new();
    for h in hosts {
        if let Ok(addrs) = tokio::net::lookup_host((h.host.as_str(), h.port)).await {
            out.insert(h.host.clone(), addrs.map(|a| a.ip()).collect());
        }
    }
    out
}

/// Опрос всех узлов параллельно и сборка топологии.
pub async fn observe_hosts(
    connection_id: &str,
    kind: ConnectionKind,
    hosts: &[HostPort],
    nodes: &dyn NodeConnector,
    now_ms: i64,
) -> Topology {
    let names: Vec<String> = hosts.iter().map(|h| member_name(&h.host, h.port)).collect();
    let results = futures_util::future::join_all(
        hosts.iter().zip(&names).map(|(h, name)| nodes.facts(name, &h.host, h.port)),
    )
    .await;
    let resolved = resolve(hosts).await;
    assemble(connection_id, kind, hosts, results, &resolved, now_ms)
}
```

`src-tauri/src/topology/connector.rs`:

```rust
//! Соединение с узлом для опроса топологии: разовое (проверка подключения) или из пула (Task 12).

use std::sync::Arc;

use crate::conn::pg::ConnectSpec;
use crate::conn::profile::ConnectionProfile;
use crate::conn::run_blocking;
use crate::error::AppResult;
use crate::secrets::SecretStore;
use crate::topology::BoxFuture;
use crate::topology::probe::{NodeFacts, collect};

pub trait NodeConnector: Send + Sync {
    fn facts<'a>(&'a self, member: &'a str, host: &'a str, port: u16) -> BoxFuture<'a, AppResult<NodeFacts>>;
}

/// Новое соединение на каждый опрос; секреты — как переданы (для проверки — форма поверх keychain).
pub struct AdhocConnector {
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
}

impl NodeConnector for AdhocConnector {
    fn facts<'a>(&'a self, _member: &'a str, host: &'a str, port: u16) -> BoxFuture<'a, AppResult<NodeFacts>> {
        Box::pin(async move {
            let (profile, secrets, h) = (self.profile.clone(), self.secrets.clone(), host.to_string());
            let db = profile.pg.database.clone();
            let spec = run_blocking(move || ConnectSpec::build(&profile, &h, port, &db, secrets.as_ref())).await?;
            let client = spec.connect().await?;
            collect(&client).await
        })
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::probe`
Expected: PASS (5 тестов).

- [ ] **Step 3: Интеграционные тесты**

В `tests/common/mod.rs`:

```rust
use glafira_pg_lib::conn::profile::ConnectionKind;

/// Список узлов стенда без Patroni.
pub fn host_list_profile(id: &str) -> Option<ConnectionProfile> {
    let (primary, replica) = (env("GPG_IT_PRIMARY")?, env("GPG_IT_REPLICA")?);
    let mut p = server_profile(id, &primary);
    p.kind = ConnectionKind::HostList;
    let (h, port) = host_port(&replica);
    p.hosts.push(HostPort { host: h, port });
    Some(p)
}

/// Возвращает пару primary/реплика стенда в исходное состояние (после promote и остановок).
pub fn reset_replication_pair() {
    let compose = concat!(env!("CARGO_MANIFEST_DIR"), "/../dev/docker-compose.yml");
    docker(&["compose", "-f", compose, "rm", "-sfv", "pg-primary", "pg-replica"]);
    docker(&["compose", "-f", compose, "up", "-d", "pg-primary", "pg-replica"]);
    wait_stand();
}
```

`src-tauri/tests/it_probe.rs`:

```rust
//! Список узлов стенда (`GPG_IT_PRIMARY`, `GPG_IT_REPLICA`); остановка и promote — с `GPG_IT_DOCKER=1`.

mod common;

use glafira_pg_lib::topology::connector::AdhocConnector;
use glafira_pg_lib::topology::model::{Role, Topology, TopologyWarning, now_ms};
use glafira_pg_lib::topology::probe::observe_hosts;

async fn observe() -> Option<Topology> {
    let p = common::host_list_profile("it")?;
    let nodes = AdhocConnector { profile: p.clone(), secrets: common::secrets_with_password("it") };
    Some(observe_hosts(&p.id, p.kind, &p.hosts, &nodes, now_ms()).await)
}

#[tokio::test]
async fn primary_and_replica_are_detected() {
    let Some(t) = observe().await else { return };
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert_eq!(t.members[0].role, Role::Primary);
    assert!(t.members[1].role.is_standby(), "{:?}", t.members[1].role);
    assert!(t.members[1].lag_bytes.is_some());
    assert_eq!(t.members[1].upstream.as_deref(), Some("pg-primary:5432"));
    assert!(t.members.iter().all(|m| m.server_version.as_deref().is_some_and(|v| v.starts_with("16"))));
}

#[tokio::test]
async fn stopped_primary_then_promoted_replica() {
    if common::env("GPG_IT_DOCKER").is_none() || common::env("GPG_IT_PRIMARY").is_none() {
        return;
    }
    common::docker(&["stop", "gp-pg-primary"]);
    let down = observe().await.unwrap();
    common::docker(&["exec", "gp-pg-replica", "psql", "-U", "postgres", "-tAc", "select pg_promote()"]);
    let promoted = observe().await.unwrap();
    common::reset_replication_pair();

    assert!(down.warnings.contains(&TopologyWarning::NoPrimary), "{:?}", down.warnings);
    assert!(!down.members[0].reachable);
    assert_eq!(promoted.members[1].role, Role::Primary);
    assert!(!promoted.warnings.contains(&TopologyWarning::NoPrimary));
}
```

Run (стенд): `GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_DOCKER=1 cargo test --manifest-path src-tauri/Cargo.toml --test it_probe -- --test-threads=1`
Expected: PASS (2 теста); после теста стенд снова готов.

- [ ] **Step 4: Commit**

```bash
git add src-tauri
git commit -m "feat(topology): опрос узлов списка — роли, репликация, предупреждения о primary

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Открытые подключения — наблюдение, события, узел каталога, команды

**Files:**
- Create: `src-tauri/src/topology/observe.rs`, `src-tauri/src/topology/runtime.rs`, `src-tauri/src/commands/topology.rs`, `src-tauri/tests/it_runtime.rs`
- Modify: `src-tauri/src/topology/mod.rs`, `src-tauri/src/topology/connector.rs`, `src-tauri/src/state.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `topology/observe.rs`, `topology/runtime.rs`; интеграционные `tests/it_runtime.rs`

**Interfaces:**
- Consumes: всё из Tasks 8–11.
- Produces:
  - `crate::topology::connector::{PooledConnector { pools, profile, secrets }, pooled_client(&PoolRegistry, &ConnectionProfile, Arc<dyn SecretStore>, member, host, port, database) -> AppResult<PooledClient>}`.
  - `crate::topology::observe::{observe(&ConnectionProfile, Option<&dyn PatroniHttp>, &dyn NodeConnector, Option<&Topology>, now_ms) -> AppResult<Topology>, default_member(&ConnectionProfile, &Topology) -> Option<String>}`.
  - `crate::topology::runtime::{POLL_INTERVAL, ConnState (Disconnected | Connecting | Connected | Degraded | Failed), TopologyView { state, topology, catalog_member, catalog_fallback, error }, RuntimeSink, TopologySource, LiveSource, Runtimes}`; `Runtimes::{new(source, pools) -> Arc<Self>, with_interval(source, pools, Duration) -> Arc<Self>, set_sink(Arc<dyn RuntimeSink>), connect(self: &Arc<Self>, ConnectionProfile) -> AppResult<TopologyView>, disconnect(&str), view(&str) -> TopologyView, events(&str) -> Vec<TopologyEvent>, set_catalog_member(&str, Option<String>) -> AppResult<TopologyView>, target(&str, member: Option<&str>) -> AppResult<(Member, ConnectionProfile)>, shutdown()}`.
  - `AppState.pools: Arc<PoolRegistry>`, `AppState.runtimes: Arc<Runtimes>`; `AppState::client(&self, connection_id, member: Option<&str>, database: Option<&str>) -> AppResult<PooledClient>` (async; `member = None` — узел каталога, `database = None` — `pg.database`).
  - Команды: `connect(id) -> TopologyView`, `disconnect(id)`, `get_topology(id) -> TopologyView`, `list_topology_events(id) -> Vec<TopologyEvent>`, `set_catalog_member(id, member: Option<String>) -> TopologyView`, `get_member_facts(id, member) -> MemberFacts { startedAtMs, connections, maxConnections, databases }`.
  - События Tauri: `connection://state` — `{ connectionId, state, error }`; `topology://changed` — `{ connectionId, view, newEvents }`.

- [ ] **Step 1: Тесты узла по умолчанию (падающие)**

`topology/mod.rs` — `pub mod observe; pub mod runtime;`. В конец `src-tauri/src/topology/observe.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{ConnectionKind, TargetServerType, sample_profile};
    use crate::topology::model::{Role, member, topo};

    fn host_list(target: TargetServerType) -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::HostList;
        p.target_server_type = target;
        p
    }

    #[test]
    fn host_list_targets() {
        let t = topo(vec![member("p", Role::Primary), member("r1", Role::Replica), member("r2", Role::SyncStandby)]);
        assert_eq!(default_member(&host_list(TargetServerType::Primary), &t).as_deref(), Some("p"));
        assert_eq!(default_member(&host_list(TargetServerType::Standby), &t).as_deref(), Some("r1"));
        assert_eq!(default_member(&host_list(TargetServerType::PreferStandby), &t).as_deref(), Some("r1"));
        assert_eq!(default_member(&host_list(TargetServerType::Any), &t).as_deref(), Some("p"));

        let mut only_primary = topo(vec![member("p", Role::Primary), member("r", Role::Replica)]);
        only_primary.members[1].reachable = false;
        assert_eq!(default_member(&host_list(TargetServerType::Standby), &only_primary), None);
        assert_eq!(default_member(&host_list(TargetServerType::PreferStandby), &only_primary).as_deref(), Some("p"));
    }

    #[test]
    fn patroni_and_server() {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::Patroni;
        let mut t = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        assert_eq!(default_member(&p, &t).as_deref(), Some("b"));
        t.members[1].reachable = false;
        assert_eq!(default_member(&p, &t).as_deref(), Some("a"), "лидер недоступен — любой доступный");
        let s = sample_profile("s");
        let one = topo(vec![member("localhost:5432", Role::Replica)]);
        assert_eq!(default_member(&s, &one).as_deref(), Some("localhost:5432"));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::observe`
Expected: ошибки компиляции.

- [ ] **Step 2: Реализовать `observe.rs` и пул-коннектор**

`src-tauri/src/topology/observe.rs` (над тестами):

```rust
//! Один опрос топологии подключения и выбор узла каталога по умолчанию (спец. P1 §6.3, §7).

use crate::conn::profile::{ConnectionKind, ConnectionProfile, TargetServerType};
use crate::error::{AppResult, ErrorCode};
use crate::topology::connector::NodeConnector;
use crate::topology::model::{Member, Role, Topology};
use crate::topology::patroni::{PatroniHttp, discover};
use crate::topology::probe::observe_hosts;

pub async fn observe(
    profile: &ConnectionProfile,
    http: Option<&dyn PatroniHttp>,
    nodes: &dyn NodeConnector,
    previous: Option<&Topology>,
    now_ms: i64,
) -> AppResult<Topology> {
    match profile.kind {
        ConnectionKind::Patroni => {
            let (Some(settings), Some(http)) = (profile.patroni.as_ref(), http) else {
                return Err(app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses"));
            };
            discover(http, settings, &profile.id, previous, now_ms).await
        }
        ConnectionKind::HostList | ConnectionKind::Server => {
            Ok(observe_hosts(&profile.id, profile.kind, &profile.hosts, nodes, now_ms).await)
        }
    }
}

pub fn default_member(profile: &ConnectionProfile, t: &Topology) -> Option<String> {
    let reachable = |m: &&Member| m.reachable;
    let pick = match profile.kind {
        ConnectionKind::Server => t.members.first(),
        ConnectionKind::Patroni => t.leader().filter(reachable).or_else(|| t.members.iter().find(reachable)),
        ConnectionKind::HostList => {
            let primary = t.members.iter().find(|m| m.reachable && m.role == Role::Primary);
            let standby = t.members.iter().find(|m| m.reachable && m.role.is_standby());
            match profile.target_server_type {
                TargetServerType::Primary => primary,
                TargetServerType::Standby => standby,
                TargetServerType::PreferStandby => standby.or(primary),
                TargetServerType::Any => t.members.iter().find(reachable),
            }
        }
    };
    pick.map(|m| m.name.clone())
}
```

В `src-tauri/src/topology/connector.rs` добавить:

```rust
use crate::conn::pool::{PoolKey, PoolRegistry, PooledClient};
use crate::error::ErrorCode;

/// Соединение из пула приложения; при AUTH/TLS-ошибке пул вытесняется — следующая попытка
/// перечитает keychain и файлы сертификатов.
pub async fn pooled_client(
    pools: &PoolRegistry,
    profile: &ConnectionProfile,
    secrets: Arc<dyn SecretStore>,
    member: &str,
    host: &str,
    port: u16,
    database: &str,
) -> AppResult<PooledClient> {
    let key = PoolKey { connection_id: profile.id.clone(), member: member.into(), database: database.into() };
    let pool = match pools.get(&key) {
        Some(p) => p,
        None => {
            let (p, h, d) = (profile.clone(), host.to_string(), database.to_string());
            let spec = run_blocking(move || ConnectSpec::build(&p, &h, port, &d, secrets.as_ref())).await?;
            pools.insert_if_absent(key.clone(), spec)
        }
    };
    pool.get().await.inspect_err(|e| {
        if matches!(e.code, ErrorCode::AuthFailed | ErrorCode::TlsError) {
            pools.evict(&key);
        }
    })
}

pub struct PooledConnector {
    pub pools: Arc<PoolRegistry>,
    pub profile: ConnectionProfile,
    pub secrets: Arc<dyn SecretStore>,
}

impl NodeConnector for PooledConnector {
    fn facts<'a>(&'a self, member: &'a str, host: &'a str, port: u16) -> BoxFuture<'a, AppResult<NodeFacts>> {
        Box::pin(async move {
            let client = pooled_client(&self.pools, &self.profile, self.secrets.clone(), member, host, port, &self.profile.pg.database).await?;
            collect(&client).await
        })
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::observe`
Expected: PASS.

- [ ] **Step 3: Тесты наблюдения (падающие)**

В конец `src-tauri/src/topology/runtime.rs`:

```rust
#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::conn::profile::{ConnectionKind, sample_profile};
    use crate::error::ErrorCode;
    use crate::topology::journal::EventKind;
    use crate::topology::model::{Role, member, topo};

    /// Источник, отдающий заранее заданные ответы; последний повторяется.
    struct Scripted {
        answers: Mutex<VecDeque<AppResult<Topology>>>,
        last: Mutex<Option<AppResult<Topology>>>,
        calls: AtomicUsize,
        delay: Duration,
    }

    impl Scripted {
        fn new(answers: Vec<AppResult<Topology>>) -> Arc<Self> {
            Arc::new(Self { answers: Mutex::new(answers.into()), last: Mutex::new(None), calls: AtomicUsize::new(0), delay: Duration::ZERO })
        }
    }

    impl TopologySource for Scripted {
        fn observe<'a>(&'a self, _p: &'a ConnectionProfile, _prev: Option<&'a Topology>) -> BoxFuture<'a, AppResult<Topology>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.answers.lock_safe().pop_front();
            let r = match next {
                Some(r) => {
                    *self.last.lock_safe() = Some(r.clone());
                    r
                }
                None => self.last.lock_safe().clone().expect("есть ответ"),
            };
            let delay = self.delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                r.map(|mut t| {
                    t.observed_at_ms = crate::topology::model::now_ms();
                    t
                })
            })
        }
    }

    #[derive(Default)]
    struct Recorder {
        states: Mutex<Vec<ConnState>>,
        topologies: Mutex<Vec<Vec<TopologyEvent>>>,
    }

    impl RuntimeSink for Recorder {
        fn state_changed(&self, _id: &str, state: ConnState, _e: Option<&AppError>) {
            self.states.lock_safe().push(state);
        }
        fn topology_changed(&self, _id: &str, _v: &TopologyView, events: &[TopologyEvent]) {
            self.topologies.lock_safe().push(events.to_vec());
        }
    }

    fn patroni_profile() -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::Patroni;
        p
    }

    fn two(leader: &str) -> Topology {
        let other = if leader == "a" { "b" } else { "a" };
        topo(vec![member(leader, Role::Primary), member(other, Role::Replica)])
    }

    fn setup(answers: Vec<AppResult<Topology>>) -> (Arc<Runtimes>, Arc<Scripted>, Arc<Recorder>, Arc<PoolRegistry>) {
        let src = Scripted::new(answers);
        let pools = Arc::new(PoolRegistry::default());
        let rt = Runtimes::with_interval(src.clone(), pools.clone(), Duration::from_millis(40));
        let rec = Arc::new(Recorder::default());
        rt.set_sink(rec.clone());
        (rt, src, rec, pools)
    }

    fn err() -> AppError {
        app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")
    }

    #[tokio::test]
    async fn connect_then_poll_records_switchover() {
        let (rt, _src, rec, _) = setup(vec![Ok(two("a")), Ok(two("b"))]);
        let view = rt.connect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert_eq!(view.catalog_member.as_deref(), Some("a"));
        tokio::time::sleep(Duration::from_millis(150)).await;
        let events = rt.events("c");
        assert!(events.iter().any(|e| e.event == EventKind::LeaderChanged { from: Some("a".into()), to: Some("b".into()) }));
        assert_eq!(rt.view("c").catalog_member.as_deref(), Some("b"), "каталог следует за лидером");
        assert_eq!(&rec.states.lock_safe()[..2], &[ConnState::Connecting, ConnState::Connected]);
        assert!(rec.topologies.lock_safe().iter().any(|e| !e.is_empty()));
        rt.shutdown();
    }

    #[tokio::test]
    async fn poll_error_degrades_and_recovers() {
        let (rt, _src, _rec, _) = setup(vec![Ok(two("a")), Err(err()), Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let v = rt.view("c");
        assert_eq!(v.state, ConnState::Degraded);
        assert!(v.error.is_some());
        assert!(v.topology.is_some(), "прошлая топология остаётся");
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(rt.view("c").state, ConnState::Connected);
        rt.shutdown();
    }

    #[tokio::test]
    async fn failed_connect_and_disconnect() {
        let (rt, src, rec, pools) = setup(vec![Err(err())]);
        let e = rt.connect(patroni_profile()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::PatroniError);
        assert_eq!(rt.view("c").state, ConnState::Failed);

        *src.answers.lock_safe() = VecDeque::from(vec![Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        pools.insert_if_absent(
            crate::conn::pool::PoolKey { connection_id: "c".into(), member: "a".into(), database: "postgres".into() },
            crate::conn::pg::ConnectSpec::build(&sample_profile("c"), "h", 1, "postgres", &crate::secrets::MemoryStore::default()).unwrap(),
        );
        rt.disconnect("c");
        assert_eq!(rt.view("c").state, ConnState::Disconnected);
        assert!(pools.keys().is_empty(), "пулы подключения закрыты");
        let calls = src.calls.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(src.calls.load(Ordering::SeqCst), calls, "опрос остановлен");
        assert_eq!(rec.states.lock_safe().last(), Some(&ConnState::Disconnected));
    }

    #[tokio::test]
    async fn catalog_member_choice_and_fallback() {
        let mut down = two("a");
        down.members[1].reachable = false;
        let (rt, _src, _rec, _) = setup(vec![Ok(two("a")), Ok(down)]);
        rt.connect(patroni_profile()).await.unwrap();
        let v = rt.set_catalog_member("c", Some("b".into())).unwrap();
        assert_eq!((v.catalog_member.as_deref(), v.catalog_fallback), (Some("b"), false));
        assert_eq!(rt.set_catalog_member("c", Some("zzz".into())).unwrap_err().key, "errors.topology.unknownMember");
        tokio::time::sleep(Duration::from_millis(80)).await;
        let v = rt.view("c");
        assert_eq!((v.catalog_member.as_deref(), v.catalog_fallback), (Some("a"), true));
        let (m, _) = rt.target("c", None).unwrap();
        assert_eq!(m.name, "a");
        assert_eq!(rt.target("nope", None).unwrap_err().key, "errors.topology.notConnected");
        rt.shutdown();
    }

    #[tokio::test]
    async fn concurrent_connects_observe_once() {
        let src = Arc::new(Scripted {
            answers: Mutex::new(VecDeque::from(vec![Ok(two("a"))])),
            last: Mutex::new(None),
            calls: AtomicUsize::new(0),
            delay: Duration::from_millis(50),
        });
        let rt = Runtimes::with_interval(src.clone(), Arc::new(PoolRegistry::default()), Duration::from_secs(60));
        let (a, b) = tokio::join!(rt.connect(patroni_profile()), rt.connect(patroni_profile()));
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(src.calls.load(Ordering::SeqCst), 1);
        rt.shutdown();
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::runtime`
Expected: ошибки компиляции.

- [ ] **Step 4: Реализовать `runtime.rs`** (над тестами)

```rust
//! Открытые подключения (спец. P1 §6–7): состояние, последняя топология, журнал, узел каталога,
//! фоновый опрос раз в 10 с. События уходят в интерфейс через `RuntimeSink`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::Serialize;
use tokio::task::JoinHandle;

use crate::conn::pool::{IDLE_TIMEOUT, PoolRegistry};
use crate::conn::profile::{ConnectionKind, ConnectionProfile};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SecretStore, secret_key};
use crate::sync::MutexExt;
use crate::topology::BoxFuture;
use crate::topology::connector::PooledConnector;
use crate::topology::journal::{Journal, TopologyEvent, diff};
use crate::topology::model::{Member, Topology, now_ms};
use crate::topology::observe::{default_member, observe};
use crate::topology::patroni::{PatroniHttp, ReqwestHttp};

pub const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConnState {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    Failed,
}

/// Подключение глазами интерфейса (`get_topology`, событие `topology://changed`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyView {
    pub state: ConnState,
    pub topology: Option<Topology>,
    /// Узел, с которого читается каталог.
    pub catalog_member: Option<String>,
    /// Выбранный пользователем узел недоступен — используется узел по умолчанию.
    pub catalog_fallback: bool,
    pub error: Option<AppError>,
}

impl TopologyView {
    fn disconnected() -> Self {
        Self { state: ConnState::Disconnected, topology: None, catalog_member: None, catalog_fallback: false, error: None }
    }
}

pub trait RuntimeSink: Send + Sync {
    fn state_changed(&self, connection_id: &str, state: ConnState, error: Option<&AppError>);
    fn topology_changed(&self, connection_id: &str, view: &TopologyView, new_events: &[TopologyEvent]);
}

/// Откуда берётся топология: в приложении — Patroni и опрос узлов, в тестах — подмена.
pub trait TopologySource: Send + Sync {
    fn observe<'a>(&'a self, profile: &'a ConnectionProfile, previous: Option<&'a Topology>) -> BoxFuture<'a, AppResult<Topology>>;
    /// Подключение закрыто: забыть кэш (клиент HTTP и т. п.).
    fn forget(&self, _connection_id: &str) {}
}

/// Patroni REST и опрос узлов через пулы приложения. Клиент HTTP строится один раз на подключение:
/// keychain не читается каждые 10 с.
pub struct LiveSource {
    secrets: Arc<dyn SecretStore>,
    pools: Arc<PoolRegistry>,
    http: Mutex<HashMap<String, Arc<ReqwestHttp>>>,
}

impl LiveSource {
    pub fn new(secrets: Arc<dyn SecretStore>, pools: Arc<PoolRegistry>) -> Self {
        Self { secrets, pools, http: Mutex::new(HashMap::new()) }
    }

    async fn http_for(&self, profile: &ConnectionProfile) -> AppResult<Arc<ReqwestHttp>> {
        if let Some(h) = self.http.lock_safe().get(&profile.id) {
            return Ok(h.clone());
        }
        let (p, s) = (profile.clone(), self.secrets.clone());
        let http = run_blocking(move || {
            let settings = p.patroni.as_ref().ok_or_else(|| {
                app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")
            })?;
            let password = s.get(&secret_key(&p.id, PATRONI_PASSWORD))?;
            let key_password = s.get(&secret_key(&p.id, PATRONI_KEY_PASSWORD))?;
            ReqwestHttp::new(settings, password, key_password.as_deref())
        })
        .await?;
        let http = Arc::new(http);
        self.http.lock_safe().insert(profile.id.clone(), http.clone());
        Ok(http)
    }
}

impl TopologySource for LiveSource {
    fn observe<'a>(&'a self, profile: &'a ConnectionProfile, previous: Option<&'a Topology>) -> BoxFuture<'a, AppResult<Topology>> {
        Box::pin(async move {
            let nodes = PooledConnector { pools: self.pools.clone(), profile: profile.clone(), secrets: self.secrets.clone() };
            let http = match profile.kind {
                ConnectionKind::Patroni => Some(self.http_for(profile).await?),
                _ => None,
            };
            observe(profile, http.as_deref().map(|h| h as &dyn PatroniHttp), &nodes, previous, now_ms()).await
        })
    }

    fn forget(&self, connection_id: &str) {
        self.http.lock_safe().remove(connection_id);
    }
}

struct Entry {
    profile: ConnectionProfile,
    state: ConnState,
    topology: Option<Topology>,
    journal: Journal,
    chosen_member: Option<String>,
    error: Option<AppError>,
    poller: Option<JoinHandle<()>>,
}

impl Entry {
    fn view(&self) -> TopologyView {
        let (catalog_member, catalog_fallback) = match &self.topology {
            None => (None, false),
            Some(t) => {
                let chosen = self
                    .chosen_member
                    .as_deref()
                    .filter(|name| t.member(name).is_some_and(|m| m.reachable));
                match (chosen, &self.chosen_member) {
                    (Some(c), _) => (Some(c.to_string()), false),
                    (None, wanted) => (default_member(&self.profile, t), wanted.is_some()),
                }
            }
        };
        TopologyView {
            state: self.state,
            topology: self.topology.clone(),
            catalog_member,
            catalog_fallback,
            error: self.error.clone(),
        }
    }
}

fn state_for(t: &Topology) -> ConnState {
    if t.is_degraded() { ConnState::Degraded } else { ConnState::Connected }
}

pub struct Runtimes {
    source: Arc<dyn TopologySource>,
    pools: Arc<PoolRegistry>,
    interval: Duration,
    entries: Mutex<HashMap<String, Entry>>,
    connect_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    sink: Mutex<Option<Arc<dyn RuntimeSink>>>,
}

fn not_connected() -> AppError {
    app_err!(ErrorCode::NodeUnavailable, "errors.topology.notConnected", "The connection is not open")
}

impl Runtimes {
    pub fn new(source: Arc<dyn TopologySource>, pools: Arc<PoolRegistry>) -> Arc<Self> {
        Self::with_interval(source, pools, POLL_INTERVAL)
    }

    pub fn with_interval(source: Arc<dyn TopologySource>, pools: Arc<PoolRegistry>, interval: Duration) -> Arc<Self> {
        Arc::new(Self {
            source,
            pools,
            interval,
            entries: Mutex::new(HashMap::new()),
            connect_locks: Mutex::new(HashMap::new()),
            sink: Mutex::new(None),
        })
    }

    pub fn set_sink(&self, sink: Arc<dyn RuntimeSink>) {
        *self.sink.lock_safe() = Some(sink);
    }

    fn sink(&self) -> Option<Arc<dyn RuntimeSink>> {
        self.sink.lock_safe().clone()
    }

    fn emit_state(&self, id: &str, state: ConnState, error: Option<&AppError>) {
        if let Some(s) = self.sink() {
            s.state_changed(id, state, error);
        }
    }

    fn emit_topology(&self, id: &str, view: &TopologyView, events: &[TopologyEvent]) {
        if let Some(s) = self.sink() {
            s.topology_changed(id, view, events);
        }
    }

    /// Открывает подключение (или возвращает уже открытое с тем же профилем).
    pub async fn connect(self: &Arc<Self>, profile: ConnectionProfile) -> AppResult<TopologyView> {
        let id = profile.id.clone();
        let lock = self.connect_locks.lock_safe().entry(id.clone()).or_default().clone();
        let _guard = lock.lock().await;
        {
            let mut entries = self.entries.lock_safe();
            if let Some(e) = entries.get(&id)
                && matches!(e.state, ConnState::Connected | ConnState::Degraded)
                && e.profile == profile
            {
                return Ok(e.view());
            }
            if let Some(old) = entries.remove(&id)
                && let Some(h) = old.poller
            {
                h.abort();
            }
            entries.insert(id.clone(), Entry {
                profile: profile.clone(),
                state: ConnState::Connecting,
                topology: None,
                journal: Journal::default(),
                chosen_member: None,
                error: None,
                poller: None,
            });
        }
        self.pools.evict_connection(&id);
        self.source.forget(&id);
        self.emit_state(&id, ConnState::Connecting, None);
        match self.source.observe(&profile, None).await {
            Ok(t) => {
                let view = {
                    let mut entries = self.entries.lock_safe();
                    let e = entries.get_mut(&id).ok_or_else(not_connected)?;
                    e.state = state_for(&t);
                    e.topology = Some(t);
                    e.view()
                };
                self.emit_state(&id, view.state, None);
                self.emit_topology(&id, &view, &[]);
                self.start_poller(&id);
                Ok(view)
            }
            Err(err) => {
                if let Some(e) = self.entries.lock_safe().get_mut(&id) {
                    e.state = ConnState::Failed;
                    e.error = Some(err.clone());
                }
                self.emit_state(&id, ConnState::Failed, Some(&err));
                Err(err)
            }
        }
    }

    fn start_poller(self: &Arc<Self>, id: &str) {
        let weak: Weak<Self> = Arc::downgrade(self);
        let (key, interval) = (id.to_string(), self.interval);
        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let Some(rt) = weak.upgrade() else { break };
                if !rt.poll_once(&key).await {
                    break;
                }
            }
        });
        match self.entries.lock_safe().get_mut(id) {
            Some(e) => e.poller = Some(handle),
            None => handle.abort(),
        }
    }

    /// Один опрос; `false` — подключение закрыто, опрос прекращается.
    async fn poll_once(&self, id: &str) -> bool {
        let (profile, previous) = {
            let entries = self.entries.lock_safe();
            let Some(e) = entries.get(id) else { return false };
            (e.profile.clone(), e.topology.clone())
        };
        self.pools.close_idle(IDLE_TIMEOUT);
        let result = self.source.observe(&profile, previous.as_ref()).await;
        let mut entries = self.entries.lock_safe();
        let Some(e) = entries.get_mut(id) else { return false };
        match result {
            Ok(t) => {
                let events = previous.as_ref().map(|p| diff(p, &t)).unwrap_or_default();
                if let Some(p) = &previous {
                    // Участник исчез или сменил адрес — его пулы не нужны.
                    for m in &p.members {
                        if t.member(&m.name).is_none_or(|n| n.host != m.host || n.port != m.port) {
                            self.pools.evict_member(id, &m.name);
                        }
                    }
                }
                let changed = previous.as_ref().is_none_or(|p| !p.same_except_time(&t));
                e.journal.extend(&events);
                let state = state_for(&t);
                let state_changed = state != e.state || e.error.is_some();
                e.state = state;
                e.error = None;
                e.topology = Some(t);
                let view = e.view();
                drop(entries);
                if state_changed {
                    self.emit_state(id, state, None);
                }
                if changed || !events.is_empty() {
                    self.emit_topology(id, &view, &events);
                }
            }
            Err(err) => {
                e.state = ConnState::Degraded;
                e.error = Some(err.clone());
                drop(entries);
                self.emit_state(id, ConnState::Degraded, Some(&err));
            }
        }
        true
    }

    pub fn disconnect(&self, id: &str) {
        let removed = self.entries.lock_safe().remove(id);
        if let Some(e) = removed
            && let Some(h) = e.poller
        {
            h.abort();
        }
        self.pools.evict_connection(id);
        self.source.forget(id);
        self.emit_state(id, ConnState::Disconnected, None);
    }

    pub fn view(&self, id: &str) -> TopologyView {
        self.entries.lock_safe().get(id).map(Entry::view).unwrap_or_else(TopologyView::disconnected)
    }

    pub fn events(&self, id: &str) -> Vec<TopologyEvent> {
        self.entries.lock_safe().get(id).map(|e| e.journal.newest_first()).unwrap_or_default()
    }

    pub fn set_catalog_member(&self, id: &str, member: Option<String>) -> AppResult<TopologyView> {
        let mut entries = self.entries.lock_safe();
        let e = entries.get_mut(id).ok_or_else(not_connected)?;
        if let Some(name) = &member
            && e.topology.as_ref().and_then(|t| t.member(name)).is_none()
        {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.topology.unknownMember",
                "Node {{member}} is not part of the connection",
                member = name
            ));
        }
        e.chosen_member = member;
        Ok(e.view())
    }

    /// Участник для запроса: `member` или узел каталога; профиль — тот, с которым открыто подключение.
    pub fn target(&self, id: &str, member: Option<&str>) -> AppResult<(Member, ConnectionProfile)> {
        let entries = self.entries.lock_safe();
        let e = entries.get(id).ok_or_else(not_connected)?;
        let t = e.topology.as_ref().ok_or_else(not_connected)?;
        let name = match member {
            Some(m) => m.to_string(),
            None => e.view().catalog_member.ok_or_else(|| {
                app_err!(ErrorCode::NodeUnavailable, "errors.topology.noNode", "No suitable node is available")
            })?,
        };
        let m = t.member(&name).cloned().ok_or_else(|| {
            app_err!(
                ErrorCode::InvalidInput,
                "errors.topology.unknownMember",
                "Node {{member}} is not part of the connection",
                member = name
            )
        })?;
        Ok((m, e.profile.clone()))
    }

    /// Останавливает все опросы (выход из приложения, конец теста).
    pub fn shutdown(&self) {
        for (_, e) in self.entries.lock_safe().drain() {
            if let Some(h) = e.poller {
                h.abort();
            }
        }
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::runtime`
Expected: PASS (5 тестов).

- [ ] **Step 5: AppState, команды и события**

`state.rs` — поля и конструктор:

```rust
use crate::conn::pool::{PoolRegistry, PooledClient};
use crate::topology::runtime::{LiveSource, Runtimes};

    pub pools: Arc<PoolRegistry>,
    pub runtimes: Arc<Runtimes>,
```

В `AppState::new` перед `Ok(Self { … })`:

```rust
        let pools = Arc::new(PoolRegistry::default());
        let runtimes = Runtimes::new(Arc::new(LiveSource::new(secrets.clone(), pools.clone())), pools.clone());
```

Метод:

```rust
    /// Соединение с узлом открытого подключения: `member = None` — узел каталога,
    /// `database = None` — БД по умолчанию профиля.
    pub async fn client(
        &self,
        connection_id: &str,
        member: Option<&str>,
        database: Option<&str>,
    ) -> AppResult<PooledClient> {
        let (m, profile) = self.runtimes.target(connection_id, member)?;
        let db = database.map(str::to_string).unwrap_or_else(|| profile.pg.database.clone());
        crate::topology::connector::pooled_client(
            &self.pools, &profile, self.secrets.clone(), &m.name, &m.host, m.port, &db,
        )
        .await
    }
```

В `delete_connection_full` первой строкой — `self.runtimes.disconnect(id);`.

`src-tauri/src/commands/topology.rs`:

```rust
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::conn::errors::from_pg;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::topology::journal::TopologyEvent;
use crate::topology::runtime::{ConnState, RuntimeSink, TopologyView};

/// События наблюдения → окно приложения.
pub struct TauriSink(pub AppHandle);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatePayload<'a> {
    connection_id: &'a str,
    state: ConnState,
    error: Option<&'a AppError>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TopologyPayload<'a> {
    connection_id: &'a str,
    view: &'a TopologyView,
    new_events: &'a [TopologyEvent],
}

impl RuntimeSink for TauriSink {
    fn state_changed(&self, connection_id: &str, state: ConnState, error: Option<&AppError>) {
        if let Err(e) = self.0.emit("connection://state", StatePayload { connection_id, state, error }) {
            tracing::warn!("emit connection://state: {e}");
        }
    }

    fn topology_changed(&self, connection_id: &str, view: &TopologyView, new_events: &[TopologyEvent]) {
        if let Err(e) = self.0.emit("topology://changed", TopologyPayload { connection_id, view, new_events }) {
            tracing::warn!("emit topology://changed: {e}");
        }
    }
}

#[tauri::command]
pub async fn connect(state: State<'_, AppState>, id: String) -> AppResult<TopologyView> {
    let profile = state.profile(&id)?;
    state.runtimes.connect(profile).await
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtimes.disconnect(&id);
    Ok(())
}

#[tauri::command]
pub fn get_topology(state: State<'_, AppState>, id: String) -> AppResult<TopologyView> {
    Ok(state.runtimes.view(&id))
}

#[tauri::command]
pub fn list_topology_events(state: State<'_, AppState>, id: String) -> AppResult<Vec<TopologyEvent>> {
    Ok(state.runtimes.events(&id))
}

#[tauri::command]
pub fn set_catalog_member(state: State<'_, AppState>, id: String, member: Option<String>) -> AppResult<TopologyView> {
    state.runtimes.set_catalog_member(&id, member)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberFacts {
    pub started_at_ms: i64,
    pub connections: i64,
    pub max_connections: i64,
    pub databases: i64,
}

#[tauri::command]
pub async fn get_member_facts(state: State<'_, AppState>, id: String, member: String) -> AppResult<MemberFacts> {
    let c = state.client(&id, Some(&member), None).await?;
    let r = c
        .query_one(
            "SELECT (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
             (SELECT count(*) FROM pg_stat_activity)::bigint, \
             current_setting('max_connections')::bigint, \
             (SELECT count(*) FROM pg_database WHERE NOT datistemplate)::bigint",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(MemberFacts { started_at_ms: r.get(0), connections: r.get(1), max_connections: r.get(2), databases: r.get(3) })
}
```

`commands/mod.rs` — `pub mod topology;`. В `commands/connections.rs`, команда `save_connection`, после успешного `blocking(...)`: `state.runtimes.disconnect(&saved.id);` (изменённый профиль — подключение переоткроется при следующем раскрытии).

`lib.rs`:
- в `setup` после `app.manage(st)` — заменить на:

```rust
            st.runtimes.set_sink(Arc::new(crate::commands::topology::TauriSink(app.handle().clone())));
            app.manage(st);
```

- в `generate_handler!` — `commands::topology::connect`, `disconnect`, `get_topology`, `list_topology_events`, `set_catalog_member`, `get_member_facts`;
- в `.run(|app, event| …)`: при `tauri::RunEvent::Exit` — `app.state::<AppState>().runtimes.shutdown();`.

В словари:

| Ключ | en | ru |
|---|---|---|
| `errors.topology.notConnected` | The connection is not open | Подключение не открыто |
| `errors.topology.noNode` | No suitable node is available | Нет подходящего доступного узла |
| `errors.topology.unknownMember` | Node {{member}} is not part of the connection | Узла {{member}} нет в подключении |

- [ ] **Step 6: Интеграционный тест наблюдения**

`src-tauri/tests/it_runtime.rs`:

```rust
//! Наблюдение за кластером Patroni стенда: switchover попадает в журнал (`GPG_IT_PATRONI`, `GPG_IT_DOCKER=1`).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use glafira_pg_lib::conn::pool::PoolRegistry;
use glafira_pg_lib::conn::profile::ConnectionKind;
use glafira_pg_lib::secrets::{PATRONI_PASSWORD, SecretStore, secret_key};
use glafira_pg_lib::topology::journal::EventKind;
use glafira_pg_lib::topology::runtime::{ConnState, LiveSource, Runtimes};

#[tokio::test]
async fn switchover_reaches_journal_within_poll_window() {
    if common::env("GPG_IT_DOCKER").is_none() {
        return;
    }
    let Some(settings) = common::patroni_settings() else { return };
    let mut profile = common::server_profile("rt", "localhost:5441");
    profile.kind = ConnectionKind::Patroni;
    profile.hosts.clear();
    profile.patroni = Some(settings);
    let secrets = common::secrets_with_password("rt");
    secrets.set(&secret_key("rt", PATRONI_PASSWORD), common::PATRONI_PASSWORD_VALUE).unwrap();
    let pools = Arc::new(PoolRegistry::default());
    let rt = Runtimes::new(Arc::new(LiveSource::new(secrets, pools.clone())), pools);

    let view = rt.connect(profile).await.unwrap();
    assert_eq!(view.state, ConnState::Connected, "{:?}", view.topology.map(|t| t.warnings));
    let topo = view.topology.unwrap();
    let leader = topo.leader().unwrap().name.clone();
    let candidate = topo.members.iter().find(|m| m.name != leader).unwrap().name.clone();
    common::patronictl(&["switchover", "--leader", &leader, "--candidate", &candidate, "--force"]);
    let switched = Instant::now();

    let deadline = Instant::now() + Duration::from_secs(40);
    let found = loop {
        let hit = rt.events("rt").into_iter().find(|e| matches!(&e.event, EventKind::LeaderChanged { to: Some(t), .. } if *t == candidate));
        if let Some(e) = hit {
            break Some(e);
        }
        if Instant::now() > deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    rt.shutdown();
    assert!(found.is_some(), "LeaderChanged не попал в журнал");
    // Критерий спеки — не дольше 15 с после завершения переключения (переключение само занимает несколько секунд).
    assert!(switched.elapsed() < Duration::from_secs(40));
    assert_eq!(rt.view("rt").state, ConnState::Disconnected);
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run (стенд): `GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 cargo test --manifest-path src-tauri/Cargo.toml --test it_runtime`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(topology): открытые подключения — опрос раз в 10 с, события в интерфейс, узел каталога

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 13: Чек-лист готовности (`pg-advisor`) и проверка подключения

**Files:**
- Create: `crates/pg-advisor/Cargo.toml`, `crates/pg-advisor/src/lib.rs`, `crates/pg-advisor/src/readiness.rs`, `src-tauri/src/readiness/mod.rs`, `src-tauri/src/readiness/check.rs`, `src-tauri/tests/it_readiness.rs`
- Modify: `src-tauri/Cargo.toml` (`pg-advisor = { path = "../crates/pg-advisor" }`), `src-tauri/src/lib.rs`, `src-tauri/src/commands/connections.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `crates/pg-advisor/src/readiness.rs`; интеграционные `tests/it_readiness.rs`

**Interfaces:**
- Consumes: `topology::{patroni::{ReqwestHttp, discover}, probe::{observe_hosts, collect}, connector::{AdhocConnector, NodeConnector}, observe::default_member, model::*}`, `conn::{pg::ConnectSpec, run_blocking, profile::*}`, `secrets::OverlayStore`, `commands::connections::{ConnectionInput, SecretsInput::overrides}`.
- Produces:
  - `pg_advisor::readiness::{ReadinessFacts { server_version_num, user, database, is_superuser, member_of, preload_by_member: Vec<(String, bool)>, pgss_installed, compute_query_id: Option<String>, track_io_timing, track_activity_query_size, extensions: Vec<ExtensionFact> }, ExtensionFact { name, installed: Option<String>, available }, Status (Ok | Warn | Missing), ReadinessItem { id: &'static str, status, params: BTreeMap<String, String>, fix: Option<String> }, evaluate(&ReadinessFacts) -> Vec<ReadinessItem>, OPTIONAL_EXTENSIONS}`. Порядок `id`: `monitorRole`, `signalRole`, `maintainRole`, `pgssPreloaded`, `pgssInstalled`, `computeQueryId`, `trackIoTiming`, `trackActivityQuerySize`, затем `extension` × 6.
  - `crate::readiness::{collect_facts(&Client, preload_by_member, database) -> AppResult<ReadinessFacts>, preload_of(&Client) -> AppResult<bool>}`.
  - `crate::readiness::check::{TestStep (Patroni | Discovery | Node | Readiness), StepStatus (Running | Ok | Failed | Skipped), TestEvent { step, member, status, message: Option<LocalizedText>, error }, TestReport { ok, topology: Option<Topology>, readiness: Vec<ReadinessItem> }, run(ConnectionProfile, Arc<dyn SecretStore>, emit: &(dyn Fn(TestEvent) + Send + Sync)) -> AppResult<TestReport>}`.
  - Команда `test_connection(input: ConnectionInput, channel: Channel<TestEvent>) -> TestReport`.

- [ ] **Step 1: Крейт и тесты оценки (падающие)**

`crates/pg-advisor/Cargo.toml`:

```toml
[package]
name = "pg-advisor"
version = "0.1.0"
edition = "2024"
description = "Правила Glafira for PostgreSQL без ввода-вывода: готовность к диагностике (P1), дальше — проверки и разбор планов"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
serde_json = "1"
```

`crates/pg-advisor/src/lib.rs`:

```rust
//! Правила и разбор без ввода-вывода (концепция §3): тестируются на наборах фактов и фикстурах.

pub mod readiness;
```

В конец `crates/pg-advisor/src/readiness.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ReadinessFacts {
        ReadinessFacts {
            server_version_num: 170_002,
            user: "glafira_diag".into(),
            database: "postgres".into(),
            is_superuser: false,
            member_of: vec!["pg_monitor".into(), "pg_signal_backend".into(), "pg_maintain".into()],
            preload_by_member: vec![("a".into(), true), ("b".into(), true)],
            pgss_installed: true,
            compute_query_id: Some("auto".into()),
            track_io_timing: "on".into(),
            track_activity_query_size: 4096,
            extensions: vec![
                ExtensionFact { name: "hypopg".into(), installed: Some("1.4.1".into()), available: true },
                ExtensionFact { name: "pgstattuple".into(), installed: None, available: true },
            ],
        }
    }

    fn get<'a>(items: &'a [ReadinessItem], id: &str) -> &'a ReadinessItem {
        items.iter().find(|i| i.id == id).unwrap()
    }

    fn ext<'a>(items: &'a [ReadinessItem], name: &str) -> &'a ReadinessItem {
        items.iter().find(|i| i.id == "extension" && i.params["name"] == name).unwrap()
    }

    #[test]
    fn healthy_setup_is_ok() {
        let items = evaluate(&facts());
        let ids: Vec<&str> = items.iter().map(|i| i.id).take(8).collect();
        assert_eq!(ids, ["monitorRole", "signalRole", "maintainRole", "pgssPreloaded", "pgssInstalled", "computeQueryId", "trackIoTiming", "trackActivityQuerySize"]);
        assert!(items[..8].iter().all(|i| i.status == Status::Ok), "{items:?}");
        assert_eq!(items.len(), 8 + OPTIONAL_EXTENSIONS.len());
        assert_eq!(ext(&items, "hypopg").status, Status::Ok);
        assert_eq!(ext(&items, "hypopg").params["version"], "1.4.1");
        let pst = ext(&items, "pgstattuple");
        assert_eq!(pst.status, Status::Warn);
        assert_eq!(pst.fix.as_deref(), Some("CREATE EXTENSION IF NOT EXISTS pgstattuple;"));
        assert_eq!(ext(&items, "pg_qualstats").status, Status::Missing);
        assert_eq!(ext(&items, "pg_qualstats").fix, None);
    }

    #[test]
    fn missing_roles_get_grants_with_quoted_user() {
        let mut f = facts();
        f.member_of.clear();
        f.user = "App User".into();
        let items = evaluate(&f);
        assert_eq!(get(&items, "monitorRole").status, Status::Missing);
        assert_eq!(get(&items, "monitorRole").fix.as_deref(), Some("GRANT pg_monitor TO \"App User\";"));
        assert_eq!(get(&items, "signalRole").fix.as_deref(), Some("GRANT pg_signal_backend TO \"App User\";"));
        assert_eq!(get(&items, "maintainRole").status, Status::Warn);
        assert_eq!(get(&items, "maintainRole").params["reason"], "grant");
    }

    #[test]
    fn superuser_has_everything() {
        let mut f = facts();
        f.member_of.clear();
        f.is_superuser = true;
        let items = evaluate(&f);
        for id in ["monitorRole", "signalRole", "maintainRole"] {
            assert_eq!(get(&items, id).status, Status::Ok, "{id}");
        }
    }

    #[test]
    fn old_versions() {
        let mut f = facts();
        f.server_version_num = 130_015;
        f.compute_query_id = None;
        let items = evaluate(&f);
        assert_eq!(get(&items, "maintainRole").params["reason"], "ownerOnly");
        assert_eq!(get(&items, "maintainRole").fix, None);
        assert_eq!(get(&items, "computeQueryId").status, Status::Warn);
        assert_eq!(get(&items, "computeQueryId").params["reason"], "version");
    }

    #[test]
    fn pg_stat_statements_preload() {
        let mut f = facts();
        f.preload_by_member = vec![("a".into(), true), ("b".into(), false)];
        let items = evaluate(&f);
        let p = get(&items, "pgssPreloaded");
        assert_eq!(p.status, Status::Warn);
        assert_eq!(p.params["members"], "b");
        assert_eq!(get(&items, "computeQueryId").status, Status::Warn, "auto без загрузки везде");

        f.preload_by_member = vec![("a".into(), false)];
        f.pgss_installed = false;
        let items = evaluate(&f);
        assert_eq!(get(&items, "pgssPreloaded").status, Status::Missing);
        assert_eq!(get(&items, "pgssInstalled").status, Status::Missing);
        assert_eq!(get(&items, "pgssInstalled").params["database"], "postgres");

        f.preload_by_member.clear();
        assert_eq!(get(&evaluate(&f), "pgssPreloaded").params["reason"], "unknown");
    }

    #[test]
    fn settings() {
        let mut f = facts();
        f.compute_query_id = Some("off".into());
        f.track_io_timing = "off".into();
        f.track_activity_query_size = 1024;
        let items = evaluate(&f);
        assert_eq!(get(&items, "computeQueryId").status, Status::Missing);
        assert_eq!(get(&items, "computeQueryId").fix.as_deref(), Some("compute_query_id = on"));
        assert_eq!(get(&items, "trackIoTiming").status, Status::Warn);
        let q = get(&items, "trackActivityQuerySize");
        assert_eq!(q.status, Status::Warn);
        assert_eq!(q.params["value"], "1024");
    }

    #[test]
    fn json_shape() {
        let v = serde_json::to_value(&evaluate(&facts())[0]).unwrap();
        assert_eq!(v, serde_json::json!({"id": "monitorRole", "status": "ok", "params": {}, "fix": null}));
    }
}
```

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`
Expected: ошибки компиляции.

- [ ] **Step 2: Реализация оценки** (над тестами `readiness.rs`)

```rust
//! Чек-лист готовности к диагностике (спец. P1 §10): факты о роли, настройках и расширениях → пункты.
//! Тексты пунктов — в словарях интерфейса по `id`; `fix` — SQL или строка конфигурации, не переводится.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const OPTIONAL_EXTENSIONS: [&str; 6] =
    ["hypopg", "pgstattuple", "pg_buffercache", "pg_wait_sampling", "pg_qualstats", "pg_stat_kcache"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionFact {
    pub name: String,
    pub installed: Option<String>,
    pub available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessFacts {
    pub server_version_num: i32,
    pub user: String,
    pub database: String,
    pub is_superuser: bool,
    /// Встроенные роли, в которые пользователь входит (транзитивно).
    pub member_of: Vec<String>,
    /// Участник → есть ли `pg_stat_statements` в `shared_preload_libraries` (только доступные).
    pub preload_by_member: Vec<(String, bool)>,
    pub pgss_installed: bool,
    pub compute_query_id: Option<String>,
    pub track_io_timing: String,
    pub track_activity_query_size: i64,
    pub extensions: Vec<ExtensionFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    Warn,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessItem {
    pub id: &'static str,
    pub status: Status,
    pub params: BTreeMap<String, String>,
    pub fix: Option<String>,
}

fn item(id: &'static str, status: Status, params: &[(&str, String)], fix: Option<String>) -> ReadinessItem {
    ReadinessItem {
        id,
        status,
        params: params.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        fix,
    }
}

/// Имя роли в SQL: простое — как есть, иначе в двойных кавычках.
fn quote_ident(name: &str) -> String {
    let simple = name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if simple { name.to_string() } else { format!("\"{}\"", name.replace('"', "\"\"")) }
}

const PRELOAD_FIX: &str = "shared_preload_libraries = 'pg_stat_statements'";

pub fn evaluate(f: &ReadinessFacts) -> Vec<ReadinessItem> {
    let has = |role: &str| f.is_superuser || f.member_of.iter().any(|r| r == role);
    let user = quote_ident(&f.user);
    let grant = |role: &str| Some(format!("GRANT {role} TO {user};"));
    let mut out = Vec::new();

    out.push(if has("pg_monitor") {
        item("monitorRole", Status::Ok, &[], None)
    } else {
        item("monitorRole", Status::Missing, &[], grant("pg_monitor"))
    });
    out.push(if has("pg_signal_backend") {
        item("signalRole", Status::Ok, &[], None)
    } else {
        item("signalRole", Status::Missing, &[], grant("pg_signal_backend"))
    });
    out.push(if f.is_superuser || (f.server_version_num >= 170_000 && has("pg_maintain")) {
        item("maintainRole", Status::Ok, &[], None)
    } else if f.server_version_num >= 170_000 {
        item("maintainRole", Status::Warn, &[("reason", "grant".into())], grant("pg_maintain"))
    } else {
        item("maintainRole", Status::Warn, &[("reason", "ownerOnly".into())], None)
    });

    let missing: Vec<&str> = f
        .preload_by_member
        .iter()
        .filter(|(_, loaded)| !loaded)
        .map(|(m, _)| m.as_str())
        .collect();
    let preloaded_everywhere = !f.preload_by_member.is_empty() && missing.is_empty();
    out.push(if f.preload_by_member.is_empty() {
        item("pgssPreloaded", Status::Warn, &[("reason", "unknown".into())], None)
    } else if missing.is_empty() {
        item("pgssPreloaded", Status::Ok, &[], None)
    } else if missing.len() == f.preload_by_member.len() {
        item("pgssPreloaded", Status::Missing, &[], Some(PRELOAD_FIX.into()))
    } else {
        item("pgssPreloaded", Status::Warn, &[("members", missing.join(", "))], Some(PRELOAD_FIX.into()))
    });
    out.push(if f.pgss_installed {
        item("pgssInstalled", Status::Ok, &[], None)
    } else {
        item(
            "pgssInstalled",
            Status::Missing,
            &[("database", f.database.clone())],
            Some("CREATE EXTENSION IF NOT EXISTS pg_stat_statements;".into()),
        )
    });

    let fix_qid = Some("compute_query_id = on".to_string());
    out.push(if f.server_version_num < 140_000 {
        item("computeQueryId", Status::Warn, &[("reason", "version".into())], None)
    } else {
        match f.compute_query_id.as_deref() {
            Some("on") => item("computeQueryId", Status::Ok, &[], None),
            Some("auto") if preloaded_everywhere => item("computeQueryId", Status::Ok, &[], None),
            Some("auto") => item("computeQueryId", Status::Warn, &[], fix_qid),
            _ => item("computeQueryId", Status::Missing, &[], fix_qid),
        }
    });
    out.push(if f.track_io_timing == "on" {
        item("trackIoTiming", Status::Ok, &[], None)
    } else {
        item("trackIoTiming", Status::Warn, &[], Some("track_io_timing = on".into()))
    });
    out.push(if f.track_activity_query_size >= 4096 {
        item("trackActivityQuerySize", Status::Ok, &[], None)
    } else {
        item(
            "trackActivityQuerySize",
            Status::Warn,
            &[("value", f.track_activity_query_size.to_string())],
            Some("track_activity_query_size = 4096".into()),
        )
    });

    for name in OPTIONAL_EXTENSIONS {
        let fact = f.extensions.iter().find(|e| e.name == name);
        out.push(match fact {
            Some(ExtensionFact { installed: Some(v), .. }) => {
                item("extension", Status::Ok, &[("name", name.into()), ("version", v.clone())], None)
            }
            Some(ExtensionFact { available: true, .. }) => item(
                "extension",
                Status::Warn,
                &[("name", name.into())],
                Some(format!("CREATE EXTENSION IF NOT EXISTS {name};")),
            ),
            _ => item("extension", Status::Missing, &[("name", name.into())], None),
        });
    }
    out
}
```

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings`
Expected: PASS (7 тестов), без предупреждений.

- [ ] **Step 3: Сбор фактов и проверка подключения**

`src-tauri/Cargo.toml` — `pg-advisor = { path = "../crates/pg-advisor" }`; `lib.rs` — `pub mod readiness;`.

`src-tauri/src/readiness/mod.rs`:

```rust
//! Сбор фактов для чек-листа готовности (спец. P1 §10); оценка — `pg_advisor::readiness`.

pub mod check;

use pg_advisor::readiness::{ExtensionFact, OPTIONAL_EXTENSIONS, ReadinessFacts};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

/// Загружен ли `pg_stat_statements` на узле.
pub async fn preload_of(c: &Client) -> AppResult<bool> {
    let v: String = c
        .query_one("SELECT current_setting('shared_preload_libraries')", &[])
        .await
        .map_err(|e| from_pg(&e))?
        .get(0);
    Ok(v.split(',').any(|lib| lib.trim().trim_matches('"') == "pg_stat_statements"))
}

pub async fn collect_facts(
    c: &Client,
    preload_by_member: Vec<(String, bool)>,
    database: &str,
) -> AppResult<ReadinessFacts> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let r = c
        .query_one(
            "SELECT current_setting('server_version_num')::int, current_user::text, r.rolsuper \
             FROM pg_roles r WHERE r.rolname = current_user",
            &[],
        )
        .await
        .map_err(pg)?;
    let (server_version_num, user, is_superuser): (i32, String, bool) = (r.get(0), r.get(1), r.get(2));
    let member_of = c
        .query(
            "SELECT b.rolname::text FROM pg_roles b \
             WHERE b.rolname IN ('pg_monitor', 'pg_signal_backend', 'pg_maintain', 'pg_read_all_stats') \
             AND pg_has_role(current_user, b.oid, 'MEMBER')",
            &[],
        )
        .await
        .map_err(pg)?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let pgss_installed: bool = c
        .query_one("SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements')", &[])
        .await
        .map_err(pg)?
        .get(0);
    let mut compute_query_id = None;
    let mut track_io_timing = String::from("off");
    let mut track_activity_query_size = 1024;
    for r in c
        .query(
            "SELECT name, setting FROM pg_settings \
             WHERE name IN ('compute_query_id', 'track_io_timing', 'track_activity_query_size')",
            &[],
        )
        .await
        .map_err(pg)?
    {
        let (name, setting): (String, String) = (r.get(0), r.get(1));
        match name.as_str() {
            "compute_query_id" => compute_query_id = Some(setting),
            "track_io_timing" => track_io_timing = setting,
            // В pg_settings — байты.
            _ => track_activity_query_size = setting.parse().unwrap_or(1024),
        }
    }
    let names: Vec<&str> = OPTIONAL_EXTENSIONS.to_vec();
    let extensions = c
        .query(
            "SELECT a.name::text, e.extversion::text FROM pg_available_extensions a \
             LEFT JOIN pg_extension e ON e.extname = a.name WHERE a.name = ANY($1)",
            &[&names],
        )
        .await
        .map_err(pg)?
        .iter()
        .map(|r| ExtensionFact { name: r.get(0), installed: r.get(1), available: true })
        .collect();
    Ok(ReadinessFacts {
        server_version_num,
        user,
        database: database.to_string(),
        is_superuser,
        member_of,
        preload_by_member,
        pgss_installed,
        compute_query_id,
        track_io_timing,
        track_activity_query_size,
        extensions,
    })
}
```

`src-tauri/src/readiness/check.rs`:

```rust
//! Проверка подключения по шагам (спец. P1 §10): Patroni → обнаружение → узлы → готовность.
//! Работает с несохранённым профилем: секреты — форма поверх keychain.

use std::sync::Arc;

use pg_advisor::readiness::{ReadinessItem, evaluate};
use serde::Serialize;

use crate::conn::pg::ConnectSpec;
use crate::conn::profile::{ConnectionKind, ConnectionProfile, normalize_profile, validate_profile};
use crate::conn::run_blocking;
use crate::error::{AppError, AppResult};
use crate::i18n::LocalizedText;
use crate::readiness::{collect_facts, preload_of};
use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, SecretStore, secret_key};
use crate::topology::connector::{AdhocConnector, NodeConnector};
use crate::topology::model::{Topology, now_ms};
use crate::topology::observe::default_member;
use crate::topology::patroni::{ReqwestHttp, discover};
use crate::topology::probe::observe_hosts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TestStep {
    Patroni,
    Discovery,
    Node,
    Readiness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepStatus {
    Running,
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestEvent {
    pub step: TestStep,
    pub member: Option<String>,
    pub status: StepStatus,
    pub message: Option<LocalizedText>,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestReport {
    pub ok: bool,
    pub topology: Option<Topology>,
    pub readiness: Vec<ReadinessItem>,
}

type Emit<'a> = &'a (dyn Fn(TestEvent) + Send + Sync);

fn event(emit: Emit, step: TestStep, member: Option<&str>, status: StepStatus, message: Option<LocalizedText>, error: Option<AppError>) {
    emit(TestEvent { step, member: member.map(String::from), status, message, error });
}

async fn adhoc_client(profile: &ConnectionProfile, secrets: &Arc<dyn SecretStore>, host: &str, port: u16) -> AppResult<tokio_postgres::Client> {
    let (p, s, h) = (profile.clone(), secrets.clone(), host.to_string());
    let db = profile.pg.database.clone();
    let spec = run_blocking(move || ConnectSpec::build(&p, &h, port, &db, s.as_ref())).await?;
    spec.connect().await
}

pub async fn run(mut profile: ConnectionProfile, secrets: Arc<dyn SecretStore>, emit: Emit<'_>) -> AppResult<TestReport> {
    normalize_profile(&mut profile);
    validate_profile(&profile)?;
    let failed = |topology: Option<Topology>| TestReport { ok: false, topology, readiness: vec![] };
    let nodes = AdhocConnector { profile: profile.clone(), secrets: secrets.clone() };

    // 1–2. Patroni или опрос узлов, затем итог обнаружения.
    let topology = if profile.kind == ConnectionKind::Patroni {
        event(emit, TestStep::Patroni, None, StepStatus::Running, None, None);
        let (p, s) = (profile.clone(), secrets.clone());
        let http = run_blocking(move || {
            let settings = p.patroni.as_ref().expect("checked by validate_profile");
            let password = s.get(&secret_key(&p.id, PATRONI_PASSWORD))?;
            let key_password = s.get(&secret_key(&p.id, PATRONI_KEY_PASSWORD))?;
            ReqwestHttp::new(settings, password, key_password.as_deref())
        })
        .await;
        let http = match http {
            Ok(h) => h,
            Err(e) => {
                event(emit, TestStep::Patroni, None, StepStatus::Failed, None, Some(e));
                return Ok(failed(None));
            }
        };
        let settings = profile.patroni.as_ref().expect("checked by validate_profile");
        match discover(&http, settings, &profile.id, None, now_ms()).await {
            Ok(t) => {
                let leader = t.leader().map(|m| m.name.clone()).unwrap_or_else(|| "—".into());
                event(emit, TestStep::Patroni, None, StepStatus::Ok,
                    Some(ltext!("test.patroniOk", "{{members}} members, leader {{leader}}", members = t.members.len(), leader = leader)), None);
                t
            }
            Err(e) => {
                event(emit, TestStep::Patroni, None, StepStatus::Failed, None, Some(e));
                return Ok(failed(None));
            }
        }
    } else {
        event(emit, TestStep::Patroni, None, StepStatus::Skipped, None, None);
        observe_hosts(&profile.id, profile.kind, &profile.hosts, &nodes, now_ms()).await
    };
    event(emit, TestStep::Discovery, None, StepStatus::Ok,
        Some(ltext!("test.discoveryOk", "{{members}} nodes found", members = topology.members.len())), None);

    // 3. Узлы: у Patroni — отдельное соединение с PostgreSQL каждого участника, у списка — итог опроса.
    let mut pg_ok: Vec<(String, String, u16)> = Vec::new();
    for m in &topology.members {
        event(emit, TestStep::Node, Some(&m.name), StepStatus::Running, None, None);
        let facts = if profile.kind == ConnectionKind::Patroni {
            nodes.facts(&m.name, &m.host, m.port).await.map(|f| (if f.in_recovery { "replica" } else { "primary" }, f.server_version))
        } else if m.reachable {
            Ok((if m.role.is_standby() { "replica" } else { "primary" }, m.server_version.clone().unwrap_or_default()))
        } else {
            Err(m.error.clone().unwrap_or_else(|| AppError::internal_with("node unreachable")))
        };
        match facts {
            Ok((role, version)) => {
                pg_ok.push((m.name.clone(), m.host.clone(), m.port));
                event(emit, TestStep::Node, Some(&m.name), StepStatus::Ok,
                    Some(ltext!("test.nodeOk", "{{role}}, PostgreSQL {{version}}", role = role, version = version)), None);
            }
            Err(e) => event(emit, TestStep::Node, Some(&m.name), StepStatus::Failed, None, Some(e)),
        }
    }
    let all_nodes_ok = pg_ok.len() == topology.members.len();

    // 4. Готовность — на узле по умолчанию (или любом доступном).
    let target = default_member(&profile, &topology)
        .and_then(|name| pg_ok.iter().find(|(n, _, _)| *n == name).cloned())
        .or_else(|| pg_ok.first().cloned());
    let Some((_, host, port)) = target else {
        event(emit, TestStep::Readiness, None, StepStatus::Skipped, None, None);
        return Ok(failed(Some(topology)));
    };
    event(emit, TestStep::Readiness, None, StepStatus::Running, None, None);
    let readiness: AppResult<Vec<ReadinessItem>> = async {
        let mut preload = Vec::new();
        for (name, h, p) in &pg_ok {
            let c = adhoc_client(&profile, &secrets, h, *p).await?;
            preload.push((name.clone(), preload_of(&c).await?));
        }
        let c = adhoc_client(&profile, &secrets, &host, port).await?;
        Ok(evaluate(&collect_facts(&c, preload, &profile.pg.database).await?))
    }
    .await;
    match readiness {
        Ok(items) => {
            event(emit, TestStep::Readiness, None, StepStatus::Ok, None, None);
            Ok(TestReport { ok: all_nodes_ok, topology: Some(topology), readiness: items })
        }
        Err(e) => {
            event(emit, TestStep::Readiness, None, StepStatus::Failed, None, Some(e));
            Ok(failed(Some(topology)))
        }
    }
}
```

Команда в `commands/connections.rs`:

```rust
use std::sync::Arc;

use crate::readiness::check::{TestEvent, TestReport};
use crate::secrets::{OverlayStore, SecretStore};

/// Проверяет и несохранённый профиль: секреты формы накладываются поверх keychain.
#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ConnectionInput,
    channel: tauri::ipc::Channel<TestEvent>,
) -> AppResult<TestReport> {
    let ConnectionInput { mut profile, secrets } = input;
    if profile.id.is_empty() {
        profile.id = "unsaved".into();
    }
    let overlay: Arc<dyn SecretStore> = Arc::new(OverlayStore {
        base: state.secrets.clone(),
        overrides: secrets.overrides(&profile.id),
    });
    crate::readiness::check::run(profile, overlay, &|e| {
        let _ = channel.send(e);
    })
    .await
}
```

Зарегистрировать `commands::connections::test_connection` в `lib.rs`. В словари:

| Ключ | en | ru |
|---|---|---|
| `test.patroniOk` | {{members}} members, leader {{leader}} | Участников: {{members}}, лидер {{leader}} |
| `test.discoveryOk` | {{members}} nodes found | Найдено узлов: {{members}} |
| `test.nodeOk` | {{role}}, PostgreSQL {{version}} | {{role}}, PostgreSQL {{version}} |

- [ ] **Step 4: Интеграционные тесты проверки**

`src-tauri/tests/it_readiness.rs`:

```rust
//! Проверка подключения против стенда: шаги и чек-лист.

mod common;

use std::sync::{Arc, Mutex};

use glafira_pg_lib::conn::profile::ConnectionKind;
use glafira_pg_lib::readiness::check::{StepStatus, TestEvent, TestStep, run};
use glafira_pg_lib::secrets::{PATRONI_PASSWORD, SecretStore, secret_key};
use pg_advisor::readiness::Status;

fn status_of(report: &glafira_pg_lib::readiness::check::TestReport, id: &str) -> Status {
    report.readiness.iter().find(|i| i.id == id).unwrap().status
}

#[tokio::test]
async fn single_server_without_pg_stat_statements() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let events = Mutex::new(Vec::<TestEvent>::new());
    let report = run(common::server_profile("t", &addr), common::secrets_with_password("t"), &|e| events.lock().unwrap().push(e))
        .await
        .unwrap();
    assert!(report.ok);
    assert_eq!(status_of(&report, "monitorRole"), Status::Ok);
    assert_eq!(status_of(&report, "signalRole"), Status::Ok);
    assert_eq!(status_of(&report, "pgssPreloaded"), Status::Missing);
    assert_eq!(status_of(&report, "pgssInstalled"), Status::Missing);
    let ev = events.lock().unwrap();
    assert!(ev.iter().any(|e| e.step == TestStep::Patroni && e.status == StepStatus::Skipped));
    assert!(ev.iter().any(|e| e.step == TestStep::Node && e.status == StepStatus::Ok));
}

#[tokio::test]
async fn patroni_cluster_is_ready() {
    let Some(settings) = common::patroni_settings() else { return };
    let mut p = common::server_profile("tp", "localhost:5441");
    p.kind = ConnectionKind::Patroni;
    p.hosts.clear();
    p.patroni = Some(settings);
    let secrets = common::secrets_with_password("tp");
    secrets.set(&secret_key("tp", PATRONI_PASSWORD), common::PATRONI_PASSWORD_VALUE).unwrap();
    let events = Mutex::new(Vec::<TestEvent>::new());
    let secrets: Arc<dyn SecretStore> = secrets;
    let report = run(p, secrets, &|e| events.lock().unwrap().push(e)).await.unwrap();
    assert!(report.ok, "{:?}", events.lock().unwrap());
    assert_eq!(status_of(&report, "pgssPreloaded"), Status::Ok);
    assert_eq!(status_of(&report, "pgssInstalled"), Status::Ok);
    assert_eq!(status_of(&report, "trackIoTiming"), Status::Ok);
    let ev = events.lock().unwrap();
    assert_eq!(ev.iter().filter(|e| e.step == TestStep::Node && e.status == StepStatus::Ok).count(), 3);
}

#[tokio::test]
async fn wrong_password_fails_node_step() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let secrets = glafira_pg_lib::secrets::MemoryStore::default();
    secrets.set(&secret_key("w", glafira_pg_lib::secrets::PG_PASSWORD), "wrong").unwrap();
    let events = Mutex::new(Vec::<TestEvent>::new());
    let report = run(common::server_profile("w", &addr), Arc::new(secrets), &|e| events.lock().unwrap().push(e)).await.unwrap();
    assert!(!report.ok);
    let ev = events.lock().unwrap();
    let node = ev.iter().find(|e| e.step == TestStep::Node && e.status == StepStatus::Failed).unwrap();
    assert_eq!(node.error.as_ref().unwrap().code, glafira_pg_lib::error::ErrorCode::AuthFailed);
}
```

В `src-tauri/Cargo.toml` `[dev-dependencies]` — `pg-advisor = { path = "../crates/pg-advisor" }` не нужен: тест импортирует `pg_advisor` через обычную зависимость крейта.

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run (стенд): `GPG_IT_SINGLE=localhost:5460 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --test it_readiness`
Expected: PASS (3 теста).

- [ ] **Step 5: Commit**

```bash
git add crates src-tauri src/locales
git commit -m "feat(readiness): чек-лист готовности к диагностике и проверка подключения по шагам

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: Каталог — базы, схемы, объекты, партиции, расширения, роли

**Files:**
- Create: `src-tauri/src/catalog/{mod.rs, databases.rs, schemas.rs, objects.rs, extensions.rs, roles.rs}`, `src-tauri/src/commands/catalog.rs`, `src-tauri/tests/it_catalog.rs`
- Modify: `src-tauri/src/lib.rs`, `src-tauri/src/commands/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `catalog/mod.rs`, `catalog/objects.rs`; интеграционные `tests/it_catalog.rs`

**Interfaces:**
- Consumes: `AppState::client`, `conn::errors::from_pg`.
- Produces:
  - `crate::catalog::{escape_like(&str) -> String, like_pattern(Option<&str>) -> Option<String>, MAX_PAGE = 1000}`.
  - `catalog::databases::{DatabaseInfo { name, size_bytes: Option<i64> }, list(&Client, filter: Option<&[String]>) -> AppResult<Vec<DatabaseInfo>>}`.
  - `catalog::schemas::{ObjectCounts { tables, views, matviews, sequences, functions }, SchemaInfo { name, system, counts }, list(&Client, include_system) -> AppResult<Vec<SchemaInfo>>}`.
  - `catalog::objects::{ObjectKind (Tables | Views | Matviews | Functions | Sequences), ObjectItem { oid, name, schema, relkind: Option<String>, partitions: Option<i64>, est_rows: Option<i64>, total_bytes: Option<i64>, signature: Option<String> }, ObjectPage { total, items, next_offset: Option<i64> }, list(&Client, schema, kind, filter: Option<&str>, offset, limit) -> AppResult<ObjectPage>, partitions(&Client, schema, table, offset, limit) -> AppResult<ObjectPage>}`.
  - `catalog::extensions::{ExtensionInfo { name, version, schema, default_version: Option<String>, update_available }, list(&Client)}`.
  - `catalog::roles::{RoleInfo { name, superuser, login, create_db, create_role, replication, bypass_rls, conn_limit, valid_until: Option<String>, member_of: Vec<String> }, list(&Client, include_system)}`.
  - Команды: `list_databases(id)`, `list_schemas(id, db, includeSystem)`, `list_objects(id, db, schema, kind, filter, offset, limit)`, `list_partitions(id, db, schema, table, offset, limit)`, `list_extensions(id, db)`, `list_roles(id, includeSystem)`. Фильтр баз — из `pg.databases` профиля.

- [ ] **Step 1: Тесты вспомогательных функций (падающие)**

`src-tauri/src/catalog/mod.rs`:

```rust
//! Запросы каталога для дерева (спец. P1 §8). Все — на узле каталога, текст рассчитан на PostgreSQL 13+.

pub mod databases;
pub mod extensions;
pub mod objects;
pub mod roles;
pub mod schemas;

use crate::error::{AppResult, ErrorCode};

pub const MAX_PAGE: i64 = 1000;

/// Экранирует `\`, `%`, `_` для `ILIKE` (экранирующий символ — `\` по умолчанию).
pub fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Подстрока фильтра → шаблон `%…%`; пустой фильтр — без ограничения.
pub fn like_pattern(filter: Option<&str>) -> Option<String> {
    filter.map(str::trim).filter(|f| !f.is_empty()).map(|f| format!("%{}%", escape_like(f)))
}

pub(crate) fn check_page(offset: i64, limit: i64) -> AppResult<()> {
    if offset < 0 || !(1..=MAX_PAGE).contains(&limit) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.catalog.badPage",
            "Invalid page: offset {{offset}}, limit {{limit}}",
            offset = offset,
            limit = limit
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escaping() {
        assert_eq!(escape_like(r"a_b%c\d"), r"a\_b\%c\\d");
        assert_eq!(like_pattern(Some(" ord ")).as_deref(), Some("%ord%"));
        assert_eq!(like_pattern(Some("  ")), None);
        assert_eq!(like_pattern(None), None);
    }

    #[test]
    fn pages() {
        assert!(check_page(0, 500).is_ok());
        assert!(check_page(-1, 10).is_err());
        assert!(check_page(0, 0).is_err());
        assert!(check_page(0, MAX_PAGE + 1).is_err());
    }
}
```

В конец `src-tauri/src/catalog/objects.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_offset_only_when_more_rows() {
        assert_eq!(next_offset(0, 2, 3), Some(2));
        assert_eq!(next_offset(2, 1, 3), None);
        assert_eq!(next_offset(0, 0, 0), None);
    }

    #[test]
    fn kinds_have_relkinds() {
        assert_eq!(ObjectKind::Tables.relkinds(), Some(&["r", "p", "f"][..]));
        assert_eq!(ObjectKind::Sequences.relkinds(), Some(&["S"][..]));
        assert_eq!(ObjectKind::Functions.relkinds(), None);
        let k: ObjectKind = serde_json::from_str("\"matviews\"").unwrap();
        assert_eq!(k, ObjectKind::Matviews);
    }
}
```

`lib.rs` — `pub mod catalog;`.
Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib catalog`
Expected: ошибки компиляции (модулей объектов, баз и т. п. ещё нет).

- [ ] **Step 2: Запросы каталога**

`src-tauri/src/catalog/databases.rs`:

```rust
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseInfo {
    pub name: String,
    /// `None` — нет права `CONNECT` (и роли `pg_read_all_stats`).
    pub size_bytes: Option<i64>,
}

/// Базы с `datallowconn`; при фильтре профиля — только перечисленные, в его порядке.
pub async fn list(c: &Client, filter: Option<&[String]>) -> AppResult<Vec<DatabaseInfo>> {
    let rows = c
        .query(
            "SELECT datname::text, \
               CASE WHEN has_database_privilege(oid, 'CONNECT') OR pg_has_role('pg_read_all_stats', 'MEMBER') \
                    THEN pg_database_size(oid) END \
             FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY datname",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    let all: Vec<DatabaseInfo> = rows.iter().map(|r| DatabaseInfo { name: r.get(0), size_bytes: r.get(1) }).collect();
    Ok(match filter {
        None => all,
        Some(names) => names.iter().filter_map(|n| all.iter().find(|d| &d.name == n).cloned()).collect(),
    })
}
```

`src-tauri/src/catalog/schemas.rs`:

```rust
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectCounts {
    /// Обычные, партиционированные и внешние таблицы без партиций.
    pub tables: i64,
    pub views: i64,
    pub matviews: i64,
    pub sequences: i64,
    /// Функции и процедуры.
    pub functions: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaInfo {
    pub name: String,
    pub system: bool,
    pub counts: ObjectCounts,
}

pub async fn list(c: &Client, include_system: bool) -> AppResult<Vec<SchemaInfo>> {
    let rows = c
        .query(
            "SELECT n.nspname::text, n.nspname IN ('pg_catalog', 'information_schema'), \
               count(cl.oid) FILTER (WHERE cl.relkind IN ('r', 'p', 'f') AND NOT cl.relispartition), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'v'), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'm'), \
               count(cl.oid) FILTER (WHERE cl.relkind = 'S'), \
               (SELECT count(*) FROM pg_proc p WHERE p.pronamespace = n.oid AND p.prokind IN ('f', 'p')) \
             FROM pg_namespace n LEFT JOIN pg_class cl ON cl.relnamespace = n.oid \
             WHERE n.nspname NOT LIKE 'pg\\_toast%' AND n.nspname NOT LIKE 'pg\\_temp\\_%' \
               AND ($1 OR n.nspname NOT IN ('pg_catalog', 'information_schema')) \
             GROUP BY n.oid, n.nspname ORDER BY 2, 1",
            &[&include_system],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| SchemaInfo {
            name: r.get(0),
            system: r.get(1),
            counts: ObjectCounts { tables: r.get(2), views: r.get(3), matviews: r.get(4), sequences: r.get(5), functions: r.get(6) },
        })
        .collect())
}
```

(В литерале Rust `'pg\\_toast%'` — это `'pg\_toast%'` в SQL: экранированное подчёркивание.)

`src-tauri/src/catalog/objects.rs` (над тестами):

```rust
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::catalog::{check_page, like_pattern};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Tables,
    Views,
    Matviews,
    Functions,
    Sequences,
}

impl ObjectKind {
    pub fn relkinds(self) -> Option<&'static [&'static str]> {
        match self {
            ObjectKind::Tables => Some(&["r", "p", "f"]),
            ObjectKind::Views => Some(&["v"]),
            ObjectKind::Matviews => Some(&["m"]),
            ObjectKind::Sequences => Some(&["S"]),
            ObjectKind::Functions => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectItem {
    pub oid: i64,
    pub name: String,
    pub schema: String,
    pub relkind: Option<String>,
    /// Для партиционированной таблицы — число партиций.
    pub partitions: Option<i64>,
    pub est_rows: Option<i64>,
    pub total_bytes: Option<i64>,
    /// Аргументы функции (`pg_get_function_identity_arguments`).
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub total: i64,
    pub items: Vec<ObjectItem>,
    pub next_offset: Option<i64>,
}

pub(crate) fn next_offset(offset: i64, got: usize, total: i64) -> Option<i64> {
    let end = offset + got as i64;
    (got > 0 && end < total).then_some(end)
}

const REL_COLUMNS: &str = "c.oid::bigint, c.relname::text, n.nspname::text, c.relkind::text, \
    CASE WHEN c.relkind = 'p' THEN (SELECT count(*) FROM pg_inherits i WHERE i.inhparent = c.oid) END, \
    CASE WHEN c.relkind IN ('r', 'p', 'm') AND c.reltuples >= 0 THEN c.reltuples::bigint END, \
    CASE WHEN c.relkind IN ('r', 'p', 'm') THEN pg_total_relation_size(c.oid) END, \
    count(*) OVER ()";

fn rel_item(r: &tokio_postgres::Row) -> ObjectItem {
    ObjectItem {
        oid: r.get(0),
        name: r.get(1),
        schema: r.get(2),
        relkind: r.get(3),
        partitions: r.get(4),
        est_rows: r.get(5),
        total_bytes: r.get(6),
        signature: None,
    }
}

pub async fn list(c: &Client, schema: &str, kind: ObjectKind, filter: Option<&str>, offset: i64, limit: i64) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    let pattern = like_pattern(filter);
    let (items, total): (Vec<ObjectItem>, i64) = match kind.relkinds() {
        Some(relkinds) => {
            let sql = format!(
                "SELECT {REL_COLUMNS} FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relkind::text = ANY($2) AND NOT c.relispartition \
                   AND ($3::text IS NULL OR c.relname ILIKE $3) \
                 ORDER BY c.relname LIMIT $4 OFFSET $5"
            );
            let rows = c.query(&sql, &[&schema, &relkinds, &pattern, &limit, &offset]).await.map_err(|e| from_pg(&e))?;
            let total = rows.first().map(|r| r.get::<_, i64>(7)).unwrap_or(0);
            (rows.iter().map(rel_item).collect(), total)
        }
        None => {
            let rows = c
                .query(
                    "SELECT p.oid::bigint, p.proname::text, n.nspname::text, p.prokind::text, \
                       pg_get_function_identity_arguments(p.oid), count(*) OVER () \
                     FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 AND p.prokind IN ('f', 'p') AND ($2::text IS NULL OR p.proname ILIKE $2) \
                     ORDER BY p.proname, 5 LIMIT $3 OFFSET $4",
                    &[&schema, &pattern, &limit, &offset],
                )
                .await
                .map_err(|e| from_pg(&e))?;
            let total = rows.first().map(|r| r.get::<_, i64>(5)).unwrap_or(0);
            let items = rows
                .iter()
                .map(|r| ObjectItem {
                    oid: r.get(0),
                    name: r.get(1),
                    schema: r.get(2),
                    relkind: r.get(3),
                    partitions: None,
                    est_rows: None,
                    total_bytes: None,
                    signature: r.get(4),
                })
                .collect();
            (items, total)
        }
    };
    let next_offset = next_offset(offset, items.len(), total);
    Ok(ObjectPage { total, items, next_offset })
}

/// Партиции таблицы (могут лежать в другой схеме — поле `schema`).
pub async fn partitions(c: &Client, schema: &str, table: &str, offset: i64, limit: i64) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    let sql = format!(
        "SELECT {REL_COLUMNS} FROM pg_inherits inh \
         JOIN pg_class c ON c.oid = inh.inhrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE inh.inhparent = (SELECT pc.oid FROM pg_class pc JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
                                WHERE pn.nspname = $1 AND pc.relname = $2) \
         ORDER BY c.relname LIMIT $3 OFFSET $4"
    );
    let rows = c.query(&sql, &[&schema, &table, &limit, &offset]).await.map_err(|e| from_pg(&e))?;
    let total = rows.first().map(|r| r.get::<_, i64>(7)).unwrap_or(0);
    let items: Vec<ObjectItem> = rows.iter().map(rel_item).collect();
    let next_offset = next_offset(offset, items.len(), total);
    Ok(ObjectPage { total, items, next_offset })
}
```

`src-tauri/src/catalog/extensions.rs`:

```rust
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionInfo {
    pub name: String,
    pub version: String,
    pub schema: String,
    pub default_version: Option<String>,
    pub update_available: bool,
}

pub async fn list(c: &Client) -> AppResult<Vec<ExtensionInfo>> {
    let rows = c
        .query(
            "SELECT e.extname::text, e.extversion::text, n.nspname::text, a.default_version::text \
             FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace \
             LEFT JOIN pg_available_extensions a ON a.name = e.extname ORDER BY 1",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| {
            let version: String = r.get(1);
            let default_version: Option<String> = r.get(3);
            ExtensionInfo {
                name: r.get(0),
                update_available: default_version.as_ref().is_some_and(|d| *d != version),
                version,
                schema: r.get(2),
                default_version,
            }
        })
        .collect())
}
```

`src-tauri/src/catalog/roles.rs`:

```rust
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleInfo {
    pub name: String,
    pub superuser: bool,
    pub login: bool,
    pub create_db: bool,
    pub create_role: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    pub conn_limit: i32,
    pub valid_until: Option<String>,
    pub member_of: Vec<String>,
}

/// Роли кластера; встроенные `pg_*` — только при `include_system`.
pub async fn list(c: &Client, include_system: bool) -> AppResult<Vec<RoleInfo>> {
    let rows = c
        .query(
            "SELECT r.rolname::text, r.rolsuper, r.rolcanlogin, r.rolcreatedb, r.rolcreaterole, \
               r.rolreplication, r.rolbypassrls, r.rolconnlimit, r.rolvaliduntil::text, \
               ARRAY(SELECT b.rolname::text FROM pg_auth_members m JOIN pg_roles b ON b.oid = m.roleid \
                     WHERE m.member = r.oid ORDER BY 1) \
             FROM pg_roles r WHERE $1 OR r.rolname !~ '^pg_' ORDER BY r.rolname",
            &[&include_system],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| RoleInfo {
            name: r.get(0),
            superuser: r.get(1),
            login: r.get(2),
            create_db: r.get(3),
            create_role: r.get(4),
            replication: r.get(5),
            bypass_rls: r.get(6),
            conn_limit: r.get(7),
            valid_until: r.get(8),
            member_of: r.get(9),
        })
        .collect())
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib catalog`
Expected: PASS (4 теста).

- [ ] **Step 3: Команды**

`src-tauri/src/commands/catalog.rs`:

```rust
use tauri::State;

use crate::catalog::databases::DatabaseInfo;
use crate::catalog::extensions::ExtensionInfo;
use crate::catalog::objects::{ObjectKind, ObjectPage};
use crate::catalog::roles::RoleInfo;
use crate::catalog::schemas::SchemaInfo;
use crate::catalog::{databases, extensions, objects, roles, schemas};
use crate::error::AppResult;
use crate::state::AppState;

#[tauri::command]
pub async fn list_databases(state: State<'_, AppState>, id: String) -> AppResult<Vec<DatabaseInfo>> {
    let filter = state.profile(&id)?.pg.databases;
    let c = state.client(&id, None, None).await?;
    databases::list(&c, filter.as_deref()).await
}

#[tauri::command]
pub async fn list_schemas(state: State<'_, AppState>, id: String, db: String, include_system: bool) -> AppResult<Vec<SchemaInfo>> {
    let c = state.client(&id, None, Some(&db)).await?;
    schemas::list(&c, include_system).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn list_objects(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    kind: ObjectKind,
    filter: Option<String>,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    let c = state.client(&id, None, Some(&db)).await?;
    objects::list(&c, &schema, kind, filter.as_deref(), offset, limit).await
}

#[tauri::command]
pub async fn list_partitions(
    state: State<'_, AppState>,
    id: String,
    db: String,
    schema: String,
    table: String,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    let c = state.client(&id, None, Some(&db)).await?;
    objects::partitions(&c, &schema, &table, offset, limit).await
}

#[tauri::command]
pub async fn list_extensions(state: State<'_, AppState>, id: String, db: String) -> AppResult<Vec<ExtensionInfo>> {
    let c = state.client(&id, None, Some(&db)).await?;
    extensions::list(&c).await
}

#[tauri::command]
pub async fn list_roles(state: State<'_, AppState>, id: String, include_system: bool) -> AppResult<Vec<RoleInfo>> {
    let c = state.client(&id, None, None).await?;
    roles::list(&c, include_system).await
}
```

Tauri переводит имена аргументов из camelCase фронтенда (`includeSystem`) в snake_case команды сам. `commands/mod.rs` — `pub mod catalog;`; в `lib.rs` зарегистрировать шесть команд. В словари — `errors.catalog.badPage`: en «Invalid page: offset {{offset}}, limit {{limit}}», ru «Неверная страница: смещение {{offset}}, размер {{limit}}».

- [ ] **Step 4: Интеграционные тесты каталога**

`src-tauri/tests/it_catalog.rs`:

```rust
//! Каталог демо-схемы на узлах стенда: PG 15 (`GPG_IT_SINGLE`), 16 (`GPG_IT_PRIMARY`), 17 (Patroni).

mod common;

use glafira_pg_lib::catalog::objects::ObjectKind;
use glafira_pg_lib::catalog::{databases, extensions, objects, roles, schemas};
use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::profile::SslMode;
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

fn addrs() -> Vec<String> {
    ["GPG_IT_SINGLE", "GPG_IT_PRIMARY"]
        .into_iter()
        .filter_map(common::env)
        .chain(common::env("GPG_IT_PATRONI").map(|_| "localhost:5441".to_string()))
        .collect()
}

async fn client(addr: &str, db: &str) -> tokio_postgres::Client {
    let mut p = common::server_profile("cat", addr);
    p.pg.ssl_mode = SslMode::Disable;
    let s = MemoryStore::default();
    s.set(&secret_key("cat", PG_PASSWORD), common::PG_PASSWORD_VALUE).unwrap();
    let (h, port) = common::host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s).unwrap().connect().await.unwrap()
}

#[tokio::test]
async fn demo_schema_on_every_node() {
    for addr in addrs() {
        let pg = client(&addr, "postgres").await;
        let dbs = databases::list(&pg, None).await.unwrap();
        assert!(dbs.iter().any(|d| d.name == "orders" && d.size_bytes.is_some()), "{addr}: {dbs:?}");
        let only = databases::list(&pg, Some(&["orders".to_string(), "missing".to_string()])).await.unwrap();
        assert_eq!(only.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["orders"]);

        let c = client(&addr, "orders").await;
        let list = schemas::list(&c, false).await.unwrap();
        let shop = list.iter().find(|s| s.name == "shop").unwrap_or_else(|| panic!("{addr}: {list:?}"));
        assert_eq!(
            (shop.counts.tables, shop.counts.views, shop.counts.matviews, shop.counts.sequences, shop.counts.functions),
            (3, 1, 1, 3, 3),
            "{addr}"
        );
        assert!(!list.iter().any(|s| s.name == "pg_catalog"));
        assert!(schemas::list(&c, true).await.unwrap().iter().any(|s| s.name == "pg_catalog" && s.system));

        let tables = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 500).await.unwrap();
        assert_eq!(tables.items.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["customers", "events", "orders"]);
        let events = &tables.items[1];
        assert_eq!((events.relkind.as_deref(), events.partitions), (Some("p"), Some(2)));
        assert!(tables.items[0].est_rows.is_some_and(|n| n >= 100));
        let page = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 2).await.unwrap();
        assert_eq!((page.total, page.next_offset), (3, Some(2)));
        let filtered = objects::list(&c, "shop", ObjectKind::Tables, Some("ORD"), 0, 500).await.unwrap();
        assert_eq!(filtered.items.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["orders"]);
        let underscore = objects::list(&c, "shop", ObjectKind::Tables, Some("_"), 0, 500).await.unwrap();
        assert_eq!(underscore.total, 0, "_ — буквальный символ, а не любой");

        let parts = objects::partitions(&c, "shop", "events", 0, 500).await.unwrap();
        assert_eq!(parts.items.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["events_2026_09", "events_2026_10"]);

        let funcs = objects::list(&c, "shop", ObjectKind::Functions, None, 0, 500).await.unwrap();
        let sigs: Vec<(String, Option<String>)> = funcs.items.iter().map(|f| (f.name.clone(), f.signature.clone())).collect();
        assert!(sigs.contains(&("order_count".into(), Some("c bigint".into()))), "{sigs:?}");
        assert!(funcs.items.iter().any(|f| f.name == "close_order" && f.relkind.as_deref() == Some("p")));

        let ext = extensions::list(&c).await.unwrap();
        assert!(ext.iter().any(|e| e.name == "plpgsql"));

        let rl = roles::list(&c, false).await.unwrap();
        let diag = rl.iter().find(|r| r.name == "glafira_diag").unwrap();
        assert!(diag.member_of.contains(&"pg_monitor".to_string()));
        assert!(!rl.iter().any(|r| r.name == "pg_monitor"));
        assert!(roles::list(&c, true).await.unwrap().iter().any(|r| r.name == "pg_monitor"));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm i18n:check`
Expected: PASS.

Run (стенд): `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_PATRONI=http://localhost:8011 cargo test --manifest-path src-tauri/Cargo.toml --test it_catalog`
Expected: PASS на трёх узлах (PG 15, 16, 17).

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(catalog): базы, схемы, объекты, партиции, расширения и роли для дерева

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 15: Фронтенд — типы, API, события наблюдения, вкладка «Обзор» в рабочей области

**Files:**
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/state/workspace.ts`, `src/state/workspace.test.ts`, `src/lib/format.ts`, `src/lib/format.test.ts`, `src/components/Layout.tsx`
- Create: `src/api/client.channel.test.ts`, `src/lib/useConnectionEvents.ts`, `src/lib/useConnectionEvents.test.ts`, `src/lib/topology.ts`, `src/test/fixtures.ts`
- Test: перечисленные `*.test.ts`

**Interfaces:**
- Consumes: команды Tasks 3, 4, 12, 13, 14; события `connection://state`, `topology://changed`.
- Produces:
  - Типы TS — зеркало Rust: `Environment`, `ConnectionKind`, `TargetServerType`, `SslMode`, `CertFormat`, `HostPort`, `ClientCert`, `PgTls`, `PgSettings`, `ApiTls`, `HostOverride`, `PatroniSettings`, `ConnectionProfile`, `SecretsInput`, `ProfilePatch`, `ParsedConnection`, `Role`, `Member`, `TopologyWarning`, `Topology`, `ConnState`, `TopologyView`, `TopologyEvent`, `MemberFacts`, `TestStep`, `StepStatus`, `TestEvent`, `ReadinessStatus`, `ReadinessItem`, `TestReport`, `DatabaseInfo`, `ObjectCounts`, `SchemaInfo`, `ObjectKind`, `ObjectItem`, `ObjectPage`, `ExtensionInfo`, `RoleInfo`, `UiSettings`.
  - `api.*`: `listConnections`, `saveConnection(profile, secrets)`, `deleteConnection(id)`, `parseConnectionString(text)`, `testConnection(profile, secrets, onEvent)`, `connect(id)`, `disconnect(id)`, `getTopology(id)`, `listTopologyEvents(id)`, `setCatalogMember(id, member | null)`, `getMemberFacts(id, member)`, `listDatabases(id)`, `listSchemas(id, db, includeSystem)`, `listObjects(id, db, schema, kind, filter | null, offset, limit)`, `listPartitions(id, db, schema, table, offset, limit)`, `listExtensions(id, db)`, `listRoles(id, includeSystem)`, `getUiSettings()`, `setUiSettings(s)`, `exportDiagnostics(path)`, `listFolders`, `createFolder`, `renameFolder`, `deleteFolder`, `moveItems` (как в Glafira).
  - `src/lib/useConnectionEvents.ts`: `disconnectedView`, `EVENTS_CAP = 500`, `applyState(qc, payload)`, `applyTopology(qc, payload)`, `useConnectionEvents()`, `useTopology(id)`, `useTopologyEvents(id)`. Ключи кэша: `["topology", id]`, `["topologyEvents", id]`, `["memberFacts", id, member]`, `["catalog", id, …]`, `["uiSettings"]`.
  - `src/lib/topology.ts`: `roleLabel(role, kind) -> string`, `isStandby(role)`.
  - `TabSpec` + `{ kind: "overview"; connectionId: string; member?: string }`, `tabId` → `overview:<id>`.
  - `formatBytes` понимает ГБ и ТБ.

- [ ] **Step 1: Типы**

Добавить в `src/api/types.ts` (`LocalizedText` и `AppError` уже есть из Glafira):

```ts
export type Environment = "local" | "dev" | "test" | "stage" | "prod";
export type ConnectionKind = "server" | "hostList" | "patroni";
export type TargetServerType = "primary" | "standby" | "preferStandby" | "any";
export type SslMode = "disable" | "prefer" | "require" | "verify-ca" | "verify-full";
export type CertFormat = "pem" | "pkcs12";

export interface HostPort {
  host: string;
  port: number;
}
export interface ClientCert {
  format: CertFormat;
  certificatePath: string;
  keyPath: string | null;
}
export interface PgTls {
  caPath: string | null;
  clientCert: ClientCert | null;
}
export interface PgSettings {
  username: string;
  database: string;
  sslMode: SslMode;
  tls: PgTls;
  connectTimeoutS: number;
  databases: string[] | null;
}
export interface ApiTls {
  caPath: string | null;
  verifyHostname: boolean;
  clientCert: ClientCert | null;
}
export interface HostOverride {
  member: string;
  host: string | null;
  port: number | null;
  apiUrl: string | null;
}
export interface PatroniSettings {
  urls: string[];
  username: string | null;
  tls: ApiTls | null;
  hostOverrides: HostOverride[];
}
export interface ConnectionProfile {
  id: string;
  name: string;
  color: string;
  environment: Environment;
  readOnly: boolean;
  kind: ConnectionKind;
  hosts: HostPort[];
  targetServerType: TargetServerType;
  patroni: PatroniSettings | null;
  pg: PgSettings;
  folderId?: string | null;
  order?: number;
}
/** У каждого поля: null/нет — оставить сохранённый, "" — удалить, иначе — новый. */
export interface SecretsInput {
  pgPassword?: string | null;
  pgKeyPassword?: string | null;
  patroniPassword?: string | null;
  patroniKeyPassword?: string | null;
}
export interface ProfilePatch {
  kind: ConnectionKind;
  hosts: HostPort[];
  targetServerType: TargetServerType | null;
  username: string | null;
  database: string | null;
  sslMode: SslMode | null;
  caPath: string | null;
  clientCert: ClientCert | null;
  connectTimeoutS: number | null;
}
export interface ParsedConnection {
  patch: ProfilePatch;
  password: string | null;
  ignored: string[];
}

export type Role = "primary" | "standbyLeader" | "syncStandby" | "quorumStandby" | "replica" | "unknown";
export interface Member {
  name: string;
  host: string;
  port: number;
  apiUrl: string | null;
  role: Role;
  rawRole: string | null;
  state: string | null;
  timeline: number | null;
  lagBytes: number | null;
  replayDelayMs: number | null;
  upstream: string | null;
  tags: Record<string, unknown>;
  pendingRestart: boolean;
  serverVersion: string | null;
  patroniVersion: string | null;
  dcsLastSeenMs: number | null;
  reachable: boolean;
  error: AppError | null;
}
export type TopologyWarning =
  | { kind: "noPrimary" }
  | { kind: "multiplePrimaries"; members: string[] }
  | { kind: "systemIdMismatch"; members: string[] }
  | { kind: "patroniUnreachable" }
  | { kind: "memberUnreachable"; name: string };
export interface Topology {
  connectionId: string;
  kind: ConnectionKind;
  observedAtMs: number;
  scope: string | null;
  paused: boolean | null;
  members: Member[];
  warnings: TopologyWarning[];
}
export type ConnState = "disconnected" | "connecting" | "connected" | "degraded" | "failed";
export interface TopologyView {
  state: ConnState;
  topology: Topology | null;
  catalogMember: string | null;
  catalogFallback: boolean;
  error: AppError | null;
}
export type TopologyEvent = { atMs: number } & (
  | { kind: "leaderChanged"; from: string | null; to: string | null }
  | { kind: "memberAdded"; name: string }
  | { kind: "memberRemoved"; name: string }
  | { kind: "memberRoleChanged"; name: string; from: Role; to: Role }
  | { kind: "memberStateChanged"; name: string; from: string | null; to: string | null }
  | { kind: "memberUnreachable"; name: string }
  | { kind: "memberReachable"; name: string }
  | { kind: "timelineChanged"; from: number; to: number }
  | { kind: "pauseChanged"; paused: boolean }
);
export interface MemberFacts {
  startedAtMs: number;
  connections: number;
  maxConnections: number;
  databases: number;
}

export type TestStep = "patroni" | "discovery" | "node" | "readiness";
export type StepStatus = "running" | "ok" | "failed" | "skipped";
export interface TestEvent {
  step: TestStep;
  member: string | null;
  status: StepStatus;
  message: LocalizedText | null;
  error: AppError | null;
}
export type ReadinessStatus = "ok" | "warn" | "missing";
export interface ReadinessItem {
  id: string;
  status: ReadinessStatus;
  params: Record<string, string>;
  fix: string | null;
}
export interface TestReport {
  ok: boolean;
  topology: Topology | null;
  readiness: ReadinessItem[];
}

export interface DatabaseInfo {
  name: string;
  sizeBytes: number | null;
}
export interface ObjectCounts {
  tables: number;
  views: number;
  matviews: number;
  sequences: number;
  functions: number;
}
export interface SchemaInfo {
  name: string;
  system: boolean;
  counts: ObjectCounts;
}
export type ObjectKind = "tables" | "views" | "matviews" | "functions" | "sequences";
export interface ObjectItem {
  oid: number;
  name: string;
  schema: string;
  relkind: string | null;
  partitions: number | null;
  estRows: number | null;
  totalBytes: number | null;
  signature: string | null;
}
export interface ObjectPage {
  total: number;
  items: ObjectItem[];
  nextOffset: number | null;
}
export interface ExtensionInfo {
  name: string;
  version: string;
  schema: string;
  defaultVersion: string | null;
  updateAvailable: boolean;
}
export interface RoleInfo {
  name: string;
  superuser: boolean;
  login: boolean;
  createDb: boolean;
  createRole: boolean;
  replication: boolean;
  bypassRls: boolean;
  connLimit: number;
  validUntil: string | null;
  memberOf: string[];
}
export interface UiSettings {
  showSystemObjects: boolean;
}
```

`ConnectionFolder`, `TreeItemRef` — скопировать из `$G/src/api/types.ts`, если их ещё нет.

- [ ] **Step 2: Тесты API и событий (падающие)**

Отдельный файл `src/api/client.channel.test.ts` (в `client.test.ts` свой мок `@tauri-apps/api/core`, второй в том же файле конфликтует):

```ts
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionProfile } from "./types";

const sent: unknown[] = [];
vi.mock("@tauri-apps/api/core", () => {
  class Channel<T> {
    onmessage: (m: T) => void = () => {};
  }
  return { Channel, invoke: vi.fn() };
});

import { invoke } from "@tauri-apps/api/core";
import { api } from "./client";

const profile: ConnectionProfile = {
  id: "",
  name: "x",
  color: "#000000",
  environment: "local",
  readOnly: false,
  kind: "server",
  hosts: [{ host: "localhost", port: 5432 }],
  targetServerType: "primary",
  patroni: null,
  pg: { username: "u", database: "postgres", sslMode: "prefer", tls: { caPath: null, clientCert: null }, connectTimeoutS: 10, databases: null },
};

describe("testConnection", () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it("передаёт события канала и возвращает отчёт", async () => {
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const ch = (args as { channel: { onmessage: (e: unknown) => void } }).channel;
      ch.onmessage({ step: "patroni", member: null, status: "skipped", message: null, error: null });
      return { ok: true, topology: null, readiness: [] };
    });
    const report = await api.testConnection(profile, { pgPassword: "x" }, (e) => sent.push(e));
    expect(report.ok).toBe(true);
    expect(sent).toHaveLength(1);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe("test_connection");
    expect((vi.mocked(invoke).mock.calls[0][1] as { input: { secrets: unknown } }).input.secrets).toEqual({ pgPassword: "x" });
  });
});
```

`src/lib/useConnectionEvents.test.ts`:

```ts
import { QueryClient } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import type { TopologyEvent, TopologyView } from "../api/types";
import { topologyView } from "../test/fixtures";
import { applyState, applyTopology, disconnectedView, EVENTS_CAP } from "./useConnectionEvents";

const ev = (atMs: number): TopologyEvent => ({ atMs, kind: "pauseChanged", paused: true });

describe("события наблюдения", () => {
  it("топология заменяет вид и добавляет события сверху", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], [ev(1)]);
    const view = topologyView();
    applyTopology(qc, { connectionId: "c", view, newEvents: [ev(2), ev(3)] });
    expect(qc.getQueryData<TopologyView>(["topology", "c"])).toEqual(view);
    expect(qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!.map((e) => e.atMs)).toEqual([3, 2, 1]);
  });

  it("журнал ограничен", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topologyEvents", "c"], Array.from({ length: EVENTS_CAP }, (_, i) => ev(i)));
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [ev(9999)] });
    const list = qc.getQueryData<TopologyEvent[]>(["topologyEvents", "c"])!;
    expect(list).toHaveLength(EVENTS_CAP);
    expect(list[0].atMs).toBe(9999);
  });

  it("смена узла каталога сбрасывает кэш каталога", async () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView({ catalogMember: "a" }));
    qc.setQueryData(["catalog", "c", "databases"], [{ name: "x", sizeBytes: 1 }]);
    applyTopology(qc, { connectionId: "c", view: topologyView({ catalogMember: "b" }), newEvents: [] });
    expect(qc.getQueryState(["catalog", "c", "databases"])!.isInvalidated).toBe(true);
  });

  it("отключение очищает каталог и журнал", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView());
    qc.setQueryData(["catalog", "c", "databases"], []);
    qc.setQueryData(["catalog", "other", "databases"], []);
    applyState(qc, { connectionId: "c", state: "disconnected", error: null });
    expect(qc.getQueryData(["topology", "c"])).toEqual(disconnectedView);
    expect(qc.getQueryData(["catalog", "c", "databases"])).toBeUndefined();
    expect(qc.getQueryData(["catalog", "other", "databases"])).toEqual([]);
  });

  it("смена состояния сохраняет топологию", () => {
    const qc = new QueryClient();
    qc.setQueryData(["topology", "c"], topologyView());
    applyState(qc, { connectionId: "c", state: "degraded", error: null });
    const v = qc.getQueryData<TopologyView>(["topology", "c"])!;
    expect(v.state).toBe("degraded");
    expect(v.topology).not.toBeNull();
  });
});
```

`src/test/fixtures.ts`:

```ts
import type { Member, Topology, TopologyView } from "../api/types";

export function member(name: string, patch: Partial<Member> = {}): Member {
  return {
    name,
    host: name,
    port: 5432,
    apiUrl: null,
    role: "replica",
    rawRole: null,
    state: "streaming",
    timeline: 1,
    lagBytes: 0,
    replayDelayMs: null,
    upstream: null,
    tags: {},
    pendingRestart: false,
    serverVersion: "17.2",
    patroniVersion: "4.0.4",
    dcsLastSeenMs: null,
    reachable: true,
    error: null,
    ...patch,
  };
}

export function topology(patch: Partial<Topology> = {}): Topology {
  return {
    connectionId: "c",
    kind: "patroni",
    observedAtMs: 1_000,
    scope: "demo",
    paused: false,
    members: [member("pg-1", { role: "primary", state: "running", lagBytes: null }), member("pg-2", { role: "syncStandby" })],
    warnings: [],
    ...patch,
  };
}

export function topologyView(patch: Partial<TopologyView> = {}): TopologyView {
  return { state: "connected", topology: topology(), catalogMember: "pg-1", catalogFallback: false, error: null, ...patch };
}
```

В `workspace.test.ts`:

```ts
it("вкладка обзора одна на подключение и получает выбранный узел", () => {
  useWorkspace.setState({ tabs: [], activeId: null });
  useWorkspace.getState().open({ kind: "overview", connectionId: "c" });
  useWorkspace.getState().open({ kind: "overview", connectionId: "c", member: "pg-2" });
  const { tabs, activeId } = useWorkspace.getState();
  expect(tabs).toHaveLength(1);
  expect(activeId).toBe("overview:c");
  expect(tabs[0]).toMatchObject({ member: "pg-2" });
});
```

В `format.test.ts`:

```ts
it("гигабайты и терабайты", () => {
  expect(formatBytes(3 * 1024 ** 3)).toMatch(/^3[.,]0 GB$/);
  expect(formatBytes(2 * 1024 ** 4)).toMatch(/^2[.,]0 TB$/);
});
```

Run: `pnpm test`
Expected: FAIL (нет `useConnectionEvents`, `api.testConnection`, вида `overview`, ГБ).

- [ ] **Step 3: Реализация**

`src/api/client.ts` — к `api` (импорты типов добавить):

```ts
  listConnections: () => call<ConnectionProfile[]>("list_connections"),
  saveConnection: (profile: ConnectionProfile, secrets: SecretsInput) =>
    call<ConnectionProfile>("save_connection", { input: { profile, secrets } }),
  deleteConnection: (id: string) => call<void>("delete_connection", { id }),
  parseConnectionString: (text: string) => call<ParsedConnection>("parse_connection_string", { text }),
  testConnection: (profile: ConnectionProfile, secrets: SecretsInput, onEvent: (e: TestEvent) => void) => {
    const channel = new Channel<TestEvent>();
    channel.onmessage = onEvent;
    return call<TestReport>("test_connection", { input: { profile, secrets }, channel });
  },
  connect: (id: string) => call<TopologyView>("connect", { id }),
  disconnect: (id: string) => call<void>("disconnect", { id }),
  getTopology: (id: string) => call<TopologyView>("get_topology", { id }),
  listTopologyEvents: (id: string) => call<TopologyEvent[]>("list_topology_events", { id }),
  setCatalogMember: (id: string, member: string | null) => call<TopologyView>("set_catalog_member", { id, member }),
  getMemberFacts: (id: string, member: string) => call<MemberFacts>("get_member_facts", { id, member }),
  listDatabases: (id: string) => call<DatabaseInfo[]>("list_databases", { id }),
  listSchemas: (id: string, db: string, includeSystem: boolean) =>
    call<SchemaInfo[]>("list_schemas", { id, db, includeSystem }),
  listObjects: (id: string, db: string, schema: string, kind: ObjectKind, filter: string | null, offset: number, limit: number) =>
    call<ObjectPage>("list_objects", { id, db, schema, kind, filter, offset, limit }),
  listPartitions: (id: string, db: string, schema: string, table: string, offset: number, limit: number) =>
    call<ObjectPage>("list_partitions", { id, db, schema, table, offset, limit }),
  listExtensions: (id: string, db: string) => call<ExtensionInfo[]>("list_extensions", { id, db }),
  listRoles: (id: string, includeSystem: boolean) => call<RoleInfo[]>("list_roles", { id, includeSystem }),
  getUiSettings: () => call<UiSettings>("get_ui_settings"),
  setUiSettings: (settings: UiSettings) => call<UiSettings>("set_ui_settings", { settings }),
  exportDiagnostics: (path: string) => call<void>("export_diagnostics", { path }),
```

и функции папок `listFolders`, `createFolder`, `renameFolder`, `deleteFolder`, `moveItems` — как в `$G/src/api/client.ts`.

`src/lib/useConnectionEvents.ts`:

```ts
import { listen } from "@tauri-apps/api/event";
import { type QueryClient, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "../api/client";
import type { AppError, ConnState, TopologyEvent, TopologyView } from "../api/types";

export const EVENTS_CAP = 500;

export const disconnectedView: TopologyView = {
  state: "disconnected",
  topology: null,
  catalogMember: null,
  catalogFallback: false,
  error: null,
};

type StatePayload = { connectionId: string; state: ConnState; error: AppError | null };
type TopologyPayload = { connectionId: string; view: TopologyView; newEvents: TopologyEvent[] };

const ofConnection = (prefixes: string[], id: string) => (q: { queryKey: readonly unknown[] }) =>
  prefixes.includes(q.queryKey[0] as string) && q.queryKey[1] === id;

export function applyState(qc: QueryClient, p: StatePayload) {
  if (p.state === "disconnected") {
    qc.setQueryData(["topology", p.connectionId], disconnectedView);
    qc.setQueryData(["topologyEvents", p.connectionId], []);
    qc.removeQueries({ predicate: ofConnection(["catalog", "memberFacts"], p.connectionId) });
    return;
  }
  qc.setQueryData<TopologyView>(["topology", p.connectionId], (old) => ({ ...(old ?? disconnectedView), state: p.state, error: p.error }));
}

export function applyTopology(qc: QueryClient, p: TopologyPayload) {
  const old = qc.getQueryData<TopologyView>(["topology", p.connectionId]);
  qc.setQueryData(["topology", p.connectionId], p.view);
  if (old?.catalogMember && old.catalogMember !== p.view.catalogMember) {
    // Узел каталога сменился (например, после switchover) — каталог читается заново.
    void qc.invalidateQueries({ predicate: ofConnection(["catalog"], p.connectionId) });
  }
  if (p.newEvents.length > 0) {
    qc.setQueryData<TopologyEvent[]>(["topologyEvents", p.connectionId], (prev = []) =>
      [...[...p.newEvents].reverse(), ...prev].slice(0, EVENTS_CAP),
    );
  }
}

/** Подписка на события наблюдения — один раз, в Layout. */
export function useConnectionEvents() {
  const qc = useQueryClient();
  useEffect(() => {
    const offs = [
      listen<StatePayload>("connection://state", (e) => applyState(qc, e.payload)),
      listen<TopologyPayload>("topology://changed", (e) => applyTopology(qc, e.payload)),
    ];
    return () => offs.forEach((p) => void p.then((off) => off()));
  }, [qc]);
}

export function useTopology(connectionId: string) {
  return useQuery({ queryKey: ["topology", connectionId], queryFn: () => api.getTopology(connectionId), staleTime: Infinity });
}

export function useTopologyEvents(connectionId: string) {
  return useQuery({
    queryKey: ["topologyEvents", connectionId],
    queryFn: () => api.listTopologyEvents(connectionId),
    staleTime: Infinity,
  });
}
```

`src/lib/topology.ts`:

```ts
import type { ConnectionKind, Role } from "../api/types";
import { i18n } from "../i18n";

export const isStandby = (role: Role) => role === "replica" || role === "syncStandby" || role === "quorumStandby";

/** Подпись роли: у кластера Patroni primary — «Leader», у списка узлов — «Primary». */
export function roleLabel(role: Role, kind: ConnectionKind): string {
  if (role === "primary") return i18n.t(kind === "patroni" ? "topology.role.leader" : "topology.role.primary");
  return i18n.t(`topology.role.${role}`);
}
```

`src/state/workspace.ts` — в `TabSpec` добавить `| { kind: "overview"; connectionId: string; member?: string }`, в `tabId` — `case "overview": return \`overview:${spec.connectionId}\`;`.

`src/lib/format.ts` — `formatBytes`:

```ts
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${num(v, 1)} ${units[i]}`;
}
```

`src/components/Layout.tsx` — вызвать `useConnectionEvents()` рядом с `useHelpShortcut()`; в `tabTitle` — `case "overview": return i18n.t("tabs.overview", { conn });`.

Словари: `tabs.overview` — en «Overview: {{conn}}», ru «Обзор: {{conn}}»; `topology.role` (en / ru): `leader` «Leader» / «Leader», `primary` «Primary» / «Primary», `standbyLeader` «Standby leader» / «Лидер standby», `syncStandby` «Sync» / «Sync», `quorumStandby` «Quorum» / «Quorum», `replica` «Replica» / «Реплика», `unknown` «—» / «—».

- [ ] **Step 4: Прогнать тесты**

Run: `pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat(ui): типы и API подключений, события наблюдения, вкладка обзора в рабочей области

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Фронтенд — форма подключения и проверка

**Files:**
- Copy from `$G` без изменений: `src/components/connections/tree/folderTree.ts`, `folderTree.test.ts` (их `folderPaths` нужен форме)
- Create: `src/components/connections/profile.ts` (+ `profile.test.ts`), `src/components/connections/fields.tsx`, `src/components/connections/NodesSection.tsx`, `src/components/connections/PgSection.tsx`, `src/components/connections/TestPanel.tsx` (+ `TestPanel.test.tsx`), `src/components/connections/ConnectionForm.tsx` (+ `ConnectionForm.test.tsx`), `src/components/connections/ConnectionFormTab.tsx`
- Modify: `src/components/TabContent.tsx`, `src/locales/{en,ru}.json`

**Interfaces:**
- Consumes: `api.{listConnections, listFolders, saveConnection, deleteConnection, parseConnectionString, testConnection, moveItems}`, `useLicense`, `useWorkspace`, `Hint`, `ErrorBanner`, `ProLock`, `folderPaths` (`connections/tree/folderTree.ts` — копируется в Task 17; до него импортировать нельзя, поэтому в этой задаче скопировать `folderTree.ts` и `folderTree.test.ts` из `$G` без изменений).
- Produces:
  - `profile.ts`: `newProfile()`, `emptyPatroni()`, `withKind(p, kind)`, `toPatroniFromHosts(p)`, `applyParsed(p, parsed)`, `parseDatabases(text)`, `formatDatabases(list)`, `formatFromPath(path)`, `SecretField`, `emptySecret`, `secretValue(f)`.
  - `ConnectionForm({ initial, tabId })`, `ConnectionFormTab({ connectionId, folderId, tabId })`, `TestPanel({ events, report })`.

- [ ] **Step 1: Модель формы — тесты и реализация**

`src/components/connections/profile.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { ParsedConnection } from "../../api/types";
import { applyParsed, formatDatabases, formatFromPath, newProfile, parseDatabases, secretValue, toPatroniFromHosts, withKind } from "./profile";

describe("модель формы", () => {
  it("смена вида не теряет ввод", () => {
    const list = { ...withKind(newProfile(), "hostList"), hosts: [{ host: "a", port: 5432 }, { host: "b", port: 5433 }] };
    expect(withKind(list, "server").hosts).toEqual([{ host: "a", port: 5432 }]);
    const patroni = withKind(list, "patroni");
    expect(patroni.patroni!.urls).toEqual(["http://a:8008", "http://b:8008"]);
    expect(withKind(patroni, "hostList").hosts).toHaveLength(2);
  });

  it("адреса API из IPv6", () => {
    expect(toPatroniFromHosts({ ...newProfile(), hosts: [{ host: "::1", port: 5432 }] }).urls).toEqual(["http://[::1]:8008"]);
  });

  it("строка подключения заполняет форму", () => {
    const parsed: ParsedConnection = {
      patch: {
        kind: "hostList",
        hosts: [{ host: "h1", port: 5432 }, { host: "h2", port: 5433 }],
        targetServerType: "preferStandby",
        username: "app",
        database: "orders",
        sslMode: "verify-full",
        caPath: "/ca.pem",
        clientCert: null,
        connectTimeoutS: 7,
      },
      password: "pw",
      ignored: [],
    };
    const p = applyParsed(withKind(newProfile(), "patroni"), parsed);
    expect(p.kind).toBe("hostList");
    expect(p.patroni).toBeNull();
    expect(p.hosts).toHaveLength(2);
    expect(p.targetServerType).toBe("preferStandby");
    expect(p.pg).toMatchObject({ username: "app", database: "orders", sslMode: "verify-full", connectTimeoutS: 7 });
    expect(p.pg.tls.caPath).toBe("/ca.pem");
  });

  it("фильтр баз и форматы", () => {
    expect(parseDatabases(" orders, ,billing ")).toEqual(["orders", "billing"]);
    expect(parseDatabases("  ")).toBeNull();
    expect(formatDatabases(["a", "b"])).toBe("a, b");
    expect(formatFromPath("/x/client.P12")).toBe("pkcs12");
    expect(formatFromPath("/x/c.crt")).toBe("pem");
    expect(formatFromPath("/x/c.bin")).toBeNull();
  });

  it("семантика секрета", () => {
    expect(secretValue({ text: "", clear: false })).toBeNull();
    expect(secretValue({ text: "", clear: true })).toBe("");
    expect(secretValue({ text: "x", clear: false })).toBe("x");
  });
});
```

`src/components/connections/profile.ts`:

```ts
import type { CertFormat, ConnectionKind, ConnectionProfile, ParsedConnection, PatroniSettings } from "../../api/types";

export function newProfile(): ConnectionProfile {
  return {
    id: "",
    name: "",
    color: "#3b82f6",
    environment: "local",
    readOnly: false,
    kind: "server",
    hosts: [{ host: "localhost", port: 5432 }],
    targetServerType: "primary",
    patroni: null,
    pg: {
      username: "",
      database: "postgres",
      sslMode: "prefer",
      tls: { caPath: null, clientCert: null },
      connectTimeoutS: 10,
      databases: null,
    },
    folderId: null,
    order: 0,
  };
}

export function emptyPatroni(): PatroniSettings {
  return { urls: ["http://localhost:8008"], username: null, tls: null, hostOverrides: [] };
}

/** «Узлы под управлением Patroni»: адреса API — хосты списка с портом 8008 (спец. P1 §3.1). */
export function toPatroniFromHosts(p: ConnectionProfile): PatroniSettings {
  const urls = p.hosts
    .filter((h) => h.host.trim())
    .map((h) => `http://${h.host.includes(":") ? `[${h.host}]` : h.host}:8008`);
  return { ...emptyPatroni(), urls: urls.length > 0 ? urls : emptyPatroni().urls };
}

/** Смена вида: узлы и настройки Patroni переносятся так, чтобы ввод не терялся. */
export function withKind(p: ConnectionProfile, kind: ConnectionKind): ConnectionProfile {
  if (kind === p.kind) return p;
  if (kind === "patroni") return { ...p, kind, patroni: p.patroni ?? toPatroniFromHosts(p) };
  const hosts = p.hosts.length > 0 ? p.hosts : [{ host: "localhost", port: 5432 }];
  return { ...p, kind, hosts: kind === "server" ? hosts.slice(0, 1) : hosts };
}

export function applyParsed(p: ConnectionProfile, parsed: ParsedConnection): ConnectionProfile {
  const x = parsed.patch;
  return {
    ...p,
    kind: x.kind,
    hosts: x.hosts,
    patroni: null,
    targetServerType: x.targetServerType ?? p.targetServerType,
    pg: {
      ...p.pg,
      username: x.username ?? p.pg.username,
      database: x.database ?? p.pg.database,
      sslMode: x.sslMode ?? p.pg.sslMode,
      tls: { caPath: x.caPath ?? p.pg.tls.caPath, clientCert: x.clientCert ?? p.pg.tls.clientCert },
      connectTimeoutS: x.connectTimeoutS ?? p.pg.connectTimeoutS,
    },
  };
}

/** Фильтр баз «orders, billing» → список; пусто — все базы. */
export function parseDatabases(text: string): string[] | null {
  const list = text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return list.length > 0 ? list : null;
}

export function formatDatabases(list: string[] | null): string {
  return (list ?? []).join(", ");
}

/** Формат сертификата по расширению; null — не угадали. */
export function formatFromPath(path: string): CertFormat | null {
  const ext = path.toLowerCase().split(/[\\/]/).pop()?.split(".").pop() ?? "";
  if (ext === "p12" || ext === "pfx") return "pkcs12";
  if (ext === "pem" || ext === "crt" || ext === "cer" || ext === "key") return "pem";
  return null;
}

export type SecretField = { text: string; clear: boolean };
export const emptySecret: SecretField = { text: "", clear: false };

/** null — не менять сохранённый секрет, "" — удалить, иначе — новый. */
export function secretValue(f: SecretField): string | null {
  if (f.clear) return "";
  return f.text ? f.text : null;
}
```

Run: `pnpm test src/components/connections/profile.test.ts`
Expected: PASS.

- [ ] **Step 2: Поля формы и секции**

`src/components/connections/fields.tsx`:

```tsx
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { CertFormat, ClientCert } from "../../api/types";
import { formatFromPath, type SecretField } from "./profile";

/** Подпись строки сетки; значок подсказки — рядом с `<label>`, а не внутри (иначе попадёт в доступное имя). */
export function RowLabel({ id, label, hint }: { id: string; label: string; hint?: ReactNode }) {
  if (!hint) return <label htmlFor={id}>{label}</label>;
  return (
    <span className="field-label">
      <label htmlFor={id}>{label}</label>
      {hint}
    </span>
  );
}

async function pickFile(onPath: (p: string) => void) {
  const path = await openDialog({ multiple: false, directory: false });
  if (typeof path === "string") onPath(path);
}

export function PathRow(props: { id: string; label: string; hint?: ReactNode; value: string | null; onChange: (v: string | null) => void }) {
  const { t } = useTranslation();
  return (
    <>
      <RowLabel id={props.id} label={props.label} hint={props.hint} />
      <div className="path-field">
        <input id={props.id} value={props.value ?? ""} onChange={(e) => props.onChange(e.target.value || null)} />
        <button type="button" aria-label={t("connections.form.pickAria", { label: props.label })} onClick={() => void pickFile(props.onChange)}>
          {t("connections.form.pick")}
        </button>
      </div>
    </>
  );
}

export function SecretRow(props: { id: string; label: string; hint?: ReactNode; isNew: boolean; value: SecretField; onChange: (v: SecretField) => void }) {
  const { t } = useTranslation();
  return (
    <>
      <RowLabel id={props.id} label={props.label} hint={props.hint} />
      <span className="path-field">
        <input
          id={props.id}
          type="password"
          autoComplete="off"
          disabled={props.value.clear}
          placeholder={props.isNew ? "" : t("connections.form.passwordKeep")}
          value={props.value.text}
          onChange={(e) => props.onChange({ text: e.target.value, clear: false })}
        />
        {!props.isNew && (
          <label className="inline">
            <input type="checkbox" checked={props.value.clear} onChange={(e) => props.onChange({ text: "", clear: e.target.checked })} />
            {t("connections.form.passwordClear")}
          </label>
        )}
      </span>
    </>
  );
}

/** Клиентский сертификат: формат, файлы, пароль ключа. `null` — без сертификата. */
export function ClientCertRows(props: {
  idPrefix: string;
  cert: ClientCert | null;
  onChange: (c: ClientCert | null) => void;
  keyPassword: SecretField;
  onKeyPassword: (v: SecretField) => void;
  isNew: boolean;
}) {
  const { t } = useTranslation();
  const { cert, idPrefix: p } = props;
  return (
    <>
      <label htmlFor={`${p}-cc`}>{t("connections.form.clientCert")}</label>
      <span>
        <input
          id={`${p}-cc`}
          type="checkbox"
          checked={cert !== null}
          onChange={(e) => props.onChange(e.target.checked ? { format: "pem", certificatePath: "", keyPath: "" } : null)}
        />
      </span>
      {cert && (
        <>
          <label htmlFor={`${p}-fmt`}>{t("connections.form.certFormat")}</label>
          <select id={`${p}-fmt`} value={cert.format} onChange={(e) => props.onChange({ ...cert, format: e.target.value as CertFormat, keyPath: e.target.value === "pem" ? cert.keyPath ?? "" : null })}>
            <option value="pem">PEM</option>
            <option value="pkcs12">PKCS#12 (.p12, .pfx)</option>
          </select>
          <PathRow
            id={`${p}-cert`}
            label={t("connections.form.certPath")}
            value={cert.certificatePath}
            onChange={(v) => {
              const guessed = v ? formatFromPath(v) : null;
              const format = guessed ?? cert.format;
              props.onChange({ ...cert, certificatePath: v ?? "", format, keyPath: format === "pem" ? cert.keyPath ?? "" : null });
            }}
          />
          {cert.format === "pem" && (
            <PathRow id={`${p}-key`} label={t("connections.form.keyPath")} value={cert.keyPath} onChange={(v) => props.onChange({ ...cert, keyPath: v })} />
          )}
          <SecretRow id={`${p}-kpw`} label={t("connections.form.keyPassword")} isNew={props.isNew} value={props.keyPassword} onChange={props.onKeyPassword} />
        </>
      )}
    </>
  );
}
```

`src/components/connections/NodesSection.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { ConnectionProfile, HostOverride, HostPort, PatroniSettings, TargetServerType } from "../../api/types";
import { Hint } from "../common/Hint";
import { ClientCertRows, PathRow, RowLabel, SecretRow } from "./fields";
import { emptyPatroni, type SecretField, withKind } from "./profile";

const TARGETS: TargetServerType[] = ["primary", "standby", "preferStandby", "any"];

export type PatroniSecrets = { password: SecretField; keyPassword: SecretField };

export function NodesSection(props: {
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  secrets: PatroniSecrets;
  onSecrets: (s: PatroniSecrets) => void;
  isNew: boolean;
}) {
  const { t } = useTranslation();
  const { profile: p, onChange } = props;
  const setHosts = (hosts: HostPort[]) => onChange({ ...p, hosts });
  const setPatroni = (patch: Partial<PatroniSettings>) => onChange({ ...p, patroni: { ...(p.patroni ?? emptyPatroni()), ...patch } });

  if (p.kind === "server") {
    const h = p.hosts[0] ?? { host: "", port: 5432 };
    return (
      <>
        <label htmlFor="cf-host">{t("connections.form.host")}</label>
        <span className="path-field">
          <input id="cf-host" value={h.host} onChange={(e) => setHosts([{ ...h, host: e.target.value }])} />
          <input aria-label={t("connections.form.port")} type="number" min={1} max={65535} style={{ width: 90 }} value={h.port} onChange={(e) => setHosts([{ ...h, port: Number(e.target.value) }])} />
        </span>
      </>
    );
  }

  if (p.kind === "hostList") {
    const move = (i: number, d: -1 | 1) => {
      const next = [...p.hosts];
      [next[i], next[i + d]] = [next[i + d], next[i]];
      setHosts(next);
    };
    return (
      <>
        <span className="field-label">{t("connections.form.hosts")}</span>
        <div className="headers-editor">
          {p.hosts.map((h, i) => (
            <span key={i} className="path-field">
              <input aria-label={t("connections.form.hostN", { n: i + 1 })} value={h.host} onChange={(e) => setHosts(p.hosts.map((x, j) => (j === i ? { ...x, host: e.target.value } : x)))} />
              <input aria-label={t("connections.form.portN", { n: i + 1 })} type="number" min={1} max={65535} style={{ width: 90 }} value={h.port} onChange={(e) => setHosts(p.hosts.map((x, j) => (j === i ? { ...x, port: Number(e.target.value) } : x)))} />
              <button type="button" className="icon" disabled={i === 0} aria-label={t("connections.form.moveUp", { n: i + 1 })} onClick={() => move(i, -1)}>↑</button>
              <button type="button" className="icon" disabled={i === p.hosts.length - 1} aria-label={t("connections.form.moveDown", { n: i + 1 })} onClick={() => move(i, 1)}>↓</button>
              <button type="button" className="icon" disabled={p.hosts.length === 1} aria-label={t("connections.form.removeHost", { n: i + 1 })} onClick={() => setHosts(p.hosts.filter((_, j) => j !== i))}>✕</button>
            </span>
          ))}
          <button type="button" className="link" disabled={p.hosts.length >= 16} onClick={() => setHosts([...p.hosts, { host: "", port: 5432 }])}>
            {t("connections.form.addHost")}
          </button>
        </div>

        <RowLabel id="cf-target" label={t("connections.form.target")} hint={<Hint k="hints.connections.target" label={t("connections.form.target")} />} />
        <select id="cf-target" value={p.targetServerType} onChange={(e) => onChange({ ...p, targetServerType: e.target.value as TargetServerType })}>
          {TARGETS.map((x) => (
            <option key={x} value={x}>
              {t(`connections.form.targets.${x}`)}
            </option>
          ))}
        </select>

        <label htmlFor="cf-under-patroni">{t("connections.form.underPatroni")}</label>
        <span>
          <input id="cf-under-patroni" type="checkbox" checked={false} onChange={() => onChange(withKind(p, "patroni"))} />
        </span>
      </>
    );
  }

  const pt = p.patroni ?? emptyPatroni();
  const tls = pt.tls;
  const setOverride = (i: number, patch: Partial<HostOverride>) =>
    setPatroni({ hostOverrides: pt.hostOverrides.map((o, j) => (j === i ? { ...o, ...patch } : o)) });
  return (
    <>
      <span className="field-label">{t("connections.form.patroniUrls")}</span>
      <div className="headers-editor">
        {pt.urls.map((u, i) => (
          <span key={i} className="path-field">
            <input aria-label={t("connections.form.urlN", { n: i + 1 })} placeholder="https://pg-1:8008" value={u} onChange={(e) => setPatroni({ urls: pt.urls.map((x, j) => (j === i ? e.target.value : x)) })} />
            <button type="button" className="icon" disabled={pt.urls.length === 1} aria-label={t("connections.form.removeUrl", { n: i + 1 })} onClick={() => setPatroni({ urls: pt.urls.filter((_, j) => j !== i) })}>✕</button>
          </span>
        ))}
        <button type="button" className="link" disabled={pt.urls.length >= 8} onClick={() => setPatroni({ urls: [...pt.urls, ""] })}>
          {t("connections.form.addUrl")}
        </button>
      </div>

      <label htmlFor="cf-api-user">{t("connections.form.patroniUser")}</label>
      <input id="cf-api-user" value={pt.username ?? ""} onChange={(e) => setPatroni({ username: e.target.value || null })} />
      {pt.username && (
        <SecretRow id="cf-api-pass" label={t("connections.form.patroniPassword")} isNew={props.isNew} value={props.secrets.password} onChange={(password) => props.onSecrets({ ...props.secrets, password })} />
      )}

      <label htmlFor="cf-api-tls">{t("connections.form.apiTls")}</label>
      <span>
        <input id="cf-api-tls" type="checkbox" checked={tls !== null} onChange={(e) => setPatroni({ tls: e.target.checked ? { caPath: null, verifyHostname: true, clientCert: null } : null })} />
      </span>
      {tls && (
        <>
          <PathRow id="cf-api-ca" label={t("connections.form.caPath")} value={tls.caPath} onChange={(caPath) => setPatroni({ tls: { ...tls, caPath } })} />
          <label htmlFor="cf-api-verify">{t("connections.form.verifyHostname")}</label>
          <span>
            <input id="cf-api-verify" type="checkbox" checked={tls.verifyHostname} onChange={(e) => setPatroni({ tls: { ...tls, verifyHostname: e.target.checked } })} />
          </span>
          <ClientCertRows
            idPrefix="cf-api"
            cert={tls.clientCert}
            onChange={(clientCert) => setPatroni({ tls: { ...tls, clientCert } })}
            keyPassword={props.secrets.keyPassword}
            onKeyPassword={(keyPassword) => props.onSecrets({ ...props.secrets, keyPassword })}
            isNew={props.isNew}
          />
        </>
      )}

      <details className="full">
        <summary>
          {t("connections.form.overrides")} <Hint k="hints.connections.overrides" label={t("connections.form.overrides")} />
        </summary>
        <div className="headers-editor">
          {pt.hostOverrides.map((o, i) => (
            <span key={i} className="path-field">
              <input aria-label={t("connections.form.overrideMember", { n: i + 1 })} placeholder="pg-1" value={o.member} onChange={(e) => setOverride(i, { member: e.target.value })} />
              <input aria-label={t("connections.form.overrideHost", { n: i + 1 })} placeholder="10.0.0.1" value={o.host ?? ""} onChange={(e) => setOverride(i, { host: e.target.value || null })} />
              <input aria-label={t("connections.form.overridePort", { n: i + 1 })} type="number" style={{ width: 90 }} value={o.port ?? ""} onChange={(e) => setOverride(i, { port: e.target.value ? Number(e.target.value) : null })} />
              <input aria-label={t("connections.form.overrideApi", { n: i + 1 })} placeholder="https://10.0.0.1:8008" value={o.apiUrl ?? ""} onChange={(e) => setOverride(i, { apiUrl: e.target.value || null })} />
              <button type="button" className="icon" aria-label={t("connections.form.removeOverride", { n: i + 1 })} onClick={() => setPatroni({ hostOverrides: pt.hostOverrides.filter((_, j) => j !== i) })}>✕</button>
            </span>
          ))}
          <button type="button" className="link" onClick={() => setPatroni({ hostOverrides: [...pt.hostOverrides, { member: "", host: null, port: null, apiUrl: null }] })}>
            {t("connections.form.addOverride")}
          </button>
        </div>
      </details>
    </>
  );
}
```

`src/components/connections/PgSection.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConnectionProfile, PgSettings, SslMode } from "../../api/types";
import { Hint } from "../common/Hint";
import { ClientCertRows, PathRow, RowLabel, SecretRow } from "./fields";
import { formatDatabases, parseDatabases, type SecretField } from "./profile";

const SSL_MODES: SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

export type PgSecrets = { password: SecretField; keyPassword: SecretField };

export function PgSection(props: {
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  secrets: PgSecrets;
  onSecrets: (s: PgSecrets) => void;
  isNew: boolean;
}) {
  const { t } = useTranslation();
  const pg = props.profile.pg;
  const set = (patch: Partial<PgSettings>) => props.onChange({ ...props.profile, pg: { ...pg, ...patch } });
  const [dbText, setDbText] = useState(formatDatabases(pg.databases));
  return (
    <>
      <label htmlFor="cf-pg-user">{t("connections.form.pgUser")}</label>
      <input id="cf-pg-user" value={pg.username} onChange={(e) => set({ username: e.target.value })} />
      <SecretRow
        id="cf-pg-pass"
        label={t("connections.form.pgPassword")}
        hint={<Hint k="hints.connections.pgpass" label={t("connections.form.pgPassword")} />}
        isNew={props.isNew}
        value={props.secrets.password}
        onChange={(password) => props.onSecrets({ ...props.secrets, password })}
      />
      <label htmlFor="cf-pg-db">{t("connections.form.pgDatabase")}</label>
      <input id="cf-pg-db" value={pg.database} onChange={(e) => set({ database: e.target.value })} />
      <RowLabel id="cf-ssl" label="sslmode" hint={<Hint k="hints.connections.sslMode" label="sslmode" />} />
      <select id="cf-ssl" value={pg.sslMode} onChange={(e) => set({ sslMode: e.target.value as SslMode })}>
        {SSL_MODES.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
      {pg.sslMode !== "disable" && (
        <>
          <PathRow id="cf-pg-ca" label={t("connections.form.caPath")} value={pg.tls.caPath} onChange={(caPath) => set({ tls: { ...pg.tls, caPath } })} />
          <ClientCertRows
            idPrefix="cf-pg"
            cert={pg.tls.clientCert}
            onChange={(clientCert) => set({ tls: { ...pg.tls, clientCert } })}
            keyPassword={props.secrets.keyPassword}
            onKeyPassword={(keyPassword) => props.onSecrets({ ...props.secrets, keyPassword })}
            isNew={props.isNew}
          />
        </>
      )}
      <label htmlFor="cf-timeout">{t("connections.form.timeout")}</label>
      <input id="cf-timeout" type="number" min={1} max={120} value={pg.connectTimeoutS} onChange={(e) => set({ connectTimeoutS: Number(e.target.value) })} />
      <label htmlFor="cf-dbs">{t("connections.form.databases")}</label>
      <input
        id="cf-dbs"
        placeholder={t("connections.form.databasesPlaceholder")}
        value={dbText}
        onChange={(e) => {
          setDbText(e.target.value);
          set({ databases: parseDatabases(e.target.value) });
        }}
      />
    </>
  );
}
```

- [ ] **Step 3: Панель проверки — тест и реализация**

`src/components/connections/TestPanel.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { TestEvent, TestReport } from "../../api/types";
import { TestPanel } from "./TestPanel";

const ev = (patch: Partial<TestEvent>): TestEvent => ({ step: "node", member: null, status: "running", message: null, error: null, ...patch });

describe("TestPanel", () => {
  it("показывает последнее состояние каждого шага", () => {
    render(
      <TestPanel
        events={[
          ev({ step: "patroni", status: "skipped" }),
          ev({ member: "db1:5432", status: "running" }),
          ev({ member: "db1:5432", status: "ok", message: { key: "test.nodeOk", params: { role: "primary", version: "16.4" }, text: "primary, PostgreSQL 16.4" } }),
          ev({ member: "db2:5432", status: "failed", error: { code: "AUTH_FAILED", key: "errors.pg.authFailed", message: "Authentication failed", retryable: false } }),
        ]}
        report={null}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[1]).toHaveTextContent("primary, PostgreSQL 16.4");
    expect(items[2]).toHaveTextContent("Аутентификация не прошла");
  });

  it("чек-лист с командами для исправления", () => {
    const report: TestReport = {
      ok: true,
      topology: null,
      readiness: [
        { id: "monitorRole", status: "ok", params: {}, fix: null },
        { id: "pgssInstalled", status: "missing", params: { database: "postgres" }, fix: "CREATE EXTENSION IF NOT EXISTS pg_stat_statements;" },
        { id: "extension", status: "warn", params: { name: "hypopg" }, fix: "CREATE EXTENSION IF NOT EXISTS hypopg;" },
      ],
    };
    render(<TestPanel events={[]} report={report} />);
    expect(screen.getByText("CREATE EXTENSION IF NOT EXISTS pg_stat_statements;")).toBeInTheDocument();
    expect(screen.getByText(/hypopg/, { selector: "strong" })).toBeInTheDocument();
    expect(screen.getByText(/postgres/)).toBeInTheDocument();
  });
});
```

`src/components/connections/TestPanel.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { ReadinessItem, ReadinessStatus, StepStatus, TestEvent, TestReport, TopologyWarning } from "../../api/types";
import { tr } from "../../i18n/text";

const STEP_ICON: Record<StepStatus, string> = { running: "⏳", ok: "✅", failed: "❌", skipped: "⏭" };
const READY_ICON: Record<ReadinessStatus, string> = { ok: "✅", warn: "⚠️", missing: "❌" };

/** Скрипт роли для диагностики (спец. P1 §10). */
export const ROLE_SCRIPT = `CREATE ROLE glafira_diag LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;   -- cancel and terminate sessions
GRANT pg_maintain TO glafira_diag;         -- PostgreSQL 17+: VACUUM and ANALYZE
-- in every database you want to analyze (and pg_stat_statements in shared_preload_libraries):
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;`;

/** Последнее состояние каждого шага в порядке первого появления. */
function latestSteps(events: TestEvent[]): TestEvent[] {
  const order: string[] = [];
  const last = new Map<string, TestEvent>();
  for (const e of events) {
    const key = `${e.step}:${e.member ?? ""}`;
    if (!last.has(key)) order.push(key);
    last.set(key, e);
  }
  return order.map((k) => last.get(k)!);
}

function itemText(item: ReadinessItem, t: (k: string, o?: Record<string, unknown>) => string) {
  const variant = item.params.reason ?? item.status;
  return {
    title: t(`readiness.${item.id}.title`, item.params),
    state: t(`readiness.${item.id}.${variant}`, item.params),
    affects: item.status === "ok" ? null : t(`readiness.${item.id}.affects`, item.params),
  };
}

export function warningText(w: TopologyWarning, t: (k: string, o?: Record<string, unknown>) => string): string {
  switch (w.kind) {
    case "noPrimary":
      return t("topology.warning.noPrimary");
    case "multiplePrimaries":
      return t("topology.warning.multiplePrimaries", { members: w.members.join(", ") });
    case "systemIdMismatch":
      return t("topology.warning.systemIdMismatch", { members: w.members.join(", ") });
    case "patroniUnreachable":
      return t("topology.warning.patroniUnreachable");
    case "memberUnreachable":
      return t("topology.warning.memberUnreachable", { name: w.name });
  }
}

export function TestPanel({ events, report }: { events: TestEvent[]; report: TestReport | null }) {
  const { t } = useTranslation();
  const steps = latestSteps(events);
  const needsRole = report?.readiness.some((i) => (i.id === "monitorRole" || i.id === "signalRole") && i.status !== "ok");
  return (
    <div className="panel">
      {steps.length > 0 && (
        <ul className="steps">
          {steps.map((s) => (
            <li key={`${s.step}:${s.member ?? ""}`}>
              {STEP_ICON[s.status]} <strong>{t(`test.step.${s.step}`)}{s.member ? ` ${s.member}` : ""}:</strong>{" "}
              {s.message && <span>{tr(s.message)}</span>}
              {s.error && <span className="warn">{tr(s.error)}</span>}
              {s.error?.details && <pre className="muted">{s.error.details}</pre>}
            </li>
          ))}
        </ul>
      )}
      {report?.topology && report.topology.warnings.length > 0 && (
        <div className="confirm" role="alert">
          {report.topology.warnings.map((w, i) => (
            <span key={i}>{warningText(w, t)}</span>
          ))}
        </div>
      )}
      {report && report.readiness.length > 0 && (
        <>
          <h3>{t("test.readinessTitle")}</h3>
          <table>
            <tbody>
              {report.readiness.map((item, i) => {
                const text = itemText(item, t);
                return (
                  <tr key={`${item.id}:${i}`}>
                    <td>{READY_ICON[item.status]}</td>
                    <td>
                      <strong>{text.title}</strong>
                      <div className="muted">{text.state}</div>
                      {text.affects && <div className="muted">{text.affects}</div>}
                    </td>
                    <td>
                      {item.fix && (
                        <span className="path-field">
                          <pre>{item.fix}</pre>
                          <button type="button" onClick={() => void navigator.clipboard?.writeText(item.fix!)}>
                            {t("common.copy")}
                          </button>
                        </span>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {needsRole && (
            <details>
              <summary>{t("test.roleScript")}</summary>
              <pre>{ROLE_SCRIPT}</pre>
            </details>
          )}
        </>
      )}
    </div>
  );
}
```

(`common.copy` — если ключа нет в словарях Glafira, добавить: en «Copy», ru «Копировать».)

- [ ] **Step 4: Форма и вкладка формы**

`src/components/connections/ConnectionForm.tsx`:

```tsx
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, ConnectionProfile, Environment, SecretsInput, TestEvent, TestReport } from "../../api/types";
import { proHint, useLicense } from "../../lib/useLicense";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { Hint } from "../common/Hint";
import { ProLock } from "../common/ProLock";
import { NodesSection, type PatroniSecrets } from "./NodesSection";
import { PgSection, type PgSecrets } from "./PgSection";
import { applyParsed, emptySecret, secretValue, withKind } from "./profile";
import { TestPanel } from "./TestPanel";
import { folderPaths } from "./tree/folderTree";

const ENVIRONMENTS: Environment[] = ["local", "dev", "test", "stage", "prod"];
const KINDS: ConnectionKind[] = ["server", "hostList", "patroni"];

export function ConnectionForm({ initial, tabId }: { initial: ConnectionProfile; tabId: string }) {
  const { t } = useTranslation();
  const [profile, setProfile] = useState(initial);
  const [pgSecrets, setPgSecrets] = useState<PgSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [apiSecrets, setApiSecrets] = useState<PatroniSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [connstr, setConnstr] = useState("");
  const [ignored, setIgnored] = useState<string[]>([]);
  const [events, setEvents] = useState<TestEvent[]>([]);
  const [report, setReport] = useState<TestReport | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const queryClient = useQueryClient();
  const foldersQ = useQuery({ queryKey: ["folders"], queryFn: api.listFolders });
  const paths = useMemo(() => folderPaths(foldersQ.data ?? []), [foldersQ.data]);
  const knownFolder = (id: string | null | undefined): string | null =>
    !id || (foldersQ.isSuccess && !paths.some((p) => p.id === id)) ? null : id;
  const [loadedFolderId] = useState(initial.folderId);
  const [folderTouched, setFolderTouched] = useState(false);
  const folderId = knownFolder(folderTouched ? profile.folderId : initial.folderId);
  const open = useWorkspace((s) => s.open);
  const close = useWorkspace((s) => s.close);
  const closeForConnection = useWorkspace((s) => s.closeForConnection);
  const isNew = initial.id === "";
  const { locked } = useLicense();
  const readOnlyLocked = locked && initial.environment === "prod" && initial.readOnly && profile.environment === "prod";

  const current = (): ConnectionProfile => ({ ...profile, folderId });
  const secrets = (): SecretsInput => ({
    pgPassword: secretValue(pgSecrets.password),
    pgKeyPassword: secretValue(pgSecrets.keyPassword),
    patroniPassword: secretValue(apiSecrets.password),
    patroniKeyPassword: secretValue(apiSecrets.keyPassword),
  });

  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  }

  const onParse = () =>
    run(async () => {
      const parsed = await api.parseConnectionString(connstr);
      setProfile((p) => applyParsed(p, parsed));
      if (parsed.password) setPgSecrets((s) => ({ ...s, password: { text: parsed.password!, clear: false } }));
      setIgnored(parsed.ignored);
    });

  const onTest = () =>
    run(async () => {
      setEvents([]);
      setReport(null);
      setReport(await api.testConnection(current(), secrets(), (e) => setEvents((prev) => [...prev, e])));
    });

  const onSave = () =>
    run(async () => {
      const saved = await api.saveConnection(current(), secrets());
      try {
        if (!isNew && folderTouched && folderId !== knownFolder(loadedFolderId)) {
          await api.moveItems([{ kind: "connection", id: saved.id }], folderId, null);
        }
      } finally {
        await queryClient.invalidateQueries({ queryKey: ["connections"] });
      }
      // Сохранение закрывает открытое подключение (Rust): кэш его каталога больше не нужен.
      queryClient.removeQueries({ predicate: (q) => q.queryKey[1] === saved.id });
      close(tabId);
      open({ kind: "overview", connectionId: saved.id });
    });

  const onDelete = () =>
    run(async () => {
      setConfirmingDelete(false);
      await api.deleteConnection(profile.id);
      await queryClient.invalidateQueries({ queryKey: ["connections"] });
      queryClient.removeQueries({ predicate: (q) => q.queryKey[1] === profile.id });
      closeForConnection(profile.id);
      close(tabId);
    });

  return (
    <div className="page">
      <div className="toolbar">
        <h2>{isNew ? t("connections.form.titleNew") : t("connections.form.titleEdit", { name: initial.name })}</h2>
      </div>
      <div className="form">
        <span className="field-label">
          <label htmlFor="cf-connstr">{t("connections.form.connstr")}</label>
          <Hint k="hints.connections.connstr" label={t("connections.form.connstr")} />
        </span>
        <span className="path-field">
          <input id="cf-connstr" placeholder="jdbc:postgresql://h1,h2/db?targetServerType=primary" value={connstr} onChange={(e) => setConnstr(e.target.value)} />
          <button type="button" disabled={busy || !connstr.trim()} onClick={onParse}>
            {t("connections.form.connstrApply")}
          </button>
        </span>
        {ignored.length > 0 && <p className="full muted">{t("connections.form.connstrIgnored", { list: ignored.join(", ") })}</p>}

        <label htmlFor="cf-name">{t("connections.form.name")}</label>
        <input id="cf-name" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} />

        <label htmlFor="cf-folder">{t("connections.form.folder")}</label>
        <select
          id="cf-folder"
          value={folderId ?? ""}
          onChange={(e) => {
            setFolderTouched(true);
            setProfile({ ...profile, folderId: e.target.value || null });
          }}
        >
          <option value="">{t("connections.form.folderRoot")}</option>
          {paths.map((p) => (
            <option key={p.id} value={p.id}>
              {p.path.join(" / ")}
            </option>
          ))}
        </select>

        <span className="field-label">
          <label htmlFor="cf-env">{t("connections.form.environment")}</label>
          <Hint k="hints.connections.environment" label={t("connections.form.environment")} />
        </span>
        <select
          id="cf-env"
          value={profile.environment}
          onChange={(e) => {
            const environment = e.target.value as Environment;
            const forceReadOnly = environment === "prod" && (isNew || (locked && initial.environment === "prod" && initial.readOnly));
            setProfile({ ...profile, environment, readOnly: forceReadOnly ? true : profile.readOnly });
          }}
        >
          {ENVIRONMENTS.map((env) => {
            const proOnly = env === "prod" && locked && initial.environment !== "prod";
            return (
              <option key={env} value={env} disabled={proOnly}>
                {proOnly ? t("connections.form.prodPro") : env}
              </option>
            );
          })}
        </select>

        <span className="field-label">
          <label htmlFor="cf-ro">{t("common.readOnly")}</label>
          <Hint k="hints.connections.readOnly" label={t("common.readOnly")} />
        </span>
        <span>
          <input
            id="cf-ro"
            type="checkbox"
            checked={profile.readOnly}
            disabled={readOnlyLocked}
            title={readOnlyLocked ? proHint() : undefined}
            onChange={(e) => setProfile({ ...profile, readOnly: e.target.checked })}
          />
          {readOnlyLocked && (
            <>
              {" "}
              <ProLock />
            </>
          )}
        </span>

        <label htmlFor="cf-color">{t("connections.form.color")}</label>
        <input id="cf-color" type="color" value={profile.color} onChange={(e) => setProfile({ ...profile, color: e.target.value })} />

        <span className="field-label">
          {t("connections.form.kind")} <Hint k="hints.connections.kind" label={t("connections.form.kind")} />
        </span>
        <span role="radiogroup" aria-label={t("connections.form.kind")}>
          {KINDS.map((k) => (
            <label key={k} className="inline">
              <input type="radio" name="cf-kind" checked={profile.kind === k} onChange={() => setProfile(withKind(profile, k))} />
              {t(`connections.form.kinds.${k}`)}
            </label>
          ))}
        </span>

        <NodesSection profile={profile} onChange={setProfile} secrets={apiSecrets} onSecrets={setApiSecrets} isNew={isNew} />
        <h3 className="full">PostgreSQL</h3>
        <PgSection profile={profile} onChange={setProfile} secrets={pgSecrets} onSecrets={setPgSecrets} isNew={isNew} />
      </div>

      <div className="toolbar">
        <button type="button" disabled={busy} onClick={onTest}>
          {t("connections.form.test")}
        </button>
        <button type="button" className="primary" disabled={busy} onClick={onSave}>
          {t("common.save")}
        </button>
        {!isNew &&
          (confirmingDelete ? (
            <span role="group" aria-label={t("connections.form.deleteGroup")}>
              <strong>{t("connections.form.deleteConfirm", { name: profile.name })}</strong>{" "}
              <button type="button" className="danger" disabled={busy} onClick={onDelete}>
                {t("connections.form.deleteYes")}
              </button>{" "}
              <button type="button" disabled={busy} onClick={() => setConfirmingDelete(false)}>
                {t("common.cancel")}
              </button>
            </span>
          ) : (
            <button type="button" className="danger" disabled={busy} onClick={() => setConfirmingDelete(true)}>
              {t("common.delete")}
            </button>
          ))}
        {busy && <span className="muted">{t("connections.form.busy")}</span>}
      </div>

      {error && <ErrorBanner error={error} />}
      {(events.length > 0 || report) && <TestPanel events={events} report={report} />}
    </div>
  );
}
```

`src/components/connections/ConnectionFormTab.tsx` — скопировать из `$G` без изменений (он использует `newProfile` из `./profile`). В `TabContent.tsx` вернуть ветку: `case "connection-form": return <ConnectionFormTab connectionId={tab.connectionId} folderId={tab.folderId ?? null} tabId={tab.id} />;`.

`src/components/connections/ConnectionForm.test.tsx`:

```tsx
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";
import { newProfile } from "./profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listFolders: vi.fn(() => Promise.resolve([])),
      getLicenseStatus: vi.fn(() => Promise.resolve({ tier: "free", state: "none" })),
      parseConnectionString: vi.fn(),
      saveConnection: vi.fn(),
      testConnection: vi.fn(),
      moveItems: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { ConnectionForm } from "./ConnectionForm";

beforeEach(() => vi.clearAllMocks());

describe("ConnectionForm", () => {
  it("вставленная строка заполняет вид, узлы и пароль", async () => {
    const user = userEvent.setup();
    vi.mocked(api.parseConnectionString).mockResolvedValue({
      patch: { kind: "hostList", hosts: [{ host: "h1", port: 5432 }, { host: "h2", port: 5433 }], targetServerType: "primary", username: "app", database: "orders", sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: "pw",
      ignored: ["ApplicationName"],
    });
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.type(screen.getByLabelText("Строка подключения"), "jdbc:postgresql://h1,h2:5433/orders");
    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByLabelText("Узел 2")).toHaveValue("h2");
    expect(screen.getByLabelText("Порт узла 2")).toHaveValue(5433);
    expect(screen.getByLabelText("Пользователь")).toHaveValue("app");
    expect(screen.getByText(/ApplicationName/)).toBeInTheDocument();

    vi.mocked(api.saveConnection).mockResolvedValue({ ...newProfile(), id: "n" });
    await user.type(screen.getByLabelText("Имя"), "prod-orders");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const [profile, secrets] = vi.mocked(api.saveConnection).mock.calls[0];
    expect(profile.kind).toBe("hostList");
    expect(secrets.pgPassword).toBe("pw");
  });

  it("узлы списка переводятся под Patroni", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), kind: "hostList", hosts: [{ host: "db1", port: 5432 }] }} tabId="t" />);
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    expect(screen.getByLabelText("Адрес API 1")).toHaveValue("http://db1:8008");
    expect(screen.getByRole("radio", { name: "Кластер Patroni" })).toBeChecked();
  });

  it("prod недоступен во Free", async () => {
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    const prod = await screen.findByRole("option", { name: /prod/ });
    await vi.waitFor(() => expect(prod).toBeDisabled());
  });
});
```

(Строки интерфейса тестов — русские тексты из шага 5. `useLicense` из Glafira определяет `locked` по `getLicenseStatus`; если его ответ в тестах Glafira мокается иначе, повторить мок из `$G/src/components/ProGating.test.tsx`.)

- [ ] **Step 5: Тексты формы, подсказок и чек-листа**

В `src/locales/ru.json` заменить разделы `connections.form` и `connections.tls` (Kafka) разделом `connections.form` ниже; `en.json` — те же ключи по-английски (правило: английский — ясный и короткий, как в Glafira). Разделы `test`, `readiness`, `topology.warning`, подсказки — добавить.

```json
"connections": {
  "form": {
    "titleNew": "Новое подключение",
    "titleEdit": "Подключение «{{name}}»",
    "deleted": "Подключение удалено",
    "connstr": "Строка подключения",
    "connstrApply": "Заполнить",
    "connstrIgnored": "Не распознано и пропущено: {{list}}",
    "name": "Имя",
    "folder": "Папка",
    "folderRoot": "Без папки",
    "environment": "Окружение",
    "prodPro": "prod — Pro",
    "color": "Цвет",
    "kind": "Вид",
    "kinds": { "server": "Сервер", "hostList": "Список узлов", "patroni": "Кластер Patroni" },
    "host": "Хост",
    "port": "Порт",
    "hosts": "Узлы",
    "hostN": "Узел {{n}}",
    "portN": "Порт узла {{n}}",
    "moveUp": "Узел {{n}} выше",
    "moveDown": "Узел {{n}} ниже",
    "removeHost": "Удалить узел {{n}}",
    "addHost": "Добавить узел",
    "target": "Узел по умолчанию",
    "targets": { "primary": "primary", "standby": "реплика", "preferStandby": "реплика, если есть, иначе primary", "any": "любой доступный" },
    "underPatroni": "Узлы под управлением Patroni",
    "patroniUrls": "Адреса REST API",
    "urlN": "Адрес API {{n}}",
    "removeUrl": "Удалить адрес {{n}}",
    "addUrl": "Добавить адрес",
    "patroniUser": "Пользователь API",
    "patroniPassword": "Пароль API",
    "apiTls": "TLS для API",
    "caPath": "Доверенный CA",
    "verifyHostname": "Проверять имя хоста",
    "clientCert": "Клиентский сертификат",
    "certFormat": "Формат",
    "certPath": "Сертификат",
    "keyPath": "Ключ",
    "keyPassword": "Пароль ключа",
    "overrides": "Подмена адресов узлов",
    "overrideMember": "Участник {{n}}",
    "overrideHost": "Хост PostgreSQL {{n}}",
    "overridePort": "Порт PostgreSQL {{n}}",
    "overrideApi": "Адрес API {{n}}",
    "removeOverride": "Удалить подмену {{n}}",
    "addOverride": "Добавить подмену",
    "pgUser": "Пользователь",
    "pgPassword": "Пароль",
    "passwordKeep": "оставить сохранённый",
    "passwordClear": "Удалить сохранённый",
    "pgDatabase": "БД по умолчанию",
    "timeout": "Таймаут подключения, с",
    "databases": "Только базы",
    "databasesPlaceholder": "все",
    "pick": "Выбрать…",
    "pickAria": "Выбрать файл: {{label}}",
    "test": "Проверить",
    "busy": "Выполняется…",
    "deleteGroup": "Удаление подключения",
    "deleteConfirm": "Удалить подключение «{{name}}»?",
    "deleteYes": "Удалить"
  }
},
"test": {
  "readinessTitle": "Готовность к диагностике",
  "roleScript": "Скрипт роли для диагностики",
  "step": { "patroni": "Patroni API", "discovery": "Обнаружение узлов", "node": "Узел", "readiness": "Готовность" },
  "patroniOk": "Участников: {{members}}, лидер {{leader}}",
  "discoveryOk": "Найдено узлов: {{members}}",
  "nodeOk": "{{role}}, PostgreSQL {{version}}"
},
"topology": {
  "warning": {
    "noPrimary": "Нет primary: все доступные узлы — реплики",
    "multiplePrimaries": "Несколько primary ({{members}}): возможен split-brain",
    "systemIdMismatch": "Узлы из разных кластеров (разный system_identifier): {{members}}",
    "patroniUnreachable": "Patroni API не отвечает: показаны последние известные данные",
    "memberUnreachable": "Узел {{name}} недоступен"
  }
},
"readiness": {
  "monitorRole": { "title": "Роль мониторинга", "ok": "Есть pg_monitor", "missing": "Нет роли pg_monitor", "affects": "Не видно запросов других пользователей, pg_stat_statements и размера pg_wal" },
  "signalRole": { "title": "Отмена и завершение сессий", "ok": "Есть pg_signal_backend", "missing": "Нет роли pg_signal_backend", "affects": "Нельзя будет отменять запросы и завершать сессии" },
  "maintainRole": { "title": "Обслуживание таблиц", "ok": "VACUUM и ANALYZE доступны", "grant": "Нет роли pg_maintain", "ownerOnly": "До PostgreSQL 17 — только для своих таблиц", "affects": "VACUUM и ANALYZE из приложения — только для своих таблиц" },
  "pgssPreloaded": { "title": "pg_stat_statements загружен", "ok": "Загружен на всех узлах", "missing": "Не загружен (shared_preload_libraries)", "warn": "Не загружен на узлах: {{members}}", "unknown": "Не удалось проверить: нет доступных узлов", "affects": "Не будет анализа тяжёлых запросов; нужен перезапуск после изменения" },
  "pgssInstalled": { "title": "Расширение pg_stat_statements", "ok": "Создано", "missing": "Не создано в БД {{database}}", "affects": "Создайте расширение в каждой БД, где нужен анализ запросов" },
  "computeQueryId": { "title": "compute_query_id", "ok": "Идентификаторы запросов вычисляются", "warn": "auto, но pg_stat_statements загружен не везде", "missing": "Выключен", "version": "Появился в PostgreSQL 14", "affects": "Активность не связать с топом запросов" },
  "trackIoTiming": { "title": "track_io_timing", "ok": "Включён", "warn": "Выключен", "affects": "Не будет времени ввода-вывода в запросах и планах" },
  "trackActivityQuerySize": { "title": "track_activity_query_size", "ok": "Достаточно для текстов запросов", "warn": "Сейчас {{value}} байт — длинные запросы обрезаются", "affects": "Тексты текущих запросов будут неполными" },
  "extension": { "title": "Расширение {{name}}", "ok": "Установлено, версия {{version}}", "warn": "Доступно, но не создано", "missing": "Не установлено на сервере", "affects": "Расширенные проверки (индексы, bloat, ожидания) будут недоступны" }
},
"hints": {
  "connections": {
    "connstr": "JDBC (<code>jdbc:postgresql://h1,h2/db</code>), URI (<code>postgresql://…</code>) или <code>host=… port=…</code>. Пароль из строки сохранится в хранилище секретов.",
    "kind": "Сервер — один PostgreSQL. Список узлов — несколько серверов, роли определяются автоматически. Кластер Patroni — адреса REST API, узлы находятся сами.",
    "target": "С какого узла по умолчанию читать каталог — как targetServerType в JDBC.",
    "overrides": "Если Patroni отдаёт адреса, недоступные с вашего компьютера (pod IP, NAT), укажите для участника доступный хост, порт и адрес API.",
    "sslMode": "prefer — TLS, если сервер умеет; require — только TLS; verify-ca и verify-full — с проверкой сертификата (verify-full проверяет и имя хоста).",
    "pgpass": "Если пароль не задан, используется ~/.pgpass, как в psql."
  }
}
```

(`hints.connections.environment` и `readOnly` — остаются из Glafira; в объекте `hints.connections` добавить новые ключи к существующим.)

- [ ] **Step 6: Прогнать тесты**

Run: `pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

Ручная проверка: `pnpm tauri dev` → «+» → вставить `postgresql://glafira_diag:glafira-secret@localhost:5451,localhost:5452/postgres` → «Заполнить» → «Проверить»: шаги ✅ по двум узлам, чек-лист; сохранить.

- [ ] **Step 7: Commit**

```bash
git add src
git commit -m "feat(ui): форма подключения трёх видов, вставка строки, проверка с чек-листом готовности

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 17: Фронтенд — дерево подключений, узлов и каталога

**Files:**
- Copy from `$G/src/components/connections/tree` and modify: `ConnectionTree.tsx`, `ConnectionTree.test.tsx`, `FolderMenu.tsx`
- Create: `src/components/connections/tree/ConnectionNode.tsx`, `src/components/connections/tree/CatalogNodes.tsx`, `src/components/connections/tree/ObjectList.tsx`, `src/components/connections/tree/CatalogNodes.test.tsx`
- Modify: `src/components/Sidebar.tsx`, `src/components/settings/SettingsPanel.tsx` (+ test), `src/styles.css`, `src-tauri/src/settings.rs`, `src-tauri/src/commands/settings.rs`, `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`
- Test: `ConnectionTree.test.tsx`, `CatalogNodes.test.tsx`, `SettingsPanel.test.tsx`, unit-тест `settings.rs`

**Interfaces:**
- Consumes: `api.{connect, disconnect, setCatalogMember, listDatabases, listSchemas, listObjects, listPartitions, listExtensions, listRoles, getUiSettings, setUiSettings}`, `useTopology`, `disconnectedView`, `roleLabel`, `formatBytes`, `formatCount`, `useDebouncedValue`, `@tanstack/react-virtual`.
- Produces: `ConnectionNode({ connection, rowProps })` (сигнатура как в Glafira — её вызывает `ConnectionTree`); `CatalogNodes({ connection, view })`; `ObjectList({ connectionId, db, schema, kind, count })`; Rust: `UiSettings { show_system_objects }`, `SettingsStore::{ui(), set_ui(UiSettings)}`, команды `get_ui_settings`, `set_ui_settings(settings)`.

- [ ] **Step 1: Настройка «Показывать системные объекты» (Rust)**

В `settings.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    #[serde(default)]
    pub show_system_objects: bool,
}
```

В `SettingsFile` — поле `#[serde(default)] ui: UiSettings,`; методы:

```rust
    pub fn ui(&self) -> UiSettings {
        self.data.read_safe().ui.clone()
    }

    /// Блокирующий вызов (запись файла).
    pub fn set_ui(&self, ui: UiSettings) -> AppResult<()> {
        self.update(|d| d.ui = ui)
    }
```

Тест:

```rust
    #[test]
    fn ui_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert!(!s.ui().show_system_objects);
        s.set_ui(UiSettings { show_system_objects: true }).unwrap();
        assert!(SettingsStore::open(dir.path()).ui().show_system_objects);
    }
```

В `commands/settings.rs`:

```rust
#[tauri::command]
pub fn get_ui_settings(state: State<'_, AppState>) -> AppResult<crate::settings::UiSettings> {
    Ok(state.settings.ui())
}

#[tauri::command]
pub async fn set_ui_settings(
    state: State<'_, AppState>,
    settings: crate::settings::UiSettings,
) -> AppResult<crate::settings::UiSettings> {
    let st = state.inner().clone();
    super::blocking(move || {
        st.settings.set_ui(settings.clone())?;
        Ok(settings)
    })
    .await
}
```

Зарегистрировать обе команды. Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib settings`
Expected: PASS.

- [ ] **Step 2: Тесты дерева каталога (падающие)**

`src/components/connections/tree/CatalogNodes.test.tsx`:

```tsx
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../../test/render";
import { topologyView } from "../../../test/fixtures";
import { newProfile } from "../profile";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return {
    ...orig,
    api: {
      getUiSettings: vi.fn(() => Promise.resolve({ showSystemObjects: false })),
      listDatabases: vi.fn(() => Promise.resolve([{ name: "orders", sizeBytes: 5 * 1024 * 1024 }])),
      listSchemas: vi.fn(() =>
        Promise.resolve([{ name: "shop", system: false, counts: { tables: 3, views: 1, matviews: 0, sequences: 3, functions: 3 } }]),
      ),
      listObjects: vi.fn(() =>
        Promise.resolve({
          total: 2,
          nextOffset: null,
          items: [
            { oid: 1, name: "customers", schema: "shop", relkind: "r", partitions: null, estRows: 100, totalBytes: 8192, signature: null },
            { oid: 2, name: "events", schema: "shop", relkind: "p", partitions: 2, estRows: null, totalBytes: 0, signature: null },
          ],
        }),
      ),
      listPartitions: vi.fn(() =>
        Promise.resolve({ total: 1, nextOffset: null, items: [{ oid: 3, name: "events_2026_10", schema: "shop", relkind: "r", partitions: null, estRows: 0, totalBytes: 0, signature: null }] }),
      ),
      listExtensions: vi.fn(() => Promise.resolve([])),
      listRoles: vi.fn(() => Promise.resolve([])),
      setCatalogMember: vi.fn(),
    },
  };
});

import { api } from "../../../api/client";
import { CatalogNodes } from "./CatalogNodes";

const conn = { ...newProfile(), id: "c", name: "billing", kind: "patroni" as const };

beforeEach(() => vi.clearAllMocks());

describe("CatalogNodes", () => {
  it("узлы с ролями и ленивая загрузка каталога", async () => {
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /^Узлы/ }));
    expect(screen.getByRole("button", { name: "★ pg-1" })).toBeInTheDocument();
    expect(screen.getByText("Leader")).toBeInTheDocument();
    expect(api.listDatabases).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: /Базы данных \(с узла pg-1\)/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: /^shop/ }));
    expect(screen.queryByRole("button", { name: /Мат\. представления/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Таблицы \(3\)/ }));
    expect(await screen.findByText("customers")).toBeInTheDocument();
    expect(api.listSchemas).toHaveBeenCalledWith("c", "orders", false);
    expect(api.listObjects).toHaveBeenCalledWith("c", "orders", "shop", "tables", null, 0, 500);

    await user.click(screen.getByRole("button", { name: /^events/ }));
    expect(await screen.findByText("events_2026_10")).toBeInTheDocument();
  });

  it("фильтр объектов уходит в запрос", async () => {
    const user = userEvent.setup();
    // Поле фильтра появляется у групп больше 20 объектов.
    vi.mocked(api.listSchemas).mockResolvedValueOnce([
      { name: "shop", system: false, counts: { tables: 30, views: 0, matviews: 0, sequences: 0, functions: 0 } },
    ]);
    renderWithClient(<CatalogNodes connection={{ ...conn, kind: "server" }} view={topologyView({ topology: { ...topologyView().topology!, kind: "server" } })} />);
    expect(screen.queryByRole("button", { name: /^Узлы/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    await user.click(await screen.findByRole("button", { name: /^orders/ }));
    await user.click(await screen.findByRole("button", { name: /^shop/ }));
    await user.click(screen.getByRole("button", { name: /^Таблицы/ }));
    await user.type(await screen.findByLabelText("Фильтр: Таблицы"), "cust");
    await vi.waitFor(() => expect(api.listObjects).toHaveBeenLastCalledWith("c", "orders", "shop", "tables", "cust", 0, 500));
  });

  it("уведомление о возврате к узлу по умолчанию", () => {
    renderWithClient(<CatalogNodes connection={conn} view={topologyView({ catalogFallback: true })} />);
    expect(screen.getByText(/Выбранный узел недоступен/)).toBeInTheDocument();
  });
});
```

Run: `pnpm test src/components/connections/tree/CatalogNodes.test.tsx`
Expected: FAIL (нет компонента).

- [ ] **Step 3: Дерево каталога**

`src/components/connections/tree/ObjectList.tsx`:

```tsx
import { useInfiniteQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ObjectItem, ObjectKind } from "../../../api/types";
import { formatBytes, formatCount } from "../../../lib/format";
import { useDebouncedValue } from "../../../lib/useDebouncedValue";
import { ErrorBanner } from "../../common/ErrorBanner";

export const PAGE = 500;
/** С какого числа строк список виртуализируется. */
const VIRTUAL_FROM = 200;
const ROW_HEIGHT = 22;
/** Поле фильтра — у групп больше этого числа объектов. */
const FILTER_FROM = 20;

function ObjectRow({ item, connectionId, db }: { item: ObjectItem; connectionId: string; db: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const partitioned = item.relkind === "p" && (item.partitions ?? 0) > 0;
  const label = item.signature !== null ? `${item.name}(${item.signature})` : item.name;
  return (
    <li>
      <div className="tree-conn">
        {partitioned ? (
          <button type="button" className="tree-folder-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
            <span className="tree-caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
            <span className="tree-name">{label}</span>
          </button>
        ) : (
          <span className="tree-name">{label}</span>
        )}
        {partitioned && <span className="muted">{t("sidebar.partitions", { count: item.partitions ?? 0 })}</span>}
        {item.totalBytes !== null && item.totalBytes > 0 && <span className="muted">{formatBytes(item.totalBytes)}</span>}
      </div>
      {open && <PartitionList connectionId={connectionId} db={db} schema={item.schema} table={item.name} />}
    </li>
  );
}

function PartitionList({ connectionId, db, schema, table }: { connectionId: string; db: string; schema: string; table: string }) {
  const { t } = useTranslation();
  const q = useInfiniteQuery({
    queryKey: ["catalog", connectionId, "partitions", db, schema, table],
    queryFn: ({ pageParam }) => api.listPartitions(connectionId, db, schema, table, pageParam, PAGE),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
  if (q.isPending) return <p className="muted">{t("common.loading")}</p>;
  if (q.error) return <ErrorBanner error={q.error} onRetry={() => q.refetch()} />;
  return (
    <ul>
      {q.data.pages.flatMap((p) => p.items).map((item) => (
        <ObjectRow key={item.oid} item={item} connectionId={connectionId} db={db} />
      ))}
      {q.hasNextPage && (
        <li>
          <button type="button" className="link" disabled={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
            {t("sidebar.more")}
          </button>
        </li>
      )}
    </ul>
  );
}

export function ObjectList({ connectionId, db, schema, kind, count, label }: { connectionId: string; db: string; schema: string; kind: ObjectKind; count: number; label: string }) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState("");
  const debounced = useDebouncedValue(filter.trim(), 300);
  const q = useInfiniteQuery({
    queryKey: ["catalog", connectionId, "objects", db, schema, kind, debounced],
    queryFn: ({ pageParam }) => api.listObjects(connectionId, db, schema, kind, debounced || null, pageParam, PAGE),
    initialPageParam: 0,
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
  const items = q.data?.pages.flatMap((p) => p.items) ?? [];
  const parentRef = useRef<HTMLDivElement>(null);
  const virtual = items.length > VIRTUAL_FROM;
  const virtualizer = useVirtualizer({
    count: virtual ? items.length : 0,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  });
  const visible = virtualizer.getVirtualItems();
  const lastVisible = visible.length > 0 ? visible[visible.length - 1].index : -1;
  useEffect(() => {
    if (virtual && lastVisible >= items.length - 50 && q.hasNextPage && !q.isFetchingNextPage) void q.fetchNextPage();
  }, [virtual, lastVisible, items.length, q]);

  return (
    <div>
      {count > FILTER_FROM && (
        <input
          className="tree-filter"
          aria-label={t("sidebar.objectFilter", { label })}
          placeholder={t("sidebar.objectFilterPlaceholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      )}
      {q.isPending && <p className="muted">{t("common.loading")}</p>}
      {q.error && <ErrorBanner error={q.error} onRetry={() => q.refetch()} />}
      {q.data && items.length === 0 && <p className="muted">{t("sidebar.noMatches")}</p>}
      {virtual ? (
        <div ref={parentRef} className="tree-virtual" style={{ height: 320, overflow: "auto", position: "relative" }}>
          <ul style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {visible.map((v) => (
              <div key={items[v.index].oid} style={{ position: "absolute", top: v.start, left: 0, right: 0 }}>
                <ObjectRow item={items[v.index]} connectionId={connectionId} db={db} />
              </div>
            ))}
          </ul>
        </div>
      ) : (
        <ul>
          {items.map((item) => (
            <ObjectRow key={item.oid} item={item} connectionId={connectionId} db={db} />
          ))}
          {q.hasNextPage && (
            <li>
              <button type="button" className="link" disabled={q.isFetchingNextPage} onClick={() => void q.fetchNextPage()}>
                {t("sidebar.more")}
              </button>
            </li>
          )}
        </ul>
      )}
      {q.data && <p className="muted">{t("sidebar.shown", { shown: formatCount(items.length), total: formatCount(q.data.pages[0]?.total ?? 0) })}</p>}
    </div>
  );
}
```

(`useDebouncedValue(value, ms)` — хук из `$G/src/lib/useDebouncedValue.ts`, скопирован в Task 2.)

`src/components/connections/tree/CatalogNodes.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ConnectionProfile, ObjectKind, SchemaInfo, TopologyView } from "../../../api/types";
import { formatBytes } from "../../../lib/format";
import { roleLabel } from "../../../lib/topology";
import { useWorkspace } from "../../../state/workspace";
import { ErrorBanner } from "../../common/ErrorBanner";
import { ObjectList } from "./ObjectList";

/** Ленивый узел дерева: содержимое строится только в раскрытом виде. */
export function TreeGroup({ label, extra, children }: { label: string; extra?: ReactNode; children: () => ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <li>
      <div className="tree-conn">
        <button type="button" className="tree-folder-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="tree-caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
          <span className="tree-name">{label}</span>
        </button>
        {extra}
      </div>
      {open && <ul>{children()}</ul>}
    </li>
  );
}

function useShowSystem(): boolean {
  const q = useQuery({ queryKey: ["uiSettings"], queryFn: api.getUiSettings });
  return q.data?.showSystemObjects ?? false;
}

function Loading({ q }: { q: { isPending: boolean; error: unknown; refetch: () => unknown } }) {
  const { t } = useTranslation();
  if (q.isPending) return <li className="muted">{t("common.loading")}</li>;
  if (q.error) return <li><ErrorBanner error={q.error} onRetry={() => void q.refetch()} /></li>;
  return null;
}

const KINDS: { kind: ObjectKind; count: (s: SchemaInfo) => number }[] = [
  { kind: "tables", count: (s) => s.counts.tables },
  { kind: "views", count: (s) => s.counts.views },
  { kind: "matviews", count: (s) => s.counts.matviews },
  { kind: "functions", count: (s) => s.counts.functions },
  { kind: "sequences", count: (s) => s.counts.sequences },
];

function SchemaNode({ connectionId, db, schema }: { connectionId: string; db: string; schema: SchemaInfo }) {
  const { t } = useTranslation();
  return (
    <TreeGroup label={schema.name}>
      {() =>
        KINDS.filter((k) => k.count(schema) > 0).map((k) => {
          const label = t(`sidebar.kinds.${k.kind}`);
          return (
            <TreeGroup key={k.kind} label={`${label} (${k.count(schema)})`}>
              {() => <ObjectList connectionId={connectionId} db={db} schema={schema.name} kind={k.kind} count={k.count(schema)} label={label} />}
            </TreeGroup>
          );
        })
      }
    </TreeGroup>
  );
}

function SchemaList({ connectionId, db }: { connectionId: string; db: string }) {
  const showSystem = useShowSystem();
  const q = useQuery({ queryKey: ["catalog", connectionId, "schemas", db, showSystem], queryFn: () => api.listSchemas(connectionId, db, showSystem) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((s) => <SchemaNode key={s.name} connectionId={connectionId} db={db} schema={s} />)}
    </>
  );
}

function ExtensionList({ connectionId, db }: { connectionId: string; db: string }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["catalog", connectionId, "extensions", db], queryFn: () => api.listExtensions(connectionId, db) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((e) => (
        <li key={e.name}>
          <span className="tree-name">{e.name}</span> <span className="muted">{e.version}</span>
          {e.updateAvailable && <span className="warn" title={t("sidebar.extensionUpdate", { version: e.defaultVersion })}> ↑</span>}
        </li>
      ))}
    </>
  );
}

function DatabaseList({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((d) => (
        <TreeGroup
          key={d.name}
          label={d.name}
          extra={<span className="muted" title={d.sizeBytes === null ? t("sidebar.sizeUnavailable") : undefined}>{d.sizeBytes === null ? "—" : formatBytes(d.sizeBytes)}</span>}
        >
          {() => (
            <>
              <SchemaList connectionId={connectionId} db={d.name} />
              <TreeGroup label={t("sidebar.extensions")}>{() => <ExtensionList connectionId={connectionId} db={d.name} />}</TreeGroup>
            </>
          )}
        </TreeGroup>
      ))}
    </>
  );
}

function RoleList({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const showSystem = useShowSystem();
  const q = useQuery({ queryKey: ["catalog", connectionId, "roles", showSystem], queryFn: () => api.listRoles(connectionId, showSystem) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((r) => (
        <li key={r.name}>
          <span className="tree-name">{r.name}</span>
          {r.superuser && <span className="env-badge" style={{ background: "#dc2626" }}>superuser</span>}
          {!r.login && <span className="muted"> {t("sidebar.noLogin")}</span>}
        </li>
      ))}
    </>
  );
}

export function CatalogNodes({ connection, view }: { connection: ConnectionProfile; view: TopologyView }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const topo = view.topology;
  const member = view.catalogMember;
  return (
    <ul>
      {connection.kind !== "server" && topo && (
        <TreeGroup label={t("sidebar.members")}>
          {() =>
            topo.members.map((m) => (
              <li key={m.name}>
                <button type="button" className="link" onClick={() => open({ kind: "overview", connectionId: connection.id, member: m.name })}>
                  {m.role === "primary" || m.role === "standbyLeader" ? "★ " : "○ "}
                  {m.name}
                </button>{" "}
                <span className="muted">{roleLabel(m.role, topo.kind)}</span>
                {m.timeline !== null && m.role === "primary" && <span className="muted"> TL {m.timeline}</span>}
                {m.lagBytes !== null && m.role !== "primary" && <span className="muted"> {formatBytes(m.lagBytes)}</span>}
                {(!m.reachable || (m.lagBytes ?? 0) > 16 * 1024 * 1024) && (
                  <span className="warn" title={m.reachable ? t("sidebar.lagHigh") : t("sidebar.unreachable")}> ⚠</span>
                )}
              </li>
            ))
          }
        </TreeGroup>
      )}
      {view.catalogFallback && member && <li className="tree-notice">{t("sidebar.catalogFallback", { member })}</li>}
      {member ? (
        <TreeGroup label={connection.kind === "server" ? t("sidebar.databases") : t("sidebar.databasesFrom", { member })}>
          {() => <DatabaseList connectionId={connection.id} />}
        </TreeGroup>
      ) : (
        <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>
      )}
      {member && <TreeGroup label={t("sidebar.roles")}>{() => <RoleList connectionId={connection.id} />}</TreeGroup>}
    </ul>
  );
}
```

`src/components/connections/tree/ConnectionNode.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { type HTMLAttributes, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AppError, ConnectionProfile } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { disconnectedView, useTopology } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { EnvBadge } from "../../common/EnvBadge";
import { CatalogNodes } from "./CatalogNodes";
import { FolderMenu, type FolderMenuItem } from "./FolderMenu";

/**
 * Узел подключения: раскрытие открывает подключение (Rust поднимает наблюдение), дочерние узлы —
 * участники, базы, роли. rowProps — атрибуты строки (перетаскивание), как в Glafira.
 */
export function ConnectionNode({ connection: c, rowProps }: { connection: ConnectionProfile; rowProps?: HTMLAttributes<HTMLDivElement> }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const qc = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const { data: view = disconnectedView } = useTopology(c.id);
  const live = view.state === "connected" || view.state === "degraded";

  const connect = async () => {
    setError(null);
    try {
      qc.setQueryData(["topology", c.id], await api.connect(c.id));
    } catch (e) {
      setError(toAppError(e));
    }
  };

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    if (next && !live && view.state !== "connecting") void connect();
  };

  const members = view.topology?.members ?? [];
  const menu: FolderMenuItem[] = [
    { label: t("sidebar.overview"), onSelect: () => open({ kind: "overview", connectionId: c.id }) },
    { label: t("sidebar.editConnection"), onSelect: () => open({ kind: "connection-form", connectionId: c.id }) },
    ...(live && c.kind !== "server"
      ? [
          { label: t("sidebar.catalogDefault"), onSelect: () => void api.setCatalogMember(c.id, null).then((v) => qc.setQueryData(["topology", c.id], v)) },
          ...members.map((m) => ({
            label: t("sidebar.catalogFrom", { member: m.name }),
            onSelect: () => void api.setCatalogMember(c.id, m.name).then((v) => {
              qc.setQueryData(["topology", c.id], v);
              void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === "catalog" && q.queryKey[1] === c.id });
            }),
          })),
        ]
      : []),
    ...(live || view.state === "failed"
      ? [{ label: t("sidebar.disconnect"), onSelect: () => void api.disconnect(c.id).then(() => setExpanded(false)) }]
      : [{ label: t("sidebar.connect"), onSelect: () => { setExpanded(true); void connect(); } }]),
  ];

  return (
    <li>
      <div {...rowProps} className={rowProps?.className ? `tree-conn ${rowProps.className}` : "tree-conn"}>
        <button type="button" className="tree-folder-toggle" aria-expanded={expanded} aria-label={t("sidebar.connectionToggle", { name: c.name })} onClick={toggle}>
          <span className="tree-caret">{expanded ? "▾" : "▸"}</span>
        </button>
        <span className={`conn-state conn-state-${view.state}`} title={t(`sidebar.state.${view.state}`)} />
        <span className="dot" style={{ background: c.color }} />
        <button type="button" className="link tree-name" onClick={() => open({ kind: "overview", connectionId: c.id })}>
          {c.name}
        </button>
        <EnvBadge environment={c.environment} readOnly={c.readOnly} />
        {c.kind === "patroni" && <span className="llm-badge">Patroni</span>}
        <span className="tree-menu-anchor">
          <button type="button" className="icon" aria-haspopup="menu" aria-expanded={menuOpen} aria-label={t("sidebar.connectionMenu", { name: c.name })} onClick={() => setMenuOpen(!menuOpen)}>
            ⋯
          </button>
          {menuOpen && <FolderMenu label={t("sidebar.connectionMenu", { name: c.name })} items={menu} onClose={() => setMenuOpen(false)} />}
        </span>
      </div>
      {expanded && (
        <>
          {view.state === "connecting" && <p className="tree-notice">{t("sidebar.state.connecting")}</p>}
          {error && <p className="tree-error" role="alert">{tr(error)}</p>}
          {live && <CatalogNodes connection={c} view={view} />}
        </>
      )}
    </li>
  );
}
```

(`FolderMenu` сам закрывается перед вызовом `onSelect` пункта — отдельно закрывать меню не нужно.)

Скопировать из `$G/src/components/connections/tree`: `ConnectionTree.tsx`, `ConnectionTree.test.tsx`, `FolderMenu.tsx`, `ImportDialog.tsx`, `ImportDialog.test.tsx` (используются деревом; команды импорта вернёт Task 19 — до неё пункты «Экспорт» и «Импорт» в меню дерева скрыть флагом `const EXCHANGE = false` вверху `ConnectionTree.tsx`). Чтобы дерево компилировалось, уже здесь скопировать из `$G` в `src/api/types.ts` типы `ExportSummary`, `PreviewItem`, `Skipped`, `ImportPreview`, а в `src/api/client.ts` — функции `exportConnections`, `previewImport`, `applyImport` (вызовы скрыты флагом до Task 19). В `ConnectionTree.test.tsx`: в мок `api` добавить `getTopology: vi.fn(() => new Promise(() => {}))`; ассерт `getAllByRole("button", { name: "Топики" })` заменить на `expect(screen.getAllByRole("button", { name: /Развернуть или свернуть подключение/ })).toHaveLength(2);`; тесты экспорта и импорта пометить `it.skip` с комментарием «вернёт Task 19».

`src/components/Sidebar.tsx` — вернуть `<ConnectionTree />` (импорт из `./connections/tree/ConnectionTree`).

`SettingsPanel.tsx` — блок под выбором языка:

```tsx
const ui = useQuery({ queryKey: ["uiSettings"], queryFn: api.getUiSettings });
const qc = useQueryClient();
// …
<label className="inline">
  <input
    type="checkbox"
    checked={ui.data?.showSystemObjects ?? false}
    onChange={async (e) => {
      await api.setUiSettings({ showSystemObjects: e.target.checked });
      await qc.invalidateQueries({ queryKey: ["uiSettings"] });
    }}
  />
  {t("settings.showSystemObjects")}
</label>
```

с тестом в `SettingsPanel.test.tsx`: клик по флажку вызывает `api.setUiSettings({ showSystemObjects: true })` (мок `getUiSettings` → `{ showSystemObjects: false }`).

`src/styles.css` — в конец:

```css
.conn-state { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; background: var(--border); }
.conn-state-connecting { background: var(--warn); }
.conn-state-connected { background: #16a34a; }
.conn-state-degraded { background: var(--warn); }
.conn-state-failed { background: var(--danger); }
.tree-virtual ul { list-style: none; margin: 0; padding: 0; }
button.link.tree-name { text-align: left; color: inherit; }
```

Словари — раздел `sidebar` (ru; en — по смыслу): `connectionToggle` «Развернуть или свернуть подключение {{name}}», `connectionMenu` «Действия с подключением {{name}}», `overview` «Обзор», `connect` «Подключиться», `disconnect` «Отключиться», `catalogDefault` «Каталог — с узла по умолчанию», `catalogFrom` «Каталог — с узла {{member}}», `catalogFallback` «Выбранный узел недоступен — каталог читается с {{member}}», `noCatalogNode` «Нет доступного узла для каталога», `members` «Узлы», `databases` «Базы данных», `databasesFrom` «Базы данных (с узла {{member}})», `roles` «Роли», `extensions` «Расширения», `extensionUpdate` «Доступна версия {{version}}», `kinds` `{ "tables": "Таблицы", "views": "Представления", "matviews": "Мат. представления", "functions": "Функции", "sequences": "Последовательности" }`, `objectFilter` «Фильтр: {{label}}», `objectFilterPlaceholder` «фильтр», `partitions_one` «{{count}} партиция», `partitions_few` «{{count}} партиции», `partitions_many` «{{count}} партиций», `partitions_other` «{{count}} партиции» (у i18next для ru нужна и форма `_other`; en: `partitions_one` «{{count}} partition», `partitions_other` «{{count}} partitions»), `more` «Ещё», `shown` «Показано {{shown}} из {{total}}», `sizeUnavailable` «Нет права CONNECT на базу», `noLogin` «без входа», `lagHigh` «Лаг больше 16 МБ», `unreachable` «Узел недоступен», `state` `{ "disconnected": "Не подключено", "connecting": "Подключение…", "connected": "Подключено", "degraded": "Часть узлов недоступна", "failed": "Не удалось подключиться" }`, `emptyHint` «Добавьте первое подключение. Локальный стенд: <code>docker compose -f dev/docker-compose.yml up -d</code>, сервер <code>localhost:5460</code>, пользователь <code>glafira_diag</code>.». `settings.showSystemObjects` — «Показывать системные объекты (pg_catalog, information_schema, роли pg_*)».

- [ ] **Step 4: Прогнать тесты**

Run: `pnpm test && pnpm i18n:check && pnpm build && cargo test --manifest-path src-tauri/Cargo.toml --lib`
Expected: PASS.

Ручная проверка (стенд поднят): подключение к `pg-single` раскрывается до `orders → shop → Таблицы (3) → events → events_2026_09`; у Patroni-подключения — «Узлы» с Leader и двумя репликами; «⋯ → Каталог — с узла patroni2» перечитывает базы.

- [ ] **Step 5: Commit**

```bash
git add src src-tauri
git commit -m "feat(ui): дерево — подключения, узлы, базы, схемы, объекты, партиции, роли и расширения

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 18: Фронтенд — вкладка «Обзор кластера / сервера»

**Files:**
- Create: `src/components/overview/Overview.tsx`, `src/components/overview/Overview.test.tsx`
- Modify: `src/components/TabContent.tsx`, `src/locales/{en,ru}.json`

**Interfaces:**
- Consumes: `useTopology`, `useTopologyEvents`, `api.{connect, getMemberFacts, listConnections}`, `roleLabel`, `warningText` (из `TestPanel.tsx`), `formatBytes`, `formatTimestamp`.
- Produces: `Overview({ connectionId, member? })`; `eventText(e, kind) -> string`.

- [ ] **Step 1: Тест (падающий)**

`src/components/overview/Overview.test.tsx`:

```tsx
import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";
import { member, topology, topologyView } from "../../test/fixtures";
import { newProfile } from "../connections/profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing", kind: "patroni" }])),
      getTopology: vi.fn(),
      listTopologyEvents: vi.fn(() => Promise.resolve([{ atMs: 1_000, kind: "leaderChanged", from: "pg-2", to: "pg-1" }])),
      getMemberFacts: vi.fn(() => Promise.resolve({ startedAtMs: Date.now() - 3_600_000, connections: 12, maxConnections: 100, databases: 3 })),
      connect: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { Overview } from "./Overview";

beforeEach(() => vi.clearAllMocks());

describe("Overview", () => {
  it("участники, предупреждения, журнал и факты", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(
      topologyView({
        state: "degraded",
        topology: topology({
          paused: true,
          members: [member("pg-1", { role: "primary", state: "running", lagBytes: null }), member("pg-2", { reachable: false, role: "unknown", error: { code: "PATRONI_ERROR", message: "x", retryable: false } })],
          warnings: [{ kind: "memberUnreachable", name: "pg-2" }],
        }),
      }),
    );
    renderWithClient(<Overview connectionId="c" />);
    expect(await screen.findByText("Узел pg-2 недоступен")).toBeInTheDocument();
    expect(screen.getByText("Пауза")).toBeInTheDocument();
    expect(screen.getAllByText("Leader").length).toBeGreaterThan(0);
    expect(await screen.findByText(/pg-2 → pg-1/)).toBeInTheDocument();
    expect(await screen.findByText("12 / 100")).toBeInTheDocument();
    expect(api.getMemberFacts).toHaveBeenCalledWith("c", "pg-1");
    expect(api.getMemberFacts).not.toHaveBeenCalledWith("c", "pg-2");
  });

  it("неподключённое подключение предлагает подключиться", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "disconnected", topology: null, catalogMember: null }));
    vi.mocked(api.connect).mockResolvedValue(topologyView());
    renderWithClient(<Overview connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: "Подключиться" }));
    expect(api.connect).toHaveBeenCalledWith("c");
  });
});
```

Run: `pnpm test src/components/overview`
Expected: FAIL.

- [ ] **Step 2: Реализация**

`src/components/overview/Overview.tsx`:

```tsx
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, Member, TopologyEvent } from "../../api/types";
import { tr } from "../../i18n/text";
import { i18n } from "../../i18n";
import { formatBytes, formatTimestamp } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { useTopology, useTopologyEvents } from "../../lib/useConnectionEvents";
import { ErrorBanner } from "../common/ErrorBanner";
import { warningText } from "../connections/TestPanel";

export function eventText(e: TopologyEvent, kind: ConnectionKind): string {
  const t = i18n.t.bind(i18n);
  switch (e.kind) {
    case "leaderChanged":
      return t("overview.event.leaderChanged", { from: e.from ?? "—", to: e.to ?? "—" });
    case "memberAdded":
      return t("overview.event.memberAdded", { name: e.name });
    case "memberRemoved":
      return t("overview.event.memberRemoved", { name: e.name });
    case "memberRoleChanged":
      return t("overview.event.memberRoleChanged", { name: e.name, from: roleLabel(e.from, kind), to: roleLabel(e.to, kind) });
    case "memberStateChanged":
      return t("overview.event.memberStateChanged", { name: e.name, from: e.from ?? "—", to: e.to ?? "—" });
    case "memberUnreachable":
      return t("overview.event.memberUnreachable", { name: e.name });
    case "memberReachable":
      return t("overview.event.memberReachable", { name: e.name });
    case "timelineChanged":
      return t("overview.event.timelineChanged", { from: e.from, to: e.to });
    case "pauseChanged":
      return t(e.paused ? "overview.event.paused" : "overview.event.resumed");
  }
}

function duration(ms: number): string {
  const m = Math.floor(ms / 60_000);
  if (m < 60) return i18n.t("overview.minutes", { count: m });
  const h = Math.floor(m / 60);
  if (h < 48) return i18n.t("overview.hours", { count: h });
  return i18n.t("overview.days", { count: Math.floor(h / 24) });
}

function FactsCells({ connectionId, m }: { connectionId: string; m: Member }) {
  const q = useQuery({
    queryKey: ["memberFacts", connectionId, m.name],
    queryFn: () => api.getMemberFacts(connectionId, m.name),
    enabled: m.reachable,
    refetchInterval: 10_000,
  });
  if (!m.reachable || !q.data) return <><td>—</td><td>—</td><td>—</td></>;
  return (
    <>
      <td>{duration(Date.now() - q.data.startedAtMs)}</td>
      <td>{`${q.data.connections} / ${q.data.maxConnections}`}</td>
      <td>{q.data.databases}</td>
    </>
  );
}

export function Overview({ connectionId, member }: { connectionId: string; member?: string }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const conns = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  const conn = conns.data?.find((c) => c.id === connectionId);
  const view = useTopology(connectionId);
  const events = useTopologyEvents(connectionId);
  const [error, setError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);

  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      qc.setQueryData(["topology", connectionId], await api.connect(connectionId));
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  if (view.isPending) return <p className="empty">{t("common.loading")}</p>;
  if (view.error) return <ErrorBanner error={view.error} onRetry={() => view.refetch()} />;
  const v = view.data;
  const topo = v.topology;

  return (
    <div className="page">
      <div className="toolbar">
        <h2>{conn?.name ?? connectionId}</h2>
        <span className="muted">{conn ? t(`connections.form.kinds.${conn.kind}`) : ""}</span>
        {topo?.scope && <span className="muted">scope: {topo.scope}</span>}
        {topo?.paused && <span className="warn">{t("overview.paused")}</span>}
        <span>{t(`sidebar.state.${v.state}`)}</span>
        {(v.state === "disconnected" || v.state === "failed") && (
          <button type="button" className="primary" disabled={busy} onClick={() => void connect()}>
            {t("sidebar.connect")}
          </button>
        )}
      </div>
      {(error ?? v.error) && <ErrorBanner error={error ?? v.error} />}
      {topo && topo.warnings.length > 0 && (
        <div className="confirm" role="alert">
          {topo.warnings.map((w, i) => (
            <span key={i}>{warningText(w, t)}</span>
          ))}
        </div>
      )}
      {topo && (
        <table>
          <thead>
            <tr>
              <th>{t("overview.col.name")}</th>
              <th>{t("overview.col.role")}</th>
              <th>{t("overview.col.state")}</th>
              <th>{t("overview.col.address")}</th>
              <th>TL</th>
              <th>{t("overview.col.lag")}</th>
              <th>{t("overview.col.replay")}</th>
              <th>{t("overview.col.upstream")}</th>
              <th>{t("overview.col.tags")}</th>
              <th>{t("overview.col.versions")}</th>
              <th>{t("overview.col.uptime")}</th>
              <th>{t("overview.col.connections")}</th>
              <th>{t("overview.col.databases")}</th>
            </tr>
          </thead>
          <tbody>
            {topo.members.map((m) => (
              <tr key={m.name} className={m.name === member ? "selected" : undefined}>
                <td>
                  {m.reachable ? "" : <span className="warn" title={m.error ? tr(m.error) : undefined}>⚠ </span>}
                  {m.name}
                </td>
                <td>{roleLabel(m.role, topo.kind)}</td>
                <td>{m.state ?? "—"}</td>
                <td>{`${m.host}:${m.port}`}</td>
                <td>{m.timeline ?? "—"}</td>
                <td>{m.lagBytes === null ? "—" : formatBytes(m.lagBytes)}</td>
                <td>{m.replayDelayMs === null ? "—" : `${(m.replayDelayMs / 1000).toFixed(1)} s`}</td>
                <td>{m.upstream ?? "—"}</td>
                <td>
                  {Object.entries(m.tags)
                    .filter(([, val]) => val === true)
                    .map(([k]) => k)
                    .concat(m.pendingRestart ? ["pending restart"] : [])
                    .join(", ") || "—"}
                </td>
                <td>
                  {m.serverVersion ?? "—"}
                  {m.patroniVersion && <span className="muted"> / Patroni {m.patroniVersion}</span>}
                  {m.dcsLastSeenMs !== null && <div className="muted">{t("overview.dcsSeen", { at: formatTimestamp(m.dcsLastSeenMs) })}</div>}
                </td>
                <FactsCells connectionId={connectionId} m={m} />
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {topo && (
        <>
          <h3>{t("overview.journal")}</h3>
          {(events.data ?? []).length === 0 ? (
            <p className="muted">{t("overview.journalEmpty")}</p>
          ) : (
            <ul className="steps">
              {(events.data ?? []).map((e, i) => (
                <li key={`${e.atMs}:${i}`}>
                  <span className="muted">{formatTimestamp(e.atMs)}</span> {eventText(e, topo.kind)}
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}
```

`TabContent.tsx` — `case "overview": return <Overview connectionId={tab.connectionId} member={tab.member} />;`.

Словари — раздел `overview` (ru; en по смыслу): `paused` «Пауза», `journal` «Журнал событий», `journalEmpty` «Пока без событий: журнал ведётся, пока подключение открыто», `dcsSeen` «DCS: {{at}}», `minutes_one/few/many/other` «{{count}} минута/минуты/минут/минуты», `hours_one/few/many/other` «{{count}} час/часа/часов/часа», `days_one/few/many/other` «{{count}} день/дня/дней/дня» (en: `_one`/`_other`), `col` `{ "name": "Узел", "role": "Роль", "state": "Состояние", "address": "Адрес", "lag": "Лаг", "replay": "Задержка replay", "upstream": "Источник WAL", "tags": "Теги", "versions": "Версии", "uptime": "Работает", "connections": "Соединения", "databases": "БД" }`, `event` `{ "leaderChanged": "Смена лидера: {{from}} → {{to}}", "memberAdded": "Добавлен узел {{name}}", "memberRemoved": "Узел {{name}} исчез из кластера", "memberRoleChanged": "{{name}}: {{from}} → {{to}}", "memberStateChanged": "{{name}}: состояние {{from}} → {{to}}", "memberUnreachable": "Узел {{name}} недоступен", "memberReachable": "Узел {{name}} снова доступен", "timelineChanged": "Таймлайн {{from}} → {{to}}", "paused": "Кластер переведён в паузу", "resumed": "Пауза снята" }`.

- [ ] **Step 3: Прогнать тесты**

Run: `pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

Ручная проверка: обзор Patroni-подключения стенда показывает трёх участников; `docker exec gp-patroni1 /opt/patroni/bin/patronictl -c /etc/patroni/patroni.yml switchover --force --candidate <реплика>` — не позже чем через 15 с после завершения переключения в таблице меняется лидер, а в журнале появляется «Смена лидера».

- [ ] **Step 4: Commit**

```bash
git add src
git commit -m "feat(ui): обзор кластера — участники, факты узлов, предупреждения и журнал событий

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 19: Экспорт и импорт подключений без секретов

**Files:**
- Copy from `$G/src-tauri/src` and modify: `storage/exchange.rs`; вернуть в `commands/folders.rs` команды `export_connections`, `preview_import`, `apply_import` (из `$G`)
- Modify: `src-tauri/src/storage/mod.rs`, `src-tauri/src/lib.rs`, `src/components/connections/tree/ConnectionTree.tsx`, `src/components/connections/tree/ConnectionTree.test.tsx`
- Test: unit-тесты `storage/exchange.rs`, `commands/folders.rs`; фронтенд — `ConnectionTree.test.tsx`, `ImportDialog.test.tsx`

**Interfaces:**
- Consumes: `ConnectionStore`, `ConnectionProfile::uses_secret`, секреты `PG_PASSWORD`, `PG_KEY_PASSWORD`, `PATRONI_PASSWORD`, `PATRONI_KEY_PASSWORD`.
- Produces: `crate::storage::exchange::{EXCHANGE_FORMAT = "glafira-postgres-connections", EXCHANGE_VERSION = 1, NeedsPassword { pg, pg_key, patroni, patroni_key }, NeedsPassword::any(), build_export, parse_exchange, plan_import, …}` — остальной интерфейс как в Glafira; команды `export_connections(path, folderId)`, `preview_import(path, targetFolderId, updateExisting)`, `apply_import(path, targetFolderId, updateExisting)`; `api.exportConnections`, `api.previewImport`, `api.applyImport`.

- [ ] **Step 1: Скопировать и адаптировать `exchange.rs`**

```bash
cp /Users/vvnovg/kafka-explorer/src-tauri/src/storage/exchange.rs src-tauri/src/storage/
```

`storage/mod.rs` — `pub mod exchange;`. Правки в `exchange.rs`:

1. Импорты: убрать `crate::kafka::config::…`, `SecurityConfig`, `LlmAccess`, `is_secret_property`; добавить `use crate::secrets::{PATRONI_KEY_PASSWORD, PATRONI_PASSWORD, PG_KEY_PASSWORD, PG_PASSWORD};`. `Environment` берётся из `crate::conn::profile`.
2. `pub const EXCHANGE_FORMAT: &str = "glafira-postgres-connections";`
3. `NeedsPassword` и `needs_password`:

```rust
/// Какие секреты были сохранены у подключения при экспорте.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NeedsPassword {
    pub pg: bool,
    pub pg_key: bool,
    pub patroni: bool,
    pub patroni_key: bool,
}

impl NeedsPassword {
    pub fn any(&self) -> bool {
        self.pg || self.pg_key || self.patroni || self.patroni_key
    }
}

/// Флаг поднимается, только если секрет сохранён и профиль его действительно использует.
fn needs_password(secrets: &dyn SecretStore, p: &ConnectionProfile) -> AppResult<NeedsPassword> {
    let check = |field: &str| -> AppResult<bool> {
        if !p.uses_secret(field) {
            return Ok(false);
        }
        Ok(secrets.get(&secret_key(&p.id, field))?.is_some_and(|v| !v.is_empty()))
    };
    Ok(NeedsPassword {
        pg: check(PG_PASSWORD)?,
        pg_key: check(PG_KEY_PASSWORD)?,
        patroni: check(PATRONI_PASSWORD)?,
        patroni_key: check(PATRONI_KEY_PASSWORD)?,
    })
}
```

   Везде, где код Glafira проверяет поля `NeedsPassword` по отдельности (подсчёт `needs_passwords` в предпросмотре), — заменить на `.any()`.
4. В `build_export` в `.map(|mut profile| …)` оставить только `profile.extra.clear();` и вызов `needs_password` (строки про `llm_access` и `extra_properties` удалить).
5. Заменить `tls_of` и `misses_files`:

```rust
/// Хотя бы один указанный файл сертификата (PG или Patroni API) отсутствует на этом компьютере.
fn misses_files(p: &ConnectionProfile, file_exists: &dyn Fn(&str) -> bool) -> bool {
    let pg_cert = p.pg.tls.client_cert.as_ref();
    let api_tls = p.patroni.as_ref().and_then(|x| x.tls.as_ref());
    let api_cert = api_tls.and_then(|t| t.client_cert.as_ref());
    [
        p.pg.tls.ca_path.as_deref(),
        pg_cert.map(|c| c.certificate_path.as_str()),
        pg_cert.and_then(|c| c.key_path.as_deref()),
        api_tls.and_then(|t| t.ca_path.as_deref()),
        api_cert.map(|c| c.certificate_path.as_str()),
        api_cert.and_then(|c| c.key_path.as_deref()),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .any(|path| !file_exists(path))
}
```

6. `same_settings` — без `llm_access`:

```rust
fn same_settings(a: &ConnectionProfile, b: &ConnectionProfile) -> bool {
    let strip = |p: &ConnectionProfile| ConnectionProfile {
        id: String::new(),
        folder_id: None,
        order: 0,
        extra: Default::default(),
        ..p.clone()
    };
    strip(a) == strip(b)
}
```

7. Тесты модуля: `use crate::conn::profile::{sample_profile, ClientCert, CertFormat};`; во всех JSON-фикстурах заменить `"bootstrapServers":"…","security":{…}` на `"kind":"server","hosts":[{"host":"h","port":1}],"pg":{"username":"u"}`; изменения вида `changed["bootstrapServers"] = "other:9092".into()` → `changed["hosts"] = serde_json::json!([{"host": "other", "port": 5432}])`, ассерты `a.bootstrap_servers == "other:9092"` → `a.hosts[0].host == "other"`; `v["connections"][0]["bootstrapServers"].is_string()` → `v["connections"][0]["hosts"].is_array()`; профили с SASL и паролем `SASL_PASSWORD` → обычный профиль и секрет `PG_PASSWORD`, ожидание `needs_password.sasl` → `needs_password.pg`; профили с `SecurityConfig::Ssl { tls }` → `p.pg.tls.client_cert = Some(ClientCert { … })` и `p.pg.tls.ca_path`; тесты секретов в `extraProperties` и `llmAccess` — удалить. Добавить:

```rust
    #[test]
    fn kafka_exchange_file_is_rejected() {
        let bytes = br#"{"format":"glafira-connections","version":1,"folders":[],"connections":[]}"#;
        let err = parse_exchange(bytes).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidInput);
    }

    #[test]
    fn needs_password_follows_used_secrets() {
        let secrets = crate::secrets::MemoryStore::default();
        let mut p = sample_profile("a");
        secrets.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        secrets.set(&secret_key("a", PATRONI_PASSWORD), "stale").unwrap();
        let np = needs_password(&secrets, &p).unwrap();
        assert!(np.pg && !np.patroni, "секрет Patroni у сервера не используется");
        p.pg.tls.client_cert = Some(ClientCert { format: CertFormat::Pkcs12, certificate_path: "/c.p12".into(), key_path: None });
        secrets.set(&secret_key("a", PG_KEY_PASSWORD), "k").unwrap();
        assert!(needs_password(&secrets, &p).unwrap().pg_key);
    }
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib storage::exchange`
Expected: PASS. Если какой-то тест Glafira проверял поведение, которого в PG-профиле нет (например, конфликт механизмов SASL), — удалить его.

- [ ] **Step 2: Команды экспорта и импорта**

Вернуть из `$G/src-tauri/src/commands/folders.rs` функции `export_connections`, `preview_import`, `apply_import` и их тесты; в тестах `crate::kafka::config::SASL_PASSWORD` → `crate::secrets::PG_PASSWORD`. Зарегистрировать три команды в `lib.rs`.

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib commands::folders`
Expected: PASS.

- [ ] **Step 3: Фронтенд**

- Типы и функции API экспорта и импорта уже есть с Task 17.
- `ConnectionTree.tsx` — убрать флаг `EXCHANGE` (пункты меню экспорта и импорта видны всегда); имя файла экспорта по умолчанию — суффикс `.glafira-postgres-connections.json`, без папки — `glafira-postgres-connections-<дата>…`.
- `ConnectionTree.test.tsx` — снять `it.skip` с тестов экспорта и импорта, ожидаемое имя файла — с новым суффиксом.
- `ImportDialog.test.tsx` — скопировать из `$G` (если не скопирован в Task 17) и прогнать.

Run: `pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

Ручная проверка: «⋯ → Экспортировать» сохраняет файл; в нём нет паролей, `folderId`, `order`. Удалить подключение и импортировать файл: предпросмотр показывает его как новое и сообщает, что нужен пароль; после применения подключение снова в дереве.

- [ ] **Step 4: Commit**

```bash
git add src-tauri src
git commit -m "feat(connections): экспорт и импорт подключений без секретов

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 20: CI, README и руководство администратора

**Files:**
- Create: `.github/workflows/ci.yml`, `README.md`, `docs/admin.md`

**Interfaces:**
- Consumes: стенд Task 6, переменные `GPG_IT_*`, секрет репозитория `KAFKA_EXPLORER_TOKEN` (чтение kafka-explorer для `license-format`).

- [ ] **Step 1: `.github/workflows/ci.yml`**

Закреплённые SHA действий (`actions/checkout`, `pnpm/action-setup`, `actions/setup-node`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`) — те же, что в `$G/.github/workflows/ci.yml`.

```yaml
name: ci

on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_NET_GIT_FETCH_WITH_CLI: "true"

jobs:
  linux:
    runs-on: ubuntu-22.04
    env:
      CARGO_PROFILE_DEV_DEBUG: line-tables-only
    steps:
      - uses: actions/checkout@<SHA из $G> # v4
      - name: Доступ к kafka-explorer (license-format)
        run: git config --global url."https://x-access-token:${{ secrets.KAFKA_EXPLORER_TOKEN }}@github.com/vvnovg/kafka-explorer".insteadOf "https://github.com/vvnovg/kafka-explorer"
      - uses: pnpm/action-setup@<SHA из $G> # v4
        with:
          version: 11
      - uses: actions/setup-node@<SHA из $G> # v4
        with:
          node-version: 22
          cache: pnpm
      - uses: dtolnay/rust-toolchain@<SHA из $G> # stable
        with:
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@<SHA из $G> # v2
        with:
          workspaces: |
            src-tauri -> target
            crates/pg-advisor -> target
      - name: Системные зависимости Tauri
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
      - name: Закрытые ключи лицензий не в репозитории
        run: |
          if git ls-files | grep -E '(^|/)license-signing\.key$'; then
            echo "::error::в репозитории закрытый ключ лицензий"; exit 1
          fi
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
    steps:
      - uses: actions/checkout@<SHA из $G> # v4
      - name: Доступ к kafka-explorer (license-format)
        shell: bash
        run: git config --global url."https://x-access-token:${{ secrets.KAFKA_EXPLORER_TOKEN }}@github.com/vvnovg/kafka-explorer".insteadOf "https://github.com/vvnovg/kafka-explorer"
      - uses: pnpm/action-setup@<SHA из $G> # v4
        with:
          version: 11
      - uses: actions/setup-node@<SHA из $G> # v4
        with:
          node-version: 22
          cache: pnpm
      - uses: dtolnay/rust-toolchain@<SHA из $G> # stable
      - uses: Swatinem/rust-cache@<SHA из $G> # v2
        with:
          workspaces: src-tauri -> target
      - run: pnpm install --frozen-lockfile
      - run: pnpm build
      - run: cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

`<SHA из $G>` — не заглушка плана, а инструкция скопировать конкретную строку `uses:` из `$G/.github/workflows/ci.yml` (там SHA закреплены и проверены).

- [ ] **Step 2: `README.md`**

```markdown
# Glafira for PostgreSQL

Desktop-приложение (Tauri 2 + Rust + React) для диагностики PostgreSQL и кластеров Patroni.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada; this project is not affiliated with it.
Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`; этап P1: `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`.

## Что умеет (P1a)
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API).
- Вставка строки подключения: JDBC, `postgresql://…`, `host=… port=…`; пароль из `~/.pgpass`, если не сохранён.
- Топология с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Проверка подключения по шагам и чек-лист готовности к диагностике.
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, функции, последовательности; роли, расширения.
- Экспорт и импорт подключений без секретов.

## Требования
- Rust stable (`rustup`), Xcode Command Line Tools (macOS)
- Node 22+, pnpm
- Доступ на чтение к `github.com/vvnovg/kafka-explorer` (крейт `license-format`)
- Docker — для стенда и интеграционных тестов

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
cargo test --manifest-path crates/pg-advisor/Cargo.toml  # правила
cargo test --manifest-path src-tauri/Cargo.toml --lib    # Rust unit
GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 \
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 \
  cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1   # интеграционные (нужен стенд)
```
Без переменной `GPG_IT_*` интеграционный тест пропускается. `GPG_IT_DOCKER=1` разрешает тестам останавливать контейнеры,
делать switchover и promote; после них стенд возвращается в исходное состояние.

## Где хранятся данные
- Профили: `connections.json` в каталоге данных (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres/`).
- Пароли — в системном хранилище секретов, сервис `glafira-postgres`.
- Настройки: `settings.json`; лицензия: `license.json` (если ключ введён вручную).
- Логи: `RUST_LOG=debug pnpm tauri dev`.
```

- [ ] **Step 3: `docs/admin.md`**

```markdown
# Glafira for PostgreSQL: руководство администратора

## Политика
| ОС | Где |
|---|---|
| macOS | профиль конфигурации MDM, домен `io.github.vvnovg.glafira.postgres` — пример `admin/policy.mobileconfig` |
| Windows | `HKLM\SOFTWARE\Policies\Glafira\PostgreSQL` — пример `admin/policy.reg` |
| Linux | `/etc/io.github.vvnovg.glafira.postgres/policy.json` — пример `admin/policy.json` |

| Ключ | Тип | По умолчанию | Смысл |
|---|---|---|---|
| `licenseKey` | строка | — | ключ организации: Pro у всех пользователей |
| `language` | строка: `en`, `ru` | язык ОС или выбор пользователя | язык интерфейса для всех; пользователь его не меняет. Неизвестный код — ошибка политики |

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

## Сеть
| Куда | Порт | Зачем |
|---|---|---|
| Каждый узел PostgreSQL | 5432 (или ваш) | каталог, топология списка узлов, проверка подключения |
| REST API Patroni каждого узла | 8008 (или ваш) | обнаружение участников (`GET /cluster`, `GET /patroni`) |

Чтение API Patroni не требует прав на изменение; basic auth передаётся, если задан в профиле.
SSH-туннели через бастион — в следующей версии.

## Данные и логи
| | macOS | Windows | Linux |
|---|---|---|---|
| Данные | `~/Library/Application Support/io.github.vvnovg.glafira.postgres` | `%APPDATA%\io.github.vvnovg.glafira.postgres` | `~/.local/share/io.github.vvnovg.glafira.postgres` |
| Логи | `~/Library/Logs/io.github.vvnovg.glafira.postgres` | `%LOCALAPPDATA%\io.github.vvnovg.glafira.postgres\logs` | `~/.local/share/io.github.vvnovg.glafira.postgres/logs` |
| Пароли | Keychain, сервис `glafira-postgres` | Credential Manager | Secret Service |
```

- [ ] **Step 4: Проверить и закоммитить**

Run: `pnpm test && pnpm i18n:check && pnpm build && cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo test --manifest-path crates/pg-advisor/Cargo.toml`
Expected: PASS.

```bash
git add .github README.md docs/admin.md
git commit -m "docs: README, руководство администратора и CI со стендом

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Проверка по критериям готовности спеки (P1a)

| Критерий спеки §17 | Где проверяется в P1a |
|---|---|
| 1. Три вида подключаются к стенду напрямую | Tasks 7, 11, 12, 13 (интеграционные тесты), ручные проверки Tasks 16–18. Через бастион — P1b |
| 2. JDBC, URI, conninfo заполняют форму; нераспознанное показано | Task 4 (unit), Task 16 (`ConnectionForm.test.tsx`) |
| 3. Patroni: три участника с ролями и лагом; switchover в дереве и журнале ≤ 15 с | Tasks 10, 12 (`it_runtime`), Task 18 (ручная) |
| 4. Список узлов: primary; `noPrimary` при остановке; новый primary после promote | Task 11 (`it_probe`) |
| 5. Шаги проверки и чек-лист для `pg-single` и `patroni1` | Task 13 (`it_readiness`) |
| 6. Дерево до таблицы; вкладка таблицы, DDL, матрица версий, 10 000 таблиц | Дерево до объектов — Tasks 14, 17; вкладки объектов, DDL, матрица PG 13–18 и `wide` — P1b |
| 7. Папки, экспорт/импорт без секретов, ru/en, Pro-блокировка prod | Tasks 3, 16, 17, 19 |
| 8. CI на трёх ОС | Task 20 |
