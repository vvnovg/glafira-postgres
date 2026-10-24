# Glafira for PostgreSQL — этап P2 «Дежурный набор»

**Дата:** 2026-10-24
**Статус:** на ревью
**Концепция:** `2026-10-19-glafira-postgres-concept-design.md` (§5 — инструменты со ★, §7 — безопасные действия, §9 — этапы)
**Предыдущий этап:** `2026-10-19-glafira-postgres-p1-design.md` (модель топологии, пулы, IPC, стиль)
**Образец кода:** Glafira for Apache Kafka, `/Users/vvnovg/kafka-explorer` (далее — «Glafira Kafka»): `guard.rs`,
`ConfirmDialog`, реестр сессий `browse/registry.rs`, журнал `crates/glafira-mcp/src/audit.rs`

P2 превращает навигатор P1 в инструмент дежурного: за минуту понять, что происходит на узле и в кластере (пульс, сессии,
блокировки, горизонт xmin, реплики, слоты, готовность Patroni к failover), и безопасно выполнить типичное лечение —
отменить запрос, завершить сессии, удалить мёртвый слот, переключить лидера — с предпросмотром, подтверждением и журналом.
Каждый инструмент — вкладка с данными и блоком «Находки»; правила находок — чистые функции `pg-advisor`. Формат отчёта
инструмента («данные + находки») — основа будущего `run_diagnostic` MCP (P4) и отчёта «Проверка кластера» (P5).

---

## 1. Объём

**Входит:**
1. Каркас диагностики: модуль `diag/`, команда `run_diagnostic` с общим конвертом отчёта, SQL-наборы по `server_version_num`,
   находки и правила в `pg-advisor`.
2. Инструменты со ★ из §5 концепции: пульс узла и кластера; активные сессии; idle in transaction; профиль ожиданий;
   дерево блокировок и DDL в очереди; что держит горизонт xmin; реплики; слоты репликации; Patroni — топология, готовность
   к failover, матрица health.
3. Сэмплер активности (раз в секунду на узел, в памяти, пока открыта вкладка).
4. Patroni: чтение `GET /config`, запланированные switchover и restart из `/cluster` в модели топологии.
5. Действия §7 концепции, кроме VACUUM / ANALYZE и reload: отмена запроса, завершение сессий поштучно и пакетом, удаление
   слота, switchover (сейчас и по расписанию, отмена), failover, restart (сейчас и по расписанию, отмена), pause / resume.
6. Защита §7.1–7.2: read-only, подтверждение именем на prod, предпросмотр, защита от переиспользования PID, наблюдение после
   переключения, предпроверка прав; журнал действий `actions.jsonl` и его вкладка.
7. Стенд: роль приложения `glafira_app`, сервис `pg-lab`; интеграционные тесты сценариев; матрица PG 13–18 для каждого SQL-набора.
8. Руководство: глава «Диагностика» (инструменты, находки, действия); `docs/admin.md` — права и нагрузка.

**План реализации делится на две части**, каждая заканчивается работающим приложением (как P1a / P1b):
- **P2a «Видеть»:** пункты 1–4, 7 (кроме сценариев действий), 8 (кроме раздела действий). Только чтение.
- **P2b «Действовать»:** пункты 5–6, сценарии действий на стенде, раздел руководства о действиях, пункт чек-листа
  `replicationAttr` (§8.7).

**Не входит:** VACUUM / ANALYZE, reload конфигурации, остальной каталог §5 и `/history` Patroni (P5); топ запросов, EXPLAIN,
`pg_query` (P3); MCP (P4); снимки на диск и их сравнение; `pg_wait_sampling` и другие опциональные расширения; настройка
строгости находок по окружению (P5, вместе с отчётом); фоновый сбор без открытой вкладки.

**Лицензия.** Все функции P2 бесплатны: кода лицензирования в приложении нет (спец. P1 §2.1), деление Free / Pro из §10
концепции не действует — действия Patroni доступны всем.

---

## 2. Что берём из P1 и Glafira Kafka

| Откуда | Что | Изменения в P2 |
|---|---|---|
| P1 `topology/model.rs` | `Topology`, `Member` | `Topology.scheduledSwitchover`, `Member.scheduledRestart` (§5.10); `same_except_time` учитывает новые поля |
| P1 `topology/patroni.rs` | `PatroniHttp`, `ReqwestHttp`, разбор `/cluster` | `get_status(url)` (матрица health), `get_config()`; P2b — `send_json(method, url, body)` |
| P1 `topology/runtime.rs` | `Runtimes`, `TopologyView`, `target(id, member)` | Подписчики закрытия подключения (§3.4); `boost(id, interval, until)` — ускоренный опрос для наблюдения (§8.6) |
| P1 `conn/pool.rs` | `PoolPurpose`, `PoolRegistry` | `PoolPurpose::Sampler` (одно соединение на узел) |
| P1 `conn/pg.rs` | `ConnectSpec`, `SESSION_OPTIONS` | P2b: `ACTION_SESSION_OPTIONS` — без `default_transaction_read_only` (§8.2) |
| P1 `commands/catalog.rs` | `on_catalog_node_within`, предел `catalog_limit` | Те же пределы и закрытие соединения при таймауте для запросов диагностики |
| P1 `error.rs`, `conn/errors.rs` | `AppError`, `classify_sqlstate` | Коды `RECOVERY_CONFLICT`; P2b — `READ_ONLY`, `CONFIRMATION_REQUIRED` (§10) |
| P1 `readiness`, `pg-advisor::readiness` | Чек-лист | P2b: пункт `replicationAttr`, факт `has_replication` |
| P1 `src/state/workspace.ts` | `TabSpec` | Виды `diag` и `actions` (§7.2) |
| P1 `src/lib/useConnectionEvents.ts` | `PG_QUERY_KEYS` | Ключ `diag` |
| Glafira Kafka `guard.rs` | `ensure_mutation_allowed`, `ensure_name_confirmed` | P2b: без изменений интерфейса |
| Glafira Kafka `common/ConfirmDialog.tsx` | Подтверждение внутри страницы, тип `Confirmation` | P2b: без изменений интерфейса, тексты — свои ключи |
| Glafira Kafka `browse/registry.rs` | Реестр длительных сессий по id | Сессии — tokio-задачи с `AbortHandle` вместо потоков с флагом (§3.5) |
| Glafira Kafka `glafira-mcp/src/audit.rs` | Журнал JSONL с ротацией и правами 0600 | P2b: копия логики в `actions/journal.rs` (не зависимость) |

Скопированные модули сохраняют имена и публичные функции (концепция §2: позже — общий монорепозиторий).

---

## 3. Архитектура

### 3.1 Модули

```
src-tauri/src/
  diag/        mod.rs     Tool, DiagRequest, DiagReport, run (диспетчер)
               caps.rs    возможности сервера по server_version_num (§3.3)
               memory.rs  прошлые счётчики для скоростей и роста (§3.6)
               pulse.rs sessions.rs waits.rs locks.rs horizon.rs replication.rs slots.rs patroni.rs
  sampler/     registry.rs (сэмплер на узел, подписчики), buffer.rs (кольцевой буфер), query.rs
  session.rs   реестр длительных сессий (сэмплер, наблюдение), stop_session
  actions/     (P2b) sessions.rs slots.rs patroni.rs watch.rs journal.rs
  guard.rs     (P2b) из Glafira Kafka
  commands/    diag.rs, actions.rs (P2b)
crates/pg-advisor/src/
  findings.rs  Finding, Severity, FindingLink, пороги
  rates.rs     скорости и дельты по двум снимкам счётчиков
  locktree.rs  лес блокировок, циклы, DDL в очереди
  ash.rs       агрегация сэмплов в профиль ожиданий
  failover.rs  вердикты кандидатов Patroni
  rules/       pulse.rs sessions.rs waits.rs locks.rs horizon.rs replication.rs slots.rs patroni.rs
```

`pg-advisor` по-прежнему без ввода-вывода: вход — структуры фактов (свои типы крейта), выход — данные инструмента
и находки. Сбор фактов (SQL, HTTP) — в `diag/`.

### 3.2 Поток данных и конверт отчёта

Вкладка вызывает `run_diagnostic(id, request)`:

```
DiagRequest { tool: pulse | sessions | waits | locks | horizon | replication | slots | failover | health,
              member: Option<String>,      // null — «следовать за лидером»; у инструментов кластера игнорируется
              windowS: Option<u32> }       // только waits: 300 | 900 | 3600, по умолчанию 900
```

1. `Runtimes::target(id, member)` разрешает узел так же, как для каталога: `null` — лидер Patroni, primary списка узлов
   (узел по умолчанию, спец. P1 §6.3) или единственный сервер. Подключение не открыто — `errors.topology.notConnected`.
2. Узловые инструменты берут соединение из пула `Catalog` узла (БД `pg.database`: все используемые представления
   кластерные). Сессия уже read-only с `statement_timeout = 5 с` и `lock_timeout = 1 с` (спец. P1 §7). Получение
   соединения и запросы укладываются в `catalog_limit` (таймаут подключения + 10 с); таймаут и обрыв закрывают соединение.
3. `diag::<tool>::collect` читает факты; длительности считаются в SQL от `now()` сервера (часы клиента не участвуют).
4. `pg_advisor::rules::<tool>` строит данные и находки.

```
DiagReport {
  tool, member: Option<String>, followsLeader: bool, observedAtMs, serverVersionNum: Option<i32>, inRecovery: Option<bool>,
  data: { kind: "<tool>", … },                  // свой тип у каждого инструмента (§5)
  findings: [Finding],                           // §4
  unavailable: [{ part: String, reason: LocalizedText }]   // что не показано и почему
}
```

Инструменты кластера (`pulse`, `failover`, `health`) возвращают `member = null`, `serverVersionNum` и `inRecovery` — `null`;
ошибки отдельных узлов — внутри `data` (строка узла, клетка матрицы), а не ошибкой команды.

### 3.3 Версии PostgreSQL 13–18

`Caps::from(server_version_num)` даёт флаги возможностей; SQL собирается из фрагментов, и колонка, которой нет в версии,
выбирается как `NULL::<тип> AS <имя>` — форма строки одна на все версии, а `unavailable` объясняет пропуск
(`ltext!("diag.unavailable.version", "{{part}}: PostgreSQL {{version}}+")`).

| Возможность | С версии | Где используется |
|---|---|---|
| `pg_stat_activity.query_id` | 14 | Сессии, сэмплер, журнал действий |
| `pg_locks.waitstart` | 14 | Время ожидания блокировки (на 13 — от `state_change`, помечается «≈») |
| `pg_get_wal_replay_pause_state()` | 14 | Реплики (на 13 — `pg_is_wal_replay_paused()`) |
| `pg_replication_slots.two_phase` | 14 | Слоты |
| `pg_stat_subscription_stats` | 15 | Слоты и подписки: счётчики ошибок |
| `reserved_connections` | 16 | Пульс: доступные соединения |
| `pg_replication_slots.conflicting` | 16 | Слоты |
| `pg_stat_database_conflicts.confl_active_logicalslot` | 16 | Реплики |
| `pg_replication_slots.inactive_since`, `invalidation_reason`, `failover`, `synced` | 17 | Слоты, предпросмотр удаления |

Возможности 13+ (`wal_status`, `safe_wal_size`, `written_lsn` в `pg_stat_wal_receiver`, `leader_pid`, `write/flush/replay_lag`,
`reply_time`, `pg_blocking_pids`, `pg_ls_waldir`) используются без флагов. Матрица стенда прогоняет каждый набор на 13–18 (§12.2).

### 3.4 Соединения и жизненный цикл

| Назначение | Соединений на узел | Когда |
|---|---|---|
| `Catalog` (каталог и все вкладки, включая диагностику) | до 3 | Как в P1 |
| `Probe` (опрос топологии) | 1 | Как в P1 |
| `Sampler` (новое) | 1 | Пока к сэмплеру узла подключена вкладка; закрывается через 5 мин простоя, как остальные пулы |
| Действие (P2b, вне пулов) | 1 | На время одного действия или пакета (§8.2) |

`Runtimes` сообщает подписчикам о закрытии и переоткрытии подключения (`disconnect`, удаление, переоткрытие с другими
настройками, исчезновение участника): сэмплеры подключения останавливаются, их буферы и прошлые счётчики (§3.6) удаляются,
сессии наблюдения прерываются. Каждое такое состояние привязано к номеру открытия подключения (`Entry::generation`): ответ,
пришедший после переоткрытия, к новому открытию не применяется.

### 3.5 Длительные сессии

`session.rs` — реестр `SessionId → { connectionId, kind: sampler | watch, abort }` (как `SessionRegistry` Glafira Kafka, но
для tokio-задач). `stop_session(sessionId)` прерывает задачу и возвращает `true`, если сессия была. Закрытие подключения
прерывает все его сессии; выход из приложения — все. Данные во фронтенд идут через `tauri::ipc::Channel`; закрытый канал
(вкладка закрыта, вебвью перезагружен) завершает сессию так же, как `stop_session`.

### 3.6 Обновление, скорости и рост

- Вкладка опрашивает `run_diagnostic`, только пока видна: по умолчанию раз в 5 с (`health` и `failover` — раз в 10 с),
  в шапке — выбор 2 / 5 / 10 / 30 с или «Пауза» и кнопка «Обновить». Выбор не сохраняется между запусками.
- После `AUTH_FAILED` опрос останавливается (как у фактов узлов, спец. P1 §9): повтор с неверным паролем блокирует учётную запись.
- Скорости пульса и рост счётчиков (конфликты recovery, ошибки подписок) — разница двух соседних вызовов: `diag/memory.rs`
  хранит прошлые счётчики на ключ (подключение, номер открытия, узел, инструмент) вместе с серверным временем снимка,
  `pg_postmaster_start_time()` и `stats_reset`. Первый вызов, перезапуск сервера или сброс статистики — скорости `null`
  («…» в интерфейсе), находки по росту не срабатывают. Расчёт — `pg_advisor::rates`.

---

## 4. Находки

```
Finding {
  id: String,                           // глобальный id правила, например "idleInTransaction"
  severity: info | warning | critical,
  member: Option<String>,               // узел, где найдено (у инструментов кластера)
  target: Option<FindingTarget>,        // session { pid, backendStart } | slot { name } | member { name }
                                        // | relation { name } | database { name }
  params: Map<String, Json>,            // цифры для текста: длительность, байты, число жертв…
  links: [FindingLink],                 // openTool { tool, member } ; P2b: action { kind, … } — открыть форму действия
}
```

- Тексты — в словарях по `id`: `findings.<id>.title` (с параметрами) и `findings.<id>.fix` («что сделать»), как пункты чек-листа.
  Готовые команды для копирования (`ROLLBACK PREPARED '…'`, `ALTER ROLE … SET idle_in_transaction_session_timeout`) — в `params.command`,
  не переводятся и приложением не выполняются.
- Пороги — константы `pg_advisor::findings::thresholds`; строгость от окружения подключения не зависит (P2).
- Одно правило может срабатывать в нескольких инструментах (например, `longTransaction` — в пульсе и в сессиях): id общий.
- Порядок в отчёте: строгость (critical → info), затем `member`, затем `id`.

| id | Инструменты | warning | critical |
|---|---|---|---|
| `connectionsSaturated` | pulse | занято ≥ 80 % доступных | ≥ 95 % |
| `idleInTransaction` | sessions, pulse | простой ≥ 5 мин | ≥ 30 мин |
| `longQuery` | sessions, pulse | активный запрос ≥ 15 мин | ≥ 1 ч |
| `longTransaction` | sessions, pulse | транзакция ≥ 1 ч | ≥ 6 ч |
| `xidAge` | pulse | `max(age(datfrozenxid))` ≥ 50 % от 2³¹ | ≥ 75 % |
| `lockWait` | pulse, locks | ожидание блокировки ≥ 30 с | ≥ 5 мин |
| `walDirSize` | pulse | `pg_wal` > 2 × `max_wal_size` | > 4 × |
| `deadlocks` | pulse | рост `deadlocks` за интервал | — |
| `lockShare` | waits | доля `Lock` в окне ≥ 20 % | ≥ 50 % |
| `rootBlocker` | locks | у корня жертва ждёт ≥ 10 с | ≥ 60 с или ≥ 10 жертв |
| `ddlQueue` | locks | — | есть DDL в очереди |
| `xminHorizon` | horizon | держатель старше 10 млн xid или 1 ч | 100 млн xid или 6 ч |
| `replicaLag` | replication, pulse | ≥ 16 МБ или ≥ 30 с | ≥ 1 ГБ или ≥ 5 мин |
| `replicaNotStreaming` | replication | `state` ≠ `streaming` | — |
| `replicaReplyStale` | replication | `reply_time` старше 60 с | — |
| `syncStandbyMissing` | replication | — | `synchronous_standby_names` задан, sync / quorum реплики нет |
| `walReceiverDown` | replication | — | на реплике нет работающего WAL receiver |
| `replayPaused` | replication | replay на паузе | — |
| `recoveryConflicts` | replication | рост конфликтов за интервал | — |
| `slotRetainsWal` | slots | неактивный слот держит ≥ 1 ГБ | ≥ 10 ГБ или `wal_status = unreserved` |
| `slotLost` | slots | — | `wal_status = lost` |
| `subscriptionErrors` | slots | рост `apply_error_count` / `sync_error_count` | — |
| `failoverCandidates` | failover | один готовый кандидат при ≥ 3 участниках | ни одного |
| `clusterPaused` | failover | pause включена | — |
| `syncModeWithoutSync` | failover | — | `synchronous_mode` включён, sync / quorum реплики нет |
| `patroniTiming` | failover | `ttl < loop_wait + 2 × retry_timeout` | — |
| `healthPrimary` | health | — | `/primary` = 200 не ровно у одного участника |
| `healthUnhealthy` | health | — | `/health` ≠ 200 у доступного API |
| `replicaOutOfPool` | health | реплика без `/replica` = 200 (вне пула чтения балансировщика) | — |

Info: `cacheHitLow` (cache hit за интервал < 90 % при ≥ 1000 прочитанных блоков), `tempFiles` (рост `temp_files`),
`noIdleTimeout` (`idle_in_transaction_session_timeout = 0` при сессиях idle in transaction ≥ 5 мин),
`hiddenQueries` (есть `<insufficient privilege>` — нужен `pg_monitor`), `lockCycle` (цикл ожиданий: deadlock detector
разрешит за `deadlock_timeout`), `recoveryConflictsTotal` (конфликты есть, но не растут).

---

## 5. Инструменты

### 5.1 Пульс (`pulse`, кластер)

Опрашивает все доступные участники параллельно, по одному запросу на узел (каждый — в пределе `catalog_limit`); недоступный
участник или ошибка — строка с ошибкой. Данные: `nodes: [PulseNode]`.

| Поле `PulseNode` | Источник |
|---|---|
| `member`, `role`, `reachable`, `error` | Топология; ошибка запроса |
| `connections { used, available, byState { active, idle, idleInTransaction, other } }` | `pg_stat_activity` (`backend_type = 'client backend'`); `available = max_connections − superuser_reserved_connections − reserved_connections (16+)` |
| `rates { tps, commitsPerS, rollbacksPerS, cacheHitPct, tempFilesPerS, tempBytesPerS, deadlocks, walBytesPerS }` | Дельты сумм `pg_stat_database` и LSN (`pg_current_wal_lsn()`, на реплике `pg_last_wal_receive_lsn()`), §3.6 |
| `longestTransactionS`, `longestQueryS`, `longestIdleInTransactionS` | `now() − xact_start` / `query_start` (активные) / `state_change` (idle in transaction), client backend |
| `lockWaiting`, `longestLockWaitS` | `wait_event_type = 'Lock'`; время — `waitstart` (14+) или `state_change` |
| `maxXidAge`, `xidAgePct` | `max(age(datfrozenxid))` из `pg_database`, доля от 2³¹ |
| `walDirBytes`, `maxWalSize` | `sum(size) FROM pg_ls_waldir()` (`pg_monitor`; без права — `unavailable`), `max_wal_size` |
| `lagBytes`, `replayDelayMs` | Из топологии (запроса нет) |

Находки пульса — правила из таблицы §4 по каждой строке (`member` заполнен).

### 5.2 Сессии и idle in transaction (`sessions`, узел)

Данные: `rows: [SessionRow]`, `settings { idleInTransactionSessionTimeout, trackActivityQuerySize }`.

`SessionRow`: `pid`, `leaderPid`, `backendType`, `database`, `user`, `application`, `clientAddr`, `clientPort`,
`backendStartUs` (микросекунды эпохи — ключ защиты от переиспользования PID, §8.3), `xactAgeS`, `queryAgeS`, `stateAgeS`,
`state`, `waitEventType`, `waitEvent`, `backendXid`, `xminAge`, `queryId` (14+), `query` (до `track_activity_query_size`),
`blockedBy: [pid]` (только у ждущих блокировку: `CASE WHEN wait_event_type = 'Lock' THEN pg_blocking_pids(pid) END`),
`locksHeld` (одним проходом `pg_locks WHERE granted GROUP BY pid`), `glafira: bool` (`application_name = 'glafira-pg'`).

Интерфейс (§7.4): пресеты «Все», «Активные», «Idle in transaction», «Ждущие»; фильтр по подстроке; группировка по приложению,
роли, клиенту, БД со счётчиками по состояниям; «Скрыть сессии Glafira» (включено по умолчанию). Пресет idle in transaction
сортирует по `stateAgeS`, добавляет колонки «держит блокировок» и «возраст xmin» и показывает значение
`idle_in_transaction_session_timeout` с подсказкой (рекомендация — задать на роль приложения).

### 5.3 Профиль ожиданий (`waits`, узел)

Читает буфер сэмплера узла (§6); если сэмплер этого узла не запущен вкладкой, отчёт содержит то, что накоплено, и
`samplerRunning = false`.

Данные: `windowS`, `bucketS` (5 с для окна 300, 10 с для 900, 40 с для 3600), `samplerRunning`, `truncated` (окно длиннее
накопленного), `buckets: [{ startMs, sampledS, aas: { cpu, lock, lwlock, io, ipc, client, timeout, bufferPin, extension, activity, other } }]`
(среднее число активных сессий = сэмплы в интервале / секунды, когда сэмплер работал), `gaps: [{ fromMs, toMs }]`,
`topEvents: [{ class, event, samples, share }]`, `topQueries: [{ key, queryId?, text, samples, share, topEvent }]`
(ключ — `query_id`, если он есть и не 0, иначе хэш текста; на PG 13 — всегда хэш), `byApplication`, `byUser`, `byDatabase`,
`byClient: [{ name, samples, share }]` (по 10 строк). Класс `cpu` — активная сессия без wait event (`CPU*`: на процессоре
или в неинструментированном ожидании, подпись это объясняет). Параллельные воркеры учитываются за запросом лидера (`leader_pid`).
Агрегация — `pg_advisor::ash::profile(samples, window, bucket)`.

### 5.4 Блокировки и DDL в очереди (`locks`, узел)

1. Ждущие: `pg_stat_activity` с `wait_event_type = 'Lock'` и `pg_blocking_pids(pid)`.
2. По всем участникам цепочек (ждущие и блокирующие) — краткая строка сессии (как `SessionRow` без `locksHeld`) и
   запрошенная блокировка из `pg_locks WHERE NOT granted`: `locktype`, `mode`, отношение (`relation::regclass` для текущей
   БД; для другой БД — «oid N в БД X»), `waitstart` (14+).
3. `pg_advisor::locktree::build` строит лес: корень — блокирующий, который сам не ждёт; потомки — сессии, в чьём
   `pg_blocking_pids` есть родитель (жёсткие и «мягкие» блокирующие — впереди в очереди); у каждого узла — число жертв
   в поддереве. Сессии, образующие цикл, выводятся отдельно (`cycles`) и дают `lockCycle`.

**DDL в очереди:** ждущая сессия, которая запросила блокировку отношения в режиме `AccessExclusiveLock`,
`ShareRowExclusiveLock` или `ShareLock` и сама блокирует хотя бы одну сессию. Данные: `ddlQueue: [{ session, waitsFor: pid,
waitS, queued: число сессий за ней, relation }]`. Находка `ddlQueue` рекомендует отменить DDL (быстро снимает очередь) или
блокирующий запрос и задавать `lock_timeout` в миграциях. Подпись инструмента: детали deadlock есть только в логе сервера.

Данные: `roots: [LockNode { session, lock?, waitS?, waitApprox: bool, victims, children: [LockNode] }]`, `cycles: [[pid]]`, `ddlQueue`.

### 5.5 Горизонт xmin (`horizon`, узел)

Держатели, отсортированные по `age(xmin)`:

| Вид | Источник | Длительность |
|---|---|---|
| `session` | `pg_stat_activity.backend_xmin`, `backend_xid` (client backend и прочие) | `now() − xact_start` |
| `preparedXact` | `pg_prepared_xacts`: `transaction`, `gid`, `owner`, `database` | `now() − prepared` |
| `slot` | `pg_replication_slots.xmin`, `catalog_xmin` | `inactive_since` (17+), иначе нет |
| `standbyFeedback` | `pg_stat_replication.backend_xmin` (на primary и каскадной реплике) | — |

Реплика `standbyFeedback` сопоставляется с участником топологии: у Patroni `application_name` = имя участника, у списка узлов —
правила спец. P1 §6.3 (`client_addr` или `application_name`); ссылка находки открывает горизонт на этом участнике. У
`preparedXact` в `params.command` — `ROLLBACK PREPARED '<gid>'` (строковая константа по правилам `quote_literal`). Данные:
`holders: [{ kind, name, member?, xminAge, durationS?, details }]`, `hotStandbyFeedback` (значение настройки на узле).

### 5.6 Реплики (`replication`, узел)

**Отправители** (на primary и каскадной реплике), `pg_stat_replication`: `pid`, `user`, `application`, `member` (сопоставление
как в §5.5), `clientAddr`, `state`, `syncState`, `syncPriority`, лаги в байтах `sent/write/flush/replay` от текущего LSN
(`pg_current_wal_lsn()`, на каскадной реплике — `pg_last_wal_receive_lsn()`), `writeLagS`, `flushLagS`, `replayLagS`,
`replyAgeS`, `slotName` (по `pg_replication_slots.active_pid`), `xminAge`. Плюс `synchronous_standby_names`, `synchronous_commit`.

**Приёмник** (на реплике): `pg_stat_wal_receiver` — `status`, `senderHost`, `senderPort`, `slotName`, `receivedTli`,
`lastMsgAgeS`, `latestEndAgeS`; `receiveReplayGapBytes` (`pg_last_wal_receive_lsn() − pg_last_wal_replay_lsn()`), `replayDelayS`
(0, если применён весь полученный WAL, иначе `now() − pg_last_xact_replay_timestamp()`), `replayPause` (`not paused` /
`pause requested` / `paused`; на 13 — `paused` / `not paused`), конфликты `pg_stat_database_conflicts` (сумма по БД:
`tablespace`, `lock`, `snapshot`, `bufferpin`, `deadlock`, `activeLogicalslot` 16+) и их рост (§3.6), `hotStandbyFeedback`,
`maxStandbyStreamingDelay`.

### 5.7 Слоты репликации и подписки (`slots`, узел)

`pg_replication_slots`: `name`, `type`, `plugin`, `database`, `temporary`, `active`, `activePid`, `xminAge`, `catalogXminAge`,
`restartLsn`, `confirmedFlushLsn`, `walStatus`, `safeWalSize`, `retainedBytes` (текущий LSN − `restart_lsn`; на реплике
от `pg_last_wal_replay_lsn()`), `twoPhase` (14+), `conflicting` (16+), `inactiveSince`, `invalidationReason`, `failover`,
`synced` (17+). Настройка `max_slot_wal_keep_size`.

**Слоты Patroni** (только подключение `patroni`): слот участника — имя, полученное из имени участника, как это делает Patroni
(нижний регистр, символы вне `[a-z0-9_]` заменяются на `_`, обрезка до 63 байт); постоянный слот — ключ `slots` в `/config`.
Отметка `patroni: member | permanent | null`. `/config` недоступен — отметки нет, причина в `unavailable`.

**Подписки:** `pg_stat_subscription` (`name`, `pid`, `receivedLsn`, `lastMsgAgeS`, `latestEndAgeS`) и
`pg_stat_subscription_stats` (15+: `applyErrorCount`, `syncErrorCount`, их рост).

### 5.8 Patroni: готовность к failover (`failover`, кластер, только `patroni`)

Входы: топология (роль, состояние, доступность, лаг, теги `nofailover`, `failover_priority` (Patroni 4), `nosync`,
`pending_restart`) и `GET /config` (`maximum_lag_on_failover`, `synchronous_mode` — `true` / `false` / `"quorum"`,
`synchronous_mode_strict`, `synchronous_node_count`, `ttl`, `loop_wait`, `retry_timeout`), `paused`. `/config` читается одним
запросом на отчёт тем же клиентом подключения (`get_config`), отдельного кэша нет.

`pg_advisor::failover::evaluate` выдаёт по каждому участнику, кроме лидера, вердикт `ready` и причины: `unreachable`,
`notStreaming` (состояние не `streaming` / `running`), `lagTooHigh`, `lagUnknown`, `noFailoverTag`, `zeroPriority`,
`notSynchronous` (при `synchronous_mode`: не `syncStandby`, а при `"quorum"` — не `quorumStandby`). `pendingRestart` —
пометка без запрета. По кластеру: число готовых, pause, проверка таймингов. Данные: `paused`, `config { … }`,
`candidates: [{ member, ready, reasons, lagBytes }]`.

### 5.9 Patroni: матрица health (`health`, кластер, только `patroni`)

Для каждого участника с `apiUrl` параллельно — HTTP-коды `GET /primary`, `/standby-leader`, `/replica`, `/read-only`, `/sync`,
`/async`, `/health` (`PatroniHttp::get_status`, срок 5 с на запрос, без авторизации — эти адреса её не требуют). Данные:
`endpoints: [String]`, `members: [{ member, apiUrl, cells: [{ endpoint, status: Option<u16>, error: Option<AppError> }] }]`.
Подпись объясняет: это то, что видит балансировщик (например, HAProxy с `option httpchk GET /primary`); параметры вида
`/replica?lag=…` не проверяются.

### 5.10 Обзор: запланированные действия Patroni

Модель топологии получает:
- `Topology.scheduledSwitchover: Option<{ at, from: Option<String>, to: Option<String> }>` из корня `/cluster`
  (`scheduled_switchover`);
- `Member.scheduledRestart: Option<{ at, postgresVersion: Option<String> }>` из участника `/cluster` (`scheduled_restart`).

Обзор показывает баннер запланированного switchover и пометку запланированного restart в строке участника. Изменение этих
полей — изменение топологии (`same_except_time`), событие `topology://changed` уходит как обычно; новых видов событий журнала нет.

---

## 6. Сэмплер активности

- **Запрос** раз в секунду на соединении пула `Sampler` узла:
  ```sql
  SELECT pid, leader_pid, backend_type, datname, usename, application_name, client_addr, wait_event_type, wait_event,
         <query_id | NULL::bigint>, query, (extract(epoch FROM clock_timestamp()) * 1000)::bigint
  FROM pg_stat_activity
  WHERE state = 'active' AND pid <> pg_backend_pid()
    AND backend_type IN ('client backend', 'autovacuum worker', 'parallel worker')
  ```
  Фоновые процессы с постоянным ожиданием-циклом (walsender, checkpointer и т. п.) не сэмплируются: они искажали бы профиль.
- **Буфер** на узел: последние 60 мин, не больше 500 000 строк сэмплов (порядка 50 МБ); старое вытесняется. Строки (роль,
  приложение, БД, клиент, wait event, текст запроса) хранятся один раз и адресуются индексом; текст — по хэшу.
- **Жизнь.** Сэмплер узла создаётся первой подпиской `start_sampler(id, member, channel)` и останавливается, когда отписалась
  последняя (`stop_session`, закрытый канал). Буфер живёт до закрытия подключения или исчезновения участника (§3.4); повторная
  подписка продолжает его, промежуток записывается в `gaps`. Подписка держится, пока вкладка открыта, в том числе скрытая
  (история не прерывается при переключении вкладок); отчёт `run_diagnostic` опрашивается только видимой вкладкой (§3.6).
  Вкладка в режиме «за лидером» при смене лидера отписывается от сэмплера старого узла и подписывается на новый.
- **Нагрузка и сбои.** Тик, на котором прошлый запрос ещё не вернулся, пропускается (запросы не накладываются); пропуск
  больше 2 с — промежуток. Ошибка запроса — паузы 1, 2, 5, 10, 30 с и событие ошибки; `AUTH_FAILED` — сэмплер
  останавливается. Запрос идёт через туннель подключения, как остальные.
- **Канал** `SamplerEvent`: `tick { atMs, active, waiting }` (раз в секунду — индикатор «живой» во вкладке), `gap { fromMs, toMs }`,
  `error { error }`, `stopped`.

---

## 7. Интерфейс

### 7.1 Дерево

У раскрытого подключения после групп «Узлы», «Базы данных», «Роли» — группа **«Диагностика»**: Пульс, Сессии,
Idle in transaction, Профиль ожиданий, Блокировки, Горизонт xmin, Реплики, Слоты репликации; у кластера Patroni — ещё
Готовность к failover и Матрица health; P2b — Журнал действий. Клик открывает вкладку инструмента («за лидером»). В меню
подключения — пункт «Пульс».

### 7.2 Вкладка инструмента

```ts
| { kind: "diag"; connectionId: string; tool: DiagTool; member?: string | null }
| { kind: "actions"; connectionId: string }          // P2b: журнал действий подключения
type DiagTool = "pulse" | "sessions" | "idleTx" | "waits" | "locks" | "horizon" | "replication" | "slots" | "failover" | "health";
```

- `idleTx` — та же вкладка сессий с пресетом «Idle in transaction»; в Rust это инструмент `sessions`.
- Id вкладки — `diag:<подключение>:<tool>` (одна вкладка инструмента на подключение); открытие с другим `member` (например,
  из клетки пульса) обновляет её, как вкладку справки. Журнал — `actions:<подключение>`.
- **Шапка** (общая): название; селектор узла — «За лидером (pg-1)» и участники с ролями (у `server` и у инструментов кластера
  селектора нет); интервал обновления; «Обновить»; время последнего ответа.
- **Состояния:** обновление не удалось — остаются прошлые данные, баннер ошибки и пометка «устарело»; в режиме «за лидером»
  узел сменился — строка «Узел сменился: pg-1 → pg-2»; подключение закрыто — «Подключение закрыто» и «Подключиться», опрос
  стоит; инструмент Patroni у подключения без Patroni не предлагается.
- Ключ запросов `["diag", connectionId, …]` добавляется в `PG_QUERY_KEYS`: отключение удаляет данные, переподключение перечитывает.

### 7.3 Находки

Блок над данными: строгость (цвет и текстовая метка — не только цвет), заголовок с цифрами, «что сделать», кнопки ссылок.
Сворачивается; без находок — «Проблем не найдено». `unavailable` — серые строки с причиной под блоком.

### 7.4 Отображения

- **Пульс** — таблица «узлы × показатели»; клетка окрашена строгостью находки этого показателя; клик открывает инструмент
  на узле (соединения, транзакции и idle → Сессии; блокировки → Блокировки; XID → Горизонт; `pg_wal` → Слоты; лаг → Реплики).
- **Сессии** — виртуализированная таблица (`@tanstack/react-virtual`), панель строки с полным текстом запроса и
  «Копировать», пресеты, фильтр, группировка.
- **Профиль ожиданий** — свой SVG без новых библиотек: столбцы с накоплением по классам ожиданий (цвета — по правилам
  навыка dataviz при реализации), легенда, подсказка у столбца, промежутки штриховкой; табличная альтернатива для экранного
  диктора; ниже — топ ожиданий, запросов и группировок; индикатор «сэмплер работает» по событиям `tick`.
- **Блокировки** — дерево с раскрытием уровней; сверху — карточки «DDL в очереди».
- **Горизонт, реплики, слоты, готовность к failover** — таблицы; у реплик на реплике — карточка приёмника.
- **Матрица health** — сетка «участники × адреса» с кодом и цветом.
- Длительности, байты и числа — функции `src/lib/format.ts`; все строки — через словари.

### 7.5 Справка и i18n

- Глава руководства `diagnostics` (`docs/guide/{en,ru}/diagnostics.md`, в `GUIDE_CHAPTERS` и `scripts/i18n-check.mjs`):
  `{#pulse}`, `{#sessions}`, `{#waits}`, `{#locks}`, `{#horizon}`, `{#replication}`, `{#slots}`, `{#failover}`, `{#health}`,
  `{#findings}`; P2b — `{#actions}`, `{#patroni-actions}`, `{#journal}`. F1 на вкладке инструмента открывает его якорь
  (`helpChapterFor`).
- Ключи: `diag.tools.*`, `diag.*` (шапка, колонки), `findings.<id>.title|fix`, `actions.*` (P2b), ошибки — `errors.diag.*`,
  `errors.actions.*`. Все строки — en и ru; новые термины (горизонт xmin, wait event, idle in transaction, профиль ожиданий,
  корневой блокирующий) — в `docs/i18n/glossary.md`.

---

## 8. Действия (P2b)

### 8.1 Guard и подтверждения

`guard.rs` из Glafira Kafka: `ensure_mutation_allowed(profile, confirm)` — read-only → `READ_ONLY`; prod без верного имени
подключения → `CONFIRMATION_REQUIRED`; `ensure_name_confirmed(expected, typed)`. Каждая команда действия принимает
`confirmation { confirm: Option<String>, typedName: Option<String> }` и проверяет его в Rust до любого обращения к серверу.

| Действие | Всегда | На prod ещё |
|---|---|---|
| Отменить запрос, завершить сессию | Предпросмотр и кнопка | Имя подключения |
| Завершить пакетом | Предпросмотр списка | Имя подключения и число сессий (`typedName`) |
| Удалить слот | Ввод имени слота | Имя подключения |
| Switchover, отмена расписания, restart, отмена расписания, pause / resume | Предпросмотр | Имя подключения |
| Failover | Ввод имени кандидата | Имя подключения |

На read-only подключении кнопок действий нет; кнопка, на которую нет права, неактивна, причина — в подсказке (§8.7).
Действия всегда адресуются узлу по имени: предпросмотр возвращает разрешённый узел, выполнение принимает именно его.

### 8.2 Соединение для действия

Отдельное соединение вне пулов: `ConnectSpec` с `ACTION_SESSION_OPTIONS = -c statement_timeout=5000 -c lock_timeout=1000
-c search_path=pg_catalog -c DateStyle=ISO` (без `default_transaction_read_only`), `application_name = glafira-pg`, через
туннель подключения (`route_for`), БД — `pg.database` (у логического слота — его БД). Открывается на одно действие или
пакет и закрывается после. Если пул `Catalog` узла запомнил отказ во входе (`auth_failure`), действие возвращает эту ошибку
без новой попытки входа. Узел недоступен — `NODE_UNAVAILABLE`.

### 8.3 Отмена запроса и завершение сессий

- `preview_session_action(id, { member, kind: cancel | terminate, target })`, где `target`:
  `selected { sessions: [{ pid, backendStartUs }] }` | `idleInTransaction { olderThanS }` | `rootBlocker { pid, backendStartUs }`.
  Заново читает сессии и возвращает `rows: [{ session, allowed, reason? }]`, где `reason`: `superuserSession` (нужна роль
  суперпользователя), `otherRole` (нет `pg_signal_backend` и роль чужая), `notClientBackend`, `ownConnection`.
  Разрешено, если текущая роль — суперпользователь, или сессия не суперпользователя и роль — член `pg_signal_backend`,
  или роль сессии — та же (`pg_has_role(current_user, usesysid, 'MEMBER')`).
- `execute_session_action(id, { member, kind, target, sessions }, confirmation)` — до 500 сессий
  (`errors.actions.batchTooLarge`), по одной, на одном соединении, общий предел 60 с:
  ```sql
  SELECT a.usename, a.datname, a.application_name, a.client_addr, a.state, <query_id>, left(a.query, 500),
         extract(epoch FROM now() - a.xact_start), pg_terminate_backend(a.pid)      -- или pg_cancel_backend
  FROM pg_stat_activity a
  WHERE a.pid = $1 AND (extract(epoch FROM a.backend_start) * 1e6)::bigint = $2
    AND a.backend_type = 'client backend' AND a.pid <> pg_backend_pid()
    AND <критерий>
  ```
  Критерий проверяется заново: `idleInTransaction` — `state IN ('idle in transaction', 'idle in transaction (aborted)') AND
  state_change <= now() - make_interval(secs => $3)`; `rootBlocker` — сессия всё ещё есть в `pg_blocking_pids` какой-либо
  ждущей; `selected` — без условия. Сессия, которая успела проснуться или освободить блокировки, не пострадает.
- Итог по каждой сессии: `terminated` (после terminate pid с тем же `backend_start` исчез за 5 с, опрос раз в 250 мс),
  `signalSent` (отмена отправлена, или pid ещё жив через 5 с), `gone` (pid с таким `backend_start` уже нет),
  `noLongerMatches` (сессия есть, критерий не выполняется), `notAllowed` (42501), `failed { error }`.
  `backend_start` передаётся микросекундами эпохи (`int8`), значение помещается в число JavaScript. Во всех запросах
  (сессии, блокировки, предпросмотр, выполнение) оно вычисляется одним SQL-фрагментом `diag::sql::BACKEND_START_US`:
  на PG 13 `extract(epoch …)` возвращает `double precision`, и только одинаковое выражение на одном сервере гарантирует
  точное равенство (на 14+ `extract` возвращает `numeric` и точен сам по себе).

### 8.4 Удаление слота

- `preview_drop_slot(id, { member, slot })`: тип, plugin, БД, `active`, удерживаемый WAL, `wal_status`, `inactive_since` (17+),
  отметка Patroni (§5.7). Слот участника Patroni — предупреждение «Patroni пересоздаст слот, пока участник есть в кластере;
  удалите участника»; постоянный слот — «уберите слот из `/config` (`patronictl edit-config`)». Кнопка при этом доступна.
- `drop_slot(id, { member, slot }, confirmation)` с `typedName` = имя слота:
  `SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = $1 AND NOT active`.
  Ноль строк — `errors.actions.slotActiveOrGone` («Слот стал активным или исчез»). Логический слот — из соединения с его БД.
  Ошибка сервера (например, синхронизированный с primary слот на реплике 17+) — как есть в `details`.

### 8.5 Действия Patroni

Только подключение `patroni`. Basic auth профиля (логин и `patroniPassword`); без логина кнопки неактивны
(`errors.patroni.authRequired`); правило P1 о неотправке пароля на `http://`-адрес при https-профиле действует и здесь.
Запросы идут к API лидера, при отказе соединения — к остальным участникам и адресам профиля (как обход `/cluster`);
restart и его отмена — только к API самого участника.

| Действие | Запрос | Предусловие (проверяется в Rust) |
|---|---|---|
| Switchover | `POST /switchover {leader, candidate?, scheduled_at?}` | Есть доступный лидер в состоянии `running`; кандидат — участник и не лидер; время — в будущем |
| Отмена switchover | `DELETE /switchover` | `scheduledSwitchover` есть |
| Failover | `POST /failover {candidate}` | Нет доступного лидера в состоянии `running` (иначе `errors.actions.leaderHealthy`: «Лидер здоров — используйте switchover») |
| Restart | `POST <API участника>/restart {restart_pending, schedule?}` | Участник доступен; время — в будущем |
| Отмена restart | `DELETE <API участника>/restart` | `scheduledRestart` есть |
| Pause / resume | `PATCH /config {"pause": true}` / `{"pause": null}` (`null` удаляет ключ из динамической конфигурации) | Текущее `paused` другое |

- `preview_patroni_action(id, action)`: для switchover — кандидаты с вердиктом готовности (§5.8; неготовые — с причиной,
  вариант «Patroni выберет сам»), число клиентских сессий на лидере (оборвутся), предупреждения «запись недоступна несколько
  секунд», «старый лидер станет репликой»; для failover — кандидаты и состояние лидера; для restart — роль участника
  (restart лидера = недоступность записи), `pending_restart`, флажок «только если pending restart» (включён, когда он есть).
- Время расписания вводится в локальном часовом поясе и уходит в RFC 3339 со смещением.
- Ответ Patroni 2xx — успех (текст ответа показывается); иначе `PATRONI_ERROR` `errors.patroni.actionRejected` с кодом и текстом
  ответа (500 символов); 401 — `errors.patroni.authFailed`.

### 8.6 Наблюдение после действия

`execute_patroni_action(id, action, confirmation, channel)` после успешного switchover / failover / restart «сейчас» и
pause / resume открывает сессию `watch` (§3.5) и возвращает её id. Сессия включает `Runtimes::boost(id, 2 с, срок)` —
опрос топологии раз в 2 с вместо 10 (события журнала топологии пишутся как обычно) — и после каждого опроса вычисляет шаги
чистой функцией `actions::watch::progress(action, before, now)`:

| Действие | Шаги | Срок |
|---|---|---|
| Switchover, failover | принят → старый лидер сложил полномочия → новый лидер `running` (кандидат, если задан) → таймлайн вырос → реплики `streaming` / `running` → готово | 120 с |
| Restart | принят → `postmaster_start_time` из `GET <API участника>/patroni` изменился → состояние `running` / `streaming` → `pending_restart` снят (если «только pending») → готово | 300 с |
| Pause / resume | принят → `paused` в топологии = заданному → готово | 30 с |

Канал `WatchEvent { step, status: running | done | failed | timedOut, atMs, detail? }`. Срок истёк — `timedOut` с последним
состоянием. Закрытие панели наблюдения (`stop_session`) прекращает и ускоренный опрос. Запланированные действия наблюдения
не открывают: обзор показывает баннер (§5.10).

### 8.7 Предпроверка прав

| Действие | Право | Без него |
|---|---|---|
| Отмена, завершение | `pg_signal_backend`, суперпользователь или своя роль | Строка неактивна, причина в подсказке и предпросмотре |
| Удаление слота | Атрибут `REPLICATION` или суперпользователь | Кнопка неактивна; подсказка — пункт чек-листа |
| Действия Patroni | Логин API в профиле | Кнопки неактивны |

Новый пункт чек-листа готовности `replicationAttr` (`warn`, факт `has_replication` из `pg_roles.rolreplication`):
«Удаление слотов репликации» — `ALTER ROLE "<user>" REPLICATION;` с предупреждением, что атрибут позволяет роли создавать
логические слоты и читать через них изменения всех таблиц БД, а при разрешающей строке `replication` в `pg_hba.conf` —
копировать кластер; `docs/admin.md` советует выдавать его отдельной роли для действий или не выдавать вовсе.

### 8.8 Журнал действий

`actions.jsonl` в каталоге данных, права 0600, при 5 МБ ротируется в `actions.1.jsonl` (логика — копия журнала MCP
Glafira Kafka). Запись — после выполнения, best effort: сбой записи не откатывает действие, ответ команды несёт
`journalWritten: false`, интерфейс говорит «Журнал не записан». Отказы guard (read-only, неверное подтверждение) не пишутся —
ничего не произошло; ошибки сервера и Patroni пишутся с `outcome: "error"`.

```json
{"ts": 1761300000000, "action": "terminate", "connection": {"id": "6f1c…", "name": "billing-prod", "environment": "prod"},
 "member": "pg-1", "prodConfirmed": true, "outcome": "ok", "code": null, "ms": 412,
 "sessions": [{"pid": 4242, "backendStartUs": 1761299000000000, "user": "app", "database": "orders", "application": "api",
               "client": "10.0.0.5", "state": "idle in transaction", "xactAgeS": 3600, "queryId": "-123…",
               "query": "UPDATE shop.orders SET …", "result": "terminated"}]}
```

`outcome`: `ok` | `partial` (часть пакета не выполнена) | `error`. У слота — `slot { name, type, plugin, database,
retainedBytes, walStatus }`; у Patroni — `patroni { method, path, body, status, response }` (без учётных данных, ответ —
500 символов). Текст запроса — первые 500 символов, литералы остаются (решение владельца: на разборе инцидента видно, что
именно завершили). Журнал не уходит в MCP (концепция §8) и не входит в архив диагностики.

`list_actions(connectionId: Option<String>, limit ≤ 1000)` читает оба файла, новые сверху, битые строки пропускает. Вкладка
«Журнал действий» подключения — таблица: время, действие, узел, объект, итог; строка раскрывается в полную запись.

---

## 9. IPC-контракт

| Часть | Команды |
|---|---|
| P2a | `run_diagnostic(id, request) -> DiagReport`; `start_sampler(id, member: Option<String>, channel: Channel<SamplerEvent>) -> SessionId`; `stop_session(sessionId) -> bool` |
| P2b | `preview_session_action(id, request) -> SessionActionPreview`; `execute_session_action(id, request, confirmation) -> SessionActionResult`; `preview_drop_slot(id, request) -> SlotDropPreview`; `drop_slot(id, request, confirmation) -> ActionResult`; `preview_patroni_action(id, action) -> PatroniActionPreview`; `execute_patroni_action(id, action, confirmation, channel: Channel<WatchEvent>) -> PatroniActionResult { result, watch: Option<SessionId>, journalWritten }`; `list_actions(connectionId?, limit) -> [ActionRecord]` |

`PatroniAction` — `switchover { candidate?, scheduledAt? }` | `cancelSwitchover` | `failover { candidate }` |
`restart { member, onlyPending, scheduledAt? }` | `cancelRestart { member }` | `pause { paused }`.
Новых событий Tauri нет: данные длительных сессий идут через их каналы, изменения топологии — через `topology://changed`.
Все команды возвращают `Result<T, AppError>`; JSON — camelCase.

---

## 10. Ошибки

| code | Причина | Поведение UI |
|---|---|---|
| `RECOVERY_CONFLICT` (новый, P2a) | Запрос на узле в recovery отменён: SQLSTATE 40001 или 40P01 с текстом «conflict with recovery» | «Запрос на реплике отменён конфликтом recovery», `retryable` — следующий опрос повторит |
| `READ_ONLY` (P2b) | Действие на read-only подключении | Кнопок нет; команда всё равно отказывает |
| `CONFIRMATION_REQUIRED` (P2b) | Нет или неверно имя подключения, объекта, число сессий | Диалог подтверждения |
| `NOT_AUTHORIZED` (42501) | Нет права на сигнал или слот | `errors.actions.signalDenied` (нужна `pg_signal_backend`), `errors.actions.replicationRequired` |
| `PATRONI_ERROR` | Отказ Patroni на действие | `errors.patroni.actionRejected` (код и текст ответа), `authRequired`, `authFailed` |
| `INVALID_INPUT` | Нарушено предусловие | `errors.actions.leaderHealthy`, `noLeader`, `pastSchedule`, `batchTooLarge`, `slotActiveOrGone` |
| `NODE_UNAVAILABLE` | Узел действия недоступен | Как в P1 |

Частичные результаты — норма: ошибка узла остаётся в его строке пульса или клетке матрицы, ошибка одной сессии пакета — в её итоге.
Колонка, недоступная по версии, — не ошибка, а `unavailable`.

---

## 11. Структура репозитория (новое)

```
src-tauri/src/   diag/  sampler/  session.rs  actions/ (P2b)  guard.rs (P2b)  commands/{diag, actions}.rs
src-tauri/tests/ it_diag.rs  it_sampler.rs  it_actions.rs (P2b)  it_patroni_actions.rs (P2b)  snapshots/diag-sql/
crates/pg-advisor/src/  findings.rs rates.rs locktree.rs ash.rs failover.rs rules/  tests/fixtures/patroni/
src/components/diag/    DiagTab DiagHeader FindingsPanel PulseView SessionsView WaitsView WaitsChart LocksView
                        HorizonView ReplicationView SlotsView FailoverView HealthView
src/components/actions/ (P2b) SessionActionDialog DropSlotDialog PatroniActionDialog WatchPanel ActionsJournal
src/components/common/  ConfirmDialog (P2b)
src/components/connections/tree/DiagnosticsNodes.tsx
src/lib/useDiagnostic.ts  useSampler.ts
dev/  lab/ (init-скрипты pg-lab)  sql/init.sql (роль glafira_app)
docs/guide/{en,ru}/diagnostics.md
```

---

## 12. Тестирование

### 12.1 Стенд

Работающий стенд не пересоздаётся (`docker compose down` и удаление контейнеров `gp-*` запрещены): новое поднимается
`docker compose -f dev/docker-compose.yml up -d --no-recreate <сервис>`.

| Что | Зачем |
|---|---|
| Роль `glafira_app` (`app-secret`, без `pg_monitor`, права на схему `shop` в БД `orders`) | Нагрузка в тестах: `pg_signal_backend` не завершает сессии суперпользователя. В `dev/sql/init.sql` — для новых стендов; для поднятого — идемпотентная подготовка `common::ensure_fixtures()` от `postgres` в начале интеграционных тестов |
| Сервис `pg-lab` (`gp-pg-lab`, `postgres:16` — образ уже есть локально, `127.0.0.1:5470`): `wal_level=logical`, `max_prepared_transactions=10`, `max_replication_slots=10`, `pg_stat_statements` в `shared_preload_libraries`, `init.sql` | Логические слоты, подписки, prepared transactions — на остальных узлах они невозможны без рестарта. Init-скрипт `dev/lab/30-logical.sh`: в `orders` — публикация `lab_pub` и слот `lab_sub_slot` (`pgoutput`); БД `lab_sub` с копией `shop.customers` и подпиской `lab_sub` на себя через unix-сокет (`create_slot = false`, `copy_data = false`) |
| `scripts/wait-stand.sh` | Ждёт и `gp-pg-lab` |

### 12.2 Автотесты

- **`pg-advisor` (unit):** `locktree` — цепочка, несколько корней, мягкие блокирующие, цикл, DDL в очереди; `rates` — первый
  снимок, сброс статистики, перезапуск; `ash` — интервалы окон, `cpu`, параллельные воркеры, промежутки, топы; `failover` —
  фикстуры `/cluster` и `/config` Patroni 3.x и 4.x (обычный, `synchronous_mode`, `"quorum"`, `nofailover`,
  `failover_priority: 0`, лаг выше предела, pause); каждое правило §4 — на границах порогов; имена слотов участников Patroni.
- **Rust (unit):** SQL каждого инструмента для `server_version_num` 130000…180000 — снимки `tests/snapshots/diag-sql/`;
  `Caps` и `unavailable`; буфер сэмплера (лимит строк и времени, интернирование, промежутки, пропуск тика); подписка и
  отписка сэмплера, остановка с последним подписчиком; разрешение «за лидером» и смена узла; память счётчиков и номер открытия;
  разбор `scheduled_switchover` / `scheduled_restart`; `get_status` на поддельном HTTP. P2b: guard и матрица подтверждений
  §8.1; параметры и критерии SQL действий; классификация итогов; тела и адреса запросов Patroni и обход при отказе на
  поддельном HTTP; предусловия (failover при здоровом лидере, расписание в прошлом); `watch::progress` на последовательностях
  топологий; журнал — ротация, права 0600, обрезка текста, отсутствие учётных данных, `list_actions` с битой строкой.
- **Интеграционные** (`GPG_IT_*`; без переменной тест пропускается; `GPG_IT_LAB=localhost:5470` — новый; сценарии с
  остановкой узлов и переключениями — под `GPG_IT_DOCKER=1`, failover — ещё и под `GPG_IT_FAILOVER=1`; каждый сценарий
  возвращает стенд в исходное состояние при любом выходе):
  - **Блокировки** (`pg-single`, `glafira_app`): транзакция держит строку `shop.orders`; вторая сессия ждёт её; третья в
    транзакции выполняет `ALTER TABLE shop.orders ADD COLUMN it_tmp int` и встаёт в очередь (потом откат) → корень, DDL в
    очереди, жертвы, `ddlQueue` critical.
  - **Idle in transaction:** `BEGIN; SELECT 1` и простой → строка и `idleInTransaction` при пороге теста.
  - **Сэмплер:** `pg_sleep` и ожидание блокировки → `Timeout:PgSleep` и `Lock:transactionid` в профиле; после отписки новых
    запросов сэмплера нет.
  - **Реплики:** `pg_wal_replay_pause()` на `pg-replica` (от `postgres`) и запись на `pg-primary` → `replicaLag`, `replayPaused`;
    затем `pg_wal_replay_resume()`.
  - **Горизонт:** долгая транзакция с `txid_current()`; `PREPARE TRANSACTION` на `pg-lab`; логический слот на `pg-lab`;
    `ALTER SYSTEM SET hot_standby_feedback = on` и `pg_reload_conf()` на `pg-replica` → `standbyFeedback` на `pg-primary`
    (в конце — `ALTER SYSTEM RESET` и reload).
  - **Слоты:** мёртвый физический `it_dead_slot` на `pg-primary` и запись WAL → `retainedBytes` растёт; активный `replica1`;
    слоты участников на лидере Patroni помечены `member`; подписка `lab_sub` на `pg-lab`.
  - **Patroni:** матрица health (ровно один `/primary` = 200); готовность к failover (две готовые реплики); после `patronictl
    switchover` вкладка «за лидером» показывает новый узел ≤ 15 с.
  - **P2b:** завершение корневого блокирующего освобождает очередь; устаревший `backend_start` → `gone`; пакет
    idle in transaction пропускает проснувшуюся сессию (`noLongerMatches`); отмена `pg_sleep` → ошибка 57014 у клиента;
    удаление `it_dead_slot`; `replica1` не удаляется (`slotActiveOrGone`); switchover сейчас с наблюдением до «готово»;
    switchover по расписанию через час → `scheduledSwitchover` → отмена; restart реплики до «готово»; pause / resume;
    failover (`GPG_IT_FAILOVER=1`): pause → остановка PostgreSQL лидера → failover на реплику → resume → старый лидер
    возвращается репликой; read-only профиль → `READ_ONLY`; prod без имени → `CONFIRMATION_REQUIRED`; каждое действие —
    строка в `actions.jsonl`.
  - **Матрица** (`GPG_IT_MATRIX`): каждый SQL-набор §5 и §6 выполняется на PG 13–18 без ошибок; недоступные по версии части
    перечислены в `unavailable`.
- **Фронтенд** (Vitest + Testing Library, IPC замокан): шапка (селектор узла, интервал, «устарело», смена узла), блок находок,
  каждый инструмент на фикстурах отчётов, пресеты и группировка сессий, табличная альтернатива графика, дерево блокировок,
  матрица health, ссылки пульса; P2b — `ConfirmDialog` (перенос тестов Glafira Kafka), диалоги действий и матрица
  подтверждений, панель наблюдения по событиям канала, вкладка журнала.
- **Скрипты:** `i18n-check` — новые ключи, глава `diagnostics`, якоря и метки.

### 12.3 CI

Linux-job поднимает стенд вместе с `pg-lab` и профилем `matrix` и запускает интеграционные тесты с `GPG_IT_LAB`
(failover — с `GPG_IT_FAILOVER=1`); macOS и Windows — как в P1.

---

## 13. Документация

- Руководство: глава `diagnostics` (§7.5) — что показывает каждый инструмент, как читать находки, нагрузка от сэмплера;
  P2b — разделы о действиях, подтверждениях, наблюдении и журнале.
- `docs/admin.md`: права для диагностики и действий (`pg_monitor`, `pg_signal_backend`, `REPLICATION` и его риск, §8.7),
  учётная запись Patroni API для действий, нагрузка (запрос в 5 с на открытую вкладку, сэмплер — запрос в секунду на узел,
  пока открыт профиль ожиданий), файл `actions.jsonl` (где лежит, что содержит, текст запросов с литералами).
- README: сервис `pg-lab`, роль `glafira_app`, переменные `GPG_IT_LAB` и `GPG_IT_FAILOVER`.
- CHANGELOG: раздел `## Unreleased`.

---

## 14. Отступления от концепции

1. «DDL в очереди» — блок вкладки блокировок, а не отдельная вкладка: это частный случай дерева.
2. «Idle in transaction» — пресет вкладки сессий с отдельным пунктом в дереве.
3. Пульс — таблица «узлы × показатели», а не плитки одного узла: дежурный видит проблемный узел сразу.
4. «Patroni: топология» — существующий обзор P1 с запланированными действиями (P2a) и кнопками действий (P2b).
5. У сэмплера своё соединение на узел (`PoolPurpose::Sampler`); бюджет соединений — §3.4.
6. Деления Free / Pro нет (§1, «Лицензия»).

---

## 15. Критерии готовности

**P2a:**
1. Каждый инструмент открывается из дерева у сервера, списка узлов и кластера Patroni стенда; у не-Patroni нет инструментов Patroni.
2. Вкладка «за лидером» после switchover показывает новый лидер ≤ 15 с и пишет о смене узла.
3. Сценарии стенда (§12.2) дают ожидаемые данные и находки: блокировки с DDL в очереди, idle in transaction, лаг и пауза
   replay, держатели горизонта, мёртвый слот, подписка, матрица health, готовность к failover.
4. Профиль ожиданий показывает `Lock` и `PgSleep`; после закрытия вкладки запросы сэмплера прекращаются.
5. Каждый SQL-набор выполняется на PG 13–18; недоступное по версии объяснено.
6. Строки — en и ru, `pnpm i18n:check` зелёный; глава руководства открывается по F1; CI зелёный на трёх ОС.

**P2b:**
1. Read-only и prod защищены в Rust (тесты команд без интерфейса).
2. Завершение и отмена работают поштучно и пакетом; устаревший `backend_start` → `gone`; пакет перепроверяет критерий.
3. Мёртвый слот удаляется, активный — нет; слот Patroni в предпросмотре помечен.
4. Switchover (сейчас и по расписанию с отменой), restart, pause / resume и failover на стенде доходят до «готово».
5. Каждое выполненное действие оставляет строку в `actions.jsonl`; вкладка журнала её показывает.
6. Пункт чек-листа `replicationAttr` верен для `glafira_diag` и `postgres`; CI зелёный на трёх ОС.
