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
5. Действия §7 концепции, кроме VACUUM / ANALYZE и reload: отмена запроса, завершение сессий поштучно и пакетом (в интерфейсе
   пакет выбирается по критерию «idle in transaction старше N минут», а не флажками строк; находки `idleInTransaction` и
   `rootBlocker` ведут к действиям, §4), удаление слота, switchover (сейчас и по расписанию, отмена), failover, restart (сейчас
   и по расписанию, отмена), pause / resume.
6. Защита §7.1–7.2: read-only, подтверждение именем на prod, предпросмотр, защита от переиспользования PID (сессия — пара PID
   и время запуска) и перепроверка критерия пакета, наблюдение после действий Patroni, предпроверка прав; журнал действий
   `actions.jsonl` и его вкладка.
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
| P1 `topology/patroni.rs` | `PatroniHttp`, `ReqwestHttp`, разбор `/cluster` | `get_status(url)` (матрица health), `fetch_config(http, bases)` — `GET /config` по `get_json` с обходом адресов (§5.8); P2b — `send_json(method, url, body, timeout)` (метод трейта с реализацией по умолчанию «не поддерживается»), `HttpMethod`, `HttpReply`; изменяющие запросы идут вторым клиентом `reqwest` без перенаправлений (§8.5) |
| P1 `topology/runtime.rs` | `Runtimes`, `TopologyView`, `target(id, member)` | `diag_target(id, member, cluster)` и `topology::observe::leader_member` — узел инструмента (§3.2); подписчики закрытия подключения (§3.4); `LiveSource::http_for` публичен (клиент HTTP подключения для Patroni-инструментов); `boost(id, owner, interval, until) -> bool` и `unboost(id, owner)` — ускоренный опрос для наблюдения, по ускорению на наблюдение (§8.6) |
| P1 `conn/pool.rs` | `PoolPurpose`, `PoolRegistry` | `PoolPurpose::Sampler` (одно соединение на узел) |
| P1 `conn/pg.rs` | `ConnectSpec`, `SESSION_OPTIONS` | P2b: поле `ConnectSpec.options` (по умолчанию `SESSION_OPTIONS`), `ConnectSpec::for_action()`, `ACTION_SESSION_OPTIONS` — без `default_transaction_read_only` (§8.2) |
| P1 `commands/catalog.rs` | `on_catalog_node_within`, предел `catalog_limit` | Те же пределы и закрытие соединения при таймауте для запросов диагностики |
| P1 `error.rs`, `conn/errors.rs` | `AppError`, `classify_sqlstate` | Коды `RECOVERY_CONFLICT`; P2b — `READ_ONLY`, `CONFIRMATION_REQUIRED` (§10) |
| P1 `readiness`, `pg-advisor::readiness` | Чек-лист | P2b: пункт `replicationAttr`, факт `has_replication` |
| P1 `src/state/workspace.ts` | `TabSpec` | Виды `diag` и `actions` (§7.2) |
| P1 `src/lib/useConnectionEvents.ts` | `PG_QUERY_KEYS` | Ключ `diag` |
| Glafira Kafka `guard.rs` | `ensure_mutation_allowed`, `ensure_name_confirmed` | P2b: без изменений интерфейса |
| Glafira Kafka `common/ConfirmDialog.tsx` | Подтверждение внутри страницы, тип `Confirmation` | P2b: интерфейс прежний плюс необязательное `disabled` (форма над подтверждением не заполнена), тексты — свои ключи |
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
  actions/     (P2b) mod.rs (Confirmation) conn.rs sessions.rs slots.rs patroni.rs watch.rs journal.rs
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

1. `Runtimes::diag_target(id, member, cluster)` разрешает узел: инструмент кластера узла не берёт; названный `member` —
   этот участник; `null` — «за лидером» (`topology::observe::leader_member`): лидер Patroni (у standby-кластера — лидер
   этого кластера), **доступный primary** списка узлов (не узел по умолчанию каталога: при `targetServerType` он может быть
   репликой, а «за лидером» означает узел, принимающий запись) или единственный сервер; только доступный узел, иначе
   `errors.topology.noNode`. Подключение не открыто — `errors.topology.notConnected`. Узел вкладки хранится только в её
   спецификации (`TabSpec.member`, §7.2).
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
  unavailable: [{ part: String, member: Option<String>, reason: LocalizedText }]   // что не показано и почему
}
```

Инструменты кластера (`pulse`, `failover`, `health`) возвращают `member = null`, `serverVersionNum` и `inRecovery` — `null`;
ошибки отдельных узлов — внутри `data` (строка узла, клетка матрицы), а не ошибкой команды.

### 3.3 Версии PostgreSQL 13–18

`Caps::from(server_version_num)` даёт флаги возможностей; SQL собирается из фрагментов, и колонка, которой нет в версии,
выбирается как `NULL::<тип> AS <имя>` — форма строки одна на все версии, а `unavailable` объясняет пропуск
(`ltext!("diag.unavailable.version", "{{part}}: PostgreSQL {{version}}+")`). `part` — внутреннее имя части (`queryId`,
`slotInactiveSince`…); фронтенд подставляет вместо него подпись `diag.unavailable.parts.<part>` («Время простоя слота»).

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
| `Sampler` (новое) | 1 | Пока к сэмплеру узла подключена вкладка; закрывается через 5 мин простоя, как остальные пулы. Соединение — `AppState::client_with(id, member, database, PoolPurpose::Sampler)`; `AppState::client` — то же для `Catalog` |
| Действие (P2b, вне пулов) | 1 | На время одного действия или пакета (§8.2) |

`Runtimes` сообщает подписчикам о закрытии и переоткрытии подключения (`disconnect`, удаление, переоткрытие с другими
настройками, исчезновение участника): сэмплеры подключения останавливаются, их буферы и прошлые счётчики (§3.6) удаляются,
сессии наблюдения прерываются. Каждое такое состояние привязано к номеру открытия подключения (`Entry::generation`): ответ,
пришедший после переоткрытия, к новому открытию не применяется.

`AppState` хранит `live: Arc<LiveSource>` — тот же экземпляр источника топологии, что у `Runtimes`: инструменты Patroni берут
клиента HTTP подключения (`LiveSource::http_for`), а не строят второго. Логика длительных сессий вынесена из команд:
`sampler::start(st, id, member, sink)` (§9), команда Tauri только оборачивает `Channel` в `SamplerSink`.

### 3.5 Длительные сессии

`session.rs` — реестр `SessionId → { connectionId, kind: sampler | watch, остановка }` (как `SessionRegistry` Glafira Kafka, но
остановка — быстрое замыкание без `await`: у сэмплера это отписка вкладки, задача узла прерывается вместе с последним
подписчиком). `stop_session(sessionId)` останавливает сессию и возвращает `true`, если она была. Закрытие подключения
останавливает все его сессии; выход из приложения — все. Данные во фронтенд идут через `tauri::ipc::Channel`. Закрытая вкладка
снимает подписку явным `stop_session`. Ошибка отправки в канал тоже отписывает вкладку, но перезагрузку вебвью она не
обнаруживает: в Tauri 2.12 `Channel::send` после перезагрузки страницы не падает. Поэтому перезагрузка останавливает все сессии
сама (`on_page_load`, начало загрузки → `SessionRegistry::stop_all`; §6), а страница подписывается заново при монтировании.

### 3.6 Обновление, скорости и рост

- Вкладка опрашивает `run_diagnostic`, только пока видна: по умолчанию раз в 5 с (`health` и `failover` — раз в 10 с),
  в шапке — выбор 2 / 5 / 10 / 30 с или «Пауза» и кнопка «Обновить». Выбор не сохраняется между запусками. Вкладка, снова
  ставшая видимой, сразу перечитывает данные, устаревшие больше чем на интервал (если не «Пауза» и не отказ во входе).
- После `AUTH_FAILED` опрос останавливается (как у фактов узлов, спец. P1 §9) и при показе вкладки не возобновляется:
  повтор с неверным паролем блокирует учётную запись.
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
  target: Option<FindingTarget>,        // session { pid, backendStartUs } | slot { name } | member { name }
                                        // | relation { name } | database { name }
  params: Map<String, Json>,            // цифры для текста: длительность, байты, число жертв…
  links: [FindingLink],                 // openTool { tool, member } | action { action: SessionActionLink, member } (P2b)
}
```

- Тексты — в словарях по `id`: `findings.<id>.title` (с параметрами) и `findings.<id>.fix` («что сделать»), как пункты чек-листа.
  Готовые команды для копирования (`ROLLBACK PREPARED '…'`, `ALTER ROLE … SET idle_in_transaction_session_timeout`) — в `params.command`,
  не переводятся и приложением не выполняются.
- **Ссылки-действия (P2b).** `FindingLink::Action { action, member }`, где `SessionActionLink` (тег `type`) —
  `terminateIdleInTransaction { olderThanS }` (правило `idleInTransaction`; порог — порог предупреждения, 300 с) или
  `terminateRootBlocker { pid, backendStartUs }` (правило `rootBlocker`, только если у корня известно время запуска; иначе
  остаётся одна ссылка на инструмент). Ссылка на инструмент идёт первой, действие — вслед за ней; другие правила действий
  не предлагают. Кнопка открывает тот же предпросмотр (§8.3) с целью `idleInTransaction` / `rootBlocker`; её нет на read-only
  подключении и без известного узла (`member` ссылки или узел отчёта). JSON ссылки:
  `{"kind":"action","member":"pg-1","action":{"type":"terminateRootBlocker","pid":42,"backendStartUs":1761299000000000}}`.
- Пороги — константы `pg_advisor::findings::thresholds`; строгость от окружения подключения не зависит (P2).
- Одно правило может срабатывать в нескольких инструментах (например, `longTransaction` — в пульсе и в сессиях): id общий.
- **Одна находка на правило и узел** у правил по множеству объектов (`idleInTransaction`, `longQuery`, `longTransaction`,
  `slotRetainsWal`, `replicaLag`, `replicaNotStreaming` …): `target` — худший объект, `params.count` — сколько объектов
  достигли порога предупреждения, `params.maxS` / `maxBytes` — худшее значение. Иначе сотня висящих сессий дала бы сотню
  одинаковых находок. Исключения, где важен сам объект: `rootBlocker` и `ddlQueue` — по находке на корень и на элемент очереди.
- Тексты: заголовок `healthPrimary` нейтральный («адрес лидера отвечает 200 у N участников»), потому что 200 считается
  и на `/primary`, и на `/standby-leader`; «что сделать» у `replicaOutOfPool` называет причины, в том числе тег `noloadbalance`.
- Порядок в отчёте: строгость (critical → info), затем `member`, затем `id`.

| id | Инструменты | warning | critical |
|---|---|---|---|
| `connectionsSaturated` | pulse | занято ≥ 80 % доступных | ≥ 95 % |
| `idleInTransaction` | sessions, pulse | простой ≥ 5 мин | ≥ 30 мин |
| `longQuery` | sessions, pulse | активный запрос ≥ 15 мин | ≥ 1 ч |
| `longTransaction` | sessions, pulse | транзакция ≥ 1 ч | ≥ 6 ч |
| `xidAge` | pulse | `max(age(datfrozenxid))` ≥ 50 % от 2³¹ | ≥ 75 % |
| `lockWait` | pulse, locks | ожидание блокировки ≥ 30 с | ≥ 5 мин |
| `walDirSize` | pulse | `pg_wal` ≥ 2 × `max_wal_size` | ≥ 4 × |
| `deadlocks` | pulse | рост `deadlocks` за интервал | — |
| `lockShare` | waits | доля `Lock` в окне ≥ 20 % (при ≥ 30 сэмплах в окне) | ≥ 50 % |
| `rootBlocker` | locks | у корня жертва ждёт ≥ 10 с | ≥ 60 с или ≥ 10 жертв |
| `ddlQueue` | locks | — | есть DDL в очереди |
| `xminHorizon` | horizon | держатель старше 10 млн xid или 1 ч | 100 млн xid или 6 ч |
| `replicaLag` | replication, pulse | ≥ 16 МБ или ≥ 30 с | ≥ 1 ГБ или ≥ 5 мин |
| `replicaNotStreaming` | replication | `state` ≠ `streaming` и ≠ `backup` | — |
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
| `healthPrimary` | health | — | `/primary` или `/standby-leader` = 200 не ровно у одного из ответивших участников |
| `healthUnhealthy` | health | — | `/health` ≠ 200 у доступного API |
| `replicaOutOfPool` | health | реплика без `/replica` = 200 (вне пула чтения балансировщика) | — |

Info: `cacheHitLow` (cache hit за интервал < 90 % при ≥ 1000 прочитанных блоков), `tempFiles` (рост `temp_files`),
`noIdleTimeout` (`idle_in_transaction_session_timeout = 0` при сессиях idle in transaction ≥ 5 мин),
`hiddenQueries` (есть `<insufficient privilege>` — нужен `pg_monitor`), `lockCycle` (цикл ожиданий: deadlock detector
разрешит за `deadlock_timeout`), `recoveryConflictsTotal` (конфликты есть, но не растут).

---

## 5. Инструменты

### 5.1 Пульс (`pulse`, кластер)

Опрашивает все доступные участники параллельно; на каждый узел — запрос показателей и отдельный запрос размера `pg_wal`
(`pg_ls_waldir()`: без `pg_monitor` он падает с отказом в праве, а остальной пульс — нет), оба в пределе `catalog_limit`;
недоступный участник или ошибка — строка с ошибкой. Данные: `nodes: [PulseNode]`.

| Поле `PulseNode` | Источник |
|---|---|
| `member`, `role`, `reachable`, `error` | Топология; ошибка запроса |
| `connections { used, available, byState { active, idle, idleInTransaction, other } }` | `pg_stat_activity` (`backend_type = 'client backend'`); `available = max_connections − superuser_reserved_connections − reserved_connections (16+)` |
| `rates { tps, commitsPerS, rollbacksPerS, cacheHitPct, tempFilesPerS, tempBytesPerS, deadlocks, walBytesPerS }` | Дельты сумм `pg_stat_database` и LSN (`pg_current_wal_lsn()`, на реплике `pg_last_wal_receive_lsn()`), §3.6 |
| `longestTransactionS`, `longestQueryS`, `longestIdleInTransactionS` | `now() − xact_start` / `query_start` (активные) / `state_change` (idle in transaction), client backend, без собственной сессии сбора (`pid <> pg_backend_pid()`: её запрос всегда активен) |
| `lockWaiting`, `longestLockWaitS` | оба — только client backend: `wait_event_type = 'Lock'`; время — `pg_locks.waitstart` строк этих сессий (14+) или `state_change`; ожидания служебных процессов (autovacuum и др.) — в «Блокировках» |
| `maxXidAge`, `xidAgePct` | `max(age(datfrozenxid))` из `pg_database`, доля от 2³¹ |
| `walDirBytes`, `maxWalSize` | `sum(size) FROM pg_ls_waldir()` (`pg_monitor`; без права — `unavailable`), `max_wal_size` |
| `lagBytes`, `replayDelayMs` | Из топологии (запроса нет) |

Находки пульса — правила из таблицы §4 по каждой строке (`member` заполнен).

### 5.2 Сессии и idle in transaction (`sessions`, узел)

Данные: `rows: [SessionRow]`, `settings { idleInTransactionSessionTimeout, trackActivityQuerySize }`.

`SessionRow`: `pid`, `leaderPid`, `backendType`, `database`, `user`, `application`, `clientAddr`, `clientPort`,
`backendStartUs` (микросекунды эпохи — ключ защиты от переиспользования PID, §8.3), `xactAgeS`, `queryAgeS`, `stateAgeS`,
`state`, `waitEventType`, `waitEvent`, `backendXid`, `xminAge`, `queryId` (14+; в JSON — десятичная строка, в TS `string | null`: 64-битный хеш не помещается в точное число JS),
`query` (не длиннее 16 384 символов: `left(query, 16384)`, флаг `queryTruncated` — текст обрезан: при `track_activity_query_size` 1 МБ и тысяче сессий ответ иначе весил бы гигабайт),
`blockedBy: [pid]` (только у ждущих блокировку: `CASE WHEN wait_event_type = 'Lock' THEN pg_blocking_pids(pid) END`),
`locksHeld` (одним проходом `pg_locks WHERE granted GROUP BY pid`), `glafira: bool` (`application_name = 'glafira-pg'`).

Интерфейс (§7.4): пресеты «Все», «Активные», «Idle in transaction», «Ждущие»; фильтр по подстроке; группировка по приложению,
роли, клиенту, БД со счётчиками по состояниям; «Скрыть сессии Glafira» (включено по умолчанию). Пресет idle in transaction
сортирует по `stateAgeS`, добавляет колонки «простой», «держит блокировок» и «возраст xmin» и убирает одинаковые у всех строк
(состояние, ожидание, «ждёт», время запроса), чтобы таблица помещалась в окно; показывает значение
`idle_in_transaction_session_timeout` с подсказкой (рекомендация — задать на роль приложения).

### 5.3 Профиль ожиданий (`waits`, узел)

Читает буфер сэмплера узла (§6); если сэмплер этого узла не запущен вкладкой, отчёт содержит то, что накоплено, и
`samplerRunning = false`.

Данные: `windowS`, `bucketS` (5 с для окна 300, 10 с для 900, 40 с для 3600), `samplerRunning`, `truncated` (окно длиннее
накопленного), `buckets: [{ startMs, sampledS, aas: { cpu, lock, lwlock, io, ipc, client, timeout, bufferPin, extension, activity, other } }]`
(среднее число активных сессий = сэмплы в интервале / секунды, когда сэмплер работал), `gaps: [{ fromMs, toMs }]`,
`topEvents: [{ class, event, samples, share }]`, `topQueries: [{ key, queryId?, text, samples, share, topEvent }]`
(ключ — `query_id`, если он есть и не 0, иначе хэш текста; на PG 13 — всегда хэш; `queryId` в JSON — десятичная строка, как у сессий), `byApplication`, `byUser`, `byDatabase`,
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
waitS, queued: число сессий за ней, relation, relationOid, relationDatabase, mode }]`: `relation` — имя отношения только для БД
соединения, для отношения другой БД вместо него `relationOid` и `relationDatabase` (интерфейс собирает «oid N в БД X»);
находка `ddlQueue` несёт `relation` либо `relationOid` и `relationDatabase`. Находка рекомендует отменить DDL (быстро
снимает очередь) или блокирующий запрос и задавать `lock_timeout` в миграциях. Постоянная подпись вида: детали deadlock
есть только в логе сервера.

Данные: `roots: [LockNode { session, lock?, alsoBlockedBy: [pid], victims, children: [LockNode] }]`, где `lock` —
запрошенная блокировка `{ locktype, mode, relation, relationOid, relationDatabase, waitS, approx }` (на PG 13 `approx` — время
оценено по `state_change`), `cycles: [[pid]]`, `ddlQueue`. Ожидающий, чьих блокирующих нет в строках (pid 0 — подготовленная
транзакция, либо сессия уже завершилась), — корень своего поддерева: `lock` у него заполнен, `alsoBlockedBy` перечисляет
неразрешённые pid (0 — prepared transaction). `rootBlocker` для такого корня не выдаётся (завершать нечего, очередь учтена
в `lockWait`); `lockWait.count` — все ожидающие сессии леса.

### 5.5 Горизонт xmin (`horizon`, узел)

Держатели, отсортированные по `age(xmin)`:

| Вид | Источник | Длительность |
|---|---|---|
| `session` | `pg_stat_activity.backend_xmin`, `backend_xid` (client backend и прочие), кроме собственной сессии, отправителей WAL, autovacuum и ленивого `VACUUM` (ручного — по строке в `pg_stat_progress_vacuum`; `VACUUM FULL` — обычная транзакция и учитывается): его xmin горизонт других процессов не держит | `now() − xact_start` |
| `preparedXact` | `pg_prepared_xacts`: `transaction`, `gid`, `owner`, `database` | `now() − prepared` |
| `slot` | `pg_replication_slots.xmin`, `catalog_xmin`; слот связывается с `pg_stat_replication` по `active_pid` (`application`, `clientAddr`, `member`) | `inactive_since` (17+), иначе нет |
| `standbyFeedback` | `pg_stat_replication.backend_xmin` (на primary и каскадной реплике) | — |

Реплика с физическим слотом передаёт обратную связь через `pg_replication_slots.xmin` (держатель — `slot`); активный физический
слот с сопоставленным участником ссылается на него, как `standbyFeedback`: ссылка находки открывает горизонт на этом
участнике. **Сопоставление отправителя WAL с участником** (здесь и в §5.6): `application_name` равен имени участника или его
хосту, а если по имени не нашлось — адрес клиента (`host(client_addr)`) равен хосту участника (IPv6 в скобках сравнивается без них);
DNS не используется (опрос топологии разрешает имена, диагностика — нет); не сопоставилось — `member` пусто и ссылки нет. У
`preparedXact` в `params.command` — `ROLLBACK PREPARED '<gid>'` (строковая константа по правилам `quote_literal`). Данные:
`holders: [{ kind, name, member?, xminAge, durationS?, details }]`, `hotStandbyFeedback` (значение настройки на узле).

**Права** (§5.5–5.6). Сведения чужих сессий (`backend_xmin`, `backend_xid`, состояние приёмника WAL и отправителей, их LSN) видит роль с
`pg_read_all_stats` (входит в `pg_monitor`); проверка — `pg_has_role(current_user, 'pg_read_all_stats', 'USAGE')` (`USAGE`: членство
без наследования права не открывает). Без права горизонт добавляет в `unavailable` часть `otherSessionsXmin` с причиной «права»
(`diag.unavailable.privilege`): пустые поля нельзя читать как «горизонт свободен».

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

**Без `pg_read_all_stats`** (проверка — как в §5.5) данные получают `detailsHidden = true`: пустые поля отправителей и приёмника
не значат, что репликация сломана, поэтому по отправителям не выдаются `walReceiverDown`, `replicaNotStreaming`,
`syncStandbyMissing`, `replicaReplyStale` и `replicaLag`; в `unavailable` — часть `replicationDetails` с причиной «права»
(`diag.unavailable.privilege`); вид скрывает таблицу отправителей, оставляя их число, и показывает подпись о правах.

### 5.7 Слоты репликации и подписки (`slots`, узел)

`pg_replication_slots`: `name`, `slotType` (в JSON не `type`), `plugin`, `database`, `temporary`, `active`, `activePid`, `xminAge`, `catalogXminAge`,
`restartLsn`, `confirmedFlushLsn`, `walStatus`, `safeWalSize`, `retainedBytes` (текущий LSN − `restart_lsn`; на реплике
от `pg_last_wal_replay_lsn()`), `twoPhase` (14+), `conflicting` (16+), `inactiveSince`, `invalidationReason`, `failover`,
`synced` (17+). Настройка `max_slot_wal_keep_size`.

Синхронизированные слоты (`synced`, PostgreSQL 17+, на реплике) неактивны по устройству и не дают `slotRetainsWal`.

**Слоты Patroni** (только подключение `patroni`): слот участника — имя, полученное из имени участника по точному правилу Patroni
(`slot_name_from_member_name`): нижний регистр, `-` и `.` → `_`, прочие символы вне `[a-z0-9_]` → `u` и код символа из четырёх
цифр, обрезка до 63 символов; постоянный слот — ключ `slots` в `/config`. Отметка `patroni: member | permanent | null`.
`/config` недоступен — отмечены только слоты участников, причина в `unavailable` (текст — `diag.unavailable.patroniConfig`), сама
ошибка пишется в лог.

**Подписки:** `pg_stat_subscription` (`name`, `pid`, `receivedLsn`, `lastMsgAgeS`, `latestEndAgeS`; на PG 16+ только строки основного процесса, `leader_pid IS NULL`, без параллельных воркеров применения) и
`pg_stat_subscription_stats` (15+: `applyErrorCount`, `syncErrorCount`, их рост).

### 5.8 Patroni: готовность к failover (`failover`, кластер, только `patroni`)

Входы: топология (роль, состояние, доступность, лаг, `pending_restart`, теги `nofailover` и `failover_priority` (Patroni 4);
тег `nosync` не читается — «синхронный» определяется ролью участника из топологии: `sync_standby`, `quorum_standby`) и `GET /config` (`maximum_lag_on_failover`, `synchronous_mode` — `true` / `false` / `"quorum"`,
`synchronous_mode_strict`, `synchronous_node_count`, `ttl`, `loop_wait`, `retry_timeout`), `paused`. `/config` читается одним
запросом на отчёт (`fetch_config`) клиентом HTTP подключения (`LiveSource::http_for`), отдельного кэша нет; обход адресов API —
как у `/cluster`, общий срок тот же, что у `discover` (`DISCOVER_TIMEOUT`). Ключ, которого в `/config` нет, берётся по умолчанию
Patroni (`maximum_lag_on_failover` — 1 МиБ); не прочитался весь `/config` — ошибка отчёта.

`pg_advisor::failover::evaluate` выдаёт по каждому участнику, кроме лидера, вердикт `ready` и причины: `unreachable`,
`notStreaming` (состояние не `streaming` / `running`), `lagTooHigh`, `lagUnknown`, `noFailoverTag`, `zeroPriority`,
`notSynchronous` (при `synchronous_mode`: не `syncStandby`, а при `"quorum"` — не `quorumStandby`). `pendingRestart` —
пометка без запрета. По кластеру: число готовых, pause, проверка таймингов. Данные: `paused`, `config { … }`,
`candidates: [{ member, ready, reasons, lagBytes }]`.

### 5.9 Patroni: матрица health (`health`, кластер, только `patroni`)

Лидером считается участник, ответивший 200 на `/primary` **или** на `/standby-leader` (лидер standby-кластера). Для каждого участника с `apiUrl` параллельно — HTTP-коды `GET /primary`, `/standby-leader`, `/replica`, `/read-only`, `/sync`,
`/async`, `/health` (`PatroniHttp::get_status`, срок 5 с на запрос, без авторизации — эти адреса её не требуют). Данные:
`endpoints: [String]`, `members: [{ member, apiUrl, cells: [{ endpoint, status: Option<u16>, error: Option<AppError>, grade }] }]`.
Оценка клетки `grade` (`pg_advisor::rules::patroni::grade_member`) — по роли участника из топологии, а не по одному коду:
`ok` — ожидаемый 200; `notApplicable` — 503 на адресе чужой роли (`/primary` у реплики, `/replica` у лидера); `degraded` —
реплика вне пула балансировщика (503 на `/replica`, `/read-only`, `/async`) или `/sync` / `/async` разошлись с топологией;
`bad` — ответ противоречит роли, `/health` не 200, иной код или нет ответа. Цвет клетки: зелёный, серый, жёлтый, красный —
здоровый кластер зелёно-серый; под матрицей — легенда, смысл оценки — в подсказке клетки. Подпись объясняет: это то, что видит балансировщик (например, HAProxy с `option httpchk GET /primary`); параметры вида
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
  SELECT a.pid, a.leader_pid, a.backend_type, a.datname, a.usename, a.application_name, host(a.client_addr),
         a.wait_event_type, a.wait_event, <a.query_id | NULL::bigint>, left(a.query, 16384)
  FROM pg_stat_activity a
  WHERE a.state = 'active' AND a.pid <> pg_backend_pid()
    AND a.backend_type IN ('client backend', 'autovacuum worker', 'parallel worker')
  ```
  Фоновые процессы с постоянным ожиданием-циклом (walsender, checkpointer и т. п.) не сэмплируются: они искажали бы профиль.
  **Время сэмпла — часы приложения** (момент ответа на запрос): тики, промежутки и окно
  отчёта считаются одними часами, и пустой сэмпл (нет активных сессий) тоже получает время. Номер версии сервера сэмплер узнаёт
  первым запросом на своём соединении и запоминает.
- **Буфер** на узел: последние 60 мин, не больше 500 000 строк сэмплов (порядка 40 МБ); старое вытесняется, при пределе строк —
  тиками целиком (не обрывками тиков, иначе окно показало бы секунды работы сэмплера без их сэмплов). Строки (роль,
  приложение, БД, клиент, wait event, текст запроса) хранятся один раз в словаре и адресуются индексом.
- **Жизнь.** Сэмплер узла создаётся первой подпиской `start_sampler(id, member, channel)` и останавливается, когда отписалась
  последняя (`stop_session`, закрытый канал). Буфер живёт до закрытия подключения или исчезновения участника (§3.4); повторная
  подписка продолжает его, промежуток записывается в `gaps`; узел, оставшийся без подписчиков после отменённого запуска, забывается.
  Подписка с номером открытия подключения старше номера узла отклоняется (вкладке — `stopped`). Подписка держится, пока вкладка
  открыта, в том числе скрытая (история не прерывается при переключении вкладок); отчёт `run_diagnostic` опрашивается только
  видимой вкладкой (§3.6). Вкладка в режиме «за лидером» при смене лидера отписывается от сэмплера старого узла и подписывается
  на новый. Перезагрузка вебвью (`on_page_load`, начало загрузки) останавливает все длительные сессии; фронтенд подписывается
  заново при монтировании. Событие `stopped` причины не несёт: после него вкладка игнорирует события прежней подписки и, пока
  включена, через 2 с подписывается заново (исчезновение участника, отклонённая запоздавшая подписка; закрытие и повторное
  открытие подключения размонтируют тело вкладки, и оно подписывается при монтировании; «Переподключить» живого подключения
  `stopped` не шлёт). Исключение — отказ во входе (`error` с `AUTH_FAILED` перед `stopped`): автоматического повтора нет, пароль
  не перебирается. Остановку снимает действие пользователя: «Подключиться» и «Переподключить» (`refreshAfterConnect`) увеличивают
  номер подключения во фронтенде, и подписка вкладок этого подключения перезапускается — так же, как перечитываются отчёты.
- **Нагрузка и сбои.** Тик, на котором прошлый запрос ещё не вернулся, пропускается (запросы не накладываются); пропуск
  больше 2 с — промежуток. Получение соединения укладывается в `catalog_limit` (как у каталога), сам запрос — в 5 с
  (`on_catalog_node_split`); таймаут и обрыв закрывают соединение. Ошибка запроса — паузы 1, 2, 5, 10, 30 с и событие
  ошибки; `AUTH_FAILED` — сэмплер останавливается. Запрос идёт через туннель подключения, как остальные.
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
  из клетки пульса) обновляет её, как вкладку справки. Узел вкладки хранится только в её спецификации (`TabSpec.member`): выбор
  узла в шапке тоже переоткрывает вкладку через `useWorkspace.open` — один источник правды, и повторное открытие из ссылки
  находки или клетки пульса с любым узлом всегда действует. Журнал — `actions:<подключение>`.
- **Шапка** (общая): название и значок «?» с пояснением инструмента в 1–2 фразы (`hints.diag.<tool>`); селектор узла — «За лидером (pg-1)» и участники с ролями (у `server` и у инструментов кластера
  селектора нет); интервал обновления; у профиля ожиданий — окно 5 / 15 / 60 мин (по умолчанию 15); «Обновить»; время
  последнего ответа (только время дня по часам приложения, полное — в подсказке). Переход из находки в инструмент той же
  вкладки не показывается.
- **Состояния:** обновление не удалось — остаются прошлые данные, баннер ошибки и пометка «устарело»; в режиме «за лидером»
  узел сменился — строка «Узел сменился: pg-1 → pg-2»; подключение закрыто — «Подключение закрыто» и «Подключиться», опрос
  стоит; вкладка, снова ставшая видимой, перечитывает устаревшие данные, но не после отказа во входе (§3.6); инструмент
  Patroni у подключения без Patroni не предлагается.
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
  навыка dataviz при реализации), легенда, подсказка у столбца, промежутки штриховкой; легенда и столбцы табличной
  альтернативы для экранного диктора содержат только классы, встреченные в окне; окно (5 / 15 / 60 мин) выбирается в шапке;
  ниже — топ ожиданий, запросов и группировок; индикатор «сэмплер работает» по событиям `tick`.
- **Блокировки** — дерево с раскрытием уровней; сверху — карточки «DDL в очереди».
- **Горизонт, реплики, слоты, готовность к failover** — таблицы; у реплик на реплике — карточка приёмника.
- **Матрица health** — сетка «участники × адреса» с кодом и цветом.
- Длительности, байты и числа — функции `src/lib/format.ts`; длительность — старшая единица и соседняя («2 ч 5 мин»); все строки — через словари.

### 7.5 Справка и i18n

- Глава руководства `diagnostics` (`docs/guide/{en,ru}/diagnostics.md`, в `GUIDE_CHAPTERS` и `scripts/i18n-check.mjs`):
  `{#pulse}`, `{#sessions}`, `{#waits}`, `{#locks}`, `{#horizon}`, `{#replication}`, `{#slots}`, `{#failover}`, `{#health}`,
  `{#findings}`; P2b — `{#actions}`, `{#patroni-actions}`, `{#journal}`. F1 на вкладке инструмента открывает главу `diagnostics`
  с якорем инструмента: `helpChapterFor` даёт главу, `helpAnchorFor` — якорь (`idleTx` → `sessions`); глава, заданная экраном
  через `useHelpChapter`, главнее, и тогда якорь не передаётся; на самой справке F1 только возвращает её на экран. Пункт
  меню «Руководство» (⇧⌘/) делает то же, что F1 (`openContextHelp`).
- Ключи: `diag.tools.*`, `diag.*` (шапка, колонки), `findings.<id>.title|fix`, `actions.*` (P2b), ошибки — `errors.diag.*`,
  `errors.actions.*`. Все строки — en и ru; новые термины (горизонт xmin, wait event, idle in transaction, профиль ожиданий,
  корневой блокирующий) — в `docs/i18n/glossary.md`.

---

## 8. Действия (P2b)

### 8.1 Guard и подтверждения

`guard.rs` из Glafira Kafka: `ensure_mutation_allowed(profile, confirm)` — read-only → `READ_ONLY` (`errors.guard.readOnly`); prod без верного имени
подключения → `CONFIRMATION_REQUIRED` (`errors.guard.prodConfirm`); `ensure_name_confirmed(expected, typed)` →
`CONFIRMATION_REQUIRED` (`errors.guard.typeToConfirm`). Имена сравниваются после обрезки пробелов по краям, регистр значим.
Каждая команда выполнения принимает `confirmation { confirm: Option<String>, typedName: Option<String> }` (`actions::Confirmation`;
`null` и отсутствующее поле равны) и проверяет его в Rust до любого обращения к серверу: первой идёт `ensure_mutation_allowed`,
затем проверки самого действия.

| Действие | Всегда | На prod ещё |
|---|---|---|
| Отменить запрос, завершить сессию (одну) | Предпросмотр и кнопка | Имя подключения |
| Завершить пакетом | Предпросмотр списка | Имя подключения и число сессий (`typedName`) |
| Удалить слот | Ввод имени слота | Имя подключения |
| Switchover, отмена расписания, restart, отмена расписания, pause / resume | Предпросмотр | Имя подключения |
| Failover | Ввод имени кандидата | Имя подключения |

Пакет — несколько сессий или любой выбор по критерию `idleInTransaction` (даже если подошла одна сессия); число для
подтверждения — число сессий без повторов пары `(pid, backendStartUs)`.

На read-only подключении кнопок действий нет (`canAct`); команда всё равно отказывает. Кнопка, на которую нет права или условия,
неактивна, причина — в подсказке (§8.7). Кнопки находок показываются, только если известен узел (у ссылки или у отчёта).
Действия всегда адресуются узлу по имени: предпросмотр возвращает разрешённый узел, выполнение принимает именно его.

Подтверждение — `ConfirmDialog` внутри страницы: имя объекта (`objectName`) вводится всегда, когда оно задано (слот, кандидат
failover, число сессий пакета на prod), имя подключения — на prod; необязательное `disabled` не даёт подтвердить, пока не
заполнена форма над ним (время расписания). Кнопка отказа в подтверждении — «Закрыть», а не «Отмена»: рядом с
«Отменить запрос» «Отмена» читалась как то же действие (смоук P2b). Время расписания в окне Patroni сразу заполняется
моментом через 15 минут с округлением вверх до 5 минут (`defaultScheduleLocal`); прошедшее время не отправляется, а меньше
минуты до срока — предупреждение (отменить, скорее всего, не успеть). Диалог действия один на вкладку (`DiagTab` — сессии и слоты, `Overview` — Patroni):
новое открытие вытесняет прежнее. Защита от вытеснения диалога, чья команда ещё выполняется, есть у удаления слота
(`DiagTab`) и у действий Patroni (`Overview`): `useIsMutating` с областью вкладки, пока команда идёт, другой диалог не
открывается, а кнопки слотов или обзора неактивны. У действий над сессиями защиты нет: новое действие над сессиями (или
удаление слота) вытесняет их диалог и во время выполнения; команда при этом завершается, запись в журнале пишется,
отчёты и журнал перечитываются по её успеху, но итог в диалоге пользователь уже не увидит. Узел диалога фиксируется при открытии. Предпросмотр читается один раз на открытие (`staleTime: Infinity`, новый
экземпляр диалога — новый предпросмотр) и не обновляется под рукой у пользователя. Escape закрывает диалог, пока команда не
выполняется; закрытие подключения закрывает диалоги. После выполнения диалог перечитывает отчёты
(`["diag", id]` или `["topology", id]`) и журнал (`["actions", id]`).

### 8.2 Соединение для действия

Отдельное соединение вне пулов: `actions::conn::open(st, id, member, database) -> ActionClient`. `ConnectSpec` получает поле
`options` (по умолчанию `SESSION_OPTIONS`); `ConnectSpec::for_action()` подставляет `ACTION_SESSION_OPTIONS =
-c statement_timeout=5000 -c lock_timeout=1000 -c search_path=pg_catalog -c DateStyle=ISO` (без `default_transaction_read_only`;
константа — в `conn/pg.rs` рядом с `SESSION_OPTIONS`). `application_name = glafira-pg`, соединение идёт через туннель подключения
(`TunnelRegistry::route_for`), БД — `pg.database` (у логического слота — его БД). Открывается на одно действие или пакет
и закрывается при `Drop` (прерывается задача соединения, без ожидания Terminate). Если пул `Catalog` узла (БД профиля) запомнил
отказ во входе (`auth_failure`), действие возвращает эту ошибку без новой попытки входа. Узел, отмеченный недоступным, —
`NODE_UNAVAILABLE` (`errors.actions.nodeUnavailable`); участник, которого нет в топологии, — ошибка разрешения узла; сбой TCP
у узла, ещё не отмеченного недоступным, — обычная ошибка соединения. Предпросмотры ничего не меняют и читают соединением
пула `Catalog` (`diag::on_member`).

### 8.3 Отмена запроса и завершение сессий

- `preview_session_action(id, { member, kind: cancel | terminate, target })`, где `target`:
  `selected { sessions: [{ pid, backendStartUs }] }` | `idleInTransaction { olderThanS }` | `rootBlocker { pid, backendStartUs }`.
  `olderThanS` — конечное неотрицательное число секунд (иначе `errors.actions.badThreshold` и в предпросмотре, и при выполнении);
  интерфейс вводит целое число минут не меньше 1. В интерфейсе пакет выбирается только по критерию `idleInTransaction`
  (панель набора «Idle in transaction» и кнопка находки); поштучно — из панели сессии (`selected` с одной сессией, клиентская
  сессия с известным временем запуска) и из корня дерева блокировок (`rootBlocker`). API принимает `selected` до 500 сессий, но
  множественного выбора строк в таблице нет.
  Заново читает сессии и возвращает `{ member, rows: [{ session, allowed, reason }] }`; `session` — `pid`, `backendStartUs`,
  `user`, `database`, `application`, `client`, `state`, `backendType`, `xactAgeS`, `stateAgeS`, `query` (первые 500 символов);
  `reason` — `null` у разрешённой строки, иначе `notClientBackend`, `ownConnection`, `superuserSession` (нужна роль
  суперпользователя), `otherRole` (нет `pg_signal_backend` и роль чужая), а у отмены ещё `notRunning`: сессия не выполняет
  запрос (`state` не `active` и не `fastpath function call`), и `pg_cancel_backend` сервер у неё молча игнорирует — транзакция
  остаётся; права проверяются раньше состояния. Кнопка отмены в панели сессии в этом случае неактивна с той же причиной. Разрешено, если текущая роль — суперпользователь,
  или роль сессии — та же (`pg_has_role(current_user, usesysid, 'MEMBER')`), или сессия не суперпользователя и роль — член
  `pg_signal_backend`. Чужую сессию без права её видеть (нет `pg_read_all_stats`) сервер отдаёт с пустым `backend_start`:
  адресовать её парой нельзя, в предпросмотр она не попадает. `ownConnection` — только соединение самого предпросмотра;
  остальные соединения приложения (`application_name = glafira-pg`) разрешены, и интерфейс помечает их примечанием
  «Соединение Glafira» (имя приложения дублирует `PG_APPLICATION_NAME`), чтобы приложение случайно не завершило само себя.
  Пустой набор — «Нет подходящих сессий», набор без разрешённых строк — «Ни одну сессию нельзя отменить или завершить»
  (для одной сессии — «Эту сессию нельзя завершить» / «Запрос этой сессии нельзя отменить»); в обоих случаях только «Закрыть».
- `execute_session_action(id, { member, kind, target, sessions }, confirmation)`. Порядок проверок после guard: пары сессий
  без повторов; `badThreshold`; `errors.actions.noSessions` (пусто); для `rootBlocker` сессии должны быть ровно парой цели
  (`errors.actions.badTarget`), для `selected` — только пары из выбора цели (тоже `badTarget`); пакет на prod — число сессий в
  `typedName`; больше 500 — `errors.actions.batchTooLarge`.
  Сессии идут по одной, на одном соединении, общий предел 60 с (`BATCH_LIMIT`): оставшиеся после него получают
  `failed { errors.actions.batchTimeout }`. У каждого запроса к серверу клиентский срок 10 с (`STATEMENT_TIMEOUT`): истёк —
  `failed { errors.actions.statementTimeout }`, и пакет останавливается, оставшиеся — `failed { errors.actions.batchStopped }`.
  Если истёк срок самого запроса-сигнала, сигнал мог дойти — исход неизвестен (снимка сессии нет); повтор по той же паре
  (`pid`, `backend_start`) безопасен.
  Подключение, закрытое или открытое заново между сессиями, останавливает пакет: оставшиеся — `failed { errors.topology.notConnected }`.
  Перед каждой сессией проверяется сначала подключение, затем `BATCH_LIMIT`, затем остановка после срока. Одна сессия — один запрос:
  ```sql
  SELECT a.usename, a.datname, a.application_name, host(a.client_addr), a.state, <query_id>, left(a.query, 500),
         extract(epoch FROM now() - a.xact_start), pg_terminate_backend(a.pid)      -- или pg_cancel_backend
  FROM pg_stat_activity a
  WHERE a.pid = $1 AND <BACKEND_START_US> = $2
    AND a.backend_type = 'client backend' AND a.pid <> pg_backend_pid()
    AND <критерий>
  ```
  Критерий проверяется заново в том же запросе: `idleInTransaction` — `state IN ('idle in transaction', 'idle in transaction (aborted)')
  AND state_change <= now() - make_interval(secs => $3)`; `rootBlocker` — сессия всё ещё есть в `pg_blocking_pids` какой-либо
  ждущей блокировку; `selected` — без условия. Отмена к критерию добавляет `state IN ('active', 'fastpath function call')`:
  запрос, закончившийся после предпросмотра, даёт `noLongerMatches`, а не пустой `signalSent`. Сессия, которая успела проснуться или освободить блокировки, не пострадает.
  Исключается только соединение самого действия (`pid <> pg_backend_pid()`).
- Итог по каждой сессии (`results: [{ pid, backendStartUs, result, error? }]`, тег `result` плоский):
  `terminated` (после terminate pid с тем же `backend_start` исчез за 5 с, опрос раз в 250 мс),
  `signalSent` (отмена доставлена, или pid ещё жив через 5 с после terminate), `gone` (строки нет и пары уже нет, либо функция
  сигнала вернула `false` — процесса нет), `noLongerMatches` (строки нет, пара ещё есть: критерий не выполняется),
  `notAllowed` (42501), `failed { error }` (прочая ошибка сервера — следующие сессии продолжаются; истёкший срок и
  остановка пакета — выше).
  Итог действия (`outcome`): `error`, если все сессии `notAllowed` или `failed`; `ok`, если таких нет; иначе `partial`
  (`gone` и `noLongerMatches` неудачей не считаются). Ответ — `{ member, results, outcome, journalWritten }`.
  Ошибка до первой сессии (соединение, чтение версии, в том числе истёкший срок 10 с) — ошибка команды, строки журнала нет. `backend_start` передаётся
  микросекундами эпохи (`int8`), значение помещается в число JavaScript. Во всех запросах (сессии, блокировки, предпросмотр,
  выполнение) оно вычисляется одним SQL-фрагментом `diag::sql::BACKEND_START_US`: на PG 13 `extract(epoch …)` возвращает
  `double precision`, и только одинаковое выражение на одном сервере гарантирует точное равенство (на 14+ `extract` возвращает
  `numeric` и точен сам по себе).

### 8.4 Удаление слота

- `preview_drop_slot(id, { member, slot }) -> { member, slot }`: `slot` — полная строка слота инструмента
  «Слоты» (§5.7: тип, plugin, БД, `active`, удерживаемый WAL, `wal_status`, `inactive_since` (17+)) с отметкой Patroni
  (`patroni: member | permanent | null`, отметки по `/config`; не прочитался — только слоты участников). Слота на узле нет —
  `errors.actions.slotNotFound` (`NOT_FOUND`). Слот участника Patroni — предупреждение «Patroni пересоздаст слот, пока участник
  есть в кластере; удалите участника»; постоянный слот — «уберите слот из `/config` (`patronictl edit-config`)». Кнопка при
  этом доступна.
- Инструмент «Слоты» читает право: `SlotsData.canDropSlots` = `rolsuper OR rolreplication` у `current_user` (запрос в
  `diag::slots::collect`; сбой запроса отчёт не роняет, право считается отсутствующим). Кнопка «Удалить…» неактивна у
  активного слота («Слот активен»), без права (`needReplication`: подсказка называет пункт чек-листа) и пока слот удаляется в
  диалоге этой вкладки; причина в подсказке названа в таком порядке. Диалог читает слот заново: если он ожил, пока
  пользователь шёл к окну, — «Слот активен — удалить можно только неактивный слот» и только «Закрыть», без подтверждения.
- `drop_slot(id, { member, slot }, confirmation) -> { journalWritten }`, `typedName` = имя слота (всегда, не только на prod).
  После guard команда читает предпросмотр заново (его ошибки — ошибки команды), открывает соединение действия (у логического
  слота — с БД слота) и выполняет
  `SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = $1 AND NOT active`.
  Ноль строк или 55006 (`object_in_use`: слот ожил между фильтром и удалением) — `errors.actions.slotActiveOrGone`
  («Слот стал активным или исчез», `INVALID_INPUT`), ничего не произошло, журнал не пишется. 42501 — `NOT_AUTHORIZED`
  `errors.actions.replicationRequired`; прочая ошибка сервера (например, синхронизированный с primary слот на реплике 17+) —
  как есть в `details`; обе записываются в журнал с итогом `error`.

### 8.5 Действия Patroni

Только подключение `patroni` (иначе `errors.actions.patroniOnly`, `INVALID_INPUT`). Basic auth профиля (логин `patroni.username`
и секрет `patroniPassword`); без логина кнопки неактивны, а команда отказывает `errors.patroni.authRequired`
(`PATRONI_ERROR`; предпросмотр сообщает `canSend`). Правило P1 о неотправке пароля на `http://`-адрес при https-профиле
действует и здесь. Порядок в `execute`: guard → ввод имени кандидата failover → тип подключения → логин → предусловия на
топологии последнего опроса (`diag_target(…).topology`, не старше интервала опроса: 10 с, при наблюдении 2 с) → готовность
названного кандидата (failover; switchover с кандидатом) → запрос → журнал. Отказы до запроса в журнал не пишутся.

| Действие | Запрос | Предусловие (`actions::patroni::check`) |
|---|---|---|
| Switchover | `POST /switchover {leader, candidate?, scheduled_at?}` на API лидера; `leader` — из действия (лидер предпросмотра) | Есть доступный лидер в состоянии `running` (`errors.actions.noLeader`), и это лидер предпросмотра `action.leader` (иначе `errors.actions.leaderChanged` с `expected` и `current`); кандидат — участник и не лидер (`badCandidate`) и готовая реплика (`candidateNotReady`); время — RFC 3339 (`badSchedule`) и в будущем (`pastSchedule`) |
| Отмена switchover | `DELETE /switchover` | `scheduledSwitchover` есть (`nothingScheduled`) |
| Failover | `POST /failover {candidate}` | Нет доступного лидера в состоянии `running` (иначе `errors.actions.leaderHealthy`: «Лидер здоров — используйте switchover»); кандидат — участник и не лидер (`badCandidate`) и готовая реплика (`errors.actions.candidateNotReady`) |
| Restart | `POST /restart {restart_pending?, schedule?}` на API участника | Участник доступен (`NODE_UNAVAILABLE`); время — в будущем |
| Отмена restart | `DELETE /restart` на API участника | У участника есть `scheduledRestart` (`nothingScheduled`) |
| Pause / resume | `PATCH /config {"pause": true}` / `{"pause": null}` (`null` удаляет ключ из динамической конфигурации) на API лидера | Текущее `paused` другое (`pauseUnchanged`) |

**Лидер switchover.** Действие несёт лидера, которого показал предпросмотр (`switchover { leader, … }`): если лидер сменился
(например, автоматический failover, пока открыт диалог), `check` отказывает `leaderChanged`, а не переключает с лидера, которого
пользователь не видел. Тот же `leader` уходит в тело запроса, поэтому оставшееся окно (до одного опроса) закрывает отказ самого
Patroni (412 «leader name does not match» → `actionRejected`). У запланированного switchover Patroni сверяет записанного лидера
и в назначенное время.

**Готовность кандидата.** После `check` Rust считает вердикт готовности (§5.8) на той же топологии — `evaluate(candidate_facts,
/config, paused)`, `/config` не прочитался — пределы Patroni по умолчанию, как в предпросмотре — и названный кандидат должен
быть `ready`: доступен, получает WAL, лаг известен и не выше `maximum_lag_on_failover`, без `nofailover` и
`failover_priority: 0`, синхронный в синхронном режиме. Иначе — `errors.actions.candidateNotReady` (`INVALID_INPUT`,
`params.member`; причины — `params.reasons` (коды через запятую), лаг — `params.lagBytes` (байты строкой, только если
известен); интерфейс переводит коды теми же ключами, что вкладка готовности); кандидата нет в вердикте — тот же отказ
с одним `params.member`. Решает чистая функция
`ensure_candidate_ready(candidates, name)`. Это главное для failover: к кандидату, названному явно, Patroni предел лага не
применяет, а вердикт диалога мог устареть до подтверждения. Проверяется и switchover с кандидатом (интерфейс и так не даёт
выбрать неготового); «Patroni выберет сам» — нет: кандидата выбирает Patroni по своим правилам.

Время расписания вводится в локальном часовом поясе и уходит в RFC 3339 со смещением (`toRfc3339`); Rust разбирает его
`time::OffsetDateTime::parse(…, &Rfc3339)` и сравнивает с часами приложения. `restart_pending: true` отправляется при флажке
«только если нужен перезапуск».

**Адреса и повторы.** Запрос идёт на адреса `bases`: у restart и его отмены — только API самого участника; иначе — доступные
участники (лидер первым), затем недоступные, затем адреса профиля, без повторов (при failover лидер как раз не отвечает, и
запрос должен сразу уйти к живой реплике). К следующему адресу переходят **только** когда запрос заведомо не дошёл до Patroni
(`not_delivered`: `errors.patroni.unreachable`, `tls`, `errors.ssh.forwardFailed`, `tunnelDown`). Это решает
`ReqwestHttp::send_json` по фазе сбоя (`reqwest::Error::is_connect()`): отказ соединения и срок соединения
(`HTTP_CONNECT_TIMEOUT` = 3 с) — «не дошёл»; срок после установки соединения — `errors.patroni.timeout`; любой иной сбой отправки
и любой сбой чтения тела — `errors.patroni.connectionLost`; в обоих случаях исход неизвестен и запрос **не повторяется**
(повтор мог бы выполнить действие дважды или дать ложный отказ). Через SSH-туннель перед изменяющим запросом — пробное
чтение `GET /patroni` того же адреса клиентом чтения (старый `ssh -D` отвечает успехом SOCKS до открытия канала к узлу, и
недоступный узел обрывает поток уже после отправки — для изменяющего запроса это неотличимо от обрыва после доставки):
сбой проброса (`forwardFailed`, `tunnelDown`), срок соединения или истёкший срок самой пробы (`HTTP_TIMEOUT`; оба —
`unreachable`: живой узел отвечает на `/patroni` за миллисекунды) — запрос не отправляется, адрес считается
недоставленным; любой ответ (как и ошибка TLS) — запрос отправляется. Ошибка пробы называет адрес действия, причина пробы —
в `details` с пометкой `probe GET /patroni: ` в начале (учётных данных из адреса там нет). Без туннеля пробного чтения нет. Неверный адрес отсекается до сети (`errors.patroni.badUrl`,
`INVALID_INPUT`). Действия не следуют HTTP-перенаправлениям: у `ReqwestHttp` второй клиент (`Policy::none()`), и `3xx` —
отказ (`actionRejected`); чтение перенаправления по-прежнему выполняет. Этот клиент не держит простаивающих соединений
(`pool_max_idle_per_host(0)`): соединение из пула, уже закрытое узлом или промежуточным звеном, оборвалось бы после отправки —
ложный неизвестный исход вместо отказа соединения. У чтения пул прежний.

**Сроки.** Запрос — `HTTP_TIMEOUT` (5 с); switchover «сейчас», failover и restart «сейчас» — `LONG_ACTION_TIMEOUT` (60 с):
Patroni отвечает после выполнения. Срок соединения на чтении (3 с) даёт `errors.patroni.unreachable`, а не `timeout`.
Срок передаётся в `PatroniHttp::send_json(method, url, body, timeout)` (метод трейта с
реализацией по умолчанию «не поддерживается», `errors.patroni.sendUnsupported`: подделки чтения в тестах не меняются).

**Ответ.** 2xx — успех (текст ответа показывается); 401 и 403 — `AUTH_FAILED` `errors.patroni.unauthorized` (или
`credentialsWithheld`, если пароль не отправлен на `http://`-адрес), те же ключи, что у чтения; `503` на `POST /switchover` и
`/failover` с «status unknown» в теле — `errors.patroni.statusUnknown` (Patroni сам не дождался смены лидера: исход
неизвестен); иначе `PATRONI_ERROR` `errors.patroni.actionRejected` (`params.status`) и первые 500 символов ответа в `details`.

**Неизвестный исход** — `timeout`, `connectionLost`, `statusUnknown`. У действия с наблюдением (§8.6) команда возвращает не
ошибку, а `Ok` с наблюдением, `outcomeUnknown: true` и причиной в `error`: выполнилось ли действие, покажет наблюдение
(switchover, failover и restart «сейчас» Patroni выполняет до ответа). У отмен и запланированных действий ошибка возвращается
как есть.

- `preview_patroni_action(id, action)` → `{ leader, leaderState, leaderReachable, paused, candidates, leaderClientSessions, member,
  scheduledSwitchover, canSend }`. Предпросмотр берёт топологию последнего опроса (как и предусловия `execute`, см. выше); заново
  читаются только `/config` и число сессий лидера. Для switchover и failover: `candidates` — вердикты готовности (§5.8; `/config` не прочитался —
  пределы Patroni по умолчанию; неготовые — с причиной, у switchover есть вариант «Patroni выберет сам») и
  `leaderClientSessions` — клиентские сессии лидера, которые оборвутся, без своей и без `glafira-pg`, но с собственным
  соединением Patroni (оно клиентское: `backend_type = 'client backend'`); `null`, если лидер недоступен или запрос не
  удался (недоступный узел предпросмотр не ждёт). Предупреждения окна — как построено, не правило проекта: число сессий и
  «запись недоступна несколько секунд» — у failover и switchover «сейчас» (число — когда лидер есть), «старый лидер станет
  репликой» — когда лидер есть, у switchover по расписанию это единственное предупреждение. Для restart и его отмены `member` — роль, состояние,
  `pendingRestart`, `scheduledRestart` (restart лидера — предупреждение: запись недоступна до конца перезапуска; флажок «только
  если нужен перезапуск» включён, когда `pendingRestart`). Предусловия в предпросмотре не проверяются, но интерфейс повторяет
  главные (`running_leader`: доступен и `running`): failover при работающем лидере и switchover без него показывают
  `leaderHealthy` / `noLeader` и только «Закрыть», а кнопки обзора неактивны с тем же текстом; решает Rust. Действие,
  которое уходит из диалога, несёт лидера из предпросмотра (`leader`), неготовые кандидаты недоступны. На паузе
  (`paused`) Patroni переключает только на названного кандидата: «Patroni выберет сам» недоступно с причиной
  (`actions.patroni.candidateAutoPaused`), по умолчанию выбран первый готовый, нет готовых — «Нет реплики, готовой стать
  лидером». Failover при лидере, которого Patroni видит `running`, но приложение не достаёт (`leaderState: running`,
  `leaderReachable: false`), разрешён, и диалог прямо говорит, что failover снимет с этого лидера роль
  (`actions.patroni.failoverDemotes`); подтверждение failover всегда опасное.
- Кнопки обзора неактивны с причиной в подсказке, приоритет: подключение не открыто (`notLive`) → нет логина API
  (`authRequired`) → действие уже идёт (`inFlight`); restart недоступного участника неактивен. Отмена switchover — у баннера
  запланированного switchover, отмена restart — в строке участника.

### 8.6 Наблюдение после действия

`execute_patroni_action(id, action, confirmation, channel)` после успешного (или неизвестного по исходу) switchover / failover /
restart «сейчас» и pause / resume открывает сессию `watch` (§3.5, `SessionKind::Watch`) и возвращает её id
(`PatroniActionResult.watch`). Запланированные действия и отмены не наблюдаются: обзор показывает баннер (§5.10).
`actions::watch::start(st, id, plan, sink)` сразу посылает первое событие (`accepted`, `running`) и ставит ускорение:
`Runtimes::boost(id, owner, 2 с, срок) -> bool` (`owner` — id сессии наблюдения). Опрос топологии идёт раз в 2 с вместо 10
(события журнала топологии пишутся как обычно); после каждого опроса шаги вычисляет чистая функция
`actions::watch::progress(plan, topology, postmaster_start_now)` — последний шаг, до которого дошло действие (первый
невыполненный останавливает), — а событие уходит только когда шаг вырос.

Ускорения лежат в отдельном мьютексе `Runtimes.boosts` (последний в порядке блокировок), по одному на наблюдение: конец одного
не замедляет другое, опрос ждёт самый короткий из живых интервалов, а `Notify` (`Entry.wake`) будит фоновый опрос сразу.
`unboost(id, owner)` берёт только `boosts`, не `entries`: его вызывает и остановка сессии из слушателя закрытия, который
работает под `entries`. `boost` возвращает `false` для неоткрытого подключения, а `SessionRegistry::contains` закрывает
гонку «закрыто и переоткрыто между вставкой сессии и ускорением».

| Действие | Шаги | Срок |
|---|---|---|
| Switchover, failover | принят → старый лидер сложил полномочия (не лидер или недоступен) → новый лидер `running` (не прежний; кандидат, если задан) → таймлайн вырос → все доступные реплики в `running` / `streaming` → готово | 120 с |
| Restart | принят → перезапущен → узел `running` / `streaming` → `pending_restart` снят (если «только pending») → готово | 300 с |
| Pause / resume | принят → `paused` в топологии = заданному → готово | 30 с |

«Перезапущен» определяется по `postmaster_start_time` из `GET <API участника>/patroni`: значение до действия запоминается при
выполнении, наблюдение ждёт другого. Если до действия его прочитать не удалось, одного значения мало (часы сервера и
приложения могут расходиться): нужен простой участника, который наблюдение видело (остановлен, недоступен, пропал из
топологии) либо ответ 2xx Patroni на restart «сейчас», и разборчивое время запуска позже отправки запроса. Реплика в состоянии,
отличном от `running` / `streaming`, не даёт дойти до шагов «реплики получают WAL» и «узел работает». Если лидером стал не
выбранный кандидат, наблюдение остаётся на шаге «старый лидер сложил полномочия» до срока.

Канал `WatchEvent { step, status: running | done | failed | timedOut, atMs, detail: Option<AppError> }`. Срок истёк —
`timedOut` с последним достигнутым шагом. `failed` с `errors.topology.notConnected` приходит, когда подключение не открыто,
закрыто за время старта или закрыто либо открыто заново во время наблюдения; остановка панелью (`stop_session`) события не
даёт. Задача наблюдения прерывается сигналом отмены, а не `abort`, чтобы послать финальное событие; финальное событие у
наблюдения одно. Закрытие панели наблюдения, как и вкладки обзора
(`stop_session`), прекращает наблюдение и ускоренный опрос; закрытие подключения останавливает все его сессии. Панель
наблюдения останавливает свою сессию при «Закрыть» и при размонтировании (микрозадачей, безопасно для двойного монтирования
StrictMode); если диалог убран, пока команда ещё шла, наблюдение останавливает обработчик успеха мутации. Ход панель объявляет
одной вежливой областью `role="status"`: пока идёт — последний пройденный шаг, в конце — «Готово», срок вышел или причина
сбоя (последние два видны и глазами).

### 8.7 Предпроверка прав

| Действие | Право | Без него |
|---|---|---|
| Отмена, завершение | `pg_signal_backend`, суперпользователь или своя роль | Строка предпросмотра неактивна, причина в ней |
| Удаление слота | Атрибут `REPLICATION` или суперпользователь (`SlotsData.canDropSlots`) | Кнопка неактивна; подсказка — пункт чек-листа |
| Действия Patroni | Логин API в профиле (`canSend`) | Кнопки неактивны |

Новый пункт чек-листа готовности `replicationAttr` (`warn`, факт `has_replication` из `pg_roles.rolreplication`):
«Удаление слотов репликации» — `ALTER ROLE "<user>" REPLICATION;` с предупреждением, что атрибут позволяет роли создавать
логические слоты и читать через них изменения всех таблиц БД, а при разрешающей строке `replication` в `pg_hba.conf` —
копировать кластер; `docs/admin.md` советует выдавать его отдельной роли для действий или не выдавать вовсе.

### 8.8 Журнал действий

`actions.jsonl` в каталоге данных (`AppState.actions: Arc<ActionJournal>`, там же, где `connections.json`), права 0600 (на Unix;
на Windows — права каталога профиля), при 5 МБ ротируется в `actions.1.jsonl` (логика — копия журнала MCP Glafira Kafka; прежний
`actions.1.jsonl` заменяется). Запись — после выполнения, best effort: сбой записи не откатывает действие, ответ команды несёт
`journalWritten: false`, интерфейс говорит «Журнал не записан». Отказы guard и предусловий (read-only, неверное подтверждение,
`badThreshold`, `noLeader`, `slotActiveOrGone` …) не пишутся — запрос не уходил или ничего не произошло; ошибки сервера и Patroni,
в том числе неизвестный исход, пишутся с `outcome: "error"` и `code`. Пакет записывает **каждую** сессию с её итогом (у сессии
без снимка — только пара и итог).

```json
{"ts": 1761300000000, "action": "terminate", "connection": {"id": "6f1c…", "name": "billing-prod", "environment": "prod"},
 "member": "pg-1", "prodConfirmed": true, "outcome": "ok", "code": null, "ms": 412,
 "sessions": [{"pid": 4242, "backendStartUs": 1761299000000000, "user": "app", "database": "orders", "application": "api",
               "client": "10.0.0.5", "state": "idle in transaction", "xactAgeS": 3600, "queryId": "-123…",
               "query": "UPDATE shop.orders SET …", "result": "terminated"}]}
```

`action`: `cancel` | `terminate` | `dropSlot` | `switchover` | `cancelSwitchover` | `failover` | `restart` | `cancelRestart` | `pause` |
`resume`. `outcome`: `ok` | `partial` (часть пакета не выполнена) | `error`; `code` — `AppError.code` при `error` (у пакета —
первой сессии с отказом или сбоем). `prodConfirmed` — подключение prod (guard пройден). У слота — `slot { name, slotType, plugin,
database, retainedBytes, walStatus }`; у Patroni — `patroni { method, path, body, status, response }`: `path` — адрес, на
который запрос ушёл в последний раз (без учётных данных; при ошибке до отправки — путь запроса), `member` — участник restart
или лидер на момент действия, `response` — первые 500 символов ответа Patroni (или `details` ошибки), очищенные от
`user:password@` в адресах. Пустые `sessions`, `slot` и `patroni` в строку не пишутся. Текст запроса — первые 500 символов (по символам, не
по байтам), литералы остаются (решение владельца: на разборе инцидента видно, что именно завершили). Журнал не уходит в MCP
(концепция §8) и не входит в архив диагностики (`info.json`, `connections.json`, `logs/`).

`list_actions(connectionId: Option<String>, limit ≤ 1000)` читает `actions.1.jsonl` и `actions.jsonl`, новые сверху, отбирает по
подключению, битые строки пропускает. Вкладка «Журнал действий» подключения (последний пункт группы «Диагностика») — таблица
последних 1000 записей: время, действие, узел, объект (слот, число сессий или адрес запроса), итог; строка раскрывается в
полную запись; скрытая вкладка журнал не читает, при показе читает заново; кнопка «Обновить».

---

## 9. IPC-контракт

| Часть | Команды |
|---|---|
| P2a | `run_diagnostic(id, request) -> DiagReport`; `start_sampler(id, member: Option<String>, channel: Channel<SamplerEvent>) -> SessionId`; `stop_session(sessionId) -> bool` |
| P2b | `preview_session_action(id, request: SessionActionRequest) -> SessionActionPreview { member, rows }`; `execute_session_action(id, request: SessionActionExecute, confirmation) -> SessionActionResult { member, results, outcome, journalWritten }`; `preview_drop_slot(id, request: SlotDropRequest) -> SlotDropPreview { member, slot }`; `drop_slot(id, request, confirmation) -> ActionResult { journalWritten }`; `preview_patroni_action(id, action) -> PatroniActionPreview`; `execute_patroni_action(id, action, confirmation, channel: Channel<WatchEvent>) -> PatroniActionResult { response, watch: Option<SessionId>, journalWritten, outcomeUnknown, error: Option<AppError> }`; `list_actions(connectionId: Option<String>, limit) -> [ActionRecord]` |

`PatroniAction` (тег `kind`) — `switchover { leader, candidate?, scheduledAt? }` (`leader` обязателен — лидер предпросмотра,
§8.5) | `cancelSwitchover` | `failover { candidate }` |
`restart { member, onlyPending, scheduledAt? }` | `cancelRestart { member }` | `pause { paused }`; `scheduledAt` — RFC 3339 со
смещением. `SessionActionRequest { member, kind: cancel | terminate, target }`; `SessionActionExecute` — то же и `sessions: [{ pid,
backendStartUs }]`. `WatchEvent` и `PatroniActionResult` — §8.6, `ActionRecord` — §8.8. Данные фронтенда, которые Rust отдаёт
с `Option`, — `| null` (например, `reason` разрешённой строки предпросмотра), типы фронтенда повторяют структуры
`src-tauri/src/actions/*`.
Новых событий Tauri нет: данные длительных сессий идут через их каналы, изменения топологии — через `topology://changed`.
Все команды возвращают `Result<T, AppError>`; JSON — camelCase. `start_sampler` — тонкая обёртка над
`sampler::start(st, id, member, sink) -> SessionId`: `sink` (`SamplerSink`) оборачивает `Channel`, поэтому сэмплер проверяется
интеграционным тестом без окна приложения.

---

## 10. Ошибки

| code | Причина | Поведение UI |
|---|---|---|
| `RECOVERY_CONFLICT` (новый, P2a) | Запрос на узле в recovery отменён: SQLSTATE 40001 или 40P01 **и** (английский текст «conflict with recovery» **или** файл сервера `postgres.c`): сообщения сервера локализуются `lc_messages` | «Запрос на реплике отменён конфликтом recovery», `retryable` — следующий опрос повторит |
| `READ_ONLY` (P2b) | Действие на read-only подключении (`errors.guard.readOnly`) | Кнопок нет; команда всё равно отказывает |
| `CONFIRMATION_REQUIRED` (P2b) | Нет или неверно имя подключения (`errors.guard.prodConfirm`), имя объекта или число сессий (`errors.guard.typeToConfirm`) | Диалог подтверждения |
| `NOT_AUTHORIZED` (42501) | Нет атрибута `REPLICATION` для слота: `errors.actions.replicationRequired`. Сигнал сессии без права — не ошибка команды, а итог сессии `notAllowed` (в журнале `code` — `NOT_AUTHORIZED`) | Подсказка у кнопки (§8.7), строка итога |
| `AUTH_FAILED` | Patroni отклонил учётные данные (401, 403) | `errors.patroni.unauthorized` или `credentialsWithheld` — те же ключи, что у чтения |
| `PATRONI_ERROR` | Отказ Patroni и сбои запроса | `errors.patroni.actionRejected` (`params.status`, 500 символов ответа в `details`), `authRequired`, `noUrls`; неизвестный исход — `timeout`, `connectionLost`, `statusUnknown` (§8.5); `unreachable` — запрос не дошёл |
| `TLS_ERROR` | Ошибка сертификата при запросе к API Patroni: `errors.patroni.tls` (запрос не дошёл, пробуется следующий адрес) | Тот же ключ, что у чтения |
| `INVALID_INPUT` | Нарушено предусловие или неверный ввод | `errors.actions.leaderHealthy`, `noLeader`, `leaderChanged`, `badCandidate`, `candidateNotReady` (причины — `params.reasons`, лаг — `params.lagBytes`; интерфейс переводит коды теми же ключами, что вкладка готовности), `badSchedule`, `pastSchedule`, `nothingScheduled`, `pauseUnchanged`, `patroniOnly`, `badThreshold`, `noSessions`, `badTarget`, `batchTooLarge`, `slotActiveOrGone`; `errors.patroni.badUrl` |
| `NOT_FOUND` | Слота нет на узле | `errors.actions.slotNotFound` |
| `TIMEOUT` | Пакет не уложился в 60 с; сервер не ответил на запрос за 10 с | Итог сессии `failed` с `errors.actions.batchTimeout` или `statementTimeout`; оставшиеся после `statementTimeout` — `batchStopped` |
| `NODE_UNAVAILABLE` | Узел действия недоступен; подключение закрыто или открыто заново за время пакета | `errors.actions.nodeUnavailable`; оставшиеся сессии пакета — итог `failed` с `errors.topology.notConnected` |

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
| Сервис `pg-lab` (`gp-pg-lab`, `127.0.0.1:5470`): образ собирается из `dev/lab/Dockerfile` (`FROM postgres:16`, `sql/init.sql` и `lab/30-logical.sh` копируются в образ; bind-монтирования нет — контейнер не зависит от пути рабочей копии, правка init-скриптов требует пересборки `pg-lab`); `wal_level=logical`, `max_prepared_transactions=10`, `max_replication_slots=10`, `pg_stat_statements` в `shared_preload_libraries` | Логические слоты, подписки, prepared transactions — на остальных узлах они невозможны без рестарта. Init-скрипт `dev/lab/30-logical.sh`: в `orders` — публикация `lab_pub` и слот `lab_sub_slot` (`pgoutput`); БД `lab_sub` с копией `shop.customers` и подпиской `lab_sub` на себя через unix-сокет (`create_slot = false`, `copy_data = false`) |
| `scripts/wait-stand.sh` | Ждёт и `gp-pg-lab` |

### 12.2 Автотесты

- **`pg-advisor` (unit):** `locktree` — цепочка, несколько корней, мягкие блокирующие, цикл, DDL в очереди; `rates` — первый
  снимок, сброс статистики, перезапуск; `ash` — интервалы окон, `cpu`, параллельные воркеры, промежутки, топы; `failover` —
  фикстуры `/cluster` и `/config` Patroni 3.x и 4.x (обычный, `synchronous_mode`, `"quorum"`, `nofailover`,
  `failover_priority: 0`, лаг выше предела, pause); каждое правило §4 — на границах порогов; имена слотов участников Patroni.
- **Rust (unit):** текст SQL по версиям сервера — модульные тесты `Caps` рядом с запросами (`diag/{pulse,sessions,replication,
  horizon,slots}.rs`, `sampler/query.rs`: границы 14 / 15 / 16 / 17, отсутствующий столбец — типизированный `NULL`, SQL
  собирается с константой `application_name`), снимков текста SQL нет — их заменяет выполнение всех наборов на PG 13–18
  (интеграционные тесты «Матрица» ниже); `Caps` и `unavailable`; буфер сэмплера (лимит строк и времени, интернирование, промежутки, пропуск тика); подписка и
  отписка сэмплера, остановка с последним подписчиком; разрешение «за лидером» и смена узла; память счётчиков и номер открытия;
  разбор `scheduled_switchover` / `scheduled_restart`; `get_status` на поддельном HTTP. P2b: guard и матрица подтверждений
  §8.1; параметры и критерии SQL действий; классификация итогов; тела и адреса запросов Patroni и обход при отказе на
  поддельном HTTP; предусловия (failover при здоровом лидере, расписание в прошлом); `watch::progress` на последовательностях
  топологий; журнал — ротация, права 0600, обрезка текста, отсутствие учётных данных, `list_actions` с битой строкой.
- **Служебные помощники** интеграционных тестов `common::superuser()` и `common::app()` открывают сессию без
  `default_transaction_read_only` (`SESSION_OPTIONS` P1 иначе запретили бы запись сценариев).
- **Интеграционные** (`GPG_IT_*`; без переменной тест пропускается; `GPG_IT_LAB=localhost:5470` — новый; сценарии с
  остановкой узлов и переключениями — под `GPG_IT_DOCKER=1`, failover — ещё и под `GPG_IT_FAILOVER=1`; каждый сценарий
  возвращает стенд в исходное состояние при любом выходе):
  - **Блокировки** (`pg-single`): держатель и жертва — `glafira_app`, DDL выполняет `postgres` (у `glafira_app` нет владения
    таблицей). Порядок: держатель делает `UPDATE` строки `shop.orders` и держит транзакцию; затем в транзакции выполняется
    `ALTER TABLE shop.orders ADD COLUMN it_tmp int` и встаёт в очередь (потом откат); затем `SELECT` жертвы — классическая очередь
    за миграцией → корень, DDL в очереди, жертвы, `ddlQueue` critical.
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
  - **Матрица** (`GPG_IT_MATRIX`): каждый SQL-набор §5 и §6 выполняется на PG 13–18 без ошибок (`it_diag`: сессии, пульс,
    блокировки, горизонт и реплики, слоты; `it_sampler`: запрос сэмплера); недоступные по версии части перечислены в
    `unavailable`.
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
