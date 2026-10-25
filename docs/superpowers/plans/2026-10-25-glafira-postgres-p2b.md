# P2b «Действовать» — безопасные действия дежурного, наблюдение и журнал: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Реализовать часть P2b спецификации `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md` (§1, пункты 5–6; §8 целиком; сценарии действий §12.2; пункт чек-листа `replicationAttr`; разделы руководства о действиях): отмена запроса и завершение сессий поштучно и пакетом, удаление слота, действия Patroni (switchover сейчас и по расписанию, отмена, failover, restart сейчас и по расписанию, отмена, pause / resume) с предпросмотром, защитой read-only и prod, наблюдением после переключения и журналом `actions.jsonl`.

**Architecture:** Каждое действие — две команды: предпросмотр (читает свежие данные, ничего не меняет) и выполнение (принимает разрешённый предпросмотром узел и объект, сначала проверяет `guard` в Rust, затем выполняет и пишет строку журнала). SQL-действия идут по отдельному соединению вне пулов (`actions::conn`, без `default_transaction_read_only`), действия Patroni — тем же HTTP-клиентом подключения (`LiveSource::http_for`), новым методом `PatroniHttp::send_json`. После switchover / failover / restart «сейчас» и pause / resume открывается сессия наблюдения (`SessionKind::Watch`): ускоренный опрос топологии (`Runtimes::boost`) и шаги чистой функции `actions::watch::progress`, события — в `Channel<WatchEvent>`. Фронтенд — общий `ConfirmDialog` (перенос из Glafira Kafka), диалоги действий у сессий, блокировок, слотов и в обзоре кластера, панель наблюдения и вкладка «Журнал действий».

**Tech Stack:** Tauri 2, Rust (edition 2024): tokio, tokio-postgres 0.7, reqwest 0.13, serde/serde_json, `time` 0.3 (разбор RFC 3339); React 19 + TS + TanStack Query + Zustand + i18next; Vitest + Testing Library; стенд Docker Compose (`gp-*`).

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`), команды выполняются из `$P`. Glafira for Kafka (`/Users/vvnovg/kafka-explorer`) — только для чтения: `src-tauri/src/guard.rs`, `src/components/common/ConfirmDialog.tsx` (+ `.test.tsx`), `crates/glafira-mcp/src/audit.rs`.

**Основа:** P2a в `main` (диагностика, сэмплер, `SessionRegistry` с видом `Watch`, `diag::sql::BACKEND_START_US`, `diag::slots`, `diag::patroni::{candidate_facts, dyn_config}`, `pg_advisor::failover::evaluate`).

## Global Constraints

- PostgreSQL 13–18, Patroni 3.0+ (включая 4.x). Платформы: macOS (arm64, x64), Windows x64, Linux x64.
- Защита в Rust до любого обращения к серверу: `guard::ensure_mutation_allowed(profile, confirm)` — read-only → `READ_ONLY`; prod без верного имени подключения → `CONFIRMATION_REQUIRED`; `guard::ensure_name_confirmed(expected, typed)`. Каждая команда выполнения принимает `confirmation { confirm: Option<String>, typedName: Option<String> }`.
- Матрица подтверждений (спец. §8.1): отмена / завершение одной сессии — предпросмотр (на prod ещё имя подключения); пакет — предпросмотр списка (на prod ещё имя подключения и число сессий в `typedName`); удаление слота — ввод имени слота (на prod ещё имя подключения); switchover, отмены расписаний, restart, pause / resume — предпросмотр (на prod ещё имя подключения); failover — ввод имени кандидата (на prod ещё имя подключения).
- Соединение для действия — вне пулов: `ConnectSpec` с `ACTION_SESSION_OPTIONS = "-c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO"` (без `default_transaction_read_only`), `application_name = glafira-pg`, через туннель подключения (`TunnelRegistry::route_for`), БД — `pg.database` (у логического слота — его БД); на одно действие или пакет; отказ во входе, запомненный пулом `Catalog` узла, возвращается без новой попытки входа; недоступный узел — `NODE_UNAVAILABLE`.
- `backend_start` везде — фрагмент `diag::sql::BACKEND_START_US` (микросекунды эпохи, `int8`); сессия адресуется парой `(pid, backendStartUs)`.
- Пакет — не больше 500 сессий (`errors.actions.batchTooLarge`), выполняется по одной на одном соединении, общий предел 60 с. Итог сессии: `terminated` | `signalSent` | `gone` | `noLongerMatches` | `notAllowed` | `failed { error }`; ожидание исчезновения после terminate — до 5 с, опрос раз в 250 мс.
- Patroni: только подключение `patroni`; basic auth профиля (логин `patroni.username` и секрет `patroniPassword`); без логина — `errors.patroni.authRequired`; пароль не уходит на `http://`-адрес при https-профиле (правило P1). Запросы — к API лидера, при отказе соединения — к остальным участникам и адресам профиля; restart и его отмена — только к API самого участника. Ответ 2xx — успех; 401/403 — `AUTH_FAILED` (`errors.patroni.unauthorized` / `credentialsWithheld`, как у чтения); иначе `PATRONI_ERROR` `errors.patroni.actionRejected` с кодом и первыми 500 символами ответа.
- Наблюдение: switchover / failover — срок 120 с; restart — 300 с; pause / resume — 30 с; опрос топологии во время наблюдения — раз в 2 с (`Runtimes::boost`), остановка панели (`stop_session`) возвращает обычный интервал.
- Журнал `actions.jsonl` в каталоге данных приложения, права 0600, ротация в `actions.1.jsonl` при 5 МБ; запись после выполнения, best effort (`journalWritten: false` при сбое); отказы guard не пишутся; текст запроса — первые 500 символов с литералами; без учётных данных; ответ Patroni — 500 символов. `list_actions(connectionId?, limit ≤ 1000)` — оба файла, новые сверху, битые строки пропускаются.
- Все команды Tauri возвращают `Result<T, AppError>`; JSON — camelCase; новые коды `AppError.code` — `READ_ONLY`, `CONFIRMATION_REQUIRED`.
- Пользовательские тексты из Rust — только `ltext!` / `app_err!` с английским шаблоном, равным тексту `src/locales/en.json`; переводы — `src/locales/{en,ru}.json`; ru == en — в `docs/i18n/same-as-en.json`; `hints` — последний ключ верхнего уровня; кириллица в литералах Rust вне `#[cfg(test)]` запрещена (`pnpm i18n:check`). Комментарии — по-русски.
- Ни одна операция не блокирует UI: keychain и файлы — в `run_blocking`; у каждой сетевой операции — таймаут (HTTP Patroni — 5 с).
- Стенд Docker (`gp-*`) запущен: никогда не выполнять `docker compose down`, не удалять контейнеры и тома, не трогать `ke-kafka`; лидер Patroni — любой узел. Сценарии, меняющие стенд (switchover, restart, pause, failover), — только под `GPG_IT_DOCKER=1` (failover — ещё и `GPG_IT_FAILOVER=1`) и с восстановлением при любом выходе (`common::restore_on_drop`, затем `common::wait_stand()`).
- Сообщения коммитов — по-русски (`feat(actions): …`, `test(actions): …`), последняя строка — трейлер `Co-Authored-By` модели, которая пишет коммит.
- Проверки каждой задачи: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`, `cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`, `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`. У задач со стендом — интеграционные тесты:
  `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_LAB=localhost:5470 GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1`
  (сценарии Patroni — ещё с `GPG_IT_DOCKER=1`, failover — с `GPG_IT_FAILOVER=1`).

## Уточнения спецификации (решения плана)

1. **Пакет в интерфейсе — по критерию**, а не произвольный выбор строк: «Завершить idle in transaction старше N мин» (панель набора idle in transaction и кнопка находки `idleInTransaction`). Поштучно — из панели сессии (`selected` с одной сессией) и из корня дерева блокировок (`rootBlocker`). API принимает `selected` до 500 сессий, но множественного выбора строк в таблице сессий в P2b нет: виртуализированная таблица с флажками — отдельная работа, а спецификация способа выбора не задаёт.
2. **Находки ведут к действиям** (спец. §4, `FindingLink::Action`): `idleInTransaction` — «Завершить idle in transaction старше порога предупреждения», `rootBlocker` — «Завершить корневой блокирующий». Кнопки находок открывают те же диалоги, что и кнопки инструментов; на read-only подключении не показываются.
3. **401 от Patroni на действие** — существующие ключи чтения `errors.patroni.unauthorized` / `credentialsWithheld` (код `AUTH_FAILED`), а не новый `authFailed`: одна причина — один текст.
4. **`ConnectSpec` получает поле `options`** (по умолчанию `SESSION_OPTIONS`); `ConnectSpec::for_action()` подставляет `ACTION_SESSION_OPTIONS`. Константа `ACTION_SESSION_OPTIONS` — в `conn/pg.rs` рядом с `SESSION_OPTIONS`.
5. **Журнал — в каталоге данных приложения** (там же, где `connections.json`); `AppState` получает `actions: Arc<ActionJournal>`.
6. **Ускоренный опрос** — `Runtimes::boost(id, interval, until)` и `Runtimes::unboost(id)`: у открытия появляются поле `boost` и `Notify`, будящий фоновый опрос, чтобы ускорение начиналось сразу, а не после текущих 10 с.
7. **Шаг «перезапущен» у restart** определяется по `postmaster_start_time` из `GET <API участника>/patroni`: значение до действия запоминается при выполнении, наблюдение ждёт другого.
8. **Кнопка удаления у активного слота неактивна** (подсказка «Слот активен»); команда всё равно проверяет `NOT active` и отвечает `slotActiveOrGone`.
9. **`PatroniHttp::send_json` — метод с реализацией по умолчанию** (ошибка «не поддерживается»): подделки HTTP в тестах P1/P2a не меняются; `ReqwestHttp` и подделка тестов действий его переопределяют.
10. **Время расписания** — строка RFC 3339 со смещением от фронтенда; Rust разбирает её `time::OffsetDateTime::parse(…, &Rfc3339)` (у зависимости `time` включается feature `parsing`) и сравнивает с часами приложения: прошлое — `errors.actions.pastSchedule`.

## Карта файлов

```
src-tauri/
  Cargo.toml                       time: + feature "parsing"
  src/guard.rs                     ensure_mutation_allowed, ensure_name_confirmed                            (задача 1)
  src/error.rs                     + ErrorCode::ReadOnly, ConfirmationRequired                               (задача 1)
  src/conn/pg.rs                   + ConnectSpec.options, ACTION_SESSION_OPTIONS, for_action                (задача 1)
  src/actions/mod.rs               Confirmation, подмодули                                                    (задача 1)
  src/actions/conn.rs              open — соединение для действия                                            (задача 1)
  src/actions/journal.rs           ActionJournal, ActionRecord, list                                          (задача 2)
  src/actions/sessions.rs          предпросмотр (3) и выполнение (4) отмены / завершения
  src/actions/slots.rs             предпросмотр и удаление слота                                              (задача 5)
  src/actions/patroni.rs           PatroniAction, предусловия, запросы, маршруты (6); предпросмотр, выполнение (7)
  src/actions/watch.rs             WatchEvent, progress, сессия наблюдения                                   (задача 8)
  src/commands/actions.rs          команды P2b (2–8)
  src/topology/patroni.rs          + PatroniHttp::send_json, HttpMethod                                      (задача 6)
  src/topology/runtime.rs          + boost, unboost                                                           (задача 8)
  src/diag/slots.rs                mark_by_config → pub(crate)                                                (задача 5)
  src/readiness/mod.rs             + has_replication                                                          (задача 5)
  src/state.rs, src/lib.rs         actions, регистрация команд
crates/pg-advisor/src/
  findings.rs                      + FindingLink::Action, ActionLink                                         (задача 3)
  rules/sessions.rs, rules/locks.rs  ссылки на действия                                                      (задача 3)
  readiness.rs                     + replicationAttr, has_replication                                        (задача 5)
src-tauri/tests/
  it_actions.rs                    сессии (1, 3, 4), слоты (5)
  it_patroni_actions.rs            switchover, расписание, restart, pause, failover (7, 8)
src/
  api/types.ts, api/client.ts      типы и команды P2b                                                         (задача 9)
  components/common/ConfirmDialog.tsx (+ test)                                                               (задача 9)
  lib/useConnectionProfile.ts                                                                                 (задача 9)
  state/workspace.ts, components/TabContent.tsx, components/connections/tree/DiagnosticsNodes.tsx            (задача 10)
  components/actions/ActionsJournal.tsx                                                                       (задача 10)
  components/actions/SessionActionDialog.tsx; diag/SessionsView, LocksView, FindingsPanel                    (задача 11)
  components/actions/DropSlotDialog.tsx; diag/SlotsView                                                       (задача 12)
  components/actions/PatroniActionDialog.tsx, WatchPanel.tsx; overview/Overview.tsx                          (задача 13)
docs/guide/{en,ru}/diagnostics.md, docs/admin.md, README.md, CHANGELOG.md, docs/i18n/glossary.md, спецификация (задача 14)
```

**Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 (Rust), 9 → 10 → 11 → 12 → 13 (фронтенд; 9 можно начинать после 7: типы зеркалят Rust), 14 (документация, последней).

---

### Task 1: Защита и соединение для действия

**Files:**
- Create: `src-tauri/src/guard.rs`, `src-tauri/src/actions/mod.rs`, `src-tauri/src/actions/conn.rs`, `src-tauri/tests/it_actions.rs`
- Modify: `src-tauri/src/error.rs`, `src-tauri/src/conn/pg.rs`, `src-tauri/src/lib.rs`, все литералы `ConnectSpec { … }` (`src-tauri/src/conn/pg.rs` в тестах, `src-tauri/src/conn/pool.rs` в тестах, `src-tauri/tests/it_pg.rs`, `src-tauri/tests/it_ssh.rs` — найти: `grep -rn "ConnectSpec {" src-tauri`), `src/api/types.ts` (`ErrorCode`), `src/locales/{en,ru}.json`
- Test: модульные тесты `guard.rs`, `conn/pg.rs`, `actions/conn.rs`; `src-tauri/tests/it_actions.rs`

**Interfaces:**
- Produces:
  - `crate::guard::ensure_mutation_allowed(p: &ConnectionProfile, confirm: Option<&str>) -> AppResult<()>`, `crate::guard::ensure_name_confirmed(expected: &str, typed: Option<&str>) -> AppResult<()>`
  - `ErrorCode::ReadOnly` (`READ_ONLY`), `ErrorCode::ConfirmationRequired` (`CONFIRMATION_REQUIRED`)
  - `crate::conn::pg::ACTION_SESSION_OPTIONS: &str`, поле `ConnectSpec.options: &'static str`, `ConnectSpec::for_action(self) -> Self`
  - `crate::actions::Confirmation { confirm: Option<String>, typed_name: Option<String> }` (Deserialize, camelCase)
  - `crate::actions::conn::ActionClient` (поле `client: tokio_postgres::Client`; закрывает соединение при `Drop`), `crate::actions::conn::open(st: &AppState, id: &str, member: &str, database: Option<&str>) -> AppResult<ActionClient>`

- [ ] **Step 1: Падающие тесты guard**

`src-tauri/src/guard.rs` — тесты перенести из Glafira Kafka (`/Users/vvnovg/kafka-explorer/src-tauri/src/guard.rs`, модуль `tests`) без изменений смысла; путь профиля — `crate::conn::profile::{ConnectionProfile, Environment, sample_profile}`, имя подключения в тесте prod — `"billing-prod"`, тексты — как ниже:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{Environment, sample_profile};
    use crate::error::ErrorCode;

    #[test]
    fn read_only_blocks_even_with_confirmation() {
        let mut p = sample_profile("a");
        p.read_only = true;
        let e = ensure_mutation_allowed(&p, Some(&p.name.clone())).unwrap_err();
        assert_eq!(e.code, ErrorCode::ReadOnly);
        assert_eq!(e.key, "errors.guard.readOnly");
        assert_eq!(e.params["name"], p.name);
        assert_eq!(e.message, format!("Connection “{}” is read-only", p.name));
    }

    #[test]
    fn prod_requires_connection_name() {
        let mut p = sample_profile("a");
        p.environment = Environment::Prod;
        p.name = "billing-prod".into();
        for confirm in [None, Some(""), Some("billing-pro"), Some("BILLING-PROD")] {
            let e = ensure_mutation_allowed(&p, confirm).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{confirm:?}");
            assert_eq!(e.key, "errors.guard.prodConfirm");
            assert_eq!(e.message, "To change a prod connection, type its name “billing-prod”");
        }
        assert!(ensure_mutation_allowed(&p, Some("billing-prod")).is_ok());
        assert!(ensure_mutation_allowed(&p, Some(" billing-prod ")).is_ok());
    }

    #[test]
    fn other_environments_need_no_confirmation() {
        for env in [Environment::Local, Environment::Dev, Environment::Test, Environment::Stage] {
            let mut p = sample_profile("a");
            p.environment = env;
            assert!(ensure_mutation_allowed(&p, None).is_ok(), "{env:?}");
        }
    }

    #[test]
    fn object_name_must_match_exactly() {
        assert!(ensure_name_confirmed("it_dead_slot", Some("it_dead_slot")).is_ok());
        assert!(ensure_name_confirmed("it_dead_slot", Some(" it_dead_slot ")).is_ok());
        for typed in [None, Some("it_dead"), Some("IT_DEAD_SLOT")] {
            let e = ensure_name_confirmed("it_dead_slot", typed).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{typed:?}");
            assert_eq!(e.key, "errors.guard.typeToConfirm");
            assert_eq!(e.message, "Type “it_dead_slot” to confirm");
        }
    }
}
```

- [ ] **Step 2: Убедиться, что тесты не собираются**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib guard`
Expected: FAIL — нет `ensure_mutation_allowed`, `ErrorCode::ReadOnly`.

- [ ] **Step 3: Коды ошибок и guard**

`src-tauri/src/error.rs`, в `ErrorCode` после `RecoveryConflict`:

```rust
    /// Действие на read-only подключении (P2b): кнопок нет, команда всё равно отказывает.
    ReadOnly,
    /// Нет или неверно подтверждение: имя подключения на prod, имя объекта, число сессий.
    ConfirmationRequired,
```

Ни один из двух кодов не `retryable` (`retryable_code` не меняется). В `src/api/types.ts` в `ErrorCode` добавить `| "READ_ONLY" | "CONFIRMATION_REQUIRED"`.

`src-tauri/src/guard.rs`:

```rust
//! Защита изменяющих действий (спец. P2 §8.1): проверяется в Rust до любого обращения к серверу, а не только в UI.
//! Перенос `guard.rs` Glafira for Kafka без изменения интерфейса.

use crate::conn::profile::{ConnectionProfile, Environment};
use crate::error::{AppResult, ErrorCode};

/// read-only запрещает любые действия; prod требует ввести имя подключения.
pub fn ensure_mutation_allowed(p: &ConnectionProfile, confirm: Option<&str>) -> AppResult<()> {
    if p.read_only {
        return Err(app_err!(
            ErrorCode::ReadOnly,
            "errors.guard.readOnly",
            "Connection “{{name}}” is read-only",
            name = p.name
        ));
    }
    if p.environment == Environment::Prod && confirm.map(str::trim) != Some(p.name.trim()) {
        return Err(app_err!(
            ErrorCode::ConfirmationRequired,
            "errors.guard.prodConfirm",
            "To change a prod connection, type its name “{{name}}”",
            name = p.name
        ));
    }
    Ok(())
}

/// Удаление слота, failover и пакет на prod требуют ввести имя объекта (или число сессий).
pub fn ensure_name_confirmed(expected: &str, typed: Option<&str>) -> AppResult<()> {
    if typed.map(str::trim) == Some(expected.trim()) {
        Ok(())
    } else {
        Err(app_err!(
            ErrorCode::ConfirmationRequired,
            "errors.guard.typeToConfirm",
            "Type “{{expected}}” to confirm",
            expected = expected
        ))
    }
}
```

`src-tauri/src/lib.rs`: `pub mod guard;` и `pub mod actions;` (рядом с `pub mod diag;`).

`src/locales/en.json`, раздел `errors`:

```json
"guard": {
  "readOnly": "Connection “{{name}}” is read-only",
  "prodConfirm": "To change a prod connection, type its name “{{name}}”",
  "typeToConfirm": "Type “{{expected}}” to confirm"
},
```

`src/locales/ru.json`:

```json
"guard": {
  "readOnly": "Подключение «{{name}}» только для чтения",
  "prodConfirm": "Чтобы изменить prod-подключение, введите его имя «{{name}}»",
  "typeToConfirm": "Введите «{{expected}}» для подтверждения"
},
```

- [ ] **Step 4: Падающие тесты параметров сессии действия**

`src-tauri/src/conn/pg.rs`, модуль `tests`:

```rust
    #[test]
    fn action_spec_is_read_write_with_the_same_limits() {
        let spec = loopback_spec(5432, "localhost", SslMode::Disable, None);
        assert_eq!(spec.pg_config().get_options(), Some(SESSION_OPTIONS));
        let action = spec.for_action();
        let opts = action.pg_config().get_options().map(str::to_string);
        assert_eq!(opts.as_deref(), Some(ACTION_SESSION_OPTIONS));
        assert!(!ACTION_SESSION_OPTIONS.contains("default_transaction_read_only"));
        for o in ["statement_timeout=5000", "lock_timeout=1000", "search_path=pg_catalog", "DateStyle=ISO"] {
            assert!(ACTION_SESSION_OPTIONS.contains(o), "{o}");
        }
    }
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pg`
Expected: FAIL — нет `for_action`, `ACTION_SESSION_OPTIONS`.

- [ ] **Step 5: `ConnectSpec.options` и `for_action`**

`src-tauri/src/conn/pg.rs`:

```rust
/// Сессия действия (спец. P2 §8.2): те же пределы, но без `default_transaction_read_only`.
pub const ACTION_SESSION_OPTIONS: &str =
    "-c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO";
```

В `ConnectSpec` — поле (последним):

```rust
    /// Параметры сессии (`-c …`): `SESSION_OPTIONS` у пулов, `ACTION_SESSION_OPTIONS` у действий.
    pub options: &'static str,
```

`build_via` ставит `options: SESSION_OPTIONS`; `pg_config` — `.options(self.options)` вместо `.options(SESSION_OPTIONS)`;

```rust
    /// То же соединение для действия (P2b): запись разрешена, пределы и `search_path` те же.
    pub fn for_action(mut self) -> Self {
        self.options = ACTION_SESSION_OPTIONS;
        self
    }
```

Во всех литералах `ConnectSpec { … }` (тесты `conn/pg.rs`, `conn/pool.rs`, `tests/it_pg.rs`, `tests/it_ssh.rs`) добавить `options: glafira_pg_lib::conn::pg::SESSION_OPTIONS` (в крейте — `SESSION_OPTIONS`). `Debug` у `ConnectSpec` поле не показывает.

- [ ] **Step 6: `Confirmation` и соединение для действия**

`src-tauri/src/actions/mod.rs`:

```rust
//! Действия дежурного (спец. P2 §8): отмена и завершение сессий, удаление слота, действия Patroni, наблюдение и журнал.

pub mod conn;

use serde::Deserialize;

/// Подтверждение действия (спец. §8.1): имя подключения на prod (`confirm`) и имя объекта или число сессий
/// (`typed_name`). Проверяется `guard` в Rust до обращения к серверу.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub confirm: Option<String>,
    pub typed_name: Option<String>,
}
```

`src-tauri/src/actions/conn.rs`:

```rust
//! Соединение для действия (спец. P2 §8.2): отдельное, вне пулов, на одно действие или пакет.

use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::conn::pool::{PoolKey, PoolPurpose};
use crate::conn::run_blocking;
use crate::error::{AppResult, ErrorCode};
use crate::state::AppState;

/// Соединение действия: закрывается при `Drop` (задача соединения прерывается — без ожидания Terminate).
pub struct ActionClient {
    pub client: Client,
    abort: AbortHandle,
}

impl Drop for ActionClient {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Открывает соединение с участником `member` (БД `database` или БД профиля) через туннель подключения.
/// Пул `Catalog` узла запомнил отказ во входе — возвращается он, без новой попытки входа.
pub async fn open(
    st: &AppState,
    id: &str,
    member: &str,
    database: Option<&str>,
) -> AppResult<ActionClient> {
    let (m, profile) = st.runtimes.target(id, Some(member))?;
    if !m.reachable {
        return Err(app_err!(
            ErrorCode::NodeUnavailable,
            "errors.actions.nodeUnavailable",
            "Node {{member}} is unavailable",
            member = m.name
        ));
    }
    let db = database
        .map(str::to_string)
        .unwrap_or_else(|| profile.pg.database.clone());
    let catalog = PoolKey {
        connection_id: profile.id.clone(),
        member: m.name.clone(),
        database: profile.pg.database.clone(),
        purpose: PoolPurpose::Catalog,
    };
    if let Some(e) = st.pools.get(&catalog).and_then(|p| p.auth_failure()) {
        return Err(e);
    }
    let socks = st.tunnels.route_for(&profile)?;
    let secrets = st.secrets.clone();
    let (host, port) = (m.host.clone(), m.port);
    let spec = run_blocking(move || {
        ConnectSpec::build_via(&profile, &host, port, &db, secrets.as_ref(), socks)
    })
    .await?
    .for_action();
    let (client, abort) = spec.connect_with_handle().await?;
    Ok(ActionClient { client, abort })
}
```

`src-tauri/src/actions/mod.rs` — `pub mod conn;` уже есть. Ключ `errors.actions.nodeUnavailable` — en «Node {{member}} is unavailable», ru «Узел {{member}} недоступен» (раздел `errors.actions` создаётся здесь и пополняется задачами 3–7).

Проверить, что `PoolKey` и поля публичны в крейте (`pub struct PoolKey` с `pub` полями, `PoolRegistry::get` — `pub`); если `PgPool::auth_failure` не `pub`, сделать `pub` (сейчас `pub fn auth_failure`).

- [ ] **Step 7: Интеграционный тест соединения действия**

`src-tauri/tests/it_actions.rs`:

```rust
//! Действия P2b на стенде: соединение, сессии, слоты (спец. P2 §8, §12.2). Без `GPG_IT_*` тест пропускается.

mod common;

use glafira_pg_lib::actions::conn;

#[tokio::test]
async fn action_connection_is_read_write_and_tagged() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let c = conn::open(&st, "it", &member, None).await.unwrap();
    let row = c
        .client
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), \
                    current_setting('statement_timeout'), current_setting('search_path')",
            &[],
        )
        .await
        .unwrap();
    let got: (String, String, String, String) = (row.get(0), row.get(1), row.get(2), row.get(3));
    assert_eq!(got, ("off".into(), "glafira-pg".into(), "5s".into(), "pg_catalog".into()));
}

#[tokio::test]
async fn unknown_member_is_rejected_before_connecting() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let (_dir, st) = common::open_server(&addr).await;
    let Err(e) = conn::open(&st, "it", "no-such-node", None).await else {
        panic!("узла нет — соединения быть не должно")
    };
    assert_eq!(e.key, "errors.topology.unknownMember");
}
```

(Имя подключения в `common::open_server` — `"it"`; `TopologyView.topology` — `Option<Topology>`; если `open_server` использует другой id, взять его оттуда.)

- [ ] **Step 8: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_actions -- --test-threads=1 && pnpm exec tsc --noEmit && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add src-tauri/src/guard.rs src-tauri/src/actions src-tauri/src/error.rs src-tauri/src/conn src-tauri/src/lib.rs src-tauri/tests/it_actions.rs src-tauri/tests/it_pg.rs src-tauri/tests/it_ssh.rs src/api/types.ts src/locales
git commit -m "feat(actions): защита read-only и prod, соединение для действия без read-only"
```

---

### Task 2: Журнал действий `actions.jsonl` и `list_actions`

**Files:**
- Create: `src-tauri/src/actions/journal.rs`, `src-tauri/src/commands/actions.rs`
- Modify: `src-tauri/src/actions/mod.rs`, `src-tauri/src/state.rs`, `src-tauri/src/commands/mod.rs`, `src-tauri/src/lib.rs`
- Test: модульные тесты `actions/journal.rs`

**Interfaces:**
- Consumes: `crate::conn::profile::{ConnectionProfile, Environment}`
- Produces:
  - `ActionJournal::new(dir: &Path) -> Self`, `append(&self, r: &ActionRecord) -> bool` (`false` — не записано), `list(&self, connection_id: Option<&str>, limit: usize) -> Vec<ActionRecord>`, `path(&self) -> PathBuf`
  - `ActionRecord { ts, action, connection: RecordConnection, member, prod_confirmed, outcome: Outcome, code, ms, sessions: Vec<SessionRecord>, slot: Option<SlotRecord>, patroni: Option<PatroniRecord> }` (Serialize + Deserialize, camelCase)
  - `RecordConnection::of(p: &ConnectionProfile) -> RecordConnection`, `Outcome { Ok, Partial, Error }`, `SessionRecord`, `SlotRecord`, `PatroniRecord`
  - `journal::cut(s: &str, chars: usize) -> String`, `journal::QUERY_CHARS = 500`, `journal::RESPONSE_CHARS = 500`, `journal::now_ms() -> i64`
  - `AppState.actions: Arc<ActionJournal>`; команда `list_actions(connection_id: Option<String>, limit: usize) -> AppResult<Vec<ActionRecord>>`

- [ ] **Step 1: Падающие тесты журнала**

`src-tauri/src/actions/journal.rs`, модуль `tests` (логика — копия `crates/glafira-mcp/src/audit.rs` Glafira Kafka):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn record(conn: &str, action: &str) -> ActionRecord {
        ActionRecord {
            ts: 1_761_300_000_000,
            action: action.into(),
            connection: RecordConnection { id: conn.into(), name: format!("{conn}-name"), environment: Environment::Prod },
            member: Some("pg-1".into()),
            prod_confirmed: true,
            outcome: Outcome::Ok,
            code: None,
            ms: 412,
            sessions: vec![SessionRecord {
                pid: 4242,
                backend_start_us: 1_761_299_000_000_000,
                user: Some("app".into()),
                database: Some("orders".into()),
                application: Some("api".into()),
                client: Some("10.0.0.5".into()),
                state: Some("idle in transaction".into()),
                xact_age_s: Some(3600.0),
                query_id: Some("-123".into()),
                query: Some("UPDATE shop.orders SET status = 'x'".into()),
                result: "terminated".into(),
            }],
            slot: None,
            patroni: None,
        }
    }

    #[test]
    fn records_lines_newest_first_and_filters_by_connection() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        assert!(j.append(&record("c1", "terminate")));
        assert!(j.append(&record("c2", "cancel")));
        assert!(j.append(&record("c1", "dropSlot")));
        let all = j.list(None, 10);
        assert_eq!(all.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(), ["dropSlot", "cancel", "terminate"]);
        let c1 = j.list(Some("c1"), 10);
        assert_eq!(c1.len(), 2);
        assert_eq!(j.list(None, 1).len(), 1);
        // JSON — camelCase, поля записи по спецификации §8.8.
        let line = std::fs::read_to_string(j.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(v["connection"]["environment"], "prod");
        assert_eq!(v["prodConfirmed"], true);
        assert_eq!(v["sessions"][0]["backendStartUs"], 1_761_299_000_000_000_i64);
        assert_eq!(v["sessions"][0]["queryId"], "-123");
        assert!(v.get("slot").is_none() && v.get("patroni").is_none(), "пустые части не пишутся");
    }

    #[test]
    fn limit_is_capped_at_1000() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        for _ in 0..1005 {
            j.append(&record("c", "cancel"));
        }
        assert_eq!(j.list(None, 5000).len(), 1000);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        let mode = std::fs::metadata(j.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn rotates_at_limit_and_reads_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        // Добить текущий файл до предела: следующая запись уйдёт в новый, старый станет actions.1.jsonl.
        let mut f = std::fs::OpenOptions::new().append(true).open(j.path()).unwrap();
        f.write_all(&vec![b' '; MAX_BYTES as usize]).unwrap();
        drop(f);
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "terminate"));
        assert!(dir.path().join(ROTATED_NAME).exists());
        let r = j.list(None, 10);
        assert_eq!(r.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(), ["terminate", "cancel"]);
    }

    #[test]
    fn broken_lines_are_skipped_not_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        let mut f = std::fs::OpenOptions::new().append(true).open(j.path()).unwrap();
        f.write_all(b"{\"action\":\"terminate\",\"query\":\"\xd0\n{not json\n").unwrap();
        drop(f);
        j.append(&record("c", "terminate"));
        assert_eq!(j.list(None, 10).len(), 2);
    }

    #[test]
    fn cut_keeps_the_first_chars() {
        assert_eq!(cut("абвгд", 3), "абв");
        assert_eq!(cut("abc", 10), "abc");
    }

    #[test]
    fn unwritable_dir_reports_false() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no").join("such");
        let j = ActionJournal::new(&missing);
        assert!(!j.append(&record("c", "cancel")));
    }
}
```

- [ ] **Step 2: Убедиться, что тесты не собираются**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::journal`
Expected: FAIL — нет `ActionJournal`.

- [ ] **Step 3: Журнал**

`src-tauri/src/actions/journal.rs`:

```rust
//! Журнал действий (спец. P2 §8.8): `actions.jsonl`, строка — выполненное действие, ротация в `actions.1.jsonl`.
//! Логика — копия журнала MCP Glafira Kafka (`crates/glafira-mcp/src/audit.rs`). Запись — best effort: сбой не
//! откатывает действие, команда отвечает `journalWritten: false`. Учётные данные не пишутся.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::conn::profile::{ConnectionProfile, Environment};
use crate::sync::MutexExt;

pub const FILE_NAME: &str = "actions.jsonl";
pub const ROTATED_NAME: &str = "actions.1.jsonl";
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Текст запроса в журнале — первые символы, литералы остаются (решение владельца, спец. §8.8).
pub const QUERY_CHARS: usize = 500;
pub const RESPONSE_CHARS: usize = 500;
pub const MAX_LIST: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    /// Часть пакета не выполнена.
    Partial,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordConnection {
    pub id: String,
    pub name: String,
    pub environment: Environment,
}

impl RecordConnection {
    pub fn of(p: &ConnectionProfile) -> Self {
        Self { id: p.id.clone(), name: p.name.clone(), environment: p.environment }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub pid: i32,
    pub backend_start_us: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub state: Option<String>,
    pub xact_age_s: Option<f64>,
    /// Десятичной строкой, как в отчётах диагностики: 64-битный хеш не помещается в число JS.
    pub query_id: Option<String>,
    pub query: Option<String>,
    /// Итог сессии: `terminated` | `signalSent` | `gone` | `noLongerMatches` | `notAllowed` | `failed`.
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotRecord {
    pub name: String,
    pub slot_type: String,
    pub plugin: Option<String>,
    pub database: Option<String>,
    pub retained_bytes: Option<i64>,
    pub wal_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniRecord {
    pub method: String,
    /// Полный адрес без учётных данных.
    pub path: String,
    pub body: Option<serde_json::Value>,
    pub status: Option<u16>,
    pub response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRecord {
    pub ts: i64,
    /// `cancel` | `terminate` | `dropSlot` | `switchover` | `cancelSwitchover` | `failover` | `restart` |
    /// `cancelRestart` | `pause` | `resume`.
    pub action: String,
    pub connection: RecordConnection,
    pub member: Option<String>,
    pub prod_confirmed: bool,
    pub outcome: Outcome,
    /// `AppError.code` при `outcome: error`.
    pub code: Option<String>,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patroni: Option<PatroniRecord>,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Первые `chars` символов (не байтов: литералы могут быть кириллицей).
pub fn cut(s: &str, chars: usize) -> String {
    s.chars().take(chars).collect()
}

pub struct ActionJournal {
    dir: PathBuf,
    file: Mutex<Option<File>>,
}

impl ActionJournal {
    pub fn new(dir: &Path) -> Self {
        Self { dir: dir.to_path_buf(), file: Mutex::new(None) }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Дописывает запись; `false` — не записано (причина — в журнале приложения).
    pub fn append(&self, r: &ActionRecord) -> bool {
        let mut bytes = match serde_json::to_vec(r) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("actions journal: {e}");
                return false;
            }
        };
        bytes.push(b'\n');
        match self.write(&bytes) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("actions journal not written: {e}");
                false
            }
        }
    }

    fn open(&self) -> io::Result<File> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(self.path())
    }

    fn write(&self, line: &[u8]) -> io::Result<()> {
        let mut guard = self.file.lock_safe();
        if guard.is_none() {
            *guard = Some(self.open()?);
        }
        let len = guard.as_ref().map_or(0, |f| f.metadata().map_or(0, |m| m.len()));
        if len + line.len() as u64 > MAX_BYTES {
            *guard = None;
            std::fs::rename(self.path(), self.dir.join(ROTATED_NAME))?;
            *guard = Some(self.open()?);
        }
        let f = guard.as_mut().expect("файл открыт выше");
        f.write_all(line)?;
        f.flush()
    }

    /// Последние `limit` (не больше 1000) записей подключения `connection_id` (или всех), новые первыми.
    pub fn list(&self, connection_id: Option<&str>, limit: usize) -> Vec<ActionRecord> {
        let mut out: Vec<ActionRecord> = Vec::new();
        for name in [ROTATED_NAME, FILE_NAME] {
            let Ok(f) = File::open(self.dir.join(name)) else { continue };
            // По байтам, не `lines()`: строка с битым UTF-8 пропускается, а не обрывает чтение файла.
            out.extend(
                BufReader::new(f)
                    .split(b'\n')
                    .map_while(Result::ok)
                    .filter_map(|l| serde_json::from_slice::<ActionRecord>(&l).ok())
                    .filter(|r| connection_id.is_none_or(|id| r.connection.id == id)),
            );
        }
        let limit = limit.min(MAX_LIST);
        let start = out.len().saturating_sub(limit);
        out.split_off(start).into_iter().rev().collect()
    }
}
```

`src-tauri/src/actions/mod.rs`: `pub mod journal;`. `lock_safe` — трейт `crate::sync::MutexExt`.

- [ ] **Step 4: `AppState.actions` и команда**

`src-tauri/src/state.rs`: поле

```rust
    /// Журнал выполненных действий (`actions.jsonl` в каталоге данных).
    pub actions: Arc<crate::actions::journal::ActionJournal>,
```

в `new_with_logs`: `actions: Arc::new(crate::actions::journal::ActionJournal::new(data_dir)),`.

`src-tauri/src/commands/actions.rs`:

```rust
//! Команды действий P2b (спец. P2 §9).

use tauri::State;

use crate::actions::journal::ActionRecord;
use crate::conn::run_blocking;
use crate::error::AppResult;
use crate::state::AppState;

/// Записи журнала действий подключения (или всех), новые первыми; `limit` не больше 1000.
#[tauri::command]
pub async fn list_actions(
    state: State<'_, AppState>,
    connection_id: Option<String>,
    limit: usize,
) -> AppResult<Vec<ActionRecord>> {
    let journal = state.actions.clone();
    run_blocking(move || Ok(journal.list(connection_id.as_deref(), limit))).await
}
```

`src-tauri/src/commands/mod.rs`: `pub mod actions;`; `src-tauri/src/lib.rs`, `generate_handler!`: `commands::actions::list_actions,`.

- [ ] **Step 5: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/actions src-tauri/src/commands src-tauri/src/state.rs src-tauri/src/lib.rs
git commit -m "feat(actions): журнал действий actions.jsonl с ротацией и правами 0600, команда list_actions"
```

---
### Task 3: Предпросмотр отмены и завершения сессий; ссылки находок на действия

**Files:**
- Create: `src-tauri/src/actions/sessions.rs`
- Modify: `src-tauri/src/actions/mod.rs`, `src-tauri/src/commands/actions.rs`, `src-tauri/src/lib.rs`, `crates/pg-advisor/src/findings.rs`, `crates/pg-advisor/src/rules/sessions.rs`, `crates/pg-advisor/src/rules/locks.rs`, `src/api/types.ts` (`FindingLink`), `src/components/diag/FindingsPanel.tsx`, `src-tauri/tests/it_actions.rs`
- Test: модульные тесты `actions/sessions.rs`, `pg-advisor` (`rules/sessions.rs`, `rules/locks.rs`), `src/components/diag/FindingsPanel.test.tsx`, `it_actions.rs`

**Interfaces:**
- Consumes: `crate::diag::{on_member, head, sql::BACKEND_START_US}`, `crate::diag::caps::{Caps, V14}`, `Runtimes::diag_target`
- Produces:
  - `pg_advisor::findings::FindingLink::Action { action: SessionActionLink, member: Option<String> }`; `SessionActionLink::TerminateIdleInTransaction { older_than_s: f64 }` | `TerminateRootBlocker { pid: i32, backend_start_us: i64 }` (JSON: `{"kind":"action","action":{"type":"terminateIdleInTransaction","olderThanS":300},"member":"pg-1"}`); `Finding::link_action(self, action, member) -> Self`
  - `actions::sessions::{SignalKind { Cancel, Terminate }, SessionRef { pid, backend_start_us }, SessionTarget { Selected { sessions }, IdleInTransaction { older_than_s }, RootBlocker { pid, backend_start_us } }, SessionActionRequest { member, kind, target }, DenyReason { SuperuserSession, OtherRole, NotClientBackend, OwnConnection }, PreviewSession, PreviewRow { session, allowed, reason }, SessionActionPreview { member, rows } }`
  - `actions::sessions::{PrivFacts, deny_reason(&PrivFacts) -> Option<DenyReason>, preview_sql(&SessionTarget) -> String, preview(st, id, &SessionActionRequest) -> AppResult<SessionActionPreview>}`
  - команда `preview_session_action(id: String, request: SessionActionRequest) -> AppResult<SessionActionPreview>`

- [ ] **Step 1: Падающие тесты ссылок находок (`pg-advisor`)**

`crates/pg-advisor/src/findings.rs`, модуль `tests`:

```rust
    #[test]
    fn action_link_json_is_tagged_and_camel_case() {
        let f = Finding::new("rootBlocker", Severity::Critical).link_action(
            SessionActionLink::TerminateRootBlocker { pid: 42, backend_start_us: 1_761_299_000_000_000 },
            Some("pg-1"),
        );
        let v = serde_json::to_value(&f.links[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "action", "member": "pg-1",
                "action": {"type": "terminateRootBlocker", "pid": 42, "backendStartUs": 1_761_299_000_000_000_i64}})
        );
        let idle = serde_json::to_value(&SessionActionLink::TerminateIdleInTransaction { older_than_s: 300.0 }).unwrap();
        assert_eq!(idle, serde_json::json!({"type": "terminateIdleInTransaction", "olderThanS": 300.0}));
    }
```

В тестах правил: находка `idleInTransaction` (`rules/sessions.rs`) несёт ссылку `Action { action: TerminateIdleInTransaction { older_than_s: th::IDLE_IN_TRANSACTION_S.0 }, member }` вслед за `OpenTool idleTx`; находка `rootBlocker` (`rules/locks.rs`) с известной сессией корня — `Action { action: TerminateRootBlocker { pid, backend_start_us }, member }`; корень без `backendStartUs` ссылки на действие не получает (завершать без защиты от переиспользования PID нельзя). Добавить по проверке в существующие тесты этих правил:

```rust
        // rules/sessions.rs, тест находки idleInTransaction:
        let idle = f.iter().find(|x| x.id == "idleInTransaction").unwrap();
        assert!(idle.links.contains(&FindingLink::Action {
            action: SessionActionLink::TerminateIdleInTransaction { older_than_s: th::IDLE_IN_TRANSACTION_S.0 },
            member: Some("pg-1".into()),
        }));
        // rules/locks.rs, тест находки rootBlocker (сессия корня с backend_start_us):
        let root = f.iter().find(|x| x.id == "rootBlocker").unwrap();
        assert!(root.links.iter().any(|l| matches!(l, FindingLink::Action {
            action: SessionActionLink::TerminateRootBlocker { .. }, .. })));
```

(Имена `f`, `th`, члена `"pg-1"` — как в существующих тестах файла; если член в тесте другой, взять его.)

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`
Expected: FAIL — нет `SessionActionLink`, `link_action`.

- [ ] **Step 2: `FindingLink::Action` и ссылки в правилах**

`crates/pg-advisor/src/findings.rs`:

```rust
/// Куда ведёт кнопка находки. `tool` — id вкладки инструмента во фронтенде (`sessions`, `idleTx`, `locks`…);
/// `Action` — форма действия P2b (на read-only подключении интерфейс её не показывает).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FindingLink {
    OpenTool {
        tool: String,
        member: Option<String>,
    },
    Action {
        action: SessionActionLink,
        member: Option<String>,
    },
}

/// Действие с сессиями, которое предлагает находка (спец. P2 §8.3: цель `idleInTransaction` или `rootBlocker`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionActionLink {
    #[serde(rename_all = "camelCase")]
    TerminateIdleInTransaction { older_than_s: f64 },
    #[serde(rename_all = "camelCase")]
    TerminateRootBlocker { pid: i32, backend_start_us: i64 },
}
```

В `impl Finding` рядом с `link_tool`:

```rust
    pub fn link_action(mut self, action: SessionActionLink, member: Option<&str>) -> Self {
        self.links.push(FindingLink::Action { action, member: member.map(str::to_string) });
        self
    }
```

`crates/pg-advisor/src/rules/sessions.rs`, в `evaluate` у `idleInTransaction`:

```rust
        out.extend(
            session_finding("idleInTransaction", member, r, v, n, th::IDLE_IN_TRANSACTION_S, "idleTx").map(|f| {
                f.link_action(
                    SessionActionLink::TerminateIdleInTransaction { older_than_s: th::IDLE_IN_TRANSACTION_S.0 },
                    member,
                )
            }),
        );
```

`crates/pg-advisor/src/rules/locks.rs`, у `rootBlocker`:

```rust
            if let Some(t) = session_target(&root.session) {
                if let FindingTarget::Session { pid, backend_start_us } = &t {
                    f = f.link_action(
                        SessionActionLink::TerminateRootBlocker { pid: *pid, backend_start_us: *backend_start_us },
                        member,
                    );
                }
                f = f.target(t);
            }
```

`src/api/types.ts`:

```ts
export type SessionActionLink =
  | { type: "terminateIdleInTransaction"; olderThanS: number }
  | { type: "terminateRootBlocker"; pid: number; backendStartUs: number };
export type FindingLink =
  | { kind: "openTool"; tool: DiagTool; member: string | null }
  | { kind: "action"; action: SessionActionLink; member: string | null };
```

`src/components/diag/FindingsPanel.tsx`: до задачи 11 кнопки действий не показываются — список ссылок фильтруется `l.kind === "openTool"` (`const links = f.links.filter((l): l is Extract<FindingLink, { kind: "openTool" }> => l.kind === "openTool" && !(…«в себя»…))`). Тест в `FindingsPanel.test.tsx`: находка с одной ссылкой `action` не рисует ни одной кнопки и не падает.

- [ ] **Step 3: Падающие тесты предпросмотра (`src-tauri`)**

`src-tauri/src/actions/sessions.rs`, модуль `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn facts(client: bool, own: bool, session_super: bool, same_role: bool, me_super: bool, signal: bool) -> PrivFacts {
        PrivFacts { client, own, session_super, same_role, me_super, signal }
    }

    #[test]
    fn privilege_rules_follow_postgres() {
        // Суперпользователь может всё, кроме чужого фонового процесса и своего соединения.
        assert_eq!(deny_reason(&facts(true, false, true, false, true, false)), None);
        // Своя роль — можно и без pg_signal_backend.
        assert_eq!(deny_reason(&facts(true, false, false, true, false, false)), None);
        // pg_signal_backend — только сессии не суперпользователя.
        assert_eq!(deny_reason(&facts(true, false, false, false, false, true)), None);
        assert_eq!(deny_reason(&facts(true, false, true, false, false, true)), Some(DenyReason::SuperuserSession));
        assert_eq!(deny_reason(&facts(true, false, false, false, false, false)), Some(DenyReason::OtherRole));
        assert_eq!(deny_reason(&facts(false, false, false, false, true, true)), Some(DenyReason::NotClientBackend));
        assert_eq!(deny_reason(&facts(true, true, false, true, true, true)), Some(DenyReason::OwnConnection));
    }

    #[test]
    fn preview_sql_filters_by_target() {
        let sel = preview_sql(&SessionTarget::Selected { sessions: vec![] });
        assert!(sel.contains("a.pid = ANY($1)"));
        let idle = preview_sql(&SessionTarget::IdleInTransaction { older_than_s: 300.0 });
        assert!(idle.contains("a.state IN ('idle in transaction', 'idle in transaction (aborted)')"));
        assert!(idle.contains("make_interval(secs => $1)"));
        assert!(idle.contains("a.backend_type = 'client backend'"));
        let root = preview_sql(&SessionTarget::RootBlocker { pid: 1, backend_start_us: 2 });
        assert!(root.contains("a.pid = $1"));
        for s in [&sel, &idle, &root] {
            assert!(s.contains(crate::diag::sql::BACKEND_START_US), "одно выражение backend_start во всех запросах");
            assert!(s.contains("pg_has_role(current_user, 'pg_signal_backend', 'MEMBER')"));
        }
    }

    #[test]
    fn request_json_is_camel_case() {
        let r: SessionActionRequest = serde_json::from_value(serde_json::json!({
            "member": "pg-1", "kind": "terminate",
            "target": {"kind": "rootBlocker", "pid": 42, "backendStartUs": 1_761_299_000_000_000_i64}
        }))
        .unwrap();
        assert_eq!(r.kind, SignalKind::Terminate);
        assert_eq!(r.target, SessionTarget::RootBlocker { pid: 42, backend_start_us: 1_761_299_000_000_000 });
        let idle: SessionTarget =
            serde_json::from_value(serde_json::json!({"kind": "idleInTransaction", "olderThanS": 300})).unwrap();
        assert_eq!(idle, SessionTarget::IdleInTransaction { older_than_s: 300.0 });
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::sessions`
Expected: FAIL — модуля нет.

- [ ] **Step 4: Предпросмотр**

`src-tauri/src/actions/sessions.rs`:

```rust
//! Отмена запроса и завершение сессий (спец. P2 §8.3): предпросмотр (задача 3) и выполнение (задача 4).

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::diag::sql::BACKEND_START_US;
use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SignalKind {
    Cancel,
    Terminate,
}

/// Сессия адресуется парой `(pid, backend_start)`: PID переиспользуется, время запуска — нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRef {
    pub pid: i32,
    pub backend_start_us: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionTarget {
    Selected { sessions: Vec<SessionRef> },
    #[serde(rename_all = "camelCase")]
    IdleInTransaction { older_than_s: f64 },
    #[serde(rename_all = "camelCase")]
    RootBlocker { pid: i32, backend_start_us: i64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionRequest {
    pub member: String,
    pub kind: SignalKind,
    pub target: SessionTarget,
}

/// Почему сессию нельзя отменить или завершить (видно в строке предпросмотра и подсказке кнопки).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DenyReason {
    /// Сессия суперпользователя: нужна роль суперпользователя.
    SuperuserSession,
    /// Чужая роль и нет `pg_signal_backend`.
    OtherRole,
    NotClientBackend,
    /// Соединение самого приложения.
    OwnConnection,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSession {
    pub pid: i32,
    pub backend_start_us: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub state: Option<String>,
    pub backend_type: Option<String>,
    pub xact_age_s: Option<f64>,
    pub state_age_s: Option<f64>,
    /// Первые 500 символов текста запроса.
    pub query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRow {
    pub session: PreviewSession,
    pub allowed: bool,
    pub reason: Option<DenyReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionPreview {
    /// Узел, к которому относится предпросмотр: выполнение принимает именно его.
    pub member: String,
    pub rows: Vec<PreviewRow>,
}

/// Факты прав для строки предпросмотра (из того же запроса).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivFacts {
    pub client: bool,
    pub own: bool,
    pub session_super: bool,
    pub same_role: bool,
    pub me_super: bool,
    pub signal: bool,
}

/// Правило PostgreSQL для `pg_cancel_backend` / `pg_terminate_backend`: суперпользователь — любую клиентскую сессию;
/// член роли сессии — её; член `pg_signal_backend` — любую сессию не суперпользователя.
pub fn deny_reason(f: &PrivFacts) -> Option<DenyReason> {
    if !f.client {
        return Some(DenyReason::NotClientBackend);
    }
    if f.own {
        return Some(DenyReason::OwnConnection);
    }
    if f.me_super || f.same_role {
        return None;
    }
    if f.session_super {
        return Some(DenyReason::SuperuserSession);
    }
    if f.signal { None } else { Some(DenyReason::OtherRole) }
}

/// Условие выбора строк по цели: `$1` — массив pid (`selected`), порог в секундах (`idleInTransaction`) или pid.
fn target_filter(target: &SessionTarget) -> &'static str {
    match target {
        SessionTarget::Selected { .. } => "a.pid = ANY($1)",
        SessionTarget::IdleInTransaction { .. } => {
            "a.backend_type = 'client backend' AND a.state IN ('idle in transaction', 'idle in transaction (aborted)') \
             AND a.state_change <= now() - make_interval(secs => $1)"
        }
        SessionTarget::RootBlocker { .. } => "a.pid = $1",
    }
}

pub fn preview_sql(target: &SessionTarget) -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.datname::text, a.application_name, host(a.client_addr), \
                a.state, a.backend_type, extract(epoch FROM now() - a.xact_start)::float8, \
                extract(epoch FROM now() - a.state_change)::float8, left(a.query, 500), \
                a.backend_type = 'client backend', a.pid = pg_backend_pid(), COALESCE(r.rolsuper, false), \
                COALESCE(pg_has_role(current_user, a.usesysid, 'MEMBER'), false), \
                (SELECT rolsuper FROM pg_roles WHERE rolname = current_user), \
                pg_has_role(current_user, 'pg_signal_backend', 'MEMBER') \
         FROM pg_stat_activity a LEFT JOIN pg_roles r ON r.oid = a.usesysid \
         WHERE {} ORDER BY a.pid",
        target_filter(target)
    )
}

async fn query_preview(c: &Client, target: &SessionTarget) -> AppResult<Vec<PreviewRow>> {
    let sql = preview_sql(target);
    let rows = match target {
        SessionTarget::Selected { sessions } => {
            let pids: Vec<i32> = sessions.iter().map(|s| s.pid).collect();
            c.query(&sql, &[&pids]).await
        }
        SessionTarget::IdleInTransaction { older_than_s } => c.query(&sql, &[older_than_s]).await,
        SessionTarget::RootBlocker { pid, .. } => c.query(&sql, &[pid]).await,
    }
    .map_err(|e| crate::conn::errors::from_pg(&e))?;
    let wanted = |pid: i32, start: i64| match target {
        SessionTarget::Selected { sessions } => {
            sessions.iter().any(|s| s.pid == pid && s.backend_start_us == start)
        }
        SessionTarget::IdleInTransaction { .. } => true,
        SessionTarget::RootBlocker { pid: p, backend_start_us } => *p == pid && *backend_start_us == start,
    };
    Ok(rows
        .iter()
        .filter(|r| wanted(r.get(0), r.get(1)))
        .map(|r| {
            let f = PrivFacts {
                client: r.get(11),
                own: r.get(12),
                session_super: r.get(13),
                same_role: r.get(14),
                me_super: r.get::<_, Option<bool>>(15).unwrap_or(false),
                signal: r.get(16),
            };
            let reason = deny_reason(&f);
            PreviewRow {
                session: PreviewSession {
                    pid: r.get(0),
                    backend_start_us: r.get(1),
                    user: r.get(2),
                    database: r.get(3),
                    application: r.get(4),
                    client: r.get(5),
                    state: r.get(6),
                    backend_type: r.get(7),
                    xact_age_s: r.get(8),
                    state_age_s: r.get(9),
                    query: r.get(10),
                },
                allowed: reason.is_none(),
                reason,
            }
        })
        .collect())
}

/// Заново читает сессии цели на узле `member` (соединение каталога: предпросмотр ничего не меняет).
pub async fn preview(st: &AppState, id: &str, req: &SessionActionRequest) -> AppResult<SessionActionPreview> {
    let target = st.runtimes.diag_target(id, Some(&req.member), false)?;
    let t = req.target.clone();
    let rows = crate::diag::on_member(st, id, &target, move |c| Box::pin(async move { query_preview(c, &t).await }))
        .await?;
    Ok(SessionActionPreview { member: req.member.clone(), rows })
}
```

`src-tauri/src/actions/mod.rs`: `pub mod sessions;`. Если `crate::diag::on_member` не принимает `move`-замыкание с владением (сигнатура `FnOnce(&Client) -> BoxFuture`), клонировать цель внутрь, как показано.

`src-tauri/src/commands/actions.rs`:

```rust
/// Предпросмотр отмены или завершения: свежие строки цели с правами (ничего не меняет).
#[tauri::command]
pub async fn preview_session_action(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::sessions::SessionActionRequest,
) -> AppResult<crate::actions::sessions::SessionActionPreview> {
    crate::actions::sessions::preview(&state, &id, &request).await
}
```

Регистрация — `commands::actions::preview_session_action` в `generate_handler!`.

- [ ] **Step 5: Интеграционный тест предпросмотра**

`src-tauri/tests/it_actions.rs`:

```rust
use glafira_pg_lib::actions::sessions::{self as sa, DenyReason, SessionActionRequest, SessionTarget, SignalKind};

/// Сессия, простаивающая в транзакции: `BEGIN; SELECT 1` и ожидание. Возвращает клиента (держит сессию) и pid.
async fn idle_in_transaction(c: &tokio_postgres::Client) -> i32 {
    c.batch_execute("BEGIN; SELECT 1").await.unwrap();
    c.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0)
}

#[tokio::test]
async fn preview_lists_idle_sessions_with_rights() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let app_pid = idle_in_transaction(&app).await;
    let su_pid = idle_in_transaction(&su).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let p = sa::preview(
        &st,
        "it",
        &SessionActionRequest {
            member: member.clone(),
            kind: SignalKind::Terminate,
            target: SessionTarget::IdleInTransaction { older_than_s: 1.0 },
        },
    )
    .await
    .unwrap();
    assert_eq!(p.member, member);
    let row = |pid| p.rows.iter().find(|r| r.session.pid == pid).unwrap_or_else(|| panic!("нет {pid}: {p:?}"));
    // glafira_diag — член pg_signal_backend: сессию glafira_app можно, суперпользователя — нет.
    assert!(row(app_pid).allowed, "{:?}", row(app_pid));
    assert_eq!(row(su_pid).reason, Some(DenyReason::SuperuserSession));
    assert_eq!(row(app_pid).session.user.as_deref(), Some("glafira_app"));
}
```

- [ ] **Step 6: Проверки**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_actions --test it_diag -- --test-threads=1 && pnpm test && pnpm exec tsc --noEmit`
Expected: PASS (`it_diag` — находки с новыми ссылками не ломают сборщиков).

- [ ] **Step 7: Commit**

```bash
git add crates/pg-advisor/src src-tauri/src/actions src-tauri/src/commands/actions.rs src-tauri/src/lib.rs src-tauri/tests/it_actions.rs src/api/types.ts src/components/diag/FindingsPanel.tsx src/components/diag/FindingsPanel.test.tsx
git commit -m "feat(actions): предпросмотр отмены и завершения сессий с правами; находки ведут к действиям"
```

---

### Task 4: Выполнение отмены и завершения сессий

**Files:**
- Modify: `src-tauri/src/actions/sessions.rs`, `src-tauri/src/commands/actions.rs`, `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`, `src-tauri/tests/it_actions.rs`
- Test: модульные тесты `actions/sessions.rs`, `it_actions.rs`

**Interfaces:**
- Consumes: `guard::{ensure_mutation_allowed, ensure_name_confirmed}`, `actions::conn::open`, `actions::journal::{ActionRecord, SessionRecord, Outcome, RecordConnection, now_ms, cut, QUERY_CHARS}`, `diag::head`, `diag::caps::{Caps, V14}`
- Produces:
  - `SessionActionExecute { member, kind, target, sessions: Vec<SessionRef> }` (Deserialize, camelCase)
  - `SessionOutcome` (`#[serde(tag = "result")]`: `terminated` | `signalSent` | `gone` | `noLongerMatches` | `notAllowed` | `failed { error }`), `SessionResult { pid, backend_start_us, #[serde(flatten)] outcome }`, `SessionActionResult { member, results, outcome: journal::Outcome, journal_written }`
  - `signal_sql(kind, &target, caps) -> String`, `batch_outcome(&[SessionResult]) -> journal::Outcome`, `execute(st, id, &SessionActionExecute, &Confirmation) -> AppResult<SessionActionResult>`
  - константы `MAX_BATCH = 500`, `BATCH_LIMIT = 60 с`, `GONE_WAIT = 5 с`, `GONE_POLL = 250 мс`
  - команда `execute_session_action(id, request: SessionActionExecute, confirmation: Confirmation) -> AppResult<SessionActionResult>`

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/actions/sessions.rs`, модуль `tests`:

```rust
    use crate::diag::caps::Caps;

    #[test]
    fn signal_sql_rechecks_the_criterion_in_the_same_statement() {
        let caps = Caps::new(170000);
        let t = signal_sql(SignalKind::Terminate, &SessionTarget::IdleInTransaction { older_than_s: 300.0 }, caps);
        assert!(t.contains("pg_terminate_backend(a.pid)"));
        assert!(t.contains("make_interval(secs => $3)"));
        assert!(t.contains(&format!("{} = $2", crate::diag::sql::BACKEND_START_US)));
        assert!(t.contains("a.backend_type = 'client backend' AND a.pid <> pg_backend_pid()"));
        let c = signal_sql(SignalKind::Cancel, &SessionTarget::Selected { sessions: vec![] }, caps);
        assert!(c.contains("pg_cancel_backend(a.pid)") && !c.contains("$3"));
        let r = signal_sql(SignalKind::Terminate, &SessionTarget::RootBlocker { pid: 1, backend_start_us: 2 }, caps);
        assert!(r.contains("a.pid = ANY(pg_blocking_pids(w.pid))"));
        // query_id — с 14; на 13 типизированный NULL.
        assert!(t.contains("a.query_id::text"));
        assert!(signal_sql(SignalKind::Terminate, &SessionTarget::Selected { sessions: vec![] }, Caps::new(130000))
            .contains("NULL::text"));
    }

    #[test]
    fn batch_outcome_counts_failures() {
        let r = |outcome| SessionResult { pid: 1, backend_start_us: 1, outcome };
        let err = || crate::error::AppError::localized(
            crate::error::ErrorCode::Internal,
            crate::i18n::LocalizedText { key: "k".into(), params: Default::default(), text: "t".into() },
        );
        use journal::Outcome::*;
        assert_eq!(batch_outcome(&[r(SessionOutcome::Terminated), r(SessionOutcome::Gone)]), Ok);
        assert_eq!(batch_outcome(&[r(SessionOutcome::NoLongerMatches)]), Ok);
        assert_eq!(batch_outcome(&[r(SessionOutcome::Terminated), r(SessionOutcome::NotAllowed)]), Partial);
        assert_eq!(batch_outcome(&[r(SessionOutcome::Failed { error: err() })]), Error);
        assert_eq!(batch_outcome(&[r(SessionOutcome::NotAllowed)]), Error);
    }

    #[test]
    fn result_json_carries_the_result_tag() {
        let v = serde_json::to_value(SessionResult { pid: 7, backend_start_us: 9, outcome: SessionOutcome::SignalSent })
            .unwrap();
        assert_eq!(v, serde_json::json!({"pid": 7, "backendStartUs": 9, "result": "signalSent"}));
    }
```

(`use super::journal` — в начале `sessions.rs` импорт `use crate::actions::journal;`.)

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::sessions`
Expected: FAIL — нет `signal_sql`, `SessionResult`.

- [ ] **Step 2: Выполнение**

`src-tauri/src/actions/sessions.rs` (дополнение):

```rust
use std::time::{Duration, Instant};

use tokio_postgres::error::SqlState;

use crate::actions::Confirmation;
use crate::actions::journal::{self, ActionRecord, RecordConnection, SessionRecord};
use crate::diag::caps::{Caps, V14};
use crate::error::{AppError, ErrorCode};
use crate::guard::{ensure_mutation_allowed, ensure_name_confirmed};

pub const MAX_BATCH: usize = 500;
pub const BATCH_LIMIT: Duration = Duration::from_secs(60);
pub const GONE_WAIT: Duration = Duration::from_secs(5);
pub const GONE_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionExecute {
    pub member: String,
    pub kind: SignalKind,
    pub target: SessionTarget,
    /// Сессии, показанные в предпросмотре и разрешённые пользователем.
    pub sessions: Vec<SessionRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum SessionOutcome {
    /// После terminate сессия с тем же `backend_start` исчезла за 5 с.
    Terminated,
    /// Отмена отправлена, или после terminate сессия ещё жива через 5 с.
    SignalSent,
    /// Сессии с таким `backend_start` уже нет.
    Gone,
    /// Сессия есть, но критерий цели больше не выполняется (проснулась, отпустила блокировки).
    NoLongerMatches,
    /// 42501: прав на сигнал нет.
    NotAllowed,
    Failed { error: AppError },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResult {
    pub pid: i32,
    pub backend_start_us: i64,
    #[serde(flatten)]
    pub outcome: SessionOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionActionResult {
    pub member: String,
    pub results: Vec<SessionResult>,
    pub outcome: journal::Outcome,
    pub journal_written: bool,
}

fn criterion(target: &SessionTarget) -> &'static str {
    match target {
        SessionTarget::Selected { .. } => "TRUE",
        SessionTarget::IdleInTransaction { .. } => {
            "a.state IN ('idle in transaction', 'idle in transaction (aborted)') \
             AND a.state_change <= now() - make_interval(secs => $3)"
        }
        SessionTarget::RootBlocker { .. } => {
            "EXISTS (SELECT 1 FROM pg_stat_activity w WHERE w.wait_event_type = 'Lock' \
             AND a.pid = ANY(pg_blocking_pids(w.pid)))"
        }
    }
}

/// Сигнал сессии одним запросом с повторной проверкой критерия (спец. §8.3): строка есть — сигнал отправлен.
pub fn signal_sql(kind: SignalKind, target: &SessionTarget, caps: Caps) -> String {
    let func = match kind {
        SignalKind::Cancel => "pg_cancel_backend",
        SignalKind::Terminate => "pg_terminate_backend",
    };
    let query_id = caps.col(V14, "a.query_id::text", "text");
    format!(
        "SELECT a.usename::text, a.datname::text, a.application_name, host(a.client_addr), a.state, {query_id}, \
                left(a.query, 500), extract(epoch FROM now() - a.xact_start)::float8, {func}(a.pid) \
         FROM pg_stat_activity a \
         WHERE a.pid = $1 AND {BACKEND_START_US} = $2 AND a.backend_type = 'client backend' \
           AND a.pid <> pg_backend_pid() AND {}",
        criterion(target)
    )
}

fn exists_sql() -> String {
    format!("SELECT 1 FROM pg_stat_activity a WHERE a.pid = $1 AND {BACKEND_START_US} = $2")
}

pub fn batch_outcome(results: &[SessionResult]) -> journal::Outcome {
    let failed = results
        .iter()
        .filter(|r| matches!(r.outcome, SessionOutcome::NotAllowed | SessionOutcome::Failed { .. }))
        .count();
    match failed {
        0 => journal::Outcome::Ok,
        n if n == results.len() => journal::Outcome::Error,
        _ => journal::Outcome::Partial,
    }
}

fn result_name(o: &SessionOutcome) -> &'static str {
    match o {
        SessionOutcome::Terminated => "terminated",
        SessionOutcome::SignalSent => "signalSent",
        SessionOutcome::Gone => "gone",
        SessionOutcome::NoLongerMatches => "noLongerMatches",
        SessionOutcome::NotAllowed => "notAllowed",
        SessionOutcome::Failed { .. } => "failed",
    }
}

async fn still_there(c: &Client, s: SessionRef) -> Result<bool, tokio_postgres::Error> {
    Ok(!c.query(&exists_sql(), &[&s.pid, &s.backend_start_us]).await?.is_empty())
}

/// Одна сессия пакета: сигнал с перепроверкой; итог и строка журнала (если сессия была).
async fn one(
    c: &Client,
    sql: &str,
    kind: SignalKind,
    target: &SessionTarget,
    s: SessionRef,
) -> (SessionOutcome, Option<SessionRecord>) {
    let rows = match target {
        SessionTarget::IdleInTransaction { older_than_s } => {
            c.query(sql, &[&s.pid, &s.backend_start_us, older_than_s]).await
        }
        _ => c.query(sql, &[&s.pid, &s.backend_start_us]).await,
    };
    let row = match rows {
        Ok(rows) => rows.into_iter().next(),
        Err(e) if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) => return (SessionOutcome::NotAllowed, None),
        Err(e) => return (SessionOutcome::Failed { error: crate::conn::errors::from_pg(&e) }, None),
    };
    let Some(row) = row else {
        // Сигнал не отправлен: сессии нет или она больше не подходит под цель.
        return match still_there(c, s).await {
            Ok(true) => (SessionOutcome::NoLongerMatches, None),
            Ok(false) => (SessionOutcome::Gone, None),
            Err(e) => (SessionOutcome::Failed { error: crate::conn::errors::from_pg(&e) }, None),
        };
    };
    let mut record = SessionRecord {
        pid: s.pid,
        backend_start_us: s.backend_start_us,
        user: row.get(0),
        database: row.get(1),
        application: row.get(2),
        client: row.get(3),
        state: row.get(4),
        query_id: row.get(5),
        query: row.get::<_, Option<String>>(6).map(|q| journal::cut(&q, journal::QUERY_CHARS)),
        xact_age_s: row.get(7),
        result: String::new(),
    };
    let outcome = if kind == SignalKind::Terminate {
        let deadline = Instant::now() + GONE_WAIT;
        loop {
            match still_there(c, s).await {
                Ok(false) => break SessionOutcome::Terminated,
                Ok(true) if Instant::now() >= deadline => break SessionOutcome::SignalSent,
                Ok(true) => tokio::time::sleep(GONE_POLL).await,
                Err(_) => break SessionOutcome::SignalSent,
            }
        }
    } else {
        SessionOutcome::SignalSent
    };
    record.result = result_name(&outcome).to_string();
    (outcome, Some(record))
}

/// Выполняет отмену или завершение сессий, разрешённых в предпросмотре (спец. §8.3). Защита — до обращения к серверу.
pub async fn execute(
    st: &AppState,
    id: &str,
    req: &SessionActionExecute,
    confirmation: &Confirmation,
) -> AppResult<SessionActionResult> {
    let profile = st.profile(id)?;
    ensure_mutation_allowed(&profile, confirmation.confirm.as_deref())?;
    let batch = req.sessions.len() > 1 || matches!(req.target, SessionTarget::IdleInTransaction { .. });
    if batch && profile.environment == crate::conn::profile::Environment::Prod {
        ensure_name_confirmed(&req.sessions.len().to_string(), confirmation.typed_name.as_deref())?;
    }
    if req.sessions.len() > MAX_BATCH {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.batchTooLarge",
            "At most {{max}} sessions per action",
            max = MAX_BATCH
        ));
    }
    let started = Instant::now();
    let c = crate::actions::conn::open(st, id, &req.member, None).await?;
    let caps = Caps::new(crate::diag::head(&c.client).await?.version_num);
    let sql = signal_sql(req.kind, &req.target, caps);
    let mut results = Vec::with_capacity(req.sessions.len());
    let mut records = Vec::new();
    for s in &req.sessions {
        let outcome = if started.elapsed() >= BATCH_LIMIT {
            SessionOutcome::Failed {
                error: app_err!(
                    ErrorCode::Timeout,
                    "errors.actions.batchTimeout",
                    "The batch did not finish within {{secs}} s",
                    secs = BATCH_LIMIT.as_secs()
                ),
            }
        } else {
            let (o, rec) = one(&c.client, &sql, req.kind, &req.target, *s).await;
            records.extend(rec);
            o
        };
        results.push(SessionResult { pid: s.pid, backend_start_us: s.backend_start_us, outcome });
    }
    let outcome = batch_outcome(&results);
    let record = ActionRecord {
        ts: journal::now_ms(),
        action: match req.kind {
            SignalKind::Cancel => "cancel",
            SignalKind::Terminate => "terminate",
        }
        .into(),
        connection: RecordConnection::of(&profile),
        member: Some(req.member.clone()),
        prod_confirmed: profile.environment == crate::conn::profile::Environment::Prod,
        outcome,
        code: None,
        ms: started.elapsed().as_millis() as u64,
        sessions: records,
        slot: None,
        patroni: None,
    };
    let journal = st.actions.clone();
    let journal_written = crate::conn::run_blocking(move || Ok(journal.append(&record))).await.unwrap_or(false);
    Ok(SessionActionResult { member: req.member.clone(), results, outcome, journal_written })
}
```

Ошибка открытия соединения или чтения версии — ошибка команды (действие не началось, строки журнала нет). Ключи `src/locales/{en,ru}.json`, раздел `errors.actions`: `batchTooLarge` — en «At most {{max}} sessions per action», ru «Не больше {{max}} сессий за одно действие»; `batchTimeout` — en «The batch did not finish within {{secs}} s», ru «Пакет не уложился в {{secs}} с».

`src-tauri/src/commands/actions.rs`:

```rust
/// Отмена или завершение сессий из предпросмотра; защита read-only и prod — в Rust.
#[tauri::command]
pub async fn execute_session_action(
    state: State<'_, AppState>,
    id: String,
    request: crate::actions::sessions::SessionActionExecute,
    confirmation: crate::actions::Confirmation,
) -> AppResult<crate::actions::sessions::SessionActionResult> {
    crate::actions::sessions::execute(&state, &id, &request, &confirmation).await
}
```

Регистрация в `generate_handler!`.

- [ ] **Step 3: Интеграционные сценарии (`it_actions.rs`)**

```rust
use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::sessions::{SessionActionExecute, SessionOutcome, SessionRef};
use glafira_pg_lib::conn::profile::Environment;

async fn backend_start_us(c: &tokio_postgres::Client, pid: i32) -> i64 {
    c.query_one(
        &format!("SELECT {} FROM pg_stat_activity a WHERE a.pid = $1", glafira_pg_lib::diag::sql::BACKEND_START_US),
        &[&pid],
    )
    .await
    .unwrap()
    .get(0)
}

fn exec(member: &str, kind: SignalKind, target: SessionTarget, sessions: Vec<SessionRef>) -> SessionActionExecute {
    SessionActionExecute { member: member.into(), kind, target, sessions }
}

#[tokio::test]
async fn terminating_the_root_blocker_frees_the_queue_and_writes_the_journal() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let holder = common::app(&addr).await;
    holder.batch_execute("BEGIN; UPDATE shop.orders SET id = id WHERE id = (SELECT min(id) FROM shop.orders)").await.unwrap();
    let holder_pid: i32 = holder.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let victim = common::app(&addr).await;
    let victim_pid: i32 = victim.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let waiting = tokio::spawn(async move {
        victim.execute("UPDATE shop.orders SET id = id WHERE id = (SELECT min(id) FROM shop.orders)", &[]).await
    });
    let probe = common::superuser(&addr, "orders").await;
    common::wait_lock_wait(&probe, victim_pid, "жертва ждёт держателя").await;
    let start = backend_start_us(&probe, holder_pid).await;
    let (dir, st) = common::open_server_in(&addr, "orders").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let r = glafira_pg_lib::actions::sessions::execute(
        &st,
        "it",
        &exec(&member, SignalKind::Terminate, SessionTarget::RootBlocker { pid: holder_pid, backend_start_us: start },
              vec![SessionRef { pid: holder_pid, backend_start_us: start }]),
        &Confirmation::default(),
    )
    .await
    .unwrap();
    assert_eq!(r.results[0].outcome, SessionOutcome::Terminated, "{r:?}");
    assert!(r.journal_written);
    // Очередь освободилась: жертва дождалась блокировки.
    tokio::time::timeout(std::time::Duration::from_secs(10), waiting).await.unwrap().unwrap().unwrap();
    let journal = std::fs::read_to_string(dir.path().join("actions.jsonl")).unwrap();
    assert!(journal.contains("\"action\":\"terminate\"") && journal.contains("\"result\":\"terminated\""), "{journal}");
}

#[tokio::test]
async fn stale_backend_start_is_gone_and_woken_session_no_longer_matches() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let pid = idle_in_transaction(&app).await;
    let probe = common::superuser(&addr, "postgres").await;
    let start = backend_start_us(&probe, pid).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let execute = |sessions, target| {
        let st = &st;
        let member = member.clone();
        async move {
            glafira_pg_lib::actions::sessions::execute(st, "it", &exec(&member, SignalKind::Terminate, target, sessions),
                &Confirmation::default()).await.unwrap()
        }
    };
    // Чужой backend_start — сессии «нет», живая сессия не тронута.
    let r = execute(vec![SessionRef { pid, backend_start_us: start - 1 }],
                    SessionTarget::Selected { sessions: vec![] }).await;
    assert_eq!(r.results[0].outcome, SessionOutcome::Gone);
    // Сессия проснулась (транзакция закончилась) — критерий idle in transaction не выполняется.
    app.batch_execute("COMMIT").await.unwrap();
    let r = execute(vec![SessionRef { pid, backend_start_us: start }],
                    SessionTarget::IdleInTransaction { older_than_s: 1.0 }).await;
    assert_eq!(r.results[0].outcome, SessionOutcome::NoLongerMatches);
    assert!(app.query_one("SELECT 1", &[]).await.is_ok(), "сессия жива");
}

#[tokio::test]
async fn cancel_interrupts_a_running_query() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let pid: i32 = app.query_one("SELECT pg_backend_pid()", &[]).await.unwrap().get(0);
    let sleeping = tokio::spawn(async move { app.execute("SELECT pg_sleep(60)", &[]).await });
    let probe = common::superuser(&addr, "postgres").await;
    common::wait_until("pg_sleep идёт", std::time::Duration::from_secs(10), async || {
        probe.query_opt("SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND wait_event = 'PgSleep'", &[&pid])
            .await.unwrap().is_some()
    }).await;
    let start = backend_start_us(&probe, pid).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let r = glafira_pg_lib::actions::sessions::execute(&st, "it",
        &exec(&member, SignalKind::Cancel, SessionTarget::Selected { sessions: vec![] },
              vec![SessionRef { pid, backend_start_us: start }]),
        &Confirmation::default()).await.unwrap();
    assert_eq!(r.results[0].outcome, SessionOutcome::SignalSent);
    let err = sleeping.await.unwrap().unwrap_err();
    assert_eq!(err.code(), Some(&tokio_postgres::error::SqlState::QUERY_CANCELED), "57014 у клиента");
}

#[tokio::test]
async fn read_only_and_prod_are_guarded_in_rust() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else { return };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let req = exec(&member, SignalKind::Cancel, SessionTarget::Selected { sessions: vec![] },
                   vec![SessionRef { pid: 1, backend_start_us: 1 }]);
    let mut p = st.profile("it").unwrap();
    p.read_only = true;
    st.connections.upsert(p.clone()).unwrap();
    let e = glafira_pg_lib::actions::sessions::execute(&st, "it", &req, &Confirmation::default()).await.unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ReadOnly);
    p.read_only = false;
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let e = glafira_pg_lib::actions::sessions::execute(&st, "it", &req, &Confirmation::default()).await.unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ConfirmationRequired);
    // Отказ guard ничего не пишет в журнал.
    assert!(st.actions.list(Some("it"), 10).is_empty());
}
```

(`st.connections.upsert` меняет сохранённый профиль; `execute` читает профиль `st.profile(id)` — проверить, что `profile()` читает хранилище, а не открытие; если подключение держит копию профиля, `ensure_mutation_allowed` всё равно берёт профиль из хранилища — так задумано: правка формы сразу защищает.)

- [ ] **Step 4: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_actions -- --test-threads=1 && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/actions/sessions.rs src-tauri/src/commands/actions.rs src-tauri/src/lib.rs src-tauri/tests/it_actions.rs src/locales
git commit -m "feat(actions): отмена и завершение сессий с перепроверкой критерия, защитой от переиспользования PID и журналом"
```

---
### Task 5: Удаление слота и пункт чек-листа `replicationAttr`

**Files:**
- Create: `src-tauri/src/actions/slots.rs`
- Modify: `src-tauri/src/actions/mod.rs`, `src-tauri/src/diag/slots.rs` (`mark_by_config` → `pub(crate)`), `src-tauri/src/commands/actions.rs`, `src-tauri/src/lib.rs`, `crates/pg-advisor/src/readiness.rs`, `src-tauri/src/readiness/mod.rs`, `src/locales/{en,ru}.json`, `src-tauri/tests/it_actions.rs`, `src-tauri/tests/it_readiness.rs`
- Test: модульные тесты `actions/slots.rs`, `pg-advisor` `readiness.rs`; `it_actions.rs`, `it_readiness.rs`

**Interfaces:**
- Consumes: `diag::slots::{collect, mark_by_config}`, `diag::patroni::dyn_config`, `pg_advisor::rules::slots::{Slot, PatroniSlot}`, `actions::conn::open`, `guard`, `actions::journal`
- Produces:
  - `actions::slots::{SlotDropRequest { member, slot }, SlotDropPreview { member, slot: Slot }, ActionResult { journal_written }, DROP_SQL, preview(st, id, &SlotDropRequest), drop(st, id, &SlotDropRequest, &Confirmation)}`
  - команды `preview_drop_slot(id, request) -> SlotDropPreview`, `drop_slot(id, request, confirmation) -> ActionResult`
  - `pg_advisor::readiness::ReadinessFacts.has_replication: bool`; пункт `replicationAttr` (`ok` | `warn`, исправление `ALTER ROLE "<user>" REPLICATION;`)

- [ ] **Step 1: Падающие тесты чек-листа**

`crates/pg-advisor/src/readiness.rs`, модуль `tests`:

```rust
    #[test]
    fn replication_attribute_is_a_warning_with_its_fix() {
        let mut f = ReadinessFacts { user: "glafira_diag".into(), server_version_num: 170000, ..Default::default() };
        let item = |f: &ReadinessFacts| evaluate(f).into_iter().find(|i| i.id == "replicationAttr").unwrap();
        let i = item(&f);
        assert_eq!(i.status, Status::Warn);
        assert_eq!(i.fix.as_deref(), Some("ALTER ROLE \"glafira_diag\" REPLICATION;"));
        f.has_replication = true;
        assert_eq!(item(&f).status, Status::Ok);
        f.has_replication = false;
        f.is_superuser = true;
        assert_eq!(item(&f).status, Status::Ok);
    }
```

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml readiness`
Expected: FAIL — нет поля `has_replication`.

- [ ] **Step 2: Пункт `replicationAttr`**

`crates/pg-advisor/src/readiness.rs`: в `ReadinessFacts` после `is_superuser`:

```rust
    /// Атрибут `REPLICATION` (`pg_roles.rolreplication`): удаление слотов (P2b).
    #[serde(default)]
    pub has_replication: bool,
```

В `evaluate` сразу после пункта `signalRole`:

```rust
    // Атрибут даёт больше, чем удаление слотов (логические слоты читают изменения всех таблиц), поэтому пункт —
    // предупреждение, а не «нет обязательного»; docs/admin.md советует отдельную роль для действий.
    out.push(if f.is_superuser || f.has_replication {
        item("replicationAttr", Status::Ok, &[], None)
    } else {
        item("replicationAttr", Status::Warn, &[], Some(format!("ALTER ROLE {user} REPLICATION;")))
    });
```

`src-tauri/src/readiness/mod.rs`: запрос

```rust
            "SELECT current_setting('server_version_num')::int, current_user::text, r.rolsuper, r.rolreplication \
             FROM pg_roles r WHERE r.rolname = current_user",
```

`let (server_version_num, user, is_superuser, has_replication): (i32, String, bool, bool) = (r.get(0), r.get(1), r.get(2), r.get(3));` и `has_replication` в `ReadinessFacts { … }`.

Словари, раздел `readiness`:

```json
"replicationAttr": {
  "title": "Dropping replication slots",
  "ok": "The role has the REPLICATION attribute",
  "warn": "No REPLICATION attribute",
  "affects": "Replication slots cannot be dropped. The attribute also lets the role create logical slots and read the changes of every table through them, and with a permitting replication line in pg_hba.conf, copy the cluster: give it to a separate role for actions, or not at all"
}
```

```json
"replicationAttr": {
  "title": "Удаление слотов репликации",
  "ok": "У роли есть атрибут REPLICATION",
  "warn": "Нет атрибута REPLICATION",
  "affects": "Нельзя будет удалять слоты репликации. Атрибут также позволяет роли создавать логические слоты и читать через них изменения всех таблиц, а при разрешающей строке replication в pg_hba.conf — копировать кластер: выдавайте его отдельной роли для действий или не выдавайте вовсе"
}
```

`src-tauri/tests/it_readiness.rs` — в тест чек-листа добавить: у `glafira_diag` (стенд) `replicationAttr` = `warn`, у подключения под `postgres` (`common::open_server_as(addr, "postgres", common::SUPERUSER, common::SUPERUSER_PASSWORD)` или как уже открыт тест суперпользователя) — `ok`.

- [ ] **Step 3: Падающие тесты удаления слота**

`src-tauri/src/actions/slots.rs`, модуль `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_sql_drops_only_an_inactive_slot() {
        assert!(DROP_SQL.contains("pg_drop_replication_slot(slot_name)"));
        assert!(DROP_SQL.contains("WHERE slot_name = $1 AND NOT active"));
    }

    #[test]
    fn slot_record_carries_what_was_dropped() {
        let s = Slot {
            name: "it_dead_slot".into(),
            slot_type: "physical".into(),
            retained_bytes: Some(4096),
            wal_status: Some("reserved".into()),
            ..Default::default()
        };
        let r = slot_record(&s);
        assert_eq!((r.name.as_str(), r.slot_type.as_str(), r.retained_bytes), ("it_dead_slot", "physical", Some(4096)));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::slots`
Expected: FAIL.

- [ ] **Step 4: Предпросмотр и удаление**

`src-tauri/src/diag/slots.rs`: `fn mark_by_config` → `pub(crate) fn mark_by_config`.

`src-tauri/src/actions/slots.rs`:

```rust
//! Удаление слота репликации (спец. P2 §8.4).

use std::time::Instant;

use pg_advisor::rules::slots::Slot;
use serde::{Deserialize, Serialize};
use tokio_postgres::error::SqlState;

use crate::actions::Confirmation;
use crate::actions::journal::{self, ActionRecord, RecordConnection, SlotRecord};
use crate::conn::profile::{ConnectionKind, Environment};
use crate::diag::caps::Caps;
use crate::error::{AppResult, ErrorCode};
use crate::guard::{ensure_mutation_allowed, ensure_name_confirmed};
use crate::state::AppState;

/// Только неактивный слот: активный за время между предпросмотром и удалением сюда не попадёт.
pub const DROP_SQL: &str =
    "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = $1 AND NOT active";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDropRequest {
    pub member: String,
    pub slot: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDropPreview {
    pub member: String,
    /// Слот с отметкой Patroni (`patroni: member | permanent`): интерфейс предупреждает, что Patroni его пересоздаст.
    pub slot: Slot,
}

/// Итог действия без подробностей: записан ли журнал.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub journal_written: bool,
}

pub fn slot_record(s: &Slot) -> SlotRecord {
    SlotRecord {
        name: s.name.clone(),
        slot_type: s.slot_type.clone(),
        plugin: s.plugin.clone(),
        database: s.database.clone(),
        retained_bytes: s.retained_bytes,
        wal_status: s.wal_status.clone(),
    }
}

/// Свежие данные слота на узле `member` с отметкой Patroni (как во вкладке «Слоты»).
pub async fn preview(st: &AppState, id: &str, req: &SlotDropRequest) -> AppResult<SlotDropPreview> {
    let target = st.runtimes.diag_target(id, Some(&req.member), false)?;
    let mut data = crate::diag::on_member(st, id, &target, |c| {
        Box::pin(async move {
            let h = crate::diag::head(c).await?;
            Ok(crate::diag::slots::collect(c, Caps::new(h.version_num)).await?.0)
        })
    })
    .await?;
    if target.profile.kind == ConnectionKind::Patroni {
        let members: Vec<String> = target.topology.members.iter().map(|m| m.name.clone()).collect();
        let config = crate::diag::patroni::dyn_config(st, &target).await;
        crate::diag::slots::mark_by_config(&mut data, id, &members, config);
    }
    let slot = data.slots.into_iter().find(|s| s.name == req.slot).ok_or_else(|| {
        app_err!(
            ErrorCode::NotFound,
            "errors.actions.slotNotFound",
            "Slot {{slot}} is not on node {{member}}",
            slot = req.slot,
            member = req.member
        )
    })?;
    Ok(SlotDropPreview { member: req.member.clone(), slot })
}

/// Удаляет неактивный слот. Подтверждение — имя слота всегда, имя подключения — на prod.
pub async fn drop(
    st: &AppState,
    id: &str,
    req: &SlotDropRequest,
    confirmation: &Confirmation,
) -> AppResult<ActionResult> {
    let profile = st.profile(id)?;
    ensure_mutation_allowed(&profile, confirmation.confirm.as_deref())?;
    ensure_name_confirmed(&req.slot, confirmation.typed_name.as_deref())?;
    let started = Instant::now();
    let slot = preview(st, id, req).await?.slot;
    // Логический слот удаляется из соединения с его БД.
    let database = if slot.slot_type == "logical" { slot.database.as_deref() } else { None };
    let c = crate::actions::conn::open(st, id, &req.member, database).await?;
    let result = c.client.query(DROP_SQL, &[&req.slot]).await;
    let (outcome, code, ret) = match result {
        Ok(rows) if rows.is_empty() => {
            // Ничего не произошло: журнал не пишется.
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.actions.slotActiveOrGone",
                "The slot became active or is gone"
            ));
        }
        Ok(_) => (journal::Outcome::Ok, None, Ok(())),
        Err(e) if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) => {
            let err = app_err!(
                ErrorCode::NotAuthorized,
                "errors.actions.replicationRequired",
                "Dropping a slot needs the REPLICATION attribute or superuser"
            )
            .with_details(crate::conn::errors::chain(&e));
            (journal::Outcome::Error, Some("NOT_AUTHORIZED".to_string()), Err(err))
        }
        Err(e) => {
            let err = crate::conn::errors::from_pg(&e);
            let code = serde_json::to_value(err.code).ok().and_then(|v| v.as_str().map(str::to_string));
            (journal::Outcome::Error, code, Err(err))
        }
    };
    let record = ActionRecord {
        ts: journal::now_ms(),
        action: "dropSlot".into(),
        connection: RecordConnection::of(&profile),
        member: Some(req.member.clone()),
        prod_confirmed: profile.environment == Environment::Prod,
        outcome,
        code,
        ms: started.elapsed().as_millis() as u64,
        sessions: vec![],
        slot: Some(slot_record(&slot)),
        patroni: None,
    };
    let journal = st.actions.clone();
    let journal_written = crate::conn::run_blocking(move || Ok(journal.append(&record))).await.unwrap_or(false);
    ret.map(|()| ActionResult { journal_written })
}
```

`src-tauri/src/actions/mod.rs`: `pub mod slots;`. Команды в `commands/actions.rs` — `preview_drop_slot(state, id, request: SlotDropRequest) -> AppResult<SlotDropPreview>` и `drop_slot(state, id, request, confirmation: Confirmation) -> AppResult<ActionResult>` (тело — вызов `crate::actions::slots::{preview, drop}`), регистрация в `generate_handler!`. Ключи `errors.actions`: `slotNotFound` (en «Slot {{slot}} is not on node {{member}}», ru «Слота {{slot}} нет на узле {{member}}»), `slotActiveOrGone` (en «The slot became active or is gone», ru «Слот стал активным или исчез»), `replicationRequired` (en «Dropping a slot needs the REPLICATION attribute or superuser», ru «Для удаления слота нужен атрибут REPLICATION или роль суперпользователя»).

- [ ] **Step 5: Интеграционные сценарии слотов (`it_actions.rs`, `GPG_IT_PRIMARY`)**

```rust
use glafira_pg_lib::actions::slots::{self as slots_action, SlotDropRequest};

#[tokio::test]
async fn dead_slot_is_dropped_active_slot_is_not() {
    let Some(addr) = common::env("GPG_IT_PRIMARY") else { return };
    let su = common::superuser(&addr, "postgres").await;
    su.batch_execute(
        "SELECT pg_drop_replication_slot('it_dead_slot') FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'; \
         SELECT pg_create_physical_replication_slot('it_dead_slot', true)",
    )
    .await
    .unwrap();
    let a = addr.clone();
    let _cleanup = common::restore_on_drop(move || {
        common::run_sql_blocking(&a, "postgres",
            "SELECT pg_drop_replication_slot('it_dead_slot') FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'");
    });
    // glafira_diag без REPLICATION: удаление отказано правом, слот на месте.
    let (_d1, diag) = common::open_server(&addr).await;
    let member = diag.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let req = |slot: &str| SlotDropRequest { member: member.clone(), slot: slot.into() };
    let p = slots_action::preview(&diag, "it", &req("it_dead_slot")).await.unwrap();
    assert!(!p.slot.active && p.slot.slot_type == "physical");
    let confirm = |name: &str| glafira_pg_lib::actions::Confirmation { confirm: None, typed_name: Some(name.into()) };
    let e = slots_action::drop(&diag, "it", &req("it_dead_slot"), &confirm("it_dead_slot")).await.unwrap_err();
    assert_eq!(e.key, "errors.actions.replicationRequired");
    // Суперпользователь: неверное имя — подтверждение; верное — слот удалён, строка журнала.
    let (_d2, st) = common::open_server_as(&addr, "postgres", common::SUPERUSER, common::SUPERUSER_PASSWORD).await;
    let e = slots_action::drop(&st, "it", &req("it_dead_slot"), &confirm("it_dead")).await.unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ConfirmationRequired);
    let r = slots_action::drop(&st, "it", &req("it_dead_slot"), &confirm("it_dead_slot")).await.unwrap();
    assert!(r.journal_written);
    assert!(su.query_opt("SELECT 1 FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'", &[]).await.unwrap().is_none());
    // Активный слот реплики не удаляется.
    let e = slots_action::drop(&st, "it", &req("replica1"), &confirm("replica1")).await.unwrap_err();
    assert_eq!(e.key, "errors.actions.slotActiveOrGone");
    assert_eq!(st.actions.list(Some("it"), 10).len(), 1, "отказ «активен» журнал не пишет");
}
```

(Имя активного слота реплики стенда — `replica1`, см. `dev/primary`; если другое — взять из `pg_replication_slots` на `pg-primary`.)

- [ ] **Step 6: Проверки**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_PRIMARY=localhost:5451 GPG_IT_SINGLE=localhost:5460 cargo test --manifest-path src-tauri/Cargo.toml --test it_actions --test it_readiness -- --test-threads=1 && pnpm test && pnpm i18n:check`
Expected: PASS (`pnpm test` — тест чек-листа фронтенда видит ключи нового пункта).

- [ ] **Step 7: Commit**

```bash
git add crates/pg-advisor/src/readiness.rs src-tauri/src src-tauri/tests src/locales
git commit -m "feat(actions): удаление неактивного слота с подтверждением именем; пункт чек-листа replicationAttr"
```

---

### Task 6: Patroni — отправка запросов, предусловия, адреса действий

**Files:**
- Create: `src-tauri/src/actions/patroni.rs`
- Modify: `src-tauri/src/topology/patroni.rs` (`HttpMethod`, `HttpReply`, `PatroniHttp::send_json`, реализация у `ReqwestHttp`), `src-tauri/src/actions/mod.rs`, `src-tauri/Cargo.toml` (`time`: `features = ["formatting", "parsing"]`), `src/locales/{en,ru}.json`
- Test: модульные тесты `actions/patroni.rs` (подделка HTTP с записью запросов), `topology/patroni.rs`

**Interfaces:**
- Consumes: `topology::model::{Topology, Member, Role}`, `conn::profile::ConnectionProfile`
- Produces:
  - `topology::patroni::{HttpMethod { Post, Patch, Delete }, HttpReply { status, body }}`; метод трейта `PatroniHttp::send_json(&self, method, url, body: Option<&Value>) -> BoxFuture<AppResult<HttpReply>>` (по умолчанию — ошибка `errors.patroni.sendUnsupported`)
  - `actions::patroni::{PatroniAction, Route { Leader, Member(String) }, PatroniRequest { method, path, body, route }, action_name(&PatroniAction) -> &'static str, schedule_ms(&str) -> AppResult<i64>, check(&PatroniAction, &Topology, now_ms) -> AppResult<()>, request(&PatroniAction, &Topology) -> PatroniRequest, bases(&Topology, &Route, profile_urls: &[String]) -> Vec<String>, send(http: &dyn PatroniHttp, &PatroniRequest, bases: &[String]) -> AppResult<(String, HttpReply)>}` (`send` возвращает адрес, на который ушёл запрос, и ответ 2xx)

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/actions/patroni.rs`, модуль `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::MutexExt;
    use crate::topology::model::{Member, Role, ScheduledSwitchover, Topology};
    use std::sync::Mutex;

    fn member(name: &str, role: Role, state: &str) -> Member {
        let mut m = Member::new(name, format!("{name}-host"), 5432);
        m.role = role;
        m.state = Some(state.into());
        m.reachable = true;
        m.api_url = Some(format!("http://{name}:8008"));
        m
    }

    fn cluster() -> Topology {
        let mut t = Topology::empty_for_tests();
        t.members = vec![
            member("pg-1", Role::Primary, "running"),
            member("pg-2", Role::Replica, "streaming"),
            member("pg-3", Role::Replica, "streaming"),
        ];
        t
    }

    const NOW: i64 = 1_761_300_000_000; // 2025-10-24T10:00:00Z

    #[test]
    fn switchover_needs_a_running_leader_and_a_real_candidate() {
        let t = cluster();
        let ok = PatroniAction::Switchover { candidate: Some("pg-2".into()), scheduled_at: None };
        assert!(check(&ok, &t, NOW).is_ok());
        let to_leader = PatroniAction::Switchover { candidate: Some("pg-1".into()), scheduled_at: None };
        assert_eq!(check(&to_leader, &t, NOW).unwrap_err().key, "errors.actions.badCandidate");
        let mut down = cluster();
        down.members[0].state = Some("stopped".into());
        assert_eq!(check(&ok, &down, NOW).unwrap_err().key, "errors.actions.noLeader");
        let past = PatroniAction::Switchover { candidate: None, scheduled_at: Some("2025-10-24T09:00:00Z".into()) };
        assert_eq!(check(&past, &t, NOW).unwrap_err().key, "errors.actions.pastSchedule");
        let bad = PatroniAction::Switchover { candidate: None, scheduled_at: Some("tomorrow".into()) };
        assert_eq!(check(&bad, &t, NOW).unwrap_err().key, "errors.actions.badSchedule");
        let later = PatroniAction::Switchover { candidate: None, scheduled_at: Some("2025-10-24T13:00:00+03:00".into()) };
        assert_eq!(check(&later, &t, NOW).unwrap_err().key, "errors.actions.pastSchedule", "смещение учитывается");
        let future = PatroniAction::Switchover { candidate: None, scheduled_at: Some("2025-10-24T14:00:00+03:00".into()) };
        assert!(check(&future, &t, NOW).is_ok());
    }

    #[test]
    fn failover_only_without_a_healthy_leader() {
        let a = PatroniAction::Failover { candidate: "pg-2".into() };
        assert_eq!(check(&a, &cluster(), NOW).unwrap_err().key, "errors.actions.leaderHealthy");
        let mut t = cluster();
        t.members[0].reachable = false;
        assert!(check(&a, &t, NOW).is_ok());
        let unknown = PatroniAction::Failover { candidate: "pg-9".into() };
        assert_eq!(check(&unknown, &t, NOW).unwrap_err().key, "errors.actions.badCandidate");
    }

    #[test]
    fn cancels_and_pause_need_something_to_change() {
        let t = cluster();
        assert_eq!(check(&PatroniAction::CancelSwitchover, &t, NOW).unwrap_err().key, "errors.actions.nothingScheduled");
        let mut s = cluster();
        s.scheduled_switchover = Some(ScheduledSwitchover { at: "2025-10-24T11:00:00Z".into(), from: None, to: None });
        assert!(check(&PatroniAction::CancelSwitchover, &s, NOW).is_ok());
        let cancel = PatroniAction::CancelRestart { member: "pg-2".into() };
        assert_eq!(check(&cancel, &t, NOW).unwrap_err().key, "errors.actions.nothingScheduled");
        assert_eq!(check(&PatroniAction::Pause { paused: false }, &t, NOW).unwrap_err().key, "errors.actions.pauseUnchanged");
        assert!(check(&PatroniAction::Pause { paused: true }, &t, NOW).is_ok());
        let mut down = cluster();
        down.members[1].reachable = false;
        let restart = PatroniAction::Restart { member: "pg-2".into(), only_pending: false, scheduled_at: None };
        assert_eq!(check(&restart, &down, NOW).unwrap_err().key, "errors.actions.nodeUnavailable");
    }

    #[test]
    fn requests_follow_the_patroni_api() {
        let t = cluster();
        let r = request(&PatroniAction::Switchover { candidate: Some("pg-2".into()), scheduled_at: None }, &t);
        assert_eq!((r.method, r.path, r.route.clone()), (HttpMethod::Post, "/switchover", Route::Leader));
        assert_eq!(r.body, Some(serde_json::json!({"leader": "pg-1", "candidate": "pg-2"})));
        let s = request(&PatroniAction::Switchover { candidate: None, scheduled_at: Some("2025-10-24T11:00:00Z".into()) }, &t);
        assert_eq!(s.body, Some(serde_json::json!({"leader": "pg-1", "scheduled_at": "2025-10-24T11:00:00Z"})));
        assert_eq!(request(&PatroniAction::CancelSwitchover, &t).method, HttpMethod::Delete);
        let f = request(&PatroniAction::Failover { candidate: "pg-2".into() }, &t);
        assert_eq!((f.path, f.body), ("/failover", Some(serde_json::json!({"candidate": "pg-2"}))));
        let rs = request(&PatroniAction::Restart { member: "pg-2".into(), only_pending: true, scheduled_at: None }, &t);
        assert_eq!((rs.path, rs.route.clone()), ("/restart", Route::Member("pg-2".into())));
        assert_eq!(rs.body, Some(serde_json::json!({"restart_pending": true})));
        let p = request(&PatroniAction::Pause { paused: true }, &t);
        assert_eq!((p.method, p.path, p.body), (HttpMethod::Patch, "/config", Some(serde_json::json!({"pause": true}))));
        let resume = request(&PatroniAction::Pause { paused: false }, &t);
        assert_eq!(resume.body, Some(serde_json::json!({"pause": null})), "null удаляет ключ из конфигурации");
        assert_eq!(action_name(&PatroniAction::Pause { paused: false }), "resume");
    }

    #[test]
    fn leader_first_then_other_members_then_profile() {
        let t = cluster();
        let profile = vec!["http://lb:8008".to_string(), "http://pg-1:8008".to_string()];
        assert_eq!(
            bases(&t, &Route::Leader, &profile),
            ["http://pg-1:8008", "http://pg-2:8008", "http://pg-3:8008", "http://lb:8008"]
        );
        assert_eq!(bases(&t, &Route::Member("pg-3".into()), &profile), ["http://pg-3:8008"]);
    }

    /// Подделка: первый адрес не отвечает, второй отвечает заданным кодом; запросы записываются.
    struct Fake {
        dead: Vec<String>,
        status: u16,
        calls: Mutex<Vec<(HttpMethod, String, Option<serde_json::Value>)>>,
    }

    impl PatroniHttp for Fake {
        fn get_json<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
        fn get_status<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, AppResult<u16>> {
            Box::pin(async { Ok(200) })
        }
        fn send_json<'a>(
            &'a self,
            method: HttpMethod,
            url: &'a str,
            body: Option<&'a serde_json::Value>,
        ) -> BoxFuture<'a, AppResult<HttpReply>> {
            self.calls.lock_safe().push((method, url.to_string(), body.cloned()));
            let dead = self.dead.iter().any(|d| url.starts_with(d.as_str()));
            let status = self.status;
            Box::pin(async move {
                if dead {
                    Err(app_err!(ErrorCode::PatroniError, "errors.patroni.unreachable", "Patroni API at {{url}} is unreachable", url = url))
                } else {
                    Ok(HttpReply { status, body: "Successfully switched over".into() })
                }
            })
        }
    }

    #[tokio::test]
    async fn send_skips_unreachable_addresses_but_not_answers() {
        let req = request(&PatroniAction::Switchover { candidate: None, scheduled_at: None }, &cluster());
        let bases = vec!["http://pg-1:8008".to_string(), "http://pg-2:8008".to_string()];
        let ok = Fake { dead: vec!["http://pg-1".into()], status: 200, calls: Mutex::new(vec![]) };
        let (url, reply) = send(&ok, &req, &bases).await.unwrap();
        assert_eq!((url.as_str(), reply.status), ("http://pg-2:8008/switchover", 200));
        // Ответ Patroni — не повод идти к следующему адресу: отказ возвращается сразу.
        let rejected = Fake { dead: vec![], status: 412, calls: Mutex::new(vec![]) };
        let e = send(&rejected, &req, &bases).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::PatroniError, "errors.patroni.actionRejected"));
        assert_eq!(e.params["status"], "412");
        assert_eq!(rejected.calls.lock_safe().len(), 1);
        let denied = Fake { dead: vec![], status: 401, calls: Mutex::new(vec![]) };
        assert_eq!(send(&denied, &req, &bases).await.unwrap_err().code, ErrorCode::AuthFailed);
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::patroni`
Expected: FAIL.

- [ ] **Step 2: `send_json` у клиента Patroni**

`src-tauri/src/topology/patroni.rs`:

```rust
/// Метод изменяющего запроса к REST API Patroni (P2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Post,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Post => "POST",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
        }
    }
}

/// Ответ на изменяющий запрос: любой HTTP-код (смысл решает вызывающий) и тело как текст.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: String,
}
```

(`Serialize` — `use serde::Serialize`, если в модуле ещё нет.) В трейт `PatroniHttp`:

```rust
    /// Изменяющий запрос (действия P2b) с basic auth профиля. По умолчанию не поддерживается: подделки чтения
    /// в тестах P1/P2a его не реализуют.
    fn send_json<'a>(
        &'a self,
        _method: HttpMethod,
        url: &'a str,
        _body: Option<&'a serde_json::Value>,
    ) -> BoxFuture<'a, AppResult<HttpReply>> {
        Box::pin(async move {
            Err(app_err!(
                ErrorCode::Internal,
                "errors.patroni.sendUnsupported",
                "This Patroni client cannot send changes to {{url}}",
                url = url
            ))
        })
    }
```

В `impl PatroniHttp for ReqwestHttp`:

```rust
    fn send_json<'a>(
        &'a self,
        method: HttpMethod,
        url: &'a str,
        body: Option<&'a serde_json::Value>,
    ) -> BoxFuture<'a, AppResult<HttpReply>> {
        Box::pin(async move {
            let m = match method {
                HttpMethod::Post => reqwest::Method::POST,
                HttpMethod::Patch => reqwest::Method::PATCH,
                HttpMethod::Delete => reqwest::Method::DELETE,
            };
            let mut req = self.client.request(m, url);
            if let Some(b) = body {
                req = req.json(b);
            }
            let withheld = self.withholds_credentials(url);
            if !withheld && let Some((u, p)) = &self.auth {
                req = req.basic_auth(u, Some(p));
            }
            let resp = req.send().await.map_err(|e| self.request_error(url, &e))?;
            let status = resp.status().as_u16();
            let text = resp.text().await.map_err(|e| self.request_error(url, &e))?;
            if withheld && matches!(status, 401 | 403) {
                return Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.credentialsWithheld",
                    "The Patroni API at {{url}} asks for credentials over plain HTTP; they are not sent because the connection uses https: set an https API address for the member in “Node address overrides”",
                    url = url
                ));
            }
            Ok(HttpReply { status, body: text })
        })
    }
```

`src-tauri/Cargo.toml`: `time = { version = "0.3", features = ["formatting", "parsing"] }`.

- [ ] **Step 3: Действия Patroni — модель, предусловия, запросы, отправка**

`src-tauri/src/actions/patroni.rs`:

```rust
//! Действия Patroni (спец. P2 §8.5): модель действия, предусловия на топологии, запрос и адреса, отправка.
//! Предпросмотр и выполнение с журналом и наблюдением — задачи 7 и 8.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::BoxFuture;
use crate::topology::model::Topology;
use crate::topology::patroni::{HttpMethod, HttpReply, PatroniHttp};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PatroniAction {
    /// `scheduled_at` — RFC 3339 со смещением.
    #[serde(rename_all = "camelCase")]
    Switchover { candidate: Option<String>, scheduled_at: Option<String> },
    CancelSwitchover,
    Failover { candidate: String },
    #[serde(rename_all = "camelCase")]
    Restart { member: String, only_pending: bool, scheduled_at: Option<String> },
    CancelRestart { member: String },
    Pause { paused: bool },
}

/// Кому адресован запрос: API лидера (с обходом остальных при отказе соединения) или API участника.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Leader,
    Member(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatroniRequest {
    pub method: HttpMethod,
    pub path: &'static str,
    pub body: Option<Value>,
    pub route: Route,
}

/// Имя действия в журнале.
pub fn action_name(a: &PatroniAction) -> &'static str {
    match a {
        PatroniAction::Switchover { .. } => "switchover",
        PatroniAction::CancelSwitchover => "cancelSwitchover",
        PatroniAction::Failover { .. } => "failover",
        PatroniAction::Restart { .. } => "restart",
        PatroniAction::CancelRestart { .. } => "cancelRestart",
        PatroniAction::Pause { paused: true } => "pause",
        PatroniAction::Pause { paused: false } => "resume",
    }
}

fn invalid(key: &str, text: &str) -> AppError {
    AppError::localized(
        ErrorCode::InvalidInput,
        crate::i18n::LocalizedText { key: key.into(), params: Default::default(), text: text.into() },
    )
}

/// Время расписания (RFC 3339 со смещением) в мс эпохи.
pub fn schedule_ms(s: &str) -> AppResult<i64> {
    let t = OffsetDateTime::parse(s, &Rfc3339).map_err(|_| {
        app_err!(ErrorCode::InvalidInput, "errors.actions.badSchedule", "The schedule time {{value}} is not valid", value = s)
    })?;
    Ok((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

fn check_schedule(at: &Option<String>, now_ms: i64) -> AppResult<()> {
    match at {
        Some(s) if schedule_ms(s)? <= now_ms => Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.pastSchedule",
            "The schedule time is in the past"
        )),
        _ => Ok(()),
    }
}

/// Лидер, принимающий запись: доступен и в состоянии `running`.
fn running_leader(t: &Topology) -> Option<&crate::topology::model::Member> {
    t.leader().filter(|m| m.reachable && m.state.as_deref() == Some("running"))
}

fn bad_candidate(name: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.actions.badCandidate",
        "{{member}} cannot become the leader: it is not a replica of this cluster",
        member = name
    )
}

/// Предусловия действия на текущей топологии (спец. §8.5); `now_ms` — часы приложения.
pub fn check(a: &PatroniAction, t: &Topology, now_ms: i64) -> AppResult<()> {
    let is_member = |n: &str| t.member(n).is_some();
    match a {
        PatroniAction::Switchover { candidate, scheduled_at } => {
            let leader = running_leader(t).ok_or_else(|| {
                app_err!(ErrorCode::InvalidInput, "errors.actions.noLeader", "There is no running leader to switch over from")
            })?;
            if let Some(c) = candidate
                && (!is_member(c) || c == &leader.name)
            {
                return Err(bad_candidate(c));
            }
            check_schedule(scheduled_at, now_ms)
        }
        PatroniAction::CancelSwitchover => match t.scheduled_switchover {
            Some(_) => Ok(()),
            None => Err(invalid("errors.actions.nothingScheduled", "Nothing is scheduled")),
        },
        PatroniAction::Failover { candidate } => {
            if running_leader(t).is_some() {
                return Err(app_err!(
                    ErrorCode::InvalidInput,
                    "errors.actions.leaderHealthy",
                    "The leader is healthy: use switchover"
                ));
            }
            if !is_member(candidate) || t.leader().is_some_and(|l| &l.name == candidate) {
                return Err(bad_candidate(candidate));
            }
            Ok(())
        }
        PatroniAction::Restart { member, scheduled_at, .. } => {
            if !t.member(member).is_some_and(|m| m.reachable) {
                return Err(app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.actions.nodeUnavailable",
                    "Node {{member}} is unavailable",
                    member = member
                ));
            }
            check_schedule(scheduled_at, now_ms)
        }
        PatroniAction::CancelRestart { member } => match t.member(member).and_then(|m| m.scheduled_restart.as_ref()) {
            Some(_) => Ok(()),
            None => Err(invalid("errors.actions.nothingScheduled", "Nothing is scheduled")),
        },
        PatroniAction::Pause { paused } => {
            if t.paused.unwrap_or(false) == *paused {
                Err(invalid("errors.actions.pauseUnchanged", "The cluster is already in this state"))
            } else {
                Ok(())
            }
        }
    }
}

/// Запрос Patroni для действия (тела — как у `patronictl`).
pub fn request(a: &PatroniAction, t: &Topology) -> PatroniRequest {
    let leader = t.leader().map(|m| m.name.clone());
    match a {
        PatroniAction::Switchover { candidate, scheduled_at } => {
            let mut body = serde_json::Map::new();
            if let Some(l) = leader {
                body.insert("leader".into(), json!(l));
            }
            if let Some(c) = candidate {
                body.insert("candidate".into(), json!(c));
            }
            if let Some(s) = scheduled_at {
                body.insert("scheduled_at".into(), json!(s));
            }
            PatroniRequest { method: HttpMethod::Post, path: "/switchover", body: Some(Value::Object(body)), route: Route::Leader }
        }
        PatroniAction::CancelSwitchover => {
            PatroniRequest { method: HttpMethod::Delete, path: "/switchover", body: None, route: Route::Leader }
        }
        PatroniAction::Failover { candidate } => PatroniRequest {
            method: HttpMethod::Post,
            path: "/failover",
            body: Some(json!({ "candidate": candidate })),
            route: Route::Leader,
        },
        PatroniAction::Restart { member, only_pending, scheduled_at } => {
            let mut body = serde_json::Map::new();
            if *only_pending {
                body.insert("restart_pending".into(), json!(true));
            }
            if let Some(s) = scheduled_at {
                body.insert("schedule".into(), json!(s));
            }
            PatroniRequest {
                method: HttpMethod::Post,
                path: "/restart",
                body: Some(Value::Object(body)),
                route: Route::Member(member.clone()),
            }
        }
        PatroniAction::CancelRestart { member } => PatroniRequest {
            method: HttpMethod::Delete,
            path: "/restart",
            body: None,
            route: Route::Member(member.clone()),
        },
        PatroniAction::Pause { paused } => PatroniRequest {
            method: HttpMethod::Patch,
            path: "/config",
            // `null` удаляет ключ `pause` из динамической конфигурации — так снимается пауза.
            body: Some(if *paused { json!({ "pause": true }) } else { json!({ "pause": null }) }),
            route: Route::Leader,
        },
    }
}

/// Базовые адреса API по порядку: лидер, остальные участники, адреса профиля (без повторов); у участника — только он.
pub fn bases(t: &Topology, route: &Route, profile_urls: &[String]) -> Vec<String> {
    let trim = |u: &str| u.trim_end_matches('/').to_string();
    match route {
        Route::Member(name) => t.member(name).and_then(|m| m.api_url.as_deref()).map(trim).into_iter().collect(),
        Route::Leader => {
            let mut out: Vec<String> = Vec::new();
            let leader = t.leader();
            let ordered = leader.into_iter().chain(t.members.iter().filter(|m| leader.is_none_or(|l| l.name != m.name)));
            for u in ordered.filter_map(|m| m.api_url.as_deref()).map(trim).chain(profile_urls.iter().map(|u| trim(u))) {
                if !out.contains(&u) {
                    out.push(u);
                }
            }
            out
        }
    }
}

/// Отправляет запрос по адресам `bases`: следующий адрес — только при отказе соединения (Patroni не ответил);
/// ответ Patroni (любой код) — окончательный. 2xx — `Ok((адрес, ответ))`; 401/403 — `AUTH_FAILED`;
/// иначе — `errors.patroni.actionRejected` с кодом и первыми 500 символами ответа в `details`.
pub async fn send(
    http: &dyn PatroniHttp,
    req: &PatroniRequest,
    bases: &[String],
) -> AppResult<(String, HttpReply)> {
    let mut last: Option<AppError> = None;
    for base in bases {
        let url = format!("{base}{}", req.path);
        match http.send_json(req.method, &url, req.body.as_ref()).await {
            Ok(r) if (200..300).contains(&r.status) => return Ok((url, r)),
            Ok(r) if matches!(r.status, 401 | 403) => {
                return Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.unauthorized",
                    "The Patroni API at {{url}} rejected the credentials",
                    url = url
                ));
            }
            Ok(r) => {
                return Err(app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.actionRejected",
                    "Patroni rejected the action ({{status}})",
                    status = r.status
                )
                .with_details(crate::actions::journal::cut(&r.body, crate::actions::journal::RESPONSE_CHARS)));
            }
            Err(e) if e.code == ErrorCode::AuthFailed => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")
    }))
}
```

(`BoxFuture` — `crate::topology::BoxFuture`, как в `topology/patroni.rs`; у `reqwest` включён feature `json`. `AppError::localized` и `LocalizedText` — проверить путь `crate::i18n::LocalizedText`; вместо `invalid(...)` можно `app_err!` без параметров — главное, чтобы английский шаблон совпал со словарём.)

`src-tauri/src/actions/mod.rs`: `pub mod patroni;`.

Словари, раздел `errors.actions` (en / ru):

| ключ | en | ru |
|---|---|---|
| `noLeader` | There is no running leader to switch over from | Нет работающего лидера, с которого можно переключиться |
| `leaderHealthy` | The leader is healthy: use switchover | Лидер здоров — используйте switchover |
| `badCandidate` | {{member}} cannot become the leader: it is not a replica of this cluster | {{member}} не может стать лидером: это не реплика этого кластера |
| `badSchedule` | The schedule time {{value}} is not valid | Время расписания {{value}} задано неверно |
| `pastSchedule` | The schedule time is in the past | Время расписания уже прошло |
| `nothingScheduled` | Nothing is scheduled | Ничего не запланировано |
| `pauseUnchanged` | The cluster is already in this state | Кластер уже в этом состоянии |

`errors.patroni`: `actionRejected` — en «Patroni rejected the action ({{status}})», ru «Patroni отклонил действие ({{status}})»; `sendUnsupported` — en «This Patroni client cannot send changes to {{url}}», ru «Этот клиент Patroni не отправляет изменения на {{url}}»; `authRequired` — en «Set the Patroni API login in the connection to run actions», ru «Чтобы выполнять действия, укажите в подключении логин API Patroni» (используется в задаче 7).

- [ ] **Step 4: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/topology/patroni.rs src-tauri/src/actions src/locales
git commit -m "feat(actions): действия Patroni — запросы, предусловия, адреса с обходом при отказе соединения"
```

---
### Task 7: Patroni — предпросмотр и выполнение действия с журналом

**Files:**
- Modify: `src-tauri/src/actions/patroni.rs`, `src-tauri/src/commands/actions.rs`, `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`
- Create: `src-tauri/tests/it_patroni_actions.rs`
- Test: модульные тесты `actions/patroni.rs` (`patroni_record`), `it_patroni_actions.rs`

**Interfaces:**
- Consumes: задача 6 (`PatroniAction`, `check`, `request`, `bases`, `send`); `diag::patroni::{candidate_facts, dyn_config}`, `pg_advisor::failover::{evaluate, Candidate, DynConfig}`, `LiveSource::http_for`, `guard`, `actions::journal`, `conn::profile::strip_url_credentials`
- Produces:
  - `MemberPreview { name, role, state, pending_restart, scheduled_restart }`, `PatroniActionPreview { leader, paused, candidates: Vec<Candidate>, leader_client_sessions: Option<i64>, member: Option<MemberPreview>, scheduled_switchover, can_send }`
  - `preview(st, id, &PatroniAction) -> AppResult<PatroniActionPreview>`; команда `preview_patroni_action(id, action) -> PatroniActionPreview`
  - `Performed { response: String, journal_written: bool, before: Topology, restart_start: Option<String> }`, `perform(st, id, &PatroniAction, &Confirmation) -> AppResult<Performed>` (задача 8 оборачивает её наблюдением и командой `execute_patroni_action`)
  - `patroni_record(method, url, body, status, response) -> PatroniRecord` (адрес без учётных данных, ответ — 500 символов)

- [ ] **Step 1: Падающий модульный тест записи журнала**

`src-tauri/src/actions/patroni.rs`, модуль `tests`:

```rust
    #[test]
    fn journal_record_has_no_credentials_and_a_short_response() {
        let r = patroni_record(
            HttpMethod::Post,
            "http://patroni:secret@pg-1:8008/switchover",
            Some(&serde_json::json!({"leader": "pg-1"})),
            Some(200),
            Some(&"x".repeat(2000)),
        );
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "http://pg-1:8008/switchover");
        assert_eq!(r.response.as_deref().map(|s| s.chars().count()), Some(500));
        assert_eq!(r.status, Some(200));
    }
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::patroni`
Expected: FAIL — нет `patroni_record`.

- [ ] **Step 2: Предпросмотр, выполнение, журнал**

`src-tauri/src/actions/patroni.rs` (дополнение):

```rust
use std::time::Instant;

use pg_advisor::failover::{Candidate, DynConfig};

use crate::actions::Confirmation;
use crate::actions::journal::{self, ActionRecord, PatroniRecord, RecordConnection};
use crate::conn::profile::{ConnectionKind, Environment, strip_url_credentials};
use crate::guard::{ensure_mutation_allowed, ensure_name_confirmed};
use crate::state::AppState;
use crate::topology::model::{Role, ScheduledRestart, ScheduledSwitchover};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberPreview {
    pub name: String,
    pub role: Role,
    pub state: Option<String>,
    pub pending_restart: bool,
    pub scheduled_restart: Option<ScheduledRestart>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniActionPreview {
    pub leader: Option<String>,
    pub paused: bool,
    /// Switchover и failover: готовность реплик (как во вкладке «Готовность к failover»).
    pub candidates: Vec<Candidate>,
    /// Switchover и failover: клиентские сессии лидера — оборвутся при переключении. `None` — не удалось прочитать.
    pub leader_client_sessions: Option<i64>,
    /// Restart и его отмена.
    pub member: Option<MemberPreview>,
    pub scheduled_switchover: Option<ScheduledSwitchover>,
    /// В профиле есть логин API: без него выполнение откажет (`errors.patroni.authRequired`).
    pub can_send: bool,
}

/// Итог отправки: ответ Patroni, запись журнала, топология до действия и `postmaster_start_time` участника до
/// restart «сейчас» (для наблюдения).
#[derive(Debug, Clone)]
pub struct Performed {
    pub response: String,
    pub journal_written: bool,
    pub before: Topology,
    pub restart_start: Option<String>,
}

pub fn patroni_record(
    method: HttpMethod,
    url: &str,
    body: Option<&Value>,
    status: Option<u16>,
    response: Option<&str>,
) -> PatroniRecord {
    PatroniRecord {
        method: method.as_str().into(),
        path: strip_url_credentials(url),
        body: body.cloned(),
        status,
        response: response.map(|r| journal::cut(r, journal::RESPONSE_CHARS)),
    }
}

fn patroni_only(kind: ConnectionKind) -> AppResult<()> {
    if kind == ConnectionKind::Patroni {
        Ok(())
    } else {
        Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.patroniOnly",
            "Patroni actions need a Patroni cluster connection"
        ))
    }
}

/// Клиентские сессии лидера (кроме своей): сколько оборвётся при переключении.
async fn leader_sessions(st: &AppState, id: &str, leader: &str) -> Option<i64> {
    let c = st.client(id, Some(leader), None).await.ok()?;
    c.query_one(
        "SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'client backend' AND pid <> pg_backend_pid()",
        &[],
    )
    .await
    .ok()
    .map(|r| r.get(0))
}

pub async fn preview(st: &AppState, id: &str, action: &PatroniAction) -> AppResult<PatroniActionPreview> {
    let target = st.runtimes.diag_target(id, None, true)?;
    patroni_only(target.profile.kind)?;
    let t = &target.topology;
    let leader = t.leader().map(|m| m.name.clone());
    let paused = t.paused.unwrap_or(false);
    let (candidates, leader_client_sessions) = match action {
        PatroniAction::Switchover { .. } | PatroniAction::Failover { .. } => {
            // Конфигурация не прочиталась — пределы Patroni по умолчанию (как во вкладке готовности).
            let config = crate::diag::patroni::dyn_config(st, &target).await.unwrap_or_else(|_| DynConfig::default());
            let (data, _) = pg_advisor::failover::evaluate(&crate::diag::patroni::candidate_facts(t), &config, paused);
            let sessions = match &leader {
                Some(l) => leader_sessions(st, id, l).await,
                None => None,
            };
            (data.candidates, sessions)
        }
        _ => (vec![], None),
    };
    let member = match action {
        PatroniAction::Restart { member, .. } | PatroniAction::CancelRestart { member } => {
            t.member(member).map(|m| MemberPreview {
                name: m.name.clone(),
                role: m.role,
                state: m.state.clone(),
                pending_restart: m.pending_restart,
                scheduled_restart: m.scheduled_restart.clone(),
            })
        }
        _ => None,
    };
    Ok(PatroniActionPreview {
        leader,
        paused,
        candidates,
        leader_client_sessions,
        member,
        scheduled_switchover: t.scheduled_switchover.clone(),
        can_send: target.profile.patroni.as_ref().is_some_and(|p| p.username.is_some()),
    })
}

/// `postmaster_start_time` участника из `GET <API участника>/patroni` (шаг «перезапущен» наблюдения restart).
pub async fn postmaster_start(http: &dyn PatroniHttp, t: &Topology, member: &str) -> Option<String> {
    let base = t.member(member)?.api_url.as_deref()?.trim_end_matches('/').to_string();
    let v = http.get_json(&format!("{base}/patroni")).await.ok()?;
    v.get("postmaster_start_time").and_then(Value::as_str).map(str::to_string)
}

/// Проверяет защиту и предусловия, отправляет запрос Patroni и пишет журнал (ошибку Patroni — тоже).
pub async fn perform(
    st: &AppState,
    id: &str,
    action: &PatroniAction,
    confirmation: &Confirmation,
) -> AppResult<Performed> {
    let profile = st.profile(id)?;
    ensure_mutation_allowed(&profile, confirmation.confirm.as_deref())?;
    if let PatroniAction::Failover { candidate } = action {
        ensure_name_confirmed(candidate, confirmation.typed_name.as_deref())?;
    }
    patroni_only(profile.kind)?;
    let settings = profile.patroni.clone().ok_or_else(|| {
        app_err!(ErrorCode::PatroniError, "errors.patroni.noUrls", "No Patroni API addresses")
    })?;
    if settings.username.is_none() {
        return Err(app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.authRequired",
            "Set the Patroni API login in the connection to run actions"
        ));
    }
    let before = st.runtimes.diag_target(id, None, true)?.topology;
    check(action, &before, journal::now_ms())?;
    let req = request(action, &before);
    let bases = bases(&before, &req.route, &settings.urls);
    let http = st.live.http_for(&profile).await?;
    let restart_start = match action {
        PatroniAction::Restart { member, scheduled_at: None, .. } => postmaster_start(http.as_ref(), &before, member).await,
        _ => None,
    };
    let started = Instant::now();
    let sent = send(http.as_ref(), &req, &bases).await;
    let member = match &req.route {
        Route::Member(m) => Some(m.clone()),
        Route::Leader => before.leader().map(|m| m.name.clone()),
    };
    let first_url = bases.first().map(|b| format!("{b}{}", req.path)).unwrap_or_default();
    let (outcome, code, record) = match &sent {
        Ok((url, reply)) => (
            journal::Outcome::Ok,
            None,
            patroni_record(req.method, url, req.body.as_ref(), Some(reply.status), Some(&reply.body)),
        ),
        Err(e) => {
            let status = e.params.get("status").and_then(|s| s.parse().ok());
            let code = serde_json::to_value(e.code).ok().and_then(|v| v.as_str().map(str::to_string));
            (
                journal::Outcome::Error,
                code,
                patroni_record(req.method, &first_url, req.body.as_ref(), status, e.details.as_deref()),
            )
        }
    };
    let entry = ActionRecord {
        ts: journal::now_ms(),
        action: action_name(action).into(),
        connection: RecordConnection::of(&profile),
        member,
        prod_confirmed: profile.environment == Environment::Prod,
        outcome,
        code,
        ms: started.elapsed().as_millis() as u64,
        sessions: vec![],
        slot: None,
        patroni: Some(record),
    };
    let journal = st.actions.clone();
    let journal_written = crate::conn::run_blocking(move || Ok(journal.append(&entry))).await.unwrap_or(false);
    let (_, reply) = sent?;
    Ok(Performed { response: reply.body, journal_written, before, restart_start })
}
```

Команда (`src-tauri/src/commands/actions.rs`):

```rust
/// Предпросмотр действия Patroni: кандидаты с готовностью, сессии лидера, участник restart.
#[tauri::command]
pub async fn preview_patroni_action(
    state: State<'_, AppState>,
    id: String,
    action: crate::actions::patroni::PatroniAction,
) -> AppResult<crate::actions::patroni::PatroniActionPreview> {
    crate::actions::patroni::preview(&state, &id, &action).await
}
```

Регистрация в `generate_handler!`. Ключ `errors.actions.patroniOnly` — en «Patroni actions need a Patroni cluster connection», ru «Действия Patroni доступны только у подключения к кластеру Patroni».

- [ ] **Step 3: Интеграционные сценарии без переключения лидера (`it_patroni_actions.rs`)**

```rust
//! Действия Patroni на стенде (спец. P2 §8.5–8.6, §12.2). Меняют кластер — только под `GPG_IT_DOCKER=1`;
//! восстановление — при любом выходе (`common::restore_on_drop`), затем `common::wait_stand()`.

mod common;

use std::time::Duration;

use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::patroni::{self as pa, PatroniAction};

fn docker_enabled() -> bool {
    common::env("GPG_IT_DOCKER").as_deref() == Some("1")
}

/// Ждёт, пока топология подключения `it` (её опрос идёт сам) не станет такой, как требует `cond`.
async fn topology_until(
    st: &glafira_pg_lib::state::AppState,
    what: &str,
    limit: Duration,
    cond: impl Fn(&glafira_pg_lib::topology::model::Topology) -> bool,
) {
    common::wait_until(what, limit, async || st.runtimes.view("it").topology.as_ref().is_some_and(&cond)).await;
}

#[tokio::test]
async fn scheduled_switchover_is_shown_and_cancelled() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let _restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["flush", "demo", "switchover", "--force"]);
        common::wait_stand();
    });
    let in_an_hour = time::OffsetDateTime::now_utc() + time::Duration::hours(1);
    let at = in_an_hour.format(&time::format_description::well_known::Rfc3339).unwrap();
    let action = PatroniAction::Switchover { candidate: None, scheduled_at: Some(at) };
    let p = pa::preview(&st, "it", &action).await.unwrap();
    assert!(p.can_send && p.leader.is_some());
    let done = pa::perform(&st, "it", &action, &Confirmation::default()).await.unwrap();
    assert!(done.journal_written);
    topology_until(&st, "запланированный switchover виден", Duration::from_secs(30), |t| t.scheduled_switchover.is_some()).await;
    pa::perform(&st, "it", &PatroniAction::CancelSwitchover, &Confirmation::default()).await.unwrap();
    topology_until(&st, "расписание снято", Duration::from_secs(30), |t| t.scheduled_switchover.is_none()).await;
    let actions: Vec<String> = st.actions.list(Some("it"), 10).into_iter().map(|r| r.action).collect();
    assert_eq!(actions, ["cancelSwitchover", "switchover"]);
}

#[tokio::test]
async fn read_only_and_missing_login_are_refused() {
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let mut p = st.profile("it").unwrap();
    p.read_only = true;
    st.connections.upsert(p.clone()).unwrap();
    let e = pa::perform(&st, "it", &PatroniAction::Pause { paused: true }, &Confirmation::default()).await.unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ReadOnly);
    p.read_only = false;
    p.patroni.as_mut().unwrap().username = None;
    st.connections.upsert(p).unwrap();
    let e = pa::perform(&st, "it", &PatroniAction::Pause { paused: true }, &Confirmation::default()).await.unwrap_err();
    assert_eq!(e.key, "errors.patroni.authRequired");
    let e = pa::perform(&st, "it", &PatroniAction::Failover { candidate: "patroni2".into() }, &Confirmation::default())
        .await
        .unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ConfirmationRequired, "имя кандидата обязательно");
    assert!(st.actions.list(Some("it"), 10).is_empty());
}
```

(Pause / resume — в задаче 8, вместе с наблюдением. Scope кластера стенда — `demo`.)

- [ ] **Step 4: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_DOCKER=1 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --test it_patroni_actions -- --test-threads=1 && pnpm i18n:check`
Expected: PASS; после теста `scripts/wait-stand.sh` проходит (лидер и две потоковые реплики).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/actions/patroni.rs src-tauri/src/commands/actions.rs src-tauri/src/lib.rs src-tauri/tests/it_patroni_actions.rs src/locales
git commit -m "feat(actions): предпросмотр и выполнение действий Patroni с журналом; расписание switchover и отмена на стенде"
```

---

### Task 8: Ускоренный опрос и наблюдение после действия

**Files:**
- Create: `src-tauri/src/actions/watch.rs`
- Modify: `src-tauri/src/topology/runtime.rs` (`boost`, `unboost`, ожидание опроса), `src-tauri/src/actions/mod.rs`, `src-tauri/src/actions/patroni.rs` (`execute`), `src-tauri/src/commands/actions.rs`, `src-tauri/src/lib.rs`, `src-tauri/tests/it_patroni_actions.rs`
- Test: модульные тесты `actions/watch.rs`, `topology/runtime.rs`; `it_patroni_actions.rs`

**Interfaces:**
- Consumes: `SessionRegistry::{insert, forget}`, `SessionKind::Watch`, `Runtimes::view`, `actions::patroni::{perform, postmaster_start, Performed}`, `LiveSource::http_for`
- Produces:
  - `Runtimes::boost(&self, id: &str, interval: Duration, until: Instant)`, `Runtimes::unboost(&self, id: &str)`
  - `actions::watch::{WatchStep, WatchStatus, WatchEvent { step, status, at_ms, detail }, WatchPlan { Switch { old_leader, candidate, timeline }, Restart { member, start_before, only_pending }, Pause { paused } }, WatchSink, BOOST_INTERVAL = 2 с, progress(&WatchPlan, &Topology, start_now: Option<&str>) -> WatchStep, start(st, id, plan, sink) -> String}`; `WatchPlan::{for_action(&PatroniAction, &Topology, restart_start) -> Option<WatchPlan>, steps(&self) -> Vec<WatchStep>, deadline(&self) -> Duration}`
  - `actions::patroni::{PatroniActionResult { response, watch: Option<String>, journal_written }, execute(st, id, &PatroniAction, &Confirmation, sink: Arc<dyn WatchSink>) -> AppResult<PatroniActionResult>}`
  - команда `execute_patroni_action(id, action, confirmation, channel: Channel<WatchEvent>) -> PatroniActionResult`

- [ ] **Step 1: Падающие тесты `progress`**

`src-tauri/src/actions/watch.rs`, модуль `tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::{Member, Role, Topology};

    fn m(name: &str, role: Role, state: &str, tl: i64) -> Member {
        let mut m = Member::new(name, name, 5432);
        m.role = role;
        m.state = Some(state.into());
        m.timeline = Some(tl);
        m.reachable = true;
        m
    }

    fn topo(members: Vec<Member>) -> Topology {
        let mut t = Topology::empty_for_tests();
        t.members = members;
        t
    }

    #[test]
    fn switchover_steps_follow_the_cluster() {
        let plan = WatchPlan::Switch { old_leader: "pg-1".into(), candidate: Some("pg-2".into()), timeline: Some(2) };
        let before = topo(vec![m("pg-1", Role::Primary, "running", 2), m("pg-2", Role::Replica, "streaming", 2)]);
        assert_eq!(progress(&plan, &before, None), WatchStep::Accepted);
        let stepped = topo(vec![m("pg-1", Role::Replica, "stopped", 2), m("pg-2", Role::Replica, "streaming", 2)]);
        assert_eq!(progress(&plan, &stepped, None), WatchStep::LeaderStepped);
        let promoted = topo(vec![m("pg-1", Role::Replica, "stopped", 2), m("pg-2", Role::Primary, "running", 3)]);
        assert_eq!(progress(&plan, &promoted, None), WatchStep::TimelineIncreased);
        let done = topo(vec![m("pg-1", Role::Replica, "streaming", 3), m("pg-2", Role::Primary, "running", 3)]);
        assert_eq!(progress(&plan, &done, None), WatchStep::Done);
        // Лидером стал не кандидат — дальше «сложил полномочия» не идём.
        let other = topo(vec![m("pg-1", Role::Replica, "streaming", 3), m("pg-3", Role::Primary, "running", 3),
                               m("pg-2", Role::Replica, "streaming", 3)]);
        assert_eq!(progress(&plan, &other, None), WatchStep::LeaderStepped);
    }

    #[test]
    fn restart_needs_a_new_postmaster_and_cleared_pending() {
        let plan = WatchPlan::Restart { member: "pg-2".into(), start_before: Some("t0".into()), only_pending: true };
        let mut r = m("pg-2", Role::Replica, "streaming", 2);
        r.pending_restart = true;
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2), r.clone()]);
        assert_eq!(progress(&plan, &t, Some("t0")), WatchStep::Accepted);
        assert_eq!(progress(&plan, &t, Some("t1")), WatchStep::MemberRunning);
        r.pending_restart = false;
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2), r]);
        assert_eq!(progress(&plan, &t, Some("t1")), WatchStep::Done);
        let plain = WatchPlan::Restart { member: "pg-2".into(), start_before: Some("t0".into()), only_pending: false };
        assert!(!plain.steps().contains(&WatchStep::PendingCleared));
    }

    #[test]
    fn pause_is_done_when_the_topology_agrees() {
        let plan = WatchPlan::Pause { paused: true };
        let mut t = topo(vec![]);
        t.paused = Some(false);
        assert_eq!(progress(&plan, &t, None), WatchStep::Accepted);
        t.paused = Some(true);
        assert_eq!(progress(&plan, &t, None), WatchStep::Done);
    }

    #[test]
    fn scheduled_actions_and_cancels_are_not_watched() {
        let t = topo(vec![m("pg-1", Role::Primary, "running", 2)]);
        let sched = PatroniAction::Switchover { candidate: None, scheduled_at: Some("2030-01-01T00:00:00Z".into()) };
        assert!(WatchPlan::for_action(&sched, &t, None).is_none());
        assert!(WatchPlan::for_action(&PatroniAction::CancelSwitchover, &t, None).is_none());
        let now = PatroniAction::Switchover { candidate: None, scheduled_at: None };
        assert_eq!(WatchPlan::for_action(&now, &t, None).unwrap().deadline(), Duration::from_secs(120));
        let restart = PatroniAction::Restart { member: "pg-1".into(), only_pending: false, scheduled_at: None };
        assert_eq!(WatchPlan::for_action(&restart, &t, Some("t0".into())).unwrap().deadline(), Duration::from_secs(300));
        assert_eq!(WatchPlan::for_action(&PatroniAction::Pause { paused: true }, &t, None).unwrap().deadline(),
                   Duration::from_secs(30));
    }

    #[test]
    fn event_json() {
        let e = WatchEvent { step: WatchStep::NewLeaderRunning, status: WatchStatus::Running, at_ms: 5, detail: None };
        assert_eq!(serde_json::to_value(e).unwrap(),
                   serde_json::json!({"step": "newLeaderRunning", "status": "running", "atMs": 5, "detail": null}));
    }
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib actions::watch`
Expected: FAIL.

- [ ] **Step 2: Шаги наблюдения**

`src-tauri/src/actions/watch.rs`:

```rust
//! Наблюдение после действия Patroni (спец. P2 §8.6): ускоренный опрос топологии и шаги до «готово».

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::actions::patroni::PatroniAction;
use crate::session::SessionKind;
use crate::state::AppState;
use crate::topology::model::{Role, Topology};

/// Опрос топологии во время наблюдения (вместо 10 с).
pub const BOOST_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchStep {
    Accepted,
    LeaderStepped,
    NewLeaderRunning,
    TimelineIncreased,
    ReplicasStreaming,
    Restarted,
    MemberRunning,
    PendingCleared,
    PauseApplied,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchStatus {
    Running,
    Done,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchEvent {
    pub step: WatchStep,
    pub status: WatchStatus,
    pub at_ms: i64,
    pub detail: Option<String>,
}

/// Куда уходят события наблюдения; `false` — канал закрыт, наблюдение заканчивается.
pub trait WatchSink: Send + Sync {
    fn send(&self, e: WatchEvent) -> bool;
}

#[derive(Debug, Clone, PartialEq)]
pub enum WatchPlan {
    Switch { old_leader: String, candidate: Option<String>, timeline: Option<i64> },
    Restart { member: String, start_before: Option<String>, only_pending: bool },
    Pause { paused: bool },
}

impl WatchPlan {
    /// Наблюдается то, что происходит сейчас: запланированные действия и отмены — нет (обзор покажет баннер).
    pub fn for_action(a: &PatroniAction, before: &Topology, restart_start: Option<String>) -> Option<WatchPlan> {
        match a {
            PatroniAction::Switchover { candidate, scheduled_at: None } => Some(WatchPlan::Switch {
                old_leader: before.leader()?.name.clone(),
                candidate: candidate.clone(),
                timeline: before.cluster_timeline(),
            }),
            PatroniAction::Failover { candidate } => Some(WatchPlan::Switch {
                old_leader: before.leader().map(|m| m.name.clone()).unwrap_or_default(),
                candidate: Some(candidate.clone()),
                timeline: before.cluster_timeline(),
            }),
            PatroniAction::Restart { member, only_pending, scheduled_at: None } => Some(WatchPlan::Restart {
                member: member.clone(),
                start_before: restart_start,
                only_pending: *only_pending,
            }),
            PatroniAction::Pause { paused } => Some(WatchPlan::Pause { paused: *paused }),
            _ => None,
        }
    }

    pub fn steps(&self) -> Vec<WatchStep> {
        use WatchStep::*;
        match self {
            WatchPlan::Switch { .. } => vec![Accepted, LeaderStepped, NewLeaderRunning, TimelineIncreased, ReplicasStreaming, Done],
            WatchPlan::Restart { only_pending: true, .. } => vec![Accepted, Restarted, MemberRunning, PendingCleared, Done],
            WatchPlan::Restart { .. } => vec![Accepted, Restarted, MemberRunning, Done],
            WatchPlan::Pause { .. } => vec![Accepted, PauseApplied, Done],
        }
    }

    pub fn deadline(&self) -> Duration {
        match self {
            WatchPlan::Switch { .. } => Duration::from_secs(120),
            WatchPlan::Restart { .. } => Duration::from_secs(300),
            WatchPlan::Pause { .. } => Duration::from_secs(30),
        }
    }
}

fn is_leader(role: Role) -> bool {
    matches!(role, Role::Primary | Role::StandbyLeader)
}

fn running_or_streaming(state: Option<&str>) -> bool {
    matches!(state, Some("running") | Some("streaming"))
}

/// Выполнено ли условие шага на текущей топологии.
fn reached(plan: &WatchPlan, step: WatchStep, t: &Topology, start_now: Option<&str>) -> bool {
    use WatchStep::*;
    match (plan, step) {
        (_, Accepted) | (_, Done) => true,
        (WatchPlan::Switch { old_leader, .. }, LeaderStepped) => {
            t.member(old_leader).is_none_or(|m| !is_leader(m.role) || !m.reachable)
        }
        (WatchPlan::Switch { old_leader, candidate, .. }, NewLeaderRunning) => t.leader().is_some_and(|l| {
            &l.name != old_leader
                && l.reachable
                && l.state.as_deref() == Some("running")
                && candidate.as_ref().is_none_or(|c| c == &l.name)
        }),
        (WatchPlan::Switch { timeline, .. }, TimelineIncreased) => match (timeline, t.cluster_timeline()) {
            (Some(before), Some(now)) => now > *before,
            _ => true,
        },
        (WatchPlan::Switch { .. }, ReplicasStreaming) => t
            .members
            .iter()
            .filter(|m| m.reachable && !is_leader(m.role))
            .all(|m| running_or_streaming(m.state.as_deref())),
        (WatchPlan::Restart { start_before, .. }, Restarted) => {
            start_now.is_some() && start_now != start_before.as_deref()
        }
        (WatchPlan::Restart { member, .. }, MemberRunning) => {
            t.member(member).is_some_and(|m| m.reachable && running_or_streaming(m.state.as_deref()))
        }
        (WatchPlan::Restart { member, .. }, PendingCleared) => t.member(member).is_some_and(|m| !m.pending_restart),
        (WatchPlan::Pause { paused }, PauseApplied) => t.paused.unwrap_or(false) == *paused,
        _ => false,
    }
}

/// Последний шаг, до которого дошло действие: шаги проходятся по порядку, первый невыполненный останавливает.
pub fn progress(plan: &WatchPlan, t: &Topology, start_now: Option<&str>) -> WatchStep {
    let mut last = WatchStep::Accepted;
    for step in plan.steps() {
        if !reached(plan, step, t, start_now) {
            break;
        }
        last = step;
    }
    last
}
```

В тесте `switchover_steps_follow_the_cluster` шаг `promoted` даёт `TimelineIncreased`, потому что `pg-1` в состоянии `stopped` и реплики не все `streaming`.

- [ ] **Step 3: Падающий тест ускоренного опроса (`topology/runtime.rs`)**

В модуле `tests` (фикстура `manual` — опрос раз в 60 с, `Scripted::calls()` — число опросов; подключение — как в соседних тестах `manual`):

```rust
    #[tokio::test]
    async fn boost_polls_often_until_it_ends() {
        let (rt, src, _rec, _pools) = manual(vec![Ok(sample_topology())]);
        rt.connect(sample_patroni_profile("c")).await.unwrap();
        let base = src.calls();
        rt.boost("c", Duration::from_millis(30), Instant::now() + Duration::from_millis(300));
        eventually("ускоренный опрос", || src.calls() >= base + 3).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        let after = src.calls();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(src.calls(), after, "срок ускорения вышел — снова раз в 60 с");
        rt.boost("c", Duration::from_millis(30), Instant::now() + Duration::from_secs(60));
        eventually("снова ускорен", || src.calls() >= after + 2).await;
        rt.unboost("c");
        let stopped = src.calls();
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(src.calls() <= stopped + 1, "unboost возвращает обычный интервал");
    }
```

(`sample_topology` / `sample_patroni_profile` — имена помощников соседних тестов модуля; взять существующие.)

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib topology::runtime::tests::boost`
Expected: FAIL — нет `boost`.

- [ ] **Step 4: `boost` и `unboost`**

`src-tauri/src/topology/runtime.rs`. В `Entry`:

```rust
    /// Ускоренный опрос наблюдения (P2b): интервал и до какого момента.
    boost: Option<(Duration, Instant)>,
    /// Будит фоновый опрос: ускорение начинается сразу, а не после текущего ожидания.
    wake: Arc<tokio::sync::Notify>,
```

(создаётся в `connect_locked` вместе с прочими полями: `boost: None, wake: Arc::new(Notify::new())`; `Instant` — `std::time::Instant`.)

```rust
    /// Опрос раз в `interval` до `until` (наблюдение после действия). Фоновый опрос просыпается сразу.
    pub fn boost(&self, id: &str, interval: Duration, until: Instant) {
        if let Some(e) = self.entries.lock_safe().get_mut(id) {
            e.boost = Some((interval, until));
            e.wake.notify_one();
        }
    }

    /// Снимает ускорение (панель наблюдения закрыта).
    pub fn unboost(&self, id: &str) {
        if let Some(e) = self.entries.lock_safe().get_mut(id) {
            e.boost = None;
        }
    }

    /// Сколько ждать до следующего опроса открытия `generation` и чем его разбудить; `None` — открытия нет.
    fn next_wait(&self, id: &str, generation: u64) -> Option<(Duration, Arc<tokio::sync::Notify>)> {
        let mut entries = self.entries.lock_safe();
        let e = entries.get_mut(id).filter(|e| e.generation == generation)?;
        let wait = match e.boost {
            Some((interval, until)) if Instant::now() < until => interval.min(self.interval),
            Some(_) => {
                e.boost = None;
                self.interval
            }
            None => self.interval,
        };
        Some((wait, e.wake.clone()))
    }
```

`spawn_poller`:

```rust
        tokio::spawn(async move {
            loop {
                let Some((wait, wake)) = weak.upgrade().and_then(|rt| rt.next_wait(&key, generation)) else { break };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = wake.notified() => {}
                }
                let Some(rt) = weak.upgrade() else { break };
                if !rt.poll_once(&key, generation).await {
                    break;
                }
            }
        })
```

Порядок блокировок не меняется: `entries` берётся коротко, без `await`.

- [ ] **Step 5: Сессия наблюдения и выполнение с наблюдением**

`src-tauri/src/actions/watch.rs` (дополнение):

```rust
fn now_ms() -> i64 {
    crate::actions::journal::now_ms()
}

/// Открывает сессию наблюдения: ускоренный опрос, шаги по каждому опросу, срок; итог — id сессии для `stop_session`.
/// Остановка сессии (панель закрыта, подключение закрыто) прерывает задачу и снимает ускорение.
pub fn start(st: &AppState, id: &str, plan: WatchPlan, sink: Arc<dyn WatchSink>) -> String {
    let session_id = uuid::Uuid::new_v4().to_string();
    let runtimes = st.runtimes.clone();
    let live = st.live.clone();
    let connections = st.connections.clone();
    let sessions = st.sessions.clone();
    let (conn, sid) = (id.to_string(), session_id.clone());
    let deadline = Instant::now() + plan.deadline();
    runtimes.boost(id, BOOST_INTERVAL, deadline);
    let task = tokio::spawn(async move {
        let mut last = WatchStep::Accepted;
        sink.send(WatchEvent { step: last, status: WatchStatus::Running, at_ms: now_ms(), detail: None });
        while Instant::now() < deadline {
            tokio::time::sleep(BOOST_INTERVAL).await;
            let Some(t) = runtimes.view(&conn).topology else { continue };
            let start_now = match &plan {
                WatchPlan::Restart { member, .. } => match connections.get(&conn) {
                    Ok(p) => match live.http_for(&p).await {
                        Ok(http) => crate::actions::patroni::postmaster_start(http.as_ref(), &t, member).await,
                        Err(_) => None,
                    },
                    Err(_) => None,
                },
                _ => None,
            };
            let step = progress(&plan, &t, start_now.as_deref());
            if step > last {
                last = step;
                let status = if step == WatchStep::Done { WatchStatus::Done } else { WatchStatus::Running };
                if !sink.send(WatchEvent { step, status, at_ms: now_ms(), detail: None }) {
                    break;
                }
                if step == WatchStep::Done {
                    break;
                }
            }
        }
        if last != WatchStep::Done {
            sink.send(WatchEvent { step: last, status: WatchStatus::TimedOut, at_ms: now_ms(), detail: None });
        }
        runtimes.unboost(&conn);
        sessions.forget(&sid);
    });
    let (abort, runtimes, conn) = (task.abort_handle(), st.runtimes.clone(), id.to_string());
    st.sessions.insert(
        session_id.clone(),
        id,
        SessionKind::Watch,
        Box::new(move || {
            abort.abort();
            runtimes.unboost(&conn);
        }),
    );
    session_id
}
```

(id сессии — `uuid::Uuid::new_v4()`, как у `sampler::start`; `AppState.connections` — `Arc<ConnectionStore>`, `get(id)` возвращает профиль.)

`src-tauri/src/actions/patroni.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniActionResult {
    pub response: String,
    /// Сессия наблюдения (switchover / failover / restart «сейчас», pause / resume); `None` — не наблюдается.
    pub watch: Option<String>,
    pub journal_written: bool,
}

pub async fn execute(
    st: &AppState,
    id: &str,
    action: &PatroniAction,
    confirmation: &Confirmation,
    sink: std::sync::Arc<dyn crate::actions::watch::WatchSink>,
) -> AppResult<PatroniActionResult> {
    let done = perform(st, id, action, confirmation).await?;
    let watch = crate::actions::watch::WatchPlan::for_action(action, &done.before, done.restart_start.clone())
        .map(|plan| crate::actions::watch::start(st, id, plan, sink));
    Ok(PatroniActionResult { response: done.response, watch, journal_written: done.journal_written })
}
```

`src-tauri/src/commands/actions.rs`:

```rust
struct WatchChannel(tauri::ipc::Channel<crate::actions::watch::WatchEvent>);

impl crate::actions::watch::WatchSink for WatchChannel {
    fn send(&self, e: crate::actions::watch::WatchEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

/// Действие Patroni; после switchover / failover / restart «сейчас» и pause / resume — наблюдение в `channel`.
#[tauri::command]
pub async fn execute_patroni_action(
    state: State<'_, AppState>,
    id: String,
    action: crate::actions::patroni::PatroniAction,
    confirmation: crate::actions::Confirmation,
    channel: tauri::ipc::Channel<crate::actions::watch::WatchEvent>,
) -> AppResult<crate::actions::patroni::PatroniActionResult> {
    crate::actions::patroni::execute(&state, &id, &action, &confirmation, std::sync::Arc::new(WatchChannel(channel)))
        .await
}
```

`src-tauri/src/actions/mod.rs`: `pub mod watch;`; регистрация команды в `generate_handler!`.

- [ ] **Step 6: Сценарии со сменой лидера и рестартом (`it_patroni_actions.rs`)**

```rust
use std::sync::{Arc, Mutex};

use glafira_pg_lib::actions::watch::{WatchEvent, WatchSink, WatchStatus, WatchStep};

#[derive(Default)]
struct Events(Mutex<Vec<WatchEvent>>);

impl WatchSink for Events {
    fn send(&self, e: WatchEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

impl Events {
    async fn until_finished(&self, limit: Duration) -> WatchEvent {
        common::wait_until("наблюдение закончилось", limit, async || {
            self.0.lock().unwrap().last().is_some_and(|e| e.status != WatchStatus::Running)
        })
        .await;
        self.0.lock().unwrap().last().cloned().unwrap()
    }
}

fn replica_of(t: &glafira_pg_lib::topology::model::Topology) -> String {
    let leader = t.leader().unwrap().name.clone();
    t.members.iter().find(|m| m.name != leader && m.reachable).unwrap().name.clone()
}

#[tokio::test]
async fn switchover_now_is_watched_until_done() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let _restore = common::restore_on_drop(common::wait_stand);
    let t = st.runtimes.view("it").topology.unwrap();
    let candidate = replica_of(&t);
    let events = Arc::new(Events::default());
    let action = PatroniAction::Switchover { candidate: Some(candidate.clone()), scheduled_at: None };
    let r = pa::execute(&st, "it", &action, &Confirmation::default(), events.clone()).await.unwrap();
    assert!(r.watch.is_some() && r.journal_written);
    let last = events.until_finished(Duration::from_secs(130)).await;
    assert_eq!((last.step, last.status), (WatchStep::Done, WatchStatus::Done), "{:?}", events.0.lock().unwrap());
    assert_eq!(st.runtimes.view("it").topology.unwrap().leader().unwrap().name, candidate);
}

#[tokio::test]
async fn replica_restart_and_pause_resume_are_watched() {
    if !docker_enabled() {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let _restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["resume", "--wait"]);
        common::wait_stand();
    });
    let t = st.runtimes.view("it").topology.unwrap();
    let replica = replica_of(&t);
    let events = Arc::new(Events::default());
    let restart = PatroniAction::Restart { member: replica, only_pending: false, scheduled_at: None };
    pa::execute(&st, "it", &restart, &Confirmation::default(), events.clone()).await.unwrap();
    assert_eq!(events.until_finished(Duration::from_secs(310)).await.step, WatchStep::Done);
    for paused in [true, false] {
        let events = Arc::new(Events::default());
        pa::execute(&st, "it", &PatroniAction::Pause { paused }, &Confirmation::default(), events.clone()).await.unwrap();
        assert_eq!(events.until_finished(Duration::from_secs(40)).await.status, WatchStatus::Done);
    }
}

/// Failover (`GPG_IT_FAILOVER=1`): пауза → остановка PostgreSQL лидера → failover на реплику → снятие паузы →
/// старый лидер возвращается репликой.
#[tokio::test]
async fn failover_after_the_leader_stops() {
    if !docker_enabled() || common::env("GPG_IT_FAILOVER").as_deref() != Some("1") {
        return;
    }
    let Some((_dir, st)) = common::open_patroni().await else { return };
    let _restore = common::restore_on_drop(|| {
        let _ = common::try_patronictl(&["resume", "--wait"]);
        common::wait_stand();
    });
    let t = st.runtimes.view("it").topology.unwrap();
    let leader = t.leader().unwrap().name.clone();
    let candidate = replica_of(&t);
    let none = Arc::new(Events::default());
    pa::execute(&st, "it", &PatroniAction::Pause { paused: true }, &Confirmation::default(), none.clone()).await.unwrap();
    none.until_finished(Duration::from_secs(40)).await;
    // Контейнер участника — gp-<имя участника>; pg_ctl — от postgres.
    common::docker(&["exec", "-u", "postgres", &format!("gp-{leader}"), "pg_ctl", "-D",
                     "/var/lib/postgresql/patroni/data", "stop", "-m", "fast"]);
    topology_until(&st, "лидер остановлен", Duration::from_secs(60), |t| {
        t.member(&leader).is_some_and(|m| m.state.as_deref() != Some("running"))
    })
    .await;
    let events = Arc::new(Events::default());
    let confirm = Confirmation { confirm: None, typed_name: Some(candidate.clone()) };
    pa::execute(&st, "it", &PatroniAction::Failover { candidate: candidate.clone() }, &confirm, events.clone())
        .await
        .unwrap();
    let last = events.until_finished(Duration::from_secs(130)).await;
    assert!(last.step >= WatchStep::NewLeaderRunning, "{:?}", events.0.lock().unwrap());
    pa::execute(&st, "it", &PatroniAction::Pause { paused: false }, &Confirmation::default(), Arc::new(Events::default()))
        .await
        .unwrap();
    common::wait_stand();
    let t = st.runtimes.view("it").topology.unwrap();
    assert_eq!(t.leader().unwrap().name, candidate);
}
```

(Если `pg_ctl` не в `PATH` образа Patroni стенда — полный путь из `docker exec gp-patroni1 ls /usr/lib/postgresql`. Шаг «реплики streaming» после failover при остановленном старом лидере не выполнится, пока пауза не снята, поэтому проверяется `NewLeaderRunning`, а не `Done`.)

- [ ] **Step 7: Проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && GPG_IT_DOCKER=1 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --test it_patroni_actions -- --test-threads=1`
Expected: PASS; стенд после тестов — лидер и две потоковые реплики (`scripts/wait-stand.sh`). Failover — отдельно, с `GPG_IT_FAILOVER=1`, один раз перед коммитом.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/src src-tauri/tests/it_patroni_actions.rs
git commit -m "feat(actions): наблюдение после действия Patroni — ускоренный опрос и шаги до «готово»; команда execute_patroni_action"
```

---
### Task 9: Фронтенд — типы и команды P2b, `ConfirmDialog`, профиль подключения во вкладке

**Files:**
- Create: `src/components/common/ConfirmDialog.tsx`, `src/components/common/ConfirmDialog.test.tsx`, `src/lib/useConnectionProfile.ts`, `src/lib/schedule.ts`, `src/lib/schedule.test.ts`
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/api/client.channel.test.ts`, `src/locales/{en,ru}.json`
- Test: `ConfirmDialog.test.tsx`, `client.channel.test.ts`, `schedule.test.ts`

**Interfaces:**
- Consumes: команды задач 2–8 (имена и JSON — как в их Interfaces)
- Produces:
  - типы TS: `Confirmation`, `SignalKind`, `SessionRef`, `SessionTarget`, `SessionActionRequest`, `DenyReason`, `PreviewSession`, `PreviewRow`, `SessionActionPreview`, `SessionActionExecute`, `SessionResult`, `ActionOutcome`, `SessionActionResult`, `SlotDropRequest`, `SlotDropPreview`, `ActionResult`, `PatroniAction`, `MemberPreview`, `PatroniActionPreview`, `PatroniActionResult`, `WatchStep`, `WatchStatus`, `WatchEvent`, `ActionRecord` (+ `SessionRecord`, `SlotRecord`, `PatroniRecord`)
  - `api.previewSessionAction(id, request)`, `api.executeSessionAction(id, request, confirmation)`, `api.previewDropSlot(id, request)`, `api.dropSlot(id, request, confirmation)`, `api.previewPatroniAction(id, action)`, `api.executePatroniAction(id, action, confirmation, onEvent)`, `api.listActions(connectionId, limit)`
  - `ConfirmDialog` и `needsConfirmation(connection, objectName?)` — интерфейс Glafira Kafka без изменений
  - `useConnectionProfile(connectionId) -> ConnectionProfile | undefined`; `canAct(profile?) -> boolean` (есть профиль и не read-only)
  - `toRfc3339(local: string, offsetMin?: number) -> string` (значение `datetime-local` → RFC 3339 с местным смещением)

- [ ] **Step 1: Типы**

`src/api/types.ts`:

```ts
/** Подтверждение действия (спец. P2 §8.1): имя подключения на prod, имя объекта или число сессий. */
export interface Confirmation {
  confirm: string | null;
  typedName: string | null;
}

export type SignalKind = "cancel" | "terminate";
export interface SessionRef {
  pid: number;
  backendStartUs: number;
}
export type SessionTarget =
  | { kind: "selected"; sessions: SessionRef[] }
  | { kind: "idleInTransaction"; olderThanS: number }
  | { kind: "rootBlocker"; pid: number; backendStartUs: number };
export interface SessionActionRequest {
  member: string;
  kind: SignalKind;
  target: SessionTarget;
}
export type DenyReason = "superuserSession" | "otherRole" | "notClientBackend" | "ownConnection";
export interface PreviewSession {
  pid: number;
  backendStartUs: number;
  user: string | null;
  database: string | null;
  application: string | null;
  client: string | null;
  state: string | null;
  backendType: string | null;
  xactAgeS: number | null;
  stateAgeS: number | null;
  query: string | null;
}
export interface PreviewRow {
  session: PreviewSession;
  allowed: boolean;
  reason: DenyReason | null;
}
export interface SessionActionPreview {
  member: string;
  rows: PreviewRow[];
}
export interface SessionActionExecute extends SessionActionRequest {
  sessions: SessionRef[];
}
export type SessionResult = SessionRef &
  ({ result: "terminated" | "signalSent" | "gone" | "noLongerMatches" | "notAllowed" } | { result: "failed"; error: AppError });
export type ActionOutcome = "ok" | "partial" | "error";
export interface SessionActionResult {
  member: string;
  results: SessionResult[];
  outcome: ActionOutcome;
  journalWritten: boolean;
}

export interface SlotDropRequest {
  member: string;
  slot: string;
}
export interface SlotDropPreview {
  member: string;
  slot: Slot;
}
export interface ActionResult {
  journalWritten: boolean;
}

export type PatroniAction =
  | { kind: "switchover"; candidate: string | null; scheduledAt: string | null }
  | { kind: "cancelSwitchover" }
  | { kind: "failover"; candidate: string }
  | { kind: "restart"; member: string; onlyPending: boolean; scheduledAt: string | null }
  | { kind: "cancelRestart"; member: string }
  | { kind: "pause"; paused: boolean };
export interface MemberPreview {
  name: string;
  role: Role;
  state: string | null;
  pendingRestart: boolean;
  scheduledRestart: ScheduledRestart | null;
}
export interface PatroniActionPreview {
  leader: string | null;
  paused: boolean;
  candidates: Candidate[];
  leaderClientSessions: number | null;
  member: MemberPreview | null;
  scheduledSwitchover: ScheduledSwitchover | null;
  canSend: boolean;
}
export interface PatroniActionResult {
  response: string;
  watch: string | null;
  journalWritten: boolean;
}
export type WatchStep =
  | "accepted"
  | "leaderStepped"
  | "newLeaderRunning"
  | "timelineIncreased"
  | "replicasStreaming"
  | "restarted"
  | "memberRunning"
  | "pendingCleared"
  | "pauseApplied"
  | "done";
export type WatchStatus = "running" | "done" | "failed" | "timedOut";
export interface WatchEvent {
  step: WatchStep;
  status: WatchStatus;
  atMs: number;
  detail: string | null;
}

export interface SessionRecord {
  pid: number;
  backendStartUs: number;
  user: string | null;
  database: string | null;
  application: string | null;
  client: string | null;
  state: string | null;
  xactAgeS: number | null;
  queryId: string | null;
  query: string | null;
  result: string;
}
export interface SlotRecord {
  name: string;
  slotType: string;
  plugin: string | null;
  database: string | null;
  retainedBytes: number | null;
  walStatus: string | null;
}
export interface PatroniRecord {
  method: string;
  path: string;
  body: unknown;
  status: number | null;
  response: string | null;
}
export interface ActionRecord {
  ts: number;
  action: string;
  connection: { id: string; name: string; environment: Environment };
  member: string | null;
  prodConfirmed: boolean;
  outcome: ActionOutcome;
  code: string | null;
  ms: number;
  sessions?: SessionRecord[];
  slot?: SlotRecord;
  patroni?: PatroniRecord;
}
```

- [ ] **Step 2: Падающие тесты команд**

`src/api/client.channel.test.ts`, в таблицу «команды и аргументы»:

```ts
    ["previewSessionAction", () => api.previewSessionAction("c", sessionReq), "preview_session_action", { id: "c", request: sessionReq }],
    ["executeSessionAction", () => api.executeSessionAction("c", sessionExec, confirmation), "execute_session_action", { id: "c", request: sessionExec, confirmation }],
    ["previewDropSlot", () => api.previewDropSlot("c", slotReq), "preview_drop_slot", { id: "c", request: slotReq }],
    ["dropSlot", () => api.dropSlot("c", slotReq, confirmation), "drop_slot", { id: "c", request: slotReq, confirmation }],
    ["previewPatroniAction", () => api.previewPatroniAction("c", pause), "preview_patroni_action", { id: "c", action: pause }],
    ["listActions", () => api.listActions("c", 200), "list_actions", { connectionId: "c", limit: 200 }],
```

с данными над таблицей:

```ts
const sessionReq: SessionActionRequest = { member: "pg-1", kind: "terminate", target: { kind: "idleInTransaction", olderThanS: 300 } };
const sessionExec: SessionActionExecute = { ...sessionReq, sessions: [{ pid: 42, backendStartUs: 1 }] };
const slotReq: SlotDropRequest = { member: "pg-1", slot: "it_dead_slot" };
const pause: PatroniAction = { kind: "pause", paused: true };
const confirmation: Confirmation = { confirm: "billing-prod", typedName: null };
```

и отдельный тест канала наблюдения (по образцу теста `startSampler`):

```ts
  it("executePatroniAction: события наблюдения приходят в onEvent", async () => {
    const got: WatchEvent[] = [];
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const ch = (args as { channel: { onmessage: (e: WatchEvent) => void } }).channel;
      ch.onmessage({ step: "accepted", status: "running", atMs: 1, detail: null });
      return { response: "ok", watch: "w-1", journalWritten: true };
    });
    const r = await api.executePatroniAction("c", pause, confirmation, (e) => got.push(e));
    expect(r.watch).toBe("w-1");
    expect(got.map((e) => e.step)).toEqual(["accepted"]);
    expect(vi.mocked(invoke).mock.calls[0][0]).toBe("execute_patroni_action");
    expect(vi.mocked(invoke).mock.calls[0][1]).toMatchObject({ id: "c", action: pause, confirmation });
  });
```

Run: `pnpm vitest run src/api`
Expected: FAIL — методов нет.

- [ ] **Step 3: Команды**

`src/api/client.ts`, после `stopSession`:

```ts
  previewSessionAction: (id: string, request: SessionActionRequest) =>
    call<SessionActionPreview>("preview_session_action", { id, request }),
  executeSessionAction: (id: string, request: SessionActionExecute, confirmation: Confirmation) =>
    call<SessionActionResult>("execute_session_action", { id, request, confirmation }),
  previewDropSlot: (id: string, request: SlotDropRequest) => call<SlotDropPreview>("preview_drop_slot", { id, request }),
  dropSlot: (id: string, request: SlotDropRequest, confirmation: Confirmation) =>
    call<ActionResult>("drop_slot", { id, request, confirmation }),
  previewPatroniAction: (id: string, action: PatroniAction) =>
    call<PatroniActionPreview>("preview_patroni_action", { id, action }),
  /** Действие Patroni; события наблюдения (если оно открылось) — в `onEvent`. */
  executePatroniAction: (id: string, action: PatroniAction, confirmation: Confirmation, onEvent: (e: WatchEvent) => void) => {
    const channel = new Channel<WatchEvent>();
    channel.onmessage = onEvent;
    return call<PatroniActionResult>("execute_patroni_action", { id, action, confirmation, channel });
  },
  listActions: (connectionId: string | null, limit: number) => call<ActionRecord[]>("list_actions", { connectionId, limit }),
```

(+ импорты типов.)

- [ ] **Step 4: `ConfirmDialog`**

Перенести `/Users/vvnovg/kafka-explorer/src/components/common/ConfirmDialog.tsx` и `ConfirmDialog.test.tsx` без изменения интерфейса (`needsConfirmation`, пропсы `connection`, `title`, `confirmLabel`, `objectName`, `objectLabel`, `danger`, `busy`, `children`, `onConfirm`, `onCancel`). Импорты: `Confirmation`, `ConnectionProfile` — из `../../api/types`; в тесте `newProfile` — из `../connections/profile`; имена объектов в тестах — наши (`it_dead_slot` вместо топика, «имя слота» вместо «имя топика»). Стиль `.confirm` в `src/styles.css` уже есть (P1).

Словари, раздел `common`:

```json
"confirm": {
  "typeObject": "To confirm, type the {{label}} <code>{{name}}</code>",
  "prodNotice": "This is a prod connection. Type its name <code>{{name}}</code>",
  "defaultLabel": "name",
  "objectAria": "Confirmation: {{label}}",
  "connectionAria": "Connection name"
}
```

```json
"confirm": {
  "typeObject": "Для подтверждения введите {{label}} <code>{{name}}</code>",
  "prodNotice": "Это prod-подключение. Введите его имя <code>{{name}}</code>",
  "defaultLabel": "имя",
  "objectAria": "Подтверждение: {{label}}",
  "connectionAria": "Имя подключения"
}
```

- [ ] **Step 5: Профиль и время расписания**

`src/lib/useConnectionProfile.ts`:

```ts
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";
import type { ConnectionProfile } from "../api/types";

/** Профиль подключения вкладки (тот же запрос `["connections"]`, что у дерева): окружение, имя, read-only. */
export function useConnectionProfile(connectionId: string): ConnectionProfile | undefined {
  const { data } = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  return data?.find((c) => c.id === connectionId);
}

/** Действия доступны: профиль известен и не read-only (Rust всё равно проверит, спец. §8.1). */
export function canAct(profile: ConnectionProfile | undefined): profile is ConnectionProfile {
  return profile !== undefined && !profile.readOnly;
}
```

`src/lib/schedule.ts`:

```ts
const pad = (n: number) => String(Math.abs(n)).padStart(2, "0");

/**
 * Значение `<input type="datetime-local">` («2026-10-25T14:30») в RFC 3339 с местным смещением
 * («2026-10-25T14:30:00+03:00»): Patroni принимает время со смещением. `offsetMin` — для тестов
 * (минуты к востоку от UTC); по умолчанию — смещение этой даты в часовом поясе машины.
 */
export function toRfc3339(local: string, offsetMin?: number): string {
  const [date, time = "00:00"] = local.split("T");
  const hms = time.length === 5 ? `${time}:00` : time;
  const off = offsetMin ?? -new Date(`${date}T${hms}`).getTimezoneOffset();
  const sign = off >= 0 ? "+" : "-";
  return `${date}T${hms}${sign}${pad(Math.trunc(off / 60))}:${pad(off % 60)}`;
}
```

`src/lib/schedule.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { toRfc3339 } from "./schedule";

describe("toRfc3339", () => {
  it("местное время со смещением", () => {
    expect(toRfc3339("2026-10-25T14:30", 180)).toBe("2026-10-25T14:30:00+03:00");
    expect(toRfc3339("2026-10-25T14:30", -330)).toBe("2026-10-25T14:30:00-05:30");
    expect(toRfc3339("2026-10-25T14:30:15", 0)).toBe("2026-10-25T14:30:15+00:00");
  });
});
```

- [ ] **Step 6: Проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/api src/components/common/ConfirmDialog.tsx src/components/common/ConfirmDialog.test.tsx src/lib/useConnectionProfile.ts src/lib/schedule.ts src/lib/schedule.test.ts src/locales
git commit -m "feat(ui): типы и команды действий P2b, ConfirmDialog из Glafira Kafka, время расписания RFC 3339"
```

---

### Task 10: Фронтенд — вкладка «Журнал действий»

**Files:**
- Create: `src/components/actions/ActionsJournal.tsx`, `src/components/actions/ActionsJournal.test.tsx`
- Modify: `src/state/workspace.ts` (`TabSpec`, `tabId`), `src/components/TabContent.tsx`, `src/components/Layout.tsx` (заголовок вкладки), `src/components/connections/tree/DiagnosticsNodes.tsx` (+ тест), `src/components/help/helpChapterFor.ts` (+ тест), `src/locales/{en,ru}.json`
- Test: `ActionsJournal.test.tsx`, `DiagnosticsNodes.test.tsx`, `helpChapterFor.test.ts`

**Interfaces:**
- Consumes: `api.listActions`, `ActionRecord`
- Produces: `TabSpec` `{ kind: "actions"; connectionId: string }` (id `actions:<подключение>`); `ActionsJournal({ connectionId })`; узел дерева «Журнал действий»; F1 на вкладке — глава `diagnostics`, якорь `journal`

- [ ] **Step 1: Падающие тесты**

`src/components/actions/ActionsJournal.test.tsx`:

```tsx
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api/client";
import type { ActionRecord } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { ActionsJournal } from "./ActionsJournal";

vi.mock("../../api/client", () => ({ api: { listActions: vi.fn() } }));

const terminate: ActionRecord = {
  ts: new Date(2026, 9, 25, 12, 0, 5).getTime(),
  action: "terminate",
  connection: { id: "c", name: "billing-prod", environment: "prod" },
  member: "pg-1",
  prodConfirmed: true,
  outcome: "partial",
  code: null,
  ms: 412,
  sessions: [
    { pid: 42, backendStartUs: 1, user: "app", database: "orders", application: "api", client: "10.0.0.5", state: "idle in transaction",
      xactAgeS: 3600, queryId: null, query: "UPDATE shop.orders SET status = 'x'", result: "terminated" },
    { pid: 43, backendStartUs: 2, user: "app", database: "orders", application: "api", client: null, state: null,
      xactAgeS: null, queryId: null, query: null, result: "notAllowed" },
  ],
};
const switchover: ActionRecord = {
  ts: new Date(2026, 9, 25, 12, 1, 0).getTime(),
  action: "switchover",
  connection: { id: "c", name: "billing-prod", environment: "prod" },
  member: "pg-1",
  prodConfirmed: true,
  outcome: "ok",
  code: null,
  ms: 80,
  patroni: { method: "POST", path: "http://pg-1:8008/switchover", body: { leader: "pg-1" }, status: 200, response: "Successfully switched over" },
};

describe("ActionsJournal", () => {
  beforeEach(() => vi.mocked(api.listActions).mockResolvedValue([switchover, terminate]));

  it("таблица: время, действие, узел, объект, итог; новые сверху", async () => {
    renderWithClient(<ActionsJournal connectionId="c" />);
    const table = await screen.findByRole("table", { name: "Журнал действий" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Switchover");
    expect(rows[1]).toHaveTextContent("Завершение сессий");
    expect(rows[1]).toHaveTextContent("2 сессии");
    expect(rows[1]).toHaveTextContent("частично");
    expect(api.listActions).toHaveBeenCalledWith("c", 1000);
  });

  it("строка раскрывается в полную запись", async () => {
    const user = userEvent.setup();
    renderWithClient(<ActionsJournal connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: /Завершение сессий/ }));
    expect(screen.getByText("UPDATE shop.orders SET status = 'x'")).toBeInTheDocument();
    expect(screen.getByText(/notAllowed|нет прав/)).toBeInTheDocument();
  });

  it("пустой журнал — сообщение", async () => {
    vi.mocked(api.listActions).mockResolvedValue([]);
    renderWithClient(<ActionsJournal connectionId="c" />);
    expect(await screen.findByText("Действий ещё не было")).toBeInTheDocument();
  });
});
```

`DiagnosticsNodes.test.tsx`: в группе «Диагностика» последний узел — «Журнал действий», клик открывает `{ kind: "actions", connectionId }`. `helpChapterFor.test.ts`: `helpChapterFor({ kind: "actions", connectionId: "c" })` → `"diagnostics"`, `helpAnchorFor` → `"journal"`.

Run: `pnpm vitest run src/components/actions src/components/connections/tree src/components/help`
Expected: FAIL.

- [ ] **Step 2: Вкладка и узел дерева**

`src/state/workspace.ts`: в `TabSpec` — `| { kind: "actions"; connectionId: string }`; в `tabId` — `case "actions": return \`actions:${spec.connectionId}\`;`.
`src/components/Layout.tsx`: заголовок — `case "actions": return i18n.t("tabs.actions", { conn });`.
`src/components/TabContent.tsx`: `case "actions": return <ActionsJournal connectionId={tab.connectionId} />;`.
`DiagnosticsNodes.tsx`, после списка инструментов:

```tsx
          <li key="actions">
            <button type="button" className="link" onClick={() => open({ kind: "actions", connectionId })}>
              {t("actions.journal.title")}
            </button>
          </li>
```

(в функции-детях `TreeGroup` вернуть массив `[...tools, journalItem]`).
`helpChapterFor.ts`: `case "actions": return "diagnostics";`; в `helpAnchorFor` — `if (tab?.kind === "actions") return "journal";` (раздел `{#journal}` появится в задаче 14; до неё справка откроется на начале главы).

- [ ] **Step 3: `ActionsJournal`**

`src/components/actions/ActionsJournal.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Fragment, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ActionRecord } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { ErrorBanner } from "../common/ErrorBanner";

/** Подпись действия: ключи литералами (правило словарей). */
const ACTION_KEYS: Record<string, string> = {
  cancel: "actions.kind.cancel",
  terminate: "actions.kind.terminate",
  dropSlot: "actions.kind.dropSlot",
  switchover: "actions.kind.switchover",
  cancelSwitchover: "actions.kind.cancelSwitchover",
  failover: "actions.kind.failover",
  restart: "actions.kind.restart",
  cancelRestart: "actions.kind.cancelRestart",
  pause: "actions.kind.pause",
  resume: "actions.kind.resume",
};
const OUTCOME_KEYS = { ok: "actions.outcome.ok", partial: "actions.outcome.partial", error: "actions.outcome.error" } as const;

function objectText(r: ActionRecord, t: (k: string, o?: Record<string, unknown>) => string): string {
  if (r.slot) return r.slot.name;
  if (r.sessions?.length) return t("actions.journal.sessions", { count: r.sessions.length });
  if (r.patroni) return r.patroni.path;
  return "";
}

/** Журнал действий подключения (спец. P2 §8.8): время, действие, узел, объект, итог; строка раскрывается. */
export function ActionsJournal({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["actions", connectionId], queryFn: () => api.listActions(connectionId, 1000) });
  const [open, setOpen] = useState<number | null>(null);
  const rows = q.data ?? [];
  return (
    <div className="diag-view">
      <div className="toolbar">
        <h2>{t("actions.journal.title")}</h2>
        <button type="button" onClick={() => void q.refetch()}>
          {t("diag.header.refresh")}
        </button>
      </div>
      {q.error && <ErrorBanner error={q.error} />}
      {q.isSuccess && rows.length === 0 && <p className="muted">{t("actions.journal.empty")}</p>}
      {rows.length > 0 && (
        <table className="diag-table" aria-label={t("actions.journal.title")}>
          <thead>
            <tr>
              <th scope="col">{t("actions.journal.columns.time")}</th>
              <th scope="col">{t("actions.journal.columns.action")}</th>
              <th scope="col">{t("actions.journal.columns.member")}</th>
              <th scope="col">{t("actions.journal.columns.object")}</th>
              <th scope="col">{t("actions.journal.columns.outcome")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <Fragment key={`${r.ts}:${i}`}>
                <tr>
                  <td>{formatTimestamp(r.ts)}</td>
                  <td>
                    <button type="button" className="link" aria-expanded={open === i} onClick={() => setOpen(open === i ? null : i)}>
                      {ACTION_KEYS[r.action] ? t(ACTION_KEYS[r.action]) : r.action}
                    </button>
                  </td>
                  <td>{r.member ?? ""}</td>
                  <td>{objectText(r, t)}</td>
                  <td className={r.outcome === "ok" ? undefined : "diag-warn"}>{t(OUTCOME_KEYS[r.outcome])}</td>
                </tr>
                {open === i && (
                  <tr>
                    <td colSpan={5}>
                      <pre>{JSON.stringify(r, null, 2)}</pre>
                    </td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
```

(Полная запись — JSON как есть: в ней всё, что записал Rust, включая текст запроса и итоги сессий. Тест «строка раскрывается» ищет текст запроса и `notAllowed` внутри `pre`.)

Словари: `tabs.actions` — en «Actions — {{conn}}», ru «Журнал действий — {{conn}}»; раздел `actions` (верхнего уровня, перед `hints`):

| ключ | en | ru |
|---|---|---|
| `journal.title` | Actions journal | Журнал действий |
| `journal.empty` | No actions yet | Действий ещё не было |
| `journal.sessions_one` / `_other` | {{count}} session / {{count}} sessions | ru: `_one` {{count}} сессия, `_few` {{count}} сессии, `_many` {{count}} сессий |
| `journal.columns.time` / `action` / `member` / `object` / `outcome` | Time / Action / Node / Object / Outcome | Время / Действие / Узел / Объект / Итог |
| `kind.cancel` / `terminate` / `dropSlot` | Query cancel / Session termination / Slot drop | Отмена запроса / Завершение сессий / Удаление слота |
| `kind.switchover` / `cancelSwitchover` / `failover` | Switchover / Switchover cancelled / Failover | Switchover / Отмена switchover / Failover |
| `kind.restart` / `cancelRestart` / `pause` / `resume` | Restart / Restart cancelled / Pause / Resume | Restart / Отмена restart / Пауза / Снятие паузы |
| `outcome.ok` / `partial` / `error` | done / partly / error | выполнено / частично / ошибка |

(Плюральные ключи — как в существующих словарях проекта; если `pnpm i18n:check` требует иной набор форм, следовать ему. «Switchover», «Failover», «Restart» — в `docs/i18n/same-as-en.json`, если ru совпадает с en.)

- [ ] **Step 4: Проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm test:scripts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): вкладка «Журнал действий» подключения и узел в группе «Диагностика»"
```

---

### Task 11: Фронтенд — отмена и завершение сессий

**Files:**
- Create: `src/components/actions/SessionActionDialog.tsx`, `src/components/actions/SessionActionDialog.test.tsx`
- Modify: `src/components/diag/DiagTab.tsx` (`ToolBody` передаёт `connectionId` и узел отчёта; диалог по кнопкам находок), `src/components/diag/SessionsView.tsx` (кнопки в панели сессии; «Завершить idle in transaction старше…» в наборе idle in transaction), `src/components/diag/LocksView.tsx` (кнопка у корня), `src/components/diag/FindingsPanel.tsx` (кнопки ссылок `action`), их тесты, `src/locales/{en,ru}.json`
- Test: `SessionActionDialog.test.tsx`, `SessionsView.test.tsx`, `LocksView.test.tsx`, `FindingsPanel.test.tsx`

**Interfaces:**
- Consumes: `api.previewSessionAction`, `api.executeSessionAction`, `ConfirmDialog`, `useConnectionProfile`, `canAct`, `SessionActionLink`
- Produces: `SessionActionDialog({ connectionId, profile, member, kind, target, onClose })`; пропсы видов `connectionId` и `member`; `FindingsPanel` — проп `onAction?: (link: Extract<FindingLink, { kind: "action" }>) => void` (нет — кнопок действий нет)

- [ ] **Step 1: Падающие тесты диалога**

`src/components/actions/SessionActionDialog.test.tsx`:

```tsx
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api/client";
import type { ConnectionProfile, SessionActionPreview } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { newProfile } from "../connections/profile";
import { SessionActionDialog } from "./SessionActionDialog";

vi.mock("../../api/client", () => ({ api: { previewSessionAction: vi.fn(), executeSessionAction: vi.fn() } }));

const dev: ConnectionProfile = { ...newProfile(), id: "c", name: "billing-dev", environment: "dev" };
const prod: ConnectionProfile = { ...dev, name: "billing-prod", environment: "prod" };
const row = (pid: number, allowed = true) => ({
  session: { pid, backendStartUs: pid * 10, user: "app", database: "orders", application: "api", client: null,
             state: "idle in transaction", backendType: "client backend", xactAgeS: 600, stateAgeS: 600, query: "UPDATE t" },
  allowed,
  reason: allowed ? null : ("superuserSession" as const),
});
const preview: SessionActionPreview = { member: "pg-1", rows: [row(1), row(2), row(3, false)] };
const target = { kind: "idleInTransaction" as const, olderThanS: 300 };

describe("SessionActionDialog", () => {
  beforeEach(() => {
    vi.mocked(api.previewSessionAction).mockResolvedValue(preview);
    vi.mocked(api.executeSessionAction).mockResolvedValue({
      member: "pg-1",
      results: [{ pid: 1, backendStartUs: 10, result: "terminated" }, { pid: 2, backendStartUs: 20, result: "noLongerMatches" }],
      outcome: "ok",
      journalWritten: true,
    });
  });

  it("предпросмотр: разрешённые сессии уходят на выполнение, запрещённые — с причиной", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const table = await screen.findByRole("table", { name: "Сессии для завершения" });
    expect(within(table).getAllByRole("row")).toHaveLength(4);
    expect(within(table).getByText("Сессия суперпользователя: нужна роль суперпользователя")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Завершить 2 сессии" }));
    expect(api.executeSessionAction).toHaveBeenCalledWith(
      "c",
      { member: "pg-1", kind: "terminate", target, sessions: [{ pid: 1, backendStartUs: 10 }, { pid: 2, backendStartUs: 20 }] },
      { confirm: null, typedName: null },
    );
    expect(await screen.findByText("Завершена")).toBeInTheDocument();
    expect(screen.getByText("Уже не подходит под условие")).toBeInTheDocument();
  });

  it("пакет на prod: имя подключения и число сессий", async () => {
    const user = userEvent.setup();
    renderWithClient(<SessionActionDialog connectionId="c" profile={prod} member="pg-1" kind="terminate" target={target} onClose={() => {}} />);
    const button = await screen.findByRole("button", { name: "Завершить 2 сессии" });
    expect(button).toBeDisabled();
    await user.type(screen.getByLabelText("Имя подключения"), "billing-prod");
    await user.type(screen.getByLabelText("Подтверждение: число сессий"), "2");
    await user.click(button);
    expect(vi.mocked(api.executeSessionAction).mock.calls[0][2]).toEqual({ confirm: "billing-prod", typedName: "2" });
  });

  it("журнал не записан — об этом сказано", async () => {
    const user = userEvent.setup();
    vi.mocked(api.executeSessionAction).mockResolvedValue({ member: "pg-1", results: [], outcome: "ok", journalWritten: false });
    renderWithClient(<SessionActionDialog connectionId="c" profile={dev} member="pg-1" kind="cancel"
      target={{ kind: "selected", sessions: [{ pid: 1, backendStartUs: 10 }] }} onClose={() => {}} />);
    await user.click(await screen.findByRole("button", { name: /Отменить запрос/ }));
    expect(await screen.findByText("Журнал не записан")).toBeInTheDocument();
  });
});
```

Run: `pnpm vitest run src/components/actions`
Expected: FAIL.

- [ ] **Step 2: Диалог**

`src/components/actions/SessionActionDialog.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionProfile, DenyReason, SessionResult, SessionTarget, SignalKind } from "../../api/types";
import { formatDuration } from "../../lib/format";
import { ConfirmDialog } from "../common/ConfirmDialog";
import { ErrorBanner } from "../common/ErrorBanner";

const REASON_KEYS: Record<DenyReason, string> = {
  superuserSession: "actions.sessions.reason.superuserSession",
  otherRole: "actions.sessions.reason.otherRole",
  notClientBackend: "actions.sessions.reason.notClientBackend",
  ownConnection: "actions.sessions.reason.ownConnection",
};
const RESULT_KEYS: Record<SessionResult["result"], string> = {
  terminated: "actions.sessions.result.terminated",
  signalSent: "actions.sessions.result.signalSent",
  gone: "actions.sessions.result.gone",
  noLongerMatches: "actions.sessions.result.noLongerMatches",
  notAllowed: "actions.sessions.result.notAllowed",
  failed: "actions.sessions.result.failed",
};

interface Props {
  connectionId: string;
  profile: ConnectionProfile;
  member: string;
  kind: SignalKind;
  target: SessionTarget;
  onClose: () => void;
}

/**
 * Отмена запроса или завершение сессий (спец. P2 §8.3): предпросмотр заново читает сессии цели с правами,
 * выполнение получает только разрешённые; пакет на prod подтверждается числом сессий.
 */
export function SessionActionDialog({ connectionId, profile, member, kind, target, onClose }: Props) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const preview = useQuery({
    queryKey: ["actionPreview", connectionId, member, kind, target],
    queryFn: () => api.previewSessionAction(connectionId, { member, kind, target }),
    staleTime: Infinity,
    gcTime: 0,
  });
  const allowed = preview.data?.rows.filter((r) => r.allowed) ?? [];
  const batch = allowed.length > 1 || target.kind === "idleInTransaction";
  const run = useMutation({
    mutationFn: (c: { confirm: string | null; typedName: string | null }) =>
      api.executeSessionAction(
        connectionId,
        { member: preview.data?.member ?? member, kind, target, sessions: allowed.map((r) => ({ pid: r.session.pid, backendStartUs: r.session.backendStartUs })) },
        c,
      ),
    onSuccess: () => void qc.invalidateQueries({ queryKey: ["diag", connectionId] }),
  });
  const title = kind === "cancel" ? t("actions.sessions.cancelTitle") : t("actions.sessions.terminateTitle");
  const label =
    kind === "cancel" ? t("actions.sessions.cancel", { count: allowed.length }) : t("actions.sessions.terminate", { count: allowed.length });
  return (
    <section className="panel" aria-label={title}>
      {preview.error && <ErrorBanner error={preview.error} />}
      {preview.data && !run.data && (
        <>
          <table className="diag-table" aria-label={kind === "cancel" ? t("actions.sessions.tableCancel") : t("actions.sessions.tableTerminate")}>
            <thead>
              <tr>
                <th scope="col">PID</th>
                <th scope="col">{t("diag.sessions.columns.user")}</th>
                <th scope="col">{t("diag.sessions.columns.application")}</th>
                <th scope="col">{t("diag.sessions.columns.state")}</th>
                <th scope="col">{t("diag.sessions.columns.xactAge")}</th>
                <th scope="col">{t("diag.sessions.columns.text")}</th>
              </tr>
            </thead>
            <tbody>
              {preview.data.rows.map((r) => (
                <tr key={`${r.session.pid}:${r.session.backendStartUs}`} className={r.allowed ? undefined : "muted"}>
                  <td>{r.session.pid}</td>
                  <td>{r.session.user ?? ""}</td>
                  <td>{r.session.application ?? ""}</td>
                  <td>{r.allowed || !r.reason ? (r.session.state ?? "") : t(REASON_KEYS[r.reason])}</td>
                  <td>{formatDuration(r.session.xactAgeS)}</td>
                  <td title={r.session.query ?? undefined}>{(r.session.query ?? "").slice(0, 120)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {allowed.length === 0 ? (
            <p className="muted">{t("actions.sessions.nothing")}</p>
          ) : (
            <ConfirmDialog
              connection={profile}
              title={title}
              confirmLabel={label}
              danger={kind === "terminate"}
              busy={run.isPending}
              objectName={batch && profile.environment === "prod" ? String(allowed.length) : undefined}
              objectLabel={t("actions.sessions.countLabel")}
              onConfirm={(c) => run.mutate(c)}
              onCancel={onClose}
            />
          )}
        </>
      )}
      {run.error && <ErrorBanner error={run.error} />}
      {run.data && (
        <>
          <ul className="diag-facts">
            {run.data.results.map((r) => (
              <li key={`${r.pid}:${r.backendStartUs}`}>
                {r.pid}: {t(RESULT_KEYS[r.result])}
                {r.result === "failed" && ` — ${r.error.message}`}
              </li>
            ))}
          </ul>
          {!run.data.journalWritten && <p className="diag-warn">{t("actions.journalNotWritten")}</p>}
          <button type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </>
      )}
    </section>
  );
}
```

(Ошибка выполнения показывается `ErrorBanner` с переводом по ключу: `READ_ONLY`, `CONFIRMATION_REQUIRED` — тексты `errors.guard.*`.)

Словари `actions.sessions` (en / ru):

| ключ | en | ru |
|---|---|---|
| `cancelTitle` / `terminateTitle` | Cancel the query? / Terminate sessions? | Отменить запрос? / Завершить сессии? |
| `cancel_one` / `_other` | Cancel the query / Cancel {{count}} queries | ru: `_one` Отменить запрос, `_few` Отменить {{count}} запроса, `_many` Отменить {{count}} запросов |
| `terminate_one` / `_other` | Terminate the session / Terminate {{count}} sessions | ru: `_one` Завершить сессию, `_few` Завершить {{count}} сессии, `_many` Завершить {{count}} сессий |
| `tableCancel` / `tableTerminate` | Queries to cancel / Sessions to terminate | Запросы для отмены / Сессии для завершения |
| `nothing` | No session can be signalled | Ни одну сессию нельзя отменить или завершить |
| `countLabel` | number of sessions | число сессий |
| `reason.superuserSession` | Superuser session: needs the superuser role | Сессия суперпользователя: нужна роль суперпользователя |
| `reason.otherRole` | Another role: needs pg_signal_backend | Чужая роль: нужна роль pg_signal_backend |
| `reason.notClientBackend` | Not a client session | Не клиентская сессия |
| `reason.ownConnection` | The app's own connection | Соединение самого приложения |
| `result.terminated` / `signalSent` / `gone` | Terminated / Signal sent / Already gone | Завершена / Сигнал отправлен / Сессии уже нет |
| `result.noLongerMatches` / `notAllowed` / `failed` | No longer matches / Not allowed / Failed | Уже не подходит под условие / Нет прав / Ошибка |
| `actions.journalNotWritten` | The journal was not written | Журнал не записан |
| `buttons.cancel` / `buttons.terminate` | Cancel query… / Terminate session… | Отменить запрос… / Завершить сессию… |
| `buttons.terminateIdle` | Terminate idle in transaction older than {{min}} min… | Завершить idle in transaction старше {{min}} мин… |
| `buttons.terminateRoot` | Terminate the root blocker… | Завершить корневой блокирующий… |
| `findings.terminateIdle` | Terminate idle in transaction older than {{min}} min | Завершить idle in transaction старше {{min}} мин |
| `findings.terminateRoot` | Terminate session {{pid}} | Завершить сессию {{pid}} |

- [ ] **Step 3: Кнопки в инструментах и находках**

- `DiagTab.tsx`: `const profile = useConnectionProfile(connectionId)`; состояние `const [action, setAction] = useState<{ kind: SignalKind; target: SessionTarget } | null>(null)`; `ToolBody` получает `profile` и `onSessionAction={canAct(profile) && report.member ? setAction : undefined}`; `FindingsPanel` — `onAction={canAct(profile) && report.member ? (l) => setAction(linkAction(l.action)) : undefined}`, где

  ```ts
  /** Действие по кнопке находки: «idle in transaction старше порога» — пакет, «корневой блокирующий» — одна сессия. */
  function linkAction(a: SessionActionLink): { kind: SignalKind; target: SessionTarget } {
    return a.type === "terminateIdleInTransaction"
      ? { kind: "terminate", target: { kind: "idleInTransaction", olderThanS: a.olderThanS } }
      : { kind: "terminate", target: { kind: "rootBlocker", pid: a.pid, backendStartUs: a.backendStartUs } };
  }
  ```

  Диалог рисуется над телом вкладки: `{action && profile && report?.member && <SessionActionDialog key={JSON.stringify(action)} connectionId={connectionId} profile={profile} member={report.member} {...action} onClose={() => setAction(null)} />}`.
- `FindingsPanel.tsx`: для ссылок `kind === "action"` — кнопка, если передан `onAction`; текст — `actions.findings.terminateIdle` (`min: Math.round(olderThanS / 60)`) или `actions.findings.terminateRoot` (`pid`). Тест: без `onAction` кнопки действия нет; с ним клик вызывает `onAction` со ссылкой.
- `SessionsView.tsx`: пропсы `onSessionAction?`. В `SessionPanel` при `onSessionAction` и `row.backendStartUs !== null` у клиентской сессии (`row.backendType === "client backend"`) — кнопки `actions.buttons.cancel` и `actions.buttons.terminate` → `onSessionAction({ kind, target: { kind: "selected", sessions: [{ pid: row.pid, backendStartUs: row.backendStartUs }] } })`. В наборе `idleTx` на панели инструментов — поле «старше, мин» (`input type="number"`, по умолчанию 5, минимум 1) и кнопка `actions.buttons.terminateIdle` → `onSessionAction({ kind: "terminate", target: { kind: "idleInTransaction", olderThanS: min * 60 } })`. Тесты: кнопки есть с `onSessionAction` и нет без него; клик передаёт цель.
- `LocksView.tsx`: пропсы `onSessionAction?`. У корня дерева без `lock` (сам ничего не ждёт) и с `session.backendStartUs` — кнопка `actions.buttons.terminateRoot` → `onSessionAction({ kind: "terminate", target: { kind: "rootBlocker", pid, backendStartUs } })`. Ждущему корню (`lock` есть) кнопки нет. Тест.

- [ ] **Step 4: Проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat(ui): отмена запроса и завершение сессий — из панели сессии, набора idle in transaction, дерева блокировок и находок"
```

---

### Task 12: Фронтенд — удаление слота

**Files:**
- Create: `src/components/actions/DropSlotDialog.tsx`, `src/components/actions/DropSlotDialog.test.tsx`
- Modify: `crates/pg-advisor/src/rules/slots.rs` (`SlotsData.can_drop_slots`), `src-tauri/src/diag/slots.rs` (право в сборе), `src/api/types.ts` (`SlotsData.canDropSlots`), `src/test/fixtures.ts`, `src/components/diag/SlotsView.tsx` (+ тест), `src/components/diag/DiagTab.tsx` (передача профиля), `src/locales/{en,ru}.json`
- Test: `DropSlotDialog.test.tsx`, `SlotsView.test.tsx`, модульный тест `diag/slots.rs`

**Interfaces:**
- Consumes: `api.previewDropSlot`, `api.dropSlot`, `ConfirmDialog`
- Produces: `DropSlotDialog({ connectionId, profile, member, slot, onClose })`; `SlotsData.canDropSlots: boolean` (суперпользователь или `REPLICATION`)

- [ ] **Step 1: Право в данных слотов**

`crates/pg-advisor/src/rules/slots.rs`, в `SlotsData`: `#[serde(default)] pub can_drop_slots: bool,` (комментарий: «Роль подключения может удалять слоты: суперпользователь или `REPLICATION` (спец. §8.7)»). `src-tauri/src/diag/slots.rs`, в `collect` — ещё один запрос:

```rust
pub const CAN_DROP_SQL: &str =
    "SELECT rolsuper OR rolreplication FROM pg_roles WHERE rolname = current_user";
```

`data.can_drop_slots = c.query_one(CAN_DROP_SQL, &[]).await.map(|r| r.get(0)).unwrap_or(false);` — ошибка этого запроса не роняет отчёт. Модульный тест: `CAN_DROP_SQL` содержит `rolreplication`; `it_diag` (слоты на матрице) — у `glafira_diag` `canDropSlots == false`.

- [ ] **Step 2: Падающие тесты**

`src/components/actions/DropSlotDialog.test.tsx` — по образцу диалога сессий (`vi.mock` команд `previewDropSlot`, `dropSlot`):
1. Предпросмотр показывает тип, БД, `wal_status` и «Удерживает WAL»; кнопка «Удалить слот» неактивна, пока не введено имя слота; после ввода `it_dead_slot` вызывает `dropSlot("c", { member: "pg-1", slot: "it_dead_slot" }, { confirm: null, typedName: "it_dead_slot" })`; затем текст «Слот удалён».
2. Слот участника Patroni (`slot.patroni = "member"`) — предупреждение «Patroni пересоздаст слот, пока участник есть в кластере; удалите участника»; постоянный (`"permanent"`) — «Уберите слот из /config (patronictl edit-config), иначе Patroni его пересоздаст»; кнопка при этом доступна.
3. На prod кроме имени слота нужно имя подключения.

`SlotsView.test.tsx`: с `onDropSlot` у неактивного слота есть кнопка «Удалить…», у активного — неактивная кнопка с подсказкой «Слот активен»; при `canDropSlots: false` кнопки неактивны с подсказкой «Нужен атрибут REPLICATION — пункт чек-листа «Удаление слотов репликации»»; без `onDropSlot` (read-only) кнопок нет.

Run: `pnpm vitest run src/components/actions src/components/diag/SlotsView.test.tsx`
Expected: FAIL.

- [ ] **Step 3: Диалог и кнопки**

`src/components/actions/DropSlotDialog.tsx` — устройство как у `SessionActionDialog`: `useQuery` предпросмотра (`["dropSlotPreview", connectionId, member, slot]`, `gcTime: 0`), блок фактов (`dl.kv`: тип и plugin, БД, состояние, `wal_status`, удерживаемый WAL через `bytesText`, неактивен с — `formatTimestamp(slot.inactiveSinceMs)` при наличии), предупреждение Patroni по `slot.patroni`, `ConfirmDialog` с `objectName={slot.name}`, `objectLabel={t("actions.slots.nameLabel")}`, `danger`; `useMutation` → `api.dropSlot`; успех — «Слот удалён» и «Журнал не записан» при `journalWritten: false`, `invalidateQueries(["diag", connectionId])`; ошибка — `ErrorBanner` (`slotActiveOrGone`, `replicationRequired`).

`SlotsView.tsx`: пропсы `onDropSlot?: (slot: string) => void`; колонка действий в таблице слотов: кнопка `actions.slots.drop` (`disabled={s.active || !data.canDropSlots}`, `title` — `actions.slots.active` или `actions.slots.needReplication`). `DiagTab`: состояние `dropSlot: string | null`; `onDropSlot={canAct(profile) && report.member ? setDropSlot : undefined}`; диалог над телом вкладки.

Словари `actions.slots`: `drop` — Drop… / Удалить…; `dropTitle` — Drop slot {{slot}}? / Удалить слот {{slot}}?; `confirm` — Drop slot / Удалить слот; `nameLabel` — slot name / имя слота; `done` — Slot dropped / Слот удалён; `active` — The slot is active / Слот активен; `needReplication` — Needs the REPLICATION attribute: see the readiness item “Dropping replication slots” / Нужен атрибут REPLICATION — пункт чек-листа «Удаление слотов репликации»; `patroniMember` — Patroni will recreate the slot while the member is in the cluster: remove the member / Patroni пересоздаст слот, пока участник есть в кластере; удалите участника; `patroniPermanent` — Remove the slot from /config (patronictl edit-config), or Patroni will recreate it / Уберите слот из /config (patronictl edit-config), иначе Patroni его пересоздаст.

- [ ] **Step 4: Проверки**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 GPG_IT_PRIMARY=localhost:5451 cargo test --manifest-path src-tauri/Cargo.toml --test it_diag -- --test-threads=1 && pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates src-tauri/src src-tauri/tests src
git commit -m "feat(ui): удаление слота с вводом имени и предупреждением о слотах Patroni; право REPLICATION во вкладке слотов"
```

---

### Task 13: Фронтенд — действия Patroni и панель наблюдения

**Files:**
- Create: `src/components/actions/PatroniActionDialog.tsx`, `src/components/actions/PatroniActionDialog.test.tsx`, `src/components/actions/WatchPanel.tsx`, `src/components/actions/WatchPanel.test.tsx`
- Modify: `src/components/overview/Overview.tsx` (+ тест), `src/locales/{en,ru}.json`
- Test: `PatroniActionDialog.test.tsx`, `WatchPanel.test.tsx`, `Overview.test.tsx`

**Interfaces:**
- Consumes: `api.previewPatroniAction`, `api.executePatroniAction`, `api.stopSession`, `ConfirmDialog`, `toRfc3339`, `useConnectionProfile`, `canAct`
- Produces: `PatroniActionDialog({ connectionId, profile, action, onClose })` (внутри — `WatchPanel` после выполнения с наблюдением); `WatchPanel({ action: PatroniAction["kind"], onlyPending?: boolean, events: WatchEvent[], watchId, onClose })`; `WATCH_STEPS(kind, onlyPending) -> WatchStep[]`

- [ ] **Step 1: Падающие тесты**

`WatchPanel.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { api } from "../../api/client";
import { WATCH_STEPS, WatchPanel } from "./WatchPanel";

vi.mock("../../api/client", () => ({ api: { stopSession: vi.fn().mockResolvedValue(true) } }));

describe("WatchPanel", () => {
  it("шаги switchover: пройденные, текущий, оставшиеся", () => {
    render(
      <WatchPanel action="switchover" events={[
        { step: "accepted", status: "running", atMs: 1, detail: null },
        { step: "newLeaderRunning", status: "running", atMs: 2, detail: null },
      ]} watchId="w" onClose={() => {}} />,
    );
    expect(WATCH_STEPS("switchover", false)).toEqual(["accepted", "leaderStepped", "newLeaderRunning", "timelineIncreased", "replicasStreaming", "done"]);
    const items = screen.getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("✓");
    expect(items[2]).toHaveTextContent("Новый лидер работает");
    expect(items[2]).toHaveTextContent("✓");
    expect(items[3]).not.toHaveTextContent("✓");
  });

  it("срок вышел — сказано; закрытие останавливает наблюдение", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<WatchPanel action="pause" events={[{ step: "accepted", status: "timedOut", atMs: 1, detail: null }]} watchId="w" onClose={onClose} />);
    expect(screen.getByText(/Срок наблюдения вышел/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Закрыть" }));
    expect(api.stopSession).toHaveBeenCalledWith("w");
    expect(onClose).toHaveBeenCalled();
  });
});
```

`PatroniActionDialog.test.tsx` (`vi.mock` `previewPatroniAction`, `executePatroniAction`):
1. Switchover: предпросмотр с двумя кандидатами (`ready: true` и `ready: false` с причиной); выбор «Patroni выберет сам» по умолчанию; неготовый кандидат в списке недоступен с причиной; видны «Оборвётся клиентских сессий лидера: 12», «Запись будет недоступна несколько секунд», «Старый лидер станет репликой»; режим «По расписанию» с `datetime-local` уходит в `scheduledAt` через `toRfc3339`; «Выполнить» вызывает `executePatroniAction("c", { kind: "switchover", candidate: null, scheduledAt: null }, { confirm: null, typedName: null }, expect.any(Function))`; после ответа с `watch: "w-1"` — панель наблюдения.
2. Failover: кнопка неактивна, пока не введено имя кандидата (`objectName` = кандидат).
3. `canSend: false` — «Укажите в подключении логин API Patroni», кнопки выполнения нет.
4. Restart лидера: «Перезапуск лидера — запись недоступна до конца перезапуска»; флажок «Только если нужен перезапуск» включён при `pendingRestart: true`.

`Overview.test.tsx`: у подключения `patroni` (не read-only) — кнопки «Switchover…», «Failover…», «Пауза» (или «Снять паузу» при `paused`), у строки участника — «Restart…»; у баннера запланированного switchover — «Отменить…»; у read-only — ни одной; у не-Patroni — ни одной.

Run: `pnpm vitest run src/components/actions src/components/overview`
Expected: FAIL.

- [ ] **Step 2: Панель наблюдения**

`src/components/actions/WatchPanel.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { PatroniAction, WatchEvent, WatchStep } from "../../api/types";

/** Шаги наблюдения по действию — как `WatchPlan::steps` в Rust (спец. §8.6). */
export function WATCH_STEPS(kind: PatroniAction["kind"], onlyPending: boolean): WatchStep[] {
  switch (kind) {
    case "switchover":
    case "failover":
      return ["accepted", "leaderStepped", "newLeaderRunning", "timelineIncreased", "replicasStreaming", "done"];
    case "restart":
      return onlyPending ? ["accepted", "restarted", "memberRunning", "pendingCleared", "done"] : ["accepted", "restarted", "memberRunning", "done"];
    default:
      return ["accepted", "pauseApplied", "done"];
  }
}

const STEP_KEYS: Record<WatchStep, string> = {
  accepted: "actions.watch.step.accepted",
  leaderStepped: "actions.watch.step.leaderStepped",
  newLeaderRunning: "actions.watch.step.newLeaderRunning",
  timelineIncreased: "actions.watch.step.timelineIncreased",
  replicasStreaming: "actions.watch.step.replicasStreaming",
  restarted: "actions.watch.step.restarted",
  memberRunning: "actions.watch.step.memberRunning",
  pendingCleared: "actions.watch.step.pendingCleared",
  pauseApplied: "actions.watch.step.pauseApplied",
  done: "actions.watch.step.done",
};

interface Props {
  action: PatroniAction["kind"];
  onlyPending?: boolean;
  events: WatchEvent[];
  watchId: string;
  onClose: () => void;
}

/** Ход действия Patroni: шаги до «готово», срок; закрытие останавливает наблюдение и ускоренный опрос. */
export function WatchPanel({ action, onlyPending = false, events, watchId, onClose }: Props) {
  const { t } = useTranslation();
  const steps = WATCH_STEPS(action, onlyPending);
  const last = events.at(-1);
  const reached = last ? steps.indexOf(last.step) : -1;
  // `stop_session` безопасен и для закончившейся сессии (вернёт false): останавливаем всегда.
  const close = () => {
    void api.stopSession(watchId);
    onClose();
  };
  return (
    <section className="panel" aria-label={t("actions.watch.title")}>
      <h3>{t("actions.watch.title")}</h3>
      <ol className="diag-facts">
        {steps.map((s, i) => (
          <li key={s} aria-current={i === reached + 1 && last?.status === "running" ? "step" : undefined}>
            <span aria-hidden="true">{i <= reached ? "✓ " : "· "}</span>
            {t(STEP_KEYS[s])}
          </li>
        ))}
      </ol>
      {last?.status === "timedOut" && <p className="diag-warn">{t("actions.watch.timedOut")}</p>}
      {last?.status === "failed" && <p className="diag-warn">{last.detail ?? t("actions.watch.failed")}</p>}
      <button type="button" onClick={close}>
        {t("common.close")}
      </button>
    </section>
  );
}
```

- [ ] **Step 3: Диалог действий Patroni**

`src/components/actions/PatroniActionDialog.tsx` — одна форма на все виды:
- `useQuery` предпросмотра по `["patroniPreview", connectionId, action.kind, member]` (`gcTime: 0`);
- состояние формы: `candidate: string | null` (switchover, по умолчанию `null` — «Patroni выберет сам»; failover — первый готовый кандидат), `when: "now" | "scheduled"` и `local` (`datetime-local`) для switchover и restart, `onlyPending` для restart (по умолчанию `preview.member.pendingRestart`);
- `actionToSend(): PatroniAction` собирает итог (`scheduledAt: when === "scheduled" ? toRfc3339(local) : null`);
- пояснения по виду: switchover / failover — `leaderClientSessions` («Оборвётся клиентских сессий лидера: N»), «Запись будет недоступна несколько секунд», «Старый лидер станет репликой»; restart — роль участника, при лидере — «Перезапуск лидера — запись недоступна до конца перезапуска»; cancel* — что именно запланировано (`scheduledSwitchover.at` / `member.scheduledRestart.at`, время через `scheduledAt` из `Overview.tsx` — вынести её в `src/lib/format.ts` как `formatScheduled(at)` и использовать в обоих местах); pause — «Patroni перестанет управлять кластером: ни failover, ни перезапусков, пока пауза не снята»;
- `canSend === false` — `actions.patroni.authRequired`, без `ConfirmDialog`;
- `ConfirmDialog`: `objectName` — кандидат у failover (`objectLabel` «имя кандидата»), иначе нет; `danger` у failover и restart лидера;
- `useMutation` → `api.executePatroniAction(connectionId, actionToSend(), c, (e) => setEvents((x) => [...x, e]))`; успех с `watch` — `WatchPanel` (`action.kind`, `onlyPending`, `events`, `watchId`); без `watch` — текст ответа Patroni (`response`) и «Журнал не записан» при `journalWritten: false`; `invalidateQueries(["topology", connectionId])`.

- [ ] **Step 4: Кнопки в обзоре**

`src/components/overview/Overview.tsx`: `const profile = useConnectionProfile(connectionId)`; при `topo.kind === "patroni" && canAct(profile)`:
- в панели инструментов — «Switchover…», «Failover…», «Пауза» / «Снять паузу» (по `topo.paused`); кнопки неактивны с подсказкой `actions.patroni.authRequired`, если `!profile.patroni?.username`;
- у баннера запланированного switchover — «Отменить…» (`{ kind: "cancelSwitchover" }`);
- в строке участника — «Restart…» (`{ kind: "restart", member, onlyPending: m.pendingRestart, scheduledAt: null }`) и при `m.scheduledRestart` — «Отменить restart…»;
- состояние `const [patroniAction, setPatroniAction] = useState<PatroniAction | null>(null)`; диалог — над таблицей участников.

Словари `actions.patroni` и `actions.watch` (en / ru):

| ключ | en | ru |
|---|---|---|
| `patroni.switchover` / `failover` / `pause` / `resume` | Switchover… / Failover… / Pause / Resume | Switchover… / Failover… / Пауза / Снять паузу |
| `patroni.restart` / `cancel` / `cancelRestart` | Restart… / Cancel… / Cancel restart… | Restart… / Отменить… / Отменить restart… |
| `patroni.candidate` / `candidateAuto` / `candidateLabel` | Candidate / Let Patroni choose / candidate name | Кандидат / Patroni выберет сам / имя кандидата |
| `patroni.when` / `now` / `scheduled` | When / Now / Scheduled | Когда / Сейчас / По расписанию |
| `patroni.onlyPending` | Only if a restart is pending | Только если нужен перезапуск |
| `patroni.sessions` | Client sessions on the leader that will break: {{count}} | Оборвётся клиентских сессий лидера: {{count}} |
| `patroni.writeDown` | Writes will be unavailable for a few seconds | Запись будет недоступна несколько секунд |
| `patroni.oldLeader` | The old leader will become a replica | Старый лидер станет репликой |
| `patroni.leaderRestart` | Restarting the leader: writes are unavailable until it is back | Перезапуск лидера — запись недоступна до конца перезапуска |
| `patroni.pauseNote` | Patroni stops managing the cluster: no failover and no restarts until resumed | Patroni перестанет управлять кластером: ни failover, ни перезапусков, пока пауза не снята |
| `patroni.authRequired` | Set the Patroni API login in the connection to run actions | Укажите в подключении логин API Patroni, чтобы выполнять действия |
| `patroni.run` / `title` | Run / Patroni: {{action}} | Выполнить / Patroni: {{action}} |
| `patroni.response` | Patroni answered: {{text}} | Ответ Patroni: {{text}} |
| `watch.title` | Progress | Ход действия |
| `watch.step.accepted` / `leaderStepped` / `newLeaderRunning` | Accepted / The old leader stepped down / The new leader is running | Принято / Старый лидер сложил полномочия / Новый лидер работает |
| `watch.step.timelineIncreased` / `replicasStreaming` | The timeline increased / Replicas are streaming | Таймлайн вырос / Реплики получают WAL |
| `watch.step.restarted` / `memberRunning` / `pendingCleared` | Restarted / The node is running / Pending restart cleared | Перезапущен / Узел работает / Отметка «нужен перезапуск» снята |
| `watch.step.pauseApplied` / `done` | The cluster state changed / Done | Состояние кластера изменилось / Готово |
| `watch.timedOut` / `failed` | The watch timed out: check the overview / The action failed | Срок наблюдения вышел — смотрите обзор / Действие не удалось |

- [ ] **Step 5: Проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src
git commit -m "feat(ui): действия Patroni в обзоре кластера — предпросмотр, подтверждение, расписание и панель наблюдения"
```

---

### Task 14: Руководство, admin.md, README, CHANGELOG, спецификация

**Files:**
- Modify: `docs/guide/{en,ru}/diagnostics.md`, `docs/admin.md`, `README.md`, `CHANGELOG.md`, `docs/i18n/glossary.md`, `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md`, `src/locales/{en,ru}.json` (`hints`, если нужны), тест якорей глав (`src/guide/…` — тот, что проверяет `helpAnchorFor` для инструментов)
- Test: `pnpm test:scripts`, тест якорей

- [ ] **Step 1: Главы руководства**

`docs/guide/ru/diagnostics.md` и `docs/guide/en/diagnostics.md` (правила `docs/i18n/README.md`, раздел «Руководство»: разделы `## … {#id}`, метки `[[ui:…]]`, якоря одинаковые) — три новых раздела перед `{#findings}`:

- `## Действия {#actions}` — что можно сделать и где: отмена запроса и завершение сессии (панель сессии), пакет idle in transaction старше N минут, корневой блокирующий (дерево блокировок и кнопка находки), удаление слота (вкладка «Слоты»); предпросмотр — заново читает данные и показывает, что можно и почему нельзя (права `pg_signal_backend`, сессии суперпользователя, своё соединение); защита: на read-only кнопок нет, на prod — имя подключения, у пакета на prod — число сессий, у слота — его имя; перепроверка: сессия адресуется PID и временем запуска (переиспользованный PID не пострадает), пакет перепроверяет условие (проснувшаяся сессия — «Уже не подходит под условие»); итоги сессий; права (ссылка на admin.md).
- `## Действия Patroni {#patroni-actions}` — в обзоре кластера: switchover сейчас и по расписанию (кандидат или «Patroni выберет сам», готовность кандидатов, сессии лидера оборвутся), отмена расписания, failover (только без здорового лидера, ввод имени кандидата), restart сейчас и по расписанию (только если нужен перезапуск), отмена, пауза и снятие; логин API в подключении; ход действия — шаги до «готово», опрос топологии раз в 2 секунды, сроки 2 / 5 мин / 30 с, закрытие панели прекращает наблюдение.
- `## Журнал действий {#journal}` — вкладка в группе «Диагностика»; файл `actions.jsonl` в каталоге данных (права только владельцу, 5 МБ и одна старая копия); что записано (подключение, узел, подтверждение prod, итог, сессии с текстом запроса до 500 символов — литералы остаются, слот, запрос Patroni без учётных данных); отказы защиты не пишутся; «Журнал не записан».

Тест якорей глав: `actions` → `journal`, якоря `actions`, `patroni-actions`, `journal` есть в обеих главах.

- [ ] **Step 2: admin.md, README, CHANGELOG, глоссарий**

- `docs/admin.md`, раздел «Диагностика» → новый подраздел «Действия»: права (`pg_signal_backend` — отмена и завершение сессий не суперпользователя; суперпользователь — любых; атрибут `REPLICATION` — удаление слотов, с риском: логические слоты читают изменения всех таблиц, при строке `replication` в `pg_hba.conf` — копия кластера; совет — отдельная роль для действий или не выдавать), учётная запись API Patroni (логин в профиле, пароль в хранилище секретов; `POST /switchover`, `POST /failover`, `POST|DELETE /restart`, `DELETE /switchover`, `PATCH /config`), соединение действия (отдельное, без read-only, `statement_timeout` 5 с), защита read-only и prod, файл `actions.jsonl` (где лежит на каждой ОС, права 0600, ротация, текст запросов с литералами, не входит в архив диагностики).
- `README.md`: строка о действиях в описании возможностей; в разделе тестов — `it_actions`, `it_patroni_actions`, переменные `GPG_IT_DOCKER` и `GPG_IT_FAILOVER`.
- `CHANGELOG.md`, `## Unreleased`: действия дежурного (отмена и завершение сессий поштучно и пакетом, удаление слота, действия Patroni), защита read-only и prod, наблюдение после переключения, журнал действий, пункт чек-листа «Удаление слотов репликации».
- `docs/i18n/glossary.md`: switchover (не переводится), failover (не переводится), корневой блокирующий (уже есть), журнал действий (actions journal), ход действия (progress), предпросмотр (preview), пакет (batch).

- [ ] **Step 3: Спецификация**

`docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md` — привести в соответствие с построенным (уточнения плана 1–10 и решения исполнения): §8.1 — пакет в интерфейсе по критерию, находки ведут к действиям; §8.2 — `ConnectSpec.options`, `ConnectSpec::for_action`; §8.3 — итоги и предел пакета как построено; §8.4 — `canDropSlots` во вкладке слотов, кнопка у активного слота неактивна; §8.5 — 401 → `errors.patroni.unauthorized`, `PatroniHttp::send_json` с реализацией по умолчанию; §8.6 — `Runtimes::boost/unboost` с `Notify`, `postmaster_start_time` до restart; §8.8 — журнал в каталоге данных (`AppState.actions`); §9 — сигнатуры команд как построены; §4 — `FindingLink::Action` с `SessionActionLink`. Статус спецификации — без изменений.

- [ ] **Step 4: Проверки**

Run: `pnpm test:scripts && pnpm i18n:check && pnpm test && pnpm exec tsc --noEmit && pnpm build`
Expected: PASS; глава открывается по F1 на вкладке «Журнал действий» на разделе «Журнал действий» (проверка в `pnpm tauri dev`).

- [ ] **Step 5: Commit**

```bash
git add docs src README.md CHANGELOG.md
git commit -m "docs: действия, действия Patroni и журнал в руководстве; права и actions.jsonl в admin.md; спецификация P2 по построенному"
```

---

## Самопроверка плана

- **Покрытие части P2b спецификации:**
  - §1 п. 5 (действия): отмена и завершение поштучно и пакетом — задачи 3, 4, 11; удаление слота — 5, 12; switchover сейчас и по расписанию, отмена — 6, 7, 13; failover — 6, 7, 8, 13; restart сейчас и по расписанию, отмена — 6, 7, 8, 13; pause / resume — 6, 8, 13.
  - §1 п. 6 (защита): read-only и prod — 1 (guard), проверка в каждой команде — 4, 5, 7; предпросмотр — 3, 5, 7; защита от переиспользования PID — 3, 4 (`BACKEND_START_US`); наблюдение — 8, 13; предпроверка прав — 3 (права в предпросмотре), 5 и 12 (`REPLICATION`, `canDropSlots`), 7 и 13 (`canSend`); журнал и вкладка — 2, 4, 5, 7, 10.
  - §8.1 — 1, матрица подтверждений — 4 (пакет на prod), 5 (имя слота), 7 (кандидат failover), 11–13 (`ConfirmDialog`); §8.2 — 1; §8.3 — 3, 4; §8.4 — 5; §8.5 — 6, 7; §8.6 — 8; §8.7 — 3, 5, 12, 13; §8.8 — 2; §9 (IPC P2b) — 2–8; §10 (`READ_ONLY`, `CONFIRMATION_REQUIRED`, `NOT_AUTHORIZED`, `PATRONI_ERROR`, `INVALID_INPUT`, `NODE_UNAVAILABLE`) — 1, 4, 5, 6, 7.
  - §12.2 P2b: корневой блокирующий освобождает очередь, устаревший `backend_start` → `gone`, пакет пропускает проснувшуюся, отмена `pg_sleep` → 57014 — задача 4; `it_dead_slot` и `replica1` — 5; switchover сейчас до «готово», по расписанию и отмена, restart реплики, pause / resume, failover — 7, 8; read-only → `READ_ONLY`, prod без имени → `CONFIRMATION_REQUIRED` — 4, 7; строка журнала у каждого действия — 4, 5, 7; фронтенд — `ConfirmDialog` (9), диалоги и матрица подтверждений (11–13), панель наблюдения (13), вкладка журнала (10).
  - §13 (документация P2b) — 14; §15 (критерии P2b): 1 — 4, 7; 2 — 4; 3 — 5, 12; 4 — 7, 8; 5 — 2, 10; 6 — 5 (`replicationAttr`), CI — проверки каждой задачи.
- **Типы между задачами:** `Confirmation` (1) → 4, 5, 7, 8 и TS (9); `ActionJournal`, `ActionRecord`, `SessionRecord`, `SlotRecord`, `PatroniRecord`, `Outcome`, `cut`, `now_ms` (2) → 4, 5, 7 и TS `ActionRecord` (9, 10); `SessionTarget`, `SessionRef`, `SignalKind` (3) → 4 и TS (9, 11); `SessionActionLink` (3) → TS (3, 11); `Slot.patroni`, `SlotsData.can_drop_slots` (5, 12) → `DropSlotDialog`; `PatroniAction`, `request`, `send`, `bases`, `check` (6) → 7, 8; `Performed`, `postmaster_start` (7) → 8; `WatchPlan`, `WatchStep`, `WatchEvent`, `WatchSink` (8) → TS (9) и `WatchPanel` (13; список шагов совпадает с `WatchPlan::steps`).
- **Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 (Rust) → 9 → 10 → 11 → 12 → 13 (фронтенд) → 14. Задача 3 меняет JSON находок, поэтому сразу учит `FindingsPanel` пропускать ссылки `action` (кнопки — в 11).
- **Риски, названные заранее:** сценарии Patroni меняют кластер — только под `GPG_IT_DOCKER=1`, восстановление `restore_on_drop` + `wait_stand`; failover останавливает PostgreSQL лидера — только под `GPG_IT_FAILOVER=1`, пауза до остановки и снятие паузы в восстановлении; `pg_ctl` в образе Patroni может быть не в `PATH`; плюральные формы ru (`_one/_few/_many`) — по правилам `pnpm i18n:check`; `send_json` у `ReqwestHttp` повторяет правило «пароль не уходит на http при https-профиле».
