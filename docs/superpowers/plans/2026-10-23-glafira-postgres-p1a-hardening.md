# Укрепление P1a перед P1b — план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Закрыть замечания финального ревью P1a (`.superpowers/sdd/final-review.md`, `final-rereview.md`, «Minor» в `progress.md`), которые стоит сделать до SSH-туннелей P1b: инварианты секретов при импорте и сохранении, устойчивость соединений к failover, отдельный пул опроса топологии, повтор входа без переоткрытия вручную, мелочи формы, дерева и обзора, CI и стенд.

**Architecture:** Точечные правки в существующих модулях без новых подсистем. Rust: блокировка операций с секретами в `AppState`, откат применённых операций keychain при сбое, TCP keepalive и причина отказа TLS в `conn/`, общий срок `discover`, назначение пула (`PoolPurpose`) в `PoolKey`, команды `reconnect` и `connection_secrets`. Фронтенд: пункт «Переподключить», группы каталога не размонтируются, дедупликация журнала, плейсхолдер «оставить сохранённый» только при сохранённом секрете, предупреждение про TLS при `disable`, плюральные формы шагов проверки.

**Tech Stack:** Tauri 2, Rust (edition 2024), tokio-postgres 0.7 + postgres-openssl 0.5 (vendored openssl), socket2 0.6, reqwest 0.13, React 19 + TS + TanStack Query 5 + Zustand + i18next, vitest.

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`). Команды шагов выполняются из `$P`. Glafira for Kafka (`/Users/vvnovg/kafka-explorer`) — только для чтения.

## Global Constraints

- PostgreSQL 13+, Patroni 3.0+ (включая 4.x). Платформы: macOS (arm64, x64), Windows x64, Linux x64.
- Идентификаторы: `PRODUCT_NAME = "Glafira for PostgreSQL"`, `IDENTIFIER = "io.github.vvnovg.glafira.postgres"`, бинарь `glafira-pg`, `KEYCHAIN_SERVICE = "glafira-postgres"`, `PG_APPLICATION_NAME = "glafira-pg"`.
- Секреты — только в keychain, ключ `<id подключения>/<поле>`; поля: `pgPassword`, `pgKeyPassword`, `patroniPassword`, `patroniKeyPassword`. В `connections.json`, файлах экспорта и архиве диагностики секретов нет.
- Соединение с PG: `application_name = glafira-pg`, параметр запуска `options = -c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000`.
- Пул: не больше 3 соединений на ключ (подключение, участник, БД) для каталога плюс одно для опроса топологии (задача 3); простаивающее соединение закрывается через 5 мин. Опрос топологии — раз в 10 с. HTTP-запрос к Patroni — таймаут 5 с.
- Все команды Tauri возвращают `Result<T, AppError>`; `AppError { code, key, params, message, details?, retryable }` сериализуется в camelCase, `code` — SCREAMING_SNAKE_CASE.
- Пользовательские тексты из Rust — только через `ltext!` / `app_err!` с английским шаблоном, равным тексту в `src/locales/en.json` (для плюрального ключа — форме `_other`, параметр `count` обязателен); переводы — `src/locales/{en,ru}.json`; ru-текст, совпадающий с en, перечисляется в `docs/i18n/same-as-en.json`; `hints` — последний верхний ключ словарей. Кириллица в строковых литералах Rust вне `#[cfg(test)]` запрещена (`pnpm i18n:check`). Комментарии в коде — по-русски.
- Ни одна операция не блокирует UI: keychain и файлы — в `blocking`/`spawn_blocking`, у каждой сетевой операции таймаут.
- Изменяющих операций с БД и Patroni нет.
- Стенд Docker (контейнеры `gp-*`) запущен: никогда не выполнять `docker compose down`, не удалять контейнеры и тома, не трогать контейнер `ke-kafka`; интеграционные тесты гонять без `GPG_IT_DOCKER`; лидер Patroni может быть любым узлом.
- Сообщения коммитов — по-русски, в стиле `fix(conn): …`, с последней строкой `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Проверки каждой задачи: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`; у задач 2 и 3 — ещё интеграционные тесты со стендом: `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1`.

## Отложено сознательно (не в этом плане)

- Keyset-пагинация `list_objects` и счётчики в заголовках «Роли»/«Расширения» — функциональные изменения каталога, не укрепление.
- Виртуализация списков партиций; таблица обзора шире 900 px (область прокручивается).
- `prefer` + файл CA не проверяет сертификат — поведение документируется в подсказке (задача 2), к libpq подгоняется при пересмотре TLS в P1b.
- `same_except_time` сравнивает `dcs_last_seen_ms` и ошибки участников: это настоящие данные обзора, их обновление раз в опрос — цель, а не шум.
- Хвосты updater после v0.1.0 (M5-2/3/4/7/9) — отдельный небольшой этап.

---

### Task 1: Инварианты секретов: блокировка, откат keychain, честный счётчик импорта

**Files:**
- Modify: `src-tauri/src/state.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/commands/folders.rs`, `src/locales/en.json`, `src/locales/ru.json`
- Test: unit-тесты в `commands/connections.rs`, `commands/folders.rs`

**Interfaces:**
- Consumes: `AppState`, `SecretStore`, `secret_ops`, `apply_secret_ops`, `plan_import`, `ConnectionStore::update_if`.
- Produces: `AppState::secrets_lock: Arc<std::sync::Mutex<()>>`; `commands::connections::apply_secret_ops_undoable(store, id, ops) -> AppResult<()>`; `not_saved(error, reset: usize)` считает только обновляемые подключения.

- [ ] **Step 1: Падающие тесты сохранения**

В `src-tauri/src/commands/connections.rs`, модуль `tests` (рядом с `BrokenKeychain`):

```rust
    /// Keychain отказывает ровно на `fail_at`-м обращении на запись (считая с 1), остальные принимает:
    /// сбой посреди набора операций, после которого откат должен пройти.
    struct FailsOnce {
        inner: MemoryStore,
        fail_at: usize,
        calls: std::sync::atomic::AtomicUsize,
    }

    impl FailsOnce {
        fn write_slot(&self) -> AppResult<()> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == self.fail_at {
                Err(AppError::internal_with("keychain unavailable"))
            } else {
                Ok(())
            }
        }
    }

    impl SecretStore for FailsOnce {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.write_slot()?;
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.write_slot()?;
            self.inner.delete(key)
        }
    }

    /// Keychain, который проверяет, что каждое обращение на запись идёт под `AppState::secrets_lock`.
    /// `try_lock` занятого замка возвращает `WouldBlock` и для своего потока (std::sync::Mutex не реентерабелен).
    struct AssertsLocked {
        inner: MemoryStore,
        lock: std::sync::OnceLock<Arc<std::sync::Mutex<()>>>,
    }

    impl AssertsLocked {
        fn assert_locked(&self) {
            assert!(
                self.lock.get().unwrap().try_lock().is_err(),
                "обращение к keychain вне блокировки секретов"
            );
        }
    }

    impl SecretStore for AssertsLocked {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.assert_locked();
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            self.assert_locked();
            self.inner.delete(key)
        }
    }

    #[test]
    fn keychain_failure_midway_restores_the_secrets_already_changed() {
        let store = Arc::new(FailsOnce {
            inner: MemoryStore::default(),
            fail_at: 3,
            calls: std::sync::atomic::AtomicUsize::new(0),
        });
        store.inner.set(&secret_key("a", PG_PASSWORD), "old").unwrap();
        let (_d, st) = state_with(store.clone());
        // Профиль Patroni использует пароль PG и пароль API; два поля ключей не используются и удаляются.
        let mut p = sample_profile("a");
        p.kind = ConnectionKind::Patroni;
        p.hosts.clear();
        p.patroni = Some(PatroniSettings {
            urls: vec!["http://h:8008".into()],
            username: Some("u".into()),
            tls: None,
            host_overrides: vec![],
        });
        // Операции идут в порядке CONNECTION_SECRET_FIELDS: set pgPassword (1), delete pgKeyPassword (2)
        // приняты, set patroniPassword (3) — отказ.
        let err = save_connection_blocking(
            &st,
            input(
                p,
                SecretsInput {
                    pg_password: Some("new".into()),
                    patroni_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(
            store.inner.get(&secret_key("a", PG_PASSWORD)).unwrap().as_deref(),
            Some("old"),
            "применённая запись возвращена к прежнему значению"
        );
        assert!(store.inner.get(&secret_key("a", PATRONI_PASSWORD)).unwrap().is_none());
        assert!(st.connections.get("a").is_err(), "новый профиль удалён");
    }

    #[test]
    fn keychain_calls_of_save_and_delete_run_under_the_secrets_lock() {
        let store = Arc::new(AssertsLocked {
            inner: MemoryStore::default(),
            lock: std::sync::OnceLock::new(),
        });
        let (_d, st) = state_with(store.clone());
        store.lock.set(st.secrets_lock.clone()).unwrap();
        save_connection_blocking(
            &st,
            input(
                sample_profile("a"),
                SecretsInput {
                    pg_password: Some("pw".into()),
                    ..Default::default()
                },
            ),
        )
        .unwrap();
        st.delete_connection_full("a").unwrap();
        assert!(st.connections.get("a").is_err());
    }
```

Импорты в модуле тестов: `use crate::conn::profile::{ConnectionKind, PatroniSettings};` и `use crate::secrets::PATRONI_PASSWORD;`, если их там ещё нет.

- [ ] **Step 2: Падающие тесты импорта**

В `src-tauri/src/commands/folders.rs`, модуль `tests`:

1. В `failed_store_write_keeps_profiles_and_reports_removed_passwords` заменить
   `assert_eq!(err.params["count"], "2", "`a` с новым адресом и новое `n`");` на
   `assert_eq!(err.params["count"], "1", "только `a` с новым адресом: у нового `n` паролей не было");`.

2. Добавить (рядом с `NoDelete`):

```rust
    /// Keychain, отказывающий ровно на `fail_at`-м удалении (считая с 1).
    struct DeleteFailsAt {
        inner: MemoryStore,
        fail_at: usize,
        deletes: std::sync::atomic::AtomicUsize,
    }

    impl SecretStore for DeleteFailsAt {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> AppResult<()> {
            let n = self.deletes.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == self.fail_at {
                return Err(AppError::internal_with("keychain unavailable"));
            }
            self.inner.delete(key)
        }
    }

    /// Файл перенаправляет оба существующих подключения; keychain падает на первом удалении второго.
    /// Пароли первого уже удалены — ошибка говорит об этом и считает только его.
    #[test]
    fn keychain_failure_on_the_second_connection_reports_the_first_as_removed() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(DeleteFailsAt {
            inner: MemoryStore::default(),
            fail_at: CONNECTION_SECRET_FIELDS.len() + 1,
            deletes: std::sync::atomic::AtomicUsize::new(0),
        });
        let st = AppState::new(&dir.path().join("data"), secrets.clone()).unwrap();
        for id in ["a", "b"] {
            st.connections.upsert(sample_profile(id)).unwrap();
            set_all_secrets(&secrets.inner, id);
        }
        let mut a = serde_json::to_value(sample_profile("a")).unwrap();
        a["hosts"] = serde_json::json!([{"host": "evil-a", "port": 5432}]);
        let mut b = serde_json::to_value(sample_profile("b")).unwrap();
        b["hosts"] = serde_json::json!([{"host": "evil-b", "port": 5432}]);
        let path = dir.path().join("in.glafira-postgres-connections.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "format": "glafira-postgres-connections", "version": 1,
                "folders": [], "connections": [a, b],
            }))
            .unwrap(),
        )
        .unwrap();

        let err = import_from_file(&st, &path, None, true, true).unwrap_err();
        assert_eq!(err.key, "errors.import.notSaved");
        assert_eq!(err.params["count"], "1");
        // Хранилище не тронуто; у `a` паролей нет, у `b` — все на месте.
        assert_eq!(st.connections.get("a").unwrap().hosts[0].host, "localhost");
        assert_eq!(st.connections.get("b").unwrap().hosts[0].host, "localhost");
        assert!(secrets_left(&secrets.inner, "a").is_empty());
        assert_eq!(secrets_left(&secrets.inner, "b").len(), CONNECTION_SECRET_FIELDS.len());
    }

    /// Удаление секретов импорта идёт под `AppState::secrets_lock` (см. `AssertsLocked` в commands/connections.rs).
    #[test]
    fn import_deletes_secrets_under_the_secrets_lock() {
        struct Locked {
            inner: MemoryStore,
            lock: std::sync::OnceLock<Arc<std::sync::Mutex<()>>>,
        }
        impl SecretStore for Locked {
            fn get(&self, key: &str) -> AppResult<Option<String>> {
                self.inner.get(key)
            }
            fn set(&self, key: &str, value: &str) -> AppResult<()> {
                self.inner.set(key, value)
            }
            fn delete(&self, key: &str) -> AppResult<()> {
                assert!(self.lock.get().unwrap().try_lock().is_err(), "удаление вне блокировки секретов");
                self.inner.delete(key)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let secrets = Arc::new(Locked { inner: MemoryStore::default(), lock: std::sync::OnceLock::new() });
        let st = AppState::new(&dir.path().join("data"), secrets.clone()).unwrap();
        secrets.lock.set(st.secrets_lock.clone()).unwrap();
        st.connections.upsert(sample_profile("a")).unwrap();
        set_all_secrets(&secrets.inner, "a");
        let path = write_file(dir.path(), "changed");
        import_from_file(&st, &path, None, true, true).unwrap();
        assert!(secrets_left(&secrets.inner, "a").is_empty());
    }
```

Если `write_file`, `set_all_secrets`, `secrets_left` принимают `&dyn SecretStore`, передавать `&secrets.inner` как есть (`MemoryStore` реализует трейт).

- [ ] **Step 3: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib commands::`
Expected: FAIL — нет поля `secrets_lock`, пароль `old` не восстановлен, `count` = 2.

- [ ] **Step 4: Реализация**

`src-tauri/src/state.rs` — поле и инициализация:

```rust
    /// Операции с секретами подключений (сохранение формы, импорт, удаление) идут по одной. Иначе импорт,
    /// удаливший пароли подключения, мог бы записать адрес из файла после того, как параллельное сохранение
    /// положило в keychain новый пароль. Берётся в блокирующем потоке и держится вокруг обращений к keychain
    /// и хранилищу; блокировка хранилища берётся внутри неё (порядок один везде — взаимной блокировки нет).
    pub secrets_lock: Arc<std::sync::Mutex<()>>,
```

в `new`: `secrets_lock: Arc::new(std::sync::Mutex::new(())),`. В `delete_connection_full` первой строкой: `let _secrets = self.secrets_lock.lock_safe();` (импорт `use crate::sync::MutexExt;`).

`src-tauri/src/commands/connections.rs`:

```rust
/// Прежние значения полей, которые операции изменят: при сбое keychain на середине уже применённые
/// записи возвращаются назад, и профиль с секретами остаются согласованными.
fn remember_before(
    store: &dyn SecretStore,
    id: &str,
    ops: &[(&'static str, SecretOp)],
) -> AppResult<Vec<(&'static str, Option<String>)>> {
    ops.iter()
        .filter(|(_, op)| !matches!(op, SecretOp::Keep))
        .map(|(field, _)| Ok((*field, store.get(&secret_key(id, field))?)))
        .collect()
}

/// Применяет операции; если keychain отказал посреди набора, возвращает уже изменённые поля к прежним
/// значениям (сбой отката только пишется в журнал: лучше, чем ничего не вернуть).
pub(crate) fn apply_secret_ops_undoable(
    store: &dyn SecretStore,
    id: &str,
    ops: Vec<(&'static str, SecretOp)>,
) -> AppResult<()> {
    let before = remember_before(store, id, &ops)?;
    if let Err(e) = apply_secret_ops(store, id, ops) {
        for (field, prev) in before {
            let key = secret_key(id, field);
            let restored = match prev {
                Some(v) => store.set(&key, &v),
                None => store.delete(&key),
            };
            if let Err(r) = restored {
                tracing::error!("failed to restore secret {field} of {id} after a keychain error: {r}");
            }
        }
        return Err(e);
    }
    Ok(())
}
```

В `save_connection_blocking`: первой строкой `let _secrets = st.secrets_lock.lock_safe();`, а `apply_secret_ops(st.secrets.as_ref(), &saved.id, ops)` заменить на `apply_secret_ops_undoable(...)`. Доктринный комментарий функции дополнить: «…и уже применённые изменения keychain возвращаются назад».

`src-tauri/src/commands/folders.rs` — `not_saved` и применение:

```rust
/// Импорт записан не будет, но пароли части подключений уже удалены: пользователю остаётся ввести их заново.
/// `reset` — сколько ОБНОВЛЯЕМЫХ подключений лишились паролей (у добавляемых удалять было нечего).
fn not_saved(error: AppError, reset: usize) -> AppError {
    tracing::warn!(
        "import not saved after the secrets of {reset} connections were removed: {}",
        error.message
    );
    app_err!(
        ErrorCode::Internal,
        "errors.import.notSaved",
        "The import was not saved. Saved passwords of {{count}} connections it would change have already been removed — enter them again",
        count = reset
    )
    .with_details(error.details.unwrap_or(error.message))
}
```

(шаблон и `.with_details` — как в текущей версии `not_saved`; менять только параметр и комментарий.)

Тело применения в `import_from_file` вынести в `apply_import_attempts`, а в `import_from_file` оставить:

```rust
    // Секреты и хранилище — под блокировкой секретов: параллельное сохранение формы не положит новый пароль
    // под адрес из файла. Keychain при этом вне блокировки записи хранилища (см. `apply_import_attempts`).
    let _secrets = st.secrets_lock.lock_safe();
    let mut cleared: HashSet<String> = HashSet::new();
    // Обновляемые подключения, уже лишившиеся паролей: о них говорит любая последующая ошибка.
    let mut reset: HashSet<String> = HashSet::new();
    apply_import_attempts(st, &parsed, &opts, &file_exists, &mut cleared, &mut reset)
        .map_err(|e| if reset.is_empty() { e } else { not_saved(e, reset.len()) })
```

и

```rust
fn apply_import_attempts(
    st: &AppState,
    parsed: &Parsed,
    opts: &ImportOptions,
    file_exists: &dyn Fn(&str) -> bool,
    cleared: &mut HashSet<String>,
    reset: &mut HashSet<String>,
) -> AppResult<ImportPreview> {
    for _ in 0..IMPORT_ATTEMPTS {
        let plan = plan_import(&st.connections.snapshot(), parsed, opts, file_exists)?;
        for id in ids_to_clear(&plan) {
            if cleared.contains(&id) {
                continue;
            }
            let profile = plan
                .next
                .connections
                .iter()
                .find(|c| c.id == id)
                .expect("planned connection exists");
            delete_stale_secrets(st.secrets.as_ref(), profile, true)?;
            cleared.insert(id.clone());
            if plan.secrets_reset.contains(&id) {
                reset.insert(id);
            }
        }
        let written = st.connections.update_if(|data| { /* как сейчас */ })?;
        let Some((preview, updated)) = written else { continue };
        /* обработка `updated` как сейчас */
        return Ok(preview);
    }
    Err(app_err!(
        ErrorCode::Internal,
        "errors.import.storeChanged",
        "Connections changed while the import was being applied: try again"
    ))
}
```

Тип `parsed` — тот, что возвращает `parse_exchange` (посмотреть сигнатуру в `storage/exchange.rs`); старые ветки `Err(e) if cleared.is_empty()`/`Err(e) => not_saved(...)` исчезают — оборачивает вызывающий.

Словари (`en.json`/`ru.json`): `warnings.import.passwordsReset_*` и `hints.import.updateExisting` — вместо «address or security settings»/«адрес или настройки безопасности» писать про настройки соединения:
- en `_one`: "Saved passwords of {{count}} connection will be removed because its connection settings (address, user, database, TLS, Patroni) change — enter them again"; `_other`: "…of {{count}} connections … because their connection settings (…) change — enter them again".
- ru `_one`: "Сохранённые пароли {{count}} подключения будут удалены, потому что у него меняются настройки соединения (адрес, пользователь, база, TLS, Patroni) — введите их заново"; `_few`/`_many`: "…{{count}} подключений … у них меняются …"; `_other`: "…{{count}} подключения … у него меняются …".
- en `hints.import.updateExisting`: "Existing connections get the settings from the file and keep their place in the tree. Saved passwords are kept only if the connection settings (address, user, database, TLS, Patroni) do not change."; ru: "Существующие подключения получат настройки из файла и останутся на своих местах. Сохранённые пароли остаются, только если не меняются настройки соединения (адрес, пользователь, база, TLS, Patroni)."

- [ ] **Step 5: Прогнать проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check && pnpm test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src src/locales
git commit -m "fix(secrets): операции с секретами по одной, откат keychain при сбое на середине, честный счётчик импорта

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Соединения: TCP keepalive, SQLSTATE остановки, причина отказа TLS, предел discover, pgpass как в libpq

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/conn/pg.rs`, `src-tauri/src/conn/errors.rs`, `src-tauri/src/conn/testutil.rs`, `src-tauri/src/conn/pgpass.rs`, `src-tauri/src/topology/patroni.rs`, `src-tauri/src/commands/catalog.rs` (тест-закрепление), `src/locales/en.json`, `src/locales/ru.json`
- Test: unit-тесты в тех же файлах

**Interfaces:**
- Consumes: `ConnectSpec::connect_with_handle`, `classify_sqlstate`, `classify_text`, `from_pg`, `discover`, `fetch_cluster`, `pgpass::{split_fields, lookup, default_path}`.
- Produces: `conn::pg::tcp_connect(host, port) -> io::Result<TcpStream>`; `conn::errors::from_handshake(e, verify: Option<X509VerifyError>) -> AppError`; `conn::testutil::make_expired_server_cert(ca, dns)`; `topology::patroni::DISCOVER_TIMEOUT = 20 s`; `pgpass::default_path_from(env: Option<OsString>)`; новые ключи `errors.pg.{serverShutdown, lockTimeout, certExpired, certNotYetValid}`, `errors.patroni.discoverTimeout`.

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/conn/pg.rs`, модуль `tests`:

```rust
    #[tokio::test]
    async fn tcp_connection_has_keepalive_and_nodelay() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = tcp_connect("127.0.0.1", port).await.unwrap();
        assert!(socket2::SockRef::from(&stream).keepalive().unwrap());
        assert!(stream.nodelay().unwrap());
    }

    /// OpenSSL в тексте ошибки причину отказа не называет; просроченный сертификат отличается от чужого CA
    /// по результату проверки из обратного вызова.
    #[tokio::test]
    async fn expired_server_certificate_is_reported_as_expired() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_expired_server_cert(&ca, "localhost"));
        let spec = loopback_spec(server.port, "localhost", SslMode::VerifyCa, Some(&ca_path));
        let e = spec.connect().await.unwrap_err();
        assert_eq!(e.code, ErrorCode::TlsError);
        assert_eq!(e.key, "errors.pg.certExpired", "{e:?}");

        // Чужой CA по-прежнему «недоверенный», а не «просроченный»: цепочка не строится раньше проверки срока.
        let other = testutil::make_ca("other-ca");
        let other_path = write_pem(dir.path(), "other.pem", &other.cert);
        let server = testutil::tls_server(&testutil::make_expired_server_cert(&ca, "localhost"));
        let e = loopback_spec(server.port, "localhost", SslMode::VerifyCa, Some(&other_path))
            .connect()
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.caUntrusted", "{e:?}");
    }
```

`src-tauri/src/conn/errors.rs`, модуль `tests` (есть ли он — посмотреть; если нет, создать):

```rust
    #[test]
    fn shutdown_and_lock_timeout_sqlstates_are_classified() {
        use tokio_postgres::error::SqlState;
        for code in [SqlState::ADMIN_SHUTDOWN, SqlState::CRASH_SHUTDOWN] {
            let (c, t) = classify_sqlstate(&code);
            assert_eq!((c, t.key.as_str()), (ErrorCode::NodeUnavailable, "errors.pg.serverShutdown"));
        }
        let (c, t) = classify_sqlstate(&SqlState::LOCK_NOT_AVAILABLE);
        assert_eq!((c, t.key.as_str()), (ErrorCode::Timeout, "errors.pg.lockTimeout"));
    }
```

`src-tauri/src/commands/catalog.rs`, тест `server_errors_never_look_like_a_lost_connection`: обрыв теперь означают 53300 и коды остановки сервера. Перенести `"57P01"`, `"57P02"` из `others` в `named`; утверждение «только 53300» заменить на: соединение считается потерянным ровно для `{"53300", "57P01", "57P02", "57P03"}` (57P03 — `NodeUnavailable` при подключении), остальные — нет. Комментарий над тестом обновить.

`src-tauri/src/topology/patroni.rs`, модуль `tests`:

```rust
    /// На каждый запрос «думает» `delay` и отвечает отказом.
    struct SlowHttp {
        delay: Duration,
    }

    impl PatroniHttp for SlowHttp {
        fn get_json<'a>(&'a self, url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            Box::pin(async move {
                tokio::time::sleep(self.delay).await;
                Err(unreachable_err(url))
            })
        }
    }

    /// Адресов много, каждый ждёт до HTTP_TIMEOUT: обнаружение ограничено общим сроком.
    #[tokio::test(start_paused = true)]
    async fn discover_has_an_overall_deadline() {
        let urls: Vec<String> = (1..=8).map(|i| format!("http://pg-{i}:8008")).collect();
        let s = settings(&urls.iter().map(String::as_str).collect::<Vec<_>>());
        let http = SlowHttp { delay: HTTP_TIMEOUT };
        let started = tokio::time::Instant::now();
        let e = discover(&http, &s, "c", None, 0).await.unwrap_err();
        assert_eq!(e.key, "errors.patroni.discoverTimeout");
        assert_eq!(started.elapsed(), DISCOVER_TIMEOUT, "а не 8 × {HTTP_TIMEOUT:?}");

        // С прежней топологией — как при любом отказе API: участники недоступны, предупреждение одно.
        let prev = discover(
            &FakeHttp::new(vec![/* как в unreachable_api_fails_first_time_and_degrades_later */]),
            &s, "c", None, 0,
        )
        .await
        .unwrap();
        let t = discover(&http, &s, "c", Some(&prev), 1).await.unwrap();
        assert_eq!(t.warnings, vec![TopologyWarning::PatroniUnreachable]);
        assert!(t.members.iter().all(|m| !m.reachable && m.error.as_ref().unwrap().key == "errors.patroni.discoverTimeout"));
    }
```

Прежнюю топологию строить так же, как в `unreachable_api_fails_first_time_and_degrades_later` (ответы `FakeHttp` для `/cluster` и `/patroni` оттуда); если `settings(...)` того теста отличается, использовать его `s` для `prev`.

`src-tauri/src/conn/pgpass.rs`, модуль `tests`:

```rust
    /// libpq: `\*` — буквальная звёздочка, а не любое значение; одинокий обратный слеш в конце остаётся.
    #[test]
    fn escaped_star_is_literal_and_trailing_backslash_is_kept() {
        let file = "\\*:5432:db:u:star-host\n*:*:*:u:x\\";
        assert_eq!(lookup(file, "*", 5432, "db", "u").as_deref(), Some("star-host"));
        assert_eq!(lookup(file, "h", 5432, "db", "u").as_deref(), Some("x\\"));
    }

    #[test]
    fn non_utf8_bytes_do_not_hide_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pgpass");
        std::fs::write(&path, b"# \xff\xfe comment\nh:5432:db:u:secret\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(find_password_in(&path, "h", 5432, "db", "u").as_deref(), Some("secret"));
    }

    #[test]
    fn empty_pgpassfile_means_the_default_location() {
        assert_eq!(default_path_from(Some("".into())), default_path_from(None));
        assert_eq!(default_path_from(Some("/x/pgpass".into())), Some(PathBuf::from("/x/pgpass")));
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn:: topology::patroni commands::catalog`
Expected: ошибки компиляции (`tcp_connect`, `make_expired_server_cert`, `DISCOVER_TIMEOUT`, `default_path_from`) и падения классификации.

- [ ] **Step 3: Реализация**

`src-tauri/Cargo.toml`, `[dependencies]`: `socket2 = { version = "0.6", features = ["all"] }`.

`src-tauri/src/conn/pg.rs`:

```rust
/// Отсечка молчащего соединения на уровне TCP: после 60 с тишины зонды раз в 10 с. После failover
/// полуоткрытое соединение иначе живёт в пуле, пока его не попробуют использовать.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// TCP-соединение с узлом: без задержки Нейгла и с keepalive (ошибки настройки сокета не фатальны).
pub(crate) async fn tcp_connect(host: &str, port: u16) -> std::io::Result<TcpStream> {
    let stream = TcpStream::connect((host, port)).await?;
    let _ = stream.set_nodelay(true);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL);
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&keepalive);
    Ok(stream)
}
```

В `connect_with_handle` строки `let stream = TcpStream::connect(...)…; let _ = stream.set_nodelay(true);` заменить на
`let stream = tcp_connect(&self.tcp_host, self.tcp_port).await.map_err(|e| errors::tcp_error(&e, &self.host, self.port))?;`.

Ветка TLS в `connect_with_handle`:

```rust
                Some(connector) => {
                    let mut ssl = connector.configure().map_err(errors::tls_setup)?;
                    if self.ssl_mode != SslMode::VerifyFull {
                        ssl.set_verify_hostname(false);
                    }
                    // OpenSSL в тексте ошибки причину отказа не называет («certificate verify failed»):
                    // запоминаем её из обратного вызова проверки, чтобы отличить просроченный сертификат
                    // от чужого CA. Режим проверки не меняется.
                    let verify_error: Arc<Mutex<Option<X509VerifyError>>> = Arc::new(Mutex::new(None));
                    let slot = verify_error.clone();
                    let mode = ssl.verify_mode();
                    ssl.set_verify_callback(mode, move |ok, ctx| {
                        if !ok {
                            *slot.lock_safe() = Some(ctx.error());
                        }
                        ok
                    });
                    let tls = postgres_openssl::TlsConnector::new(ssl, &self.host);
                    let (client, conn) = cfg
                        .connect_raw(stream, tls)
                        .await
                        .map_err(|e| errors::from_handshake(&e, verify_error.lock_safe().take()))?;
                    Ok((client, spawn_connection(conn)))
                }
```

Импорты: `use std::sync::{Arc, Mutex};`, `use openssl::x509::X509VerifyError;`, `use crate::sync::MutexExt;`.

`src-tauri/src/conn/errors.rs`:

```rust
/// Ошибка рукопожатия TLS с причиной отказа проверки сертификата (`verify` — из обратного вызова, см.
/// `ConnectSpec::connect_with_handle`); без неё — обычная классификация по тексту.
pub fn from_handshake(e: &tokio_postgres::Error, verify: Option<X509VerifyError>) -> AppError {
    match verify {
        Some(v) if v == X509VerifyError::CERT_HAS_EXPIRED => app_err!(
            ErrorCode::TlsError,
            "errors.pg.certExpired",
            "The server certificate has expired"
        ),
        Some(v) if v == X509VerifyError::CERT_NOT_YET_VALID => app_err!(
            ErrorCode::TlsError,
            "errors.pg.certNotYetValid",
            "The server certificate is not valid yet"
        ),
        Some(v) if v == X509VerifyError::HOSTNAME_MISMATCH => app_err!(
            ErrorCode::TlsError,
            "errors.pg.hostnameMismatch",
            "The server host name does not match its certificate"
        ),
        _ => from_pg(e),
    }
}
```

(импорт `use openssl::x509::X509VerifyError;`). В `classify_sqlstate` перед последней веткой `else`:

```rust
    } else if *code == SqlState::ADMIN_SHUTDOWN || *code == SqlState::CRASH_SHUTDOWN {
        (
            ErrorCode::NodeUnavailable,
            ltext!(
                "errors.pg.serverShutdown",
                "The server is shutting down or restarting"
            ),
        )
    } else if *code == SqlState::LOCK_NOT_AVAILABLE {
        (
            ErrorCode::Timeout,
            ltext!(
                "errors.pg.lockTimeout",
                "The query waited for a lock longer than lock_timeout"
            ),
        )
```

`src-tauri/src/commands/catalog.rs`: `connection_lost` — `matches!(e.code, ErrorCode::ConnectionFailed | ErrorCode::TlsError | ErrorCode::NodeUnavailable)`, комментарий: после остановки сервера соединение всё равно мертво, его закрывают сразу.

`src-tauri/src/conn/testutil.rs`: `issue(...)` переименовать в `issue_with_validity(cn, issuer, ca, dns, serial, not_before: i64, not_after: i64)` (оба срока — `Asn1Time::from_unix(...)`), `issue(...)` вызывает её с `(unix_now() - 3600, unix_now() + 2 * 86_400)`, и добавить:

```rust
/// Сертификат сервера, подписанный `ca`, срок действия которого истёк час назад.
pub fn make_expired_server_cert(ca: &Issued, dns: &str) -> Issued {
    issue_with_validity(dns, Some(ca), false, Some(dns), 4, unix_now() - 7200, unix_now() - 3600)
}
```

`src-tauri/src/topology/patroni.rs`:

```rust
/// Общий предел обнаружения кластера: адресов API может быть много (профиль плюс прежние участники),
/// и каждый ждёт до `HTTP_TIMEOUT` — без общего срока один опрос мог бы длиться дольше минуты.
pub const DISCOVER_TIMEOUT: Duration = Duration::from_secs(20);
```

в `discover`:

```rust
    let fetched = match tokio::time::timeout(
        DISCOVER_TIMEOUT,
        fetch_cluster(http, &cluster_bases(settings, previous)),
    )
    .await
    {
        Ok(r) => r,
        Err(_) => Err(app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.discoverTimeout",
            "Patroni API did not answer within {{secs}} s",
            secs = DISCOVER_TIMEOUT.as_secs()
        )),
    };
    let cluster = match fetched {
        Ok(c) => c,
        Err(e) => { /* ветка «прежние участники недоступны» как сейчас */ }
    };
```

`src-tauri/src/conn/pgpass.rs`:

```rust
/// Поле строки .pgpass: `*` без экранирования — любое значение, `\*` — буквальная звёздочка (как в libpq).
struct Field {
    text: String,
    wildcard: bool,
}

fn split_fields(line: &str) -> Vec<Field> {
    // (текст, был ли в нём экранированный символ)
    let mut fields: Vec<(String, bool)> = vec![(String::new(), false)];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        let last = fields.last_mut().expect("at least one field");
        match c {
            '\\' => match chars.next() {
                Some(n) => {
                    last.0.push(n);
                    last.1 = true;
                }
                // Одинокий обратный слеш в конце строки libpq оставляет как есть.
                None => last.0.push('\\'),
            },
            ':' => fields.push((String::new(), false)),
            c => last.0.push(c),
        }
    }
    fields
        .into_iter()
        .map(|(text, escaped)| Field { wildcard: text == "*" && !escaped, text })
        .collect()
}
```

`lookup`: `let matches = |f: &Field, value: &str| f.wildcard || f.text == value;` и `.map(|f| f[4].text.clone())`. `find_password_in`: `let bytes = std::fs::read(path).ok()?; lookup(&String::from_utf8_lossy(&bytes), ...)`. `default_path`:

```rust
pub fn default_path() -> Option<PathBuf> {
    default_path_from(std::env::var_os("PGPASSFILE"))
}

/// `PGPASSFILE`: пустое значение — как не заданное (libpq берёт файл по умолчанию).
fn default_path_from(env: Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(p) = env.filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    /* ветки windows / не windows как сейчас */
}
```

Словари — новые ключи в обоих языках: `errors.pg.serverShutdown` (ru «Сервер останавливается или перезапускается»), `errors.pg.lockTimeout` (ru «Запрос ждал блокировку дольше lock_timeout»), `errors.pg.certExpired` (ru «Срок действия сертификата сервера истёк»), `errors.pg.certNotYetValid` (ru «Сертификат сервера ещё не вступил в силу»), `errors.patroni.discoverTimeout` (ru «Patroni API не ответил за {{secs}} с»). Подсказка `hints.connections.sslMode`: en «prefer — TLS if the server supports it, the certificate is not checked even with a CA file; require — TLS only, with a CA file the certificate is checked too; verify-ca and verify-full — check the certificate (verify-full also the host name).», ru «prefer — TLS, если сервер умеет, сертификат не проверяется даже с файлом CA; require — только TLS, с файлом CA сертификат тоже проверяется; verify-ca и verify-full — с проверкой сертификата (verify-full проверяет и имя хоста).»

- [ ] **Step 4: Прогнать проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check` и интеграционные тесты со стендом (см. Global Constraints).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/locales
git commit -m "fix(conn): TCP keepalive, остановка сервера и lock_timeout как отдельные ошибки, просроченный сертификат, срок discover, pgpass как в libpq

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Runtime: пул опроса отдельно от каталога, переподключение, какие секреты сохранены

**Files:**
- Modify: `src-tauri/src/conn/pool.rs`, `src-tauri/src/topology/connector.rs`, `src-tauri/src/topology/runtime.rs`, `src-tauri/src/state.rs`, `src-tauri/src/commands/topology.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/commands/catalog.rs` (вызовы `pooled_client` в тестах), `src-tauri/src/lib.rs`, `src/api/types.ts`, `src/api/client.ts`, `src/api/client.channel.test.ts`, `src/locales/en.json`, `src/locales/ru.json`
- Test: unit-тесты в `pool.rs`, `connector.rs`, `runtime.rs`, `commands/connections.rs`; таблица команд в `client.channel.test.ts`

**Interfaces:**
- Consumes: `PoolRegistry`, `pooled_client`, `Runtimes::{connect, disconnect}`, `blocking`.
- Produces: `conn::pool::PoolPurpose { Catalog, Probe }` (поле `PoolKey::purpose`), `PgPool::with_max(spec, max)`; `pooled_client(..., purpose)` (последний параметр); `Runtimes::reconnect(profile) -> AppResult<TopologyView>`; команды `reconnect(id) -> TopologyView`, `connection_secrets(id) -> SecretsSet { pgPassword, pgKeyPassword, patroniPassword, patroniKeyPassword: bool }`; `api.reconnect(id)`, `api.connectionSecrets(id)`, тип `SecretsSet` в `types.ts`. Текст `errors.pg.loginNotRetried` отсылает к «Переподключить».

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/topology/connector.rs`, модуль `tests` (все конструкторы `PoolKey` и вызовы `pooled_client` получают `PoolPurpose::Catalog`; новый тест):

```rust
    /// Опрос топологии не ждёт, пока страницы каталога освободят пул: у него свой пул из одного соединения.
    #[tokio::test]
    async fn probe_has_its_own_pool_and_is_not_blocked_by_catalog_clients() {
        let server = testutil::stalling_pg_server();
        let nodes = pooled(SslMode::Disable);
        let mut held = Vec::new();
        for _ in 0..crate::conn::pool::MAX_CONNECTIONS {
            held.push(client_of(&nodes, server.port).await);
        }
        assert_eq!(server.accepted(), 3);
        // Молчащий сервер: опрос заканчивается своим пределом, но соединение он открыл — четвёртое.
        let e = nodes
            .facts_within("m", "127.0.0.1", server.port, Duration::from_millis(300))
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.pg.noAnswer");
        assert_eq!(server.accepted(), 4, "опрос не стоял в очереди за местами каталога");
        let mut keys = nodes.pools.keys();
        keys.sort_by_key(|k| k.purpose == PoolPurpose::Probe);
        assert_eq!(keys.iter().map(|k| k.purpose).collect::<Vec<_>>(), [PoolPurpose::Catalog, PoolPurpose::Probe]);
        drop(held);
    }
```

`client_of` вызывает `pooled_client(..., "postgres", PoolPurpose::Catalog)`; `facts_within` внутри использует `PoolPurpose::Probe`. (`stalling_pg_server` принимает соединение и молчит — `client_of` на нём не завершится: если так, занимать места каталога `silent_server`/другим сервером, который отвечает на startup, как в `waiting_for_a_pool_permit_is_bounded_by_the_limit`; взять ровно тот сервер, что там.)

`src-tauri/src/conn/pool.rs`, модуль `tests`:

```rust
    #[tokio::test]
    async fn probe_pool_has_a_single_permit() {
        let pool = PgPool::with_max(spec(), 1);
        assert_eq!(pool.permits.available_permits(), 1);
        assert_eq!(PgPool::new(spec()).permits.available_permits(), MAX_CONNECTIONS);
    }
```

`src-tauri/src/topology/runtime.rs`, модуль `tests`:

```rust
    /// «Переподключить»: закрыть и открыть заново — пулы (с отметкой об отказе во входе) сброшены, состояния
    /// прошли через Disconnected, замок подключения после отключения не копится.
    #[tokio::test]
    async fn reconnect_closes_and_reopens_the_connection() {
        let (rt, _src, rec, pools) = manual(vec![Ok(two("a")), Ok(two("a"))]);
        rt.connect(patroni_profile()).await.unwrap();
        add_pool(&pools, "a");
        assert!(!pools.keys().is_empty());

        let view = rt.reconnect(patroni_profile()).await.unwrap();
        assert_eq!(view.state, ConnState::Connected);
        assert!(pools.keys().is_empty(), "пулы вытеснены");
        assert_eq!(
            *rec.states.lock_safe(),
            vec![
                ConnState::Connecting,
                ConnState::Connected,
                ConnState::Disconnected,
                ConnState::Connecting,
                ConnState::Connected
            ]
        );

        rt.disconnect("c");
        assert!(rt.connect_locks.lock_safe().is_empty(), "замок удалён вместе с подключением");
    }
```

`src-tauri/src/commands/connections.rs`, модуль `tests`:

```rust
    #[test]
    fn connection_secrets_report_which_fields_are_saved() {
        let store = Arc::new(MemoryStore::default());
        store.set(&secret_key("a", PG_PASSWORD), "pw").unwrap();
        store.set(&secret_key("a", PATRONI_KEY_PASSWORD), "kp").unwrap();
        store.set(&secret_key("a", PG_KEY_PASSWORD), "").unwrap();
        assert_eq!(
            secrets_set(store.as_ref(), "a").unwrap(),
            SecretsSet { pg_password: true, pg_key_password: false, patroni_password: false, patroni_key_password: true }
        );
        assert_eq!(serde_json::to_value(secrets_set(store.as_ref(), "a").unwrap()).unwrap()["pgKeyPassword"], false);
    }
```

`src/api/client.channel.test.ts`: в таблицу «команда / аргументы» добавить строки `["reconnect", (a) => a.reconnect("c"), { id: "c" }]` и `["connection_secrets", (a) => a.connectionSecrets("c"), { id: "c" }]` (в формате, которым записаны соседние строки).

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::pool topology:: commands::connections` и `pnpm exec vitest run src/api/client.channel.test.ts`
Expected: ошибки компиляции (`PoolPurpose`, `with_max`, `reconnect`, `secrets_set`), падение таблицы команд.

- [ ] **Step 3: Реализация**

`src-tauri/src/conn/pool.rs`:

```rust
/// Назначение пула. Опрос топологии получает свой пул из одного соединения: иначе страницы каталога
/// занимали бы все три места, и опрос ждал бы их дольше своего предела — ложное `memberUnreachable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PoolPurpose {
    Catalog,
    Probe,
}

impl PoolPurpose {
    fn max_connections(self) -> usize {
        match self {
            Self::Catalog => MAX_CONNECTIONS,
            Self::Probe => 1,
        }
    }
}
```

`PoolKey` получает `pub purpose: PoolPurpose`. `PgPool::build(spec, validate_after, close_grace, max)` → `Semaphore::new(max)`; `new` передаёт `MAX_CONNECTIONS`; `with_validate_after` — тоже; новый `pub fn with_max(spec: ConnectSpec, max: usize) -> Arc<Self>` (`build(spec, VALIDATE_AFTER, spec.timeout, max)`). `insert_if_absent`: `let pool = PgPool::with_max(spec, key.purpose.max_connections());`. Отдельный пул опроса — одно место на узел сверх трёх каталожных; константу `MAX_CONNECTIONS` и её документацию не менять, в доктринный комментарий `PoolRegistry` добавить фразу про пул опроса.

`src-tauri/src/topology/connector.rs`: `pooled_client(..., database: &str, purpose: PoolPurpose)` — `purpose` в `PoolKey`; `facts_within` передаёт `PoolPurpose::Probe`; `state.rs::client` — `PoolPurpose::Catalog`; тесты `commands/catalog.rs` и `connector.rs` — `PoolPurpose::Catalog`; конструкторы `PoolKey` в тестах `runtime.rs`, `pool.rs`, `connector.rs` — поле `purpose: PoolPurpose::Catalog`.

`src-tauri/src/topology/runtime.rs`:

```rust
    /// «Переподключить»: закрыть и открыть заново. Пулы (и запомненный отказ во входе) сбрасываются,
    /// секреты читаются из keychain заново — без ручного «Отключиться» и раскрытия узла.
    pub async fn reconnect(self: &Arc<Self>, profile: ConnectionProfile) -> AppResult<TopologyView> {
        self.disconnect(&profile.id);
        self.connect(profile).await
    }
```

в `disconnect` после `self.source.forget(id);`:

```rust
        // Замок подключения нужен только пока кто-то подключается: иначе карта росла бы с каждым id.
        self.connect_locks
            .lock_safe()
            .retain(|k, l| k != id || Arc::strong_count(l) > 1);
```

`src-tauri/src/commands/topology.rs`:

```rust
/// Закрыть и открыть подключение заново (меню «Переподключить»): после смены пароля на сервере или сбоя
/// LDAP запомненный отказ во входе сбрасывается и секреты читаются заново.
#[tauri::command]
pub async fn reconnect(state: State<'_, AppState>, id: String) -> AppResult<TopologyView> {
    let profile = state.profile(&id)?;
    state.runtimes.reconnect(profile).await
}
```

`src-tauri/src/commands/connections.rs`:

```rust
/// Какие секреты подключения сохранены (значения не возвращаются): форма показывает «оставить
/// сохранённый» только там, где есть что оставить.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretsSet {
    pub pg_password: bool,
    pub pg_key_password: bool,
    pub patroni_password: bool,
    pub patroni_key_password: bool,
}

fn secrets_set(store: &dyn SecretStore, id: &str) -> AppResult<SecretsSet> {
    let has = |field: &str| -> AppResult<bool> {
        Ok(store.get(&secret_key(id, field))?.is_some_and(|v| !v.is_empty()))
    };
    Ok(SecretsSet {
        pg_password: has(PG_PASSWORD)?,
        pg_key_password: has(PG_KEY_PASSWORD)?,
        patroni_password: has(PATRONI_PASSWORD)?,
        patroni_key_password: has(PATRONI_KEY_PASSWORD)?,
    })
}

#[tauri::command]
pub async fn connection_secrets(state: State<'_, AppState>, id: String) -> AppResult<SecretsSet> {
    let st = state.inner().clone();
    blocking(move || secrets_set(st.secrets.as_ref(), &id)).await
}
```

`src-tauri/src/lib.rs`: зарегистрировать `commands::topology::reconnect` и `commands::connections::connection_secrets`.

`src/api/types.ts`:

```ts
/** Какие секреты подключения сохранены в keychain (значения не передаются). */
export interface SecretsSet {
  pgPassword: boolean;
  pgKeyPassword: boolean;
  patroniPassword: boolean;
  patroniKeyPassword: boolean;
}
```

`src/api/client.ts`: `reconnect: (id: string) => call<TopologyView>("reconnect", { id }),` рядом с `connect`; `connectionSecrets: (id: string) => call<SecretsSet>("connection_secrets", { id }),` рядом с `saveConnection`.

Текст `errors.pg.loginNotRetried` (`conn/errors.rs::login_not_retried` и en/ru): en «Authentication failed; the login is not retried until the connection is reopened: check the user, the password and pg_hba.conf, then choose “Reconnect” in the connection menu»; ru «Вход не удался; повторных попыток не будет, пока подключение не откроют заново: проверьте пользователя, пароль и pg_hba.conf и выберите «Переподключить» в меню подключения». Ключ `sidebar.reopen` (en «Reconnect», ru «Переподключить») добавить уже здесь — его использует задача 4.

- [ ] **Step 4: Прогнать проверки**

Run: все проверки из Global Constraints, включая интеграционные тесты со стендом.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/api src/locales
git commit -m "fix(runtime): свой пул для опроса топологии, команда переподключения, какие секреты сохранены

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Дерево и обзор: «Переподключить», живые группы каталога, журнал без дублей, тесты без гонок

**Files:**
- Modify: `src/components/connections/tree/ConnectionNode.tsx` (+ `.test.tsx`), `src/components/connections/tree/CatalogNodes.tsx` (+ `CatalogNodes.details.test.tsx`), `src/components/overview/Overview.tsx` (+ `.test.tsx`), `src/lib/useConnectionEvents.ts` (+ `.test.ts`), `src/components/connections/tree/ObjectList.test.tsx`, `src/styles.css`, `src/lib/plainInputs.ts`, `src/locales/en.json`, `src/locales/ru.json`

**Interfaces:**
- Consumes: `api.reconnect` (задача 3), `sidebar.reopen`, `useTopology`, `TreeGroup`.
- Produces: пункт меню и кнопка «Переподключить»; `CatalogNodes` не размонтирует группы при `catalogMember === null`; `applyTopology` не добавляет событие, уже лежащее в кэше.

- [ ] **Step 1: Падающие тесты**

`src/components/connections/tree/ConnectionNode.test.tsx`:
- в ожидаемых списках меню подключённого Patroni и одиночного сервера добавить `"Переподключить"` перед `"Отключиться"`; у упавшего подключения список не меняется;
- новый тест:

```tsx
  it("меню живого подключения: «Переподключить» закрывает и открывает заново, узел остаётся раскрытым", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    vi.mocked(api.reconnect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Часть узлов недоступна" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    await user.click(screen.getByRole("button", { name: "Действия с подключением billing" }));
    await user.click(screen.getByRole("menuitem", { name: "Переподключить" }));
    expect(api.reconnect).toHaveBeenCalledWith("c");
    expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: toggleName })).toHaveAttribute("aria-expanded", "true");
  });
```

(в `vi.mock` клиента добавить `reconnect: vi.fn()`).

`src/components/overview/Overview.test.tsx`:
- тест «состояние %s само подключение не запускает»: для `degraded` дополнительно проверить кнопку:

```tsx
  it("часть узлов недоступна: кнопка «Переподключить» закрывает и открывает подключение заново", async () => {
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ state: "degraded" }));
    vi.mocked(api.reconnect).mockResolvedValue(topologyView());
    const user = userEvent.setup();
    renderWithClient(<Overview connectionId="c" />);
    await user.click(await screen.findByRole("button", { name: "Переподключить" }));
    expect(api.reconnect).toHaveBeenCalledWith("c");
    expect(await screen.findByText("Подключено")).toBeInTheDocument();
    expect(api.connect).not.toHaveBeenCalled();
  });
```

- в тесте «неподключённое подключение подключается само, а после явного отключения предлагает подключиться кнопкой» (и в любом тесте, где вкладка открывается в `connected` и получает событие `disconnected`) добавить `expect(api.connect).not.toHaveBeenCalled()` сразу после события отключения, до клика по кнопке;
- в тесте «журнал: полные дубли различаются номером и не склеиваются» перехватывать `console.error`:

```tsx
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      /* рендер и прежние проверки */
      expect(errors.mock.calls.flat().join(" ")).not.toMatch(/same key/);
    } finally {
      errors.mockRestore();
    }
```

`src/components/connections/tree/CatalogNodes.details.test.tsx` — заменить тест «нет узла для каталога — сообщение вместо баз и ролей»:

```tsx
  it("нет узла для каталога — сообщение, группы остаются, но без запросов", async () => {
    const user = userEvent.setup();
    renderWithClient(<CatalogNodes connection={conn} view={topologyView({ catalogMember: null })} />);
    expect(screen.getByText("Нет доступного узла для каталога")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    expect(screen.getAllByText("Нет доступного узла для каталога")).toHaveLength(2);
    expect(api.listDatabases).not.toHaveBeenCalled();
  });

  it("раскрытые группы переживают пропажу и возврат узла каталога (перезапуск сервера)", async () => {
    const user = userEvent.setup();
    const { rerender } = renderWithClient(<CatalogNodes connection={conn} view={topologyView()} />);
    await user.click(screen.getByRole("button", { name: /Базы данных/ }));
    expect(await screen.findByRole("button", { name: /^orders/ })).toBeInTheDocument();
    rerender(<CatalogNodes connection={conn} view={topologyView({ catalogMember: null })} />);
    expect(screen.getByRole("button", { name: /Базы данных/ })).toHaveAttribute("aria-expanded", "true");
    rerender(<CatalogNodes connection={conn} view={topologyView()} />);
    expect(await screen.findByRole("button", { name: /^orders/ })).toBeInTheDocument();
    expect(api.listDatabases).toHaveBeenCalledTimes(2);
  });
```

(`renderWithClient` должен возвращать `rerender`, обёрнутый в тот же провайдер; если нет — добавить в `src/test/render.tsx`.)

`src/lib/useConnectionEvents.test.ts`:

```ts
  it("событие, которое ответ журнала уже принёс, второй раз не добавляется", () => {
    const qc = new QueryClient();
    const e1: TopologyEvent = { atMs: 1_000, kind: "pauseChanged", paused: true };
    const e2: TopologyEvent = { atMs: 2_000, kind: "memberUnreachable", name: "pg-3" };
    const e3: TopologyEvent = { atMs: 3_000, kind: "memberReachable", name: "pg-3" };
    qc.setQueryData(["topologyEvents", "c"], [e2, e1]);
    applyTopology(qc, { connectionId: "c", view: topologyView(), newEvents: [e1, e2, e3] });
    expect(qc.getQueryData(["topologyEvents", "c"])).toEqual([e3, e2, e1]);
  });
```

`src/components/connections/tree/ObjectList.test.tsx` — тест «смена фильтра возвращает прокрутку…» переписать на фейковые таймеры (по образцу теста задержки фильтра в `CatalogNodes.test.tsx`):

```tsx
  it("смена фильтра возвращает прокрутку виртуального списка в начало", async () => {
    vi.mocked(api.listObjects).mockResolvedValue(page(0, 300, 600));
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      renderList(600);
      const scroller = await screen.findByRole("group", { name: "Список: Таблицы (orders.shop)" });
      scroller.scrollTop = 400;
      await user.type(screen.getByLabelText("Фильтр: Таблицы (orders.shop)"), "a");
      await act(() => vi.advanceTimersByTimeAsync(299));
      expect(scroller.scrollTop).toBe(400);
      await act(() => vi.advanceTimersByTimeAsync(1));
      expect(api.listObjects).toHaveBeenLastCalledWith("c", "orders", "shop", "tables", "a", 0, PAGE);
      expect(scroller.scrollTop).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm exec vitest run src/components/connections/tree src/components/overview src/lib/useConnectionEvents.test.ts`
Expected: FAIL (нет пункта «Переподключить», группы размонтируются, дубль добавлен).

- [ ] **Step 3: Реализация**

`ConnectionNode.tsx` — в `menu` перед блоком «Отключиться/Подключиться»:

```tsx
    // Живое подключение можно закрыть и открыть заново на месте: после смены пароля на сервере или сбоя LDAP
    // отказ во входе запоминается до переоткрытия (errors.pg.loginNotRetried).
    ...(live
      ? [
          {
            label: t("sidebar.reopen"),
            onSelect: () =>
              void api
                .reconnect(c.id)
                .then(setView)
                .catch((e: unknown) => setError(toAppError(e))),
          },
        ]
      : []),
```

`Overview.tsx` — рядом с `connect`:

```tsx
  const reopen = async () => {
    setBusy(true);
    setError(null);
    try {
      qc.setQueryData(["topology", connectionId], await api.reconnect(connectionId));
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };
```

и в шапке после кнопки «Подключиться»:

```tsx
        {v.state === "degraded" && (
          <button type="button" disabled={busy} onClick={() => void reopen()}>
            {t("sidebar.reopen")}
          </button>
        )}
```

`CatalogNodes.tsx` — нижняя часть списка:

```tsx
      {view.catalogFallback && member && <li className="tree-notice">{t("sidebar.catalogFallback", { member })}</li>}
      {!member && <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>}
      {/* Группы не размонтируются, когда узел каталога пропал (например, на время перезапуска сервера):
          раскрытое состояние переживает один неудачный опрос. Без узла внутри — то же сообщение, запросов нет. */}
      {topo && (
        <>
          <TreeGroup label={member && connection.kind !== "server" ? t("sidebar.databasesFrom", { member }) : t("sidebar.databases")}>
            {() => (member ? <DatabaseList connectionId={connection.id} /> : <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>)}
          </TreeGroup>
          <TreeGroup label={t("sidebar.roles")}>
            {() => (member ? <RoleList connectionId={connection.id} /> : <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>)}
          </TreeGroup>
        </>
      )}
```

(остальные тесты `CatalogNodes.*.test.tsx`, которые ждут группы только при `member`, обновить по новому поведению.)

`useConnectionEvents.ts`:

```ts
function supersede(qc: QueryClient, key: readonly unknown[]) {
  void qc.cancelQueries({ queryKey: key, exact: true, revert: true });
}

/** Каноническая запись события: порядок ключей не зависит от источника (ответ команды или событие). */
const signature = (e: TopologyEvent) => JSON.stringify(e, Object.keys(e).sort());
/** Сколько верхних записей кэша сверяются с новыми событиями (ответ `list_topology_events` мог опередить событие). */
const DEDUPE_WINDOW = 50;
```

и в `applyTopology` запись журнала:

```ts
      qc.setQueryData<TopologyEvent[]>(eventsKey(id), (prev = []) => {
        // Ответ list_topology_events, посчитанный в Rust после записи события, мог прийти раньше самого
        // события: такое событие в кэше уже есть и второй раз не добавляется.
        const seen = new Set(prev.slice(0, DEDUPE_WINDOW).map(signature));
        const fresh = p.newEvents.filter((e) => !seen.has(signature(e)));
        return [...[...fresh].reverse(), ...prev].slice(0, EVENTS_CAP);
      });
```

`styles.css`: `button.danger { color: var(--danger-text); }`, `.tree-error { … color: var(--danger-text); … }`.

`plainInputs.ts`: комментарий «адреса, имена топиков, пути, JSON» → «адреса, имена объектов, пути, JSON».

- [ ] **Step 4: Прогнать проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "fix(ui): «Переподключить» для живого подключения, группы каталога переживают пропажу узла, журнал без дублей

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Форма подключения: «оставить сохранённый» только при сохранённом секрете, TLS при disable, порты Patroni, плюральные шаги

**Files:**
- Modify: `src/components/connections/ConnectionForm.tsx` (+ `.test.tsx`), `src/components/connections/PgSection.tsx`, `src/components/connections/NodesSection.tsx`, `src/components/connections/fields.tsx`, `src/components/connections/profile.ts` (+ `.test.ts`), `src/components/connections/tree/ImportDialog.tsx`, `src/components/connections/TestPanel.test.tsx`, `src-tauri/src/readiness/check.rs`, `src/locales/en.json`, `src/locales/ru.json`
- Test: vitest в перечисленных `.test.ts(x)`, `cargo test` для `readiness::check`

**Interfaces:**
- Consumes: `api.connectionSecrets` (задача 3), `SecretsSet`, `visibleProfile`, `portProblem`.
- Produces: `SecretRow` принимает `saved: boolean` вместо `isNew`; `PgSection`/`NodesSection` принимают `saved: SecretsSet`; `visibleProfile` отдаёт `hosts: []` для Patroni; ключи `test.patroniOk_*`, `test.discoveryOk_*`, `connections.form.tlsDroppedOnDisable`.

- [ ] **Step 1: Падающие тесты**

`src/components/connections/profile.test.ts`:

```ts
  it("у профиля Patroni скрытые узлы в Rust не уходят: порт из прежнего вида не ломает сохранение", () => {
    const p: ConnectionProfile = { ...withKind(newProfile(), "patroni"), hosts: [{ host: "h", port: 70000 }] };
    expect(visibleProfile(p).hosts).toEqual([]);
    expect(portProblem(visibleProfile(p))).toBeNull();
  });
```

`src/components/connections/ConnectionForm.test.tsx` (в `vi.mock` добавить `connectionSecrets: vi.fn()`; в `beforeEach` — `vi.mocked(api.connectionSecrets).mockResolvedValue({ pgPassword: false, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false })`):

```tsx
  it("«оставить сохранённый» и «Удалить сохранённый» показываются только у сохранённых секретов", async () => {
    vi.mocked(api.connectionSecrets).mockResolvedValue({ pgPassword: true, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false });
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "оставить сохранённый"));
    expect(screen.getByRole("checkbox", { name: /Удалить сохранённый.*Пароль/ })).toBeInTheDocument();
    expect(api.connectionSecrets).toHaveBeenCalledWith("c1");
  });

  it("у подключения без сохранённых секретов поле пароля — как у нового", async () => {
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledWith("c1"));
    expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "");
    expect(screen.queryByRole("checkbox", { name: /Удалить сохранённый/ })).not.toBeInTheDocument();
  });

  it("sslmode=disable при заданном CA предупреждает, что настройки TLS будут удалены", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), pg: { ...newProfile().pg, tls: { caPath: "/etc/ca.pem", clientCert: null } } }} tabId="t" />);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("sslmode"), "disable");
    expect(screen.getByRole("note")).toHaveTextContent("Настройки TLS (CA, клиентский сертификат) при сохранении будут удалены — верните sslmode, чтобы их оставить");
  });
```

Существующие тесты формы, которые ждали плейсхолдер у любого существующего подключения, перевести на мок `connectionSecrets` с нужными флагами. Доступное имя флажка — текущее `passwordClearAria` (`{{label}}` с разделом); регулярное выражение подобрать по нему.

`src/components/connections/TestPanel.test.tsx`: проверить тексты шагов с числом 1 и 5 через `tr`: события `discovery` с `message: { key: "test.discoveryOk", params: { count: "1" }, text: "1 nodes found" }` → «Найден 1 узел», `count: "5"` → «Найдено 5 узлов»; `patroniOk` с `count: "3", leader: "pg-1"` → «3 участника, лидер pg-1».

`src-tauri/src/readiness/check.rs`, тест `skipped_steps_say_why` дополнить (или новый тест): событие `Discovery` несёт `message.params["count"]`, а не `members`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm exec vitest run src/components/connections && cargo test --manifest-path src-tauri/Cargo.toml --lib readiness::check`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`profile.ts::visibleProfile`:

```ts
/** Профиль, каким его видит пользователь: без TLS при `disable` и без узлов у Patroni (Rust их всё равно очищает,
 * а порт из прежнего вида формы иначе ломал бы разбор аргументов команды). */
export function visibleProfile(p: ConnectionProfile): ConnectionProfile {
  const hosts = p.kind === "patroni" ? [] : p.hosts;
  const tls = p.pg.sslMode === "disable" ? { caPath: null, clientCert: null } : p.pg.tls;
  return { ...p, hosts, pg: { ...p.pg, tls } };
}
```

`fields.tsx::SecretRow`: проп `isNew: boolean` → `saved: boolean`; `placeholder={props.saved ? t("connections.form.passwordKeep") : ""}`; флажок «Удалить сохранённый» — `{props.saved && (…)}`.

`PgSection`/`NodesSection`: проп `isNew` → `saved: SecretsSet`; каждому `SecretRow` передавать соответствующий флаг (`saved.pgPassword`, `saved.pgKeyPassword`, `saved.patroniPassword`, `saved.patroniKeyPassword`).

`ConnectionForm.tsx`:

```tsx
  const NO_SECRETS: SecretsSet = { pgPassword: false, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false };
  // Какие секреты сохранены: у нового и копии — никаких (запрос не нужен).
  const savedSecrets = useQuery({
    queryKey: ["connectionSecrets", initial.id],
    queryFn: () => api.connectionSecrets(initial.id),
    enabled: !isNew,
  });
  const saved = savedSecrets.data ?? NO_SECRETS;
```

(`NO_SECRETS` — константа модуля); `secrets()`:

```tsx
  const secrets = (): SecretsInput => ({
    pgPassword: secretValue(pgSecrets.password),
    // При sslmode=disable сертификат из профиля убирается (visibleProfile), и Rust удаляет пароль ключа вместе
    // с ним: значение поля здесь роли не играет, оно уходит как есть.
    pgKeyPassword: secretValue(pgSecrets.keyPassword),
    patroniPassword: secretValue(apiSecrets.password),
    patroniKeyPassword: secretValue(apiSecrets.keyPassword),
  });
```

после сохранения: `void queryClient.invalidateQueries({ queryKey: ["connectionSecrets", saved.id], exact: true });` (переменная `saved` результата `saveConnection` — переименовать во избежание тени: `const stored = await api.saveConnection(...)`). `ImportDialog.tsx` после применения — в `Promise.all` добавить `queryClient.invalidateQueries({ queryKey: ["connectionSecrets"] })`.

`PgSection.tsx` после `<select>` sslmode:

```tsx
      {pg.sslMode === "disable" && (pg.tls.caPath !== null || pg.tls.clientCert !== null) && (
        <p className="muted" role="note">{t("connections.form.tlsDroppedOnDisable")}</p>
      )}
```

Словари: `connections.form.tlsDroppedOnDisable` — en «TLS settings (CA, client certificate) will be removed on save — switch sslmode back to keep them», ru «Настройки TLS (CA, клиентский сертификат) при сохранении будут удалены — верните sslmode, чтобы их оставить».

Плюральные шаги. `check.rs`:

```rust
                    Some(ltext!(
                        "test.patroniOk",
                        "{{count}} members, leader {{leader}}",
                        count = t.members.len(),
                        leader = leader
                    )),
…
        Some(ltext!(
            "test.discoveryOk",
            "{{count}} nodes found",
            count = topology.members.len()
        )),
```

en: удалить `test.patroniOk`/`test.discoveryOk`, добавить `patroniOk_one: "{{count}} member, leader {{leader}}"`, `patroniOk_other: "{{count}} members, leader {{leader}}"`, `discoveryOk_one: "{{count}} node found"`, `discoveryOk_other: "{{count}} nodes found"`. ru: `patroniOk_one: "{{count}} участник, лидер {{leader}}"`, `_few: "{{count}} участника, лидер {{leader}}"`, `_many: "{{count}} участников, лидер {{leader}}"`, `_other: "{{count}} участника, лидер {{leader}}"`; `discoveryOk_one: "Найден {{count}} узел"`, `_few: "Найдено {{count}} узла"`, `_many: "Найдено {{count}} узлов"`, `_other: "Найдено {{count}} узла"`.

- [ ] **Step 4: Прогнать проверки**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm build && cargo test --manifest-path src-tauri/Cargo.toml --lib readiness::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src src-tauri/src/readiness
git commit -m "fix(ui): форма — «оставить сохранённый» только при сохранённом секрете, предупреждение про TLS при disable, порты Patroni, склонение шагов проверки

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: CI, стенд и устаревшие комментарии

**Files:**
- Modify: `.github/workflows/ci.yml`, `dev/docker-compose.yml`, `src-tauri/build.rs`, `src-tauri/src/logging.rs`, `src-tauri/src/policy/mod.rs`, `README.md` (если в нём описана матрица CI или адреса стенда)
- Test: `pnpm test:scripts` (парсинг YAML не ломается), `scripts/wait-stand.sh` после перезапуска стенда — НЕ выполнять `down`: порты применятся при следующем `docker compose up -d` владельцем; в отчёте это отметить

**Interfaces:** нет новых.

- [ ] **Step 1: CI**

`.github/workflows/ci.yml`: в `platforms` `os: [macos-14, macos-15-intel, windows-2022]` с комментарием «macos-15-intel — x86_64, как в release.yml» (в `release.yml` раннер Intel уже `macos-15-intel`); `timeout-minutes: 90` у обеих задач (`linux`, `platforms`) с комментарием, что обрыв по таймауту пропускает сохранение кэша rust-cache. Проверить YAML: `ruby -ryaml -e 'YAML.load_file(ARGV[0])' .github/workflows/ci.yml` или `python3 -c 'import yaml,sys; yaml.safe_load(open(sys.argv[1]))' .github/workflows/ci.yml`.

- [ ] **Step 2: Стенд**

`dev/docker-compose.yml`: все шесть `ports:` с префиксом `127.0.0.1:` (например `"127.0.0.1:5441:5432"`, `"127.0.0.1:8011:8008"`), комментарий над первым: «Стенд слушает только loopback: учётные данные известные». `localhost` в тестах и `scripts/wait-stand.sh` остаётся — он резолвится и в 127.0.0.1. Запущенный стенд не трогать: правка применится при следующем `docker compose up -d` владельцем.

- [ ] **Step 3: Комментарии и build.rs**

- `src-tauri/src/logging.rs:2`: «Секреты в логи не пишутся: отладочный вывод профилей и ввода формы их скрывает (`Debug` у `SecretsInput`, `ConnectionInput`, `ParsedConnection`; тесты `debug_output_hides_secrets` в commands/connections.rs и conn/connstr.rs).»
- `src-tauri/src/policy/mod.rs:1`: «(спец. P1 §2, docs/admin.md)» вместо «(спец. M3 §5)».
- `src-tauri/build.rs::git_commit`: после `rerun-if-changed=../.git/HEAD` добавить

```rust
    // HEAD обычно ссылается на ветку: без слежения за самой веткой коммит в «О программе» устаревал бы
    // до следующей смены HEAD.
    if let Ok(head) = std::fs::read_to_string("../.git/HEAD")
        && let Some(reference) = head.trim().strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=../.git/{reference}");
    }
```

- [ ] **Step 4: Прогнать проверки**

Run: `pnpm test:scripts && cargo build --manifest-path src-tauri/Cargo.toml && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .github dev src-tauri/build.rs src-tauri/src/logging.rs src-tauri/src/policy/mod.rs README.md
git commit -m "ci: Intel-раннер macOS и запас по времени; стенд только на loopback; комментарии и слежение build.rs за веткой

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Самопроверка плана

- Покрытие: каждый пункт списка «до P1b» из `final-review.md` (секреты/атомарность, failover, UI и i18n, docs/CI/стенд) и все шесть Minor из `final-rereview.md` либо в задаче, либо в разделе «Отложено сознательно» с причиной.
- Типы: `PoolPurpose`, `SecretsSet`, `api.reconnect`/`api.connectionSecrets`, `sidebar.reopen` вводятся в задаче 3 и используются в 4–5; `visibleProfile`/`portProblem` — существующие имена.
- Порядок: 1 → 2 → 3 (Rust) → 4 → 5 (frontend, зависят от 3) → 6 (независима).
