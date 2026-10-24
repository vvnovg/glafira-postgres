# P1b — SSH-туннель, вкладки объектов, DDL, bastion и матрица версий: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Завершить этап P1 по спецификации `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`: подключение через SSH-туннель системным OpenSSH (ключ, агент, пароль и пассфраза через askpass, подтверждение ключа хоста), вкладки таблицы, представления, функции и последовательности с генератором DDL, стенд с `bastion` и матрицей версий PG 13–18, руководство.

**Architecture:** Туннель — один процесс `ssh -N -D 127.0.0.1:<порт>` на подключение (SOCKS5 с разрешением имён на бастионе; отступление 1): соединения с PostgreSQL идут через SOCKS5 (`tokio-socks`) с TLS-проверкой по исходному имени, клиент Patroni — через `reqwest` с прокси `socks5h://`. Пароли и пассфразы отдаёт сам бинарь в режиме askpass; ключ хоста подтверждается через отчёт askpass и команду `trust_ssh_host_key`. Супервизор туннеля следит за процессом, перезапускает его с паузами и вытесняет пулы. Вкладки объектов — новые запросы каталога (`catalog/{table,view,function,sequence}.rs`) и чистый генератор DDL (`catalog/ddl.rs`) со снимками. Стенд получает `bastion` (ключи генерируются, не коммитятся) и профиль `matrix`.

**Tech Stack:** Tauri 2, Rust (edition 2024): tokio (`process`), tokio-socks 0.5, tokio-postgres 0.7 + postgres-openssl, reqwest 0.13 (`socks`); React 19 + TS + TanStack Query + Zustand + i18next; Docker Compose (OpenSSH-сервер в `bastion`, postgres:13…18).

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`), Glafira for Kafka (`/Users/vvnovg/kafka-explorer`) — только для чтения. Команды выполняются из `$P`.

## Global Constraints

- PostgreSQL 13+, Patroni 3.0+ (включая 4.x). Платформы: macOS (arm64, x64), Windows x64, Linux x64. OpenSSH 8.4+ для `password` и `key` с пассфразой (`SSH_ASKPASS_REQUIRE`); `agent` и ключ без пассфразы — на любой версии.
- Идентификаторы: `PRODUCT_NAME = "Glafira for PostgreSQL"`, бинарь `glafira-pg`, `KEYCHAIN_SERVICE = "glafira-postgres"`, `PG_APPLICATION_NAME = "glafira-pg"`.
- Секреты — только в keychain, ключ `<id подключения>/<поле>`; поля P1: `pgPassword`, `pgKeyPassword`, `patroniPassword`, `patroniKeyPassword`, `sshPassword`, `sshPassphrase`. В `connections.json`, файлах экспорта, архиве диагностики и логах (в том числе логах `ssh -E`) секретов нет.
- Соединение с PG: `application_name = glafira-pg`, `options = -c default_transaction_read_only=on -c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO` (последние два — отступление 11); при туннеле TCP-поток — через SOCKS5 к `127.0.0.1:<порт туннеля>`, TLS и `~/.pgpass` — по исходному имени хоста.
- Пул: не больше 3 соединений на ключ (подключение, участник, БД) для каталога плюс одно для опроса; опрос топологии раз в 10 с; HTTP-запрос к Patroni — таймаут 5 с, обход `/cluster` — 20 с.
- Все команды Tauri возвращают `Result<T, AppError>`; `AppError { code, key, params, message, details?, retryable }` camelCase, `code` SCREAMING_SNAKE_CASE. Новые коды: `SSH_ERROR` (`params.kind`: `notFound` | `tooOld` | `hostKeyUnknown` | `hostKeyChanged` | `authFailed` | `forwardFailed` | `exited`), `FEATURE_UNAVAILABLE`.
- Пользовательские тексты из Rust — только `ltext!` / `app_err!` с английским шаблоном, равным тексту `src/locales/en.json` (плюральный ключ — форма `_other`, параметр `count`); переводы — `src/locales/{en,ru}.json`; ru == en перечисляется в `docs/i18n/same-as-en.json`; `hints` — последний верхний ключ; кириллица в литералах Rust вне `#[cfg(test)]` запрещена (`pnpm i18n:check`). Комментарии — по-русски.
- Ни одна операция не блокирует UI: keychain, файлы, запуск процессов — в `blocking`/`spawn_blocking`; у каждой сетевой операции и ожидания процесса — таймаут. Аргументы `ssh` собираются только из проверенных значений (без пробелов, без ведущего `-`): командная строка не должна допускать подстановку опций.
- Изменяющих операций с БД и Patroni нет. Файлы `~/.ssh/config` и `~/.ssh/known_hosts` приложение не редактирует (ключ хоста в `known_hosts` пишет сам ssh после ответа `yes`).
- Стенд Docker (`gp-*`) запущен: никогда не выполнять `docker compose down`, не удалять контейнеры и тома, не трогать `ke-kafka`; интеграционные тесты без `GPG_IT_DOCKER`; лидер Patroni — любой узел. Новые сервисы (`bastion`, профиль `matrix`) поднимаются `docker compose -f dev/docker-compose.yml --profile matrix up -d --build` — это не трогает работающие контейнеры.
- Сообщения коммитов — по-русски, `feat(ssh): …`, с последней строкой `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Проверки каждой задачи: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`; у задач со стендом — интеграционные тесты: `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1` плюс переменные задачи (`GPG_IT_BASTION`, `GPG_IT_SSH_KEYS`, `GPG_IT_MATRIX`).

## Отступления от спецификации (решения плана)

1. **Динамический проброс `-D` (SOCKS5) вместо набора `-L`** (спец. §5). Один процесс `ssh -N -T -D 127.0.0.1:<порт>` на подключение: имена разрешаются на бастионе (`socks5h`), новым участникам Patroni и подменам адресов не нужны новые процессы и порты, гонок за локальные порты нет, адреса API Patroni остаются как есть (TLS-имя не подменяется). `ExitOnForwardFailure` по-прежнему относится к привязке локального порта. Задача 11 переписывает §5.
2. **`trust_ssh_host_key(target, fingerprint)`** вместо `(connectionId | profile, fingerprint)`: доверие хранится в памяти по цели ssh (`ssh.target`) до выхода из приложения, а повтор подключения или проверки делает интерфейс обычной командой. Так одна и та же логика работает и для несохранённого профиля в форме, и для открытого подключения.
3. **`AppError.details` — строка** (хвост stderr); вид ошибки ssh — в `params.kind`, хост и отпечаток — в `params.host`, `params.fingerprint`, цель — `params.target`.
4. **Файл `known_hosts` для тестов** задаётся переменной `GLAFIRA_SSH_KNOWN_HOSTS` (добавляется `-o UserKnownHostsFile=…`): интеграционные тесты не трогают `~/.ssh/known_hosts` разработчика. Без переменной ssh использует свой файл по умолчанию.
5. **Тестовые ключи `bastion` генерируются**, а не хранятся в репозитории (`dev/ssh/gen-keys.sh` → `dev/ssh/keys/`, в `.gitignore`): приватный ключ в публичном репозитории поймал бы gitleaks.
6. **Поддельный `ssh` для unit-тестов** — `src-tauri/examples/fake_ssh.rs` (собирается `cargo test`): супервизор, SOCKS-путь, перезапуск и разбор ошибок проверяются без стенда на всех трёх ОС; настоящий OpenSSH — в интеграционных тестах со стендом (Linux).
7. **Размер и статистика партиционированной таблицы во вкладке** считаются суммой по дереву партиций (в списках дерева по-прежнему не считаются). Уточнение по ревью задачи 7: дерево обходится рекурсивно по `pg_inherits`, а не через `pg_partition_tree` — та берёт `AccessShareLock` на каждую партицию до конца запроса, а для партиционированных индексов повторяет это на каждый индекс (N×K блокировок; у таблицы с 2000 партиций и 4 индексами — около 8000, при `max_locks_per_transaction=64` это «out of shared memory»). Счётчики суммируются по всему дереву вместе со строкой самого родителя, `last_*` — максимум по дереву.
8. **Туннель изолирован от разделения соединений ssh** (решение по ревью задачи 2): к фиксированным опциям добавлены `-o ControlMaster=no -o ControlPath=none`. С `ControlMaster auto` в `~/.ssh/config` проброс `-D` ушёл бы в уже открытую сессию пользователя и пережил бы приложение как SOCKS-прокси без аутентификации, либо приложение стало бы мастером и закрытие туннеля рвало бы сессии пользователя.
9. **Ошибки ssh читаются из лога `-E`** (решение по задаче 2): настоящий OpenSSH с `-E` пишет ошибки в лог-файл, а stderr оставляет пустым (кроме баннера сервера). Классификация берёт последние 10 строк stderr и последние 20 строк лога этого запуска.
10. **Пассфраза отдаётся только на собственный запрос ssh** (решение по ревью задачи 3). В режиме «ключ» добавлена `-o PreferredAuthentications=publickey`: без неё после неудачи с ключом ssh переходил бы к keyboard-interactive, где текст запроса выбирает сервер, и скомпрометированный бастион мог бы попросить «passphrase» и получить пассфразу ключа. В askpass запрос пассфразы распознаётся только по началу `Enter passphrase for` (точный текст клиента OpenSSH), а не по вхождению слова; запрос, содержащий слово «passphrase» в другом месте и без слова «password», получает код 1. Пароль по-прежнему распознаётся по вхождению `password`: в режиме «пароль» `PreferredAuthentications=password` не оставляет серверу своего текста. Если переменная `GLAFIRA_ASKPASS_SECRET_FILE` задана, askpass никогда не обращается к keychain (пустое хранилище при отсутствующем или негодном файле).
11. **DDL воспроизводим и безопасен** (решения по ревью задачи 8). а) К параметрам сессии добавлены `-c search_path=pg_catalog -c DateStyle=ISO`: иначе `pg_get_constraintdef`/`pg_get_triggerdef`/`pg_get_expr`/`pg_get_viewdef` печатают имена из `search_path` без схемы (`REFERENCES customers(id)` для `public.customers`), и один и тот же объект даёт разный DDL в зависимости от роли и базы, а граница партиции под `SQL, DMY` печатается как `'01/03/2026'`; запросы приложения от `search_path` не зависят. б) PostgreSQL принимает, но игнорирует `NOT VALID` внутри `CREATE TABLE` (ограничение оказывается проверенным), поэтому все внешние ключи и непроверенные ограничения выводятся после индексов отдельными `ALTER TABLE … ADD CONSTRAINT … ;` (порядок pg_dump; заодно работает самоссылающийся FK на колонку с простым уникальным индексом); `NOT ENFORCED` (PG 18) остаётся в строке. Также: текст внутри `--`-комментариев очищается от переводов строки (имя партиции с `\n` выходило из комментария — аналог CVE-2012-0868 в pg_dump), для партиционированной таблицы из `pg_get_indexdef` убирается `ON ONLY`, строковые литералы с `\` выводятся как `E'…'`, `CREATE UNLOGGED TABLE` по `relpersistence` (поле `unlogged`), для отфильтрованных унаследованных триггеров партиции с состоянием не `O` выводится `ALTER TABLE … TRIGGER`.

---

### Task 1: Модель SSH в профиле, секреты `sshPassword` и `sshPassphrase`, коды ошибок

**Files:**
- Modify: `src-tauri/src/conn/profile.rs`, `src-tauri/src/secrets.rs`, `src-tauri/src/error.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/storage/exchange.rs`, `src-tauri/tests/common/mod.rs`, `src/api/types.ts`, `src/components/connections/profile.ts`, `src/locales/en.json`, `src/locales/ru.json`
- Test: unit-тесты в `profile.rs`, `exchange.rs`, `commands/connections.rs`; `src/components/connections/profile.test.ts`

**Interfaces:**
- Consumes: `ConnectionProfile`, `validate_profile`, `normalize_profile`, `same_connection_settings`, `SecretsInput`, `SecretsSet`, `NeedsPassword`.
- Produces: `conn::profile::{SshAuth { Agent, Key, Password }, SshSettings { target, port: Option<u16>, user: Option<String>, auth, key_path: Option<String> }}`, поле `ConnectionProfile::ssh: Option<SshSettings>`; `secrets::{SSH_PASSWORD = "sshPassword", SSH_PASSPHRASE = "sshPassphrase"}`, `CONNECTION_SECRET_FIELDS` из 6 полей; `ErrorCode::{SshError, FeatureUnavailable}`; `SecretsInput::{ssh_password, ssh_passphrase}`, `SecretsSet::{ssh_password, ssh_passphrase}`, `NeedsPassword::{ssh, ssh_key}`; TS `SshAuth`, `SshSettings`, `ConnectionProfile.ssh`, `SecretsInput`/`SecretsSet` с двумя полями, `ErrorCode` с `"SSH_ERROR" | "FEATURE_UNAVAILABLE"`.

- [ ] **Step 1: Падающие тесты модели**

`src-tauri/src/conn/profile.rs`, модуль `tests`:

```rust
    #[test]
    fn ssh_settings_round_trip_and_old_files_without_ssh() {
        let mut p = sample_profile("a");
        p.ssh = Some(SshSettings {
            target: "bastion-prod".into(),
            port: Some(2222),
            user: Some("tester".into()),
            auth: SshAuth::Key,
            key_path: Some("/home/me/.ssh/id_ed25519".into()),
        });
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["ssh"]["auth"], "key");
        assert_eq!(json["ssh"]["keyPath"], "/home/me/.ssh/id_ed25519");
        let back: ConnectionProfile = serde_json::from_value(json).unwrap();
        assert_eq!(back.ssh, p.ssh);
        // Файл P1a без поля ssh читается: подключение напрямую.
        let mut old = serde_json::to_value(sample_profile("b")).unwrap();
        old.as_object_mut().unwrap().remove("ssh");
        assert_eq!(serde_json::from_value::<ConnectionProfile>(old).unwrap().ssh, None);
    }

    #[test]
    fn ssh_secrets_follow_the_auth_method() {
        let mut p = sample_profile("a");
        assert!(!p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
        p.ssh = Some(SshSettings { target: "b".into(), port: None, user: None, auth: SshAuth::Password, key_path: None });
        assert!(p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
        p.ssh.as_mut().unwrap().auth = SshAuth::Key;
        assert!(!p.uses_secret(SSH_PASSWORD) && p.uses_secret(SSH_PASSPHRASE));
        p.ssh.as_mut().unwrap().auth = SshAuth::Agent;
        assert!(!p.uses_secret(SSH_PASSWORD) && !p.uses_secret(SSH_PASSPHRASE));
    }

    #[test]
    fn ssh_changes_the_connection_settings() {
        let a = sample_profile("a");
        let mut b = a.clone();
        b.ssh = Some(SshSettings { target: "b".into(), port: None, user: None, auth: SshAuth::Agent, key_path: None });
        assert!(!same_connection_settings(&a, &b));
    }

    #[test]
    fn ssh_validation() {
        let ssh = |target: &str, user: Option<&str>, auth: SshAuth, key: Option<&str>| {
            let mut p = sample_profile("a");
            p.ssh = Some(SshSettings {
                target: target.into(),
                port: None,
                user: user.map(String::from),
                auth,
                key_path: key.map(String::from),
            });
            validate_profile(&p).map_err(|e| e.key)
        };
        assert_eq!(ssh("", None, SshAuth::Agent, None), Err("errors.connections.sshTarget".into()));
        // Аргументы ssh: ни пробелов, ни ведущего «-» — иначе цель или пользователь стали бы опцией ssh.
        assert_eq!(ssh("-oProxyCommand=x", None, SshAuth::Agent, None), Err("errors.connections.sshTarget".into()));
        assert_eq!(ssh("bastion x", None, SshAuth::Agent, None), Err("errors.connections.sshTarget".into()));
        assert_eq!(ssh("bastion", Some("-v"), SshAuth::Agent, None), Err("errors.connections.sshUser".into()));
        assert_eq!(ssh("bastion", None, SshAuth::Key, None), Err("errors.connections.sshKeyPath".into()));
        assert_eq!(ssh("bastion", None, SshAuth::Key, Some("relative/key")), Err("errors.connections.pathNotAbsolute".into()));
        assert!(ssh("bastion", Some("tester"), SshAuth::Key, Some("/k/id_ed25519")).is_ok());
        assert!(ssh("user@host-alias.example", None, SshAuth::Password, None).is_ok());
        // Ключ без способа «ключ» не хранится.
        let mut p = sample_profile("a");
        p.ssh = Some(SshSettings { target: "b".into(), port: Some(22), user: None, auth: SshAuth::Agent, key_path: Some("/k".into()) });
        normalize_profile(&mut p);
        assert_eq!(p.ssh.as_ref().unwrap().key_path, None);
    }
```

(`errors.connections.pathNotAbsolute` — существующий ключ `validate_path`; если он называется иначе, использовать настоящее имя.)

`src-tauri/src/commands/connections.rs`, тесты: в `debug_output_hides_secrets` добавить `ssh_password: Some("s1".into()), ssh_passphrase: Some("s2".into())` и проверить, что `s1`/`s2` в выводе нет; в `connection_secrets_report_which_fields_are_saved` добавить `store.set(&secret_key("a", SSH_PASSPHRASE), "pp")` и ожидать `ssh_passphrase: true, ssh_password: false`.

`src-tauri/src/storage/exchange.rs`, тест `needs_password_follows_used_secrets`: профиль с `ssh.auth = Password` и сохранённым `sshPassword` → `needs.ssh == true`, `needs.ssh_key == false`; сериализация `needsPassword` содержит `"ssh": true`.

`src/components/connections/profile.test.ts`:

```ts
  it("новый профиль — без SSH", () => {
    expect(newProfile().ssh).toBeNull();
  });
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib conn::profile commands::connections storage::exchange` и `pnpm exec vitest run src/components/connections/profile.test.ts`
Expected: ошибки компиляции (`SshSettings`, `SSH_PASSWORD`), падение `newProfile().ssh`.

- [ ] **Step 3: Реализация**

`src-tauri/src/secrets.rs`:

```rust
/// Пароль SSH (`ssh.auth = password`).
pub const SSH_PASSWORD: &str = "sshPassword";
/// Пассфраза зашифрованного ключа SSH (`ssh.auth = key`).
pub const SSH_PASSPHRASE: &str = "sshPassphrase";

pub const CONNECTION_SECRET_FIELDS: [&str; 6] = [
    PG_PASSWORD,
    PG_KEY_PASSWORD,
    PATRONI_PASSWORD,
    PATRONI_KEY_PASSWORD,
    SSH_PASSWORD,
    SSH_PASSPHRASE,
];
```

`src-tauri/src/conn/profile.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshAuth {
    Agent,
    Key,
    Password,
}

/// Туннель через системный OpenSSH (спец. P1 §3.1, §5). `target` — хост или алиас из `~/.ssh/config`;
/// `port` и `user` — `None` = как в конфиге ssh; `key_path` — только для `Key`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshSettings {
    pub target: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub user: Option<String>,
    pub auth: SshAuth,
    #[serde(default)]
    pub key_path: Option<String>,
}
```

В `ConnectionProfile` после `pg`: `#[serde(default)] pub ssh: Option<SshSettings>,`. В `uses_secret`:

```rust
            SSH_PASSWORD => self.ssh.as_ref().is_some_and(|s| s.auth == SshAuth::Password),
            SSH_PASSPHRASE => self.ssh.as_ref().is_some_and(|s| s.auth == SshAuth::Key),
```

В `same_connection_settings` добавить `ssh` в деструктуризацию и `&& *ssh == b.ssh`. В `normalize_profile`: `if let Some(s) = &mut p.ssh { s.target = s.target.trim().to_string(); s.user = s.user.take().map(|u| u.trim().to_string()).filter(|u| !u.is_empty()); if s.auth != SshAuth::Key { s.key_path = None; } }`. В `validate_profile` перед возвратом `Ok(())`: `validate_ssh(p.ssh.as_ref())?;`:

```rust
/// Цель и пользователь попадают в командную строку ssh как отдельные аргументы: пробелы и ведущий «-»
/// превратили бы их в опции.
fn ssh_argument_ok(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(char::is_whitespace)
}

fn validate_ssh(ssh: Option<&SshSettings>) -> AppResult<()> {
    let Some(s) = ssh else { return Ok(()) };
    if !ssh_argument_ok(&s.target) {
        return Err(invalid(ltext!(
            "errors.connections.sshTarget",
            "SSH target must be a host name or an alias from ~/.ssh/config without spaces"
        )));
    }
    if let Some(u) = &s.user
        && !ssh_argument_ok(u)
    {
        return Err(invalid(ltext!(
            "errors.connections.sshUser",
            "SSH user name must not contain spaces or start with “-”"
        )));
    }
    if s.port == Some(0) {
        return Err(invalid(ltext!("errors.connections.sshPort", "SSH port must be from 1 to 65535")));
    }
    if s.auth == SshAuth::Key {
        match &s.key_path {
            None => {
                return Err(invalid(ltext!(
                    "errors.connections.sshKeyPath",
                    "Choose the private key file for the “key” method"
                )));
            }
            Some(path) => validate_path(path)?,
        }
    }
    Ok(())
}
```

`src-tauri/src/error.rs`: в `ErrorCode` добавить `/// Туннель ssh: нет клиента, ключ хоста, вход, проброс, процесс (вид — в params.kind). SshError,` и `/// Возможность недоступна в этой среде (например, OpenSSH старше 8.4 для askpass). FeatureUnavailable,`. Если `retryable` вычисляется по коду — `SshError` retryable только для `exited` (задаётся при создании ошибки в задаче 2), `FeatureUnavailable` — нет.

`src-tauri/src/commands/connections.rs`: `SecretsInput` — поля `ssh_password`, `ssh_passphrase` (`Option<String>`, `#[serde(default)]`), `get(field)`/`overrides` — ветки для `SSH_PASSWORD`/`SSH_PASSPHRASE`, `Debug` скрывает оба; `SecretsSet` — `ssh_password`, `ssh_passphrase`; `secrets_set` читает их.

`src-tauri/src/storage/exchange.rs`: `NeedsPassword { …, pub ssh: bool, pub ssh_key: bool }`, `any()` учитывает оба, `needs_password` — `ssh: check(SSH_PASSWORD)?, ssh_key: check(SSH_PASSPHRASE)?`.

`src-tauri/tests/common/mod.rs` `server_profile`: `ssh: None,`. Все конструкторы `ConnectionProfile { … }` в тестах (`sample_profile` и литералы в `profile.rs`, `exchange.rs`, `connector.rs`, `runtime.rs`, `folders.rs`) получают `ssh: None`.

`src/api/types.ts`:

```ts
export type SshAuth = "agent" | "key" | "password";
export interface SshSettings {
  /** Хост или алиас из ~/.ssh/config. */
  target: string;
  port: number | null;
  user: string | null;
  auth: SshAuth;
  keyPath: string | null;
}
```

в `ConnectionProfile`: `ssh: SshSettings | null;`; `SecretsInput` — `sshPassword?: string | null; sshPassphrase?: string | null;`; `SecretsSet` — `sshPassword: boolean; sshPassphrase: boolean;`; `ErrorCode` — `| "SSH_ERROR" | "FEATURE_UNAVAILABLE"`; если есть тип `NeedsPassword`/`ImportPreview` с полями — добавить `ssh`, `sshKey`. `src/components/connections/profile.ts::newProfile` — `ssh: null`. Все фикстуры `ConnectionProfile` во фронтенд-тестах (`src/api/client.channel.test.ts` и другие литералы с `pg: {…}`) получают `ssh: null`. Словари: `errors.connections.{sshTarget, sshUser, sshPort, sshKeyPath}` (ru: «Цель SSH — имя хоста или алиас из ~/.ssh/config без пробелов», «Имя пользователя SSH не может содержать пробелы или начинаться с «-»», «Порт SSH — от 1 до 65535», «Выберите файл приватного ключа для способа «ключ»»).

- [ ] **Step 4: Прогнать проверки**

Run: все проверки из Global Constraints (без стенда).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/api src/components/connections/profile.ts src/components/connections/profile.test.ts src/locales
git commit -m "feat(ssh): настройки туннеля в профиле, секреты sshPassword и sshPassphrase, коды SSH_ERROR и FEATURE_UNAVAILABLE

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Процесс `ssh`, SOCKS5 и сироты (`net/ssh`)

**Files:**
- Create: `src-tauri/src/net/mod.rs`, `src-tauri/src/net/ssh/mod.rs`, `src-tauri/src/net/ssh/process.rs`, `src-tauri/src/net/ssh/socks.rs`, `src-tauri/src/net/ssh/orphans.rs`
- Modify: `src-tauri/Cargo.toml` (tokio `process`, `tokio-socks = "0.5"`, reqwest `socks`), `src-tauri/src/lib.rs` (`pub mod net;`), `src/locales/{en,ru}.json`
- Test: unit-тесты в каждом файле

**Interfaces:**
- Consumes: `SshSettings`, `AppError`.
- Produces:
  - `net::ssh::process::{find_ssh() -> AppResult<PathBuf>, ssh_version(bin) -> AppResult<(u32, u32)>, SshArgs { bin, log_file, socks_port, settings: SshSettings, known_hosts: Option<PathBuf> }, SshArgs::command_line(&self) -> Vec<OsString>, AskpassEnv { connection_id, report_file, trust: Option<String> }, spawn(args, env) -> AppResult<SshChild>, SshChild { pid(), wait_ready(socks_addr, deadline) -> AppResult<()>, stderr_tail(), kill() }, SshErrorKind, classify_stderr(code, stderr, report: Option<&str>) -> SshErrorKind, ssh_error(kind, target, tail) -> AppError, free_port() -> std::io::Result<u16>}`;
  - `net::ssh::socks::connect(proxy: SocketAddr, host: &str, port: u16) -> AppResult<Socks5Stream<TcpStream>>`;
  - `net::ssh::orphans::{register(data_dir, pid, log), unregister(data_dir, pid), reap(data_dir) -> usize}`.
  - Ключи: `errors.ssh.{notFound, tooOld, hostKeyUnknown, hostKeyChanged, authFailed, forwardFailed, exited, socks}`.

- [ ] **Step 1: Зависимости**

`src-tauri/Cargo.toml`: `tokio = { version = "1", features = ["rt", "rt-multi-thread", "net", "time", "macros", "sync", "io-util", "process"] }`, `tokio-socks = "0.5"`, `reqwest = { version = "0.13", default-features = false, features = ["json", "rustls", "socks"] }`. Run `cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses` (tokio-socks — MIT) и `node scripts/third-party.mjs` (или команду из `docs/release-checklist.md`, обновляющую `THIRD_PARTY.md`) — файл обновить, если он изменился.

- [ ] **Step 2: Падающие тесты**

`src-tauri/src/net/ssh/process.rs`, модуль `tests`:

```rust
    fn settings() -> SshSettings {
        SshSettings {
            target: "bastion".into(),
            port: Some(2222),
            user: Some("tester".into()),
            auth: SshAuth::Key,
            key_path: Some("/keys/id_ed25519".into()),
        }
    }

    #[test]
    fn command_line_is_fixed_and_uses_dynamic_forward() {
        let args = SshArgs {
            bin: PathBuf::from("/usr/bin/ssh"),
            log_file: PathBuf::from("/logs/ssh-c1.log"),
            socks_port: 40123,
            settings: settings(),
            known_hosts: Some(PathBuf::from("/tmp/kh")),
        };
        let line: Vec<String> = args.command_line().into_iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            line,
            [
                "-N", "-T", "-E", "/logs/ssh-c1.log",
                "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3",
                "-o", "StrictHostKeyChecking=ask", "-o", "NumberOfPasswordPrompts=1", "-o", "BatchMode=no",
                "-o", "ControlMaster=no", "-o", "ControlPath=none",
                "-o", "UserKnownHostsFile=/tmp/kh",
                "-p", "2222", "-l", "tester", "-i", "/keys/id_ed25519", "-o", "IdentitiesOnly=yes",
                "-D", "127.0.0.1:40123", "--", "bastion",
            ]
        );
        // Способ «агент»: без -i; «пароль» — тоже без -i, но с PreferredAuthentications=password.
        let mut agent = settings();
        agent.auth = SshAuth::Agent;
        agent.key_path = None;
        let line = SshArgs { settings: agent, ..args.clone() }.command_line();
        assert!(!line.iter().any(|a| a == "-i"));
        let mut pw = settings();
        pw.auth = SshAuth::Password;
        pw.key_path = None;
        let line: Vec<String> = SshArgs { settings: pw, ..args }.command_line().into_iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(line.windows(2).any(|w| w == ["-o", "PreferredAuthentications=password"]));
    }

    #[test]
    fn stderr_is_classified() {
        let c = |code: i32, text: &str| classify_stderr(Some(code), text, None);
        assert_eq!(c(255, "tester@bastion: Permission denied (publickey,password).\n"), SshErrorKind::AuthFailed);
        assert!(matches!(c(255, "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\n"), SshErrorKind::HostKeyChanged));
        assert_eq!(c(255, "Host key verification failed.\n"), SshErrorKind::HostKeyUnknown { host: None, fingerprint: None });
        assert!(matches!(c(255, "bind [127.0.0.1]:40123: Address already in use\nCould not request local forwarding.\n"), SshErrorKind::BindFailed));
        assert!(matches!(c(255, "channel 2: open failed: connect failed: Connection refused\n"), SshErrorKind::ForwardFailed));
        assert!(matches!(c(255, "ssh: connect to host bastion port 22: Connection refused\n"), SshErrorKind::Exited));
        // Отчёт askpass точнее stderr: в нём хост и отпечаток.
        let report = "The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:abcDEF+/=.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert_eq!(
            classify_stderr(Some(255), "Host key verification failed.\n", Some(report)),
            SshErrorKind::HostKeyUnknown { host: Some("bastion (10.0.0.5)".into()), fingerprint: Some("SHA256:abcDEF+/=".into()) }
        );
    }

    #[test]
    fn version_is_parsed_from_ssh_v_output() {
        assert_eq!(parse_version("OpenSSH_9.6p1, LibreSSL 3.3.6\n"), Some((9, 6)));
        assert_eq!(parse_version("OpenSSH_8.4p1 Debian-5, OpenSSL 1.1.1k\n"), Some((8, 4)));
        assert_eq!(parse_version("OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2\n"), Some((9, 5)));
        assert_eq!(parse_version("ssh: command not found"), None);
        assert!(askpass_supported((8, 4)) && !askpass_supported((8, 3)));
    }

    #[test]
    fn free_port_is_bindable_again() {
        let p = free_port().unwrap();
        assert!(p > 0);
        std::net::TcpListener::bind(("127.0.0.1", p)).unwrap();
    }
```

`src-tauri/src/net/ssh/socks.rs`, модуль `tests`:

```rust
    /// Поддельный SOCKS5-прокси без аутентификации: CONNECT к имени → TCP к `upstream`, затем прокачка байтов.
    async fn fake_socks(upstream: std::net::SocketAddr) -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 262];
                    s.read_exact(&mut buf[..2]).await.unwrap();
                    let n = buf[1] as usize;
                    s.read_exact(&mut buf[..n]).await.unwrap();
                    s.write_all(&[5, 0]).await.unwrap();
                    s.read_exact(&mut buf[..4]).await.unwrap();
                    assert_eq!((buf[0], buf[1], buf[3]), (5, 1, 3), "CONNECT к доменному имени");
                    s.read_exact(&mut buf[..1]).await.unwrap();
                    let n = buf[0] as usize;
                    s.read_exact(&mut buf[..n]).await.unwrap();
                    assert_eq!(&buf[..n], b"pg-single");
                    s.read_exact(&mut buf[..2]).await.unwrap();
                    assert_eq!(u16::from_be_bytes([buf[0], buf[1]]), 5432);
                    let mut up = tokio::net::TcpStream::connect(upstream).await.unwrap();
                    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn connect_goes_through_the_proxy_with_remote_name_resolution() {
        let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = echo.accept().await.unwrap();
            let mut b = [0u8; 5];
            tokio::io::AsyncReadExt::read_exact(&mut s, &mut b).await.unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut s, &b).await.unwrap();
        });
        let proxy = fake_socks(upstream).await;
        let mut stream = connect(proxy, "pg-single", 5432).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut stream, b"hello").await.unwrap();
        let mut b = [0u8; 5];
        tokio::io::AsyncReadExt::read_exact(&mut stream, &mut b).await.unwrap();
        assert_eq!(&b, b"hello");
    }

    #[tokio::test]
    async fn refused_proxy_is_an_ssh_error() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let e = connect(addr, "pg-single", 5432).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::SshError, "errors.ssh.socks"));
    }
```

`src-tauri/src/net/ssh/orphans.rs`, модуль `tests` (только unix: `#[cfg(unix)]`):

```rust
    #[test]
    fn reap_kills_only_processes_whose_command_line_names_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("ssh-c1.log");
        let marker = log.to_string_lossy().into_owned();
        // Командная строка содержит путь лога — как у настоящего ssh с «-E <лог>».
        let mut ours = std::process::Command::new("sh").args(["-c", &format!("sleep 30 # {marker}")]).spawn().unwrap();
        let mut foreign = std::process::Command::new("sh").args(["-c", "sleep 30 # other"]).spawn().unwrap();
        register(dir.path(), ours.id(), &log).unwrap();
        register(dir.path(), foreign.id(), &dir.path().join("ssh-other.log")).unwrap();
        let file = dir.path().join("tunnels.json");
        assert!(file.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(reap(dir.path()), 1);
        assert!(ours.wait().unwrap().code().is_none(), "наш процесс завершён сигналом");
        assert!(foreign.try_wait().unwrap().is_none(), "чужой процесс с другой командной строкой не тронут");
        foreign.kill().unwrap();
        assert!(!file.exists() || std::fs::read_to_string(&file).unwrap().contains("\"pids\":[]"));
    }
```

- [ ] **Step 3: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib net::`
Expected: ошибка компиляции — модуля `net` нет.

- [ ] **Step 4: Реализация**

`src-tauri/src/net/mod.rs`: `pub mod askpass;` (задача 3 — пока создать пустой файл с `//! Режим askpass (задача 3).`) и `pub mod ssh;`. `src-tauri/src/net/ssh/mod.rs`: `pub mod orphans; pub mod process; pub mod socks; pub mod tunnel;` (`tunnel.rs` — задача 4; пока файл с `//! Супервизор туннеля (задача 4).`).

`src-tauri/src/net/ssh/process.rs`:

```rust
//! Процесс `ssh` (OpenSSH): поиск клиента, аргументы, запуск с askpass, готовность, разбор ошибок (спец. P1 §5).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::conn::profile::{SshAuth, SshSettings};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sync::MutexExt;

/// Сколько последних строк stderr хранится для `details` ошибки.
const STDERR_TAIL: usize = 20;

/// Клиент OpenSSH в PATH; на Windows — ещё и штатное место компонента «Клиент OpenSSH».
pub fn find_ssh() -> AppResult<PathBuf> {
    let name = if cfg!(windows) { "ssh.exe" } else { "ssh" };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if cfg!(windows)
        && let Some(root) = std::env::var_os("SystemRoot")
    {
        dirs.push(PathBuf::from(root).join("System32").join("OpenSSH"));
    }
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            ssh_error(SshErrorKind::NotFound, "", "")
        })
}

/// Версия OpenSSH из `ssh -V` (пишется в stderr): `(major, minor)`.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let rest = text.split("OpenSSH_").nth(1)?;
    let rest = rest.trim_start_matches("for_Windows_");
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// `SSH_ASKPASS_REQUIRE=force` появился в OpenSSH 8.4: без него пароль и пассфраза не спрашиваются через askpass.
pub fn askpass_supported(v: (u32, u32)) -> bool {
    v >= (8, 4)
}

pub async fn ssh_version(bin: &Path) -> AppResult<(u32, u32)> {
    let out = tokio::time::timeout(Duration::from_secs(5), Command::new(bin).arg("-V").output())
        .await
        .map_err(|_| ssh_error(SshErrorKind::Exited, "", "ssh -V timed out"))?
        .map_err(|e| ssh_error(SshErrorKind::Exited, "", &e.to_string()))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    parse_version(&text).ok_or_else(|| ssh_error(SshErrorKind::Exited, "", text.trim()))
}

/// Свободный локальный порт: слушатель на `127.0.0.1:0` открывается и закрывается; при гонке
/// (`ExitOnForwardFailure` на занятом порту) супервизор делает одну повторную попытку с новым портом.
pub fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

#[derive(Debug, Clone)]
pub struct SshArgs {
    pub bin: PathBuf,
    pub log_file: PathBuf,
    pub socks_port: u16,
    pub settings: SshSettings,
    /// Только для тестов (`GLAFIRA_SSH_KNOWN_HOSTS`): свой known_hosts, чтобы не трогать файл пользователя.
    pub known_hosts: Option<PathBuf>,
}

impl SshArgs {
    /// Аргументы без имени программы. Значения проверены `validate_profile` (без пробелов и ведущего «-»),
    /// цель идёт после `--`.
    pub fn command_line(&self) -> Vec<OsString> {
        let s = &self.settings;
        let mut a: Vec<OsString> = vec!["-N".into(), "-T".into(), "-E".into(), self.log_file.clone().into()];
        for o in [
            "ExitOnForwardFailure=yes",
            "ServerAliveInterval=15",
            "ServerAliveCountMax=3",
            "StrictHostKeyChecking=ask",
            "NumberOfPasswordPrompts=1",
            "BatchMode=no",
            // Разделение соединений из ~/.ssh/config (ControlMaster) не используется: иначе порт -D достался бы
            // чужой сессии пользователя и пережил бы приложение (отступление 8).
            "ControlMaster=no",
            "ControlPath=none",
        ] {
            a.push("-o".into());
            a.push(o.into());
        }
        if let Some(kh) = &self.known_hosts {
            a.push("-o".into());
            let mut v = OsString::from("UserKnownHostsFile=");
            v.push(kh);
            a.push(v);
        }
        if let Some(p) = s.port {
            a.push("-p".into());
            a.push(p.to_string().into());
        }
        if let Some(u) = &s.user {
            a.push("-l".into());
            a.push(u.into());
        }
        match s.auth {
            SshAuth::Key => {
                if let Some(k) = &s.key_path {
                    a.push("-i".into());
                    a.push(k.into());
                    a.push("-o".into());
                    a.push("IdentitiesOnly=yes".into());
                }
            }
            SshAuth::Password => {
                a.push("-o".into());
                a.push("PreferredAuthentications=password".into());
            }
            SshAuth::Agent => {}
        }
        a.push("-D".into());
        a.push(format!("127.0.0.1:{}", self.socks_port).into());
        a.push("--".into());
        a.push(s.target.clone().into());
        a
    }
}

/// Окружение askpass (спец. P1 §5.1): секреты отдаёт сам бинарь приложения.
#[derive(Debug, Clone)]
pub struct AskpassEnv {
    pub connection_id: String,
    pub report_file: PathBuf,
    pub trust: Option<String>,
}

pub struct SshChild {
    child: Child,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl SshChild {
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr.lock_safe().join("\n")
    }

    /// Завершился ли процесс (без ожидания); `Some(code)` — код выхода.
    pub fn try_exit_code(&mut self) -> Option<Option<i32>> {
        self.child.try_wait().ok().flatten().map(|s| s.code())
    }

    pub async fn wait(&mut self) -> Option<i32> {
        self.child.wait().await.ok().and_then(|s| s.code())
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    /// Туннель готов, когда порт SOCKS принимает TCP; процесс завершился раньше — ошибка по stderr.
    pub async fn wait_ready(&mut self, socks: std::net::SocketAddr, deadline: Instant) -> Result<(), Option<i32>> {
        loop {
            if let Some(code) = self.try_exit_code() {
                return Err(code);
            }
            if tokio::net::TcpStream::connect(socks).await.is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                self.kill().await;
                return Err(None);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Запуск `ssh`: stdin закрыт (пароли — только через askpass), stderr читается в хвост. На Windows — без
/// консольного окна.
pub fn spawn(args: &SshArgs, env: &AskpassEnv) -> AppResult<SshChild> {
    let mut cmd = Command::new(&args.bin);
    cmd.args(args.command_line())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("SSH_ASKPASS", std::env::current_exe().map_err(|e| ssh_error(SshErrorKind::Exited, "", &e.to_string()))?)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("GLAFIRA_ASKPASS", &env.connection_id)
        .env("GLAFIRA_ASKPASS_REPORT", &env.report_file);
    if std::env::var_os("DISPLAY").is_none() {
        cmd.env("DISPLAY", ":0");
    }
    match &env.trust {
        Some(fp) => {
            cmd.env("GLAFIRA_ASKPASS_TRUST", fp);
        }
        None => {
            cmd.env_remove("GLAFIRA_ASKPASS_TRUST");
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ssh_error(SshErrorKind::Exited, &args.settings.target, &e.to_string()))?;
    let tail: Arc<Mutex<Vec<String>>> = Arc::default();
    if let Some(err) = child.stderr.take() {
        let tail = tail.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut t = tail.lock_safe();
                if t.len() == STDERR_TAIL {
                    t.remove(0);
                }
                t.push(line);
            }
        });
    }
    Ok(SshChild { child, stderr: tail })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshErrorKind {
    NotFound,
    TooOld,
    HostKeyUnknown { host: Option<String>, fingerprint: Option<String> },
    HostKeyChanged,
    AuthFailed,
    /// Локальный порт SOCKS занят (гонка за портом): супервизор повторяет с новым портом.
    BindFailed,
    ForwardFailed,
    Exited,
}

impl SshErrorKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "notFound",
            Self::TooOld => "tooOld",
            Self::HostKeyUnknown { .. } => "hostKeyUnknown",
            Self::HostKeyChanged => "hostKeyChanged",
            Self::AuthFailed => "authFailed",
            Self::BindFailed | Self::ForwardFailed => "forwardFailed",
            Self::Exited => "exited",
        }
    }
}

/// Отчёт askpass о неизвестном ключе хоста: хост в кавычках и отпечаток `SHA256:…`.
pub fn parse_host_key_report(report: &str) -> (Option<String>, Option<String>) {
    let host = report
        .split_once("authenticity of host '")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(h, _)| h.to_string());
    let fingerprint = report
        .split_whitespace()
        .find(|w| w.starts_with("SHA256:"))
        .map(|w| w.trim_end_matches('.').to_string());
    (host, fingerprint)
}

pub fn classify_stderr(code: Option<i32>, stderr: &str, report: Option<&str>) -> SshErrorKind {
    let _ = code;
    let s = stderr;
    if let Some(r) = report
        && r.contains("authenticity of host")
    {
        let (host, fingerprint) = parse_host_key_report(r);
        return SshErrorKind::HostKeyUnknown { host, fingerprint };
    }
    if s.contains("REMOTE HOST IDENTIFICATION HAS CHANGED") {
        SshErrorKind::HostKeyChanged
    } else if s.contains("Host key verification failed") {
        SshErrorKind::HostKeyUnknown { host: None, fingerprint: None }
    } else if s.contains("Permission denied") {
        SshErrorKind::AuthFailed
    } else if s.contains("Address already in use") || s.contains("Could not request local forwarding") {
        SshErrorKind::BindFailed
    } else if s.contains("open failed") || s.contains("connect failed") {
        SshErrorKind::ForwardFailed
    } else {
        SshErrorKind::Exited
    }
}

/// `SSH_ERROR` с видом в `params.kind` (спец. §5.2, отступление 3); `tail` — хвост stderr в `details`.
pub fn ssh_error(kind: SshErrorKind, target: &str, tail: &str) -> AppError {
    let mut e = match &kind {
        SshErrorKind::NotFound => app_err!(
            ErrorCode::SshError,
            "errors.ssh.notFound",
            "OpenSSH client (ssh) was not found: install it (Windows: the “OpenSSH Client” optional feature)"
        ),
        SshErrorKind::TooOld => app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.ssh.tooOld",
            "OpenSSH 8.4 or newer is needed for password and passphrase prompts; use an agent or a key without a passphrase"
        ),
        SshErrorKind::HostKeyUnknown { host, fingerprint } => app_err!(
            ErrorCode::SshError,
            "errors.ssh.hostKeyUnknown",
            "The host key of {{host}} is not known yet ({{fingerprint}})",
            host = host.clone().unwrap_or_else(|| target.to_string()),
            fingerprint = fingerprint.clone().unwrap_or_default()
        ),
        SshErrorKind::HostKeyChanged => app_err!(
            ErrorCode::SshError,
            "errors.ssh.hostKeyChanged",
            "The host key of {{target}} has changed: check ~/.ssh/known_hosts yourself",
            target = target
        ),
        SshErrorKind::AuthFailed => app_err!(
            ErrorCode::SshError,
            "errors.ssh.authFailed",
            "SSH login to {{target}} was refused: check the user, the key or the password",
            target = target
        ),
        SshErrorKind::BindFailed | SshErrorKind::ForwardFailed => app_err!(
            ErrorCode::SshError,
            "errors.ssh.forwardFailed",
            "The bastion {{target}} could not reach the node (the forward failed)",
            target = target
        ),
        SshErrorKind::Exited => app_err!(
            ErrorCode::SshError,
            "errors.ssh.exited",
            "The ssh process to {{target}} exited",
            target = target
        ),
    };
    e.params.insert("kind".into(), kind.name().into());
    e.params.insert("target".into(), target.into());
    if !tail.trim().is_empty() {
        e = e.with_details(tail.trim().to_string());
    }
    if matches!(kind, SshErrorKind::Exited | SshErrorKind::ForwardFailed) {
        e.retryable = true;
    }
    e
}
```

(Поля `AppError.params`/`retryable`/`with_details` — существующие; если `params` недоступен для прямой вставки, использовать имеющийся конструктор `app_err!` с параметрами и затем `with_param` — посмотреть `error.rs`.)

`src-tauri/src/net/ssh/socks.rs`:

```rust
//! Соединение с узлом через SOCKS5-порт туннеля (`ssh -D`): имя узла разрешает бастион.

use std::net::SocketAddr;

use tokio::net::TcpStream;
use tokio_socks::tcp::Socks5Stream;

use crate::conn::pg::tcp_connect;
use crate::error::{AppError, AppResult, ErrorCode};

pub type SocksStream = Socks5Stream<TcpStream>;

/// TCP к `proxy` (с keepalive, как прямое соединение), затем SOCKS5 CONNECT к `host:port` по имени.
pub async fn connect(proxy: SocketAddr, host: &str, port: u16) -> AppResult<SocksStream> {
    let tcp = tcp_connect(&proxy.ip().to_string(), proxy.port())
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))?;
    Socks5Stream::connect_with_socket(tcp, (host, port))
        .await
        .map_err(|e| socks_error(host, port, &e.to_string()))
}

fn socks_error(host: &str, port: u16, cause: &str) -> AppError {
    let mut e = app_err!(
        ErrorCode::SshError,
        "errors.ssh.socks",
        "The bastion could not connect to {{host}}:{{port}}",
        host = host,
        port = port
    )
    .with_details(cause.to_string());
    e.params.insert("kind".into(), "forwardFailed".into());
    e.retryable = true;
    e
}
```

(`tcp_connect` из `conn/pg.rs` стал `pub(crate)` в этапе укрепления — сделать `pub(crate)` доступным, модули в одном крейте.)

`src-tauri/src/net/ssh/orphans.rs`:

```rust
//! Сироты ssh после сбоя приложения (спец. P1 §5): PID и путь лога в `tunnels.json` (0600) в каталоге данных.
//! При старте процессы из файла завершаются, если в их командной строке есть путь их лога (чужой процесс с
//! тем же PID не пострадает).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sync::MutexExt;

const FILE: &str = "tunnels.json";
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    pids: Vec<Entry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    pid: u32,
    log: PathBuf,
}

fn read(dir: &Path) -> Registry {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write(dir: &Path, r: &Registry) -> std::io::Result<()> {
    let path = dir.join(FILE);
    let data = serde_json::to_vec(r)?;
    crate::storage::fs::write_private(&path, &data)
}

pub fn register(dir: &Path, pid: u32, log: &Path) -> std::io::Result<()> {
    let _g = LOCK.lock_safe();
    let mut r = read(dir);
    r.pids.retain(|e| e.pid != pid);
    r.pids.push(Entry { pid, log: log.to_path_buf() });
    write(dir, &r)
}

pub fn unregister(dir: &Path, pid: u32) -> std::io::Result<()> {
    let _g = LOCK.lock_safe();
    let mut r = read(dir);
    r.pids.retain(|e| e.pid != pid);
    write(dir, &r)
}

/// Командная строка процесса (пусто — процесса нет или узнать нельзя).
fn command_line(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\").CommandLine"),
            ])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }
}

fn terminate(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status();
    }
}

/// Завершает сирот и очищает файл; возвращает число завершённых.
pub fn reap(dir: &Path) -> usize {
    let _g = LOCK.lock_safe();
    let r = read(dir);
    let mut killed = 0;
    for e in &r.pids {
        if command_line(e.pid).contains(&e.log.to_string_lossy().into_owned()) {
            terminate(e.pid);
            killed += 1;
        }
    }
    let _ = write(dir, &Registry::default());
    killed
}
```

(`crate::storage::fs::write_private` — запись файла с правами 0600; если в `storage/fs.rs` такой функции нет, добавить её рядом с атомарной записью: `std::fs::write` + на unix `set_permissions(0o600)`.) Словари: `errors.ssh.{notFound, tooOld, hostKeyUnknown, hostKeyChanged, authFailed, forwardFailed, exited, socks}` на en (как в шаблонах) и ru («Клиент OpenSSH (ssh) не найден: установите его (Windows: компонент «Клиент OpenSSH»)», «Для запроса пароля и пассфразы нужен OpenSSH 8.4 или новее; используйте агент или ключ без пассфразы», «Ключ хоста {{host}} ещё не известен ({{fingerprint}})», «Ключ хоста {{target}} изменился: проверьте ~/.ssh/known_hosts сами», «Вход по SSH на {{target}} отклонён: проверьте пользователя, ключ или пароль», «Бастион {{target}} не смог достучаться до узла (проброс не удался)», «Процесс ssh к {{target}} завершился», «Бастион не смог подключиться к {{host}}:{{port}}»).

- [ ] **Step 5: Прогнать проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib net::`, затем все проверки из Global Constraints.
Expected: PASS (тест сирот — на unix).

- [ ] **Step 6: Commit**

```bash
git add src-tauri src/locales THIRD_PARTY.md
git commit -m "feat(ssh): процесс ssh с динамическим пробросом, SOCKS5 к узлу, разбор ошибок и сироты туннелей

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Режим askpass

**Files:**
- Create: `src-tauri/src/net/askpass.rs`
- Modify: `src-tauri/src/main.rs`, `src-tauri/src/lib.rs` (`pub fn askpass_main`)
- Test: unit-тесты в `askpass.rs`

**Interfaces:**
- Consumes: `KeyringStore`, `secret_key`, `SSH_PASSWORD`, `SSH_PASSPHRASE`, `parse_host_key_report`.
- Produces: `net::askpass::{Answer { Secret(&'static str), HostKey { fingerprint: Option<String> }, Unknown }, decide(prompt: &str) -> Answer, respond(prompt, env: AskpassVars, secrets: &dyn SecretStore) -> (i32, String), run(args) -> i32}`; `glafira_pg_lib::askpass_main() -> i32`.

- [ ] **Step 1: Падающие тесты**

```rust
    #[test]
    fn prompts_are_recognised() {
        assert_eq!(decide("tester@bastion's password: "), Answer::Secret(SSH_PASSWORD));
        assert_eq!(decide("Enter passphrase for key '/k/id_ed25519': "), Answer::Secret(SSH_PASSPHRASE));
        assert_eq!(
            decide("The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?"),
            Answer::HostKey { fingerprint: Some("SHA256:abc".into()) }
        );
        assert_eq!(decide("Please type 'yes', 'no' or the fingerprint: "), Answer::HostKey { fingerprint: None });
        assert_eq!(decide("Verification code: "), Answer::Unknown);
    }

    #[test]
    fn responses_follow_the_spec() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("report");
        let secrets = MemoryStore::default();
        secrets.set(&secret_key("c1", SSH_PASSWORD), "pw").unwrap();
        let vars = |trust: Option<&str>| AskpassVars {
            connection_id: "c1".into(),
            report_file: report.clone(),
            trust: trust.map(String::from),
        };
        assert_eq!(respond("tester@bastion's password: ", &vars(None), &secrets), (0, "pw".into()));
        // Секрета нет — код 1, stdout пуст.
        assert_eq!(respond("Enter passphrase for key '/k': ", &vars(None), &secrets), (1, String::new()));
        let prompt = "The authenticity of host 'b (1.2.3.4)' can't be established.\nED25519 key fingerprint is SHA256:abc.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
        assert_eq!(respond(prompt, &vars(Some("SHA256:abc")), &secrets), (0, "yes".into()));
        assert_eq!(respond(prompt, &vars(Some("SHA256:other")), &secrets), (0, "no".into()));
        assert_eq!(std::fs::read_to_string(&report).unwrap(), prompt);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&report).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(respond("Verification code: ", &vars(None), &secrets), (1, String::new()));
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib net::askpass`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`src-tauri/src/net/askpass.rs`:

```rust
//! Режим askpass (спец. P1 §5.1): ssh запускает этот же бинарь с текстом запроса единственным аргументом,
//! а окружение `GLAFIRA_ASKPASS*` говорит, чей секрет отдать. Tauri и WebView не инициализируются.

use std::path::PathBuf;

use crate::net::ssh::process::parse_host_key_report;
use crate::secrets::{SSH_PASSPHRASE, SSH_PASSWORD, SecretStore, secret_key};

#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    Secret(&'static str),
    HostKey { fingerprint: Option<String> },
    Unknown,
}

pub fn decide(prompt: &str) -> Answer {
    let lower = prompt.to_lowercase();
    if lower.contains("authenticity of host") || lower.contains("yes', 'no' or the fingerprint") {
        let (_, fingerprint) = parse_host_key_report(prompt);
        return Answer::HostKey { fingerprint };
    }
    if lower.contains("passphrase") {
        return Answer::Secret(SSH_PASSPHRASE);
    }
    if lower.contains("password") {
        return Answer::Secret(SSH_PASSWORD);
    }
    Answer::Unknown
}

pub struct AskpassVars {
    pub connection_id: String,
    pub report_file: PathBuf,
    pub trust: Option<String>,
}

/// Код выхода и stdout. Секрет печатается без перевода строки, как ждёт ssh.
pub fn respond(prompt: &str, vars: &AskpassVars, secrets: &dyn SecretStore) -> (i32, String) {
    match decide(prompt) {
        Answer::Secret(field) => match secrets.get(&secret_key(&vars.connection_id, field)) {
            Ok(Some(v)) if !v.is_empty() => (0, v),
            _ => (1, String::new()),
        },
        Answer::HostKey { fingerprint } => {
            let trusted = fingerprint.is_some() && fingerprint == vars.trust;
            if trusted {
                (0, "yes".into())
            } else {
                let _ = crate::storage::fs::write_private(&vars.report_file, prompt.as_bytes());
                (0, "no".into())
            }
        }
        Answer::Unknown => (1, String::new()),
    }
}

/// Точка входа режима askpass; `args` — аргументы процесса без имени программы.
pub fn run(args: &[String]) -> i32 {
    let Some(connection_id) = std::env::var("GLAFIRA_ASKPASS").ok() else { return 1 };
    let Some(report_file) = std::env::var_os("GLAFIRA_ASKPASS_REPORT").map(PathBuf::from) else { return 1 };
    let vars = AskpassVars {
        connection_id,
        report_file,
        trust: std::env::var("GLAFIRA_ASKPASS_TRUST").ok().filter(|t| !t.is_empty()),
    };
    let prompt = args.join(" ");
    let (code, out) = respond(&prompt, &vars, &crate::secrets::KeyringStore::default());
    if !out.is_empty() {
        use std::io::Write;
        let _ = std::io::stdout().write_all(out.as_bytes());
        let _ = std::io::stdout().flush();
    }
    code
}
```

`src-tauri/src/lib.rs`: `pub fn askpass_main() -> i32 { let args: Vec<String> = std::env::args().skip(1).collect(); crate::net::askpass::run(&args) }`. `src-tauri/src/main.rs`:

```rust
fn main() {
    // Режим askpass для ssh (спец. P1 §5.1): Tauri не запускается.
    if std::env::var_os("GLAFIRA_ASKPASS").is_some() {
        std::process::exit(glafira_pg_lib::askpass_main());
    }
    glafira_pg_lib::run()
}
```

- [ ] **Step 4: Прогнать проверки и commit**

Run: проверки из Global Constraints.

```bash
git add src-tauri
git commit -m "feat(ssh): режим askpass — пароль, пассфраза и ответ о ключе хоста из самого приложения

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Супервизор туннеля и встраивание в соединения, опрос и проверку

**Files:**
- Create: `src-tauri/src/net/ssh/tunnel.rs`, `src-tauri/examples/fake_ssh.rs`
- Modify: `src-tauri/src/conn/pg.rs`, `src-tauri/src/topology/connector.rs`, `src-tauri/src/topology/patroni.rs`, `src-tauri/src/topology/runtime.rs`, `src-tauri/src/state.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/commands/connections.rs`, `src-tauri/src/commands/app.rs` (архив диагностики — логи `ssh-*.log`), `src-tauri/src/readiness/check.rs`, `src-tauri/src/storage/fs.rs` (`write_private`), `src-tauri/tests/it_runtime.rs` и `src-tauri/tests/it_readiness.rs` (конструкторы), `src/api/client.ts`, `src/api/client.channel.test.ts`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `tunnel.rs` (через `fake_ssh`), `pg.rs` (verify-full через SOCKS), `connector.rs`, `runtime.rs`, `check.rs`

**Interfaces:**
- Consumes: задачи 1–3; `PoolRegistry::evict_connection`; `tcp_connect`; `ReqwestHttp`.
- Produces:
  - `net::ssh::tunnel::{TunnelConfig { data_dir, log_dir, ssh_bin: Option<PathBuf>, known_hosts: Option<PathBuf>, backoff: Vec<Duration>, extra_env: Vec<(String, String)>, stage_secrets_in_file: bool }, TunnelConfig::new(data_dir, log_dir), TunnelStatus { Starting, Up { socks }, Down { error, attempt }, Closed }, TunnelRegistry::new(cfg, pools, secrets) -> Arc<Self>, open(&self, profile, timeout) -> AppResult<SocketAddr>, route(&self, id) -> AppResult<Option<SocketAddr>>, status(&self, id) -> Option<TunnelStatus>, restart_if_down(&self, id) -> AppResult<()>, close(&self, id), close_all(&self), trust(&self, target, fingerprint), reap_orphans(&self) -> usize, stage_secret(&self, id, field, value) -> AppResult<()>, unstage_secrets(&self, id)}`;
  - `ConnectSpec::socks: Option<SocketAddr>`, `ConnectSpec::build_via(profile, host, port, database, secrets, socks)`;
  - `pooled_client(…, purpose, socks: Option<SocketAddr>)`; `PooledConnector::tunnels: Arc<TunnelRegistry>`; `AdhocConnector::socks: Option<SocketAddr>`;
  - `ReqwestHttp::new(settings, password, key_password, proxy: Option<SocketAddr>)`;
  - `Runtimes::new(source, pools, tunnels)`, `Runtimes::with_interval(source, pools, tunnels, interval)`; `LiveSource::new(secrets, pools, tunnels)`;
  - `AppState::tunnels`, `AppState::new_with_logs(data_dir, log_dir, secrets)`; команда `trust_ssh_host_key(target, fingerprint)`; `api.trustSshHostKey(target, fingerprint)`;
  - `readiness::check::TestStep::Ssh` (первый шаг), ключи `test.step.ssh`, `test.sshOk`, `test.sshNotUsed`.

- [ ] **Step 1: Поддельный `ssh` для тестов**

`src-tauri/examples/fake_ssh.rs` (только `std`; собирается `cargo test` вместе с примерами):

```rust
//! Поддельный OpenSSH для unit-тестов туннеля: разбирает те же аргументы, что настоящий ssh, держит SOCKS5 на
//! порту из `-D` и ведёт себя по `FAKE_SSH_MODE`: `ok` (по умолчанию), `auth`, `hostkey`. Переменные:
//! `FAKE_SSH_UPSTREAM=host:port` — куда идут все CONNECT (иначе — к запрошенному адресу);
//! `FAKE_SSH_DIE_MS=N` — завершиться с кодом 255 через N мс после готовности; `FAKE_SSH_VERSION` — ответ на `-V`;
//! `FAKE_SSH_BIND_FAIL_ONCE=<файл-маркер>` — первый запуск падает «Address already in use».
//! Режим `hostkey`: отпечаток `SHA256:fake`; с `GLAFIRA_ASKPASS_TRUST=SHA256:fake` ведёт себя как `ok`, иначе пишет
//! запрос в `GLAFIRA_ASKPASS_REPORT` и падает «Host key verification failed.» — как настоящий ssh после ответа
//! askpass «no».

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-V") {
        eprintln!("{}", std::env::var("FAKE_SSH_VERSION").unwrap_or_else(|_| "OpenSSH_9.9p1 fake".into()));
        return;
    }
    let socks = args
        .windows(2)
        .find(|w| w[0] == "-D")
        .map(|w| w[1].clone())
        .expect("-D 127.0.0.1:port");
    match std::env::var("FAKE_SSH_MODE").as_deref().unwrap_or("ok") {
        "auth" => {
            eprintln!("tester@bastion: Permission denied (publickey,password).");
            std::process::exit(255);
        }
        "hostkey" if std::env::var("GLAFIRA_ASKPASS_TRUST").as_deref() != Ok("SHA256:fake") => {
            let prompt = "The authenticity of host 'bastion (10.0.0.5)' can't be established.\nED25519 key fingerprint is SHA256:fake.\nAre you sure you want to continue connecting (yes/no/[fingerprint])?";
            if let Ok(report) = std::env::var("GLAFIRA_ASKPASS_REPORT") {
                std::fs::write(report, prompt).unwrap();
            }
            eprintln!("Host key verification failed.");
            std::process::exit(255);
        }
        _ => {}
    }
    if let Ok(marker) = std::env::var("FAKE_SSH_BIND_FAIL_ONCE")
        && !std::path::Path::new(&marker).exists()
    {
        std::fs::write(&marker, b"1").unwrap();
        eprintln!("bind [127.0.0.1]:{}: Address already in use", socks.rsplit(':').next().unwrap());
        eprintln!("Could not request local forwarding.");
        std::process::exit(255);
    }
    let listener = TcpListener::bind(&socks).expect("bind -D port");
    if let Ok(ms) = std::env::var("FAKE_SSH_DIE_MS") {
        let ms: u64 = ms.parse().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            eprintln!("Connection to bastion closed by remote host.");
            std::process::exit(255);
        });
    }
    for client in listener.incoming().flatten() {
        std::thread::spawn(move || serve(client));
    }
}

/// SOCKS5 без аутентификации: CONNECT к имени (0x03) или IPv4 (0x01).
fn serve(mut c: TcpStream) {
    let mut b = [0u8; 300];
    c.read_exact(&mut b[..2]).unwrap();
    let n = b[1] as usize;
    c.read_exact(&mut b[..n]).unwrap();
    c.write_all(&[5, 0]).unwrap();
    c.read_exact(&mut b[..4]).unwrap();
    let target = match b[3] {
        3 => {
            c.read_exact(&mut b[..1]).unwrap();
            let n = b[0] as usize;
            c.read_exact(&mut b[..n]).unwrap();
            let host = String::from_utf8_lossy(&b[..n]).into_owned();
            c.read_exact(&mut b[..2]).unwrap();
            format!("{host}:{}", u16::from_be_bytes([b[0], b[1]]))
        }
        1 => {
            c.read_exact(&mut b[..6]).unwrap();
            format!("{}.{}.{}.{}:{}", b[0], b[1], b[2], b[3], u16::from_be_bytes([b[4], b[5]]))
        }
        _ => {
            c.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
            return;
        }
    };
    let upstream = std::env::var("FAKE_SSH_UPSTREAM").unwrap_or(target);
    let Ok(mut up) = TcpStream::connect(&upstream) else {
        // Бастион не видит узел: ответ «connection refused».
        c.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        return;
    };
    c.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
    let mut c2 = c.try_clone().unwrap();
    let mut up2 = up.try_clone().unwrap();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut c2, &mut up2);
    });
    let _ = std::io::copy(&mut up, &mut c);
}
```

- [ ] **Step 2: Падающие тесты туннеля**

`src-tauri/src/net/ssh/tunnel.rs`, модуль `tests`:

```rust
    use super::*;
    use crate::conn::pool::{PoolKey, PoolPurpose, PoolRegistry};
    use crate::conn::profile::{SshAuth, SshSettings, sample_profile};
    use crate::secrets::MemoryStore;

    /// `cargo test` собирает примеры: бинарь лежит рядом с каталогом тестовых бинарей.
    fn fake_ssh_bin() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let debug = exe.parent().unwrap().parent().unwrap();
        debug.join("examples").join(format!("fake_ssh{}", std::env::consts::EXE_SUFFIX))
    }

    fn profile(auth: SshAuth) -> ConnectionProfile {
        let mut p = sample_profile("c1");
        p.pg.connect_timeout_s = 2;
        p.ssh = Some(SshSettings {
            target: "bastion".into(),
            port: None,
            user: Some("tester".into()),
            auth,
            key_path: (auth == SshAuth::Key).then(|| "/k/id".to_string()),
        });
        p
    }

    fn registry(dir: &Path, env: Vec<(&str, String)>) -> (Arc<TunnelRegistry>, Arc<PoolRegistry>) {
        let pools = Arc::new(PoolRegistry::default());
        let mut cfg = TunnelConfig::new(dir.join("data"), dir.join("logs"));
        cfg.ssh_bin = Some(fake_ssh_bin());
        cfg.backoff = vec![Duration::from_millis(50)];
        cfg.extra_env = env.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        (TunnelRegistry::new(cfg, pools.clone(), Arc::new(MemoryStore::default())), pools)
    }

    /// Эхо-сервер: что пришло, то и ушло.
    async fn echo_server() -> std::net::SocketAddr {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn tunnel_opens_routes_through_socks_and_closes() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let p = profile(SshAuth::Agent);
        let socks = reg.open(&p, Duration::from_secs(5)).await.unwrap();
        assert_eq!(reg.route("c1").unwrap(), Some(socks));
        let mut s = crate::net::ssh::socks::connect(socks, "pg-single", 5432).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut s, b"ping").await.unwrap();
        let mut b = [0u8; 4];
        tokio::io::AsyncReadExt::read_exact(&mut s, &mut b).await.unwrap();
        assert_eq!(&b, b"ping");
        // Лог ssh и запись о PID на месте, пока туннель жив.
        assert!(dir.path().join("logs").join("ssh-c1.log").parent().unwrap().exists());
        assert!(std::fs::read_to_string(dir.path().join("data").join("tunnels.json")).unwrap().contains("ssh-c1.log"));
        reg.close("c1");
        assert_eq!(reg.route("c1").unwrap(), None);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(tokio::net::TcpStream::connect(socks).await.is_err(), "процесс завершён");
        assert!(std::fs::read_to_string(dir.path().join("data").join("tunnels.json")).unwrap().contains("\"pids\":[]"));
    }

    #[tokio::test]
    async fn login_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_MODE", "auth".into())]);
        let e = reg.open(&profile(SshAuth::Password), Duration::from_secs(5)).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::SshError, "errors.ssh.authFailed"));
        assert_eq!(e.params["kind"], "authFailed");
        assert!(e.details.as_deref().unwrap_or("").contains("Permission denied"));
        assert_eq!(reg.route("c1").unwrap(), None, "неудачный туннель не остаётся в реестре");
    }

    #[tokio::test]
    async fn unknown_host_key_is_reported_and_trust_lets_the_next_attempt_through() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_MODE", "hostkey".into()), ("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let e = reg.open(&profile(SshAuth::Agent), Duration::from_secs(5)).await.unwrap_err();
        assert_eq!(e.key, "errors.ssh.hostKeyUnknown");
        assert_eq!(e.params["kind"], "hostKeyUnknown");
        assert_eq!(e.params["fingerprint"], "SHA256:fake");
        assert_eq!(e.params["host"], "bastion (10.0.0.5)");
        assert_eq!(e.params["target"], "bastion");
        reg.trust("bastion", "SHA256:fake");
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn busy_local_port_is_retried_once() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let marker = dir.path().join("bind-failed");
        let (reg, _) = registry(
            dir.path(),
            vec![("FAKE_SSH_BIND_FAIL_ONCE", marker.to_string_lossy().into_owned()), ("FAKE_SSH_UPSTREAM", echo.to_string())],
        );
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5)).await.unwrap();
        assert!(marker.exists(), "первая попытка упала на занятом порту, вторая удалась");
    }

    #[tokio::test]
    async fn dead_tunnel_restarts_with_backoff_and_evicts_pools() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, pools) = registry(dir.path(), vec![("FAKE_SSH_DIE_MS", "300".into()), ("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let first = reg.open(&profile(SshAuth::Agent), Duration::from_secs(5)).await.unwrap();
        pools.insert_if_absent(
            PoolKey { connection_id: "c1".into(), member: "m".into(), database: "postgres".into(), purpose: PoolPurpose::Catalog },
            crate::conn::pg::ConnectSpec::build(&sample_profile("c1"), "h", 1, "postgres", &MemoryStore::default()).unwrap(),
        );
        // Процесс умирает через 300 мс: статус Down с ошибкой, пулы вытеснены, маршрута нет.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(TunnelStatus::Down { .. }) = reg.status("c1") {
                break;
            }
            assert!(Instant::now() < deadline, "не дождались падения");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(pools.keys().is_empty());
        assert_eq!(reg.route("c1").unwrap_err().params["kind"], "exited");
        // После паузы (50 мс в тестах) процесс перезапущен на новом порту.
        loop {
            if let Some(TunnelStatus::Up { socks }) = reg.status("c1") {
                assert_ne!(socks, first);
                break;
            }
            assert!(Instant::now() < deadline, "не дождались перезапуска");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        reg.close("c1");
    }

    #[tokio::test]
    async fn old_openssh_blocks_password_and_passphrase_only() {
        let dir = tempfile::tempdir().unwrap();
        let echo = echo_server().await;
        let (reg, _) = registry(dir.path(), vec![("FAKE_SSH_VERSION", "OpenSSH_8.3p1".into()), ("FAKE_SSH_UPSTREAM", echo.to_string())]);
        let e = reg.open(&profile(SshAuth::Password), Duration::from_secs(5)).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::FeatureUnavailable, "errors.ssh.tooOld"));
        reg.open(&profile(SshAuth::Agent), Duration::from_secs(5)).await.unwrap();
    }

    #[tokio::test]
    async fn staged_secret_is_written_for_askpass_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (reg, _) = registry(dir.path(), vec![]);
        reg.stage_secret("c1", SSH_PASSWORD, "pw").unwrap();
        let file = dir.path().join("data").join("askpass-c1.secrets");
        assert!(file.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        reg.unstage_secrets("c1");
        assert!(!file.exists());
    }
```

`src-tauri/src/conn/pg.rs`, модуль `tests`:

```rust
    /// Через SOCKS TLS проверяется по исходному имени узла, а не по адресу прокси (спец. P1 §7).
    #[tokio::test]
    async fn verify_full_via_socks_checks_the_original_host_name() {
        let dir = tempfile::tempdir().unwrap();
        let ca = testutil::make_ca("test-ca");
        let ca_path = write_pem(dir.path(), "ca.pem", &ca.cert);
        let server = testutil::tls_server(&testutil::make_server_cert(&ca, "pg-single"));
        let proxy = testutil::socks_proxy_to(("127.0.0.1", server.port)).await;
        let ok = loopback_spec_via(proxy, "pg-single", SslMode::VerifyFull, Some(&ca_path));
        assert_handshake_ok(ok.connect().await.unwrap_err());
        let bad = loopback_spec_via(proxy, "other-name", SslMode::VerifyFull, Some(&ca_path));
        let e = bad.connect().await.unwrap_err();
        assert_eq!(e.key, "errors.pg.hostnameMismatch", "{e:?}");
    }
```

(`loopback_spec_via(proxy, host, mode, ca)` — как `loopback_spec`, но `ConnectSpec::build_via(…, Some(proxy))` с `host` вместо адреса и портом 5432; `testutil::socks_proxy_to(upstream)` — поддельный SOCKS5 из тестов `socks.rs`, перенесённый в `testutil` как `pub async fn socks_proxy_to(upstream: (&str, u16)) -> SocketAddr`.)

`src-tauri/src/readiness/check.rs`, тест `skipped_steps_say_why`: первый пропущенный шаг теперь `(TestStep::Ssh, Some("test.sshNotUsed"))`.

`src/api/client.channel.test.ts`: строка `["trust_ssh_host_key", (a) => a.trustSshHostKey("bastion", "SHA256:x"), { target: "bastion", fingerprint: "SHA256:x" }]`.

- [ ] **Step 3: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib net::ssh::tunnel conn::pg readiness::check`
Expected: ошибки компиляции.

- [ ] **Step 4: Реализация**

`src-tauri/src/storage/fs.rs`: `pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()>` — `std::fs::write`, затем на unix `set_permissions(0o600)` (на Windows права наследуются от каталога пользователя).

`src-tauri/src/net/ssh/tunnel.rs`:

```rust
//! Супервизор туннеля (спец. P1 §5): один процесс `ssh -D` на подключение, готовность по порту SOCKS,
//! перезапуск с паузами после падения, доверие ключу хоста, сироты.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::conn::pool::PoolRegistry;
use crate::conn::profile::{ConnectionProfile, SshAuth, SshSettings};
use crate::error::{AppError, AppResult};
use crate::net::ssh::orphans;
use crate::net::ssh::process::{
    self, AskpassEnv, SshArgs, SshChild, SshErrorKind, askpass_supported, classify_stderr, ssh_error,
};
use crate::secrets::{SSH_PASSPHRASE, SSH_PASSWORD, SecretStore};
use crate::sync::MutexExt;

/// Паузы перед перезапуском упавшего процесса; последняя повторяется.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

#[derive(Debug, Clone)]
pub struct TunnelConfig {
    pub data_dir: PathBuf,
    pub log_dir: PathBuf,
    /// Путь к `ssh` вместо поиска в PATH (тесты: `fake_ssh`).
    pub ssh_bin: Option<PathBuf>,
    /// Свой `known_hosts` (`GLAFIRA_SSH_KNOWN_HOSTS`): тесты не трогают файл пользователя.
    pub known_hosts: Option<PathBuf>,
    pub backoff: Vec<Duration>,
    /// Дополнительное окружение процесса ssh (тесты управляют `fake_ssh`).
    pub extra_env: Vec<(String, String)>,
    /// Секреты для askpass кладутся в файл 0600 рядом с данными, а не читаются из keychain: только там, где
    /// keychain недоступен (CI) — `common::tunnel_registry_for_tests`. В приложении — всегда keychain (спец. §5.1).
    pub stage_secrets_in_file: bool,
    /// Бинарь для `SSH_ASKPASS` вместо текущего процесса: интеграционные тесты — это бинарь libtest, а askpass
    /// умеет только `glafira-pg` (`env!("CARGO_BIN_EXE_glafira-pg")`).
    pub askpass_bin: Option<PathBuf>,
}

impl TunnelConfig {
    pub fn new(data_dir: PathBuf, log_dir: PathBuf) -> Self {
        Self {
            data_dir,
            log_dir,
            ssh_bin: None,
            known_hosts: std::env::var_os("GLAFIRA_SSH_KNOWN_HOSTS").map(PathBuf::from),
            backoff: BACKOFF.to_vec(),
            extra_env: Vec::new(),
            stage_secrets_in_file: false,
            askpass_bin: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TunnelStatus {
    Starting,
    Up { socks: SocketAddr },
    Down { error: AppError, attempt: u32 },
    Closed,
}

struct Tunnel {
    settings: SshSettings,
    status: Mutex<TunnelStatus>,
    closed: AtomicBool,
    supervisor: Mutex<Option<JoinHandle<()>>>,
    child: tokio::sync::Mutex<Option<SshChild>>,
}

pub struct TunnelRegistry {
    cfg: TunnelConfig,
    pools: Arc<PoolRegistry>,
    secrets: Arc<dyn SecretStore>,
    tunnels: Mutex<HashMap<String, Arc<Tunnel>>>,
    /// Доверенные отпечатки по цели ssh — до выхода из приложения (отступление 2).
    trust: Mutex<HashMap<String, String>>,
    version: OnceLock<(u32, u32)>,
}

fn needs_askpass(s: &SshSettings) -> bool {
    matches!(s.auth, SshAuth::Password | SshAuth::Key)
}

impl TunnelRegistry {
    pub fn new(cfg: TunnelConfig, pools: Arc<PoolRegistry>, secrets: Arc<dyn SecretStore>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            pools,
            secrets,
            tunnels: Mutex::new(HashMap::new()),
            trust: Mutex::new(HashMap::new()),
            version: OnceLock::new(),
        })
    }

    pub fn trust(&self, target: &str, fingerprint: &str) {
        self.trust.lock_safe().insert(target.to_string(), fingerprint.to_string());
    }

    /// Сироты прошлого запуска (`tunnels.json`): блокирующий вызов при старте приложения.
    pub fn reap_orphans(&self) -> usize {
        orphans::reap(&self.cfg.data_dir)
    }

    fn report_file(&self, id: &str) -> PathBuf {
        self.cfg.data_dir.join(format!("askpass-{id}.report"))
    }

    fn secrets_file(&self, id: &str) -> PathBuf {
        self.cfg.data_dir.join(format!("askpass-{id}.secrets"))
    }

    /// Секрет формы для проверки несохранённого профиля: askpass читает его из keychain под id профиля
    /// (или из файла 0600 — только `stage_secrets_in_file`). Снимается `unstage_secrets`.
    pub fn stage_secret(&self, id: &str, field: &str, value: &str) -> AppResult<()> {
        if self.cfg.stage_secrets_in_file {
            let path = self.secrets_file(id);
            let mut map: HashMap<String, String> = std::fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default();
            map.insert(field.to_string(), value.to_string());
            crate::storage::fs::write_private(&path, &serde_json::to_vec(&map).expect("map"))?;
            Ok(())
        } else {
            self.secrets.set(&crate::secrets::secret_key(id, field), value)
        }
    }

    pub fn unstage_secrets(&self, id: &str) {
        if self.cfg.stage_secrets_in_file {
            let _ = std::fs::remove_file(self.secrets_file(id));
        } else {
            for f in [SSH_PASSWORD, SSH_PASSPHRASE] {
                let _ = self.secrets.delete(&crate::secrets::secret_key(id, f));
            }
        }
    }

    async fn ssh_bin(&self) -> AppResult<PathBuf> {
        match &self.cfg.ssh_bin {
            Some(b) => Ok(b.clone()),
            None => tokio::task::spawn_blocking(process::find_ssh).await.expect("find_ssh"),
        }
    }

    async fn version(&self, bin: &Path) -> AppResult<(u32, u32)> {
        if let Some(v) = self.version.get() {
            return Ok(*v);
        }
        let v = process::ssh_version(bin).await?;
        let _ = self.version.set(v);
        Ok(v)
    }

    fn askpass_env(&self, id: &str, target: &str) -> AskpassEnv {
        AskpassEnv {
            connection_id: id.to_string(),
            report_file: self.report_file(id),
            trust: self.trust.lock_safe().get(target).cloned(),
        }
    }

    fn args(&self, bin: PathBuf, id: &str, settings: &SshSettings, port: u16) -> SshArgs {
        SshArgs {
            bin,
            log_file: self.cfg.log_dir.join(format!("ssh-{id}.log")),
            socks_port: port,
            settings: settings.clone(),
            known_hosts: self.cfg.known_hosts.clone(),
        }
    }

    /// Один запуск процесса до готовности порта SOCKS (внутри — одна повторная попытка на занятом порту).
    async fn spawn_ready(&self, id: &str, settings: &SshSettings, deadline: Instant) -> AppResult<(SshChild, SocketAddr)> {
        let bin = self.ssh_bin().await?;
        let mut attempts = 0;
        loop {
            attempts += 1;
            let port = process::free_port().map_err(|e| ssh_error(SshErrorKind::Exited, &settings.target, &e.to_string()))?;
            let socks = SocketAddr::from(([127, 0, 0, 1], port));
            let _ = std::fs::create_dir_all(&self.cfg.log_dir);
            let _ = std::fs::remove_file(self.report_file(id));
            let mut args = self.args(bin.clone(), id, settings, port);
            let mut env = self.askpass_env(id, &settings.target);
            for (k, v) in &self.cfg.extra_env {
                args.extra_env.push((k.clone(), v.clone()));
            }
            if self.cfg.stage_secrets_in_file {
                env.secret_file = Some(self.secrets_file(id));
            }
            let mut child = process::spawn(&args, &env)?;
            match child.wait_ready(socks, deadline).await {
                Ok(()) => {
                    if let Some(pid) = child.pid() {
                        let _ = orphans::register(&self.cfg.data_dir, pid, &args.log_file);
                    }
                    return Ok((child, socks));
                }
                Err(code) => {
                    let report = std::fs::read_to_string(self.report_file(id)).ok();
                    let kind = classify_stderr(code, &child.stderr_tail(), report.as_deref());
                    if kind == SshErrorKind::BindFailed && attempts == 1 {
                        continue;
                    }
                    return Err(ssh_error(kind, &settings.target, &child.stderr_tail()));
                }
            }
        }
    }

    /// Открывает туннель подключения (повторный вызов для живого туннеля возвращает его порт). Ожидание —
    /// до `timeout` (`connect_timeout_s + 5` у вызывающего).
    pub async fn open(self: &Arc<Self>, profile: &ConnectionProfile, timeout: Duration) -> AppResult<SocketAddr> {
        let settings = profile.ssh.clone().ok_or_else(|| AppError::internal_with("profile has no ssh settings"))?;
        if let Some(t) = self.tunnels.lock_safe().get(&profile.id)
            && let TunnelStatus::Up { socks } = *t.status.lock_safe()
        {
            return Ok(socks);
        }
        let bin = self.ssh_bin().await?;
        if needs_askpass(&settings) && !askpass_supported(self.version(&bin).await?) {
            return Err(ssh_error(SshErrorKind::TooOld, &settings.target, ""));
        }
        let deadline = Instant::now() + timeout;
        let (child, socks) = self.spawn_ready(&profile.id, &settings, deadline).await?;
        let tunnel = Arc::new(Tunnel {
            settings,
            status: Mutex::new(TunnelStatus::Up { socks }),
            closed: AtomicBool::new(false),
            supervisor: Mutex::new(None),
            child: tokio::sync::Mutex::new(Some(child)),
        });
        self.close(&profile.id);
        self.tunnels.lock_safe().insert(profile.id.clone(), tunnel.clone());
        let handle = tokio::spawn(Self::supervise(self.clone(), profile.id.clone(), tunnel.clone()));
        *tunnel.supervisor.lock_safe() = Some(handle);
        Ok(socks)
    }

    /// Следит за процессом: упал — статус Down, пулы подключения вытеснены (их соединения мертвы), пауза
    /// по `backoff`, перезапуск на новом порту; `close` прекращает.
    async fn supervise(self: Arc<Self>, id: String, tunnel: Arc<Tunnel>) {
        let mut attempt: u32 = 0;
        loop {
            // Есть процесс — ждём его конца; нет (прошлый перезапуск не удался) — сразу к паузе.
            let exited = {
                let mut guard = tunnel.child.lock().await;
                match guard.as_mut() {
                    Some(c) => Some(c.wait().await),
                    None => None,
                }
            };
            if tunnel.closed.load(Ordering::SeqCst) {
                return;
            }
            if let Some(code) = exited {
                let tail = tunnel.child.lock().await.as_ref().map(|c| c.stderr_tail()).unwrap_or_default();
                let error = ssh_error(classify_stderr(code, &tail, None), &tunnel.settings.target, &tail);
                tracing::warn!(connection = %id, attempt, "ssh tunnel exited: {}", error.message);
                *tunnel.status.lock_safe() = TunnelStatus::Down { error, attempt };
                // Соединения через упавший процесс мертвы: пулы подключения вытесняются.
                self.pools.evict_connection(&id);
            }
            let pause = self.cfg.backoff[(attempt as usize).min(self.cfg.backoff.len() - 1)];
            attempt += 1;
            tokio::time::sleep(pause).await;
            if tunnel.closed.load(Ordering::SeqCst) {
                return;
            }
            let deadline = Instant::now() + Duration::from_secs(30);
            match self.spawn_ready(&id, &tunnel.settings, deadline).await {
                Ok((child, socks)) => {
                    *tunnel.child.lock().await = Some(child);
                    *tunnel.status.lock_safe() = TunnelStatus::Up { socks };
                    attempt = 0;
                }
                Err(error) => {
                    tracing::warn!(connection = %id, attempt, "ssh tunnel restart failed: {}", error.message);
                    *tunnel.status.lock_safe() = TunnelStatus::Down { error, attempt };
                    *tunnel.child.lock().await = None;
                }
            }
        }
    }

    /// Куда идти соединениям подключения: `None` — напрямую, `Some` — порт SOCKS; туннель упал — его ошибка.
    pub fn route(&self, id: &str) -> AppResult<Option<SocketAddr>> {
        let Some(t) = self.tunnels.lock_safe().get(id).cloned() else { return Ok(None) };
        match &*t.status.lock_safe() {
            TunnelStatus::Up { socks } => Ok(Some(*socks)),
            TunnelStatus::Down { error, .. } => Err(error.clone()),
            TunnelStatus::Starting | TunnelStatus::Closed => Err(ssh_error(SshErrorKind::Exited, &t.settings.target, "")),
        }
    }

    pub fn status(&self, id: &str) -> Option<TunnelStatus> {
        self.tunnels.lock_safe().get(id).map(|t| t.status.lock_safe().clone())
    }

    /// «Переподключить»: упавший туннель поднимается сразу, не дожидаясь паузы; живой не трогается.
    pub async fn restart_if_down(self: &Arc<Self>, profile: &ConnectionProfile, timeout: Duration) -> AppResult<()> {
        match self.status(&profile.id) {
            Some(TunnelStatus::Down { .. }) => {
                self.close(&profile.id);
                self.open(profile, timeout).await.map(|_| ())
            }
            _ => Ok(()),
        }
    }

    pub fn close(&self, id: &str) {
        let Some(t) = self.tunnels.lock_safe().remove(id) else { return };
        t.closed.store(true, Ordering::SeqCst);
        *t.status.lock_safe() = TunnelStatus::Closed;
        if let Some(h) = t.supervisor.lock_safe().take() {
            h.abort();
        }
        let data_dir = self.cfg.data_dir.clone();
        tokio::spawn(async move {
            if let Some(mut c) = t.child.lock().await.take() {
                let pid = c.pid();
                c.kill().await;
                if let Some(pid) = pid {
                    let _ = orphans::unregister(&data_dir, pid);
                }
            }
        });
        let _ = std::fs::remove_file(self.report_file(id));
    }

    pub fn close_all(&self) {
        let ids: Vec<String> = self.tunnels.lock_safe().keys().cloned().collect();
        for id in ids {
            self.close(&id);
        }
    }
}
```

Правки `process.rs` под это: `SshArgs` получает `pub extra_env: Vec<(String, String)>` (по умолчанию пусто; `command_line` его не касается, `spawn` добавляет в окружение; литералы `SshArgs` в тестах `process.rs` дополнить `extra_env: vec![]`), `AskpassEnv` — `pub secret_file: Option<PathBuf>` (если задан, `spawn` ставит `GLAFIRA_ASKPASS_SECRET_FILE`) и `pub askpass_bin: Option<PathBuf>` (`spawn` ставит `SSH_ASKPASS` в него, иначе в `current_exe()`); `TunnelRegistry::askpass_env` заполняет оба из `cfg`. Сторона askpass уже сделана в задаче 3 и не трогается: при заданной `GLAFIRA_ASKPASS_SECRET_FILE` секреты берутся только из этого файла (JSON `{поле: значение}` → `MemoryStore`), keychain не используется. `close()` на выходе приложения: `tokio::spawn` недоступен вне runtime — в `close_all` для `RunEvent::Exit` использовать блокирующее завершение: если `tokio::runtime::Handle::try_current()` нет, убить процесс через `std::process::Command` (`kill -TERM`/`taskkill`) по PID из `tunnels.json` — проще: `close_all_blocking()` вызывает `orphans::reap(&data_dir)` (PID и лог там же). Так и сделать: `pub fn close_all_blocking(&self) -> usize { self.tunnels.lock_safe().clear(); orphans::reap(&self.cfg.data_dir) }`.

`src-tauri/src/conn/pg.rs`: поле `pub socks: Option<std::net::SocketAddr>` в `ConnectSpec` (`build` → `None`; `pub fn build_via(profile, host, port, database, secrets, socks) -> AppResult<Self>` — `build` + установка поля); `connect_with_handle`:

```rust
            let stream: Pin<Box<dyn AsyncStream>> = match self.socks {
                None => Box::pin(
                    tcp_connect(&self.tcp_host, self.tcp_port)
                        .await
                        .map_err(|e| errors::tcp_error(&e, &self.host, self.port))?,
                ),
                // Через туннель имя узла разрешает бастион; TLS ниже проверяется по тому же имени.
                Some(proxy) => Box::pin(crate::net::ssh::socks::connect(proxy, &self.host, self.port).await?),
            };
```

с `pub trait AsyncStream: AsyncRead + AsyncWrite + Send {} impl<T: AsyncRead + AsyncWrite + Send> AsyncStream for T {}` и `use std::pin::Pin; use tokio::io::{AsyncRead, AsyncWrite};`. `Debug` для `ConnectSpec` печатает `socks`.

`src-tauri/src/topology/connector.rs`: `pooled_client(…, purpose, socks: Option<SocketAddr>)` → `ConnectSpec::build_via(&p, &h, port, &d, secrets.as_ref(), socks)`; `PooledConnector { pools, profile, secrets, tunnels: Arc<TunnelRegistry> }` — в `facts_within` сначала `let socks = self.tunnels.route(&self.profile.id)?;` (ошибка туннеля = узел недоступен с `SSH_ERROR`); `AdhocConnector { profile, secrets, socks: Option<SocketAddr> }` → `build_via`. Тесты `connector.rs` и `commands/catalog.rs` передают `None` / реестр с пустым конфигом (`TunnelRegistry::new(TunnelConfig::new(tmp, tmp), pools, secrets)`).

`src-tauri/src/topology/patroni.rs`: `ReqwestHttp::new(settings, password, key_password, proxy: Option<SocketAddr>)` — `if let Some(p) = proxy { b = b.proxy(reqwest::Proxy::all(format!("socks5h://{p}")).map_err(http_setup)?); }`; все вызовы в тестах — `None`.

`src-tauri/src/topology/runtime.rs`: `LiveSource { secrets, pools, tunnels, http }`, `LiveSource::new(secrets, pools, tunnels)`; `http_for` — `let proxy = self.tunnels.route(&profile.id)?;` и передача в `ReqwestHttp::new`; `observe` строит `PooledConnector` с `tunnels`. `Runtimes { …, tunnels: Arc<TunnelRegistry> }`, `new(source, pools, tunnels)`, `with_interval(source, pools, tunnels, interval)`; `connect_locked` после `emit_state(Connecting)` и до `observe`:

```rust
        if profile.ssh.is_some() {
            let timeout = Duration::from_secs(profile.pg.connect_timeout_s + 5);
            if let Err(err) = self.tunnels.open(&profile, timeout).await {
                let mut entries = self.entries.lock_safe();
                let Some(e) = entries.get_mut(&id).filter(|e| e.generation == generation) else {
                    drop(entries);
                    return Err(self.discard_stale(&id));
                };
                e.state = ConnState::Failed;
                e.error = Some(err.clone());
                self.emit_state(&id, ConnState::Failed, Some(&err));
                return Err(err);
            }
        }
```

`disconnect`: после `self.source.forget(id)` — `self.tunnels.close(id);`; `discard_stale` — тоже `close`; `reconnect` (живая ветка) — перед `observe_entry`: `self.tunnels.restart_if_down(&profile_clone, timeout).await?` (профиль — из записи, таймаут как выше); `shutdown` — `self.tunnels.close_all_blocking()`. Тесты `runtime.rs`: фикстура `setup_with` создаёт реестр `TunnelRegistry::new(TunnelConfig::new(tmp.join("d"), tmp.join("l")), pools.clone(), Arc::new(MemoryStore::default()))` (временный каталог — `tempfile::tempdir()`, хранить в фикстуре, чтобы не удалился). Новый тест `runtime.rs`: `tunnel_failure_fails_the_connection` — профиль с `ssh`, реестр с `ssh_bin = fake_ssh`, `FAKE_SSH_MODE=auth` → `connect` → `Err` с ключом `errors.ssh.authFailed`, состояние `Failed`, записи `Connecting, Failed`; `disconnect` закрывает туннель (`status` → `None`).

`src-tauri/src/state.rs`: поле `pub tunnels: Arc<TunnelRegistry>`; `pub fn new(data_dir, secrets)` → `Self::new_with_logs(data_dir, &data_dir.join("logs"), secrets)`; `new_with_logs` создаёт реестр `TunnelRegistry::new(TunnelConfig::new(data_dir.to_path_buf(), log_dir.to_path_buf()), pools.clone(), secrets.clone())` и передаёт его в `LiveSource::new` и `Runtimes::new`; `client()` — `let socks = self.tunnels.route(connection_id)?;` → `pooled_client(…, socks)`. `lib.rs`: `AppState::new_with_logs(&dir, &app.path().app_log_dir()?, …)`, после создания — `tracing::info!(reaped = st.tunnels.reap_orphans(), "ssh orphans")`; в `RunEvent::Exit` — `st.tunnels.close_all_blocking()`; регистрация `commands::connections::trust_ssh_host_key`. `commands/app.rs::export_diagnostics`: к `log_files` добавить `crate::logging::log_files_named(&log_dir, "ssh-")` (логи ssh без секретов: `-E` пишет диагностику ssh).

`src-tauri/src/commands/connections.rs`:

```rust
/// Пользователь подтвердил ключ хоста ssh (`errors.ssh.hostKeyUnknown`): следующий запуск туннеля к этой
/// цели отвечает askpass «yes», а ssh сам записывает ключ в known_hosts. Действует до выхода из приложения.
#[tauri::command]
pub fn trust_ssh_host_key(state: State<'_, AppState>, target: String, fingerprint: String) -> AppResult<()> {
    state.tunnels.trust(&target, &fingerprint);
    Ok(())
}
```

`test_connection`: `crate::readiness::check::run(profile, overlay, state.tunnels.clone(), &|e| …)`.

`src-tauri/src/readiness/check.rs`: `TestStep::Ssh` первым вариантом; `run(profile, secrets, tunnels: Arc<TunnelRegistry>, emit)`; перед шагом Patroni:

```rust
    // 0. SSH: версия клиента и туннель (спец. §5). Секрет формы несохранённого профиля askpass ищет под id
    // профиля — на время проверки он кладётся в keychain (или файл, см. TunnelRegistry::stage_secret).
    let mut socks = None;
    let _tunnel_guard;
    if let Some(ssh) = &profile.ssh {
        event(emit, TestStep::Ssh, None, StepStatus::Running, None, None);
        let field = match ssh.auth {
            SshAuth::Password => Some(SSH_PASSWORD),
            SshAuth::Key => Some(SSH_PASSPHRASE),
            SshAuth::Agent => None,
        };
        if let Some(f) = field
            && let Some(v) = secrets.get(&secret_key(&profile.id, f))?
            && !v.is_empty()
        {
            tunnels.stage_secret(&profile.id, f, &v)?;
        }
        _tunnel_guard = TunnelGuard { tunnels: tunnels.clone(), id: profile.id.clone() };
        let timeout = Duration::from_secs(profile.pg.connect_timeout_s + 5);
        match tunnels.open(&profile, timeout).await {
            Ok(addr) => {
                socks = Some(addr);
                let version = tunnels.openssh_version().await.map(|(a, b)| format!("{a}.{b}")).unwrap_or_default();
                event(emit, TestStep::Ssh, None, StepStatus::Ok, Some(ltext!("test.sshOk", "OpenSSH {{version}}, tunnel is up", version = version)), None);
            }
            Err(e) => {
                event(emit, TestStep::Ssh, None, StepStatus::Failed, None, Some(e));
                return Ok(failed(None));
            }
        }
    } else {
        event(emit, TestStep::Ssh, None, StepStatus::Skipped, Some(ltext!("test.sshNotUsed", "Not used: direct connection")), None);
    }
```

с `struct TunnelGuard { tunnels: Arc<TunnelRegistry>, id: String }` и `impl Drop { fn drop(&mut self) { self.tunnels.close(&self.id); self.tunnels.unstage_secrets(&self.id); } }`; `AdhocConnector { profile, secrets, socks }`; клиент Patroni — `ReqwestHttp::new(settings, password, key_password, socks)`. `TunnelRegistry::openssh_version()` — публичная обёртка над `version(bin)`. Секрет формы для сохранённого профиля (`id` настоящий) уже лежит в keychain под тем же ключом: `stage_secret` его перезапишет тем же значением из overlay — безопасно, но `unstage_secrets` удалил бы настоящий секрет! Поэтому `TunnelGuard` снимает секреты только у несохранённого профиля: `if self.id.starts_with("unsaved-")`.

`src/api/client.ts`: `trustSshHostKey: (target: string, fingerprint: string) => call<void>("trust_ssh_host_key", { target, fingerprint }),`. Словари: `test.step.ssh` «SSH», `test.sshOk` (ru «OpenSSH {{version}}, туннель поднят»), `test.sshNotUsed` (ru «Не используется: прямое подключение»).

- [ ] **Step 5: Прогнать проверки**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib`, интеграционные тесты со стендом (конструкторы `Runtimes`/`LiveSource` в `it_runtime.rs`, `it_readiness.rs` — с реестром туннелей во временном каталоге), остальные проверки из Global Constraints. На macOS и Windows `cargo test --lib` тоже обязан проходить (CI `platforms`): `fake_ssh` там собирается тем же `cargo test`.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri src/api src/locales
git commit -m "feat(ssh): супервизор туннеля с перезапуском, соединения и Patroni через SOCKS, шаг SSH в проверке, доверие ключу хоста

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Форма: раздел SSH, пароли и пассфраза, диалог доверия ключу хоста

**Files:**
- Create: `src/components/connections/SshSection.tsx`, `src/components/connections/TrustHostKeyDialog.tsx` (+ `.test.tsx`)
- Modify: `src/components/connections/ConnectionForm.tsx` (+ `.test.tsx`), `src/components/connections/profile.ts` (+ `.test.ts`), `src/components/connections/TestPanel.tsx` (+ `.test.tsx`), `src/components/connections/tree/ConnectionNode.tsx` (+ `.test.tsx`), `src/components/overview/Overview.tsx` (+ `.test.tsx`), `src/components/connections/tree/ImportDialog.tsx` (needs-password: ssh), `src/locales/{en,ru}.json`, `docs/i18n/same-as-en.json`
- Test: vitest

**Interfaces:**
- Consumes: `SshSettings`, `SshAuth`, `SecretsInput`/`SecretsSet` с ssh-полями, `api.trustSshHostKey`, `api.connect`, `api.testConnection`.
- Produces: `SshSection({ idPrefix, profile, onChange, secrets, onSecrets, saved })`, `SshSecrets = { password: SecretField; passphrase: SecretField }`; `profile.ts::{emptySsh(), hostKeyUnknown(error) -> { target, host, fingerprint } | null}`; `TrustHostKeyDialog({ error, onTrusted, onCancel })`; `portProblem` учитывает `ssh.port`.

- [ ] **Step 1: Падающие тесты**

`src/components/connections/profile.test.ts`:

```ts
  it("ошибка неизвестного ключа хоста распознаётся по коду и виду", () => {
    const e = { code: "SSH_ERROR", key: "errors.ssh.hostKeyUnknown", message: "x", retryable: false, params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion (10.0.0.5)", fingerprint: "SHA256:abc" } } as AppError;
    expect(hostKeyUnknown(e)).toEqual({ target: "bastion", host: "bastion (10.0.0.5)", fingerprint: "SHA256:abc" });
    expect(hostKeyUnknown({ ...e, params: { kind: "authFailed" } })).toBeNull();
    expect(hostKeyUnknown({ ...e, code: "CONNECTION_FAILED" })).toBeNull();
  });

  it("порт SSH проверяется вместе с портами узлов", () => {
    const p: ConnectionProfile = { ...newProfile(), ssh: { ...emptySsh(), target: "b", port: 70000 } };
    expect(portProblem(p)?.key).toBe("errors.connections.portInvalid");
    expect(portProblem({ ...p, ssh: { ...p.ssh!, port: null } })).toBeNull();
  });
```

`src/components/connections/ConnectionForm.test.tsx`:

```tsx
  it("раздел SSH: включение добавляет цель и способ входа, пароль уходит как sshPassword", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockImplementation(async (p) => ({ ...p, id: "c9" }));
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.type(screen.getByLabelText("Имя"), "via-bastion");
    expect(screen.queryByLabelText("Цель SSH")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText("Через SSH-туннель"));
    await user.type(screen.getByLabelText("Цель SSH"), "bastion");
    await user.selectOptions(screen.getByLabelText("Вход на бастион"), "password");
    await user.type(screen.getByLabelText("Пароль SSH"), "s3cret");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
    const [profile, secrets] = vi.mocked(api.saveConnection).mock.calls[0];
    expect(profile.ssh).toEqual({ target: "bastion", port: null, user: null, auth: "password", keyPath: null });
    expect(secrets.sshPassword).toBe("s3cret");
    expect(secrets.sshPassphrase).toBeNull();
  });

  it("способ «ключ» показывает путь к ключу и пассфразу; выключение SSH убирает раздел и шлёт ssh: null", async () => {
    const user = userEvent.setup();
    const initial = { ...newProfile(), ssh: { target: "b", port: 2222, user: "tester", auth: "key" as const, keyPath: "/k/id" } };
    vi.mocked(api.saveConnection).mockImplementation(async (p) => ({ ...p, id: "c9" }));
    renderWithClient(<ConnectionForm initial={initial} tabId="t" />);
    expect(screen.getByLabelText("Файл ключа")).toHaveValue("/k/id");
    expect(screen.getByLabelText("Пассфраза ключа")).toBeInTheDocument();
    await user.click(screen.getByLabelText("Через SSH-туннель"));
    expect(screen.queryByLabelText("Цель SSH")).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("Имя"), "x");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
    expect(vi.mocked(api.saveConnection).mock.calls[0][0].ssh).toBeNull();
  });

  it("проверка с неизвестным ключом хоста предлагает доверять и повторяет проверку", async () => {
    const user = userEvent.setup();
    const hostKey: AppError = { code: "SSH_ERROR", key: "errors.ssh.hostKeyUnknown", message: "unknown host key", retryable: false, params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion (10.0.0.5)", fingerprint: "SHA256:abc" } };
    vi.mocked(api.testConnection).mockRejectedValueOnce(hostKey).mockResolvedValueOnce(EMPTY_REPORT);
    vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), name: "n", ssh: { ...emptySsh(), target: "bastion" } }} tabId="t" />);
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
    expect(dialog).toHaveTextContent("bastion (10.0.0.5)");
    expect(dialog).toHaveTextContent("SHA256:abc");
    await user.click(within(dialog).getByRole("button", { name: "Доверять и подключиться" }));
    expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
    await waitFor(() => expect(api.testConnection).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
```

(в `vi.mock` клиента формы добавить `trustSshHostKey: vi.fn()`, `connectionSecrets` уже есть.)

`src/components/connections/tree/ConnectionNode.test.tsx`:

```tsx
  it("неизвестный ключ хоста при подключении из дерева: диалог, доверие, повтор", async () => {
    const hostKey = { code: "SSH_ERROR", key: "errors.ssh.hostKeyUnknown", message: "unknown host key", retryable: false, params: { kind: "hostKeyUnknown", target: "bastion", host: "bastion", fingerprint: "SHA256:abc" } };
    vi.mocked(api.connect).mockRejectedValueOnce(hostKey).mockResolvedValueOnce(topologyView());
    vi.mocked(api.trustSshHostKey).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderNode();
    await screen.findByRole("img", { name: "Не подключено" });
    await user.click(screen.getByRole("button", { name: toggleName }));
    const dialog = await screen.findByRole("dialog", { name: "Неизвестный ключ хоста" });
    await user.click(within(dialog).getByRole("button", { name: "Доверять и подключиться" }));
    expect(api.trustSshHostKey).toHaveBeenCalledWith("bastion", "SHA256:abc");
    expect(await screen.findByRole("img", { name: "Подключено" })).toBeInTheDocument();
    expect(api.connect).toHaveBeenCalledTimes(2);
  });
```

`src/components/connections/TestPanel.test.tsx`: событие `{ step: "ssh", status: "ok", message: { key: "test.sshOk", params: { version: "9.6" }, text: "…" } }` рендерится как «SSH: OpenSSH 9.6, туннель поднят».

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm exec vitest run src/components/connections src/components/overview`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`profile.ts`:

```ts
export function emptySsh(): SshSettings {
  return { target: "", port: null, user: null, auth: "agent", keyPath: null };
}

/** Ошибка «ключ хоста ssh ещё не известен» (SSH_ERROR с params.kind = hostKeyUnknown): данные для диалога доверия. */
export function hostKeyUnknown(e: AppError | null | undefined): { target: string; host: string; fingerprint: string } | null {
  if (!e || e.code !== "SSH_ERROR" || e.params?.kind !== "hostKeyUnknown") return null;
  return { target: e.params.target ?? "", host: e.params.host ?? e.params.target ?? "", fingerprint: e.params.fingerprint ?? "" };
}
```

`portProblem`: `const ports = [...(p.kind === "patroni" ? … : p.hosts.map((h) => h.port)), ...(p.ssh?.port !== null && p.ssh?.port !== undefined ? [p.ssh.port] : [])];`.

`SshSection.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { ConnectionProfile, SecretsSet, SshAuth, SshSettings } from "../../api/types";
import { Hint } from "../common/Hint";
import { FieldGroup, PathRow, RowLabel, SecretRow } from "./fields";
import { emptySsh, isValidPort, type SecretField } from "./profile";

export type SshSecrets = { password: SecretField; passphrase: SecretField };
const AUTHS: SshAuth[] = ["agent", "key", "password"];

/** Раздел «SSH» формы (спец. P1 §12): выключатель, цель, порт, пользователь, способ входа, ключ и секреты. */
export function SshSection(props: { idPrefix: string; profile: ConnectionProfile; onChange: (p: ConnectionProfile) => void; secrets: SshSecrets; onSecrets: (s: SshSecrets) => void; saved: SecretsSet }) {
  const { t } = useTranslation();
  const { profile: p, onChange, idPrefix: id } = props;
  const ssh = p.ssh;
  const set = (patch: Partial<SshSettings>) => onChange({ ...p, ssh: { ...(ssh ?? emptySsh()), ...patch } });
  return (
    <FieldGroup legend={t("connections.form.ssh.section")}>
      <label htmlFor={`${id}-ssh-on`}>{t("connections.form.ssh.enabled")}</label>
      <input id={`${id}-ssh-on`} type="checkbox" checked={ssh !== null} onChange={(e) => onChange({ ...p, ssh: e.target.checked ? emptySsh() : null })} />
      {ssh && (
        <>
          <RowLabel id={`${id}-ssh-target`} label={t("connections.form.ssh.target")} hint={<Hint k="hints.connections.sshTarget" label={t("connections.form.ssh.target")} />} />
          <input id={`${id}-ssh-target`} value={ssh.target} onChange={(e) => set({ target: e.target.value })} />
          <label htmlFor={`${id}-ssh-port`}>{t("connections.form.ssh.port")}</label>
          <input id={`${id}-ssh-port`} type="number" min={1} max={65535} placeholder={t("connections.form.ssh.fromConfig")} aria-invalid={ssh.port !== null && !isValidPort(ssh.port)} value={ssh.port ?? ""} onChange={(e) => set({ port: e.target.value === "" ? null : Number(e.target.value) })} />
          <label htmlFor={`${id}-ssh-user`}>{t("connections.form.ssh.user")}</label>
          <input id={`${id}-ssh-user`} value={ssh.user ?? ""} placeholder={t("connections.form.ssh.fromConfig")} onChange={(e) => set({ user: e.target.value || null })} />
          <label htmlFor={`${id}-ssh-auth`}>{t("connections.form.ssh.auth")}</label>
          <select id={`${id}-ssh-auth`} value={ssh.auth} onChange={(e) => set({ auth: e.target.value as SshAuth })}>
            {AUTHS.map((a) => (
              <option key={a} value={a}>
                {t(`connections.form.ssh.auths.${a}`)}
              </option>
            ))}
          </select>
          {ssh.auth === "key" && (
            <>
              <PathRow id={`${id}-ssh-key`} label={t("connections.form.ssh.keyPath")} value={ssh.keyPath} onChange={(keyPath) => set({ keyPath })} />
              <SecretRow id={`${id}-ssh-passphrase`} label={t("connections.form.ssh.passphrase")} saved={props.saved.sshPassphrase} value={props.secrets.passphrase} onChange={(passphrase) => props.onSecrets({ ...props.secrets, passphrase })} />
            </>
          )}
          {ssh.auth === "password" && <SecretRow id={`${id}-ssh-password`} label={t("connections.form.ssh.password")} saved={props.saved.sshPassword} value={props.secrets.password} onChange={(password) => props.onSecrets({ ...props.secrets, password })} />}
        </>
      )}
    </FieldGroup>
  );
}
```

`ConnectionForm.tsx`: состояние `sshSecrets`; `secrets()` — `sshPassword: secretValue(sshSecrets.password), sshPassphrase: secretValue(sshSecrets.passphrase)`; раздел `<SshSection …/>` после `PgSection`; обработка ошибки проверки: `const trust = hostKeyUnknown(error)`; рендер `{trust && <TrustHostKeyDialog error={error!} onTrusted={() => { setError(null); void onTest(); }} onCancel={() => setError(null)} />}`. `visibleProfile` — без изменений (`ssh` уходит как есть; при выключенном флажке `null`).

`TrustHostKeyDialog.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError } from "../../api/types";
import { hostKeyUnknown } from "./profile";

/** Диалог доверия ключу хоста ssh (спец. P1 §5.2): хост, отпечаток, «Доверять и подключиться» → trust_ssh_host_key, затем повтор. */
export function TrustHostKeyDialog({ error, onTrusted, onCancel }: { error: AppError; onTrusted: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<AppError | null>(null);
  const info = hostKeyUnknown(error);
  if (!info) return null;
  const trust = async () => {
    setBusy(true);
    try {
      await api.trustSshHostKey(info.target, info.fingerprint);
      onTrusted();
    } catch (e) {
      setFailure(toAppError(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="dialog-backdrop">
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="host-key-title">
        <h3 id="host-key-title">{t("hostKey.title")}</h3>
        <p>{t("hostKey.explain")}</p>
        <dl>
          <dt>{t("hostKey.host")}</dt>
          <dd>{info.host}</dd>
          <dt>{t("hostKey.fingerprint")}</dt>
          <dd>
            <code>{info.fingerprint}</code>
          </dd>
        </dl>
        {failure && <p className="warn">{failure.message}</p>}
        <div className="dialog-actions">
          <button type="button" onClick={onCancel} disabled={busy}>
            {t("common.cancel")}
          </button>
          <button type="button" className="primary" onClick={() => void trust()} disabled={busy}>
            {t("hostKey.trust")}
          </button>
        </div>
      </div>
    </div>
  );
}
```

(классы `dialog-backdrop`/`dialog`/`dialog-actions` — как у `ImportDialog`; использовать те же.) `ConnectionNode.tsx` и `Overview.tsx`: `{hostKeyUnknown(error) && <TrustHostKeyDialog error={error!} onTrusted={() => { setError(null); void connect(); }} onCancel={() => setError(null)} />}`. `ImportDialog`: предпросмотр «нужен пароль» учитывает `needsPassword.ssh`/`sshKey`, если список полей выводится.

Словари: `connections.form.ssh.{section: "SSH" / «SSH», enabled: "Through an SSH tunnel" / «Через SSH-туннель», target: "SSH target" / «Цель SSH», port: "SSH port" / «Порт SSH», user: "SSH user" / «Пользователь SSH», fromConfig: "as in ~/.ssh/config" / «как в ~/.ssh/config», auth: "Bastion login" / «Вход на бастион», auths: { agent: "SSH agent" / «SSH-агент», key: "Key file" / «Файл ключа», password: "Password" / «Пароль» }, keyPath: "Key file" / «Файл ключа», passphrase: "Key passphrase" / «Пассфраза ключа», password: "SSH password" / «Пароль SSH"}`; `hints.connections.sshTarget` («Хост или алиас из ~/.ssh/config: учитываются ProxyJump, агент и аппаратные ключи. Порт и пользователь — как в конфиге, если не заданы.»); `hostKey.{title: "Unknown host key" / «Неизвестный ключ хоста», explain: "The bastion presented a key that is not in known_hosts yet. Compare the fingerprint with the one your administrator gave you." / «Бастион предъявил ключ, которого ещё нет в known_hosts. Сверьте отпечаток с тем, что дал администратор.», host: "Host" / «Хост», fingerprint: "Fingerprint" / «Отпечаток», trust: "Trust and connect" / «Доверять и подключиться»}`; `test.step.ssh` «SSH» (в `same-as-en.json`). Запятые/порядок — по алфавиту внутри групп, `hints` последним.

- [ ] **Step 4: Прогнать проверки и commit**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm test:scripts && pnpm build`.

```bash
git add src docs/i18n
git commit -m "feat(ui): раздел SSH в форме, пароль и пассфраза, диалог доверия ключу хоста

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Стенд `bastion`, интеграционные тесты SSH, CI

**Files:**
- Create: `dev/ssh/Dockerfile`, `dev/ssh/sshd_config`, `dev/ssh/gen-keys.sh`, `dev/ssh/README.md`, `src-tauri/tests/it_ssh.rs`
- Modify: `dev/docker-compose.yml`, `.gitignore`, `scripts/wait-stand.sh`, `.github/workflows/ci.yml`, `README.md`, `src-tauri/tests/common/mod.rs`
- Test: `it_ssh.rs` (переменные `GPG_IT_BASTION`, `GPG_IT_SSH_KEYS`)

**Interfaces:**
- Consumes: `TunnelRegistry` (задача 4), `ConnectSpec::build_via`, `ReqwestHttp::new(…, proxy)`, `AdhocConnector`, `observe_hosts`.
- Produces: сервис `bastion` (`127.0.0.1:2222`), пользователь `tester`, пароль `tester-secret`, ключи `dev/ssh/keys/tester_ed25519` (без пассфразы) и `tester_ed25519_pass` (пассфраза `test-passphrase`); `common::{bastion_settings(auth) -> Option<SshSettings>, ssh_keys_dir() -> Option<PathBuf>, tunnel_registry_for_tests(dir) -> Arc<TunnelRegistry>}`.

- [ ] **Step 1: Стенд**

`dev/ssh/gen-keys.sh`:

```bash
#!/usr/bin/env bash
# Тестовые ключи пользователя tester для bastion: без пассфразы и с пассфразой test-passphrase.
# Каталог keys/ в .gitignore — приватные ключи в репозиторий не попадают.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p keys
[ -f keys/tester_ed25519 ] || ssh-keygen -q -t ed25519 -N '' -C tester -f keys/tester_ed25519
[ -f keys/tester_ed25519_pass ] || ssh-keygen -q -t ed25519 -N 'test-passphrase' -C tester-pass -f keys/tester_ed25519_pass
chmod 600 keys/tester_ed25519 keys/tester_ed25519_pass
echo "keys ready in $(pwd)/keys"
```

`dev/ssh/Dockerfile`:

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /run/sshd \
 && useradd -m -s /bin/bash tester \
 && echo 'tester:tester-secret' | chpasswd \
 && ssh-keygen -A
COPY ssh/sshd_config /etc/ssh/sshd_config
COPY ssh/keys/tester_ed25519.pub ssh/keys/tester_ed25519_pass.pub /tmp/
RUN mkdir -p /home/tester/.ssh && cat /tmp/tester_ed25519.pub /tmp/tester_ed25519_pass.pub > /home/tester/.ssh/authorized_keys \
 && chown -R tester:tester /home/tester/.ssh && chmod 700 /home/tester/.ssh && chmod 600 /home/tester/.ssh/authorized_keys
EXPOSE 22
CMD ["/usr/sbin/sshd", "-D", "-e"]
```

`dev/ssh/sshd_config`:

```
Port 22
PermitRootLogin no
PubkeyAuthentication yes
PasswordAuthentication yes
KbdInteractiveAuthentication no
AllowTcpForwarding yes
GatewayPorts no
X11Forwarding no
AuthorizedKeysFile .ssh/authorized_keys
Subsystem sftp /usr/lib/openssh/sftp-server
```

`dev/docker-compose.yml` — сервис:

```yaml
  bastion:
    build:
      context: .
      dockerfile: ssh/Dockerfile
    image: glafira-pg-dev/bastion
    container_name: gp-bastion
    hostname: bastion
    ports: ["127.0.0.1:2222:22"]
```

(контекст `.` — каталог `dev`, как у Patroni; перед `up --build` нужен `dev/ssh/gen-keys.sh`.) `.gitignore`: `dev/ssh/keys/`. `scripts/wait-stand.sh`: `wait_for bash -c 'exec 3<>/dev/tcp/127.0.0.1/2222'` после PG. `dev/ssh/README.md`: учётные данные, команда генерации ключей, проверка руками: `ssh -p 2222 -i dev/ssh/keys/tester_ed25519 tester@localhost`. `README.md`: раздел стенда — `bastion` и `gen-keys.sh`.

- [ ] **Step 2: Интеграционные тесты**

`src-tauri/tests/common/mod.rs`:

```rust
/// Адрес bastion (`GPG_IT_BASTION=localhost:2222`) и каталог ключей (`GPG_IT_SSH_KEYS`, абсолютный путь).
pub fn bastion_settings(auth: SshAuth) -> Option<SshSettings> {
    let (host, port) = host_port(&env("GPG_IT_BASTION")?);
    let keys = ssh_keys_dir()?;
    Some(SshSettings {
        target: host,
        port: Some(port),
        user: Some("tester".into()),
        auth,
        key_path: match auth {
            SshAuth::Key => Some(keys.join("tester_ed25519").to_string_lossy().into_owned()),
            _ => None,
        },
    })
}

pub fn ssh_keys_dir() -> Option<PathBuf> {
    env("GPG_IT_SSH_KEYS").map(PathBuf::from)
}

/// Реестр туннелей для тестов: свой known_hosts во временном каталоге, секреты для askpass — в файле 0600
/// (keychain на CI недоступен).
pub fn tunnel_registry_for_tests(dir: &Path, pools: Arc<PoolRegistry>) -> Arc<TunnelRegistry> {
    let mut cfg = TunnelConfig::new(dir.join("data"), dir.join("logs"));
    cfg.known_hosts = Some(dir.join("known_hosts"));
    cfg.stage_secrets_in_file = true;
    // Askpass умеет только бинарь приложения; cargo собирает его для интеграционных тестов и даёт путь.
    cfg.askpass_bin = Some(PathBuf::from(env!("CARGO_BIN_EXE_glafira-pg")));
    TunnelRegistry::new(cfg, pools, Arc::new(MemoryStore::default()))
}
```

`src-tauri/tests/it_ssh.rs`:

```rust
//! Подключения через bastion стенда (`GPG_IT_BASTION=localhost:2222`, `GPG_IT_SSH_KEYS=<путь к dev/ssh/keys>`):
//! ключ, ключ с пассфразой и пароль через askpass, неизвестный ключ хоста и доверие, неверный пароль,
//! Patroni и список узлов через туннель. Имена узлов (`pg-single`, `patroni1`…) разрешает бастион.

mod common;

use std::sync::Arc;
use std::time::Duration;

use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::pool::PoolRegistry;
use glafira_pg_lib::conn::profile::{ConnectionKind, HostPort, SshAuth, SslMode};
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SSH_PASSPHRASE, SSH_PASSWORD, SecretStore, secret_key};
use glafira_pg_lib::topology::connector::AdhocConnector;
use glafira_pg_lib::topology::patroni::{ReqwestHttp, discover};
use glafira_pg_lib::topology::probe::observe_hosts;

const TIMEOUT: Duration = Duration::from_secs(20);

fn setup(auth: SshAuth) -> Option<(tempfile::TempDir, Arc<glafira_pg_lib::net::ssh::tunnel::TunnelRegistry>, glafira_pg_lib::conn::profile::ConnectionProfile)> {
    let ssh = common::bastion_settings(auth)?;
    let dir = tempfile::tempdir().unwrap();
    let reg = common::tunnel_registry_for_tests(dir.path(), Arc::new(PoolRegistry::default()));
    // Отпечаток bastion заранее неизвестен: первый запуск спотыкается о ключ хоста — доверяем ему явно.
    let mut p = common::server_profile("ssh", "pg-single:5432");
    p.ssh = Some(ssh);
    Some((dir, reg, p))
}

/// Первое подключение: неизвестный ключ → `hostKeyUnknown` с отпечатком → доверие → успех.
async fn open_trusting(reg: &Arc<glafira_pg_lib::net::ssh::tunnel::TunnelRegistry>, p: &glafira_pg_lib::conn::profile::ConnectionProfile) -> std::net::SocketAddr {
    match reg.open(p, TIMEOUT).await {
        Ok(addr) => addr,
        Err(e) => {
            assert_eq!(e.params["kind"], "hostKeyUnknown", "{e:?}");
            assert!(e.params["fingerprint"].starts_with("SHA256:"));
            reg.trust(&p.ssh.as_ref().unwrap().target, &e.params["fingerprint"]);
            reg.open(p, TIMEOUT).await.unwrap()
        }
    }
}

async fn query_version(socks: std::net::SocketAddr, p: &glafira_pg_lib::conn::profile::ConnectionProfile) -> String {
    let secrets = common::secrets_with_password("ssh");
    let spec = ConnectSpec::build_via(p, "pg-single", 5432, "postgres", secrets.as_ref(), Some(socks)).unwrap();
    let c = spec.connect().await.unwrap();
    c.query_one("SELECT version()", &[]).await.unwrap().get::<_, String>(0)
}

#[tokio::test]
async fn key_without_passphrase_opens_a_tunnel_to_a_single_server() {
    let Some((_d, reg, p)) = setup(SshAuth::Key) else { return };
    let socks = open_trusting(&reg, &p).await;
    assert!(query_version(socks, &p).await.contains("PostgreSQL 15"));
    reg.close("ssh");
}

#[tokio::test]
async fn key_with_passphrase_uses_askpass() {
    let Some((_d, reg, mut p)) = setup(SshAuth::Key) else { return };
    p.ssh.as_mut().unwrap().key_path = Some(common::ssh_keys_dir().unwrap().join("tester_ed25519_pass").to_string_lossy().into_owned());
    reg.stage_secret("ssh", SSH_PASSPHRASE, "test-passphrase").unwrap();
    let socks = open_trusting(&reg, &p).await;
    assert!(query_version(socks, &p).await.contains("PostgreSQL"));
    reg.close("ssh");
    reg.unstage_secrets("ssh");
}

#[tokio::test]
async fn password_login_uses_askpass_and_a_wrong_password_is_auth_failed() {
    let Some((_d, reg, p)) = setup(SshAuth::Password) else { return };
    reg.stage_secret("ssh", SSH_PASSWORD, "wrong").unwrap();
    let e = match reg.open(&p, TIMEOUT).await {
        Err(e) if e.params["kind"] == "hostKeyUnknown" => {
            reg.trust(&p.ssh.as_ref().unwrap().target, &e.params["fingerprint"]);
            reg.open(&p, TIMEOUT).await.unwrap_err()
        }
        Err(e) => e,
        Ok(_) => panic!("неверный пароль принят"),
    };
    assert_eq!((e.code, e.params["kind"].as_str()), (ErrorCode::SshError, "authFailed"));
    reg.stage_secret("ssh", SSH_PASSWORD, "tester-secret").unwrap();
    let socks = reg.open(&p, TIMEOUT).await.unwrap();
    assert!(query_version(socks, &p).await.contains("PostgreSQL"));
    reg.close("ssh");
    reg.unstage_secrets("ssh");
}

#[tokio::test]
async fn patroni_and_host_list_are_observed_through_the_tunnel() {
    let Some((_d, reg, p)) = setup(SshAuth::Key) else { return };
    let socks = open_trusting(&reg, &p).await;
    // Patroni: адреса API по именам контейнеров, разрешает бастион.
    let settings = glafira_pg_lib::conn::profile::PatroniSettings {
        urls: vec!["http://patroni1:8008".into(), "http://patroni2:8008".into(), "http://patroni3:8008".into()],
        username: Some("patroni".into()),
        tls: None,
        host_overrides: vec![],
    };
    let http = ReqwestHttp::new(&settings, Some(common::PATRONI_PASSWORD_VALUE.into()), None, Some(socks)).unwrap();
    let t = discover(&http, &settings, "ssh", None, 0).await.unwrap();
    assert_eq!(t.members.len(), 3);
    assert!(t.leader().is_some());
    // Список узлов: primary и реплика по именам.
    let mut hl = p.clone();
    hl.kind = ConnectionKind::HostList;
    hl.hosts = vec![HostPort { host: "pg-primary".into(), port: 5432 }, HostPort { host: "pg-replica".into(), port: 5432 }];
    hl.pg.ssl_mode = SslMode::Prefer;
    let secrets = common::secrets_with_password("ssh");
    let nodes = AdhocConnector { profile: hl.clone(), secrets: secrets.clone(), socks: Some(socks) };
    let t = observe_hosts("ssh", ConnectionKind::HostList, &hl.hosts, &nodes, 0).await;
    assert!(t.members.iter().any(|m| m.role == glafira_pg_lib::topology::model::Role::Primary));
    reg.close("ssh");
}
```

(имена и сигнатуры `observe_hosts`, `PATRONI_PASSWORD_VALUE` — из `common` и `topology/probe.rs`; при расхождении использовать настоящие. Пароль PG роли `glafira_diag` для `pg-single` через `secrets_with_password("ssh")`.)

`.github/workflows/ci.yml`, job `linux`: перед «Стенд» шаг `- name: Ключи bastion\n  run: dev/ssh/gen-keys.sh`; в env тестов `GPG_IT_BASTION: localhost:2222`, `GPG_IT_SSH_KEYS: ${{ github.workspace }}/dev/ssh/keys`.

- [ ] **Step 3: Поднять bastion и прогнать**

Run: `dev/ssh/gen-keys.sh && docker compose -f dev/docker-compose.yml up -d --build bastion && scripts/wait-stand.sh`, затем `GPG_IT_BASTION=localhost:2222 GPG_IT_SSH_KEYS=$PWD/dev/ssh/keys GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1`.
Expected: `it_ssh` 4 passed, остальные без изменений.

- [ ] **Step 4: Commit**

```bash
git add dev .gitignore scripts/wait-stand.sh .github/workflows/ci.yml README.md src-tauri/tests
git commit -m "feat(stand): bastion с OpenSSH и тестовыми ключами, интеграционные тесты туннеля, CI

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Каталог — детали таблицы, представления, функции и последовательности

**Files:**
- Create: `src-tauri/src/catalog/table.rs`, `src-tauri/src/catalog/view.rs`, `src-tauri/src/catalog/function.rs`, `src-tauri/src/catalog/sequence.rs`
- Modify: `src-tauri/src/catalog/mod.rs`, `src-tauri/src/commands/catalog.rs`, `src-tauri/src/lib.rs`, `src-tauri/tests/it_catalog.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `catalog/mod.rs` (выбор текста запроса по версии), `catalog/sequence.rs` (процент), интеграционные в `it_catalog.rs`

**Interfaces:**
- Consumes: `on_catalog_node`, `from_pg`, `objects::partitions`.
- Produces (все структуры — `Serialize + Deserialize + Clone + Debug + PartialEq`, camelCase):
  - `catalog::table::{Column { name, data_type, not_null, default_expr: Option<String>, identity: Option<String> ("a"|"d"), generated: Option<String> ("s"), collation: Option<String>, stat_target: Option<i32>, storage: String, comment: Option<String> }, Index { name, definition, primary, unique, valid, size_bytes, idx_scan: Option<i64>, idx_tup_read: Option<i64>, last_idx_scan_ms: Option<i64> }, Constraint { name, kind: String ("p"|"u"|"f"|"c"|"x"|"t"|"n"), definition, validated: bool }, IncomingFk { constraint, schema, table, definition }, TableStats { heap_bytes, index_bytes, toast_bytes, total_bytes, reltuples: f64, seq_scan, idx_scan, n_live_tup, n_dead_tup, n_mod_since_analyze, n_ins_since_vacuum, last_vacuum_ms, last_autovacuum_ms, last_analyze_ms, last_autoanalyze_ms, vacuum_count, autovacuum_count, analyze_count, autoanalyze_count (все счётчики Option<i64>) }, Partitioning { key: Option<String>, bound: Option<String>, parent: Option<String>, partitions: Vec<String> }, Trigger { name, enabled: String, definition }, TableDetails { oid, schema, name, relkind, owner, tablespace: Option<String>, comment: Option<String>, reloptions: Vec<String>, columns, indexes, constraints, incoming_fks, stats, partitioning, triggers }, get(&Client, schema, name) -> AppResult<TableDetails>}`;
  - `catalog::view::{ViewDetails { oid, schema, name, matview: bool, owner, comment, definition, columns: Vec<Column>, populated: Option<bool>, size_bytes: Option<i64>, indexes: Vec<Index> }, get(&Client, schema, name)}`;
  - `catalog::function::{FunctionDetails { oid, schema, name, kind: String ("f"|"p"|"a"|"w"), signature, returns: Option<String>, language, volatility: String, security_definer, owner, comment, definition: Option<String> }, get(&Client, oid: i64)}`;
  - `catalog::sequence::{SequenceDetails { oid, schema, name, data_type, start, increment, min, max, cycle, cache, last_value: Option<i64>, percent_used: Option<f64>, owned_by: Option<String> }, get(&Client, schema, name), percent_used(last, min, max, increment) -> Option<f64>}`;
  - команды `get_table(id, db, schema, name)`, `get_view(id, db, schema, name)`, `get_function(id, db, oid)`, `get_sequence(id, db, schema, name)`; `catalog::server_version_num(&Client) -> AppResult<i32>`; ключ `errors.catalog.objectNotFound`.

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/catalog/sequence.rs`, `tests`:

```rust
    #[test]
    fn percent_used_counts_from_the_start_in_the_direction_of_the_increment() {
        assert_eq!(percent_used(Some(50), 1, 100, 1), Some(49.49494949494949));
        assert_eq!(percent_used(Some(100), 1, 100, 1), Some(100.0));
        assert_eq!(percent_used(Some(1), 1, 100, 1), Some(0.0));
        // Убывающая: использовано расстояние от max.
        assert_eq!(percent_used(Some(-50), -100, -1, -1), Some(49.49494949494949));
        assert_eq!(percent_used(None, 1, 100, 1), None);
        assert_eq!(percent_used(Some(5), 5, 5, 1), None, "пустой диапазон");
    }
```

`src-tauri/src/catalog/mod.rs`, `tests`:

```rust
    #[test]
    fn index_query_mentions_last_idx_scan_only_on_16_plus() {
        assert!(table::index_sql(160000).contains("last_idx_scan"));
        assert!(!table::index_sql(150004).contains("last_idx_scan"));
    }
```

`src-tauri/tests/it_catalog.rs` — в `demo_schema_on_every_node` после ролей:

```rust
        let tbl = table::get(&c, "shop", "orders").await.unwrap();
        assert_eq!(tbl.columns.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["id", "customer_id", "total", "total_with_tax", "status"]);
        let total = &tbl.columns[2];
        assert_eq!((total.data_type.as_str(), total.not_null), ("numeric(12,2)", true));
        assert_eq!(tbl.columns[3].generated.as_deref(), Some("s"));
        assert_eq!(tbl.columns[0].default_expr.as_deref(), Some("nextval('shop.orders_id_seq'::regclass)"));
        assert!(tbl.indexes.iter().any(|i| i.name == "orders_pkey" && i.primary && i.valid));
        assert!(tbl.indexes.iter().any(|i| i.name == "orders_customer_idx" && !i.unique));
        assert!(tbl.constraints.iter().any(|k| k.kind == "f" && k.definition.contains("REFERENCES shop.customers(id)")));
        assert!(tbl.constraints.iter().any(|k| k.kind == "c" && k.definition.contains("total >= 0")));
        assert!(tbl.stats.total_bytes > 0 && tbl.stats.n_live_tup.is_some());
        assert_eq!(tbl.partitioning.key, None);
        assert_eq!(tbl.owner, "postgres");

        let customers = table::get(&c, "shop", "customers").await.unwrap();
        assert_eq!(customers.comment.as_deref(), Some("Покупатели"));
        assert_eq!(customers.columns[0].identity.as_deref(), Some("a"));
        assert_eq!(customers.triggers.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["customers_touch"]);
        assert!(customers.incoming_fks.iter().any(|f| f.table == "orders"));

        let events = table::get(&c, "shop", "events").await.unwrap();
        assert_eq!(events.relkind, "p");
        assert_eq!(events.partitioning.key.as_deref(), Some("RANGE (happened_at)"));
        assert_eq!(events.partitioning.partitions, ["shop.events_2026_09", "shop.events_2026_10"]);
        let part = table::get(&c, "shop", "events_2026_09").await.unwrap();
        assert_eq!(part.partitioning.parent.as_deref(), Some("shop.events"));
        assert!(part.partitioning.bound.as_deref().unwrap().starts_with("FOR VALUES FROM"));

        let v = view::get(&c, "shop", "active_orders").await.unwrap();
        assert!(!v.matview && v.definition.contains("status <> 'done'"));
        let mv = view::get(&c, "shop", "order_totals").await.unwrap();
        assert_eq!((mv.matview, mv.populated), (true, Some(true)));
        assert!(mv.size_bytes.unwrap() > 0);

        let f_oid = funcs.items.iter().find(|f| f.name == "order_count").unwrap().oid;
        let f = function::get(&c, f_oid).await.unwrap();
        assert_eq!((f.kind.as_str(), f.language.as_str(), f.volatility.as_str()), ("f", "sql", "s"));
        assert!(f.definition.as_deref().unwrap().contains("CREATE OR REPLACE FUNCTION shop.order_count"));
        assert_eq!(f.returns.as_deref(), Some("bigint"));

        let s = sequence::get(&c, "shop", "invoice_seq").await.unwrap();
        assert_eq!((s.data_type.as_str(), s.increment, s.cycle), ("integer", 1, false));
        assert_eq!(s.last_value, None, "последовательность не использовалась");
        let owned = sequence::get(&c, "shop", "orders_id_seq").await.unwrap();
        assert_eq!(owned.owned_by.as_deref(), Some("shop.orders.id"));
        assert!(owned.last_value.is_some_and(|v| v >= 1000));

        let e = table::get(&c, "shop", "missing").await.unwrap_err();
        assert_eq!(e.key, "errors.catalog.objectNotFound");
```

(`funcs` — переменная списка функций, уже есть в тесте; импорты `glafira_pg_lib::catalog::{function, sequence, table, view}`.)

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib catalog::`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация**

`src-tauri/src/catalog/mod.rs`: `pub mod function; pub mod sequence; pub mod table; pub mod view;` и

```rust
/// Версия сервера для выбора текста запроса (`last_idx_scan` — 16+ и т. п.).
pub async fn server_version_num(c: &Client) -> AppResult<i32> {
    let r = c.query_one("SELECT current_setting('server_version_num')::int", &[]).await.map_err(|e| from_pg(&e))?;
    Ok(r.get(0))
}

/// OID отношения по схеме и имени; нет — `objectNotFound`.
pub async fn relation_oid(c: &Client, schema: &str, name: &str) -> AppResult<i64> {
    let row = c
        .query_opt(
            "SELECT c.oid::bigint FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname = $1 AND c.relname = $2",
            &[&schema, &name],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    row.map(|r| r.get(0)).ok_or_else(|| {
        app_err!(
            ErrorCode::NotFound,
            "errors.catalog.objectNotFound",
            "Object {{schema}}.{{name}} was not found (dropped or renamed?)",
            schema = schema,
            name = name
        )
    })
}

```

(`Client` — `tokio_postgres::Client`; миллисекунды эпохи из `timestamptz` берутся в SQL как `(extract(epoch FROM x) * 1000)::bigint` — без chrono.) Для OID в `$1` использовать `i64` → в SQL `$1::oid` (pg_class.oid — тип `oid`, u32; `tokio-postgres` отдаёт `oid` как `u32`: читать `oid::bigint` и передавать параметры как `i64` с явным приведением `$1::bigint::oid`? Надёжнее: `let oid: u32 = …` и параметры типа `u32` (`ToSql` для `u32` ↔ `oid` есть). Везде ниже `oid` в Rust — `u32`, в структурах наружу — `i64`.)

`src-tauri/src/catalog/table.rs` (ключевые части; структуры — как в Interfaces):

```rust
//! Вкладка таблицы (спец. P1 §9): колонки, индексы, ограничения, размер и статистика, партиционирование, триггеры.

const COLUMNS_SQL: &str = "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull, \
    pg_get_expr(d.adbin, d.adrelid), NULLIF(a.attidentity::text, ''), NULLIF(a.attgenerated::text, ''), \
    CASE WHEN a.attcollation <> 0 AND a.attcollation <> t.typcollation THEN quote_ident(cn.nspname) || '.' || quote_ident(co.collname) END, \
    a.attstattarget::int, a.attstorage::text, col_description(a.attrelid, a.attnum) \
  FROM pg_attribute a JOIN pg_type t ON t.oid = a.atttypid \
  LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
  LEFT JOIN pg_collation co ON co.oid = a.attcollation LEFT JOIN pg_namespace cn ON cn.oid = co.collnamespace \
  WHERE a.attrelid = $1 AND a.attnum > 0 AND NOT a.attisdropped ORDER BY a.attnum";

/// `last_idx_scan` появился в 16: на старых версиях — NULL, иначе запрос не разберётся.
pub fn index_sql(version_num: i32) -> String {
    let last = if version_num >= 160000 { "(extract(epoch FROM s.last_idx_scan) * 1000)::bigint" } else { "NULL::bigint" };
    format!(
        "SELECT ic.relname, pg_get_indexdef(i.indexrelid), i.indisprimary, i.indisunique, i.indisvalid, \
         pg_relation_size(i.indexrelid), s.idx_scan, s.idx_tup_read, {last} \
         FROM pg_index i JOIN pg_class ic ON ic.oid = i.indexrelid \
         LEFT JOIN pg_stat_all_indexes s ON s.indexrelid = i.indexrelid \
         WHERE i.indrelid = $1 ORDER BY ic.relname"
    )
}

const CONSTRAINTS_SQL: &str = "SELECT conname, contype::text, pg_get_constraintdef(oid, true), convalidated \
  FROM pg_constraint WHERE conrelid = $1 ORDER BY contype, conname";

const INCOMING_FK_SQL: &str = "SELECT c.conname, n.nspname, r.relname, pg_get_constraintdef(c.oid, true) \
  FROM pg_constraint c JOIN pg_class r ON r.oid = c.conrelid JOIN pg_namespace n ON n.oid = r.relnamespace \
  WHERE c.contype = 'f' AND c.confrelid = $1 ORDER BY n.nspname, r.relname, c.conname";

/// Размеры: у партиционированной таблицы — сумма по дереву партиций (одна таблица по запросу пользователя;
/// в списках дерева так не делается — заблокировались бы все партиции).
const HEAD_SQL: &str = "SELECT n.nspname, c.relname, c.relkind::text, pg_get_userbyid(c.relowner), ts.spcname, \
    obj_description(c.oid, 'pg_class'), COALESCE(c.reloptions, '{}'::text[]), c.reltuples::float8, \
    CASE WHEN c.relkind = 'p' THEN (SELECT COALESCE(sum(pg_relation_size(relid)), 0) FROM pg_partition_tree(c.oid)) ELSE pg_relation_size(c.oid) END, \
    CASE WHEN c.relkind = 'p' THEN (SELECT COALESCE(sum(pg_indexes_size(relid)), 0) FROM pg_partition_tree(c.oid)) ELSE pg_indexes_size(c.oid) END, \
    CASE WHEN c.relkind = 'p' THEN (SELECT COALESCE(sum(COALESCE(pg_total_relation_size(x.reltoastrelid), 0)), 0) FROM pg_partition_tree(c.oid) t JOIN pg_class x ON x.oid = t.relid) ELSE COALESCE(pg_total_relation_size(c.reltoastrelid), 0) END, \
    CASE WHEN c.relkind = 'p' THEN (SELECT COALESCE(sum(pg_total_relation_size(relid)), 0) FROM pg_partition_tree(c.oid)) ELSE pg_total_relation_size(c.oid) END, \
    s.seq_scan, s.idx_scan, s.n_live_tup, s.n_dead_tup, s.n_mod_since_analyze, s.n_ins_since_vacuum, \
    (extract(epoch FROM s.last_vacuum) * 1000)::bigint, (extract(epoch FROM s.last_autovacuum) * 1000)::bigint, \
    (extract(epoch FROM s.last_analyze) * 1000)::bigint, (extract(epoch FROM s.last_autoanalyze) * 1000)::bigint, \
    s.vacuum_count, s.autovacuum_count, s.analyze_count, s.autoanalyze_count, \
    pg_get_partkeydef(c.oid), pg_get_expr(c.relpartbound, c.oid), \
    (SELECT quote_ident(pn.nspname) || '.' || quote_ident(p.relname) FROM pg_inherits i JOIN pg_class p ON p.oid = i.inhparent JOIN pg_namespace pn ON pn.oid = p.relnamespace WHERE i.inhrelid = c.oid LIMIT 1) \
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
  LEFT JOIN pg_tablespace ts ON ts.oid = c.reltablespace LEFT JOIN pg_stat_all_tables s ON s.relid = c.oid \
  WHERE c.oid = $1";

const PARTITIONS_SQL: &str = "SELECT quote_ident(n.nspname) || '.' || quote_ident(c.relname) FROM pg_inherits i \
  JOIN pg_class c ON c.oid = i.inhrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
  WHERE i.inhparent = $1 ORDER BY n.nspname, c.relname";

const TRIGGERS_SQL: &str = "SELECT t.tgname, t.tgenabled::text, pg_get_triggerdef(t.oid, true) \
  FROM pg_trigger t WHERE t.tgrelid = $1 AND NOT t.tgisinternal ORDER BY t.tgname";

fn column_from(r: &Row) -> Column {
    Column {
        name: r.get(0),
        data_type: r.get(1),
        not_null: r.get(2),
        default_expr: r.get(3),
        identity: r.get(4),
        generated: r.get(5),
        collation: r.get(6),
        // В 17+ NULL — значение по умолчанию.
        stat_target: r.get(7),
        storage: r.get(8),
        comment: r.get(9),
    }
}

fn index_from(r: &Row) -> Index {
    Index {
        name: r.get(0),
        definition: r.get(1),
        primary: r.get(2),
        unique: r.get(3),
        valid: r.get(4),
        size_bytes: r.get(5),
        idx_scan: r.get(6),
        idx_tup_read: r.get(7),
        last_idx_scan_ms: r.get(8),
    }
}

async fn rows(c: &Client, sql: &str, oid: u32) -> AppResult<Vec<Row>> {
    c.query(sql, &[&oid]).await.map_err(|e| from_pg(&e))
}

pub async fn get(c: &Client, schema: &str, name: &str) -> AppResult<TableDetails> {
    let oid = relation_oid(c, schema, name).await? as u32;
    let version = server_version_num(c).await?;
    let h = c.query_one(HEAD_SQL, &[&oid]).await.map_err(|e| from_pg(&e))?;
    let relkind: String = h.get(2);
    let columns = rows(c, COLUMNS_SQL, oid).await?.iter().map(column_from).collect();
    let indexes = rows(c, &index_sql(version), oid).await?.iter().map(index_from).collect();
    let constraints = rows(c, CONSTRAINTS_SQL, oid)
        .await?
        .iter()
        .map(|r| Constraint { name: r.get(0), kind: r.get(1), definition: r.get(2), validated: r.get(3) })
        .collect();
    let incoming_fks = rows(c, INCOMING_FK_SQL, oid)
        .await?
        .iter()
        .map(|r| IncomingFk { constraint: r.get(0), schema: r.get(1), table: r.get(2), definition: r.get(3) })
        .collect();
    let partitions = if relkind == "p" {
        rows(c, PARTITIONS_SQL, oid).await?.iter().map(|r| r.get::<_, String>(0)).collect()
    } else {
        Vec::new()
    };
    let triggers = rows(c, TRIGGERS_SQL, oid)
        .await?
        .iter()
        .map(|r| Trigger { name: r.get(0), enabled: r.get(1), definition: r.get(2) })
        .collect();
    let mut details = TableDetails {
        oid: oid as i64,
        schema: h.get(0),
        name: h.get(1),
        relkind,
        owner: h.get(3),
        tablespace: h.get(4),
        comment: h.get(5),
        reloptions: h.get(6),
        stats: TableStats {
            reltuples: h.get(7),
            heap_bytes: h.get(8),
            index_bytes: h.get(9),
            toast_bytes: h.get(10),
            total_bytes: h.get(11),
            seq_scan: h.get(12),
            idx_scan: h.get(13),
            n_live_tup: h.get(14),
            n_dead_tup: h.get(15),
            n_mod_since_analyze: h.get(16),
            n_ins_since_vacuum: h.get(17),
            last_vacuum_ms: h.get(18),
            last_autovacuum_ms: h.get(19),
            last_analyze_ms: h.get(20),
            last_autoanalyze_ms: h.get(21),
            vacuum_count: h.get(22),
            autovacuum_count: h.get(23),
            analyze_count: h.get(24),
            autoanalyze_count: h.get(25),
        },
        partitioning: Partitioning { key: h.get(26), bound: h.get(27), parent: h.get(28), partitions },
        columns,
        indexes,
        constraints,
        incoming_fks,
        triggers,
        ddl: String::new(),
    };
    details.ddl = super::ddl::table_ddl(&details);
    Ok(details)
}
```

(`Row` — `tokio_postgres::Row`; типы колонок `HEAD_SQL` по порядку: 0 `nspname` text, 1 `relname` text, 2 `relkind` text, 3 owner text, 4 `spcname` Option<String>, 5 comment Option<String>, 6 `reloptions` Vec<String>, 7 `reltuples` f64, 8–11 размеры i64 (суммы из `pg_partition_tree` — `numeric`: обернуть каждую ветку `CASE … END::bigint`), 12–17 и 22–25 счётчики Option<i64>, 18–21 Option<i64> миллисекунды, 26 `pg_get_partkeydef` Option<String>, 27 `relpartbound` Option<String>, 28 родитель Option<String>. До задачи 8 `details.ddl = String::new()` — строку `super::ddl::table_ddl` добавляет задача 8.)

`view.rs`: `get` — `relation_oid`, затем `SELECT c.relkind::text, pg_get_userbyid(c.relowner), obj_description(c.oid, 'pg_class'), pg_get_viewdef(c.oid, true), c.relispopulated, CASE WHEN c.relkind = 'm' THEN pg_total_relation_size(c.oid) END FROM pg_class c WHERE c.oid = $1` (`relkind` не `v`/`m` → `objectNotFound`), колонки — `COLUMNS_SQL` из `table`, индексы (только мат. представление) — `index_sql`.

`function.rs`:

```rust
const SQL: &str = "SELECT n.nspname, p.proname, p.prokind::text, pg_get_function_identity_arguments(p.oid), \
    CASE WHEN p.prokind IN ('f', 'w') THEN pg_get_function_result(p.oid) END, l.lanname, p.provolatile::text, p.prosecdef, \
    pg_get_userbyid(p.proowner), obj_description(p.oid, 'pg_proc'), \
    CASE WHEN p.prokind = 'a' THEN NULL ELSE pg_get_functiondef(p.oid) END \
  FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace JOIN pg_language l ON l.oid = p.prolang WHERE p.oid = $1";
```

(`query_opt`; нет строки → `objectNotFound` с `schema = ""`, `name = oid`.)

`sequence.rs`:

```rust
const SQL: &str = "SELECT n.nspname, c.relname, format_type(s.seqtypid, NULL), s.seqstart, s.seqincrement, s.seqmin, s.seqmax, s.seqcycle, s.seqcache, \
    (SELECT last_value FROM pg_sequences ps WHERE ps.schemaname = n.nspname AND ps.sequencename = c.relname), \
    (SELECT quote_ident(tn.nspname) || '.' || quote_ident(t.relname) || '.' || quote_ident(a.attname) \
       FROM pg_depend d JOIN pg_class t ON t.oid = d.refobjid JOIN pg_namespace tn ON tn.oid = t.relnamespace \
       JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
       WHERE d.objid = c.oid AND d.classid = 'pg_class'::regclass AND d.refclassid = 'pg_class'::regclass AND d.deptype IN ('a', 'i') LIMIT 1) \
  FROM pg_sequence s JOIN pg_class c ON c.oid = s.seqrelid JOIN pg_namespace n ON n.oid = c.relnamespace WHERE s.seqrelid = $1";

/// Доля использованного диапазона: для растущей — от min, для убывающей — от max; пустой диапазон — None.
pub fn percent_used(last: Option<i64>, min: i64, max: i64, increment: i64) -> Option<f64> {
    let last = last?;
    let span = (max as f64) - (min as f64);
    if span <= 0.0 {
        return None;
    }
    let used = if increment >= 0 { last as f64 - min as f64 } else { max as f64 - last as f64 };
    Some((used / span * 100.0).clamp(0.0, 100.0))
}
```

`commands/catalog.rs`: четыре команды через `on_catalog_node(&state, &id, Some(&db), move |c| Box::pin(async move { table::get(c, &schema, &name).await }))` (как `list_objects`); регистрация в `lib.rs`. Словарь: `errors.catalog.objectNotFound` (ru «Объект {{schema}}.{{name}} не найден (удалён или переименован?)»).

- [ ] **Step 4: Прогнать проверки**

Run: lib-тесты, clippy, fmt, i18n:check и интеграционные тесты со стендом (`it_catalog` на PG 15, 16 и Patroni 17).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(catalog): детали таблицы, представления, функции и последовательности для вкладок

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: Генератор DDL со снимками

**Files:**
- Create: `src-tauri/src/catalog/ddl.rs`, `src-tauri/tests/snapshots/ddl/{orders.json, orders.sql, events.json, events.sql, customers.json, customers.sql}`, `src-tauri/tests/ddl_snapshots.rs`
- Modify: `src-tauri/src/catalog/mod.rs`, `src-tauri/src/catalog/table.rs` (`TableDetails::ddl()`, поле `ddl: String` в ответе `get_table`)
- Test: `ddl_snapshots.rs`, unit-тесты в `ddl.rs`

**Interfaces:**
- Consumes: `TableDetails` (задача 7).
- Produces: `catalog::ddl::table_ddl(&TableDetails) -> String`; `TableDetails.ddl: String` (заполняется в `table::get`); константа `DDL_HEADER = "-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)"`.

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/catalog/ddl.rs`, `tests`:

```rust
    fn table(name: &str) -> TableDetails {
        TableDetails {
            oid: 1, schema: "shop".into(), name: name.into(), relkind: "r".into(), owner: "postgres".into(),
            tablespace: None, comment: None, reloptions: vec![], columns: vec![], indexes: vec![], constraints: vec![],
            incoming_fks: vec![], stats: Default::default(), partitioning: Default::default(), triggers: vec![], ddl: String::new(),
        }
    }

    fn column(name: &str, data_type: &str) -> Column {
        Column { name: name.into(), data_type: data_type.into(), not_null: false, default_expr: None, identity: None, generated: None, collation: None, stat_target: None, storage: "p".into(), comment: None }
    }

    #[test]
    fn minimal_table() {
        let mut t = table("t");
        t.columns = vec![column("id", "integer"), column("note", "text")];
        assert_eq!(
            table_ddl(&t),
            format!("{DDL_HEADER}\nCREATE TABLE shop.t (\n    id integer,\n    note text\n);\n")
        );
    }

    #[test]
    fn columns_with_defaults_identity_generated_collation_and_not_null() {
        let mut t = table("t");
        let mut id = column("id", "bigint");
        id.not_null = true;
        id.identity = Some("a".into());
        let mut total = column("total", "numeric(12,2)");
        total.not_null = true;
        total.default_expr = Some("0".into());
        let mut tax = column("tax", "numeric(12,2)");
        tax.generated = Some("s".into());
        tax.default_expr = Some("(total * 1.2)".into());
        let mut name = column("name", "text");
        name.collation = Some("pg_catalog.\"C\"".into());
        t.columns = vec![id, total, tax, name];
        let ddl = table_ddl(&t);
        assert!(ddl.contains("    id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,\n"));
        assert!(ddl.contains("    total numeric(12,2) DEFAULT 0 NOT NULL,\n"));
        assert!(ddl.contains("    tax numeric(12,2) GENERATED ALWAYS AS ((total * 1.2)) STORED,\n"));
        assert!(ddl.contains("    name text COLLATE pg_catalog.\"C\"\n"));
    }

    #[test]
    fn constraints_indexes_triggers_comments_and_options() {
        let mut t = table("orders");
        t.columns = vec![column("id", "bigint"), column("customer_id", "bigint")];
        t.constraints = vec![
            Constraint { name: "orders_pkey".into(), kind: "p".into(), definition: "PRIMARY KEY (id)".into(), validated: true },
            Constraint { name: "orders_customer_id_fkey".into(), kind: "f".into(), definition: "FOREIGN KEY (customer_id) REFERENCES shop.customers(id)".into(), validated: false },
        ];
        t.indexes = vec![
            Index { name: "orders_pkey".into(), definition: "CREATE UNIQUE INDEX orders_pkey ON shop.orders USING btree (id)".into(), primary: true, unique: true, valid: true, size_bytes: 0, idx_scan: None, idx_tup_read: None, last_idx_scan_ms: None },
            Index { name: "orders_customer_idx".into(), definition: "CREATE INDEX orders_customer_idx ON shop.orders USING btree (customer_id)".into(), primary: false, unique: false, valid: true, size_bytes: 0, idx_scan: None, idx_tup_read: None, last_idx_scan_ms: None },
        ];
        t.triggers = vec![Trigger { name: "touch".into(), enabled: "O".into(), definition: "CREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch()".into() }];
        t.reloptions = vec!["fillfactor=90".into(), "autovacuum_enabled=false".into()];
        t.comment = Some("Заказы 'клиентов'".into());
        t.columns[0].comment = Some("id".into());
        let ddl = table_ddl(&t);
        assert_eq!(
            ddl,
            format!(
                "{DDL_HEADER}\nCREATE TABLE shop.orders (\n    id bigint,\n    customer_id bigint,\n    CONSTRAINT orders_pkey PRIMARY KEY (id),\n    CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES shop.customers(id) NOT VALID\n) WITH (fillfactor=90, autovacuum_enabled=false);\n\nCREATE INDEX orders_customer_idx ON shop.orders USING btree (customer_id);\n\nCREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch();\n\nCOMMENT ON TABLE shop.orders IS 'Заказы ''клиентов''';\nCOMMENT ON COLUMN shop.orders.id IS 'id';\n"
            )
        );
    }

    #[test]
    fn partitioned_parent_and_partition() {
        let mut parent = table("events");
        parent.relkind = "p".into();
        parent.columns = vec![column("id", "bigint"), column("happened_at", "timestamp with time zone")];
        parent.partitioning = Partitioning { key: Some("RANGE (happened_at)".into()), bound: None, parent: None, partitions: vec!["shop.events_2026_09".into()] };
        let ddl = table_ddl(&parent);
        assert!(ddl.contains(") PARTITION BY RANGE (happened_at);\n"));
        assert!(ddl.contains("-- partitions: shop.events_2026_09\n"));
        let mut part = table("events_2026_09");
        part.partitioning = Partitioning { key: None, bound: Some("FOR VALUES FROM ('2026-09-01') TO ('2026-10-01')".into()), parent: Some("shop.events".into()), partitions: vec![] };
        assert!(table_ddl(&part).contains("CREATE TABLE shop.events_2026_09 PARTITION OF shop.events\n    FOR VALUES FROM ('2026-09-01') TO ('2026-10-01');\n"));
    }

    #[test]
    fn identifiers_are_quoted_when_needed() {
        let mut t = table("Order Items");
        t.schema = "Shop".into();
        t.columns = vec![column("select", "text"), column("ok_name", "text")];
        let ddl = table_ddl(&t);
        assert!(ddl.contains("CREATE TABLE \"Shop\".\"Order Items\" (\n    \"select\" text,\n    ok_name text\n);"));
    }
```

`src-tauri/tests/ddl_snapshots.rs`:

```rust
//! Эталоны DDL: фикстура `TableDetails` (JSON) → `table_ddl` → сравнение с `*.sql`. Обновить эталон:
//! `UPDATE_SNAPSHOTS=1 cargo test --test ddl_snapshots`.

use glafira_pg_lib::catalog::ddl::table_ddl;
use glafira_pg_lib::catalog::table::TableDetails;

#[test]
fn ddl_matches_snapshots() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/ddl");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let details: TableDetails = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let actual = table_ddl(&details);
            let expected_path = path.with_extension("sql");
            if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
                std::fs::write(&expected_path, &actual).unwrap();
            }
            let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
            assert_eq!(actual, expected, "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 3, "фикстур меньше, чем ожидалось");
}
```

Фикстуры `orders.json`, `events.json`, `customers.json` — `TableDetails` из `it_catalog` на `pg-single` (снять один раз: `serde_json::to_string_pretty(&table::get(…))`, положить файл; `stats` обнулить, чтобы снимок не зависел от стенда), эталоны `.sql` — вывод генератора, проверенный глазами и `psql`-прогоном на пустой БД стенда (`docker exec -i gp-pg-single psql -U postgres -d postgres -c 'CREATE SCHEMA tmp_ddl'` → выполнить DDL в схеме `tmp_ddl`? Нет: DDL содержит `shop.`; выполнить в отдельной БД `CREATE DATABASE ddl_check` и удалить её после — стенд от этого не страдает).

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib catalog::ddl`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`src-tauri/src/catalog/ddl.rs`:

```rust
//! Генератор DDL по каталогу (спец. P1 §9): CREATE TABLE с колонками, ограничениями, партиционированием,
//! индексами, триггерами и комментариями. Не замена pg_dump: без прав, политик RLS и владельцев.

use super::table::{Column, Constraint, TableDetails};

pub const DDL_HEADER: &str =
    "-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)";

/// Идентификатор в кавычках, если он не простой (`[a-z_][a-z0-9_]*`) или совпадает с ключевым словом.
pub fn quote_ident(s: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "all", "and", "any", "as", "asc", "case", "check", "collate", "column", "constraint", "create", "default",
        "desc", "distinct", "do", "else", "end", "false", "for", "foreign", "from", "grant", "group", "having", "in",
        "into", "is", "join", "limit", "not", "null", "offset", "on", "or", "order", "primary", "references",
        "select", "table", "then", "to", "true", "union", "unique", "user", "using", "when", "where", "with",
    ];
    let simple = !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !KEYWORDS.contains(&s);
    if simple { s.to_string() } else { format!("\"{}\"", s.replace('"', "\"\"")) }
}

fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn column_def(c: &Column) -> String {
    let mut parts = vec![quote_ident(&c.name), c.data_type.clone()];
    if let Some(coll) = &c.collation {
        parts.push(format!("COLLATE {coll}"));
    }
    match (c.generated.as_deref(), c.identity.as_deref(), &c.default_expr) {
        (Some("s"), _, Some(expr)) => parts.push(format!("GENERATED ALWAYS AS ({expr}) STORED")),
        (_, Some("a"), _) => parts.push("GENERATED ALWAYS AS IDENTITY".into()),
        (_, Some("d"), _) => parts.push("GENERATED BY DEFAULT AS IDENTITY".into()),
        (_, _, Some(expr)) => parts.push(format!("DEFAULT {expr}")),
        _ => {}
    }
    if c.not_null && c.generated.is_none() {
        parts.push("NOT NULL".into());
    }
    parts.join(" ")
}

fn constraint_def(k: &Constraint) -> String {
    let mut s = format!("CONSTRAINT {} {}", quote_ident(&k.name), k.definition);
    if !k.validated {
        s.push_str(" NOT VALID");
    }
    s
}

pub fn table_ddl(t: &TableDetails) -> String {
    let qname = format!("{}.{}", quote_ident(&t.schema), quote_ident(&t.name));
    let mut out = format!("{DDL_HEADER}\n");
    let partition_of = t.partitioning.parent.as_ref().zip(t.partitioning.bound.as_ref());
    match partition_of {
        Some((parent, bound)) => {
            out.push_str(&format!("CREATE TABLE {qname} PARTITION OF {parent}\n"));
            let extra: Vec<String> = t.constraints.iter().filter(|k| k.kind != "f" || true).map(constraint_def).collect();
            if !extra.is_empty() {
                out.push_str(&format!("(\n    {}\n)\n", extra.join(",\n    ")));
            }
            out.push_str(&format!("    {bound}"));
        }
        None => {
            let mut items: Vec<String> = t.columns.iter().map(column_def).collect();
            items.extend(t.constraints.iter().map(constraint_def));
            out.push_str(&format!("CREATE TABLE {qname} (\n    {}\n)", items.join(",\n    ")));
            if let Some(key) = &t.partitioning.key {
                out.push_str(&format!(" PARTITION BY {key}"));
            }
        }
    }
    if !t.reloptions.is_empty() {
        out.push_str(&format!(" WITH ({})", t.reloptions.join(", ")));
    }
    if let Some(ts) = &t.tablespace {
        out.push_str(&format!(" TABLESPACE {}", quote_ident(ts)));
    }
    out.push_str(";\n");
    if !t.partitioning.partitions.is_empty() {
        out.push_str(&format!("-- partitions: {}\n", t.partitioning.partitions.join(", ")));
    }
    // Индексы ограничений (PK, UNIQUE, EXCLUDE) уже созданы ограничениями.
    let constraint_indexes: Vec<&str> = t.constraints.iter().filter(|k| matches!(k.kind.as_str(), "p" | "u" | "x")).map(|k| k.name.as_str()).collect();
    let indexes: Vec<String> = t.indexes.iter().filter(|i| !constraint_indexes.contains(&i.name.as_str())).map(|i| format!("{};", i.definition)).collect();
    if !indexes.is_empty() {
        out.push_str(&format!("\n{}\n", indexes.join("\n")));
    }
    if !t.triggers.is_empty() {
        let triggers: Vec<String> = t.triggers.iter().map(|x| format!("{};", x.definition)).collect();
        out.push_str(&format!("\n{}\n", triggers.join("\n")));
    }
    let mut comments = Vec::new();
    if let Some(c) = &t.comment {
        comments.push(format!("COMMENT ON TABLE {qname} IS {};", literal(c)));
    }
    for c in t.columns.iter().filter(|c| c.comment.is_some()) {
        comments.push(format!("COMMENT ON COLUMN {qname}.{} IS {};", quote_ident(&c.name), literal(c.comment.as_deref().unwrap_or(""))));
    }
    if !comments.is_empty() {
        out.push_str(&format!("\n{}\n", comments.join("\n")));
    }
    out
}
```

(в ветке партиции убрать заглушку `|| true`: у партиции свои ограничения выводятся все; `TableStats` и `Partitioning` получают `#[derive(Default)]` для тестов.) `table::get` заполняет `ddl: ddl::table_ddl(&details)` перед возвратом (поле `ddl: String` в `TableDetails`, в фикстурах JSON присутствует — для снимков игнорируется: `table_ddl` его не читает).

- [ ] **Step 4: Прогнать проверки и commit**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib catalog:: && cargo test --manifest-path src-tauri/Cargo.toml --test ddl_snapshots` и остальные проверки.

```bash
git add src-tauri
git commit -m "feat(catalog): генератор DDL таблицы по каталогу со снимками-эталонами

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: Вкладки объектов во фронтенде

**Files:**
- Create: `src/components/catalog/ObjectTab.tsx`, `src/components/catalog/TableTab.tsx` (+ `.test.tsx`), `src/components/catalog/ViewTab.tsx` (+ `.test.tsx`), `src/components/catalog/FunctionTab.tsx` (+ `.test.tsx`), `src/components/catalog/SequenceTab.tsx` (+ `.test.tsx`), `src/components/catalog/DdlView.tsx`, `src/components/catalog/KeyValue.tsx`
- Modify: `src/state/workspace.ts` (+ `.test.ts`), `src/components/TabContent.tsx`, `src/components/Layout.tsx` (+ `.test.tsx`), `src/components/connections/tree/ObjectList.tsx` (+ `.test.tsx`), `src/api/types.ts`, `src/api/client.ts`, `src/api/client.channel.test.ts`, `src/lib/format.ts` (`formatTimestamp`, если нет — есть), `src/locales/{en,ru}.json`, `docs/i18n/same-as-en.json`, `src/styles.css`
- Test: vitest

**Interfaces:**
- Consumes: команды задачи 7; `ObjectItem`, `ObjectKind`; `useWorkspace`; `ofConnection`/`PG_QUERY_KEYS`.
- Produces: `TabSpec` варианты `{ kind: "table"; connectionId; database; schema; name }`, `{ kind: "view"; connectionId; database; schema; name; matview: boolean }`, `{ kind: "function"; connectionId; database; oid: number; name: string }`, `{ kind: "sequence"; connectionId; database; schema; name }`; `tabId` — `table:<id>:<db>:<schema>.<name>`, `view:…`, `function:<id>:<db>:<oid>`, `sequence:…`; TS-типы `TableDetails`, `ViewDetails`, `FunctionDetails`, `SequenceDetails` (зеркало Rust); `api.getTable(id, db, schema, name)`, `getView`, `getFunction(id, db, oid)`, `getSequence`; ключи кэша `["catalog", id, "table", db, schema, name]` и т. п.; `ObjectList` открывает вкладку по клику на имя.

- [ ] **Step 1: Падающие тесты**

`src/state/workspace.test.ts` (в «id вкладок»):

```ts
    expect(tabId({ kind: "table", connectionId: "c", database: "orders", schema: "shop", name: "events" })).toBe("table:c:orders:shop.events");
    expect(tabId({ kind: "view", connectionId: "c", database: "orders", schema: "shop", name: "v", matview: true })).toBe("view:c:orders:shop.v");
    expect(tabId({ kind: "function", connectionId: "c", database: "orders", oid: 16401, name: "order_count" })).toBe("function:c:orders:16401");
    expect(tabId({ kind: "sequence", connectionId: "c", database: "orders", schema: "shop", name: "s" })).toBe("sequence:c:orders:shop.s");
```

`src/components/connections/tree/ObjectList.test.tsx`:

```tsx
  it("клик по имени объекта открывает вкладку своего вида", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 2, nextOffset: null, items: [item(1, { name: "orders" }), item(2, { name: "events", relkind: "p", partitions: 2 })] });
    const user = userEvent.setup();
    renderList(2);
    await user.click(await screen.findByRole("button", { name: "Открыть orders" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "table", connectionId: "c", database: "orders", schema: "shop", name: "orders", id: "table:c:orders:shop.orders" }]);
    // Партиционированная таблица: имя открывает вкладку, стрелка раскрывает партиции.
    await user.click(screen.getByRole("button", { name: "Открыть events" }));
    expect(useWorkspace.getState().activeId).toBe("table:c:orders:shop.events");
  });

  it("функция открывается по oid, представление — с признаком мат. представления", async () => {
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(77, { name: "order_count", signature: "c bigint" })] });
    const user = userEvent.setup();
    renderListOf("functions", 1);
    await user.click(await screen.findByRole("button", { name: "Открыть order_count(c bigint)" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "function", oid: 77, name: "order_count" });
    vi.mocked(api.listObjects).mockResolvedValue({ total: 1, nextOffset: null, items: [item(5, { name: "order_totals", relkind: "m" })] });
    renderListOf("matviews", 1);
    await user.click(await screen.findByRole("button", { name: "Открыть order_totals" }));
    expect(useWorkspace.getState().tabs.at(-1)).toMatchObject({ kind: "view", matview: true, name: "order_totals" });
  });
```

(`renderListOf(kind, count)` — как `renderList`, но с `kind`; `useWorkspace.setState({ tabs: [], activeId: null })` в `beforeEach`.)

`src/components/catalog/TableTab.test.tsx`:

```tsx
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TableDetails } from "../../api/types";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getTable: vi.fn() } };
});

import { api } from "../../api/client";
import { TableTab } from "./TableTab";

const details: TableDetails = {
  oid: 1, schema: "shop", name: "orders", relkind: "r", owner: "postgres", tablespace: null, comment: "Заказы", reloptions: ["fillfactor=90"],
  columns: [
    { name: "id", dataType: "bigint", notNull: true, defaultExpr: null, identity: "a", generated: null, collation: null, statTarget: null, storage: "p", comment: null },
    { name: "total", dataType: "numeric(12,2)", notNull: true, defaultExpr: "0", identity: null, generated: null, collation: null, statTarget: 500, storage: "m", comment: "сумма" },
  ],
  indexes: [
    { name: "orders_pkey", definition: "CREATE UNIQUE INDEX orders_pkey ON shop.orders USING btree (id)", primary: true, unique: true, valid: true, sizeBytes: 16384, idxScan: 12, idxTupRead: 30, lastIdxScanMs: null },
    { name: "orders_bad", definition: "CREATE INDEX orders_bad ON shop.orders USING btree (total)", primary: false, unique: false, valid: false, sizeBytes: 8192, idxScan: 0, idxTupRead: 0, lastIdxScanMs: 1_700_000_000_000 },
  ],
  constraints: [{ name: "orders_pkey", kind: "p", definition: "PRIMARY KEY (id)", validated: true }],
  incomingFks: [{ constraint: "items_order_fkey", schema: "shop", table: "items", definition: "FOREIGN KEY (order_id) REFERENCES shop.orders(id)" }],
  stats: { heapBytes: 8192, indexBytes: 24576, toastBytes: 0, totalBytes: 32768, reltuples: 1000, seqScan: 3, idxScan: 12, nLiveTup: 1000, nDeadTup: 5, nModSinceAnalyze: 0, nInsSinceVacuum: 0, lastVacuumMs: null, lastAutovacuumMs: 1_700_000_000_000, lastAnalyzeMs: null, lastAutoanalyzeMs: null, vacuumCount: 0, autovacuumCount: 1, analyzeCount: 0, autoanalyzeCount: 1 },
  partitioning: { key: null, bound: null, parent: null, partitions: [] },
  triggers: [{ name: "touch", enabled: "O", definition: "CREATE TRIGGER touch BEFORE INSERT ON shop.orders FOR EACH ROW EXECUTE FUNCTION shop.touch()" }],
  ddl: "-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)\nCREATE TABLE shop.orders (\n    id bigint\n);\n",
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getTable).mockResolvedValue(details);
});

describe("TableTab", () => {
  it("подвкладки: колонки по умолчанию, индексы с подсветкой INVALID, DDL с кнопкой копирования", async () => {
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    expect(await screen.findByRole("heading", { name: "shop.orders" })).toBeInTheDocument();
    expect(api.getTable).toHaveBeenCalledWith("c", "orders", "shop", "orders");
    const columns = screen.getByRole("table", { name: "Колонки" });
    expect(within(columns).getByText("total")).toBeInTheDocument();
    expect(within(columns).getByText("numeric(12,2)")).toBeInTheDocument();
    expect(within(columns).getByText("GENERATED ALWAYS AS IDENTITY")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Индексы" }));
    const bad = screen.getByRole("row", { name: /orders_bad/ });
    expect(bad).toHaveClass("invalid");
    expect(within(bad).getByText("INVALID")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Ограничения" }));
    expect(screen.getByText(/REFERENCES shop.orders\(id\)/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Размер и статистика" }));
    expect(screen.getByText("32,0 KB")).toBeInTheDocument();
    expect(screen.getByText("fillfactor=90")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Триггеры" }));
    expect(screen.getByText(/CREATE TRIGGER touch/)).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "DDL" }));
    expect(screen.getByText(/CREATE TABLE shop.orders/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать DDL" })).toBeInTheDocument();
  });

  it("партиционирование показывается только у партиций и родителей", async () => {
    vi.mocked(api.getTable).mockResolvedValue({ ...details, relkind: "p", partitioning: { key: "RANGE (happened_at)", bound: null, parent: null, partitions: ["shop.events_2026_09"] } });
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="events" />);
    await user.click(await screen.findByRole("tab", { name: "Партиционирование" }));
    expect(screen.getByText("RANGE (happened_at)")).toBeInTheDocument();
    expect(screen.getByText("shop.events_2026_09")).toBeInTheDocument();
  });

  it("ошибка загрузки показывается с повтором", async () => {
    vi.mocked(api.getTable).mockRejectedValueOnce({ code: "NOT_FOUND", key: "errors.catalog.objectNotFound", message: "gone", retryable: false }).mockResolvedValueOnce(details);
    const user = userEvent.setup();
    renderWithClient(<TableTab connectionId="c" database="orders" schema="shop" name="orders" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("gone");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByRole("heading", { name: "shop.orders" })).toBeInTheDocument();
  });
});
```

Аналогичные короткие тесты для `ViewTab` (определение, `ispopulated`, индексы у мат. представления), `FunctionTab` (сигнатура, язык, volatility «stable», SECURITY DEFINER, определение; агрегат — «только сигнатура»), `SequenceTab` (тип, last_value, min/max/шаг, процент использования «49,5 %», владеющая колонка; `lastValue: null` → «—»). `src/components/Layout.test.tsx`: заголовок вкладки `table` — «shop.orders», `function` — «order_count».

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm exec vitest run src/components/catalog src/components/connections/tree/ObjectList.test.tsx src/state`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`src/api/types.ts` — зеркала Rust (camelCase, `Option` → `| null`): `Column`, `Index`, `Constraint`, `IncomingFk`, `TableStats`, `Partitioning`, `Trigger`, `TableDetails`, `ViewDetails`, `FunctionDetails`, `SequenceDetails` (поля — как в Interfaces задачи 7; `ddl: string`). `src/api/client.ts`:

```ts
  getTable: (id: string, db: string, schema: string, name: string) => call<TableDetails>("get_table", { id, db, schema, name }),
  getView: (id: string, db: string, schema: string, name: string) => call<ViewDetails>("get_view", { id, db, schema, name }),
  getFunction: (id: string, db: string, oid: number) => call<FunctionDetails>("get_function", { id, db, oid }),
  getSequence: (id: string, db: string, schema: string, name: string) => call<SequenceDetails>("get_sequence", { id, db, schema, name }),
```

(+ строки в таблице `client.channel.test.ts`). `workspace.ts`: варианты `TabSpec` и `tabId` из Interfaces. `Layout.tsx::tabTitle`: `table`/`view`/`sequence` → `${schema}.${name}`, `function` → `name`. `TabContent.tsx`:

```tsx
    case "table":
      return <TableTab connectionId={tab.connectionId} database={tab.database} schema={tab.schema} name={tab.name} />;
    case "view":
      return <ViewTab connectionId={tab.connectionId} database={tab.database} schema={tab.schema} name={tab.name} matview={tab.matview} />;
    case "function":
      return <FunctionTab connectionId={tab.connectionId} database={tab.database} oid={tab.oid} name={tab.name} />;
    case "sequence":
      return <SequenceTab connectionId={tab.connectionId} database={tab.database} schema={tab.schema} name={tab.name} />;
```

`ObjectTab.tsx` — общая обёртка: заголовок `<h2>{title}</h2>` + `<span className="muted">{kindLabel}</span>`, загрузка (`common.loading`), `ErrorBanner` с `onRetry`, тело через render-prop:

```tsx
export function ObjectTab<T>({ title, kind, query, children }: { title: string; kind: string; query: UseQueryResult<T>; children: (data: T) => ReactNode }) {
  const { t } = useTranslation();
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{title}</h2>
        <span className="muted">{kind}</span>
      </div>
      {query.isPending && <p className="empty">{t("common.loading")}</p>}
      {query.error && <ErrorBanner error={query.error} onRetry={() => void query.refetch()} />}
      {query.data !== undefined && children(query.data)}
    </div>
  );
}
```

`KeyValue.tsx` — `<dl className="kv">` из пар; `DdlView.tsx` — `<pre>` + кнопка «Копировать DDL» (переиспользовать `CopyButton` из `TestPanel.tsx`, вынеся его в `src/components/common/CopyButton.tsx`). `TableTab.tsx`: `useQuery({ queryKey: ["catalog", connectionId, "table", database, schema, name], queryFn: () => api.getTable(connectionId, database, schema, name) })`; подвкладки — `role="tablist"`/`role="tab"` с `aria-selected`, состояние `useState<"columns"|"indexes"|"constraints"|"stats"|"partitioning"|"triggers"|"ddl">("columns")`; подвкладка «Партиционирование» показывается, если `relkind === "p"` или `partitioning.parent !== null`; таблицы — `<table aria-label={t("object.columns")}>` с `scope="col"`; строка невалидного индекса — `className="invalid"` и бейдж `INVALID`; размеры — `formatBytes`, даты — `formatTimestamp(ms)`, счётчики — `formatCount`; `—` для `null`. `ViewTab`, `FunctionTab`, `SequenceTab` — по спеке §9 (функция: `definition === null` → `object.signatureOnly`; последовательность: `percentUsed` → `formatPercent`, добавить в `format.ts` как `${n.toLocaleString(lang, { maximumFractionDigits: 1 })} %`). `ObjectList.tsx`: имя объекта — `<button type="button" className="link tree-name" aria-label={t("sidebar.openObject", { name: label })} onClick={open…}>`, где `open` строит `TabSpec` по `kind` (`tables`→`table`, `views`→`view` с `matview: false`, `matviews`→`view` с `matview: true`, `functions`→`function` с `oid`, `sequences`→`sequence`); у партиционированной таблицы стрелка-раскрытие остаётся отдельной кнопкой. Словари: `object.{kinds: {table, view, matview, function, sequence, partition}, columns, indexes, constraints, incomingFks, stats, partitioning, triggers, ddl, copyDdl, name, type, notNull, default, identity, generated, collation, statTarget, storage, comment, definition, primary, unique, invalid, size, idxScan, idxTupRead, lastIdxScan, heap, indexesSize, toast, total, reltuples, seqScan, liveTup, deadTup, modSinceAnalyze, insSinceVacuum, lastVacuum, lastAutovacuum, lastAnalyze, lastAutoanalyze, vacuumCount, autovacuumCount, analyzeCount, autoanalyzeCount, reloptions, tablespace, owner, partitionKey, partitionBound, parent, partitions, enabled, populated, signature, returns, language, volatility: {i, s, v}, securityDefiner, signatureOnly, dataType, lastValue, min, max, increment, cycle, cache, percentUsed, ownedBy}`, `sidebar.openObject` «Открыть {{name}}», `tabs.object` не нужен (заголовок — имя). `styles.css`: `.subtabs` (горизонтальный список вкладок), `tr.invalid { color: var(--danger-text) }`, `.kv { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px }`, `.ddl pre { max-height: 60vh; overflow: auto }`.

- [ ] **Step 4: Прогнать проверки и commit**

Run: `pnpm test && pnpm exec tsc --noEmit && pnpm i18n:check && pnpm test:scripts && pnpm build`.

```bash
git add src docs/i18n
git commit -m "feat(ui): вкладки таблицы, представления, функции и последовательности с DDL; открытие из дерева

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Матрица версий PG 13–18 на стенде и в тестах

**Files:**
- Create: `dev/sql/many-tables.sql`, `scripts/wait-matrix.sh`
- Modify: `dev/docker-compose.yml`, `src-tauri/tests/it_catalog.rs`, `src-tauri/tests/common/mod.rs`, `.github/workflows/ci.yml`, `README.md`
- Test: `it_catalog.rs` с `GPG_IT_MATRIX`

**Interfaces:**
- Consumes: `catalog::{databases, schemas, objects, extensions, roles, table, view, function, sequence}`.
- Produces: сервисы `pg13`…`pg18` (профиль `matrix`, порты `127.0.0.1:5513`…`5518`), `common::matrix_addrs() -> Vec<String>` (`GPG_IT_MATRIX=localhost:5513,…`), тест `catalog_queries_on_every_supported_version`.

- [ ] **Step 1: Стенд**

`dev/docker-compose.yml` — якорь и сервисы:

```yaml
x-matrix: &matrix
  profiles: ["matrix"]
  environment:
    POSTGRES_PASSWORD: postgres-secret
  command: ["postgres", "-c", "shared_preload_libraries=pg_stat_statements", "-c", "track_io_timing=on"]
  volumes:
    - ./sql/init.sql:/docker-entrypoint-initdb.d/20-init.sql:ro

services:
  # …существующие…
  pg13:
    <<: *matrix
    image: postgres:13
    container_name: gp-pg13
    ports: ["127.0.0.1:5513:5432"]
  pg14:
    <<: *matrix
    image: postgres:14
    container_name: gp-pg14
    ports: ["127.0.0.1:5514:5432"]
  pg15:
    <<: *matrix
    image: postgres:15
    container_name: gp-pg15
    ports: ["127.0.0.1:5515:5432"]
  pg16:
    <<: *matrix
    image: postgres:16
    container_name: gp-pg16
    ports: ["127.0.0.1:5516:5432"]
  pg17:
    <<: *matrix
    image: postgres:17
    container_name: gp-pg17
    ports: ["127.0.0.1:5517:5432"]
  pg18:
    <<: *matrix
    image: postgres:18
    container_name: gp-pg18
    ports: ["127.0.0.1:5518:5432"]
```

`scripts/wait-matrix.sh`: `wait_for docker exec gp-pg$v pg_isready -h localhost -q` для 13..18 и `psql -U postgres -d orders -tAc "select 1"` (те же функции, что в `wait-stand.sh`; общий код вынести в `scripts/lib-wait.sh` и подключать `source`). `dev/sql/many-tables.sql`:

```sql
-- Схема wide с 10 000 таблиц для проверки прокрутки и пагинации дерева (запускать вручную):
-- docker exec -i gp-pg-single psql -U postgres -d orders < dev/sql/many-tables.sql
CREATE SCHEMA IF NOT EXISTS wide;
DO $$
BEGIN
  FOR i IN 1..10000 LOOP
    EXECUTE format('CREATE TABLE IF NOT EXISTS wide.t_%s (id int PRIMARY KEY, v text)', lpad(i::text, 5, '0'));
  END LOOP;
END $$;
```

`README.md`: профиль `matrix` (`docker compose -f dev/docker-compose.yml --profile matrix up -d`), порты, `many-tables.sql`.

- [ ] **Step 2: Тест матрицы**

`src-tauri/tests/common/mod.rs`: `pub fn matrix_addrs() -> Vec<String> { env("GPG_IT_MATRIX").map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default() }`. `src-tauri/tests/it_catalog.rs`:

```rust
/// Каждый запрос каталога и вкладок (§8–9) на демо-схеме для PG 13–18 (`GPG_IT_MATRIX=localhost:5513,…,localhost:5518`).
#[tokio::test]
async fn catalog_queries_on_every_supported_version() {
    for addr in common::matrix_addrs() {
        let pg = client(&addr, "postgres").await;
        let version = glafira_pg_lib::catalog::server_version_num(&pg).await.unwrap();
        assert!((130000..190000).contains(&version), "{addr}: {version}");
        let dbs = databases::list(&pg, None).await.unwrap();
        assert!(dbs.iter().any(|d| d.name == "orders"), "{addr}");
        let c = client(&addr, "orders").await;
        let shop = schemas::list(&c, false).await.unwrap().into_iter().find(|s| s.name == "shop").unwrap_or_else(|| panic!("{addr}: нет shop"));
        assert_eq!((shop.counts.tables, shop.counts.views, shop.counts.matviews, shop.counts.sequences, shop.counts.functions), (3, 1, 1, 3, 3), "{addr}");
        for kind in [ObjectKind::Tables, ObjectKind::Views, ObjectKind::Matviews, ObjectKind::Functions, ObjectKind::Sequences] {
            objects::list(&c, "shop", kind, None, 0, 500).await.unwrap_or_else(|e| panic!("{addr} {kind:?}: {e:?}"));
        }
        objects::partitions(&c, "shop", "events", 0, 500).await.unwrap();
        extensions::list(&c).await.unwrap();
        roles::list(&c, false).await.unwrap();
        let t = table::get(&c, "shop", "orders").await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert_eq!(t.columns.len(), 5, "{addr}");
        assert!(t.indexes.iter().all(|i| version >= 160000 || i.last_idx_scan_ms.is_none()), "{addr}");
        table::get(&c, "shop", "events").await.unwrap();
        table::get(&c, "shop", "events_2026_09").await.unwrap();
        view::get(&c, "shop", "active_orders").await.unwrap();
        view::get(&c, "shop", "order_totals").await.unwrap();
        let funcs = objects::list(&c, "shop", ObjectKind::Functions, None, 0, 500).await.unwrap();
        for f in &funcs.items {
            function::get(&c, f.oid).await.unwrap_or_else(|e| panic!("{addr} {}: {e:?}", f.name));
        }
        sequence::get(&c, "shop", "invoice_seq").await.unwrap();
        sequence::get(&c, "shop", "orders_id_seq").await.unwrap();
    }
}
```

`.github/workflows/ci.yml`: «Стенд» → `docker compose -f dev/docker-compose.yml --profile matrix up -d --build` и `scripts/wait-stand.sh && scripts/wait-matrix.sh`; env `GPG_IT_MATRIX: localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518`.

- [ ] **Step 3: Поднять матрицу и прогнать**

Run: `docker compose -f dev/docker-compose.yml --profile matrix up -d && scripts/wait-matrix.sh`, затем интеграционные тесты с `GPG_IT_MATRIX=…` (и остальными переменными).
Expected: `catalog_queries_on_every_supported_version` проходит на шести версиях; в отчёте — какие версии образов скачались (`docker compose images`).

- [ ] **Step 4: Commit**

```bash
git add dev scripts src-tauri/tests .github/workflows/ci.yml README.md
git commit -m "feat(stand): матрица версий PostgreSQL 13–18, схема wide и прогон каталога на каждой версии

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 11: Руководство, admin.md, README, спецификация

**Files:**
- Create: `docs/guide/en/connections.md`, `docs/guide/ru/connections.md`, `docs/guide/en/objects.md`, `docs/guide/ru/objects.md`
- Modify: `src/guide/chapters.ts`, `scripts/i18n-check.mjs` (`GUIDE_CHAPTERS`), `scripts/guide-publish.mjs` (если список глав там свой), `src/components/help/helpChapterFor.ts` (+ `.test.ts`), `docs/admin.md`, `README.md`, `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md` (§5, §11, §15.1), `CHANGELOG.md` (раздел «Unreleased», если проект его ведёт — см. `docs/release-checklist.md`)
- Test: `pnpm test:scripts` (проверка глав: якоря `{#id}`, оба языка), `helpChapterFor.test.ts`

**Interfaces:**
- Produces: `GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "updates"]`; `helpChapterFor`: `connection-form` → `connections`, `table | view | function | sequence` → `objects`, `about` → `updates`, остальное → `getting-started`.

- [ ] **Step 1: Главы**

`docs/guide/ru/connections.md` — разделы с якорями: `# Подключения`; `## Виды подключений {#kinds}` (сервер, список узлов с целевым узлом, кластер Patroni); `## Строка подключения {#connection-string}` (JDBC, URI, conninfo; пароль — в хранилище секретов; нераспознанные параметры); `## SSH-туннель {#ssh}` (цель или алиас из `~/.ssh/config`, способы входа: агент, ключ (пассфраза), пароль; требуется OpenSSH 8.4+ для пароля и пассфразы; первый вход — подтверждение ключа хоста по отпечатку; что делать при «ключ хоста изменился»; бастион должен видеть узлы по именам из профиля); `## Patroni {#patroni}` (адреса API, basic auth, TLS API; подмена адресов узлов для NAT/Kubernetes); `## Чек-лист готовности {#readiness}` (что проверяется, скрипт роли, `pg_stat_statements`). `docs/guide/en/connections.md` — тот же текст по-английски с теми же якорями. `docs/guide/ru/objects.md`: `# Дерево и объекты`; `## Дерево {#tree}` (узлы, базы с узла каталога, схемы, счётчики, фильтр, системные объекты, партиции); `## Вкладки объектов {#tabs}` (таблица: подвкладки; представление; функция; последовательность); `## DDL {#ddl}` (как построен, чего в нём нет, кнопка копирования). `src/guide/chapters.ts` и `scripts/i18n-check.mjs`: `GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "updates"]` (`guide-publish.mjs` берёт список оттуда же или из своего — синхронизировать). `helpChapterFor`:

```ts
export function helpChapterFor(tab: TabSpec | null): ChapterId {
  switch (tab?.kind) {
    case "about":
      return "updates";
    case "connection-form":
      return "connections";
    case "table":
    case "view":
    case "function":
    case "sequence":
      return "objects";
    default:
      return "getting-started";
  }
}
```

с тестом на каждую ветку.

- [ ] **Step 2: admin.md, README, спецификация**

`docs/admin.md`: раздел «Сеть» — какие порты нужны напрямую (PG узлов, REST API Patroni) и через бастион (только SSH до бастиона; бастион должен разрешать имена узлов и достигать их портов; `AllowTcpForwarding yes` в sshd); раздел «OpenSSH» — клиент 8.4+ для пароля/пассфразы, где лежит лог туннеля (`ssh-<id>.log` в каталоге логов), `tunnels.json`; ключ хоста записывается в `~/.ssh/known_hosts` самим ssh после подтверждения. `README.md`: стенд — `bastion` и профиль `matrix`, переменные `GPG_IT_BASTION`, `GPG_IT_SSH_KEYS`, `GPG_IT_MATRIX`; требования — OpenSSH. Спецификация P1: §5 — переписать под `-D` (отступление 1: один процесс, SOCKS5, имена разрешает бастион, готовность по порту SOCKS, новые участники без новых процессов, перезапуск с паузами), §5.1 — `GLAFIRA_ASKPASS_SECRET_FILE` только для тестов (при заданной переменной keychain не используется), отступление 10: пассфраза только на запрос `Enter passphrase for`, в режиме «ключ» `PreferredAuthentications=publickey`, §5.2 — `params.kind` вместо `details.kind`, §11 — `trust_ssh_host_key(target, fingerprint)`, `get_table/get_view/get_function/get_sequence` уже есть; §15.1 — ключи `bastion` генерируются `dev/ssh/gen-keys.sh`; §15.2 — `fake_ssh` в unit-тестах; §16 — главы руководства.

- [ ] **Step 3: Проверки и commit**

Run: `pnpm test:scripts && pnpm i18n:check && pnpm test && pnpm build` (главы попадают в сборку), `pnpm guide:publish` (если скрипт есть — собирает `dist/guide`).

```bash
git add docs src/guide src/components/help scripts README.md CHANGELOG.md
git commit -m "docs: главы «Подключения» и «Дерево и объекты», сеть и OpenSSH в admin.md, спецификация под туннель -D

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Самопроверка плана

- **Покрытие спеки P1b** (§1, пункты 3 и 8 без обзора, стенд, руководство): §3.1 `ssh`/§3.2 `sshPassword`, `sshPassphrase` — задача 1; §5 процесс, готовность, перезапуск, сироты — задачи 2 и 4; §5.1 askpass — задача 3; §5.2 ошибки — задачи 2, 5; §7 TLS по исходному имени через туннель — задача 4 (тест `verify_full_via_socks_checks_the_original_host_name`); §9 вкладки таблицы (все подвкладки), представления, функции, последовательности, DDL с первой строкой-пометкой и кнопкой «Копировать» — задачи 7–9; §10 шаг `ssh` — задача 4; §11 команды — задачи 4, 7; §12 раздел SSH формы — задача 5; §13 `SSH_ERROR`, `FEATURE_UNAVAILABLE` — задача 1; §15.1 `bastion`, `matrix`, `many-tables.sql` — задачи 6, 10; §15.2 askpass/stderr/DDL-снимки/SSH-интеграционные/матрица — задачи 2, 3, 6, 8, 10; §15.3 CI — задачи 6, 10; §16 руководство, admin.md, README — задача 11; §17 критерии 1 и 6 — задачи 6 и 10.
- **Типы между задачами:** `SshSettings`/`SshAuth` (1) → `SshArgs` (2), `TunnelRegistry::open` (4), `SshSection` (5); `pooled_client(…, socks)` и `ConnectSpec::build_via` (4) → `it_ssh` (6); `TableDetails` (7) → `table_ddl` (8) → TS `TableDetails` и `TableTab` (9); `GUIDE_CHAPTERS` (11) согласован между `chapters.ts` и `i18n-check.mjs`.
- **Порядок:** 1 → 2 → 3 → 4 → 5 → 6 (SSH), 7 → 8 → 9 (объекты; 7 не зависит от SSH и может идти после 1), 10 (стенд), 11 (документация, после всего).
- **Риски, названные заранее:** Windows OpenSSH и askpass проверяются только сборкой (CI `platforms`) и `fake_ssh`; keychain второго процесса на macOS в неподписанной сборке может спросить разрешение (отмечено в admin.md); `postgres:18` — образ должен быть доступен в реестре (иначе матрица 13–17 с пометкой в отчёте задачи 10).
