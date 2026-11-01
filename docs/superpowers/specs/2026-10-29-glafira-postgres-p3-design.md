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
   буферами; консоль `explain` с запросом или вставленным планом (JSON или текст psql / auto_explain).
7. Рекомендации по плану (11 правил §6.3 концепции) с доказательством, командой, эффектом и рисками; «Проверить гипотезу» через
   hypopg (стоимость «была / стала», использован ли индекс, оценка размера).
8. Снимки pgss на диске (`snapshots/<подключение>/<узел>/…json`, 0600), сравнение (новые, исчезнувшие, деградировавшие,
   ускорившиеся), режим топа «между снимками».
9. Парсер текстовых планов (psql / auto_explain) в тот же `PlanTree`. *Перенесён из P3b в P3a (решение владельца 2026-11-01, §15, «Поправки по смоуку P3a»).*
10. Опциональные расширения: `pg_stat_kcache` (CPU и диск в топе и карточке), `pg_wait_sampling` (точный профиль ожиданий
    запроса); подсказки готовности к расширениям и к `track_planning`; пункт готовности о правах чтения для EXPLAIN.
11. Руководство: глава «Запросы»; `docs/admin.md` — права, нагрузка сэмплера, ярусы EXPLAIN ANALYZE, снимки.

**План реализации делится на две части**, каждая заканчивается работающим приложением (как P2a / P2b):
- **P3a «Видеть»:** пункты 1–6, 9, 11 (без разделов о рекомендациях, снимках и расширениях). Ветка `feat/p3a`.
- **P3b «Советовать и сравнивать»:** пункты 7, 8, 10 и остаток пункта 11. Ветка `feat/p3b` после релиза P3a.

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
   **Решение владельца (2026-11-01):** парсер текстовых планов переносится из P3b в P3a; консоль P3a принимает SQL, JSON и текст (psql, auto_explain).
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
  `to_text(&PlanTree)`; `plan/text_in.rs` — `from_text(&str)` и диспетчер `json::from_any(&str)` (JSON или текст; перенесены из P3b, §9.4); P3b: `plan_rules/` — 11 правил и `Recommendation` (§9.1).
- `findings.rs` — новые варианты цели и ссылки.

**`src-tauri/src`**:
- `sampler/statements.rs` — `StatementsSource` (чтение pgss по `PgssCaps`), `StatementsBuffer` (дельты по тикам),
  `StatementsEvent`; `sampler/registry.rs` обобщается трейтом `Sampling`.
- `diag/queries.rs` — SQL по `PgssCaps`, режимы, тексты и их кэш, сборка `DiagData::Queries`.
- `diag/query_detail.rs` — карточка: накопительные и оконные метрики, история, разбивка, выполняющиеся экземпляры, ожидания.
- `explain/mod.rs` (`ExplainRequest`, `ExplainResult`), `explain/run.rs` (пробный план → ярус → выполнение), `explain/params.rs`
  (счётчик `$N`, привязка значений), `explain/tier.rs` (определение яруса по дереву и первому слову), `explain/llm.rs` (данные
  для «Скопировать для LLM» — `LlmBundle`, §7.4; markdown собирает фронтенд, `src/lib/llmMarkdown.ts`); P3b: `explain/hypopg.rs`.
- P3b: `snapshots/{mod,store,compare}.rs`.
- `commands/queries.rs` (`query_texts`, `start_statements_sampler`, `query_detail`, `query_llm_bundle`), `commands/explain.rs`
  (`explain`, `parse_plan`); P3b: `commands/snapshots.rs`.

**Фронтенд** (`src/components/queries/`): `QueriesView` (инструмент в `DiagTab`), `QueryTab` → `QueryOverview`, `QueryPlan`;
`ExplainTab`; `plan/PlanTree`, `plan/PlanView`, `plan/PlanRunner` (параметры, ярусы, подтверждение, история запусков),
`plan/PlanNodeDetails`, `plan/PlanSummary`, `plan/ParamsForm`, `plan/AnalyzeConfirm`, `plan/planText`; `Sparkline`,
`CopyLlmButton`; P3b: `Recommendations`, `HypoCheck`, `SnapshotsPanel`, `SnapshotCompare`. Хуки `src/lib/useStatementsSampler.ts`
(по образцу `useSampler`), `useQueryDetail.ts`, `useExplain.ts`, `llmMarkdown.ts` (markdown из `LlmBundle`). Типы —
`src/api/types.ts`, вызовы — `src/api/client.ts`.

### 3.2 Поток данных

1. Вкладка «Запросы» подписывается на сэмплер pgss узла (`start_statements_sampler`) и запрашивает отчёт `run_diagnostic`
   с `tool: queries`. Режим «с момента сброса» — одно чтение сервера на отчёт; «за окно» — отчёт собирается из буфера без
   обращения к серверу и обновляется по событию `tick` (раз в минуту) и при смене окна, сортировки или фильтра.
2. Строки отчёта несут тексты из кэша; для строк без текста вкладка вызывает `query_texts` одним пакетом, ответ попадает в кэш.
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

`PgssCaps` отдаёт версию и флаги `toplevel`, `info`, `jit`, `temp_io_time`, `shared_io_names` (в нём же `local_blk_*_time` и
`jit_deform_time` версии 1.11), `stats_since`, `parallel`; версия ниже 1.8 — `Unavailable{part: statements, reason: «pg_stat_statements старше 1.8»}` (PG 12 не поддерживается
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

Закрытие и переоткрытие подключения, исчезновение участника — как в P2 §3.4: сэмплер pgss узла останавливается, буфер и кэш
текстов удаляются, поколение открытия защищает от запоздавших ответов. Вкладки `query` и `explain` при закрытии подключения
показывают заглушку «подключение закрыто» и переподписываются после «Подключиться».

---

## 4. Топ запросов (`queries`, узел)

### 4.1 Источник

Расширение ищется в базе подключения: `SELECT extversion, nspname FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
WHERE extname = 'pg_stat_statements'`. Нет строки — `Unavailable{part: statements}` с пометкой «pg_stat_statements не установлен в этой базе (`CREATE EXTENSION pg_stat_statements`)»
(пункты готовности `pgssPreloaded` и `pgssInstalled` P1 уже красные). Функция и представление вызываются по найденной схеме.

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
       <s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time | NULL::float8>,
       <s.parallel_workers_to_launch, s.parallel_workers_launched | NULL::bigint, NULL::bigint>
FROM <ns>.pg_stat_statements(false) s
LEFT JOIN pg_database d ON d.oid = s.dbid
LEFT JOIN pg_roles r ON r.oid = s.userid
```

`jit_deform_time` (1.11+) в сумму не входит: он уже включён в `jit_generation_time`.

Отдельно: `dealloc`, `stats_reset` из `pg_stat_statements_info` (1.9+; на 1.8 — `NULL`), `current_setting('pg_stat_statements.max')`,
`current_setting('pg_stat_statements.track_planning', true)`. Тексты не читаются (`showtext := false`): файл текстов не трогается.
Чужие строки доступны роли с `pg_read_all_stats` (`pg_monitor` включает её); без неё pgss отдаёт строки с `queryid = NULL` и
текстом `<insufficient privilege>` — такие строки отбрасываются, отчёт получает `Unavailable{part: others, reason: «нужна роль
pg_read_all_stats»}`.

### 4.2 Режимы

- **С момента сброса** — одно чтение; сортировка, обрезка до `limit` и итоги по всему набору (`totals`) — в Rust.
  `since.statsResetMs` и `since.dealloc` — в шапке.
- **За окно 5 / 15 / 60 мин** — сумма дельт буфера сэмплера (§5) по тикам в окне `[последний тик − W, последний тик]`.
  Покрытие `coverageS` — число тиков окна с дельтами × интервал (60 с), не больше окна: считаются реально собранные минуты, а промежуток,
  когда сбор стоял, в покрытие не входит («накоплено N из 60 мин»; решение владельца 2026-11-01, §15 п. 12). `ticks` — число тиков окна, у которых была база для
  дельт: самый первый тик сэмплера — только база (`ticks: 0`), тик сразу после промежутка тоже только база и сам не считается,
  поэтому в его событии `ticks` — число прежних тиков часа (например, `ticks: 1, changed: 0`: окно часа ещё держит прошлый
  тик). Тиков в окне нет — `rows` пустые, `coverageS: null`, вкладка показывает «идёт первый тик сэмплера».
  Среднее — `totalExecMs / calls` за окно; **min / max / stddev за окно недоступны** (накопительные счётчики не дельтируются) —
  колонки серые, сортировки по ним выключены.
- **Между снимками** (P3b, §9.3) — дельты между двумя файлами снимков того же подключения.

Фильтры: база (`database: null` — все), только верхний уровень (`toplevelOnly`, по умолчанию `true`; флажок виден всегда, на 1.8
колонки `toplevel` нет — в SQL она константа `true`, и фильтр ничего не меняет).

**Собственные запросы Глафиры.** Периодические запросы приложения (опрос топологии, сэмплеры, отчёты диагностики и карточки) начинаются
с комментария-метки `/* glafira */` (`product::SQL_MARKER`, макрос `marked_sql!`, `is_marked_sql`): `pg_stat_statements` хранит текст с
комментарием, а `queryid` его не учитывает. Отчёт узнаёт «свои» строки по тексту (кэш текстов узла, §4.6; для кандидатов N+1 без
текста — чтение тех же ключей, что у `texts`) и ставит `QueryStat.own = true`. Свои строки остаются в таблице, но не участвуют в
правилах находок (§6) и не входят в `totals`, от которых считаются доли (`sharePct`, доля вызовов N+1); запросы EXPLAIN и консоли
(SQL пользователя) метки не несут. Строка, которая и не кандидат N+1, и без текста в кэше, опознаётся после того, как её текст
попал в кэш (долей это касается слабо: следующий отчёт может сдвинуть их немного).

**Поиск.** Параметр `search` (подстрока текста запроса без учёта регистра; пустая строка или одни пробелы — без фильтра). С непустым
`search` один запрос `pg_stat_statements(true)` с `query ILIKE '%' || $1 || '%'` (значение экранируется: `%`, `_`, `\`) возвращает
ключи, и строки отчёта обоих режимов ограничиваются ими до сортировки и обрезки. `totals` по-прежнему по всей нагрузке (доли — от
неё). Ошибка поиска — ошибка отчёта.

### 4.3 Сортировки

`QuerySort`: `totalTime` (по умолчанию), `meanTime`, `maxTime`, `stddevTime`, `calls`, `rowsPerCall`, `sharedRead`, `hitRatio`
(по возрастанию — худшие сверху), `tempWritten`, `walBytes`, `ioTime`, `planTime`, `jitTime`; P3b — `cpuTime`. Остальные —
по убыванию. Сортировка, недоступная по версии pgss или режиму, в интерфейсе серая с подсказкой; запрос с такой сортировкой
отвечает `INVALID_INPUT` (`errors.queries.sortUnavailable`). Времена в таблице топа и в дереве плана форматирует `formatMs`:
`0` → «0 мс», значение между 0 и 0,01 мс → «<0,01 мс» (§8.6).

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
  own: boolean;                                         // запрос самой Глафиры (метка `/* glafira */`): вне находок и итогов
  text: string | null; textTruncated: boolean;          // из кэша; null — ещё не запрашивался
  cpuUserMs?: number | null; cpuSysMs?: number | null; diskReadBytes?: number | null; diskWriteBytes?: number | null; // P3b
}
```

### 4.5 Запрос и отчёт

```ts
interface QueriesParams {
  mode: { kind: "sinceReset" } | { kind: "window"; s: 300 | 900 | 3600 } | { kind: "between"; from: string; to: string };
  sort: QuerySort; limit: 50 | 100 | 200; database: string | null; toplevelOnly: boolean;
  search?: string | null;                               // подстрока текста, без учёта регистра; пусто — без фильтра
}
// DiagRequest: { tool: "queries", member, queries: QueriesParams }
interface QueriesData {
  kind: "queries"; mode: QueriesParams["mode"]; coverageS: number | null; ticks: number | null;
  pgss: { version: string; max: number; trackPlanning: boolean; hasInfo: boolean; statsResetMs: number | null; dealloc: number | null };   // hasInfo: есть pg_stat_statements_info (1.9+), иначе statsResetMs и dealloc — null
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
Кэш на узел (`HashMap<QueryKey, Option<(Arc<str>, bool)>>`; `None` — «текста нет»: у строки pgss он `NULL`, и карточка не должна
перечитывать файл текстов на каждом опросе) — до 2000 записей вместе с отметками, при переполнении очищается целиком; живёт до
закрытия подключения. Вкладка запрашивает тексты только для видимых строк без текста, одним вызовом на отчёт.

### 4.7 Интерфейс

Инструмент в `DiagTab` рядом с остальными; пункт «Запросы» в дереве у сервера, узлов и кластера (`DiagnosticsNodes`).
Шапка: узел и «за лидером» (как у P2), переключатель режима (5 / 15 / 60 / с момента сброса; P3b — между снимками), значок
покрытия («накоплено 12 из 15 мин», «идёт первый тик»), фильтр базы, флажок `toplevel`, предел строк, значок «топ неполный»
при `dealloc`. Таблица: первая строка текста моноширинным шрифтом с многоточием (полный текст — во всплывающей подсказке),
полоса доли, метрики по сортировкам; клик по заголовку сортирует; клик по строке открывает вкладку запроса; контекст —
«Открыть в консоли». Панель находок — общая (`FindingsPanel`); ссылка `OpenQuery` открывает карточку; находка о запросе показывает начало его текста
(моноширинным, до 90 символов, полный — в подсказке; текст берётся из строк отчёта по `queryId` и базе, нет текста — `queryId`). В панели фильтров —
поле «Поиск» (задержка ввода около 300 мс, параметр `search`, §4.2; пустой результат — «Ничего не найдено по «…»»); строки с `own` помечены
«Глафира» (подсказка «Запрос самой Глафиры: не участвует в находках»); неактивная кнопка строки без `datname` (ключа уже нет в
`pg_stat_statements`) в подсказке называет причину. Версия pgss и `stats_reset` — в подвале; на pgss без `pg_stat_statements_info`
(`hasInfo = false`) вместо времени сброса — «время сброса недоступно (нужен pg_stat_statements 1.9+)»; при выключенном `track_planning` колонка
«Планирование» таблицы и метрика карточки показывают «—». *Реализовано иначе (§15, п. 18):* значка «топ неполный» нет — о вытеснении говорит находка
`queries_incomplete`; доля — колонка с числом, а не полоса; карточку открывает клик по тексту запроса (не по всей строке), меню строки
«Открыть в консоли» нет — консоль открывает кнопка в шапке инструмента; во всплывающей подсказке полный текст есть.

---

## 5. Сэмплер pg_stat_statements

- **Запрос** раз в 60 с — SQL §4.1 без текстов плюс `dealloc` / `stats_reset`; время тика — часы приложения, как в P2.
  Первый тик после запуска — только база (дельт нет); дельты появляются со второго.
- **Буфер** (`StatementsBuffer`): `latest: HashMap<QueryKey, Counters>` (последние накопительные значения) и кольцо тиков
  `Tick { atMs, dealloc, deltas: Vec<(QueryKey, Delta)> }`, где в тик попадают только ключи, у которых изменились `calls`
  или `total_exec_time`. `Delta` — аддитивные счётчики (`calls`, времена выполнения и планирования, `plans`, `rows`, 12 счётчиков
  блоков, времена I/O, WAL, JIT, параллельные воркеры; P3b — kcache). Ключ, у которого накопительные счётчики уменьшились
  (вытеснен и вернулся), получает новую базу: дельта тика равна его новому накопительному значению. Смена `stats_reset` —
  новая база для всех ключей тем же правилом, прошлые тики остаются (их дельты верны); событие `gap` не шлётся. На тике сброса
  `dealloc_delta` равен текущему значению `dealloc` (сброс обнулил его вместе со статистикой — то же правило, что у счётчиков).
  Хранение — 60 мин; предел — 200 000 записей `(ключ, дельта)` на узел (порядка 50 МБ), при переполнении вытесняются целые
  старые тики. Окно считается суммой дельт по тикам в `[последний − W, последний]`. Имена `datname` / `rolname` ключей хранятся отдельной картой
  (`tick_names`, счётчик ссылок по тикам окна) и живут, пока на ключ ссылается хотя бы один тик: строка ключа, которого уже нет в `latest`
  (сброс, вытеснение), сохраняет базу и роль, и карточка открывается. Покрытие окна — число тиков с дельтами × интервал, не больше окна (§4.2).
- **Жизнь** — как у сэмплера активности (P2 §6): узел создаётся первой подпиской `start_statements_sampler(id, member, channel)`,
  задача останавливается с последней отпиской, буфер живёт до закрытия подключения; повторная подписка продолжает буфер,
  промежуток записывается как `gap` и показывается на истории карточки. Подписчики — инструмент «Запросы» и вкладки `query`
  того же узла. «Запросы» «за лидером» (`member = None`) подписываются на узел последнего настоящего отчёта, а не на `None`: Rust
  разрешает `None` в лидера один раз, на момент подписки, — и переподписываются, когда отчёт называет другого лидера. Перезагрузка вебвью останавливает сессии; вкладки подписываются заново при монтировании.
- **Нагрузка и сбои.** Одно чтение в минуту на соединении пула `Sampler` (§3.4); предел запроса 5 с (`on_catalog_node_split`),
  таймаут и обрыв закрывают соединение. Ошибка — паузы 1, 2, 5, 10, 30 с и событие `error`; `AUTH_FAILED` — остановка (и фронтенд после неё не переподписывается и не
  перечитывает отчёт и карточку, пока подключение не открыто заново, — правило lockout P2);
  при исчезновении расширения на ходу источник повторяет `detect` и останавливает сэмплер событием `error` с `FEATURE_UNAVAILABLE`;
  смена схемы или версии расширения — повтор чтения с новыми `PgssCaps`.
- **Канал** `StatementsEvent`: `tick { atMs, coverageS, ticks, changed }` (`changed` — число ключей с дельтами; `ticks` и `coverageS` — как в §4.2), `gap { fromMs, toMs }`,
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

Строки с `own` (§4.2) ни в одном построчном правиле не участвуют, а доля вызовов N+1 считается от итогов без своих строк; правило
`queries_incomplete` от `own` не зависит. Параметры находки — цифры условия по единицам (`calls`, `callsSharePct`, `meanMs`, `stddevMs`, `hitRatioPct`, `readBytes`,
`tempBytes`, `planMs`, `execMs`, `dealloc`, `max`); текст — в словарях (`findings.<id>.title` и `.fix`). `query_planning_heavy` — Info: подсказка, а не предупреждение. Строка pgss с
`datname = NULL` (база удалена) даёт цель с `database: ""`, и кнопка «Открыть запрос» у такой находки не показывается. Критических находок у топа нет: это подсказки «куда смотреть», а не аварии.
Сортировка и слияние — `grade` / `sort` P2.

---

## 7. Карточка запроса

### 7.1 Вкладка

`TabSpec { kind: "query", connectionId, member, database, queryId, title? }`. Заголовок — `queryTitle(text, queryId)`: начало
текста одной строкой (до 60 символов), пока текст не получен — `queryId`; вкладка живёт, как остальные вкладки, до закрытия
приложения (workspace между запусками не сохраняется). Узел пропал из топологии — карточка показывает заглушку с выбором
другого узла и ничего не опрашивает. У одиночного сервера (`topology.kind = "server"`) переключателя узла в шапке нет (§15, п. 18). Личность запроса — `queryid` в пределах базы: строки pgss по ролям и `toplevel`
суммируются, разбивка показывается отдельной таблицей. Идентификатор вкладки содержит узел, поэтому переключатель узла в
шапке не меняет `member` вкладки, а открывает новую вкладку `query` для выбранного узла: на физических репликах `queryid`
совпадает (OID реплицируются), и поведение запроса на primary и реплике можно сравнить рядом.

### 7.2 Данные

`query_detail(id, member, database, queryId) -> QueryDetail`:

```ts
interface QueryDetail {
  text: string | null; textTruncated: boolean;
  sinceReset: QueryStat | null;                                  // сумма строк pgss с этим queryid в базе
  windows: { windowS: 300 | 900 | 3600; stat: QueryStat | null }[];   // из буфера сэмплера; null — у запроса в окне нет дельт
  totals: { calls: number; totalExecMs: number; rows: number };  // итоги всего представления pgss (все базы и уровни) — для долей и правил
  byUser: { rolname: string | null; toplevel: boolean; stat: QueryStat }[];
  history: { atMs: number; calls: number; totalExecMs: number; meanExecMs: number }[];   // по тикам за 60 мин
  gaps: { fromMs: number; toMs: number }[];
  running: RunningInstance[];                                    // 14+; на 13 — unavailable: running
  waits: QueryWaits | null;                                      // null — буфера активности нет
  findings: Finding[];                                           // правила §6 по sinceReset (без queries_incomplete), доли — от totals
  unavailable: Unavailable[];
  serverVersionNum: number; inRecovery: boolean; pgssVersion: string | null;
}
interface RunningInstance {
  pid: number; backendStartUs: number; state: string | null; startedMs: number | null; durationS: number | null;
  waitEventType: string | null; waitEvent: string | null; usename: string | null; applicationName: string | null;
  clientAddr: string | null; query: string; truncated: boolean;  // truncated: срез по track_activity_query_size или QUERY_LIMIT (§7.3)
}
interface QueryWaits {
  source: "sampler" | "pgWaitSampling"; sinceMs: number; total: number;
  items: { eventType: string; event: string | null; count: number; sharePct: number }[];
}
```

- `running` — `pg_stat_activity WHERE query_id = $1 AND datname = $2 AND state = 'active' AND leader_pid IS NULL` (с `backend_start`
  для адресации сессии, как в P2; подробности отбора — §7.3).
- `waits` — сэмплы буфера активности этого узла за 60 мин с `query_id = $1`, сгруппированные по типу и событию; активные без
  ожидания считаются как `CPU`. Нет буфера (вкладка «Профиль ожиданий» не открывалась: сэмплер активности запускает только она) — `null`, карточка подсказывает открыть её.
  P3b: при установленном `pg_wait_sampling` — точный профиль (§9.5).
- Таблицы запроса карточка берёт из последнего плана (`Schema` / `Relation Name` узлов, §8.5) и хранит в состоянии вкладки;
  до плана — кнопка «Получить план» (пробный EXPLAIN без выполнения, §8.3); она скрыта, если текста нет (запроса уже нет в pgss) или
  он усечён.
- Запроса нет в pgss (вытеснен, статистика сброшена) — не ошибка: `text = null`, `sinceReset = null`, окна, история и разбивка
  пусты (ключей нет), `running` и `waits` остаются (§11). Итоги `totals` и доли находок считаются по всему представлению pgss
  (без фильтров базы и `toplevel`), в отличие от отчёта топа, где итоги — по отфильтрованному набору.

### 7.3 Обзор

Шапка: текст (моноширинный, подсветка ключевых слов средствами CSS без переформатирования, «Скопировать»), пометка
«обрезан», база, узел, `queryId`. Блоки: метрики в две колонки («с момента сброса» и окно до 60 мин, в P3a одно — §15, п. 18: вызовы, общее, среднее,
min / max / stddev, строк на вызов, hit ratio, shared read / written, temp, WAL, планирование, JIT); два спарклайна истории
(вызовы в минуту, среднее) с промежутками; находки запроса; разбивка по ролям и `toplevel`; выполняющиеся экземпляры
(кнопка «Открыть «Сессии»» — вкладка `sessions` узла); ожидания; таблицы (ссылки на вкладки таблиц). Кнопки: «План»
(подвкладка), «Открыть в консоли» (вкладка `explain` с текстом, базой и узлом), «Копировать» у текста запроса; в шапке карточки —
переключатель узла (§7.1), «Обновить» и «Скопировать для LLM». «Получить план» (у блока таблиц) открывает подвкладку «План» и
запускает пробный план; спарклайн рвёт линию на промежутках сбора. Обновление — по тикам сэмплера и кнопкой; `running`
перечитывается каждые 5 с, пока вкладка видима.

Выполняющиеся экземпляры — только `state = 'active'` и `leader_pid IS NULL`: без простаивающих сессий (у них сохраняется
`query_id` прошлого запроса) и без параллельных воркеров (дубликаты лидера). `truncated` истинен при срезе по
`track_activity_query_size` (по `pg_settings.setting`, в байтах) или по `QUERY_LIMIT` (16 384 символа). Текст запроса карточка
читает из кэша текстов узла сэмплера pgss (§4.6); `pg_stat_statements(true)` вызывается только при промахе для запроса, у которого
в pgss есть строки (после чего текст кладётся в кэш); если строк нет, файл не читается, а если текста у строки нет (`NULL`),
в кэш ложится отметка «текста нет» — опрос каждые 5 с файл текстов не читает.

### 7.4 «Скопировать для LLM»

`query_llm_bundle(id, member, database, source, plan: PlanTree | null) -> LlmBundle`, где `source` —
`{ kind: "query", queryId }` (карточка) или `{ kind: "sql", sql }` (консоль), собирает в Rust **данные**, а markdown с
заголовками на языке интерфейса собирает фронтенд (`src/lib/llmMarkdown.ts`):

```ts
interface LlmBundle {
  serverVersionNum: number; pgssVersion: string | null; inRecovery: boolean; member: string; database: string;
  text: string | null; textTruncated: boolean;
  stats: { sinceReset: QueryStat | null; window3600: QueryStat | null } | null;   // у источника sql — null
  planText: string | null;                                                       // plan::to_text
  tables: { schema: string; name: string; ddl: string }[]; tablesOmitted: number;
  warnings: string[];                                                            // paramsNeeded, queryGone, planFailed, ddlFailed
}
```

1. «Среда» — версия сервера, версия pgss, роль узла (primary / реплика), база.
2. «Запрос» — нормализованный текст pgss в блоке ```sql (с `$N`; литералы выполняющихся экземпляров не включаются — это данные
   пользователей; экземпляр попадает в markdown только если пользователь выбрал его источником плана).
3. «Статистика» — таблицы: с момента сброса и «окно до 60 мин (за время работы сэмплера)» (если в окне есть дельты); строка
   «Планирование» — только если планирование учитывалось (`plans ≠ 0` или время планирования ≠ 0).
4. «План» — `plan::to_text` переданного дерева (карточка и консоль передают план последнего запуска; карточка — только если он
   получен на узле вкладки: ANALYZE чтения с лидера идёт на реплику, и такой план к узлу вкладки не относится); плана нет — пробный
   `EXPLAIN (FORMAT JSON[, GENERIC_PLAN])` (§8.3, без выполнения); на PG < 16 текст с `$N` без значений — предупреждение
   `paramsNeeded`. Любая ошибка пробного плана (права, ярус записи, отвергнутый текст, срок, узел) — предупреждение `planFailed`:
   бандл остаётся с текстом и статистикой. Усечённый текст не пробуется: плана нет, предупреждения нет — об этом говорит
   пометка усечения (`textTruncated`).
5. «Таблицы» — для отношений плана, не больше 10: DDL (`catalog::table::get` → `ddl`); общий предел
   64 КБ (считаются байты `ddl`), остаток — «… ещё N таблиц». Ошибка чтения DDL — предупреждение `ddlFailed`, такие таблицы входят в `tablesOmitted`.
Заголовки — на языке интерфейса; промптов и инструкций модели в тексте нет: это данные. Пределы: текст запроса — 16 384
символа, DDL — 64 КБ и 10 таблиц; общего предела у markdown нет (план не усекается). Копирование — `CopyLlmButton`: сборка
данных, markdown и буфер обмена; итог показан, как у `CopyButton`. Для источника `sql` (консоль, §8.7) раздела «Статистика»
нет, план и таблицы — те же, а в блок «Запрос» идёт введённый текст как есть, с литералами (они могут быть данными пользователей:
руководство просит проверить markdown перед вставкой в модель).

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
  forceWrite: boolean;                                             // false по умолчанию; «Выполнить как запись» после EXPLAIN_WRITES (§8.3, п. 4)
}
interface ExplainResult {
  mode: "plan" | "analyze"; tier: "read" | "write";
  plan: PlanTree; raw: unknown;                                    // raw — JSON сервера как есть
  relations: { schema: string | null; name: string }[];
  elapsedMs: number; warnings: string[];                           // ключи i18n: genericPlan, probeWithoutReadOnly
  recommendations?: Recommendation[];                              // P3b
}
```

Проверки до сервера (`INVALID_INPUT`): пустой `sql`; `sql` длиннее 256 КБ (`errors.explain.sqlTooLong`); `generic` с
`mode: analyze` (`errors.explain.genericAnalyze`); число `values` не равно max `$N` текста (`explain::params::count_params`: ручной
разбор по лексеру PostgreSQL, без регулярных выражений); `forceWrite` не с `mode: analyze` (`errors.explain.forceWritePlan` — ярус
записи с ограждением и журналом идёт только с выполнением, а проба «как запись» вне `READ ONLY` без журнала в режиме «план» была бы
дырой); `timeoutS` вне 5…600 (только ANALYZE); `searchPath` с недопустимыми символами (`errors.explain.badSearchPath`: допустимы
латиница, цифры, `_`, `$`, `"`, `,`, пробел, `.` и `-`, до 1024 байт; текст ошибки называет тот же набор: «идентификаторы (буквы,
цифры, _ $ . -), двойные кавычки, запятые и пробелы»). `generic` на
сервере < 16 — не `INVALID_INPUT`, а `FEATURE_UNAVAILABLE` (`errors.explain.genericUnsupported`): версия известна только после
подключения.

**Счёт `$N`.** Параметром считается `$` с цифрами вне строковых литералов (`'…'`, в том числе `E'…'` с экранированием `\`),
идентификаторов в кавычках (`"…"`), комментариев (`--`, вложенные `/* */`) и тел долларовых кавычек (`$$…$$`, `$tag$…$tag$`); `$`
сразу после символа идентификатора (`a$1`) — часть идентификатора. Иначе лишний `$N` (например, `'$5'` в литерале консоли) был бы
объявлен параметром без типа, и сервер отказал бы (42P18 «could not determine data type of parameter»), а не показал бы лишнее поле
формы. Номер насыщается и обрезается до 65 535 — предела числа параметров протокола (счётчик в Bind 16-битный). Зеркало — `countParams`
в `src/lib/useExplain.ts` с теми же правилами и тестами; в форме при числе параметров свыше 100 (`PARAM_FIELDS_LIMIT`) полей нет (§8.2).

**`GENERIC_PLAN` и значения параметров.** Расширенный протокол превращает `$N` в параметры самой команды EXPLAIN, и тогда
план строился бы по переданным значениям, а не обобщённый. Поэтому при `generic` текст сначала проверяется `prepare`
расширенным протоколом (одна инструкция; ошибка 42P18 — тип параметра не выведен, как у `$1 IS NULL` — допускается, проверка
идёт под `SAVEPOINT`), а затем `EXPLAIN (… GENERIC_PLAN …)` выполняется простым протоколом в той же `READ ONLY` транзакции.
Значения (`values`) передаются текстовым форматом с типом `unknown` (обёртка `TextParam`): тип выводит сервер, и он же разбирает
текст значения.

### 8.2 Источники текста и параметры

В подвкладке «План» карточки источник выбирается переключателем:
- **Нормализованный текст pgss** — параметров `max $N`. На 16+ по умолчанию `GENERIC_PLAN` для кнопки «План» (несовместим с ANALYZE).
  `ParamsForm`: поле на каждый `$N`, пустое поле — `NULL`; значения хранятся в состоянии вкладки. Поля видны и при включённом
  `GENERIC_PLAN` (над ними подсказка: флажок действует только на «План»): «План с выполнением» берёт значения из полей без снятия
  флажка, и ANALYZE из карточки идёт в один шаг. Свыше 100 параметров (`PARAM_FIELDS_LIMIT`) полей нет — сообщение, и запуски, которым
  нужны значения («План с выполнением», а без `GENERIC_PLAN` и «План»), недоступны; на 16+ «План» с `GENERIC_PLAN` работает.
  Нормализованный текст, усечённый до 16 384 символов (`textTruncated`), запустить нельзя: пункт серый с пометкой, вместо запуска —
  пояснение (экземпляр с полным текстом выбирается и запускается).
- **Текст выполняющегося экземпляра** — список из `running` (§7.2); текст с литералами, параметров нет; обрезанный текст
  (`truncated`) предлагать нельзя — пункт серый с объяснением.

Значения передаются расширенным протоколом с типом `unknown` (`prepare_typed(sql, [UNKNOWN; n])`) в текстовом формате: типы
выводит сервер, как при выполнении приложением.

### 8.3 Ход и ярусы (`explain::run`)

Соединение — `ActionClient` с `ConnectSpec::for_explain()`: `EXPLAIN_SESSION_OPTIONS = -c statement_timeout=5000 -c lock_timeout=1000
-c DateStyle=ISO` — без `search_path=pg_catalog` (запрос пользователя должен видеть его схемы) и без
`default_transaction_read_only`. Все шаги — внутри одной транзакции; SQL пользователя идёт отдельной командой расширенного
протокола, поэтому вторая инструкция в тексте отклоняется сервером (`cannot insert multiple commands into a prepared statement`
→ `INVALID_INPUT`, `errors.explain.oneStatement`); распознаётся она по routine `exec_parse_message` (не зависит от
`lc_messages`), а текст сообщения — запасной признак. У каждого шага EXPLAIN есть клиентский срок — серверный плюс 5 с
(`timeout_s + 5`, у пробного плана `PROBE_TIMEOUT_S + 5`): по его истечении соединение закрывается, а сервер откатывает
транзакцию.

1. **Пробный план.** `BEGIN READ ONLY` → `SET LOCAL statement_timeout = '15s'` → `SET LOCAL lock_timeout = '1s'` →
   `[SET LOCAL search_path = …]` → `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS[, GENERIC_PLAN]) <sql>` → `ROLLBACK`. Разбор →
   `PlanTree`, список отношений. **Ярус записи**, если в дереве есть `ModifyTable` (в том числе внутри CTE) или первое слово
   инструкции (после комментариев) — `CREATE` (CREATE TABLE AS / CREATE MATERIALIZED VIEW AS), или VERBOSE-план вызывает
   административную функцию из списка `explain::tier::ADMIN_FUNCTIONS` (завершение и отмена сессий, сброс статистики,
   `pg_reload_conf`, создание и удаление слотов, `pg_switch_wal`, `pg_promote`, `dblink*`, `http*` и др.: `READ ONLY` их не
   останавливает; в результате — предупреждение `adminFunction`, окно подтверждения называет причину; §15 п. 10); иначе
   **ярус чтения**.
   EXPLAIN без выполнения сервер считает чтением и в `READ ONLY` строит план даже для UPDATE; если же пробный план отвергнут
   25006 (инструкция вроде CREATE … AS на отдельных версиях), он повторяется в обычной транзакции `BEGIN … ROLLBACK` — только
   чтобы определить ярус и отношения, выполнения по-прежнему нет. Повтор — только для первого слова `CREATE`; в результате
   предупреждение `probeWithoutReadOnly`.
2. `mode: plan` — результат шага 1 (одно обращение к серверу).
3. `mode: analyze`, ярус чтения: `BEGIN READ ONLY` → `SET LOCAL statement_timeout = '<timeoutS>s'` → `SET LOCAL lock_timeout = '1s'`
   → `[search_path]` → `EXPLAIN (ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON) <sql>` → `ROLLBACK`. Подтверждения нет
   и на prod. `READ ONLY` — предохранитель и на случай ошибки яруса: попытка записи (volatile-функция, триггер) даёт SQLSTATE
   25006 → код `EXPLAIN_WRITES` с предложением выполнить как запись. На реплике — то же (там и так read-only).
   Если узел карточки — лидер, пробный план показал ярус чтения и в топологии есть доступная реплика, ANALYZE по умолчанию
   предлагается на реплике (поле «Выполнить на узле», пометка «на реплике план может отличаться»); лидера можно вернуть.
   Запуск на узле, отличном от узла вкладки, называет узел в сводке плана и в истории запусков, а «Скопировать для LLM» такой план
   не берёт (§7.4). В консоли выбора узла для ANALYZE нет: он идёт на узле консоли.
4. `mode: analyze`, ярус записи: `ensure_mutation_allowed(profile, confirmation.confirm)` (read-only профиль — `READ_ONLY`;
   prod — имя подключения); узел в recovery — `FEATURE_UNAVAILABLE` (`errors.explain.writeOnReplica`, «выберите primary»).
   P3b (§15 п. 17 и 20 з): затем `ensure_write_confirmed` — без флага `writeConfirmed` (его выставляет «Выполнить и откатить» окна
   подтверждения) `CONFIRMATION_REQUIRED` (`errors.explain.writeNotConfirmed`) на любом окружении; для `forceWrite` та же проверка — в
   `validate`, до соединения. Порядок в `ensure_write_allowed`: `ensure_mutation_allowed`, `ensure_write_confirmed`, проверка реплики.
   Фронтенд перед вызовом показывает `ConfirmDialog`: отношения из пробного плана и предупреждение — строки изменятся и
   откатятся. Откат отменяет изменения строк, в том числе сделанные триггерами, и `NOTIFY` не доставляется (он уходит только при
   commit); не отменяются сдвиг последовательностей, внешние вызовы (`dblink`, `http`) и нетранзакционные функции, в том числе
   вызванные из триггеров; блокировки держатся, пока запрос выполняется. Затем `BEGIN` → `SET LOCAL …` → `EXPLAIN (ANALYZE, BUFFERS, WAL, …)` → `ROLLBACK`.
   Любая ошибка, обрыв соединения или таймаут откатывают транзакцию сервером; `ROLLBACK` явный — страховка. «Выполнить как
   запись» (`forceWrite`, только с `mode: analyze`, кнопка после `EXPLAIN_WRITES`, §11): `ensure_mutation_allowed` и проверка реплики выполняются **до**
   пробного плана, а сам пробный план строится без `READ ONLY` — побочный эффект, который сервер вычисляет уже при
   планировании, отверг бы его там; ярус результата — запись.
5. 57014 → `TIMEOUT` (`errors.explain.timeout`, «план не получен за N с; выполнение прервано, изменений нет»); 42501 →
   `NOT_AUTHORIZED`, подсказка зависит от инструкции: для INSERT / UPDATE / DELETE / MERGE / CREATE (по первому слову или при
   ярусе записи) — `errors.explain.notAuthorizedWrite` («для EXPLAIN записи нужны права на запись даже без выполнения»:
   PostgreSQL проверяет их при построении плана), иначе — о правах чтения (`pg_read_all_data`, §9.6); `WITH … UPDATE` по
   первому слову не распознаётся, и до построения плана подсказка — о правах чтения. Все ошибки классов 42 и 22 (синтаксис,
   неизвестные объекты, значения параметров) → `INVALID_INPUT` (`errors.explain.server`) с сообщением сервера и
   `details: "position N"`, где N — позиция в символах, с 1, относительно текста пользователя (ошибка вне его текста позиции не
   имеет). 25006 в `READ ONLY` (пробный план, ярус чтения) → `EXPLAIN_WRITES`; вне `READ ONLY` (ярус записи, повтор `CREATE`) →
   `READ_ONLY` (`errors.explain.serverReadOnly`): сервер в этом сеансе запись не принимает, и «как запись» ничего не изменит.

Фронтенд: подвкладка «План» и консоль сначала вызывают `explain` с `mode: plan` (кнопка «План»); кнопка «План с выполнением»
по результату пробного плана либо сразу вызывает `analyze` (ярус чтения), либо открывает подтверждение (ярус записи). Ярус
результата показывается в сводке.

### 8.4 Журнал

Ярус записи пишет `ActionRecord { action: "explainAnalyze", member, prodConfirmed, outcome, code, ms, explain: { database,
tables: string[], timeoutS, sqlHead } }` (`sqlHead` — первые 200 символов). `tables` — все отношения плана, в том числе те, что
запрос только читает, а не только изменяемые. Поле `explain` необязательное, как `slot` и `patroni`; вкладка журнала P2 показывает
базу, таблицы и срок. Ярус чтения в журнал не пишется — как остальные чтения. Сбой записи журнала у `explainAnalyze` тихий (результат
плана уже получен, запрос откатан, окно `journalNotWritten` P2 здесь не показывается) — §15, п. 14.

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
  actual: { startupMs: number | null; totalMs: number | null; rows: number; loops: number } | null;   // null — без ANALYZE; времена null при TIMING OFF
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
  inclusiveMs: number | null; exclusiveMs: number | null; sharePct: number | null;   // actual.totalMs × loops (под Gather — см. ниже); минус дети (≥ 0); от totalMs; null при TIMING OFF
  estimateRatio: number | null;                                 // actual.rows / planRows (строки на цикл, как у сервера); null у невыполнявшегося узла (loops = 0); бесконечность → null при misestimated: true
  hot: boolean; misestimated: boolean;                          // три узла с наибольшим exclusiveMs (без ANALYZE и при TIMING OFF — по totalCost минус дети); ratio ≥ 10 или ≤ 0.1 при max(план, факт) ≥ 100; у невыполнявшегося узла false
  children: PlanNode[];
}
```

**Время под `Gather`.** Под `Gather` / `Gather Merge` сервер отдаёт в `actual.totalMs` среднее на процесс, а `loops` — число
процессов (воркеры и ведущий), умноженное на внутренние циклы, поэтому `totalMs × loops` там — суммарное время всех процессов, а не
время по часам. Включённое время такого узла — `actual.totalMs × loops / processes`, где `processes` — `loops` внешнего потомка
`Gather`, делённые на `loops` самого `Gather` (при повторном запуске `Gather`, на внутренней стороне Nested Loop или под
коррелированным SubPlan, у потомка `loops` = запуски × процессы; значение не меньше 1). InitPlan и SubPlan самого `Gather` в
параллельную часть не входят и на процессы не делятся; вне `Gather` — `totalMs × loops`. При `TIMING OFF` времена узлов `null`,
доли тоже, а горячие узлы определяются по стоимости. Времена JIT (`generationMs`, `inliningMs`, `optimizationMs`, `emissionMs`, `totalMs`) — `Option`: блока `Timing` у сервера нет без ANALYZE и при
`TIMING OFF`, тогда все они `null` (а не нулевые), и `to_text` печатает у блока `JIT:` только `Functions: N`, как psql; строка `Timing: …` — лишь при
наличии времён, сводка на фронтенде при `totalMs = null` пишет «JIT: функций N». JIT: `Timing.Generation` на PG 17+ — объект `{Deform, Total}` (`Deform` уже
входит в `Total`), берётся `Total`; раньше — число.

Незнакомые типы узлов и поля не ломают разбор (`extra`). `to_text(&PlanTree) -> String` — рендер в стиле psql: заголовок узла
(`Node Type [using idx] on schema.rel alias  (cost=… rows=… width=…) (actual time=… rows=… loops=…)`), строки условий, отсева,
сортировки, буферов, воркеров; после дерева — строка `Settings: name = 'value', …` (только при непустых настройках, как у
`ExplainPrintSettings`), затем Planning Time, триггеры, JIT, Execution Time. Входные фикстуры — `crates/pg-advisor/tests/fixtures/plans/`;
выходных golden-текстов нет: рендер проверяют подстроками, а тест `every_fixture_node_keeps_every_key` — что каждый ключ узла фикстур
либо разобран, либо попал в `extra`. Текстовых рендеров два (Rust и `planText.ts` для кнопки «Как текст»), общих фикстур у них нет.

### 8.6 Дерево в интерфейсе

`PlanTree` — таблица-дерево с отступами и развёрткой: узел (тип, отношение или индекс, стратегия, `subplanName`), время
(включённое и собственное, полоса доли; у невыполнявшегося узла и при `TIMING OFF` — прочерк, без ANALYZE вместо времени
стоимость), строки (оценка → факт; `misestimated` показана как «×N», N = max(r, 1/r), округлено: оранжевая при ×10, красная при
×100 и при бесконечной ошибке «×∞»), loops, буферы (hit / read; temp отдельно), пометки (сброс на диск, lossy, heap fetches,
воркеры меньше плана, параллельный узел, `never executed`). Горячие узлы выделены фоном строки и меткой «горячий» в колонке «Пометок», а не только шрифтом. Времена форматирует `formatMs` (§4.3).
`PlanNodeDetails` — панель выбранного узла со всеми распознанными полями (пустые не показываются) и `extra`. `PlanSummary` —
узел (если план получен не на узле вкладки), планирование, выполнение, ярус, полный цикл (от обращения к серверу до ответа), JIT,
триггеры, settings, предупреждения. Кнопки: «JSON» (сырой, копировать), «Как текст» (копировать
`to_text`), P3b — блок рекомендаций под сводкой.

### 8.7 Консоль

`TabSpec { kind: "explain", connectionId, member?: string | null, database?: string, sql?: string }`. Поля: узел, база (список
баз каталога), SQL (`textarea` моноширинным шрифтом; редактор-зависимость не добавляем), `search_path` (свёрнутое поле),
кнопки «План» и «План с выполнением» со сроком (5 с, 15 с, 30 с, 1 мин, 5 мин, 10 мин), форма параметров при `$N`
(до 100 полей, §8.2; флажок `GENERIC_PLAN` на 16+), режим «Вставить план» — второе текстовое поле («текст или JSON», над ним строка назначения: план, полученный вне Глафиры) → `parse_plan(text)`: формат
определяется сам — JSON (с обрамлением psql — `QUERY PLAN`, разделители, `+` переносов, `(N rows)` — снимается) или текст psql / auto_explain (§9.4). История запусков в пределах вкладки
(не сохраняется): список «№, режим, ярус, время, длительность» с возвратом к плану. «Скопировать для LLM» — запрос, план и DDL
таблиц (§7.4). Вкладка открывается из панели инструментов диагностики, из карточки и из дерева («Консоль EXPLAIN» у сервера).

---

## 9. P3b: советовать и сравнивать

### 9.1 Рекомендации по плану (`pg-advisor::plan_rules`)

`evaluate(&PlanTree) -> Report { recommendations, skipped }`; `Recommendation { id, severity, nodeIds, params, command: string | null,
hypo: { indexDdl } | null }` (camelCase). Доказательство — не массив `evidence`, а `params` (цифры узла, как `Finding.params`); тексты
заголовка, доказательства, эффекта и рисков — во фронтенде по ключам `recommend.<id>.{title,evidence,effect,risks}` (у части правил —
варианты ключей: `titlePartitions` / `evidencePartitions` у сгруппированных партиций, `titleBitmap` / `evidenceBitmap` и `effectMemory` у
`plan_spill`, `evidencePlan` у `plan_no_pruning` без ANALYZE, `evidenceParallel` у `plan_jit_heavy` с `processes > 1`). Команда собирается
в Rust с кавычками идентификаторов, приложение её **не выполняет** — только показывает и копирует. Правила с пометкой «факт» (`FACT_RULES`,
9 из 11) требуют `actual` (ANALYZE); без него пропускаются и перечисляются в `skipped`, блок показывает «выполните план с выполнением, чтобы
проверить ещё N правил». Порядок — по уровню (сначала critical), затем `nodeIds`, затем `id`. Пороги — `findings::thresholds::PLAN_*`.

| id | Условие | Команда / совет | Уровень |
|---|---|---|---|
| `plan_seq_scan_filter` (факт) | Seq Scan, `(actual.rows + rowsRemovedByFilter) × loops ≥ 10 000`, `rowsRemovedByFilter ≥ 10 × actual.rows` | `CREATE INDEX CONCURRENTLY ON s.t (cols)` по колонкам фильтра; не разобрано — шаблон `(<columns>)` без hypopg. Сканы партиций под одним `Append` / `Merge Append` (от двух) объединяются в одну рекомендацию (`group_partitions`): `partitions`, `tables`, суммы строк, максимум `loops`, уровень — наибольший, `nodeIds` — все сканы, команда — строка `CREATE INDEX` на партицию, без `hypo` | warning |
| `plan_index_filter` (факт) | Index Scan / Bitmap Heap Scan (по `recheckCond`), `rowsRemovedByFilter ≥ 10 × actual.rows`, `≥ 1000` | составной индекс: колонки `indexCond` + колонки `filter`; равенства раньше диапазонов | warning |
| `plan_nested_loop_seq` (факт) | Nested Loop, внутренний ребёнок — Seq Scan с `loops ≥ 1000` | индекс по ключу соединения: колонки `filter` внутреннего узла, а если их нет — `joinFilter` Nested Loop (колонка берётся с той стороны сравнения, что относится к внутреннему узлу); `nodeIds` — [Nested Loop, Seq Scan] | warning |
| `plan_misestimate` (факт) | флаг `misestimated` узла P3a (`estimateRatio ≥ 10` или `≤ 0.1`) при `max(planRows, actual.rows) ≥ 1000`; переоценка (`direction = over`) под `Limit` без блокирующего узла между ними (`early_stop_nodes`: Sort, Hash, Hashed SetOp, Aggregate не `Sorted`) не показывается | `ANALYZE s.t;` + `ALTER TABLE … ALTER COLUMN … SET STATISTICS 1000;` по колонкам условия + `CREATE STATISTICS s."t_c1_c2_stat" ON … FROM s.t;` при двух колонках и больше (имя обязательно до PG 16; основа обрезается до 58 байт, суффикс `_stat` — до 63); `ratio` — конечный (`max(actual,1) / max(plan,1)`, обращённый при < 1) | warning |
| `plan_spill` (факт) | `sortSpaceType = Disk`, `hashBatches > 1`, `lossyHeapBlocks > 0` | сортировка и хеш: `SET work_mem = '<объём × 1,5, не больше 1 ГБ>kB';`, у сортировки по колонкам — индекс под `sortKey` в тексте эффекта; lossy bitmap: объём не считается (`lossyBlocks`, `exactBlocks`), команда — шаблон `SET work_mem = '<work_mem>';` | warning |
| `plan_sort_limit` | правило на узле `Limit`: под ним Sort (допустим один `Gather Merge` между ними), вход Sort ≥ 100 × строк Limit — на одно выполнение Limit (`rows × loops / loops Limit`; без факта — по `planRows`); вход Sort — скан таблицы (над соединением или агрегатом молчит) | индекс под префикс ключей сортировки одного направления из простых колонок (выражение, `NULLS`, `COLLATE`, чужой alias, смена ASC / DESC обрывают список); нет колонок — шаблон `(<columns>)` без hypopg; `nodeIds` — [Sort, Limit] | info |
| `plan_heap_fetches` (факт) | Index Only Scan, `heapFetches ≥ 0.1 × actual.rows × loops`, `≥ 1000` | `VACUUM s.t;` (карта видимости) | info |
| `plan_no_pruning` | Append / Merge Append с ≥ 10 детьми-сканами и без `subplansRemoved`; факт: ≥ 50 % детей с `actual.rows = 0` | без команды: условие по ключу партиционирования; с фактом — warning, без факта — info (`evidencePlan`) | warning / info |
| `plan_workers_short` (факт) | `workersLaunched < workersPlanned` | `SHOW max_parallel_workers;` `SHOW max_worker_processes;` | info |
| `plan_subplan_per_row` (факт) | узел с `parentRelationship = SubPlan`, `loops ≥ 1000` | без команды: переписать на JOIN / LATERAL | warning |
| `plan_jit_heavy` (факт) | `jit.totalMs / processes > 0.5 × executionMs`, `processes` = 1 + сумма `Workers Launched` всех Gather / Gather Merge (сервер складывает JIT всех процессов) | `SET jit = off;` для сессии / роли или выше `jit_above_cost`; `nodeIds` — корень | info |

**Колонки** (`columns_of`, без парсера SQL): условие очищается от внешних скобок, делится по `AND` верхнего уровня (вложенные группы `AND`
раскрываются, кавычки `'…'` и `"…"` учитываются); из конъюнкта берётся ведущий идентификатор `(alias.)?col`, если за ним идёт оператор
`= <> != <= >= < > ~~ !~~ ~~* !~~* IS IN`; приведение снимается **только у текстовых типов** (`text`, `varchar` / `character varying`, `bpchar` /
`character`, `name`, с модификатором длины или без), любое другое приведение (в том числе `::date` и массивы) и функции делают конъюнкт
выражением без колонки; у ключа соединения `other.col op alias.col` колонка берётся справа; дубликаты убираются, равенства — раньше
диапазонов. `hypo.indexDdl` (`CREATE INDEX ON s.t (cols)`, без `CONCURRENTLY` и `;`) есть у `plan_seq_scan_filter` (кроме
сгруппированной), `plan_index_filter`, `plan_nested_loop_seq`, `plan_sort_limit`, когда колонки разобраны.

**Блок `Recommendations`** под сводкой плана (`PlanView.afterSummary`; только у планов, полученных `explain`, у вставленного плана блока нет):
заголовок с числом, «Советов нет», строка о пропущенных правилах; карточка на рекомендацию — уровень, заголовок, доказательство, команда с
«Копировать команду» и пометкой, что приложение её не выполняет (у шаблона — ещё «замените <…> своими значениями»; шаблон узнаётся по
точным токенам `<columns>` и `<work_mem>`), эффект, риски, «Проверить гипотезу» (только при `hypo`), «Показать узел»: выбирает первый узел,
отмечает все `nodeIds` (полоса и метка «рекомендация», не только цвет), раскрывает свёрнутых предков и прокручивает к первому; клик по строке
дерева снимает отметки. Рекомендация с неизвестным фронтенду `id` (нет всех четырёх ключей) — карточка «Рекомендация <id>» с советом обновить
приложение и командой, без эффекта и рисков.

### 9.2 hypopg

`hypo_check(id, { member, database, sql, params, searchPath, indexDdl }) -> HypoResult { indexName, estimatedBytes, costBefore,
costAfter, used, planAfter: PlanTree }` на отдельном соединении `for_explain()`. Запрос — запуск, давший показанный план: фронтенд берёт
`member`, `database`, `sql`, `params` и `searchPath` из сохранённого `ExplainRequest` этого запуска, а не из текущих полей формы; у плана с
выполнением на реплике это реплика (там нужен пакет `hypopg`; работа на hot standby автотестами не проверена). Ход:
1. Проверки до соединения: `explain::validate` над запросом в режиме «План» (пустой текст, длина, число `$N`, `search_path`, `GENERIC_PLAN`
   на < 16 по версии из `head`) и `validate_ddl`: ровно `CREATE INDEX ON …` (без имени индекса, `UNIQUE`, `CONCURRENTLY`), вне кавычек
   `'…'` / `"…"` нет `;`, `--`, `/*` и `$`, обратной косой черты нет нигде (иначе `E'\''` или `standard_conforming_strings = off` прятали бы
   вторую инструкцию) — `errors.hypo.badDdl`.
2. Схема расширения из `pg_extension` (нет — `errors.hypo.notInstalled`, `FEATURE_UNAVAILABLE`); функции `hypopg` вызываются с этой схемой.
3. Стоимость «до» — проба `explain_in_tx` в своей транзакции `READ ONLY` с опциями пробы P3a (`probe_options`: `PROBE_OPTIONS` плюс
   `GENERIC_PLAN`, когда параметры общие) и `Origin::new` с той же строкой опций.
4. Одна транзакция `BEGIN READ ONLY` (`run::begin_tx`: `SET LOCAL statement_timeout`, `lock_timeout` и проверенный `search_path` запроса) →
   `<schema>.hypopg_create_index($1)` → `<schema>.hypopg_relation_size(indexrelid)` → EXPLAIN «после» (`explain_in_open_tx`) →
   `<schema>.hypopg_reset()` → `ROLLBACK`. Виртуальный индекс живёт в памяти сессии и переживает откат, поэтому сброс идёт до `ROLLBACK`
   при любом исходе; если транзакция уже прервана и сброс в ней не прошёл, он повторяется после `ROLLBACK`. Сбой отката или повторного
   сброса возвращается ошибкой (после более ранней ошибки шага). Истёк клиентский срок (`within`) — возврат сразу, соединение
   закрывается без ожидания отката.
5. `used` — в `planAfter` есть узел с `indexName = indexName`.

Ошибки сервера при создании гипотезы — `errors.hypo.rejected` (`INVALID_INPUT`, `{{message}}`) для 42601, 42P01, 42703, 42704, 42883,
42809, 42P17 и XX000 с сообщением «hypopg…»; остальное — обычным путём `from_pg`. Кнопка «Проверить гипотезу» есть у любой рекомендации с
`hypo` (установлен ли `hypopg`, фронтенд заранее не проверяет): без расширения окно показывает `errors.hypo.notInstalled`. `HypoCheck` —
модальное окно (фокус на «Закрыть», Tab в окне, Escape закрывает): команда, пояснение, «Стоимость: до → после», использован ли индекс,
оценка размера, дерево «после». Публичные `check_on(client, req)`, `schema_of`, `hypothesize` — для `it_hypopg`, который на том же
клиенте проверяет пустой `hypopg_list_indexes` после успешной проверки, после сбоя пробы «после» и после отвергнутой гипотезы.

### 9.3 Снимки

Файл `data_dir/snapshots/<connectionId>/<member>/<yyyy-mm-ddThh-mm-ss>.json` (время снятия в UTC; запись — `storage::fs::atomic_write`:
временный файл 0600 → rename → fsync, так что список не видит недописанных файлов; каталоги `snapshots/`, `<id>/`, `<member>/` выставляются в
0700 и тогда, когда уже существовали). Каталог `<member>` — имя узла в переносимой форме (`member_dir`): ASCII-буквы, цифры,
`-`, `_`, `.`, пробел и не-ASCII — как есть, остальное (`%`, `:`, `[`, `]`, разделители, управляющие), точка или пробел в
конце и имена устройств Windows — `%XX` по байтам UTF-8 (`db1:5432` → `db1%3A5432`, `[::1]:5432` → `%5B%3A%3A1%5D%3A5432`):
на Windows `:` в имени запрещён. Настоящее имя узла — `member` в файле. Каталоги, записанные на macOS и Linux до кодирования
(имя как есть), читаются и удаляются по-прежнему: адрес файла `SnapshotInfo.file` — имя каталога на диске; новые снимки того же
узла ложатся в закодированный каталог, в списке узел один и тот же (из содержимого). Второй снимок узла в ту же секунду отклоняется (`errors.snapshots.exists`), а не заменяет первый.
```ts
interface Snapshot {
  v: 1; takenAtMs: number; connection: { id: string; name: string }; member: string;
  serverVersionNum: number; inRecovery: boolean; pgssVersion: string; statsResetMs: number | null; dealloc: number | null;
  max: number; trackPlanning: boolean; hidden: boolean;   // отчёт «между снимками» без догадок о max и track_planning
  kcache: boolean;                       // при снятии был pg_stat_kcache (поле без смены v; старые файлы — false)
  rowCount: number;                      // для списка без разбора rows
  rows: Cumulative[];                    // все строки pgss, накопительно, без текстов
  texts: Record<string, string>;         // queryId → текст: 500 верхних по totalExecMs и 500 по calls, без повторов
  truncated: string[];                   // queryId с текстом, обрезанным по QUERY_LIMIT
}
```
Тексты по вызовам добавлены к уточнению 5 плана: без них у частых дешёвых запросов (опрос узла самим приложением, кандидаты N+1) не было бы
текста, и пометка `own` и N+1 «между снимками» ошибались бы. SQL снятия несёт метку `/* glafira */`.

Адрес файла в командах — относительный путь `<member>/<имя>.json` (`valid_file`: ровно два обычных компонента, без `..` и обратной косой
черты; id подключения — один компонент) → `errors.snapshots.badFile`. Команды: `snapshot_take(id, member) -> SnapshotInfo` (нет
`pg_stat_statements` — `errors.queries.pgssMissing` / `pgssTooOld`); `snapshot_list(id) -> SnapshotInfo[]` (новые сверху, без открытого
подключения; читается только заголовок файла); `snapshot_delete(id, file) -> bool` (`false` — файла уже не было; пустой каталог узла
удаляется); `snapshot_compare(id, from, to) -> Comparison`. `SnapshotInfo { file, member, takenAtMs, rows: number | null, bytes, unreadable?:
"tooLarge" | "corrupt" }`: файл больше `MAX_SNAPSHOT_BYTES` (128 МиБ) или не разбираемый в список попадает (время — из имени файла, узел —
из каталога, размер — из `stat`), выбрать его нельзя, удалить можно; `read` такие файлы отклоняет (`errors.snapshots.tooLarge`, ошибка
JSON), файл новее `SNAPSHOT_VERSION` — `errors.storage.newerSchema`. Снимки подключения удаляются вместе с ним (`remove_connection` в
`delete_connection_full`; сбой — предупреждение в логе, удаление профиля не откатывается); в архив диагностики не входят (`write_archive` не
читает `data_dir`).

Пара читается `read_pair`: тот же файл дважды — `errors.snapshots.samePair`; снимки **разных узлов** отклоняются
(`errors.snapshots.differentMembers`, `INVALID_INPUT`: счётчики не связаны), а не сравниваются с пометкой; порядок `from` / `to` любой —
пара упорядочивается по времени (старший — начало). Сравнение (`queries::compare`, по ключу): `new` (есть только в `to`), `gone` (только в
`from`), `degraded` (`meanExecMs` ≥ ×2 при `calls ≥ 50` — `COMPARE_*`), `improved` (≤ ×0,5 при тех же условиях), `totalsFrom` / `totalsTo`,
`reset` — `stats_reset` различается («статистика сбрасывалась, надёжны только новые и исчезнувшие»). `before` — накопленное значение `from`,
`after` — дельта; у строк — `text`, `textTruncated`, `own` из текстов снимков (свои строки не отбрасываются, фронтенд их приглушает).

Режим топа «между снимками» — `run_diagnostic` с `queries.mode = { kind: "between", from, to }`; `diag::run` обрабатывает его до выбора
цели диагностики (`report_between` строит `DiagReport` сам), поэтому **открытое подключение не нужно**, `member` запроса игнорируется, узел
отчёта — узел снимков, `followsLeader = false`, шапка (версия, `inRecovery`, `observedAtMs`) — из снимка `to`. Строки — дельты `to − from`;
ключ только в `to` или с уменьшившимися счётчиками — значение `to` целиком; после сброса — все строки `to` целиком; неизменившиеся строки
отбрасываются. `coverageS` — `to.takenAt − from.takenAt`, а при сбросе между снимками — `to.takenAt − max(to.statsReset, from.takenAt)`
(значок значит одно и то же с окном: время, за которое накоплены дельты); `ticks = null`; сброс — `unavailable{part: between,
reason: betweenReset}`, скрытые чужие строки в `to` — `unavailable{others}`. Находки — как в режиме окна (`dealloc_grew` — рост `dealloc`
без сброса). `minmax` нет: сортировки `maxTime` и `stddevTime` — `errors.queries.sortUnavailable`; `cpuTime` — только если оба снимка с
`kcache` (иначе поля kcache снимаются с обеих сторон, `without_kcache`). Поиск — подстрока без учёта регистра по текстам снимков
(`cached_matches`), на сервер не ходит. Пометка `own` — по тем же правилам, что в P3a.

Интерфейс: кнопка «Снимки» в шапке инструмента (`aria-expanded`) открывает `SnapshotsPanel`: подсказка «хранятся только на этом
компьютере», список (время, узел, строк, размер, метки нечитаемых), «Сделать снимок» (узел инструмента; неактивна без узла, то есть без
подключения), выбор до двух (третий заменяет более ранний), «Сравнить» — только для двух читаемых снимков одного узла (разные узлы —
пояснение, кнопка неактивна), «Удалить» с подтверждением в строке (Escape отменяет). `SnapshotCompare` — четыре списка, сброс, итоги
вызовов, свои строки приглушены, CPU у новых и исчезнувших при данных kcache; «Топ между снимками» берёт пару, которая сравнивалась.
В режиме «между снимками» над таблицей — «Между снимками узла {{member}}: A → B» (локальное время) и «Выйти из режима снимков»; значок
«{{duration}} между снимками»; `query_texts` не вызывается; удаление снимка текущей пары выходит из режима. Без подключения инструмент
показывает пометку о закрытом подключении, подсказку и панель снимков, а в режиме «между снимками» — отчёт; «С момента сброса» и окна в
списке режимов неактивны.

### 9.4 Текстовые планы (`plan::from_text`) — в P3a

*Перенесено из P3b в P3a решением владельца 2026-11-01 (смоук P3a). Реализация: `crates/pg-advisor/src/plan/text_in.rs` (`from_text`), диспетчер
`plan::json::from_any(&str)` (JSON, в том числе внутри обрамления psql или записи auto_explain, иначе текст), Tauri-команда `parse_plan`; ошибка
разбора — `errors.plan.unparsed` с номером строки исходного текста. Поддержаны вывод psql (выровненный и невыровненный режимы, `+`, `(N rows)`),
голый текст, запись auto_explain из лога сервера и в виде, полученном клиентом (`Query Text:` и `Query Parameters:` — в `extra`); форматы ANALYZE, BUFFERS,
VERBOSE, SETTINGS, WAL, TIMING OFF, `never executed`, параллельные воркеры, InitPlan / SubPlan / CTE, рамка psql `\pset border 2`
и копии из pgAdmin / DBeaver с кавычками. Не поддержаны юникодные линии и `\x`; тип узла не с буквы — ошибка разбора. Фикстуры — пары `<имя>.txt` / `<имя>.json` одного запроса с `pg-perf`
(`crates/pg-advisor/tests/fixtures/plans/text/`, PG 17) и круговой тест `from_json → to_text → from_text` по всем JSON-фикстурам PG 13–18; тест сравнивает
типы узлов, отношения, стоимости, оценки, условия, ключи, факт, буферы, JIT и итоги дерева. Ниже — исходный замысел разбора.*

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

- **`pg_stat_kcache`.** `detect` (P3a) вторым помеченным запросом `DETECT_KCACHE_SQL` читает версию и схему расширения, видимый роли
  `shared_preload_libraries` и `has_function_privilege(… 'EXECUTE')` (через `to_regprocedure`, сломанное расширение даёт `false`);
  `kcache_usable` отбрасывает расширение, если список preload виден и библиотеки в нём нет или нет права. Затем — проба
  `SELECT 1 FROM <schema>.pg_stat_kcache() LIMIT 1` (`LIMIT 0` не вызвал бы функцию, и ошибка «must be loaded via shared_preload_libraries»
  не сработала бы), в автокоммите; любая ошибка — расширения нет (лог debug), отчёт строится как без него. Итог — `KcacheCaps { schema,
  exec_prefix, has_top }` в `PgssInstalled.kcache` (2.2+ — `exec_*` и `top`). `statements_sql(ns, caps, kcache, where)`: `LEFT JOIN
  pg_stat_kcache() k ON (queryid, userid, dbid)` плюс `AND k.top = s.toplevel`; с pgss 1.8 (нет `toplevel`) и kcache 2.2+ — `AND k.top`,
  иначе строки верхнего уровня и вложенные задвоили бы строку pgss (при `pg_stat_kcache.track = all` CPU вложенных инструкций тогда не
  учитывается); старше 2.2 — `user_time`, `system_time`, `reads`, `writes` без `top`. Времена × 1000 → мс. Поля `cpuUserMs`, `cpuSysMs`,
  `diskReadBytes`, `diskWriteBytes` (`Option`) в `Counters` / `QueryStat`; в дельте счётчик kcache, который уменьшился, берётся текущим
  значением (у kcache свой `pg_stat_kcache_reset()`, его `decreased_from` pgss не видит); сортировка `cpuTime` = user + sys, доступна при
  `PgssMeta.kcache`. Сэмплер кэширует `PgssInstalled`: при смене наличия kcache тик даёт дельты `without_kcache()` (новая база для колонок
  kcache); установленное при работающем сэмплере расширение окно увидит после его перезапуска или ошибки чтения. Снимок хранит признак
  `kcache` (§9.3). Интерфейс: колонки «CPU» (сортируемая) и «Чтение с диска» в таблице **скрыты**, когда `pgss.kcache = false` (а не
  прочерки); карточка — метрики «CPU (user + sys)» и «Диск прочитано / записано» при `QueryDetail.kcache` (прочерк, если у запроса данных
  нет); «Скопировать для LLM» — строки CPU и диска; при входе в режим «между снимками» сортировка `cpuTime` сбрасывается на `totalTime`.
- **`pg_wait_sampling`.** `WAIT_SAMPLING_SQL` (помеченный, один запрос к каталогу при каждом чтении карточки): схема, `current_setting(
  'pg_wait_sampling.profile_queries', true)` (NULL — библиотека не загружена), `shared_preload_libraries`, права на
  `pg_wait_sampling_get_profile()` и `SELECT` на представление профиля, есть ли колонка `queryid` (её нет в 1.0). `wait_sampling_plan` →
  `Skip` (нет расширения, прав, `queryid`, preload виден и без библиотеки, параметр не определён или неизвестен — карточка берёт сэмплы P3a
  без пометки), `Off` (`none` / `off` / `no` / `false` / `0` — `unavailable{part: waitsProfile}`, текст `diag.unavailable.waitsProfileOff`,
  ожидания из сэмплов), `Read` (`top` / `all` / `on` / `yes` / `true` / `1` без учёта регистра: перечисление 1.1.4+ и логический параметр
  старых версий). Чтение: `SELECT COALESCE(event_type, 'CPU'), COALESCE(event, 'CPU'), sum(count) FROM <schema>.pg_wait_sampling_profile
  WHERE queryid = $1 GROUP BY 1, 2` (сэмплы CPU при `sample_cpu = on` хранятся с NULL; сумма по pid и **по базам**: в профиле нет `dbid`,
  ожидания одного `queryid` в разных базах складываются; при `top` ожидания вложенных инструкций приписаны верхней) → `waits` с
  `source: "pgWaitSampling"`, `sinceMs: 0`, `coverageS: null`. `WaitProfiles` в `AppState` (`CloseListener`) кэширует готовый результат по
  `(узел: id подключения, поколение, member; queryId)` на `WAIT_PROFILE_TTL_MS = 30 с`, включая «запроса в профиле нет»: сборщик расширения
  отдаёт на каждый запрос весь профиль через `shm_mq`, и на стенде (≈ 360 тыс. строк) чтение стоит ≈ 65 мс. Ошибки состояния сервера
  (классы 55 и 42, 58P01, XX000 с упоминанием `pg_wait_sampling` или `shared_preload_libraries`) запоминаются на
  `WAIT_SAMPLING_RETRY_MS = 10 мин`, остальные (57014 и прочие) — нет; во всех случаях карточка берёт сэмплы, закрытое подключение — ошибка
  карточки. Кэш чистится при закрытии подключения и удалении узла, ключ с поколением — переподключение начинает заново. Профиль
  предпочитается сэмплам (`pick_waits`). У сэмплов P3a в `QueryWaits` появилось `coverageS` (`now − since`, `since` — позднее из «час назад»
  и первого тика буфера). Подпись над таблицей: «Профиль pg_wait_sampling с момента его сброса» или «По сэмплам «Сессий» за {{coverageS}}».
  `pg_wait_sampling.profile_period` в подвале блока не показывается (замысел спеки не реализован). Версия 1.0 пропускается без пометки.
Оба расширения — только если установлены и пригодны; иначе поведение P3a.

### 9.6 Готовность

Факты `ReadinessFacts`: `pgss_version: Option<String>`, `pgss_track_planning: Option<String>`, `read_all_data: bool`
(член `pg_read_all_data` или суперпользователь; на 13 роли нет — `false`). Пункты чек-листа:
- `pgssTrackPlanning` (P3a) — только при созданном расширении (без него настройки нет); warning при `off`: «колонки планирования
  пустые; `pg_stat_statements.track_planning = on`»;
- `readAllData` (P3a) — warning на 14+ без роли: «EXPLAIN запросов приложения требует права чтения таблиц: `GRANT pg_read_all_data`»;
  на 13 — warning без команды «нужны права SELECT на таблицы запросов»; суперпользователь — ok. Роль даёт только чтение:
  EXPLAIN запроса, который пишет, требует прав на запись и без выполнения (§8.3, п. 5);
- P3b — параметр `hint` у пунктов `extension` для `hypopg`, `pg_stat_kcache`, `pg_wait_sampling` (`HINTED`; у остальных трёх
  необязательных расширений его нет). `TestPanel` у пункта не в состоянии ok показывает вместо общего `affects` текст
  `readiness.extension.hints.<name>` (что даёт: «Проверка гипотез индексов («Проверить гипотезу» в плане)», «CPU и диск по запросам в
  инструменте «Запросы»», «Точный профиль ожиданий запроса в карточке») и строкой ниже `readiness.extension.enable.<name>` — как включить:
  hypopg — только `CREATE EXTENSION`, без preload и перезапуска; `pg_stat_kcache` — в `shared_preload_libraries` после
  `pg_stat_statements`, перезапуск, `CREATE EXTENSION`, `EXECUTE` на `pg_stat_kcache()`, если его нет; `pg_wait_sampling` — preload,
  перезапуск, `CREATE EXTENSION`, `profile_queries = top` (`on` в старых версиях), версия 1.1+, `EXECUTE` на функции профиля, если его нет.
  `pg_monitor` этих прав не даёт, поэтому текст говорит «выдать, если нет».

---

## 10. IPC-контракт

| Часть | Команды |
|---|---|
| P3a | `run_diagnostic(id, request)` с `tool: "queries"` и `queries: QueriesParams` → `DiagReport` (§4.5); `start_statements_sampler(id, member: Option<String>, channel: Channel<StatementsEvent>) -> SessionId` (останов — существующий `stop_session`); `query_texts(id, member, keys: QueryKey[]) -> QueryText[]`; `query_detail(id, member, database, queryId) -> QueryDetail`; `explain(id, request: ExplainRequest) -> ExplainResult`; `parse_plan(text: String) -> PlanTree`; `query_llm_bundle(id, member, database, source: LlmSource, plan: Option<PlanTree>) -> LlmBundle` (данные, §7.4; markdown собирает фронтенд) |
| P3b | `explain` возвращает `recommendations` и `recommendationsSkipped`, `ExplainRequest` принимает `writeConfirmed: boolean` (serde default `false`, §8.3 п. 4); `hypo_check(id, request: HypoRequest) -> HypoResult` (§9.2); `snapshot_take(id, member) -> SnapshotInfo`; `snapshot_list(id) -> SnapshotInfo[]`; `snapshot_delete(id, file) -> bool`; `snapshot_compare(id, from, to) -> Comparison` (§9.3); режим «между снимками» — `run_diagnostic` с `queries.mode = { kind: "between", from, to }`, без открытого подключения; `QueryStat` / `PgssMeta` / `QueryDetail` — поля kcache (§9.5), `QueryWaits.coverageS`. Текстовые планы в `parse_plan` — уже в P3a (§9.4) |

`member: None` у `start_statements_sampler` — «за лидером» на момент подписки, как у `start_sampler`. Все команды возвращают
`Result<T, AppError>`; JSON — camelCase; `Option` → `null`; теги — `kind`. Типы фронтенда повторяют структуры
`pg_advisor::{queries, plan, plan_rules}` и `src-tauri/src/{diag/queries, diag/query_detail, explain, snapshots}`. Новых событий
Tauri нет: данные сэмплера идут каналом подписки. `queryId` везде — строка.

---

## 11. Ошибки

| code | Причина | Поведение UI |
|---|---|---|
| `EXPLAIN_WRITES` (новый, P3a) | SQLSTATE 25006 в `READ ONLY` (ярус чтения или пробный план): запрос, признанный чтением, пытается писать (`errors.explain.writes`) | Сообщение и кнопка «Выполнить как запись»: повторяет запрос с `forceWrite` (§8.3, п. 4) → ярус записи с подтверждением |
| `FEATURE_UNAVAILABLE` | Ярус записи на реплике (`errors.explain.writeOnReplica`); hypopg не установлен (`errors.hypo.notInstalled`, показывается в окне проверки); снимок без `pg_stat_statements` (`errors.queries.pgssMissing` / `pgssTooOld`); `GENERIC_PLAN` на < 16 (`errors.explain.genericUnsupported`); pgss исчез у сэмплера | Подсказка у кнопки; сэмплер — событие `error` и остановка |
| `NOT_AUTHORIZED` (42501) | Нет прав на таблицы запроса при EXPLAIN (`errors.explain.notAuthorized`, подсказка `pg_read_all_data`; у INSERT / UPDATE / DELETE / MERGE / CREATE — `errors.explain.notAuthorizedWrite`, права на запись нужны и без выполнения) | Сообщение с подсказкой; пункт готовности `readAllData` |
| `TIMEOUT` (57014) | EXPLAIN ANALYZE не уложился в срок (`errors.explain.timeout`); пробный план — в 15 с (тот же `errors.explain.timeout`) | «План не получен за отведённое время; выполнение прервано, изменений нет» |
| `INVALID_INPUT` | Вторая инструкция (`errors.explain.oneStatement`), число параметров (`paramsCount`), ошибки сервера классов 42 и 22 — синтаксис, неизвестные объекты, значения параметров (`errors.explain.server` с `params.message`, `details: "position N"`), пустой запрос, слишком длинный запрос (`sqlTooLong`), `GENERIC_PLAN` с ANALYZE (`genericAnalyze`), `forceWrite` не с ANALYZE (`forceWritePlan`), срок вне 5…600, `search_path` с недопустимыми символами (`badSearchPath`), сортировка вне версии (`errors.queries.sortUnavailable`), отчёт без `queries`, `indexDdl` не одна инструкция `CREATE INDEX ON …` без `CONCURRENTLY`, комментариев, `$` и обратной косой черты (`errors.hypo.badDdl`), сервер отверг гипотезу — 42601, 42P01, 42703, 42704, 42883, 42809, 42P17 и XX000 «hypopg…» (`errors.hypo.rejected`, `params.message`), неверный путь снимка (`errors.snapshots.badFile`), снимки разных узлов (`errors.snapshots.differentMembers`), один снимок дважды (`errors.snapshots.samePair`), снимок узла в ту же секунду (`errors.snapshots.exists`), файл снимка больше 128 МиБ (`errors.snapshots.tooLarge`). Снимков разных подключений команды не принимают по построению (адрес — внутри каталога подключения), ключа `errors.snapshots.foreign` нет | Сообщение у формы; позиция ошибки подсвечивается в консоли |
| `READ_ONLY`, `CONFIRMATION_REQUIRED` | Guard яруса записи (как P2); `CONFIRMATION_REQUIRED` также без флага `writeConfirmed` (`errors.explain.writeNotConfirmed`, P3b): `forceWrite` — в `validate` до соединения, ярус записи по пробе — после пробы `READ ONLY` и до выполнения, на любом окружении; `READ_ONLY` также при SQLSTATE 25006 вне `READ ONLY` — сервер не принимает запись в сеансе (`errors.explain.serverReadOnly`) | Кнопки нет / диалог подтверждения; «как запись» не предлагается |
| `NOT_FOUND` | Файл снимка исчез (`errors.snapshots.notFound`) | Сообщение в панели снимков, список перечитывается |
| `NODE_UNAVAILABLE` | Узел недоступен или подключение закрыто за время EXPLAIN | Как в P2 |
| `RECOVERY_CONFLICT` | EXPLAIN ANALYZE на реплике отменён конфликтом recovery | Как в P2, `retryable` |

Отсутствие pgss, прав на чужие строки, `query_id` на 13 — не ошибки, а `unavailable` отчёта. Запрос, которого с таким `queryId` в pgss
уже нет (вытеснен, статистика сброшена), тоже не ошибка: `query_detail` отдаёт `text = null` и `sinceReset = null`, карточка пишет
«Запроса больше нет в pg_stat_statements» (`diag.query.gone`), а «Скопировать для LLM» добавляет предупреждение `queryGone`. Ключа
`errors.queries.gone` нет; «последние известные данные» из буфера для такого запроса не показываются (бэклог). Текст плана, который не удалось
разобрать (`parse_plan`), — `INVALID_INPUT` (`errors.plan.unparsed`, `params.line`) с показом строки.

---

## 12. Структура репозитория (новое)

```
crates/pg-advisor/src/  pgss.rs  queries/{mod,rules}.rs  plan/{mod,json,text_out,text_in}.rs
                        plan/text_in.rs (из P3b, §9.4)
                        P3b: queries/compare.rs  plan_rules/{mod,scan,exec}.rs
crates/pg-advisor/tests/fixtures/plans/{json,text}/  (PG 13–18, по правилу — положительная и отрицательная)
src-tauri/src/   sampler/statements.rs  diag/{queries,query_detail}.rs  explain/{mod,run,params,tier,llm}.rs
                 commands/{queries,explain}.rs   P3b: explain/hypopg.rs  snapshots/{mod,store}.rs  commands/snapshots.rs
src-tauri/tests/ it_queries.rs  it_explain.rs   P3b: it_hypopg.rs  it_snapshots.rs
src/components/queries/  QueriesView QueryTab QueryOverview QueryPlan ExplainTab Sparkline CopyLlmButton queriesText
                         plan/{PlanTree,PlanView,PlanRunner,PlanNodeDetails,PlanSummary,ParamsForm,AnalyzeConfirm,planText}
                         P3b: plan/{Recommendations,HypoCheck,recommendText}  SnapshotsPanel SnapshotCompare
src/lib/  useStatementsSampler.ts  useQueriesReport.ts  useQueryDetail.ts  useExplain.ts  llmMarkdown.ts
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
база `perf` с расширениями; схема `shop`: `customers` (200 тыс.), `orders` (500 тыс., `status` без индекса), `order_items`
(1,5 млн), `events` — партиционирована по месяцам (24 партиции, 1 млн строк), `audit` (200 тыс.) с `autovacuum_enabled = off` и
скошенными данными (ошибка оценки), `balances` (200 тыс.) с обновлёнными строками без VACUUM (heap fetches); всего около 3,6 млн
строк. `glafira_app` получает `SELECT`, `INSERT`, `UPDATE`, `DELETE` на таблицы `shop` и `USAGE`, `SELECT` на её последовательности,
без `pg_monitor`; `glafira_diag` записи в `shop` не имеет. `heavy.sql` — сценарий `pgbench` из 13 инструкций
с `\set` случайных параметров: точечный выбор клиента (шаблон N+1: три инструкции прохода — по `:cid`, `:cid1`, `:cid2` — нормализуются
в один `queryid` `id = $1`, около 23 % вызовов при пороге 20 % правила `query_n_plus_one`; соседние id считаются в `\set`, потому что
выражение `:cid + 1` прямо в запросе дало бы другой текст и другую строку, и доля упала бы ниже порога), бимодальный запрос к `audit`
по `order_id` (`query_unstable`: у заказа 1 — 99 % строк; среднее около 1 мс — на пороге правила, находка может не появиться),
фильтр по `orders.status` (seq scan с отсевом), соединение с
`audit` (nested loop по неверной оценке), сортировка заказов без индекса (сброс), `ORDER BY … LIMIT` без индекса, Index Only
Scan по `balances`, коррелированный подзапрос на строку, выбор из `events` без ключа партиционирования, тяжёлая агрегация
(параллельность и JIT), запрос с temp-файлами. Запуск (в `dev/perf/README.md`):
`docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300`. Поднимается
`docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf` (сервис в профиле `perf`: общий `up` его не поднимает, по
имени — поднимает), готовность — `scripts/wait-perf.sh`; существующие контейнеры не трогаются. Образ запекает
`heavy.sql`, поэтому контейнер, созданный до правки сценария, несёт старый файл (12 инструкций, N+1 недостижим); до пересоздания
контейнера свежий файл подкладывают `docker cp`, смоук это оговаривает. `queries_incomplete` стендом недостижим: при
`pg_stat_statements.max=5000` вытеснений нет, правило проверяют тесты. Интеграционные тесты `heavy.sql` не требуют (нужные строки pgss
они создают сами).

### 13.2 Автотесты

**`pg-advisor` (unit):** `PgssCaps` по версиям 1.8–1.12 и строкам вида `1.10`; эвристики §6 на границах порогов и по режимам
(stddev только с момента сброса); доли и hit ratio; `plan::from_json` на фикстурах PG 13–18 (seq scan, index scan, hash join,
nested loop, параллельный план с воркерами, CTE и InitPlan, Append по партициям, Sort на диске, JIT, триггеры, план без ANALYZE,
`never executed`) — числа узлов, собственное время, горячие узлы, ошибка оценки, `extra`; `to_text` — проверки подстроками (golden-текстов нет), JIT без времён (без ANALYZE и `TIMING OFF`); `plan::text_in` — пары JSON / текст
(psql обычный и с `+`, ANALYZE + BUFFERS, `TIMING OFF`, `never executed`, параллельный Gather, auto_explain), круговой тест; `queries::rules` — исключение `own`.
P3b: каждое правило — положительные и отрицательные unit-случаи на построенных узлах (пороги, `never executed`, `Limit`, Gather Merge,
JIT-объект PG 17+, параллельный JIT), `columns_of` (вложенные `AND`, кавычки, текстовые и прочие приведения, ключ соединения справа);
`tests/plans.rs::analyze_fixtures_trigger_expected_recommendations` — восемь фикстур `*_analyze` с `pg-perf` дают ожидаемые правила
(три фикстуры пересняты в P3b, запросы — в комментарии теста и в скрипте плана P3a), `partition_grouping_and_parallel_jit_on_fixtures`;
пары JSON / текст — в P3a; `compare` (новые, исчезнувшие, деградировавшие, ускорившиеся, сброс между снимками, уменьшившиеся счётчики);
kcache в `Counters` (сложение, дельта, сброс kcache, `without_kcache`, сортировка `cpuTime`).

**`src-tauri` (unit):** `StatementsBuffer` — дельты, первый тик без дельт, вытеснение ключа, смена `stats_reset`, предел записей
(вытеснение целыми тиками), суммы по окнам и покрытие; `explain::tier` — `ModifyTable` в корне и в CTE, `CREATE`, комментарии
перед первым словом; `explain::params::count_params` (лексер PostgreSQL: литералы, идентификаторы, комментарии, долларовые кавычки, предел 65 535); `llm` — состав markdown и пределы; SQL по `PgssCaps` — снимки текста запросов
(`snapshots/diag-sql/`, как P2); `ActionRecord` с `explain`; P3b: формат файла снимка, права 0600 / 0700, листинг (заголовок, нечитаемые
файлы), удаление и удаление с подключением, отказ в ту же секунду, порядок пары, покрытие после сброса, режим «между снимками» без
подключения; `validate_ddl` (обход через `;` в кавычках, комментарии, `$`, обратная косая черта); `ensure_write_confirmed`;
`statements_sql` с kcache по версиям и проба; `wait_sampling_plan`, кэш профиля и ошибок, `pick_waits`; `readiness` — `hint`.

**Интеграция** (`GPG_IT_PERF=localhost:5471`; без переменной тест пропускается, как с матрицей `GPG_IT_MATRIX`; на CI — workflow `perf`, §13.3):
- `it_queries`: отчёт «с момента сброса» под нагрузкой — строки, итоги, тексты по ключам, `Unavailable{others}` под ролью без
  `pg_read_all_stats`; сэмплер — два тика, дельты, окно; `query_detail` — `running` на 14+, разбивка; на матрице 13–18 — SQL
  по версиям pgss выполняется (`pg_stat_statements` есть на `gp-pg13..18`), на 13 — `unavailable: running`.
- `it_explain` (ярус записи идёт под ролью `glafira_app`: у `glafira_diag` на `pg-perf` нет прав записи в `shop`, и EXPLAIN DML
  под ней — `NOT_AUTHORIZED` с подсказкой о правах записи; побочные эффекты проверяются на `lo_creat` — он транзакционен, и откат
  его отменяет, — а не на `nextval`: сдвиг последовательности откат не отменяет, а без права `USAGE` `nextval` даёт 42501):
  пробный план и отношения; `GENERIC_PLAN` на 16+ и `FEATURE_UNAVAILABLE` (`genericUnsupported`) на 15; значения параметров `unknown`;
  ANALYZE SELECT без подтверждения и без строки журнала; UPDATE — ярус записи, без `confirm` на prod → `CONFIRMATION_REQUIRED`,
  с подтверждением — выполнен и откатан (число строк не изменилось), строка журнала `explainAnalyze`; volatile-функция с записью
  в ярусе чтения → `EXPLAIN_WRITES`; две инструкции → `INVALID_INPUT`; `pg_sleep` сверх срока → `TIMEOUT`; read-only профиль →
  `READ_ONLY` на ярусе записи, ярус чтения работает; на реплике (`gp-pg-replica`) ярус записи → `FEATURE_UNAVAILABLE`.
- P3b `it_hypopg` (2 теста): индекс-гипотеза снижает стоимость и используется; `hypopg_list_indexes` пуст на том же клиенте после
  успешной проверки, после сбоя пробы «после» и после отвергнутой гипотезы (`errors.hypo.rejected`); неквалифицированные имена работают с
  `search_path` запроса и не работают без него. `it_explain`: без ANALYZE пропущено 9 правил, ANALYZE `shop.orders WHERE status = $1`
  даёт `plan_seq_scan_filter`; `forceWrite` и ANALYZE `UPDATE` без `writeConfirmed` отклонены без записи журнала, с флагом — выполнены
  и откатаны. `it_snapshots`: снимок 0600, порядок списка, сравнение в обоих порядках, режим «между снимками» после отключения (строки,
  покрытие, тексты, своя строка, `pgss.max`, CPU), поиск, отказ сортировки по stddev, удаление и `notFound`. `it_queries`:
  `kcache_columns_on_perf` (на `pg-perf` CPU и сортировка, на `pg-lab` — `sortUnavailable` и без kcache),
  `wait_sampling_profile_on_perf` (профиль, `sinceMs = 0`, повторное чтение из кэша без вызова профиля).

**Фронтенд (vitest):** `QueriesView` — режимы, покрытие, пустое окно, серые сортировки, `unavailable`, открытие карточки;
`QueryTab` — обзор без буфера, таблицы после плана, выбор источника текста; `PlanTree` — развёртка, горячие узлы, пометки
ошибки оценки; `ExplainTab` — форма из `$N`, `GENERIC_PLAN` по версии, вставка JSON и текстового плана с обрамлением psql, поиск и пометка «Глафира», подтверждение яруса
записи; `findingText` для новых находок; `pnpm i18n:check`.

**Смоук GUI** на `pg-perf` под `heavy.sql` — чек-лист в плане (`docs/smoke/`): топ в обоих режимах, карточка, план и ANALYZE
обоих ярусов, консоль, «для LLM»; P3b — `docs/smoke/p3b.md`: рекомендации по каждому правилу (запросы консоли, а не только `heavy.sql`),
hypopg (на реплике — шаг «проверить»), снимки, kcache, `pg_wait_sampling`, подсказки готовности.

### 13.3 CI

Новых нативных зависимостей нет (`pg_query` не берётся): сборка на трёх ОС не меняется. `pg-advisor` и unit-тесты `src-tauri` —
в CI. Интеграционные тесты идут в job `linux` (`.github/workflows/ci.yml`) на каждый push, но без `GPG_IT_PERF`: части
`it_queries` и `it_explain` на `pg-perf` там пропускаются. Сервис `pg-perf` — в профиле `perf` (образ собирается из пакетов PGDG на
каждом запуске), и тесты на нём гоняет отдельный workflow `.github/workflows/perf.yml` — по расписанию (ночью) и по
`workflow_dispatch`: стенд с профилями `matrix` и `perf`, `scripts/wait-perf.sh`, `--test it_queries --test it_explain --test it_hypopg
--test it_snapshots` (P3b; там же `GPG_IT_LAB` для части «без kcache»). macOS и
Windows — только unit-тесты и сборка. Фикстуры планов — в репозитории. Решение владельца — §15, п. 16.

---

## 14. Документация

- `docs/guide/{ru,en}/queries.md` (новая глава, `GUIDE_CHAPTERS`): топ и режимы, покрытие окна и неполный топ, карточка,
  план и дерево, ярусы EXPLAIN ANALYZE и что откат не отменяет, консоль и вставка плана (JSON и текст), поиск, пометка «Глафира», «для LLM»; P3b — рекомендации
  (команды приложение не выполняет), hypopg, снимки, расширения. F1 из инструмента и вкладок `query` / `explain`.
- `docs/admin.md`: Права — `pg_read_all_stats` для чужих строк, pgss в базе подключения, `track_planning`, `pg_read_all_data`
  для EXPLAIN, права записи для яруса записи; Нагрузка — одно чтение pgss в минуту на узел (без текстов), тексты по
  запросу, EXPLAIN на отдельном соединении; Действия — ярусы EXPLAIN ANALYZE, что пишется в журнал; Данные и логи — снимки
  (путь, 0600, удаление из приложения).
- `dev/perf/README.md` — сервис, нагрузка, роли.
- `docs/guide/*/diagnostics.md` — ссылка на новую главу; `CHANGELOG.md` — при релизе.
- P3b: разделы главы «Запросы» `{#recommendations}`, `{#snapshots}`, `{#extensions}`; `docs/admin.md` — необязательные расширения
  (preload, перезапуск, `CREATE EXTENSION`, `profile_queries`, права `EXECUTE`), нагрузка kcache / `pg_wait_sampling` / hypopg, `writeConfirmed`,
  снимки в «Данные и логи»; смоук — `docs/smoke/p3b.md`.

---

## 15. Отступления от концепции

1. Без `pg_query`: затронутые таблицы — из плана, число параметров — по `$N`; парсер придёт в P4.
2. EXPLAIN ANALYZE запроса-чтения — без подтверждения и без имени подключения на prod: защищает транзакция `READ ONLY`;
   подтверждение и имя на prod — только у яруса записи. Границы этой защиты — п. 10.
3. «Взять из текущего выполнения» — не извлечение литералов, а выбор текста выполняющегося экземпляра целиком.
4. Текст запроса не переформатируется (нужен парсер): подсветка ключевых слов средствами CSS.
5. Сэмплер pgss живёт, пока открыт потребитель (концепция о жизненном цикле молчит); буфер — 200 000 записей дельт, а не строк.
6. Режим «между снимками» — в P3b; `pg_qualstats` — после MVP. Текстовый парсер планов сначала планировался в P3b, но перенесён в P3a (п. 19, решение владельца 2026-11-01).
7. У сэмплера pgss нет своего соединения: одно чтение в минуту делит соединение `Sampler` с сэмплером активности.
8. Деления Free / Pro нет (§1, «Лицензия»).

**На решение владельца** (открыты, реализация P3a их не решает):
9. `TextParam` (значения параметров текстовым форматом с типом `unknown`, §8.1) опирается на скрытый модуль
   `tokio_postgres::types::private` (`BytesMut` — тот же путь использует макрос `to_sql_checked!`). Альтернатива — прямая
   зависимость `bytes = "1"` (крейт уже есть в `Cargo.lock`, но это новая зависимость манифеста при правиле «без новых
   крейтов»). Нужен выбор: оставить как есть или добавить зависимость. Финальное ревью P3a рекомендует `bytes = "1"` как
   предпочтительный вариант: `BytesMut` из `bytes` — документированный способ реализовать `ToSql`, и приложение перестаёт
   зависеть от внутреннего модуля чужого крейта.
   **Решение владельца (2026-10-30):** прямая зависимость `bytes = "1"`; `TextParam` пишет в `bytes::BytesMut`.
10. `BEGIN READ ONLY` не останавливает нетранзакционные административные функции — например `pg_terminate_backend`,
    `pg_cancel_backend`, `pg_stat_reset*`, `pg_stat_statements_reset`, `pg_reload_conf`: EXPLAIN ANALYZE такого `SELECT` пройдёт
    ярусом чтения без подтверждения (п. 2) и выполнит функцию. Список шире, чем казалось: финальное ревью добавило
    `pg_drop_replication_slot` (роль с `REPLICATION` обходит им ограждение удаления слотов P2 — предпросмотр, подтверждение именем
    слота, журнал), `pg_switch_wal`, `pg_create_restore_point`, `pg_promote` и внешние вызовы (`dblink_exec`, `http`). Нужно решение
    владельца, как закрыть этот случай. Вариант от ревью: deny-list по VERBOSE-плану пробы — вызываемые функции схемно-квалифицированы
    в `Output` и условиях плана, и инструкция с функцией из списка переводится в ярус записи (подтверждение, журнал). Он не видит
    функции, вызванные внутри тела другой функции или триггера, поэтому закрывает случай частично.
    **Решение владельца (2026-10-30):** deny-list по VERBOSE-плану пробы (`explain::tier::ADMIN_FUNCTIONS`, §8.3 шаг 1):
    вызов из списка переводит инструкцию в ярус записи с предупреждением `adminFunction`. Вызовы внутри тел функций и триггеров
    список не видит — закрытие частичное.
11. Включённое время узла под `Gather` и в параллельном плане считается формулой §8.5 (`actual.totalMs × loops / processes`), а не
    одобренным раньше `actual.totalMs × loops`: умножение на `loops` там давало суммарное время всех процессов, а не время по часам, и
    доли узлов под `Gather` завышались. Нужно подтвердить замену или вернуть `× loops` (вне `Gather` формулы совпадают).
12. Покрытие окна `coverageS` (§4.2) считается от первого тика окна до последнего плюс минута и охватывает промежутки, когда сбор
    стоял: «накоплено N из M мин» может показывать больше, чем собрано. Вариант — `min(ticks × 60, W)`, то есть только фактически
    собранные минуты. Нужно решение владельца.
    **Решение владельца (2026-11-01):** считать только реально собранное — число тиков с дельтами × интервал, не больше окна (§4.2).
13. Итоги карточки (`totals`, §7.2) считаются по всему представлению pgss (все базы и уровни `toplevel`), а итоги топа — по набору
    после фильтров, поэтому доля вызовов и находка N+1 в карточке могут отличаться от топа. У столбца «За 60 мин» карточки нет
    показателя покрытия (сколько минут реально собрано), окна 5 и 15 мин в карточке не выбираются. Нужно решение: оставить
    (в руководстве сказано) или сделать итоги карточки как у топа и показать покрытие.
14. Сбой записи журнала у `explainAnalyze` тихий (§8.4): запрос уже выполнен и откатан, но следа в журнале нет, а окно
    `journalNotWritten` P2 здесь не показывается. Нужно решение: показывать то же сообщение, что у остальных действий.
15. «Скопировать для LLM» есть в режиме «Запрос» консоли и в карточке, а в режиме «Вставить план» кнопки нет (§8.7): markdown из
    вставленного плана собирался бы без запроса и статистики. Нужна ли она — по желанию владельца (опционально).
16. Интеграционные тесты `it_queries` и `it_explain` на `pg-perf` идут на CI (§13.3), а не только локально, как предполагалось: job
    `linux` собирает образ `pg-perf` из пакетов PGDG (`apt-get` на каждом запуске, кэша слоёв образа в CI нет) и тратит на тесты по
    оценке ревью порядка 3 минут. Нужно подтвердить эту цену или вынести `pg-perf` в отдельную job (по расписанию или
    `workflow_dispatch`).
    **Решение владельца (2026-10-30):** отдельный workflow `perf` (расписание и `workflow_dispatch`), `pg-perf` — в профиле `perf`; §13.3.
17. На профилях не-prod единственная защита записи ярусом записи (EXPLAIN ANALYZE, который пишет) — окно подтверждения в интерфейсе:
    Rust-ограждение там не требует полезной нагрузки подтверждения, поэтому запрос, посланный мимо окна, выполнится без него. Вариант
    «в глубину» — флаг запроса `writeConfirmed`: без него Rust отказывает ярусу записи, окно подтверждения его выставляет. Нужно
    решение владельца для P3b.

**Отступления реализации P3a от текста этой спеки** (не решения владельца, а перечень того, что не сделано или сделано иначе;
бэклог P3b/P4):
    **Решение владельца (2026-10-30):** флаг `writeConfirmed` — в P3b (поправка в плане P3b).
18. Топ и карточка: (а) §4.7 — нет значка «топ неполный» (о вытеснении говорит находка `queries_incomplete`), доля в таблице —
    колонка с числом, а не полоса, карточку открывает клик по тексту запроса, а не по всей строке, меню строки «Открыть в консоли» нет
    (консоль открывает кнопка в шапке инструмента); (б) §7.3 и п. 4 выше — подсветки ключевых слов нет, текст показан без неё;
    (в) §7.3 — метрики карточки в двух столбцах «С момента сброса» и «За 60 мин», окно 5 и 15 мин не выбирается, покрытие окна не
    показано (п. 13); (г) §7.1 — у одиночного сервера в шапке карточки нет переключателя узла; (д) §4.7 — у «Запросов» «за лидером»
    при смене лидера сэмплер и отчёт переходят на нового молча, строки «Узел сменился» (как у других вкладок P2) нет.

19. **Поправки по смоуку P3a (2026-11-01).** Смоук P3a (17 шагов) выявил расхождения; решения владельца и исправления:
    (а) *Парсер текстовых планов перенесён из P3b в P3a* (§1 п. 9, §2 п. 6, §9.4): `plan::text_in::from_text` и `plan::json::from_any`,
    `parse_plan` принимает JSON (в том числе с обрамлением psql) и текст psql / auto_explain, формат определяется сам, ошибка — с номером строки;
    подпись и пояснение поля «Вставить план» говорят «текст или JSON». Задача 5 плана P3b снята.
    (б) *Собственные запросы Глафиры* (§4.2, §6): периодические SQL приложения несут метку `/* glafira */`; строки с `own` помечены в топе
    «Глафира», не участвуют в находках и не входят в итоги, по которым считаются находки (опрос топологии давал ложный N+1 и размывал долю вызовов); колонка «Доля» — от всей нагрузки у всех строк. Строки, записанные версией без метки, узнаются по тексту запросов опроса узла (с точностью до литералов и `$N`).
    (в) *Поиск* по подстроке текста в топе (§4.2): `search`, `ILIKE` на сервере, итоги — по всей нагрузке.
    (г) *Покрытие окна* — число тиков с дельтами × интервал, а не пролёт от первого тика (п. 12, §4.2).
    (д) Имена `datname` / `rolname` ушедших из `pg_stat_statements` ключей сохраняются, пока на них ссылаются тики окна (§5); неактивная
    кнопка строки объясняет причину.
    (е) Находки топа показывают начало текста запроса (§4.7); подвал на pgss без `pg_stat_statements_info` пишет «время сброса недоступно»,
    «Планирование» при выключенном `track_planning` — «—», блок «Выполняющиеся экземпляры» на серверах до 14 скрыт целиком (`PgssMeta.hasInfo`,
    `QueryDetail.trackPlanning`).
    (ж) *«Скопировать для LLM» без дублей индексов* (§7.4): DDL таблицы каталога уже содержит `CREATE INDEX` индексов вне ограничений, поэтому
    `LlmTable.indexes` убран, каждый индекс стоит один раз.
    (з) *JIT без времён* (§8.5): времена `Option`; строка `Timing:` в `to_text` и «0 мс» в сводке — только при реальных временах.
    (и) Интерфейс: кнопки карточки одним рядом под текстом («Получить план» вместо «План», «Открыть в консоли», «Копировать запрос»,
    «Скопировать для LLM»), в консоли «Скопировать для LLM» — в ряду копирования результата; горячий узел плана — фон и метка «горячий»;
    кнопка «План (общий)» при `GENERIC_PLAN`; режим запуска в сводке; отказы и ошибки — красной плашкой без плана прошлого запуска;
    подсказка «Выполняется экземпляр (PID …) — взять его текст»; в окне подтверждения без таблиц — «Данные изменятся и откатятся.».

20. **Решения по ходу реализации P3b (2026-10-30 — 2026-11-01).** Решения контролёра плана по вопросам исполнителей и ревью; текст §9–§13
    уже приведён к ним:
    (а) *JIT по процессам* (§9.1): сервер складывает время JIT лидера и воркеров и сравнивает с временем по часам, поэтому
    `plan_jit_heavy` срабатывал почти на любом параллельном плане; время JIT делится на число процессов (1 + запущенные воркеры), `jitMs`
    в параметрах — как отдал сервер, плюс `processes`.
    (б) *Группировка сканов партиций* (§9.1): `plan_seq_scan_filter` на каждой партиции давал по рекомендации на партицию (24 на стенде);
    сканы под одним `Append` объединяются в одну рекомендацию со строкой `CREATE INDEX` на партицию и без гипотезы.
    (в) *Приведения в колонках* (§9.1, уточнение 1 плана): снимается только приведение к текстовому типу; `::date` и прочие делают условие
    выражением — индекс по самой колонке такому условию не помогает, а команда вводила бы в заблуждение.
    (г) *Шаблоны команд*: нет колонок — `(<columns>)`, lossy bitmap — `SET work_mem = '<work_mem>';` (объём битовой карты по страницам heap
    не считается), без гипотезы; фронтенд узнаёт шаблон по этим токенам и просит заменить `<…>`.
    (д) *«Показать узел»* отмечает все `nodeIds` (Sort и Limit, Nested Loop и скан, все партиции), а не только первый.
    (е) *Неизвестное правило* — карточка с `id`, командой и советом обновить приложение, а не пустой текст.
    (ж) *hypopg* (§9.2): одна транзакция `READ ONLY` с `search_path` запроса, `hypopg_reset()` до `ROLLBACK` (и повтор после, если
    транзакция прервана); проверка DDL — одна инструкция `CREATE INDEX ON`, без комментариев, `$` и обратной косой черты; запрос — сохранённый
    запуск, а не текущие поля формы. На реплике проверка идёт на реплике и требует там пакет `hypopg`; работа на hot standby автотестами не
    проверена (смоук P3b, шаг 13 — «проверить»).
    (з) *`writeConfirmed`* (п. 17): ярусу записи нужен флаг из окна подтверждения на любом окружении; `forceWrite` без флага отклоняется до
    соединения, ярус записи, найденный пробой, — после пробы `READ ONLY` и до выполнения (раньше пробы ярус неизвестен).
    (и) *Снимки* (§9.3): отказ при втором снимке узла в ту же секунду (а не замена); пара в любом порядке; снимки разных узлов — отказ
    (`differentMembers`), а не сравнение с пометкой; нечитаемые файлы видны в списке и удаляются; снимки удаляются вместе с подключением;
    в архив диагностики не входят; режим «между снимками» работает без открытого подключения; покрытие — время между снимками, а при сбросе —
    от сброса до второго снимка (п. 12: значок значит одно — время, за которое накоплены дельты); тексты — 500 верхних по времени и 500 по
    вызовам.
    (к) *`pg_stat_kcache`* (§9.5): колонки только при пригодном расширении (проверка каталога и пробный вызов, иначе — как без него, чтобы
    одно сломанное необязательное расширение не ломало топ); «между снимками» — только если оба снимка с kcache; с pgss 1.8 и kcache ≥ 2.2
    join по `k.top` — учитывается CPU только верхнего уровня; без расширения колонок нет (а не прочерки).
    (л) *`pg_wait_sampling`* (§9.5): `profile_queries` — перечисление `none` / `top` / `all` (логический `on` / `off` в старых версиях),
    версия 1.1+ (колонка `queryid`); профиль кэшируется на 30 с на запрос и узел, ошибки состояния сервера — на 10 мин; при недоступности —
    сэмплы «Сессий»; ожидания одного `queryid` суммируются по базам; подпись источника и охвата над таблицей.
    (м) *Готовность* (§9.6): кроме «что даёт» — строка «как включить» (`readiness.extension.enable.<name>`), тексты подсказок оставлены
    дословно.
    (н) *Фикстуры* `index_only_heap_fetches_analyze`, `parallel_jit_analyze`, `append_partitions_analyze` пересняты другими запросами
    (поправка 3 плана P3b); скрипт снятия в плане P3a обновлён.

---

## 16. Критерии готовности

**P3a:**
1. На `pg-perf` под `heavy.sql` топ показывает режим «с момента сброса» сразу и «за окно 5 мин» через пять минут с верным
   покрытием; на первой строке по числу вызовов — `SELECT * FROM shop.customers WHERE id = $1` (три инструкции из тринадцати, около 23 % вызовов);
   находки `query_n_plus_one`, `query_temp` и `query_low_hit_ratio` появляются на своих запросах сценария. Оговорка про N+1: он виден
   на образе с обновлённым `heavy.sql` (или со свежим файлом, подложенным `docker cp`); на старом стенде его проверяют одной
   инструкцией в цикле (`pgbench` с файлом из одного запроса, смоук, последний шаг) или тестом правила на границе порога.
   `query_unstable` на запросе к `audit` расчётно на пороге правила, `queries_incomplete` стендом недостижим (§13.1).
2. На `pg-single` (без pgss) инструмент показывает `unavailable` с подсказкой; на матрице 13–18 отчёт строится по версии pgss,
   недоступные колонки серые.
3. Карточка открывается из строки и из находки, показывает историю по тикам, разбивку, выполняющиеся экземпляры (14+) и
   ожидания после открытия вкладки «Профиль ожиданий»; после первого плана — таблицы со ссылками.
4. План параметризованного запроса — через `GENERIC_PLAN` на 17 и через форму значений на 15; план по тексту выполняющегося
   экземпляра.
5. ANALYZE SELECT выполняется без подтверждения в `READ ONLY`; UPDATE требует подтверждения (и имени подключения на prod),
   откатывается, оставляет строку журнала; на read-only профиле ярус записи недоступен; срок прерывает выполнение с `TIMEOUT`.
6. Консоль разбирает вставленный план — JSON-план psql, текстовый EXPLAIN и запись auto_explain; «для LLM» даёт markdown с запросом, статистикой, планом и DDL таблиц.
7. Закрытие последней вкладки-потребителя останавливает чтения pgss; повторное открытие продолжает буфер с промежутком.
8. Строки — en и ru, `pnpm i18n:check` зелёный; глава открывается по F1; CI зелёный на трёх ОС.

**P3b:**
1. Каждое из 11 правил срабатывает на своей фикстуре и на своём запросе `heavy.sql` и молчит на отрицательной фикстуре.
   *Уточнение по реализации:* на стенде часть правил срабатывает не на запросе `heavy.sql`, а на запросе консоли из смоука P3b, шаг 5
   (`heavy.sql` даёт Bitmap Heap Scan вместо Index Only Scan, фильтр `events` не по ключу партиционирования, JIT ниже порога); `plan_nested_loop_seq`
   стендом не воспроизводится и проверяется unit-тестами; отрицательные случаи — unit-тесты на построенных узлах.
2. «Проверить гипотезу» показывает стоимость «была / стала», использование индекса и размер; сессия после проверки чиста.
3. Снимок снимается и хранится с правами 0600; два снимка сравниваются; режим «между снимками» строит топ.
4. ~~Текстовые планы psql с PG 13–18 разбираются в то же дерево, что JSON.~~ Выполнено в P3a (решение владельца 2026-11-01, §15 п. 19).
5. На `pg-perf` колонки CPU и диск заполнены, ожидания карточки берутся из `pg_wait_sampling`; без расширений поведение P3a.
6. Подсказки готовности к `track_planning`, `pg_read_all_data` и расширениям верны для `glafira_diag`; CI зелёный на трёх ОС.
