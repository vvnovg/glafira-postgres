# Glafira for PostgreSQL — этап P1 «Каркас и подключения»

**Дата:** 2026-10-19
**Статус:** на ревью
**Концепция:** `2026-10-19-glafira-postgres-concept-design.md` (решения, каталог инструментов, этапы P1–P6)
**Образец кода:** Glafira for Apache Kafka, `/Users/vvnovg/kafka-explorer` (далее — «Glafira Kafka»)

P1 делает приложение, которым уже можно пользоваться как навигатором: подключиться к серверу, списку узлов или кластеру
Patroni (напрямую или через SSH), увидеть топологию с ролями и лагом, пройти дерево до таблицы и посмотреть её
устройство. Диагностики (§5 концепции), действий и MCP в P1 нет — для них P1 закладывает подключения, пулы, топологию
и каркас приложения.

---

## 1. Объём

**Входит:**
1. Репозиторий, каркас Tauri 2 + React 19 из Glafira Kafka без Kafka; хранилище, keychain, политика, лицензия, i18n
   (ru + en), логи, «О программе», справка (F1).
2. Профиль подключения трёх видов: сервер, список узлов, кластер Patroni; импорт строки подключения (JDBC, URI,
   conninfo); `~/.pgpass`.
3. SSH-туннель через системный OpenSSH: ключ, агент, пароль и пассфраза через askpass, подтверждение ключа хоста.
4. Топология: клиент Patroni REST, опрос узлов списка, опрос раз в 10 с, журнал событий топологии.
5. Пулы соединений по (узел, БД), выбор узла для чтения каталога.
6. Проверка подключения по шагам и чек-лист готовности к диагностике (крейт `pg-advisor`).
7. Дерево: папки → подключения → узлы / базы → схемы → объекты; роли; расширения.
8. Вкладки: обзор кластера (или сервера), таблица, представление / мат. представление, функция, последовательность.
9. Папки, перетаскивание, экспорт и импорт без секретов — как в Glafira Kafka.
10. Флаги read-only и окружение с индикацией; создание prod-подключений — Pro.
11. Docker-стенд, интеграционные тесты, матрица версий PG 13–18, CI.

**План реализации делится на две части** (как M2a / M2b в Glafira Kafka), каждая заканчивается работающим
приложением:
- **P1a:** пункты 1, 2, 4, 5, 6, 7, 9, 10 при прямом подключении; обзор кластера; стенд без `bastion` и без `matrix`; CI.
- **P1b:** пункт 3 (SSH), вкладки объектов и генератор DDL (пункт 8, кроме обзора), `bastion` и матрица версий
  на стенде, руководство.

**Не входит (этап):** диагностика и пульс (P2, P5), сэмплер (P2), действия, guard и подтверждения (P2), тяжёлые запросы
и EXPLAIN (P3), MCP и `llmAccess` (P4), Patroni `/config` и `/history` (P2, P5), оценка bloat во вкладке таблицы (P5),
остальные 10 языков, подписание, установщики, автообновление (P6).

---

## 2. Что берём из Glafira Kafka

| Модуль Glafira Kafka | В P1 | Изменения |
|---|---|---|
| `src-tauri/src/storage/{fs, tree, exchange}.rs` | Да | `exchange` — под новый профиль; формат файла экспорта со своим `format` (`glafira-postgres-connections`) |
| `src-tauri/src/storage/connections.rs` | Логика хранения | Новая модель профиля (§3) |
| `secrets.rs` | Да | Новые поля секретов (§3.2) |
| `policy/`, `license/`, `i18n.rs`, `logging.rs`, `error.rs`, `sync.rs`, `settings.rs`, `product.rs` | Да | Ключи политики `licenseKey`, `language`; новые коды ошибок (§11); идентификаторы продукта (§2.1) |
| `commands/{folders, app, license, settings}.rs` | Да | Без `check_update` / `install_update` (P6) |
| `migrate.rs`, `kafka/`, `browse/`, `consume/`, `produce/`, `tls/jks.rs`, `mcp/`, `guard.rs` | Нет | JKS в PostgreSQL не нужен; guard — P2; MCP — P4 |
| `src/components/{Layout, Sidebar, TabContent}` | Да | Новые виды вкладок |
| `src/components/connections/tree/*` | Да | Узлы дерева под PG (§8) |
| `src/components/common/{ErrorBanner, Hint, EnvBadge, ProLock}` | Да | `ConfirmDialog`, `ConfigTable`, `ConfigEditor` — с P2 |
| `src/components/{settings, license, about, help}` | Да | Без MCP-раздела и уведомления об обновлении |
| `src/lib/*`, `src/state/workspace.ts`, `src/api/client.ts`, `src/i18n/*`, `src/guide/*`, `src/styles.css` | Да | Новые `TabSpec`; языки — `en`, `ru` |
| `scripts/i18n-check.mjs`, `guide-publish.mjs` | Да | Список обязательных языков — `en`, `ru` |
| `crates/license-format` | git-зависимость | Без изменений |

Скопированные модули сохраняют имена, файлы и публичные функции, чтобы позже вынести их в общий монорепозиторий
механически (концепция §2). Где модуль меняется, изменение делается так, чтобы его можно было вернуть в общий код
(параметр, а не правка внутри).

### 2.1 Идентификаторы продукта (`product.rs`, `tauri.conf.json`)

| Константа | Значение |
|---|---|
| `PRODUCT_NAME` | `Glafira for PostgreSQL` |
| `IDENTIFIER` | `io.github.vvnovg.glafira.postgres` |
| `mainBinaryName` | `glafira-pg` |
| `KEYCHAIN_SERVICE` | `glafira-postgres` |
| `POLICY_VENDOR` / `POLICY_PRODUCT` | `Glafira` / `PostgreSQL` |
| `PG_APPLICATION_NAME` | `glafira-pg` (виден в `pg_stat_activity`) |

Политика: macOS — домен `io.github.vvnovg.glafira.postgres`; Windows — `HKLM\SOFTWARE\Policies\Glafira\PostgreSQL`;
Linux — `/etc/io.github.vvnovg.glafira.postgres/policy.json`.

**Лицензия:** формат `license-format` без изменений, но своя пара Ed25519: `src-tauri/license-public.key` — открытый
ключ продукта PostgreSQL. Ключ Glafira Kafka в этом приложении подпись не проходит, и наоборот. Выдача — существующая
утилита `issue-license` из kafka-explorer (`keygen --out`, `issue --key <ключ PostgreSQL>`).

**Зависимость:** `license-format = { git = "https://github.com/vvnovg/kafka-explorer", rev = … }`, где `rev` —
коммит `main` kafka-explorer на момент начала P1 (закрепляется в `Cargo.toml` и обновляется осознанно);
`.cargo/config.toml` — `net.git-fetch-with-cli = true`, чтобы работали системные учётные данные git; в CI — токен
с доступом на чтение к kafka-explorer.

---

## 3. Профиль подключения

### 3.1 Модель

`connections.json` (`schemaVersion: 1`), запись профиля:

```json
{
  "id": "6f1c…",
  "name": "billing-prod",
  "color": "#e11d48",
  "environment": "prod",
  "readOnly": true,
  "kind": "patroni",
  "hosts": [],
  "targetServerType": "primary",
  "patroni": {
    "urls": ["https://pg-1:8008", "https://pg-2:8008"],
    "username": "patroni",
    "tls": { "caPath": "/certs/ca.crt", "verifyHostname": true, "clientCert": null },
    "hostOverrides": [{ "member": "pg-1", "host": "10.1.2.3", "port": 5432, "apiUrl": "https://10.1.2.3:8008" }]
  },
  "pg": {
    "username": "glafira_diag",
    "database": "postgres",
    "sslMode": "verify-full",
    "tls": { "caPath": "/certs/ca.crt", "clientCert": { "format": "pem", "certificatePath": "…", "keyPath": "…" } },
    "connectTimeoutS": 10,
    "databases": null
  },
  "ssh": { "target": "bastion-prod", "port": null, "user": null, "auth": "agent", "keyPath": null },
  "folderId": null,
  "order": 0
}
```

| Поле | Правила |
|---|---|
| `kind` | `server` \| `hostList` \| `patroni` |
| `hosts` | `server` — ровно 1; `hostList` — от 1 до 16; `patroni` — пусто (узлы приходят из `/cluster`). Элемент — `{ host, port }`, порт по умолчанию 5432, IPv6 без скобок |
| `targetServerType` | Для `hostList`: `primary` (по умолчанию) \| `standby` \| `preferStandby` \| `any` — узел по умолчанию для каталога. Для `patroni` и `server` не используется |
| `patroni` | Только для `patroni`. `urls` — от 1 до 8 адресов API (`http://` или `https://`). `username` — basic auth, `null` — без авторизации. `hostOverrides` — подмена адресов участника по имени: PG (`host`, `port`) и/или API (`apiUrl`) |
| `pg.sslMode` | `disable` \| `prefer` (по умолчанию) \| `require` \| `verify-ca` \| `verify-full` |
| `pg.tls` | CA — PEM-файл, пусто — системные корневые. Клиентский сертификат — `pem` (сертификат + ключ) или `pkcs12` |
| `pg.databases` | `null` — все БД с `datallowconn`; список — только эти (в порядке списка) |
| `ssh` | `null` — напрямую. `target` — хост или алиас из `~/.ssh/config`; `port`/`user` — `null` = как в конфиге ssh. `auth`: `agent` \| `key` \| `password`; `keyPath` — для `key` |
| `folderId`, `order` | Локальные, как в Glafira Kafka: экспорт их не пишет, импорт не переносит |

Окружения, цвет, `readOnly` — как в Glafira Kafka. Новый профиль с `environment = prod` создаётся с `readOnly = true`;
создание prod-профиля и перевод профиля в prod — Pro (`ProLock`, проверка в Rust при `save_connection`).

Превращение `hostList` в `patroni` — в форме: «Узлы под управлением Patroni» → переключает `kind`, адреса API
предзаполняются из `hosts` с портом 8008, `hosts` очищается.

### 3.2 Секреты

Keychain, сервис `glafira-postgres`, ключ `<id>/<поле>`:

| Поле | Когда |
|---|---|
| `pgPassword` | Пароль PG. Не задан → ищется в `~/.pgpass` (или файле из `PGPASSFILE`) по правилам libpq: `host:port:database:user`, хост — исходный, не адрес туннеля |
| `pgKeyPassword` | Пароль PKCS#12 или зашифрованного PEM-ключа |
| `patroniPassword` | Пароль basic auth Patroni |
| `sshPassword` | `ssh.auth = password` |
| `sshPassphrase` | `ssh.auth = key` с зашифрованным ключом |

`save_connection(profile, secrets)`: у каждого поля `secrets` — `null` (не менять), `""` (удалить), строка (записать).
Удаление профиля удаляет все его секреты.

### 3.3 Экспорт и импорт

Как в Glafira Kafka (`exchange.rs`): дерево или папка — в файл без секретов и без локальных полей; предпросмотр перед
применением; новые профили добавляются, существующие (по `id`) обновляются по желанию. Пути к сертификатам и ключам
экспортируются как есть (на другой машине их правят в форме). Файл экспорта Glafira Kafka при импорте отклоняется
понятной ошибкой: другой `format`.

---

## 4. Импорт строки подключения

Поле «Вставить строку подключения» в форме; команда `parse_connection_string(text)` возвращает
`{ patch, password, ignored: [String] }`. `patch` заполняет форму (вид, узлы, БД, пользователь, TLS, целевой узел),
`password` — поле пароля (уйдёт в keychain при сохранении), `ignored` — список нераспознанных параметров
(показывается под полем, не ошибка).

| Формат | Пример |
|---|---|
| JDBC | `jdbc:postgresql://h1:5432,h2:5433/orders?user=app&targetServerType=primary&ssl=true` |
| URI | `postgresql://app:secret@h1,h2:5433/orders?target_session_attrs=read-write&sslmode=verify-full` (и `postgres://`) |
| conninfo | `host=h1,h2 port=5432,5433 dbname=orders user=app sslmode=require` |

**Правила:**
- Один хост → `server`, несколько → `hostList`. `patroni` строка не задаёт.
- Порты: у каждого хоста свой, иначе общий (`port=5432`), иначе 5432. В conninfo и URI `port` со списком сопоставляется
  с хостами по позиции; число портов ≠ 1 и ≠ числу хостов → ошибка `INVALID_INPUT`.
- URI и conninfo: проценты декодируются; в conninfo — значения в `'…'` с экранированием `\'` и `\\`; `hostaddr`
  используется, если нет `host`. IPv6 — `[::1]:5432`.
- Unix-сокеты (`host=/var/run/postgresql`, `%2Fvar%2Frun`) → `INVALID_INPUT` «Unix sockets are not supported».
- Соответствие параметров:

| Параметр | Поле |
|---|---|
| `user`, `dbname` / путь, `password` | `pg.username`, `pg.database`, пароль |
| `sslmode` | `pg.sslMode` |
| JDBC `ssl=true` без `sslmode` | `verify-full` (поведение pgjdbc) |
| `sslrootcert`, `sslcert`, `sslkey` | `pg.tls` (PEM); `sslcert` с `.p12`/`.pfx` → `pkcs12` |
| `connect_timeout`, JDBC `connectTimeout` | `pg.connectTimeoutS` |
| `target_session_attrs`: `read-write`, `primary`; JDBC `targetServerType`: `primary`, `master` | `primary` |
| `standby`, `read-only`; JDBC `secondary`, `standby` | `standby` |
| `prefer-standby`; JDBC `preferSecondary`, `preferStandby` | `preferStandby` |
| `any` | `any` |

Остальное (`ApplicationName`, `currentSchema`, `loadBalanceHosts`, `options` и т. п.) → `ignored`.

---

## 5. SSH-туннель

Модуль `net/ssh`. Используется системный `ssh` (OpenSSH): учитываются `~/.ssh/config`, ProxyJump, агент, аппаратные
ключи. Нет `ssh` в `PATH` → `SSH_ERROR` `notFound` с подсказкой (Windows: компонент «Клиент OpenSSH»).

**Процесс.** На подключение — один или несколько процессов `ssh`, каждый с набором пробросов:

```
ssh -N -T -E <каталог логов>/ssh-<id>.log
    -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -o ServerAliveCountMax=3
    -o StrictHostKeyChecking=ask -o NumberOfPasswordPrompts=1
    [-p <port>] [-l <user>] [-i <keyPath> -o IdentitiesOnly=yes]
    -L 127.0.0.1:<локальный порт>:<хост>:<порт> …  <target>
```

- **Цели проброса:** `server` и `hostList` — каждый хост; `patroni` — сначала адреса API из `urls`, после обнаружения —
  PG-адрес и API каждого участника (с учётом `hostOverrides`). Адреса разрешаются на стороне бастиона.
- **Локальные порты:** приложение открывает слушатель на `127.0.0.1:0`, запоминает порт и закрывает слушатель;
  при гонке (`ExitOnForwardFailure` сработал на занятом порту) — одна повторная попытка с новыми портами.
- **Готовность:** туннель готов, когда каждый локальный порт принимает TCP-соединение; если процесс завершился
  раньше — ошибка по его stderr (§5.2). Ожидание — до `pg.connectTimeoutS` + 5 с.
- **Новые цели** (новый участник Patroni, смена `apiUrl`) — дополнительный процесс `ssh` только с новыми пробросами;
  работающие соединения не рвутся.
- **Жизненный цикл:** процессы живут, пока подключение открыто; `disconnect` и выход из приложения завершают их.
  Процесс упал (обрыв связи, ServerAlive) → состояние подключения `degraded`, автоматический перезапуск с паузами
  1, 2, 5, 10, 30 с; узлы через упавший процесс помечаются недоступными.
- **Сироты после сбоя приложения:** PID процессов пишутся в `tunnels.json` в каталоге данных (0600). При старте
  приложение завершает процессы из файла, у которых в командной строке есть путь их `-E` лог-файла (так чужой процесс
  с тем же PID не пострадает), и очищает файл.

### 5.1 Askpass

Пароль и пассфразу отдаёт сам бинарь `glafira-pg` в режиме askpass. Окружение процесса `ssh`:
`SSH_ASKPASS=<путь к текущему бинарю>`, `SSH_ASKPASS_REQUIRE=force`, `DISPLAY=:0` (если не задан),
`GLAFIRA_ASKPASS=<id подключения>`, `GLAFIRA_ASKPASS_REPORT=<файл 0600 в каталоге данных>`,
`GLAFIRA_ASKPASS_TRUST=<отпечаток>` (только при подтверждении ключа хоста).

`main()` до запуска Tauri: задана `GLAFIRA_ASKPASS` → режим askpass (Tauri, WebView, драйверы не инициализируются).
ssh передаёт текст запроса единственным аргументом:
- запрос содержит `password` или `passphrase` (без учёта регистра) → в stdout секрет `sshPassword` / `sshPassphrase`
  из keychain, код 0; секрета нет → код 1;
- запрос о неизвестном ключе хоста (`authenticity of host` … `fingerprint is SHA256:…`) → если отпечаток в запросе
  совпадает с `GLAFIRA_ASKPASS_TRUST` — `yes`, иначе текст запроса пишется в `GLAFIRA_ASKPASS_REPORT` и ответ `no`;
- любой другой запрос → код 1.

Требуется OpenSSH 8.4+ (`SSH_ASKPASS_REQUIRE`). Проверка подключения читает `ssh -V`; на более старой версии способы
`password` и `key` с пассфразой недоступны с объяснением, `agent` и ключ без пассфразы работают.

### 5.2 Ошибки ssh

Код процесса и stderr превращаются в `SSH_ERROR` с `details.kind`:

| kind | Признак | UI |
|---|---|---|
| `notFound` | нет `ssh` | подсказка по установке |
| `hostKeyUnknown` | отчёт askpass с отпечатком | диалог: хост, отпечаток, «Доверять и подключиться» → повтор с `GLAFIRA_ASKPASS_TRUST`; ssh сам пишет ключ в `known_hosts` |
| `hostKeyChanged` | `REMOTE HOST IDENTIFICATION HAS CHANGED` | только объяснение; обойти из приложения нельзя |
| `authFailed` | `Permission denied` | «Изменить подключение» |
| `forwardFailed` | `channel … open failed` / `connect failed` | какой проброс: бастион не видит хост |
| `exited` | прочее | хвост stderr в `details` |

---

## 6. Топология

Модуль `topology/`. Узел топологии — «участник» (`member`); имя участника — имя Patroni либо `host:port`
для `server` и `hostList`.

### 6.1 Модель

```
Topology {
  connectionId, kind, observedAtMs,
  scope: Option<String>,             // Patroni: имя кластера
  paused: Option<bool>,              // Patroni: режим обслуживания
  members: [Member],
  warnings: [TopologyWarning],
}
Member {
  name, host, port, apiUrl: Option<String>,
  role: primary | standbyLeader | syncStandby | quorumStandby | replica | unknown,
  rawRole: Option<String>, state: Option<String>,   // как прислал Patroni
  timeline: Option<i64>,
  lagBytes: Option<i64>, replayDelayMs: Option<i64>,
  upstream: Option<String>,                          // список узлов: от кого реплика получает WAL
  tags: Map<String, Json>, pendingRestart: bool,
  serverVersion: Option<String>, patroniVersion: Option<String>, dcsLastSeenMs: Option<i64>,
  reachable: bool, error: Option<AppError>,
}
TopologyWarning = noPrimary | multiplePrimaries { members } | systemIdMismatch { members }
                | patroniUnreachable | memberUnreachable { name }
```

В интерфейсе роль `primary` у кластера Patroni подписывается «Leader», у списка узлов — «Primary».

### 6.2 Кластер Patroni

Клиент `reqwest`: таймаут запроса 5 с, basic auth (если задан `username`), TLS — CA, клиентский сертификат,
проверка имени хоста. При туннеле URL переписывается на локальный порт с сохранением исходного имени хоста, а имя
разрешается в `127.0.0.1` (`ClientBuilder::resolve`) — так SNI и проверка сертификата идут по исходному имени.

**Обнаружение:**
1. `GET /cluster` по адресам из `urls` по порядку до первого успешного ответа. Все не ответили →
   `patroniUnreachable`; если это первое подключение — ошибка `PATRONI_ERROR` с ответом последнего адреса.
2. Из `members[]` берутся `name`, `role`, `state`, `host`, `port`, `api_url`, `timeline`, `lag` (Patroni 3) или
   `receive_lag` / `replay_lag` (Patroni 4, в `lagBytes` — `replay_lag`), `tags`, `pending_restart`; из корня —
   `scope`, `pause`. Роль: `leader` → `primary`, `standby_leader` → `standbyLeader`, `sync_standby` → `syncStandby`,
   `quorum_standby` → `quorumStandby`, `replica` → `replica`, прочее → `unknown` (`rawRole` сохраняется).
   Неизвестные поля игнорируются.
3. `hostOverrides` применяются по имени участника.
4. Параллельно `GET /patroni` у каждого участника: `patroni.version`, `server_version`, `dcs_last_seen`,
   `database_system_identifier`. Не ответил → `memberUnreachable` (данные из `/cluster` остаются). Разные
   `database_system_identifier` → `systemIdMismatch`.

**Опрос:** раз в 10 с `GET /cluster` — сначала у лидера, затем у остальных участников, затем по `urls`.

### 6.3 Список узлов и сервер

Параллельно к каждому хосту — соединение из пула (БД `pg.database`) и запрос:

```sql
SELECT pg_is_in_recovery(),
       current_setting('server_version'),
       (SELECT system_identifier FROM pg_control_system())  -- при отказе в правах: NULL
```

- Ровно один узел не в recovery → `primary`; ноль → `noPrimary`; больше одного → `multiplePrimaries`
  (возможен split-brain — предупреждение красным в обзоре).
- Разные `system_identifier` → `systemIdMismatch` (в списке узлы разных кластеров).
- На primary: `pg_stat_replication` — `application_name`, `client_addr`, `state`, `sync_state`,
  `pg_wal_lsn_diff(pg_current_wal_lsn(), replay_lsn)`, `replay_lag`. Реплика сопоставляется со строкой по `client_addr`
  (после разрешения имени хоста) или `application_name` = имени хоста; сопоставилась → её `lagBytes`, `syncStandby`
  при `sync_state = 'sync'`.
- На репликах: `pg_stat_wal_receiver.sender_host:sender_port` → `upstream` (совпал с узлом списка — имя узла,
  иначе как есть); `pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())` — `lagBytes`, если primary
  не дал значения; `replayDelayMs` — 0, если реплика применила весь полученный WAL, иначе
  `now() - pg_last_xact_replay_timestamp()`, не меньше 0.
- Узел не ответил → `reachable = false`, `memberUnreachable`.

**Узел по умолчанию** (`hostList`): `primary` — primary; `standby` — первая доступная реплика, нет — ошибка
`NODE_UNAVAILABLE`; `preferStandby` — первая доступная реплика, иначе primary; `any` — первый доступный узел
в порядке списка.

`server` — тот же опрос одного узла, без предупреждений о primary.

**Опрос:** раз в 10 с.

### 6.4 Журнал событий

В памяти, до 500 событий на подключение (старые вытесняются), очищается при `disconnect`. События — разница между
двумя опросами: `leaderChanged { from, to }`, `memberAdded`, `memberRemoved`, `memberRoleChanged { from, to }`,
`memberStateChanged { from, to }`, `memberUnreachable`, `memberReachable`, `timelineChanged { from, to }`,
`pauseChanged { paused }`. Изменение топологии → событие Tauri `topology://changed`
`{ connectionId, topology, newEvents }`.

---

## 7. Соединения с PostgreSQL

Модуль `conn/`.

- **Сборка:** `tokio_postgres::Config` — пользователь, пароль, БД, `application_name = glafira-pg`,
  `connect_timeout`; параметр запуска `options = -c default_transaction_read_only=on -c statement_timeout=5000
  -c lock_timeout=1000`.
- **TLS:** `postgres-openssl` по `pg.sslMode` и `pg.tls`. При туннеле TCP-поток открывается к локальному порту,
  а TLS проверяется по исходному имени хоста: `Config::connect_raw(stream, TlsConnector::new(ssl, исходный_хост))`.
  `prefer` — попытка TLS, при отказе сервера — без TLS.
- **Пулы:** `deadpool-postgres` на ключ (подключение, участник, БД): максимум 3 соединения, проверка соединения при
  выдаче, закрытие после 5 мин простоя. Участник исчез из топологии → его пулы закрываются. `disconnect` → все пулы
  и туннели закрываются.
- **Узел каталога:** у подключения в памяти — `catalogMember`: по умолчанию лидер / primary (для `hostList` — узел по
  умолчанию, §6.3). Контекстное меню «Читать каталог с узла…» выбирает участника. Выбранный участник пропал или
  недоступен → возврат к узлу по умолчанию и уведомление в дереве.

**Состояние подключения** (событие `connection://state { connectionId, state, error }`):
`disconnected` → `connecting` → `connected` | `degraded` (часть участников или Patroni API недоступна, туннель
перезапускается) | `failed`. Подключение открывается при раскрытии узла дерева или открытии вкладки и закрывается
командой «Отключиться» или при выходе.

---

## 8. Дерево и каталог

```
📁 Папка
  ● billing-prod  [PROD] [Patroni] ●           ← клик: обзор кластера; точка — состояние подключения
     ▸ Узлы
        ★ pg-1   Leader   TL 7
        ○ pg-2   Sync     0 B
        ○ pg-3   Replica  12 MB  ⚠             ← ⚠ при недоступности или лаге > 16 МБ
     ▸ Базы данных  (с узла pg-1)
        ▸ orders
           ▸ public
              ▸ Таблицы (142)
                 ▸ events  (партиционированная, 24)
              ▸ Представления (3)  ▸ Мат. представления (1)
              ▸ Функции (12)       ▸ Последовательности (8)
           ▸ Расширения (4)
     ▸ Роли (17)
```

- `server` — без узла «Узлы»; в заголовке подключения — роль единственного узла.
- Уровни загружаются лениво при раскрытии; счётчики — одним запросом на схему.
- Списки объектов — страницами по 500 с виртуализацией (`@tanstack/react-virtual`), фильтр по подстроке на каждом
  уровне (выполняется в SQL, `ILIKE`).
- Системные схемы (`pg_catalog`, `information_schema`) и встроенные роли `pg_*` скрыты; показать — настройка
  «Показывать системные объекты». `pg_toast*` и `pg_temp*` не показываются никогда.
- Партиции — под родительской таблицей, с числом партиций; таблицы-партиции не дублируются в списке схемы.
- Клик по участнику — обзор кластера с выделенным участником.

**Запросы каталога** (все — на `catalogMember`, текст выбирается по `server_version_num`):

| Уровень | Источник |
|---|---|
| Базы | `pg_database` (`NOT datistemplate AND datallowconn`), `pg_database_size` (нет права `CONNECT` → размер «—») |
| Схемы и счётчики | `pg_namespace`, `pg_class` (`relkind` r, p, v, m, S, f), `pg_proc` (`prokind` f, p) |
| Объекты | `pg_class` + `reltuples`, `pg_total_relation_size`; функции — `pg_proc` с сигнатурой `pg_get_function_identity_arguments` |
| Расширения | `pg_extension` + `pg_available_extensions` (есть ли обновление) |
| Роли | `pg_roles`: атрибуты, `rolconnlimit`, `rolvaliduntil`, членство (`pg_auth_members`) |

---

## 9. Вкладки

**Обзор кластера / сервера** (`TabSpec { kind: "overview", connectionId, member? }`):
- шапка: имя, вид, `scope` и бейдж «Пауза» (Patroni), лидер / primary, состояние подключения;
- таблица участников: имя, роль, состояние, `host:port`, таймлайн, лаг, задержка replay, upstream, теги,
  pending restart, версии PG и Patroni, `dcs_last_seen`, доступность (ошибка — в подсказке);
- быстрые факты по каждому доступному участнику — один запрос на узел при каждом опросе топологии, пока вкладка
  открыта: время работы (`pg_postmaster_start_time`), соединения / `max_connections`, число БД;
- предупреждения топологии;
- журнал событий (§6.4), новые сверху.

**Таблица** (`{ kind: "table", connectionId, database, schema, name }`), подвкладки:
- **Колонки:** имя, тип (`format_type`), NOT NULL, default (`pg_get_expr`), identity / generated, collation,
  статистика колонки (`attstattarget`), storage, комментарий;
- **Индексы:** определение (`pg_get_indexdef`), PK / unique, `indisvalid` (INVALID подсвечивается), размер, `idx_scan`,
  `idx_tup_read`, `last_idx_scan` (16+);
- **Ограничения:** тип, определение (`pg_get_constraintdef`); входящие внешние ключи (кто ссылается на таблицу);
- **Размер и статистика:** heap, индексы, TOAST, всего; `reltuples`; из `pg_stat_user_tables` — seq / idx scans,
  `n_live_tup`, `n_dead_tup`, `n_mod_since_analyze`, `n_ins_since_vacuum` (13+), последние (auto)vacuum / analyze
  и их счётчики; reloptions; табличное пространство, владелец;
- **Партиционирование:** ключ (`pg_get_partkeydef`), границы партиции (`pg_get_expr(relpartbound)`), список партиций;
- **Триггеры:** имя, включён, `pg_get_triggerdef`;
- **DDL:** сгенерирован по каталогу — `CREATE TABLE` с колонками, default, NOT NULL, identity / generated,
  `PARTITION BY` / `PARTITION OF`, ограничения, `WITH (reloptions)`, индексы, триггеры, комментарии. Первая строка —
  `-- DDL построен Glafira по каталогу; не замена pg_dump` (без прав, RLS-политик, владельцев). Кнопка «Копировать».

**Представление / мат. представление:** колонки, определение (`pg_get_viewdef(oid, true)`); для мат. представления —
`ispopulated`, размер, индексы.
**Функция:** сигнатура, язык, volatility, `SECURITY DEFINER`, владелец, определение (`pg_get_functiondef`;
для агрегатов — только сигнатура).
**Последовательность:** тип, `last_value`, min, max, шаг, cycle, процент использования
(`(last_value - min) / (max - min)`), владеющая колонка.

---

## 10. Проверка подключения и чек-лист готовности

`test_connection(profile, secrets, channel)` работает со значениями формы без сохранения; секреты — из формы,
иначе из keychain по `id`, иначе `~/.pgpass`. События в канал:
`{ step, member?, status: running | ok | failed | skipped, message?, error? }`.

Шаги: `ssh` (версия ssh, туннель) → `patroni` (`/cluster`) → `discovery` (участники, предупреждения топологии) →
`node` (по каждому участнику: TCP, TLS, аутентификация, версия, роль) → `readiness`.

**Готовность** — сбор фактов в приложении, оценка — чистой функцией `pg_advisor::readiness::evaluate(facts)`.
Факты собираются на узле по умолчанию и в БД `pg.database`; `shared_preload_libraries` — на каждом участнике.

| Пункт | Проверка | Без него не работает |
|---|---|---|
| Роль мониторинга | superuser или член `pg_monitor` | Запросы других пользователей в активности, `pg_stat_statements`, `pg_ls_waldir` (P2, P3) |
| Сигналы | член `pg_signal_backend` | Отмена и завершение сессий (P2) |
| Обслуживание | владелец таблиц или `pg_maintain` (17+) | VACUUM / ANALYZE из приложения (P5) |
| `pg_stat_statements` загружен | в `shared_preload_libraries` на каждом участнике | Тяжёлые запросы (P3) |
| `pg_stat_statements` создан | расширение в БД `pg.database` | То же; подсказка: `CREATE EXTENSION` в каждой БД, где нужен анализ |
| `compute_query_id` | `on` или `auto` при загруженном `pg_stat_statements` (14+) | Связь активности с топом запросов (P2, P3) |
| `track_io_timing` | `on` | Время I/O в запросах и планах (P3) |
| `track_activity_query_size` | ≥ 4096 | Полные тексты текущих запросов (P2) |
| Доп. расширения | `hypopg`, `pgstattuple`, `pg_buffercache`, `pg_wait_sampling`, `pg_qualstats`, `pg_stat_kcache`: установлено / доступно / нет | Проверка индексов, точный bloat, точный профиль ожиданий (P3, P5) |

Статус пункта: `ok` / `warn` / `missing`; к каждому — текст «что делать» и SQL или строка конфигурации для
копирования. Скрипт создания роли для диагностики — в `docs/admin.md` и в справке:

```sql
CREATE ROLE glafira_diag LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;   -- отмена и завершение сессий
GRANT pg_maintain TO glafira_diag;         -- PostgreSQL 17+: VACUUM и ANALYZE
-- в каждой БД, где нужен анализ запросов (и pg_stat_statements в shared_preload_libraries):
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
```

---

## 11. IPC-контракт

| Группа | Команды |
|---|---|
| Подключения | `list_connections`, `save_connection(profile, secrets)`, `delete_connection`, `parse_connection_string(text)`, `test_connection(profile, secrets, channel)`, `trust_ssh_host_key(connectionId \| profile, fingerprint)`, `connect(id) -> Topology`, `disconnect(id)` |
| Топология | `get_topology(id)`, `list_topology_events(id)`, `set_catalog_member(id, member \| null)`, `get_member_facts(id, member)` |
| Каталог | `list_databases(id)`, `list_schemas(id, db)`, `list_objects(id, db, schema, kind, filter?, offset, limit)`, `list_partitions(id, db, schema, table)`, `get_table(id, db, schema, name)`, `get_view(id, db, schema, name)`, `get_function(id, db, oid)`, `get_sequence(id, db, schema, name)`, `list_extensions(id, db)`, `list_roles(id)` |
| Папки | как в Glafira Kafka: `list_folders`, `create_folder`, `rename_folder`, `delete_folder`, `move_items`, `export_connections`, `preview_import`, `apply_import` |
| Приложение | `get_app_info`, `export_diagnostics`, `get_policy_status`, `get_license_status`, `set_license_key`, `remove_license_key`, `get_language_settings`, `set_language`, `get_ui_settings`, `set_ui_settings` |
| События | `connection://state`, `topology://changed` |

`trust_ssh_host_key` повторяет подключение (или проверку) с `GLAFIRA_ASKPASS_TRUST`. Все команды возвращают
`Result<T, AppError>`.

---

## 12. Форма подключения

Разделы (раскладка `.form` из Glafira Kafka):
1. **Общее:** имя, цвет, окружение, read-only, поле «Вставить строку подключения», вид (три варианта с пояснением).
2. **Узлы:** `server` — хост и порт; `hostList` — редактируемый список (добавить, удалить, переставить), целевой узел,
   флажок «Узлы под управлением Patroni»; `patroni` — список адресов API, пользователь и пароль API, TLS API,
   свёрнутый раздел «Подмена адресов узлов».
3. **PostgreSQL:** пользователь, пароль (подсказка про `~/.pgpass`), БД по умолчанию, `sslmode`, CA, клиентский
   сертификат, таймаут, фильтр баз.
4. **SSH:** выключатель; цель (хост или алиас), порт, пользователь, способ входа, путь к ключу, пароль или пассфраза.
5. Кнопки «Проверить» и «Сохранить»; под ними — шаги проверки и чек-лист готовности.

Поля с неочевидным смыслом получают `Hint` (как в Glafira Kafka): `targetServerType`, подмена адресов, `sslmode`,
алиас ssh, `~/.pgpass`.

---

## 13. Ошибки

Модель и коды Glafira Kafka (`CONNECTION_FAILED`, `TLS_ERROR`, `AUTH_FAILED`, `NOT_AUTHORIZED`, `TIMEOUT`,
`INVALID_INPUT`, `INTERNAL`) плюс новые: `SSH_ERROR` (§5.2), `PATRONI_ERROR` (`details`: адрес, HTTP-статус, текст
ответа), `NODE_UNAVAILABLE`, `FEATURE_UNAVAILABLE`. Тексты — ключи i18n (`ltext!`), как в Glafira Kafka.

| SQLSTATE / причина | code | Подсказка |
|---|---|---|
| `28P01`, `28000` | `AUTH_FAILED` | Пароль, `pg_hba.conf` (текст сервера — в `details`) |
| `3D000` | `INVALID_INPUT` | БД не существует |
| `42501` | `NOT_AUTHORIZED` | Какой роли не хватает |
| `57014` | `TIMEOUT` | Запрос каталога прерван `statement_timeout` |
| `53300` | `CONNECTION_FAILED` | Исчерпан `max_connections` |
| `57P03` | `NODE_UNAVAILABLE` | Узел запускается или в восстановлении; `retryable` |
| Ошибка TLS-рукопожатия, несовпадение имени | `TLS_ERROR` | Конкретная причина OpenSSL |
| TCP: отказ, таймаут, DNS | `CONNECTION_FAILED` | При туннеле — «бастион не видит хост» |

Частичные результаты: недоступный участник помечается в дереве и обзоре, остальные работают; размер БД без права
`CONNECT` — «—» с причиной в подсказке.

---

## 14. Структура репозитория

```
postgres-patroni-diag/
  package.json, pnpm-lock.yaml, vite.config.ts, vitest.config.ts, tsconfig*.json, index.html
  src/                      api/ components/{connections, tree, overview, catalog, settings, license, about, help, common}
                            lib/ state/ i18n/ locales/{en,ru}.json guide/
  src-tauri/
    src/  main.rs lib.rs product.rs error.rs logging.rs settings.rs secrets.rs state.rs i18n.rs sync.rs
          policy/ license/ storage/{fs, connections, tree, exchange}
          conn/{profile, connstr, pgpass, pg, pool}  net/{ssh, askpass}
          topology/{model, patroni, probe, journal, poller}
          catalog/{databases, schemas, objects, table, view, function, sequence, extensions, roles, ddl}
          readiness/  commands/
    tests/                  интеграционные
    license-public.key, tauri.conf.json, capabilities/
  crates/pg-advisor/        readiness (P1); правила и разбор планов — с P2/P3
  dev/                      docker-compose.yml, patroni/, ssh/, sql/
  docs/                     admin.md, guide/{en,ru}/, superpowers/{specs,plans}/
  scripts/                  i18n-check.mjs, guide-publish.mjs
  .github/workflows/ci.yml
```

---

## 15. Тестирование

### 15.1 Стенд (`dev/docker-compose.yml`)

| Сервис | Назначение | Порт на хосте |
|---|---|---|
| `etcd` | DCS для Patroni | — |
| `patroni1..3` | Свой образ: `postgres:17` + `patroni[etcd3]`; `pg_stat_statements` в `shared_preload_libraries`; basic auth REST API | PG 5441–5443, API 8011–8013 |
| `pg-primary`, `pg-replica` | `postgres:16`, потоковая репликация без Patroni (`hostList`) | 5451, 5452 |
| `pg-single` | `postgres:15` (`server`), без `pg_stat_statements` — для «красного» чек-листа | 5460 |
| `bastion` | OpenSSH-сервер во внутренней сети стенда: пользователь `tester`, тестовый ключ (без пассфразы и с пассфразой) и пароль | 2222 |
| профиль `matrix`: `pg13`…`pg18` | Матрица версий для запросов каталога | 5513–5518 |

`dev/sql/init.sql` на всех PG: роль `glafira_diag` (§10), демо-схема `shop` — таблицы с PK, FK, индексами, identity
и generated-колонками, партиционированная таблица, представление, мат. представление, функция, процедура,
последовательность, триггер. `dev/sql/many-tables.sql` — схема `wide` с 10 000 таблиц (запускается вручную).
Тестовые ключи и пароли стенда — в `dev/ssh/README.md` и README.

### 15.2 Автотесты

- **Rust unit:**
  - `connstr`: все форматы и правила §4, включая IPv6, кавычки и экранирование, списки портов, unix-сокеты,
    `ignored`;
  - `pgpass`: экранирование `\:`, `*`, порядок строк, права файла;
  - сериализация профиля и правила полей §3.1, локальные поля в экспорте;
  - разбор `/cluster` и `/patroni` по фикстурам Patroni 3.x и 4.x; соответствие ролей;
  - вычисление ролей и предупреждений списка узлов по фикстурам фактов;
  - журнал событий — разница двух топологий;
  - askpass: выбор ответа по тексту запроса и `GLAFIRA_ASKPASS_TRUST`; разбор stderr ssh в `details.kind`;
  - генератор DDL — сравнение с эталонами `src-tauri/tests/snapshots/ddl/*.sql` по фикстурам `TableDetails`;
  - `product.rs` совпадает с `tauri.conf.json` (как в Glafira Kafka).
- **`pg-advisor`:** `readiness::evaluate` на наборах фактов (суперпользователь, `pg_monitor` без сигналов,
  нет расширения, PG 13 без `compute_query_id`).
- **Интеграционные** (`KE_IT_*`-подобные переменные `GPG_IT_*`; без переменной тест пропускается):
  - Patroni: обнаружение трёх участников и лидера; `patronictl switchover` в контейнере → `leaderChanged`
    в журнале ≤ 15 с; остановка участника → `memberUnreachable`; `patronictl pause` → `paused`;
  - список узлов: primary и реплика с лагом; остановка primary → `noPrimary`; `pg_promote()` на реплике →
    новый primary;
  - SSH: подключение к каждому виду через `bastion` — ключ, ключ с пассфразой (askpass), пароль (askpass);
    неизвестный ключ хоста → `hostKeyUnknown` с отпечатком → `trust_ssh_host_key` → успех; неверный пароль →
    `authFailed`;
  - TLS: `verify-full` через туннель проверяет исходное имя хоста;
  - каталог: каждый запрос §8–9 на демо-схеме для PG 13–18 (профиль `matrix`) без ошибок, с ожидаемыми объектами;
  - чек-лист: `pg-single` — `missing` для `pg_stat_statements`, `patroni1` — `ok`.
- **Фронтенд** (Vitest + Testing Library, IPC замокан): форма (виды, вставка строки, переключение в Patroni,
  Pro-блокировка prod), дерево (ленивая загрузка, фильтр, системные объекты, партиции, состояние подключения),
  обзор (предупреждения, журнал), вкладка таблицы, диалог доверия ключу хоста, импорт.
- **Скрипты:** `i18n-check` (все строки в `en` и `ru`, в Rust нет кириллицы в пользовательских сообщениях),
  `guide-publish`.

### 15.3 CI

GitHub Actions: Linux — `pnpm test`, `pnpm i18n:check`, `cargo test` (unit, `pg-advisor`), интеграционные тесты
со стендом (`docker compose up`, включая `matrix`); macOS и Windows — `cargo check` и `pnpm build` (платформенный код
ssh, askpass и завершения процессов должен собираться).

---

## 16. Документация

- `README.md`: назначение, требования, запуск, стенд и его учётные данные, тесты, где хранятся данные, пометка
  о товарном знаке PostgreSQL.
- `docs/admin.md`: политика (`licenseKey`, `language`), каталоги данных и логов, роль `glafira_diag`, сеть
  (какие порты нужны напрямую и через бастион), версия OpenSSH.
- Руководство (`docs/guide/{en,ru}`, F1): «Начало работы», «Подключения» (виды, строка подключения, SSH, Patroni,
  подмена адресов, чек-лист готовности), «Дерево и объекты».

---

## 17. Критерии готовности

1. Каждый из трёх видов подключается к стенду напрямую и через `bastion` (ключ, ключ с пассфразой, пароль).
2. Строка JDBC, URI и conninfo из §4 заполняет форму; нераспознанные параметры показаны.
3. Кластер Patroni: в дереве три участника с ролями и лагом; после switchover дерево и журнал обновляются ≤ 15 с.
4. Список узлов: primary определяется; при остановке primary — предупреждение `noPrimary`; после promote реплики —
   новый primary.
5. Проверка подключения показывает шаги и чек-лист с верными статусами для `pg-single` и `patroni1`.
6. Дерево доходит до таблицы демо-схемы на PG 13–18; вкладка таблицы показывает колонки, индексы, ограничения,
   размеры, статистику, партиции, триггеры и DDL; схема `wide` (10 000 таблиц) прокручивается без задержек.
7. Папки, экспорт и импорт без секретов, переключение ru / en, Pro-блокировка prod работают.
8. CI зелёный на всех трёх ОС.
