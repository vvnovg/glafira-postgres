# Glafira for PostgreSQL — этап P3 «Тяжёлые запросы»

**Дата:** 2026-10-29
**Статус:** на ревью
**Концепция:** `2026-10-19-glafira-postgres-concept-design.md` (§6 — тяжёлые запросы и рекомендации целиком, §7 — безопасные
действия, §11 — ошибки, §12 — тестирование)
**Предыдущий этап:** `2026-10-24-glafira-postgres-p2-design.md` (каркас диагностики `run_diagnostic`, конверт отчёта, сэмплер,
guard и журнал действий, стиль спеки)
**Образец кода:** `src-tauri/src/{diag,sampler,actions}`, `crates/pg-advisor`, `src/components/diag`

P3 отвечает на вопрос дежурного и разработчика «какие запросы грузят базу и почему»: топ запросов по `pg_stat_statements`
за окно и с момента сброса, карточка запроса с историей и эвристиками, EXPLAIN и EXPLAIN ANALYZE с деревом плана и
двухъярусной защитой, консоль планов; во второй части — рекомендации по плану с проверкой гипотезы через hypopg, снимки
статистики и их сравнение, опциональные расширения. Правила остаются чистыми функциями `pg-advisor`, отчёты — в конверте
`DiagReport` P2, интерфейс — поверх вкладки диагностики. Парсер SQL (`pg_query`) в P3 не берётся: таблицы запроса даёт план,
тексты pgss уже нормализованы.

---

## 1. Объём

**Входит:**
1. Стенд: сервис `pg-perf` (PG 17, `pg_stat_statements` с `track_planning`, hypopg, `pg_stat_kcache`, `pg_wait_sampling`,
   объёмная схема и сценарий `pgbench` с «плохими» запросами).
2. Сэмплер `pg_stat_statements`: раз в 60 с на узел, `showtext := false`, буфер дельт на 60 мин; живёт, пока открыт потребитель
   (инструмент «Запросы» или карточка запроса узла), как сэмплер активности P2.
3. Инструмент «Запросы» (`queries`) во вкладке диагностики: режимы «за окно 5 / 15 / 60 мин» и «с момента сброса»,
   сортировки, фильтр по базе и `toplevel`, покрытие окна, предупреждение о неполном топе (`dealloc`), тексты по запросу.
4. Находки без плана в `pg-advisor`: N+1, нестабильный план, низкий hit ratio, temp-файлы, планирование дольше выполнения,
   неполный топ; новые цель `Query` и ссылка `OpenQuery`.
5. Вкладка запроса (`query`): обзор (текст, метрики с момента сброса и по окнам, история, разбивка по ролям, выполняющиеся
   экземпляры, ожидания из сэмплов сессий, таблицы из плана, находки), подвкладка «План», «Открыть в консоли», «Скопировать
   для LLM» (markdown с запросом, статистикой, планом и DDL таблиц).
6. Планы: `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS)`, `GENERIC_PLAN` на 16+, форма параметров и текст выполняющегося экземпляра;
   EXPLAIN ANALYZE в два яруса (чтение — `READ ONLY` без подтверждения; запись — действие с подтверждением, списком таблиц и
   журналом); парсер JSON-плана и модель `PlanTree` в `pg-advisor`; дерево узлов с собственным временем, долей, ошибкой оценки,
   буферами; консоль `explain` с запросом или вставленным JSON-планом.
7. Рекомендации по плану (11 правил §6.3 концепции) с доказательством, командой, эффектом и рисками; «Проверить гипотезу» через
   hypopg (стоимость «была / стала», использован ли индекс, оценка размера).
8. Снимки pgss на диске (`snapshots/<подключение>/<узел>/…json`, 0600), сравнение (новые, исчезнувшие, деградировавшие,
   ускорившиеся), режим топа «между снимками».
9. Парсер текстовых планов (psql / auto_explain) в тот же `PlanTree`.
10. Опциональные расширения: `pg_stat_kcache` (CPU и диск в топе и карточке), `pg_wait_sampling` (точный профиль ожиданий
    запроса); подсказки готовности к расширениям и к `track_planning`; пункт готовности о правах чтения для EXPLAIN.
11. Руководство: глава «Запросы»; `docs/admin.md` — права, нагрузка сэмплера, ярусы EXPLAIN ANALYZE, снимки.

**План реализации делится на две части**, каждая заканчивается работающим приложением (как P2a / P2b):
- **P3a «Видеть»:** пункты 1–6, 11 (без разделов о рекомендациях, снимках и расширениях). Ветка `feat/p3a`.
- **P3b «Советовать и сравнивать»:** пункты 7–10 и остаток пункта 11. Ветка `feat/p3b` после релиза P3a.

**Не входит:** `pg_query` и любой разбор SQL на клиенте (P4 — проверка SQL от LLM); `pg_qualstats` и индексный советник по
нагрузке (после MVP); разбор логов auto_explain файлами (после MVP; вставка одного плана в консоль — входит); выполнение
рекомендованных команд приложением (оно их только показывает и копирует); переформатирование SQL; фоновый сбор без открытого
потребителя; настройка порогов пользователем (P5, вместе с отчётом); MCP (P4).

**Лицензия.** Все функции P3 бесплатны: кода лицензирования в приложении нет (спец. P1 §2.1), деление Free / Pro из §10 концепции
не действует.

**Решения владельца (брейншторм 2026-10-29):**
1. Разбиение P3a / P3b — как выше (одна спека, два плана).
2. Без `pg_query` в P3: таблицы — из плана, нормализация не нужна, число параметров — по `$N`.
3. Топ — инструмент диагностики; карточка запроса и консоль EXPLAIN — свои виды вкладок.
4. Сэмплер pgss живёт, пока открыт потребитель, по механике P2; буфер сохраняется между открытиями.
5. EXPLAIN ANALYZE в два яруса по пробному плану; чтение — без подтверждения в `READ ONLY`.
6. Парсер текстовых планов — в P3b; консоль P3a принимает SQL и JSON.
7. В P3b — `pg_stat_kcache` и `pg_wait_sampling`; `pg_qualstats` — после MVP.
8. Новый сервис стенда `pg-perf` — первой задачей P3a.

---

## 2. Что берём из P2

| Откуда | Что | Изменения в P3 |
|---|---|---|
| P2 `diag/mod.rs` | `Tool`, `DiagRequest`, `DiagData`, `DiagReport`, `Unavailable`, `head`, `on_member`, `make_report` | `Tool::Queries` (узловой), `DiagRequest.queries: Option<QueriesParams>`, `DiagData::Queries` (§4.5) |
| P2 `diag/caps.rs` | `Caps` по версии сервера | Используется для `pg_stat_activity` (`query_id` на 14+); колонки pgss — отдельные `PgssCaps` по версии расширения (§3.3) |
| P2 `sampler/registry.rs`, `buffer.rs` | `SamplerRegistry`, узлы с поколением, подписчики, `SamplerSink`, `Buffer` | Реестр параметризуется видом сэмплера (трейт `Sampling`: источник, буфер, событие тика, интервал); второй экземпляр для pgss с `StatementsBuffer` (§5) |
| P2 `sampler/query.rs` | `sample_sql(caps)` с `query_id` | Без изменений; буфер активности — источник ожиданий запроса (§7.2) |
| P2 `session.rs` | `SessionRegistry`, `stop_session` | Без изменений: подписки сэмплера pgss — те же сессии |
| P2 `actions/conn.rs` | `ActionClient`, `ConnectSpec::for_action()` | `ConnectSpec::for_explain()` с `EXPLAIN_SESSION_OPTIONS` — без `search_path=pg_catalog` (§8.3) |
| P2 `guard.rs` | `ensure_mutation_allowed` | Ярус записи EXPLAIN ANALYZE; интерфейс прежний |
| P2 `actions/journal.rs` | `ActionRecord`, запись и ротация | Действие `explainAnalyze`, необязательное поле `explain` (§8.4) |
| P2 `common/ConfirmDialog.tsx` | Подтверждение внутри страницы | Ярус записи: список таблиц и предупреждение |
| P2 `pg-advisor::findings` | `Finding`, `FindingTarget`, `FindingLink`, `thresholds`, `grade` | `FindingTarget::Query { query_id, database }`, `FindingLink::OpenQuery { query_id, database, member }`; пороги §6 |
| P1 `catalog/table.rs`, `catalog/ddl.rs` | `table::get`, `TableDetails.ddl`, индексы | DDL таблиц для «Скопировать для LLM» (§7.4) |
| P1 `readiness`, `pg-advisor::readiness` | Чек-лист, `OPTIONAL_EXTENSIONS` | Факты `pgss_version`, `pgss_track_planning`, `read_all_data`; пункты `pgssTrackPlanning`, `readAllData`; P3b — подсказки к расширениям (§9.6) |
| P1 `storage/fs.rs` | `write_private`, `read_json`, `atomic_write` | Файлы снимков (§9.3) |
| P1 `src/state/workspace.ts` | `TabSpec` | Виды `query` и `explain` (§7.1, §8.7) |
| P1 `src/components/catalog/DdlView.tsx`, `common/CopyButton` | Показ SQL, копирование | Текст запроса в карточке, «JSON», «Как текст», «для LLM» |

---

## 3. Архитектура

### 3.1 Модули

**`crates/pg-advisor`** (без I/O, зависимости только serde / serde_json):
- `pgss.rs` — `PgssCaps::from_version(&str)`: какие колонки есть у установленной версии расширения (§3.3).
- `queries/mod.rs` — `QueryStat`, `QueryKey`, `Totals`, вычисление долей, hit ratio, строк на вызов; `queries/rules.rs` —
  эвристики без плана (§6); P3b: `queries/compare.rs` — сравнение двух наборов (§9.3).
- `plan/mod.rs` — `PlanTree`, `PlanNode`, вычисляемые поля; `plan/json.rs` — `from_json(&Value)`; `plan/text_out.rs` —
  `to_text(&PlanTree)`; P3b: `plan/text_in.rs` — `from_text(&str)`; `plan_rules/` — 11 правил и `Recommendation` (§9.1).
- `findings.rs` — новые варианты цели и ссылки.

**`src-tauri/src`**:
- `sampler/statements.rs` — `StatementsSource` (чтение pgss по `PgssCaps`), `StatementsBuffer` (дельты по тикам),
  `StatementsEvent`; `sampler/registry.rs` обобщается трейтом `Sampling`.
- `diag/queries.rs` — SQL по `PgssCaps`, режимы, тексты и их кеш, сборка `DiagData::Queries`.
- `diag/query_detail.rs` — карточка: накопительные и оконные метрики, история, разбивка, выполняющиеся экземпляры, ожидания.
- `explain/mod.rs` (`ExplainRequest`, `ExplainResult`), `explain/run.rs` (пробный план → ярус → выполнение), `explain/params.rs`
  (счётчик `$N`, привязка значений), `explain/tier.rs` (определение яруса по дереву и первому слову), `explain/llm.rs` (markdown);
  P3b: `explain/hypopg.rs`.
- P3b: `snapshots/{mod,store,compare}.rs`.
- `commands/queries.rs`, `commands/explain.rs`; P3b: `commands/snapshots.rs`.

**Фронтенд** (`src/components/queries/`): `QueriesView` (инструмент в `DiagTab`), `QueryTab` → `QueryOverview`, `QueryPlan`;
`ExplainTab`; `plan/PlanTree`, `plan/PlanNodeDetails`, `plan/PlanSummary`, `plan/ParamsForm`, `plan/AnalyzeConfirm`;
P3b: `Recommendations`, `HypoCheck`, `SnapshotsPanel`, `SnapshotCompare`. Хуки `src/lib/useStatementsSampler.ts` (по образцу
`useSampler`), `useQueryDetail.ts`, `useExplain.ts`. Типы — `src/api/types.ts`, вызовы — `src/api/client.ts`.

### 3.2 Поток данных

1. Вкладка «Запросы» подписывается на сэмплер pgss узла (`start_statements_sampler`) и запрашивает отчёт `run_diagnostic`
   с `tool: queries`. Режим «с момента сброса» — одно чтение сервера на отчёт; «за окно» — отчёт собирается из буфера без
   обращения к серверу и обновляется по событию `tick` (раз в минуту) и при смене окна, сортировки или фильтра.
2. Строки отчёта несут тексты из кеша; для строк без текста вкладка вызывает `query_texts` одним пакетом, ответ попадает в кеш.
3. Строка таблицы или ссылка `OpenQuery` находки открывает вкладку `query`; она подписывается на тот же сэмплер (история
   продолжает копиться, пока открыта хотя бы одна из вкладок) и вызывает `query_detail`.
4. Подвкладка «План» и консоль вызывают `explain`; результат — `PlanTree` для дерева и сырой JSON для кнопки «JSON».
   Вставленный план разбирает `parse_plan` без сервера.
5. P3b: `explain` возвращает ещё `recommendations`; «Проверить гипотезу» вызывает `hypo_check`; снимки — `snapshot_*`.

Отчёт инструмента остаётся конвертом P2 (`DiagReport`): `member`, `serverVersionNum`, `inRecovery`, `data`, `findings`,
`unavailable` — на нём держатся будущие `run_diagnostic` MCP (P4) и отчёт P5.

### 3.3 Версии pg_stat_statements и PostgreSQL

Набор колонок определяется **версией расширения** (`pg_extension.extversion`), а не сервера: после `pg_upgrade` без
`ALTER EXTENSION … UPDATE` сервер 17 может работать с pgss 1.10.

| extversion | Сервер | Что появляется |
|---|---|---|
| 1.8 | 13 | `plans`, `total_plan_time` и прочие `*_plan_time`, `wal_records`, `wal_fpi`, `wal_bytes`; колонки времени называются `*_exec_time` |
| 1.9 | 14 | `toplevel`; представление `pg_stat_statements_info` (`dealloc`, `stats_reset`) |
| 1.10 | 15 | `jit_*` (8 колонок), `temp_blk_read_time`, `temp_blk_write_time` |
| 1.11 | 17 | `blk_read_time` / `blk_write_time` → `shared_blk_read_time` / `shared_blk_write_time`; `local_blk_*_time`; `jit_deform_*`; `stats_since`, `minmax_stats_since` |
| 1.12 | 18 | `parallel_workers_to_launch`, `parallel_workers_launched`, `generic_plan_calls`, `custom_plan_calls` |

`PgssCaps` отдаёт флаги `toplevel`, `info`, `jit`, `temp_io_time`, `shared_io_names`, `local_io_time`, `stats_since`,
`parallel`; версия ниже 1.8 — `Unavailable{part: statements, reason: «pg_stat_statements старше 1.8»}` (PG 12 не поддерживается
и так). Колонки планирования заполняются только при `pg_stat_statements.track_planning = on` (по умолчанию выключен) — иначе
колонки серые с подсказкой, правило `query_planning_heavy` молчит (§6, §9.6).

Со стороны сервера: `pg_stat_activity.query_id` — 14+ (`compute_query_id`), `GENERIC_PLAN` — 16+, `EXPLAIN (WAL)` — 13+,
`SETTINGS` — 12+. Остальное одинаково на 13–18.

### 3.4 Соединения и жизненный цикл

| Назначение | Соединений на узел | Когда |
|---|---|---|
| `Catalog` | до 3 | Отчёт «с момента сброса», `query_texts`, `query_detail`, DDL для markdown — как остальные вкладки |
| `Sampler` | 1 | Общий для сэмплера активности (раз в секунду) и сэмплера pgss (раз в минуту): минутное чтение на 100–300 мс задерживает один секундный тик, нового соединения не добавляем |
| EXPLAIN (вне пулов) | 1 | На время одного `explain` / `hypo_check`: `ConnectSpec::for_explain()` (§8.3), закрывается по завершении |

Закрытие и переоткрытие подключения, исчезновение участника — как в P2 §3.4: сэмплер pgss узла останавливается, буфер и кеш
текстов удаляются, поколение открытия защищает от запоздавших ответов. Вкладки `query` и `explain` при закрытии подключения
показывают заглушку «подключение закрыто» и переподписываются после «Подключиться».

---

## 4. Топ запросов (`queries`, узел)

### 4.1 Источник

Расширение ищется в базе подключения: `SELECT extversion, nspname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
WHERE extname = 'pg_stat_statements'`. Нет строки — `Unavailable{part: statements}` с подсказкой `CREATE EXTENSION pg_stat_statements`
(пункт готовности `pgss` P1 уже красный). Функция и представление вызываются по найденной схеме.

```sql
SELECT s.userid, s.dbid, s.queryid, <s.toplevel | true>, d.datname, r.rolname,
       s.calls, s.total_exec_time, s.min_exec_time, s.max_exec_time, s.mean_exec_time, s.stddev_exec_time, s.rows,
       s.plans, s.total_plan_time,
       s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written,
       s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written,
       s.temp_blks_read, s.temp_blks_written,
       <s.blk_read_time | s.shared_blk_read_time>, <s.blk_write_time | s.shared_blk_write_time>,
       <s.temp_blk_read_time + s.temp_blk_write_time | NULL::float8>,
       s.wal_records, s.wal_fpi, s.wal_bytes,
       <s.jit_functions | NULL::bigint>,
       <s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time [+ s.jit_deform_time] | NULL::float8>,
       <s.parallel_workers_to_launch, s.parallel_workers_launched | NULL::bigint, NULL::bigint>
FROM <ns>.pg_stat_statements(false) s
LEFT JOIN pg_database d ON d.oid = s.dbid
LEFT JOIN pg_roles r ON r.oid = s.userid
```

Отдельно: `dealloc`, `stats_reset` из `pg_stat_statements_info` (1.9+; на 1.8 — `NULL`), `current_setting('pg_stat_statements.max')`,
`current_setting('pg_stat_statements.track_planning', true)`. Тексты не читаются (`showtext := false`): файл текстов не трогается.
Чужие строки доступны роли с `pg_read_all_stats` (`pg_monitor` включает её); без неё pgss отдаёт строки с `queryid = NULL` и
текстом `<insufficient privilege>` — такие строки отбрасываются, отчёт получает `Unavailable{part: others, reason: «нужна роль
pg_read_all_stats»}`.

### 4.2 Режимы

- **С момента сброса** — одно чтение; сортировка, обрезка до `limit` и итоги по всему набору (`totals`) — в Rust.
  `since.statsResetMs` и `since.dealloc` — в шапке.
- **За окно 5 / 15 / 60 мин** — сумма дельт буфера сэмплера (§5) по тикам в окне `[последний тик − W, последний тик]`.
  Покрытие `coverageS` — от первого тика с дельтами в окне (или первого тика буфера, если он моложе) до последнего;
  `ticks` — число тиков с дельтами. Меньше одного тика с дельтами — `rows` пустые, вкладка показывает «накоплено 0 из 5 мин».
  Среднее — `totalExecMs / calls` за окно; **min / max / stddev за окно недоступны** (накопительные счётчики не дельтируются) —
  колонки серые, сортировки по ним выключены.
- **Между снимками** (P3b, §9.3) — дельты между двумя файлами снимков того же подключения.

Фильтры: база (`database: null` — все), только верхний уровень (`toplevelOnly`, по умолчанию `true`; на 1.8 поля нет —
фильтр скрыт).

### 4.3 Сортировки

`QuerySort`: `totalTime` (по умолчанию), `meanTime`, `maxTime`, `stddevTime`, `calls`, `rowsPerCall`, `sharedRead`, `hitRatio`
(по возрастанию — худшие сверху), `tempWritten`, `walBytes`, `ioTime`, `planTime`, `jitTime`; P3b — `cpuTime`. Остальные —
по убыванию. Сортировка, недоступная по версии pgss или режиму, в интерфейсе серая с подсказкой; запрос с такой сортировкой
отвечает `INVALID_INPUT` (`errors.queries.sortUnavailable`).

### 4.4 Модель

```ts
interface QueryKey { queryId: string; dbid: number; userid: number; toplevel: boolean }   // queryId — строка: i64 не влезает в number
interface QueryStat {
  key: QueryKey; datname: string | null; rolname: string | null;
  calls: number; totalExecMs: number; meanExecMs: number; minExecMs: number | null; maxExecMs: number | null;
  stddevExecMs: number | null; rows: number; rowsPerCall: number;
  plans: number; totalPlanMs: number;
  sharedHit: number; sharedRead: number; sharedDirtied: number; sharedWritten: number;
  localHit: number; localRead: number; localDirtied: number; localWritten: number;
  tempRead: number; tempWritten: number; ioReadMs: number; ioWriteMs: number; tempIoMs: number | null;
  walRecords: number; walFpi: number; walBytes: number;
  jitFunctions: number | null; jitMs: number | null; parallelPlanned: number | null; parallelLaunched: number | null;
  sharePct: number; hitRatio: number | null;            // доля в totals.totalExecMs; null при hit + read = 0
  text: string | null; textTruncated: boolean;          // из кеша; null — ещё не запрашивался
  cpuUserMs?: number | null; cpuSysMs?: number | null; diskReadBytes?: number | null; diskWriteBytes?: number | null; // P3b
}
```

### 4.5 Запрос и отчёт

```ts
interface QueriesParams {
  mode: { kind: "sinceReset" } | { kind: "window"; s: 300 | 900 | 3600 } | { kind: "between"; from: string; to: string };
  sort: QuerySort; limit: 50 | 100 | 200; database: string | null; toplevelOnly: boolean;
}
// DiagRequest: { tool: "queries", member, queries: QueriesParams }
interface QueriesData {
  kind: "queries"; mode: QueriesParams["mode"]; coverageS: number | null; ticks: number | null;
  pgss: { version: string; max: number; trackPlanning: boolean; statsResetMs: number | null; dealloc: number | null };
  totals: { calls: number; totalExecMs: number; rows: number };
  rows: QueryStat[];
}
```

`Tool::Queries` — узловой (`is_cluster() = false`), доступен серверу, узлам и кластеру Patroni («за лидером», как `sessions`).
`validate` отказывает запросу без `queries` (`INVALID_INPUT`). Отчёт в режиме окна не ходит на сервер, но `head` (версия,
`in_recovery`, время) берётся из последнего тика сэмплера, чтобы конверт был полным. Сэмплер узла ещё не запущен (вкладка
только подписывается) — `rows` пустые, `coverageS: null`, `unavailable{part: window, reason: «сэмплер запускается»}`, `head` —
обычным `head(c)`.

### 4.6 Тексты

`query_texts(id, member, keys: QueryKey[]) -> { key, text, truncated }[]`:
```sql
SELECT queryid, dbid, userid, <toplevel | true>, left(query, 16384), length(query) > 16384
FROM <ns>.pg_stat_statements(true) WHERE queryid = ANY($1)
```
Кеш на узел (`HashMap<QueryKey, Arc<str>>`) — до 2000 текстов, при переполнении очищается целиком; живёт до закрытия
подключения. Вкладка запрашивает тексты только для видимых строк без текста, одним вызовом на отчёт.

### 4.7 Интерфейс

Инструмент в `DiagTab` рядом с остальными; пункт «Запросы» в дереве у сервера, узлов и кластера (`DiagnosticsNodes`).
Шапка: узел и «за лидером» (как у P2), переключатель режима (5 / 15 / 60 / с момента сброса; P3b — между снимками), значок
покрытия («накоплено 12 из 15 мин», «идёт первый тик»), фильтр базы, флажок `toplevel`, предел строк, значок «топ неполный»
при `dealloc`. Таблица: первая строка текста моноширинным шрифтом с многоточием (полный текст — во всплывающей подсказке),
полоса доли, метрики по сортировкам; клик по заголовку сортирует; клик по строке открывает вкладку запроса; контекст —
«Открыть в консоли». Панель находок — общая (`FindingsPanel`); ссылка `OpenQuery` открывает карточку. Версия pgss и
`stats_reset` — в подвале.

---

## 5. Сэмплер pg_stat_statements

- **Запрос** раз в 60 с — SQL §4.1 без текстов плюс `dealloc` / `stats_reset`; время тика — часы приложения, как в P2.
  Первый тик после запуска — только база (дельт нет); дельты появляются со второго.
- **Буфер** (`StatementsBuffer`): `latest: HashMap<QueryKey, Counters>` (последние накопительные значения) и кольцо тиков
  `Tick { atMs, dealloc, deltas: Vec<(QueryKey, Delta)> }`, где в тик попадают только ключи, у которых изменились `calls`
  или `total_exec_time`. `Delta` — аддитивные счётчики (`calls`, времена выполнения и планирования, `plans`, `rows`, 12 счётчиков
  блоков, времена I/O, WAL, JIT, параллельные воркеры; P3b — kcache). Ключ, у которого накопительные счётчики уменьшились
  (вытеснен и вернулся), получает новую базу: дельта тика равна его новому накопительному значению. Смена `stats_reset` —
  новая база для всех ключей тем же правилом, прошлые тики остаются (их дельты верны); событие `gap` не шлётся.
  Хранение — 60 мин; предел — 200 000 записей `(ключ, дельта)` на узел (порядка 50 МБ), при переполнении вытесняются целые
  старые тики. Окно считается суммой дельт по тикам в `[последний − W, последний]`.
- **Жизнь** — как у сэмплера активности (P2 §6): узел создаётся первой подпиской `start_statements_sampler(id, member, channel)`,
  задача останавливается с последней отпиской, буфер живёт до закрытия подключения; повторная подписка продолжает буфер,
  промежуток записывается как `gap` и показывается на истории карточки. Подписчики — инструмент «Запросы» и вкладки `query`
  того же узла. Перезагрузка вебвью останавливает сессии; вкладки подписываются заново при монтировании.
- **Нагрузка и сбои.** Одно чтение в минуту на соединении пула `Sampler` (§3.4); предел запроса 5 с (`on_catalog_node_split`),
  таймаут и обрыв закрывают соединение. Ошибка — паузы 1, 2, 5, 10, 30 с и событие `error`; `AUTH_FAILED` — остановка;
  исчезновение расширения (`Unavailable`) — событие `error` с `FEATURE_UNAVAILABLE` и остановка.
- **Канал** `StatementsEvent`: `tick { atMs, coverageS, ticks, changed }` (`changed` — число ключей с дельтами), `gap { fromMs, toMs }`,
  `error { error }`, `stopped`.
- **Реестр.** `SamplerRegistry<S: Sampling>`: `Sampling` задаёт `type Source`, `type Buffer`, `type Event`, `interval()`,
  `tick(buffer, sample) -> Event`. Экземпляры в `AppState`: `samplers` (активность, 1 с) и `statements` (pgss, 60 с); оба —
  слушатели закрытия подключений.

---

## 6. Находки без плана

Правила `pg_advisor::queries::rules::evaluate(rows, totals, pgss, mode) -> Vec<Finding>` считаются по строкам отчёта (уже
обрезанным до `limit`) в выбранном режиме; пороги — `findings::thresholds`. Цель — `Query { queryId, database }` (строка
находки показывает начало текста), ссылка — `OpenQuery { queryId, database, member }`; у `queries_incomplete` цель — `Member`.

| id | Условие | Уровень | Режимы |
|---|---|---|---|
| `query_n_plus_one` | `calls / totals.calls ≥ 0.2`, `meanExecMs ≤ 1`, `calls ≥ 1000` | warning | оба |
| `query_unstable` | `stddevExecMs ≥ 2 × meanExecMs`, `meanExecMs ≥ 1`, `calls ≥ 50` | warning | с момента сброса |
| `query_low_hit_ratio` | `sharedRead ≥ 10 000` блоков, `hitRatio < 0.9` | warning | оба |
| `query_temp` | `tempWritten / calls ≥ 128` блоков (1 МБ на вызов), `calls ≥ 10` | warning | оба |
| `query_planning_heavy` | `totalPlanMs > totalExecMs`, `plans ≥ 100`; только при `trackPlanning` | info | оба |
| `queries_incomplete` | с момента сброса — `dealloc > 0`; за окно — `dealloc` вырос в окне | warning | оба |

Параметры находки — цифры условия (`calls`, `sharePct`, `meanMs`, `stddevMs`, `hitRatio`, `tempMb`, `planMs`, `execMs`, `dealloc`,
`max`); текст — во фронтенде (`findingText.ts`). Критических находок у топа нет: это подсказки «куда смотреть», а не аварии.
Сортировка и слияние — `grade` / `sort` P2.

---

## 7. Карточка запроса

### 7.1 Вкладка

`TabSpec { kind: "query", connectionId, member, database, queryId }`. Заголовок — начало текста (или `queryId`, пока текст не
получен); вкладка сохраняется в workspace и восстанавливается при запуске; узел пропал — заглушка с выбором другого узла.
Личность запроса — `queryid` в пределах базы: строки pgss по ролям и `toplevel` суммируются, разбивка показывается отдельной
таблицей. Переключатель узла в шапке меняет `member` вкладки: на физических репликах `queryid` совпадает (OID реплицируются),
и поведение запроса на primary и реплике можно сравнить.

### 7.2 Данные

`query_detail(id, member, database, queryId) -> QueryDetail`:

```ts
interface QueryDetail {
  text: string | null; textTruncated: boolean;
  sinceReset: QueryStat | null;                                  // сумма строк pgss с этим queryid в базе
  windows: Record<"300" | "900" | "3600", QueryStat | null>;     // из буфера сэмплера; null — покрытия нет
  totals: { calls: number; totalExecMs: number };                // для долей и правил
  byUser: { rolname: string | null; toplevel: boolean; stat: QueryStat }[];
  history: { atMs: number; calls: number; totalExecMs: number; meanExecMs: number }[];   // по тикам за 60 мин
  gaps: { fromMs: number; toMs: number }[];
  running: RunningInstance[];                                    // 14+; на 13 — unavailable: running
  waits: QueryWaits | null;                                      // null — буфера активности нет
  findings: Finding[];                                           // правила §6 по sinceReset
  unavailable: Unavailable[];
}
interface RunningInstance {
  pid: number; backendStartUs: number; state: string; startedMs: number | null; durationMs: number | null;
  waitEventType: string | null; waitEvent: string | null; usename: string | null; applicationName: string | null;
  clientAddr: string | null; query: string; truncated: boolean;  // truncated: length ≥ track_activity_query_size − 1
}
interface QueryWaits {
  source: "sampler" | "pgWaitSampling"; sinceMs: number; total: number;
  items: { eventType: string; event: string | null; count: number; sharePct: number }[];
}
```

- `running` — `pg_stat_activity WHERE query_id = $1 AND pid <> pg_backend_pid()` (с `backend_start` для адресации сессии, как в P2).
- `waits` — сэмплы буфера активности этого узла за 60 мин с `query_id = $1`, сгруппированные по типу и событию; активные без
  ожидания считаются как `CPU`. Нет буфера (вкладка «Сессии» не открывалась) — `null`, карточка подсказывает открыть её.
  P3b: при установленном `pg_wait_sampling` — точный профиль (§9.5).
- Таблицы запроса карточка берёт из последнего плана (`Schema` / `Relation Name` узлов, §8.5) и хранит в состоянии вкладки;
  до плана — кнопка «Получить план» (пробный EXPLAIN без выполнения, §8.3).

### 7.3 Обзор

Шапка: текст (моноширинный, подсветка ключевых слов средствами CSS без переформатирования, «Скопировать»), пометка
«обрезан», база, узел, `queryId`. Блоки: метрики в две колонки («с момента сброса» и выбранное окно: вызовы, общее, среднее,
min / max / stddev, строк на вызов, hit ratio, shared read / written, temp, WAL, планирование, JIT); два спарклайна истории
(вызовы в минуту, среднее) с промежутками; находки запроса; разбивка по ролям и `toplevel`; выполняющиеся экземпляры
(кнопка «Открыть в сессиях» — вкладка `sessions` узла); ожидания; таблицы (ссылки на вкладки таблиц). Кнопки: «План»
(подвкладка), «Открыть в консоли» (вкладка `explain` с текстом, базой и узлом), «Скопировать для LLM». Обновление —
по тикам сэмплера и кнопкой; `running` перечитывается каждые 5 с, пока вкладка видима.

### 7.4 «Скопировать для LLM»

`query_llm_markdown(id, member, database, source, plan: PlanTree | null) -> { markdown, warnings }`, где `source` —
`{ kind: "query", queryId }` (карточка) или `{ kind: "sql", sql }` (консоль), собирает в Rust:
1. «Среда» — версия сервера, версия pgss, роль узла (primary / реплика), база.
2. «Запрос» — нормализованный текст pgss в блоке ```sql (с `$N`; литералы выполняющихся экземпляров не включаются — это данные
   пользователей; экземпляр попадает в markdown только если пользователь выбрал его источником плана).
3. «Статистика» — таблица: с момента сброса и за 60 мин (если есть покрытие).
4. «План» — `plan::to_text` переданного дерева; плана нет — пробный `EXPLAIN (FORMAT JSON[, GENERIC_PLAN])` (§8.3, без выполнения);
   на PG < 16 текст с `$N` без значений — строка «план требует параметров» в `warnings`.
5. «Таблицы» — для отношений плана, не больше 10: DDL (`catalog::table::get` → `ddl`) и определения индексов; общий предел
   64 КБ, остаток — «… ещё N таблиц».
Заголовки — на языке интерфейса; промптов и инструкций модели в тексте нет: это данные. Общий предел markdown — 256 КБ.
Копирование — существующий `CopyButton`. Для источника `sql` (консоль, §8.7) раздела «Статистика» нет, план и таблицы — те же.

---

## 8. Планы

### 8.1 Запрос и результат

```ts
interface ExplainRequest {
  member: string; database: string; sql: string;                   // sql ≤ 256 КБ
  mode: "plan" | "analyze";
  params: { kind: "none" } | { kind: "generic" } | { kind: "values"; values: (string | null)[] };
  timeoutS: number;                                                // analyze: 5…600, по умолчанию 30; plan — игнорируется (15 с)
  searchPath: string | null;                                       // null — серверный по умолчанию для роли
  confirmation: Confirmation;                                      // ярус записи
}
interface ExplainResult {
  mode: "plan" | "analyze"; tier: "read" | "write";
  plan: PlanTree; raw: unknown;                                    // raw — JSON сервера как есть
  relations: { schema: string; name: string }[];
  elapsedMs: number; warnings: string[];                           // ключи i18n: genericPlan, paramsFromRunning, truncatedText
  recommendations?: Recommendation[];                              // P3b
}
```

Проверки до сервера (`INVALID_INPUT`): пустой `sql`; `generic` при версии < 16 или `mode: analyze`; число `values` не равно
max `$N` текста (`explain::params::count`: регулярное выражение по `\$(\d+)`; текст pgss литералов не содержит, ложных
совпадений внутри строк нет); `timeoutS` вне 5…600.

### 8.2 Источники текста и параметры

В подвкладке «План» карточки источник выбирается переключателем:
- **Нормализованный текст pgss** — параметров `max $N`. На 16+ по умолчанию `GENERIC_PLAN` (несовместим с ANALYZE: при
  переключении на «с выполнением» форма значений обязательна). Иначе — `ParamsForm`: поле на каждый `$N`, пустое поле — `NULL`;
  значения хранятся в состоянии вкладки.
- **Текст выполняющегося экземпляра** — список из `running` (§7.2); текст с литералами, параметров нет; обрезанный текст
  (`truncated`) предлагать нельзя — пункт серый с объяснением.

Значения передаются расширенным протоколом с типом `unknown` (`prepare_typed(sql, [UNKNOWN; n])`): типы выводит сервер, как
при выполнении приложением.

### 8.3 Ход и ярусы (`explain::run`)

Соединение — `ActionClient` с `ConnectSpec::for_explain()`: `EXPLAIN_SESSION_OPTIONS = -c statement_timeout=5000 -c lock_timeout=1000
-c DateStyle=ISO` — без `search_path=pg_catalog` (запрос пользователя должен видеть его схемы) и без
`default_transaction_read_only`. Все шаги — внутри одной транзакции; SQL пользователя идёт отдельной командой расширенного
протокола, поэтому вторая инструкция в тексте отклоняется сервером (`cannot insert multiple commands into a prepared statement`
→ `INVALID_INPUT`, `errors.explain.oneStatement`).

1. **Пробный план.** `BEGIN READ ONLY` → `SET LOCAL statement_timeout = '15s'` → `SET LOCAL lock_timeout = '1s'` →
   `[SET LOCAL search_path = …]` → `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS[, GENERIC_PLAN]) <sql>` → `ROLLBACK`. Разбор →
   `PlanTree`, список отношений. **Ярус записи**, если в дереве есть `ModifyTable` (в том числе внутри CTE) или первое слово
   инструкции (после комментариев) — `CREATE` (CREATE TABLE AS / CREATE MATERIALIZED VIEW AS); иначе **ярус чтения**.
   EXPLAIN без выполнения сервер считает чтением и в `READ ONLY` строит план даже для UPDATE; если же пробный план отвергнут
   25006 (инструкция вроде CREATE … AS на отдельных версиях), он повторяется в обычной транзакции `BEGIN … ROLLBACK` — только
   чтобы определить ярус и отношения, выполнения по-прежнему нет.
2. `mode: plan` — результат шага 1 (одно обращение к серверу).
3. `mode: analyze`, ярус чтения: `BEGIN READ ONLY` → `SET LOCAL statement_timeout = '<timeoutS>s'` → `SET LOCAL lock_timeout = '1s'`
   → `[search_path]` → `EXPLAIN (ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON) <sql>` → `ROLLBACK`. Подтверждения нет
   и на prod. `READ ONLY` — предохранитель и на случай ошибки яруса: попытка записи (volatile-функция, триггер) даёт SQLSTATE
   25006 → код `EXPLAIN_WRITES` с предложением выполнить как запись. На реплике — то же (там и так read-only).
   Если узел карточки — лидер и в топологии есть реплика, форма предлагает её по умолчанию с пометкой «план может отличаться».
4. `mode: analyze`, ярус записи: `ensure_mutation_allowed(profile, confirmation.confirm)` (read-only профиль — `READ_ONLY`;
   prod — имя подключения); узел в recovery — `FEATURE_UNAVAILABLE` (`errors.explain.writeOnReplica`, «выберите primary»).
   Фронтенд перед вызовом показывает `ConfirmDialog`: отношения из пробного плана и предупреждение — строки изменятся и
   откатятся; побочные эффекты откат не отменяет (сработавшие триггеры, сдвиг последовательностей, NOTIFY, внешние вызовы,
   удержанные блокировки на время выполнения). Затем `BEGIN` → `SET LOCAL …` → `EXPLAIN (ANALYZE, BUFFERS, WAL, …)` → `ROLLBACK`.
   Любая ошибка, обрыв соединения или таймаут откатывают транзакцию сервером; `ROLLBACK` явный — страховка.
5. 57014 → `TIMEOUT` (`errors.explain.timeout`, «план не получен за N с; выполнение прервано, изменений нет»); 42501 →
   `NOT_AUTHORIZED` с подсказкой о правах чтения (`pg_read_all_data`, §9.6); 42601 и прочие ошибки разбора → `INVALID_INPUT`
   с сообщением сервера и `details.position`.

Фронтенд: подвкладка «План» и консоль сначала вызывают `explain` с `mode: plan` (кнопка «План»); кнопка «План с выполнением»
по результату пробного плана либо сразу вызывает `analyze` (ярус чтения), либо открывает подтверждение (ярус записи). Ярус
результата показывается в сводке.

### 8.4 Журнал

Ярус записи пишет `ActionRecord { action: "explainAnalyze", member, prodConfirmed, outcome, code, ms, explain: { database,
tables: string[], timeoutS, sqlHead } }` (`sqlHead` — первые 200 символов). Поле `explain` необязательное, как `slot` и `patroni`;
вкладка журнала P2 показывает базу, таблицы и срок. Ярус чтения в журнал не пишется — как остальные чтения.

### 8.5 Парсер и модель плана (`pg-advisor::plan`)

`from_json(&Value) -> Result<PlanTree, PlanError>` принимает массив из одного объекта (`Plan`, `Planning Time`, `Execution Time`,
`Triggers`, `JIT`, `Settings`) или сам объект.

```ts
interface PlanTree {
  root: PlanNode; planningMs: number | null; executionMs: number | null;
  triggers: { name: string; relation: string | null; timeMs: number; calls: number }[];
  jit: { functions: number; generationMs: number; inliningMs: number; optimizationMs: number; emissionMs: number; totalMs: number } | null;
  settings: { name: string; value: string }[];
  totalMs: number | null;                                       // для долей: executionMs или включённое время корня
}
interface PlanNode {
  id: number;                                                   // порядок обхода в глубину
  nodeType: string; parentRelationship: string | null; subplanName: string | null;
  relation: { schema: string | null; name: string; alias: string | null } | null;
  indexName: string | null; strategy: string | null; joinType: string | null; parallelAware: boolean;
  startupCost: number; totalCost: number; planRows: number; planWidth: number;
  actual: { startupMs: number | null; totalMs: number | null; rows: number; loops: number } | null;   // null — без ANALYZE
  neverExecuted: boolean;
  workersPlanned: number | null; workersLaunched: number | null;
  rowsRemovedByFilter: number | null; rowsRemovedByIndexRecheck: number | null; rowsRemovedByJoinFilter: number | null;
  heapFetches: number | null;
  filter: string | null; indexCond: string | null; recheckCond: string | null; hashCond: string | null;
  joinFilter: string | null; mergeCond: string | null; sortKey: string[]; groupKey: string[];
  sortMethod: string | null; sortSpaceKb: number | null; sortSpaceType: string | null;
  hashBatches: number | null; hashBuckets: number | null; peakMemoryKb: number | null;
  exactHeapBlocks: number | null; lossyHeapBlocks: number | null;
  subplansRemoved: number | null;
  buffers: { sharedHit: number; sharedRead: number; sharedDirtied: number; sharedWritten: number; localHit: number; localRead: number;
             localDirtied: number; localWritten: number; tempRead: number; tempWritten: number } | null;
  ioReadMs: number | null; ioWriteMs: number | null;
  output: string[];
  extra: Record<string, unknown>;                               // всё нераспознанное — без потерь
  inclusiveMs: number | null; exclusiveMs: number | null; sharePct: number | null;   // actual.totalMs × loops; минус дети (≥ 0); от totalMs
  estimateRatio: number | null;                                 // actual.rows / planRows (строки на цикл, как у сервера)
  hot: boolean; misestimated: boolean;                          // три узла с наибольшим exclusiveMs (без ANALYZE — по totalCost минус дети); ratio ≥ 10 или ≤ 0.1 при max(план, факт) ≥ 100
  children: PlanNode[];
}
```

Незнакомые типы узлов и поля не ломают разбор (`extra`). `to_text(&PlanTree) -> String` — рендер в стиле psql: заголовок узла
(`Node Type [using idx] on schema.rel alias  (cost=… rows=… width=…) (actual time=… rows=… loops=…)`), строки условий, отсева,
сортировки, буферов, воркеров; в конце — Planning Time, JIT, Execution Time. Входные и выходные фикстуры — `crates/pg-advisor/tests/fixtures/plans/`.

### 8.6 Дерево в интерфейсе

`PlanTree` — таблица-дерево с отступами и развёрткой: узел (тип, отношение или индекс, стратегия, `subplanName`), время
(включённое и собственное, полоса доли), строки (оценка → факт; `misestimated` — оранжевый при ×10, красный при ×100), loops,
буферы (hit / read; temp отдельно), пометки (сброс на диск, lossy, heap fetches, воркеры меньше плана, `never executed`).
Горячие узлы выделены. `PlanNodeDetails` — панель выбранного узла со всеми полями и `extra`. `PlanSummary` — планирование,
выполнение, ярус, JIT, триггеры, settings, предупреждения. Кнопки: «JSON» (сырой, копировать), «Как текст» (копировать
`to_text`), P3b — блок рекомендаций под сводкой.

### 8.7 Консоль

`TabSpec { kind: "explain", connectionId, member?: string | null, database?: string, sql?: string }`. Поля: узел, база (список
баз каталога), SQL (`textarea` моноширинным шрифтом; редактор-зависимость не добавляем), `search_path` (свёрнутое поле),
кнопки «План» и «План с выполнением» со сроком (5 с, 15 с, 30 с, 1 мин, 5 мин, 10 мин), форма параметров при `$N`
(и флажок `GENERIC_PLAN` на 16+), режим «Вставить план» — второе текстовое поле → `parse_plan(text)`: JSON (с обрамлением
psql — `QUERY PLAN`, разделители, `+` переносов, `(N rows)` — снимается), P3b — текст. История запусков в пределах вкладки
(не сохраняется): список «№, режим, ярус, время, длительность» с возвратом к плану. «Скопировать для LLM» — запрос, план и DDL
таблиц (§7.4). Вкладка открывается из панели инструментов диагностики, из карточки и из дерева («Консоль EXPLAIN» у сервера).

---

## 9. P3b: советовать и сравнивать

### 9.1 Рекомендации по плану (`pg-advisor::plan_rules`)

`evaluate(&PlanTree) -> Vec<Recommendation>`; `Recommendation { id, severity, nodeIds, params, evidence: { label, value }[],
command: string | null, hypo: { indexDdl } | null }`. Тексты (заголовок, эффект, риски) — во фронтенде по `id` и `params`,
как у находок; команда собирается в Rust с `quote_ident`, приложение её **не выполняет** — только показывает и копирует.
Правила с пометкой «факт» требуют `actual` (ANALYZE); без него пропускаются, блок показывает «выполните план с выполнением,
чтобы проверить ещё N правил».

| id | Условие | Команда / совет | Уровень |
|---|---|---|---|
| `plan_seq_scan_filter` (факт) | Seq Scan, `(actual.rows + rowsRemovedByFilter) × loops ≥ 10 000`, `rowsRemovedByFilter ≥ 10 × actual.rows` | `CREATE INDEX CONCURRENTLY ON s.t (cols)` — колонки простых сравнений `col op const` из `filter` (регулярное выражение по `(alias.)?ident op`); не разобрано — шаблон с `<колонки фильтра>` без hypopg | warning |
| `plan_index_filter` (факт) | Index Scan / Bitmap Heap Scan, `rowsRemovedByFilter ≥ 10 × actual.rows`, `≥ 1000` | составной индекс: колонки `indexCond` + колонки `filter`; порядок — равенства раньше диапазонов | warning |
| `plan_nested_loop_seq` (факт) | Nested Loop, внутренний ребёнок — Seq Scan с `loops ≥ 1000` | индекс по колонкам ключа соединения (`joinFilter` / `filter` внутреннего узла) | warning |
| `plan_misestimate` (факт) | `estimateRatio ≥ 10` или `≤ 0.1` при `max(planRows, actual.rows) ≥ 1000` | `ANALYZE s.t`; `ALTER TABLE … ALTER COLUMN … SET STATISTICS 1000`; при ≥ 2 колонках одной таблицы в условии — `CREATE STATISTICS` | warning |
| `plan_spill` (факт) | `sortSpaceType = Disk`, `hashBatches > 1`, `lossyHeapBlocks > 0` | `SET work_mem = '<объём × 1,5, не больше 1 ГБ>'` для сессии или `ALTER ROLE … SET work_mem`; индекс под `sortKey` | warning |
| `plan_sort_limit` | Sort под Limit, `planRows` входа ≥ 100 × `planRows` Limit (факт — по `actual`) | индекс под `sortKey` (`CREATE INDEX CONCURRENTLY … (keys)`) | info |
| `plan_heap_fetches` (факт) | Index Only Scan, `heapFetches ≥ 0.1 × actual.rows × loops`, `≥ 1000` | `VACUUM s.t` (карта видимости) | info |
| `plan_no_pruning` | Append / Merge Append с ≥ 10 детьми-сканами и без `subplansRemoved`; факт: ≥ 50 % детей с `actual.rows = 0` | добавить условие по ключу партиционирования; проверить `enable_partition_pruning` | warning |
| `plan_workers_short` (факт) | `workersLaunched < workersPlanned` | `max_parallel_workers`, `max_worker_processes` | info |
| `plan_subplan_per_row` (факт) | узел с `parentRelationship = SubPlan`, `loops ≥ 1000` | переписать на JOIN / LATERAL | warning |
| `plan_jit_heavy` (факт) | `jit.totalMs > 0.5 × executionMs` | `SET jit = off` для сессии / роли или выше `jit_above_cost` | info |

`evidence` — цифры узла (строки, отсев, loops, объём, время); `hypo.indexDdl` есть у `plan_seq_scan_filter`, `plan_index_filter`,
`plan_nested_loop_seq`, `plan_sort_limit`, когда колонки разобраны. Блок `Recommendations` под сводкой плана: карточка на
рекомендацию (заголовок, доказательство, команда с копированием, эффект, риски, «Проверить гипотезу», «Показать узел» —
прокрутка и выделение в дереве).

### 9.2 hypopg

`hypo_check(id, { member, database, sql, params, searchPath, indexDdl }) -> HypoResult { indexName, estimatedBytes, costBefore,
costAfter, used, planAfter: PlanTree }` на одном соединении `for_explain()`:
`BEGIN READ ONLY` → `SET LOCAL …` → `SELECT indexrelid, indexname FROM hypopg_create_index($1)` → `SELECT hypopg_relation_size($1)`
→ `EXPLAIN (FORMAT JSON[, GENERIC_PLAN]) <sql>` → `SELECT hypopg_reset()` → `ROLLBACK`. `used` — в `planAfter` есть узел с
`indexName = indexName`. `indexDdl` проверяется: начинается с `CREATE INDEX`, без `CONCURRENTLY` (hypopg его не принимает),
одна инструкция. Расширение не установлено в базе (`pg_extension`) — кнопка выключена с подсказкой (`errors.hypo.notInstalled`,
`FEATURE_UNAVAILABLE`). `HypoCheck`: «стоимость была / стала», использован ли индекс, оценка размера, дерево «после».

### 9.3 Снимки

Файл `data_dir/snapshots/<connectionId>/<member>/<yyyy-mm-ddThh-mm-ss>.json` (`write_private`, 0600; каталоги 0700):
```ts
interface Snapshot {
  v: 1; takenAtMs: number; connection: { id: string; name: string }; member: string;
  serverVersionNum: number; pgssVersion: string; statsResetMs: number | null; dealloc: number | null;
  rows: QueryStat[];                     // все строки pgss, накопительно, без text
  texts: Record<string, string>;         // queryId → текст для 500 верхних по totalExecMs
}
```
Команды: `snapshot_take(id, member) -> SnapshotInfo`, `snapshot_list(id) -> SnapshotInfo[]` (`{ file, member, takenAtMs, rows, bytes }`),
`snapshot_delete(id, file)`, `snapshot_compare(id, from, to) -> Comparison`. Сравнение (`queries::compare`, по ключу):
`new` (есть только в `to`), `gone` (только в `from`), `degraded` (`meanExecMs` ≥ ×2 при `calls ≥ 50` в обоих), `improved`
(≤ ×0,5 при тех же условиях), `totals` обоих; `stats_reset` между снимками — предупреждение «статистика сбрасывалась, дельты
неполны». Режим топа «между снимками» — `DiagData::Queries` из дельт `to − from` (счётчик уменьшился — берётся значение `to`),
`coverageS` — разница времён, узел — узел снимка. Снимки разных узлов одного подключения сравнивать можно (primary и реплика)
с пометкой; разных подключений — нет (список показывает только текущее). `SnapshotsPanel` в инструменте: список, «Сделать
снимок», «Сравнить» (два выбранных), «Удалить», размер; `SnapshotCompare` — четыре списка с дельтами и переход в режим
«между снимками».

### 9.4 Текстовые планы (`plan::from_text`)

Тот же `PlanTree`. Отступы и `->` задают дерево; заголовок узла — `^(?<head>.+?)\s+\(cost=(\d+\.\d+)\.\.(\d+\.\d+) rows=(\d+)
width=(\d+)\)(\s+\((actual time=(\d+\.\d+)\.\.(\d+\.\d+) )?rows=(\d+) loops=(\d+)\)|\s+\(never executed\))?$`; `head` разбирается
на тип, `using idx`, `on schema.rel alias`, `Parallel`, `Partial`, стратегию в скобках (`Hash Join`, `Aggregate` — стратегия
отдельным словом); строки деталей — `Filter`, `Index Cond`, `Recheck Cond`, `Hash Cond`, `Join Filter`, `Merge Cond`,
`Rows Removed by *`, `Heap Fetches`, `Sort Key`, `Group Key`, `Sort Method: … Memory/Disk: … kB`, `Buckets: … Batches: …
Memory Usage: … kB`, `Heap Blocks: exact=… lossy=…`, `Buffers: shared hit=… read=…`, `I/O Timings`, `Workers Planned/Launched`,
`Subplans Removed`, `Output`, `Settings`, `Planning Time`, `Execution Time`, блок `JIT`, `Trigger …`; неразобранное — строкой
в `extra`. Обрамление psql и переносы `+` снимаются до разбора. Фикстуры — пары JSON / текст одного плана с `pg-perf` и матрицы
13–18; тест сравнивает форму дерева, типы узлов, cost, строки, время и буферы.

### 9.5 Расширения

- **`pg_stat_kcache`** (`pg_extension` в базе подключения): `pg_stat_kcache()` соединяется с pgss по `(queryid, userid, dbid[, top])`;
  2.2+ — `exec_user_time`, `exec_system_time`, `exec_reads`, `exec_writes` (байты); старше — `user_time`, `system_time`, `reads`,
  `writes`. Поля `cpuUserMs`, `cpuSysMs`, `diskReadBytes`, `diskWriteBytes` в `QueryStat` и `Delta`; сортировка `cpuTime`;
  колонки «CPU» и «Диск» в таблице и карточке.
- **`pg_wait_sampling`**: `SELECT event_type, event, sum(count) FROM pg_wait_sampling_profile WHERE queryid = $1 GROUP BY 1, 2`
  → `waits` с `source: "pgWaitSampling"`; `pg_wait_sampling.profile_queries = off` — подсказка в блоке; `pg_wait_sampling.profile_period`
  — в подвале блока.
Оба — только если установлены; иначе поведение P3a.

### 9.6 Готовность

Факты `ReadinessFacts`: `pgss_version: Option<String>`, `pgss_track_planning: Option<String>`, `read_all_data: bool`
(член `pg_read_all_data` или суперпользователь; на 13 роли нет — `false`). Пункты чек-листа:
- `pgssTrackPlanning` (P3a) — info при `off`: «колонки планирования пустые; `pg_stat_statements.track_planning = on`»;
- `readAllData` (P3a) — warning на 14+ без роли: «EXPLAIN запросов приложения требует права чтения таблиц: `GRANT pg_read_all_data`»;
  на 13 — info «нужны права SELECT на таблицы запросов»;
- P3b — подсказки к существующим пунктам расширений: hypopg — «для «Проверить гипотезу»», `pg_stat_kcache` — «CPU и диск
  в топе», `pg_wait_sampling` — «точные ожидания запроса».

---

## 10. IPC-контракт

| Часть | Команды |
|---|---|
| P3a | `run_diagnostic(id, request)` с `tool: "queries"` и `queries: QueriesParams` → `DiagReport` (§4.5); `start_statements_sampler(id, member: Option<String>, channel: Channel<StatementsEvent>) -> SessionId` (останов — существующий `stop_session`); `query_texts(id, member, keys: QueryKey[]) -> QueryText[]`; `query_detail(id, member, database, queryId) -> QueryDetail`; `explain(id, request: ExplainRequest) -> ExplainResult`; `parse_plan(text: String) -> PlanTree`; `query_llm_markdown(id, member, database, source: LlmSource, plan: Option<PlanTree>) -> { markdown, warnings }` |
| P3b | `explain` возвращает `recommendations`; `parse_plan` принимает текст; `hypo_check(id, request: HypoRequest) -> HypoResult`; `snapshot_take(id, member) -> SnapshotInfo`; `snapshot_list(id) -> SnapshotInfo[]`; `snapshot_delete(id, file) -> bool`; `snapshot_compare(id, from, to) -> Comparison` |

`member: None` у `start_statements_sampler` — «за лидером» на момент подписки, как у `start_sampler`. Все команды возвращают
`Result<T, AppError>`; JSON — camelCase; `Option` → `null`; теги — `kind`. Типы фронтенда повторяют структуры
`pg_advisor::{queries, plan, plan_rules}` и `src-tauri/src/{diag/queries, diag/query_detail, explain, snapshots}`. Новых событий
Tauri нет: данные сэмплера идут каналом подписки. `queryId` везде — строка.

---

## 11. Ошибки

| code | Причина | Поведение UI |
|---|---|---|
| `EXPLAIN_WRITES` (новый, P3a) | SQLSTATE 25006 в ярусе чтения: запрос, признанный чтением, пытается писать (`errors.explain.writes`) | Сообщение и кнопка «Выполнить как запись» → ярус записи с подтверждением |
| `FEATURE_UNAVAILABLE` | Ярус записи на реплике (`errors.explain.writeOnReplica`); hypopg не установлен (`errors.hypo.notInstalled`); `GENERIC_PLAN` на < 16 (`errors.explain.genericUnsupported`); pgss исчез у сэмплера | Подсказка у кнопки; сэмплер — событие `error` и остановка |
| `NOT_AUTHORIZED` (42501) | Нет прав на таблицы запроса при EXPLAIN (`errors.explain.notAuthorized`, подсказка `pg_read_all_data`) | Сообщение с подсказкой; пункт готовности `readAllData` |
| `TIMEOUT` (57014) | EXPLAIN ANALYZE не уложился в срок (`errors.explain.timeout`); пробный план — в 15 с (`errors.explain.planTimeout`) | «Выполнение прервано, изменений нет»; предложить больший срок |
| `INVALID_INPUT` | Вторая инструкция (`errors.explain.oneStatement`), число параметров (`paramsCount`), синтаксическая ошибка сервера 42601 и ошибки разбора (`errors.explain.server` с `params.message`, `details.position`), пустой запрос, срок вне 5…600, сортировка вне версии (`errors.queries.sortUnavailable`), отчёт без `queries`, `indexDdl` не `CREATE INDEX` (`errors.hypo.badDdl`), снимки разных подключений (`errors.snapshots.foreign`) | Сообщение у формы; позиция ошибки подсвечивается в консоли |
| `READ_ONLY`, `CONFIRMATION_REQUIRED` | Guard яруса записи (как P2) | Кнопки нет / диалог подтверждения |
| `NOT_FOUND` | Файл снимка исчез (`errors.snapshots.notFound`); запроса с таким `queryId` в pgss уже нет (`errors.queries.gone`) | Список перечитывается; карточка показывает последнее известное с пометкой |
| `NODE_UNAVAILABLE` | Узел недоступен или подключение закрыто за время EXPLAIN | Как в P2 |
| `RECOVERY_CONFLICT` | EXPLAIN ANALYZE на реплике отменён конфликтом recovery | Как в P2, `retryable` |

Отсутствие pgss, прав на чужие строки, `query_id` на 13 — не ошибки, а `unavailable` отчёта. Текст плана, который не удалось
разобрать (`parse_plan`), — `INVALID_INPUT` (`errors.plan.unparsed`, `params.line`) с показом строки.

---

## 12. Структура репозитория (новое)

```
crates/pg-advisor/src/  pgss.rs  queries/{mod,rules}.rs  plan/{mod,json,text_out}.rs
                        P3b: queries/compare.rs  plan/text_in.rs  plan_rules/{mod, seq_scan, index_filter, …}.rs
crates/pg-advisor/tests/fixtures/plans/{json,text}/  (PG 13–18, по правилу — положительная и отрицательная)
src-tauri/src/   sampler/statements.rs  diag/{queries,query_detail}.rs  explain/{mod,run,params,tier,llm}.rs
                 commands/{queries,explain}.rs   P3b: explain/hypopg.rs  snapshots/{mod,store,compare}.rs  commands/snapshots.rs
src-tauri/tests/ it_queries.rs  it_explain.rs   P3b: it_hypopg.rs  it_snapshots.rs
src/components/queries/  QueriesView QueryTab QueryOverview QueryPlan ExplainTab
                         plan/{PlanTree,PlanNodeDetails,PlanSummary,ParamsForm,AnalyzeConfirm}
                         P3b: Recommendations HypoCheck SnapshotsPanel SnapshotCompare
src/lib/  useStatementsSampler.ts  useQueryDetail.ts  useExplain.ts
dev/perf/  Dockerfile  20-init.sql  heavy.sql  README.md
docs/guide/{en,ru}/queries.md
```

---

## 13. Тестирование

### 13.1 Стенд: сервис `pg-perf`

`dev/perf/Dockerfile` — `FROM postgres:17`, пакеты PGDG `postgresql-17-hypopg`, `postgresql-17-pg-stat-kcache`,
`postgresql-17-pg-wait-sampling`; init-скрипты внутри образа (как `pg-lab`). Сервис `pg-perf`, контейнер `gp-pg-perf`,
`127.0.0.1:5471`, образ `glafira-pg-dev/perf:17`, параметры командой: `shared_preload_libraries=pg_stat_statements,pg_stat_kcache,pg_wait_sampling`,
`pg_stat_statements.track_planning=on`, `pg_stat_statements.max=5000`, `track_io_timing=on`, `compute_query_id=on`,
`work_mem=1MB` (сбросы на диск), `max_parallel_workers=2` при `max_parallel_workers_per_gather=4` (воркеров меньше плана),
`jit_above_cost=1000`, `jit_inline_above_cost=1000`, `jit_optimize_above_cost=1000` (JIT на простых запросах).

`20-init.sql`: роли `glafira_diag` (`pg_monitor`, `pg_signal_backend`, `pg_read_all_data`) и `glafira_app` с паролями стенда;
база `perf` с расширениями; схема `shop`: `customers` (200 тыс.), `orders` (1 млн, `status` без индекса), `order_items`
(3 млн), `events` — партиционирована по месяцам (24 партиции, 2 млн строк), `audit` с `autovacuum_enabled = off` и скошенными
данными (ошибка оценки), `balances` с миллионом обновлённых строк без VACUUM (heap fetches). `heavy.sql` — сценарий `pgbench`
с `\set` случайных параметров: точечный выбор клиента (N+1), фильтр по `orders.status` (seq scan с отсевом), соединение с
`audit` (nested loop по неверной оценке), сортировка заказов без индекса (сброс), `ORDER BY … LIMIT` без индекса, Index Only
Scan по `balances`, коррелированный подзапрос на строку, выбор из `events` без ключа партиционирования, тяжёлая агрегация
(параллельность и JIT), запрос с temp-файлами. Запуск (в `dev/perf/README.md`):
`docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300`. Поднимается
`docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf`; существующие контейнеры не трогаются.

### 13.2 Автотесты

**`pg-advisor` (unit):** `PgssCaps` по версиям 1.8–1.12 и строкам вида `1.10`; эвристики §6 на границах порогов и по режимам
(stddev только с момента сброса); доли и hit ratio; `plan::from_json` на фикстурах PG 13–18 (seq scan, index scan, hash join,
nested loop, параллельный план с воркерами, CTE и InitPlan, Append по партициям, Sort на диске, JIT, триггеры, план без ANALYZE,
`never executed`) — числа узлов, собственное время, горячие узлы, ошибка оценки, `extra`; `to_text` — снимки вывода.
P3b: каждое правило — положительная и отрицательная фикстура; `from_text` на парах JSON / текст; `compare` (новые, исчезнувшие,
деградировавшие, ускорившиеся, сброс между снимками).

**`src-tauri` (unit):** `StatementsBuffer` — дельты, первый тик без дельт, вытеснение ключа, смена `stats_reset`, предел записей
(вытеснение целыми тиками), суммы по окнам и покрытие; `explain::tier` — `ModifyTable` в корне и в CTE, `CREATE`, комментарии
перед первым словом; `explain::params::count`; `llm` — состав markdown и пределы; SQL по `PgssCaps` — снимки текста запросов
(`snapshots/diag-sql/`, как P2); `ActionRecord` с `explain`; P3b: формат файла снимка, права 0600, листинг, удаление.

**Интеграция** (`GPG_IT_PERF=localhost:5471`, по умолчанию `#[ignore]`, как матрица `GPG_IT_MATRIX`):
- `it_queries`: отчёт «с момента сброса» под нагрузкой — строки, итоги, тексты по ключам, `Unavailable{others}` под ролью без
  `pg_read_all_stats`; сэмплер — два тика, дельты, окно; `query_detail` — `running` на 14+, разбивка; на матрице 13–18 — SQL
  по версиям pgss выполняется (`pg_stat_statements` есть на `gp-pg13..18`), на 13 — `unavailable: running`.
- `it_explain`: пробный план и отношения; `GENERIC_PLAN` на 16+ и `INVALID_INPUT` на 15; значения параметров `unknown`;
  ANALYZE SELECT без подтверждения и без строки журнала; UPDATE — ярус записи, без `confirm` на prod → `CONFIRMATION_REQUIRED`,
  с подтверждением — выполнен и откатан (число строк не изменилось), строка журнала `explainAnalyze`; volatile-функция с записью
  в ярусе чтения → `EXPLAIN_WRITES`; две инструкции → `INVALID_INPUT`; `pg_sleep` сверх срока → `TIMEOUT`; read-only профиль →
  `READ_ONLY` на ярусе записи, ярус чтения работает; на реплике (`gp-pg-replica`) ярус записи → `FEATURE_UNAVAILABLE`.
- P3b `it_hypopg`: индекс-гипотеза снижает стоимость и используется; `hypopg_reset` оставляет сессию чистой; без расширения —
  `FEATURE_UNAVAILABLE`. `it_snapshots`: снять два снимка под нагрузкой, сравнить, режим «между снимками».

**Фронтенд (vitest):** `QueriesView` — режимы, покрытие, пустое окно, серые сортировки, `unavailable`, открытие карточки;
`QueryTab` — обзор без буфера, таблицы после плана, выбор источника текста; `PlanTree` — развёртка, горячие узлы, пометки
ошибки оценки; `ExplainTab` — форма из `$N`, `GENERIC_PLAN` по версии, вставка JSON с обрамлением psql, подтверждение яруса
записи; `findingText` для новых находок; `pnpm i18n:check`.

**Смоук GUI** на `pg-perf` под `heavy.sql` — чек-лист в плане (`docs/smoke/`): топ в обоих режимах, карточка, план и ANALYZE
обоих ярусов, консоль, «для LLM»; P3b — рекомендации по каждому сценарию, hypopg, снимки.

### 13.3 CI

Новых нативных зависимостей нет (`pg_query` не берётся): сборка на трёх ОС не меняется. `pg-advisor` и unit-тесты `src-tauri` —
в CI; интеграционные — локально на стенде. Фикстуры планов — в репозитории.

---

## 14. Документация

- `docs/guide/{ru,en}/queries.md` (новая глава, `GUIDE_CHAPTERS`): топ и режимы, покрытие окна и неполный топ, карточка,
  план и дерево, ярусы EXPLAIN ANALYZE и что откат не отменяет, консоль и вставка плана, «для LLM»; P3b — рекомендации
  (команды приложение не выполняет), hypopg, снимки, расширения. F1 из инструмента и вкладок `query` / `explain`.
- `docs/admin.md`: Права — `pg_read_all_stats` для чужих строк, pgss в базе подключения, `track_planning`, `pg_read_all_data`
  для EXPLAIN, права записи для яруса записи; Нагрузка — одно чтение pgss в минуту на узел (без текстов), тексты по
  запросу, EXPLAIN на отдельном соединении; Действия — ярусы EXPLAIN ANALYZE, что пишется в журнал; Данные и логи — снимки
  (путь, 0600, удаление из приложения).
- `dev/perf/README.md` — сервис, нагрузка, роли.
- `docs/guide/*/diagnostics.md` — ссылка на новую главу; `CHANGELOG.md` — при релизе.

---

## 15. Отступления от концепции

1. Без `pg_query`: затронутые таблицы — из плана, число параметров — по `$N`; парсер придёт в P4.
2. EXPLAIN ANALYZE запроса-чтения — без подтверждения и без имени подключения на prod: защищает транзакция `READ ONLY`;
   подтверждение и имя на prod — только у яруса записи.
3. «Взять из текущего выполнения» — не извлечение литералов, а выбор текста выполняющегося экземпляра целиком.
4. Текст запроса не переформатируется (нужен парсер): подсветка ключевых слов средствами CSS.
5. Сэмплер pgss живёт, пока открыт потребитель (концепция о жизненном цикле молчит); буфер — 200 000 записей дельт, а не строк.
6. Текстовый парсер планов и режим «между снимками» — в P3b; `pg_qualstats` — после MVP.
7. У сэмплера pgss нет своего соединения: одно чтение в минуту делит соединение `Sampler` с сэмплером активности.
8. Деления Free / Pro нет (§1, «Лицензия»).

---

## 16. Критерии готовности

**P3a:**
1. На `pg-perf` под `heavy.sql` топ показывает режим «с момента сброса» сразу и «за окно 5 мин» через пять минут с верным
   покрытием; находки `query_n_plus_one`, `query_temp`, `query_low_hit_ratio` появляются на своих запросах сценария.
2. На `pg-single` (без pgss) инструмент показывает `unavailable` с подсказкой; на матрице 13–18 отчёт строится по версии pgss,
   недоступные колонки серые.
3. Карточка открывается из строки и из находки, показывает историю по тикам, разбивку, выполняющиеся экземпляры (14+) и
   ожидания при открытой вкладке «Сессии»; после первого плана — таблицы со ссылками.
4. План параметризованного запроса — через `GENERIC_PLAN` на 17 и через форму значений на 15; план по тексту выполняющегося
   экземпляра.
5. ANALYZE SELECT выполняется без подтверждения в `READ ONLY`; UPDATE требует подтверждения (и имени подключения на prod),
   откатывается, оставляет строку журнала; на read-only профиле ярус записи недоступен; срок прерывает выполнение с `TIMEOUT`.
6. Консоль разбирает вставленный JSON-план psql; «для LLM» даёт markdown с запросом, статистикой, планом и DDL таблиц.
7. Закрытие последней вкладки-потребителя останавливает чтения pgss; повторное открытие продолжает буфер с промежутком.
8. Строки — en и ru, `pnpm i18n:check` зелёный; глава открывается по F1; CI зелёный на трёх ОС.

**P3b:**
1. Каждое из 11 правил срабатывает на своей фикстуре и на своём запросе `heavy.sql` и молчит на отрицательной фикстуре.
2. «Проверить гипотезу» показывает стоимость «была / стала», использование индекса и размер; сессия после проверки чиста.
3. Снимок снимается и хранится с правами 0600; два снимка сравниваются; режим «между снимками» строит топ.
4. Текстовые планы psql с PG 13–18 разбираются в то же дерево, что JSON.
5. На `pg-perf` колонки CPU и диск заполнены, ожидания карточки берутся из `pg_wait_sampling`; без расширений поведение P3a.
6. Подсказки готовности к `track_planning`, `pg_read_all_data` и расширениям верны для `glafira_diag`; CI зелёный на трёх ОС.
