# P3a «Видеть» — топ запросов, сэмплер pg_stat_statements, карточка запроса, EXPLAIN и планы: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Реализовать часть P3a спецификации `docs/superpowers/specs/2026-10-29-glafira-postgres-p3-design.md` (§1, пункты 1–6 и 11 без разделов о рекомендациях, снимках и расширениях): стенд `pg-perf`, сэмплер `pg_stat_statements` с буфером дельт, инструмент «Запросы» (режимы «с момента сброса» и «за окно»), находки без плана, вкладка запроса с историей и выполняющимися экземплярами, EXPLAIN и EXPLAIN ANALYZE в два яруса, парсер JSON-плана и дерево узлов, консоль планов, «Скопировать для LLM», готовность (`track_planning`, права чтения), глава руководства «Запросы».

**Architecture:** Инструмент «Запросы» — инструмент диагностики P2 (`run_diagnostic` с `tool: queries` и полем `queries`): режим «с момента сброса» — одно чтение `pg_stat_statements(showtext := false)` на соединении пула `Catalog`, режим «за окно» — сумма дельт буфера сэмплера pgss без обращения к серверу. Сэмплер pgss — второй экземпляр реестра сэмплеров P2, обобщённого трейтом `Sampling` (тик 60 с, соединение пула `Sampler`, буфер дельт на 60 мин, живёт, пока подписана вкладка). Чистые типы и правила — в `pg-advisor` (`pgss`, `queries`, `plan`), сбор и EXPLAIN — в `src-tauri` (`diag/queries`, `diag/query_detail`, `sampler/statements`, `explain/`). EXPLAIN идёт на отдельном соединении `for_explain()` в транзакции: ярус определяется пробным планом, чтение — `BEGIN READ ONLY` без подтверждения, запись — guard P2b, подтверждение и журнал. Интерфейс: `QueriesView` внутри `DiagTab`, вкладки `query` и `explain`, общий `PlanRunner` и `PlanTree`.

**Tech Stack:** Tauri 2, Rust (edition 2024): tokio, tokio-postgres 0.7, serde/serde_json; React 19 + TS + TanStack Query + Zustand + i18next; Vitest + Testing Library; Docker Compose (стенд `gp-*`, новый сервис `pg-perf` от `postgres:17` с пакетами PGDG).

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`), команды выполняются из `$P`. Образцы P2: `src-tauri/src/diag/sessions.rs` (модуль инструмента), `src-tauri/src/sampler/` (реестр, буфер, источник), `src-tauri/src/actions/slots.rs` (действие с журналом), `src/components/diag/{DiagTab,WaitsView,FindingsPanel}.tsx`, `src/lib/useSampler.ts`.

## Global Constraints

- PostgreSQL 13–18; `pg_stat_statements` 1.8–1.12 — набор колонок по `pg_extension.extversion`, а не по версии сервера (спец. §3.3). `pg_stat_activity.query_id` — 14+, `GENERIC_PLAN` — 16+. Платформы: macOS (arm64, x64), Windows x64, Linux x64.
- Соединения: отчёт «с момента сброса», тексты, `query_detail`, DDL — пул `Catalog` (`SESSION_OPTIONS`, предел `catalog_limit`, `on_catalog_node_within`); сэмплер pgss — пул `PoolPurpose::Sampler` (общий с сэмплером активности, одно чтение в минуту, предел запроса 5 с); EXPLAIN и гипотезы — отдельное соединение `ConnectSpec::for_explain()` с `EXPLAIN_SESSION_OPTIONS = "-c statement_timeout=5000 -c lock_timeout=1000 -c DateStyle=ISO"` (без `search_path=pg_catalog` и без `default_transaction_read_only`), закрывается по завершении.
- EXPLAIN всегда внутри транзакции и ровно одна инструкция пользователя расширенным протоколом; пробный план и ярус чтения — `BEGIN READ ONLY`; ярус записи — `ensure_mutation_allowed`, `ConfirmDialog`, `BEGIN … ROLLBACK`, запись журнала `explainAnalyze`. Параметры — с типом `unknown`. Опции: пробный план `FORMAT JSON, VERBOSE, SETTINGS[, GENERIC_PLAN]`; ANALYZE — `ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON`. Срок ANALYZE 5…600 с (по умолчанию 30), пробного плана — 15 с.
- Без `pg_query` и без новых нативных зависимостей; без новых крейтов вообще (счётчик `$N` — ручной разбор, не `regex`). `pg-advisor` — только serde / serde_json, без I/O.
- `queryId` в JSON — всегда строка (`crate::serde_util::i64_string`); `Option` → `null`; все команды Tauri возвращают `Result<T, AppError>`; JSON — camelCase; `AppError.code` — SCREAMING_SNAKE_CASE. Новый код P3a — `EXPLAIN_WRITES` (не `retryable`).
- Пользовательские тексты из Rust — только `ltext!` / `app_err!` с английским шаблоном, равным тексту `src/locales/en.json`; переводы — `src/locales/{en,ru}.json`; ru == en — в `docs/i18n/same-as-en.json`; `hints` — последний ключ верхнего уровня; кириллица в литералах Rust вне `#[cfg(test)]` запрещена (`pnpm i18n:check`). Комментарии — по-русски. Тексты находок — ключи `findings.<id>.title` и `findings.<id>.fix`.
- Параметры находок называются по единицам: `…S` — секунды, `…Ms` — миллисекунды (новый суффикс P3a), `…Bytes` — байты, `…Pct` — проценты, `count` — число объектов, `command` — готовая команда (приложение её не выполняет).
- Приложение ничего не меняет на сервере, кроме транзакций EXPLAIN ANALYZE, которые всегда откатываются; рекомендованные команды только показываются (P3b).
- Ни одна операция не блокирует UI: файлы — в `run_blocking`; у каждой сетевой операции — таймаут.
- Стенд Docker (`gp-*`) запущен: никогда не выполнять `docker compose down`, не удалять и не пересоздавать контейнеры и тома `gp-*`, не трогать `ke-kafka`; новые сервисы — `docker compose -f dev/docker-compose.yml up -d --no-recreate <сервис>`; лидер Patroni — любой узел.
- Сообщения коммитов — по-русски (`feat(queries): …`, `test(explain): …`), последняя строка — трейлер `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; без `Signed-off-by`.
- Проверки каждой задачи: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`, `cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`, `cargo test --manifest-path src-tauri/Cargo.toml --lib`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`, `cargo fmt --manifest-path src-tauri/Cargo.toml --check`, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`. У задач со стендом — интеграционные тесты:
  `GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 GPG_IT_LAB=localhost:5470 GPG_IT_PERF=localhost:5471 GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1`
  (`pg-perf` поднимается `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf`, матрица — `--profile matrix up -d --no-recreate` и `scripts/wait-matrix.sh`).

## Уточнения спецификации (решения плана)

1. **Вкладки не сохраняются между запусками** (в приложении нет сохранения workspace, спец. §7.1 ошибается): вкладки `query` и `explain` живут, как остальные, до закрытия приложения. Спека поправлена в задаче 16.
2. **Счётчик `$N` — ручной разбор** (`$` и следующие цифры), без крейта `regex`; максимум — число параметров. Текст pgss литералов не содержит, ложных совпадений нет.
3. **Реестр сэмплеров обобщается трейтом `Sampling`** (`type Sample`, `type Buffer`, `type Event`, `interval`, `push`, `error_event`, `stopped_event`, `fatal`): `SamplerRegistry<S>`, `SamplerSink<E>`, `SampleSource<T>`; активность — `Activity`, pgss — `Statements`; `fatal` у pgss включает `FEATURE_UNAVAILABLE` (расширение исчезло — остановка, не повторы). Публичные имена P2 остаются (`SamplerEvent`, `Buffer`, `RawSample`, `sampler::start`).
4. **Кеш текстов — в буфере узла сэмплера pgss** (`StatementsBuffer::texts`): живёт, пока живёт узел (до закрытия подключения); до 2000 текстов, при переполнении очищается целиком. Инструмент и карточка всегда подписаны на сэмплер, так что узел есть.
5. **Шапка отчёта «за окно» — из последнего тика**: запрос сэмплера pgss начинается с колонок `HEAD_SQL` (версия, `in_recovery`, время сервера), буфер хранит `last_head`. Сэмплер только запускается — `unavailable{part: window}` и `head(c)` обычным путём.
6. **Первый тик — только база**: дельт нет, событие `tick` с `ticks: 0`. Строка с `queryid = NULL` (чужая без `pg_read_all_stats`) отбрасывается, отчёт получает `unavailable{part: others}`.
7. **Новый `SessionKind::Statements`** в реестре длительных сессий — остановка сессии отписывает от реестра pgss, как у `Sampler`.
8. **«Скопировать для LLM» — данные из Rust, markdown во фронтенде**: команда `query_llm_bundle` отдаёт среду, текст, статистику, текст плана и DDL таблиц; `src/lib/llmMarkdown.ts` собирает markdown с заголовками на языке интерфейса (словари i18n живут во фронтенде). Спека §7.4 называла команду `query_llm_markdown`; поправлено в задаче 16.
9. **Консоль — одна на подключение** (`tabId = explain:<connectionId>`): «Открыть в консоли» подставляет текст в уже открытую консоль и активирует её, как глава справки. История запусков внутри вкладки.
10. **`DiagTool` получает `queries`**, но инструмент не идёт через `useDiagnostic`: у него другой запрос (`queries`) и другое обновление (по тику сэмплера в режиме окна) — хук `useQueriesReport`. `DiagTab` для `queries` отключает свой `useDiagnostic` (`live: false`) и отдаёт тело `QueriesView`, которая рисует общую `DiagHeader` сама.
11. **Ссылка `openQuery` без узла** (находка отчёта «за лидером», узел отчёта тоже неизвестен) кнопки не даёт: вкладка `query` требует имени узла.
12. **Пробный план, отвергнутый 25006** (CREATE … AS на отдельных версиях), повторяется в `BEGIN … ROLLBACK` только для определения яруса; выполнения нет.
13. **Ошибка «вторая инструкция»** распознаётся по тексту сервера `cannot insert multiple commands into a prepared statement` (SQLSTATE 42601) — только она; прочие 42601 — `errors.explain.server` с позицией.
14. **`EXPLAIN (WAL)`** добавляется только при `mode: analyze`: без ANALYZE опция `WAL` сервером отвергается.
15. **Колонки `oid`** (`userid`, `dbid`) читаются как `u32` (`Type::OID`); `wal_bytes` (`numeric`) приводится к `bigint` в SQL.
16. **`ExplainRequest.forceWrite`**: после `EXPLAIN_WRITES` кнопка «Выполнить как запись» повторяет запрос с `forceWrite: true` — ярус записи (guard, подтверждение, журнал) независимо от пробного плана. Спека §8.1 поля не называла; поправлено в задаче 16.
17. **`jit_deform_time` входит в `jit_generation_time`** (документация pgss 1.11) и к сумме JIT не прибавляется; спека §4.1 поправлена в задаче 16.

## Карта файлов

```
crates/pg-advisor/
  Cargo.toml                      без изменений (serde, serde_json)
  src/lib.rs                      + pgss, queries, plan
  src/pgss.rs                     PgssVersion, PgssCaps                                                      (задача 2)
  src/queries/mod.rs              QueryKey, Counters, Cumulative, QueryStat, Totals, QuerySort, stat, totals, top (2)
  src/queries/rules.rs            Mode, PgssState, evaluate                                                   (3)
  src/findings.rs                 FindingTarget::Query, FindingLink::OpenQuery, link_query, пороги            (2)
  src/plan/mod.rs                 PlanTree, PlanNode, Buffers, Jit, Trigger, finish (вычисляемые поля)        (8)
  src/plan/json.rs                from_json                                                                  (8)
  src/plan/text_out.rs            to_text                                                                    (8)
  tests/fixtures/plans/json/*.json фикстуры PG 13–18 с pg-perf                                               (8)
src-tauri/src/
  diag/mod.rs                     Tool::Queries, DiagRequest.queries, DiagData::Queries, validate, run      (4)
  diag/queries.rs                 detect, SQL по PgssCaps, read_cumulative, report, texts                   (4, 6)
  diag/query_detail.rs            QueryDetail, detail                                                       (7)
  sampler/registry.rs             Sampling, SamplerRegistry<S>, Activity                                    (5)
  sampler/statements.rs           StatementsBuffer, StatementsEvent, Statements, PoolStatementsSource, start (6)
  sampler/mod.rs, query.rs, state.rs, session.rs, commands/diag.rs                                          (5, 6)
  explain/mod.rs                  ExplainRequest, Params, ExplainResult, validate                            (9)
  explain/params.rs               count_params, first_word, valid_search_path                               (9)
  explain/tier.rs                 Tier, tier_of                                                             (9)
  explain/run.rs                  run (пробный план, ярусы, журнал)                                          (9)
  explain/llm.rs                  LlmSource, LlmBundle, bundle                                              (10)
  conn/pg.rs                      EXPLAIN_SESSION_OPTIONS, for_explain                                      (9)
  actions/conn.rs                 open_with, open_explain                                                   (9)
  actions/journal.rs              ExplainRecord, ActionRecord.explain                                       (9)
  error.rs                        ErrorCode::ExplainWrites                                                  (9)
  readiness/mod.rs                факты pgss_version, pgss_track_planning, read_all_data                    (11)
  commands/queries.rs             query_texts, start_statements_sampler, query_detail, query_llm_bundle     (4, 6, 7, 10)
  commands/explain.rs             explain, parse_plan                                                       (9)
  lib.rs                          регистрация команд
src-tauri/tests/
  it_stand.rs (+perf)  it_queries.rs (4, 6, 7)  it_explain.rs (9, 10)  common/mod.rs (perf_addr)
src/
  api/types.ts, api/client.ts, state/workspace.ts, components/Layout.tsx, components/help/helpChapterFor.ts,
  components/connections/tree/DiagnosticsNodes.tsx, components/diag/{DiagTab,DiagHeader,FindingsPanel,findingText,unavailable},
  lib/format.ts, lib/useStatementsSampler.ts, lib/useQueriesReport.ts, components/queries/{QueriesView,queriesText}   (12)
  components/queries/plan/{PlanTree,PlanNodeDetails,PlanSummary,planText}, test/plans.ts                        (13)
  components/queries/plan/{PlanRunner,ParamsForm,AnalyzeConfirm}, lib/{useExplain,useQueryDetail,llmMarkdown},
  components/queries/{QueryTab,QueryOverview,QueryPlan,CopyLlmButton,Sparkline}, components/TabContent.tsx (query),
  lib/useConnectionEvents.ts (queryDetail), components/diag/FindingsPanel.tsx (openQuery)                      (14)
  components/queries/ExplainTab.tsx, components/TabContent.tsx (explain), components/actions/ActionsJournal.tsx,
  components/connections/tree/DiagnosticsNodes.tsx (консоль)                                                   (15)
  locales/{en,ru}.json, docs/i18n/same-as-en.json                                                            (12–15)
dev/perf/{Dockerfile,20-init.sql,heavy.sql,README.md}, dev/docker-compose.yml, scripts/wait-stand.sh, .github/workflows/ci.yml, README.md (1)
docs/guide/{en,ru}/queries.md, src/guide/chapters.ts, components/help/helpChapterFor.ts, docs/admin.md, docs/smoke/p3a.md, спека (16)
```

**Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 (Rust) → 12 → 13 → 14 → 15 (фронтенд; 12 можно начинать после 4, но типы зеркалят задачи 4–10) → 16 (документация, последней). Задача 6 зависит от 4 (SQL и `Snapshot`) и 5 (трейт `Sampling`); 7 — от 6 (буфер); 9 — от 8 (`PlanTree`); 10 — от 8 и 7; 14 — от 13 (`PlanTreeView`); 15 — от 14 (`PlanRunner`).

---

### Task 1: Стенд — сервис `pg-perf` (PG 17, расширения, объёмная схема, сценарий `pgbench`)

**Files:**
- Create: `dev/perf/Dockerfile`, `dev/perf/20-init.sql`, `dev/perf/heavy.sql`, `dev/perf/README.md`
- Modify: `dev/docker-compose.yml` (сервис после `pg-lab`), `scripts/wait-stand.sh` (контейнер `gp-pg-perf`), `.github/workflows/ci.yml` (`GPG_IT_PERF`), `README.md` (таблица стенда), `src-tauri/tests/common/mod.rs` (`perf_addr`), `src-tauri/tests/it_stand.rs`
- Test: `it_stand.rs` (со стендом)

**Interfaces:**
- Consumes: `common::{env, superuser, connect_as, PG_USER, PG_PASSWORD_VALUE}`.
- Produces: `common::perf_addr() -> Option<String>` (`GPG_IT_PERF`); сервис `pg-perf` (`gp-pg-perf`, `127.0.0.1:5471`, PG 17): база `perf`, схема `shop` (таблицы `customers`, `orders`, `order_items`, `events` с 24 партициями, `audit`, `balances`), расширения `pg_stat_statements` (с `track_planning`), `pg_stat_kcache`, `pg_wait_sampling`, `hypopg`; роли `glafira_diag` (`pg_monitor`, `pg_signal_backend`, `pg_read_all_data`) и `glafira_app`; файл `/perf/heavy.sql` для `pgbench`.

- [ ] **Step 1: Падающий интеграционный тест**

В `src-tauri/tests/it_stand.rs` добавить:

```rust
/// Стенд P3 (`GPG_IT_PERF`): PG 17 с pg_stat_statements (track_planning), hypopg, pg_stat_kcache, pg_wait_sampling,
/// объёмной схемой и ролью диагностики с правом чтения таблиц.
#[tokio::test]
async fn perf_has_extensions_settings_and_data() {
    let Some(perf) = common::perf_addr() else { return };
    let c = common::superuser(&perf, "perf").await;
    let r = c
        .query_one(
            "SELECT current_setting('server_version_num')::int, \
                    current_setting('pg_stat_statements.track_planning'), current_setting('compute_query_id'), \
                    (SELECT array_agg(extname::text ORDER BY extname) FROM pg_extension \
                      WHERE extname IN ('pg_stat_statements', 'pg_stat_kcache', 'pg_wait_sampling', 'hypopg')), \
                    (SELECT count(*) FROM shop.orders)::bigint, \
                    (SELECT count(*) FROM pg_inherits WHERE inhparent = 'shop.events'::regclass)::bigint, \
                    pg_has_role('glafira_diag', 'pg_read_all_data', 'USAGE')",
            &[],
        )
        .await
        .unwrap();
    assert!(r.get::<_, i32>(0) >= 170_000);
    assert_eq!(r.get::<_, String>(1), "on");
    assert_eq!(r.get::<_, String>(2), "on");
    assert_eq!(
        r.get::<_, Vec<String>>(3),
        ["hypopg", "pg_stat_kcache", "pg_stat_statements", "pg_wait_sampling"]
    );
    assert!(r.get::<_, i64>(4) >= 500_000, "заказы загружены");
    assert_eq!(r.get::<_, i64>(5), 24, "24 партиции событий");
    assert!(r.get::<_, bool>(6), "glafira_diag читает таблицы приложения (EXPLAIN)");
    // Роль диагностики видит статистику запросов других ролей.
    let d = common::connect_as(&perf, "perf", common::PG_USER, common::PG_PASSWORD_VALUE).await;
    let n: i64 = d
        .query_one("SELECT count(*) FROM pg_stat_statements WHERE queryid IS NOT NULL", &[])
        .await
        .unwrap()
        .get(0);
    assert!(n >= 1, "pg_stat_statements заполнен хотя бы init-скриптом");
}
```

- [ ] **Step 2: Убедиться, что тест падает**

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_stand perf_has -- --test-threads=1`
Expected: ошибка компиляции (`perf_addr` нет).

- [ ] **Step 3: Образ и init-скрипты**

`dev/perf/Dockerfile`:

```dockerfile
# Стенд P3 (pg-perf): PostgreSQL 17 с расширениями, которых нет в официальном образе (hypopg, pg_stat_kcache,
# pg_wait_sampling), объёмной схемой и сценарием pgbench «плохих» запросов. Init-скрипты запечены в образ (как у
# pg-lab), чтобы контейнер не зависел от пути рабочей копии. Контекст сборки — каталог dev.
FROM postgres:17
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      postgresql-17-hypopg postgresql-17-pg-stat-kcache postgresql-17-pg-wait-sampling \
 && rm -rf /var/lib/apt/lists/*
COPY perf/20-init.sql /docker-entrypoint-initdb.d/20-init.sql
COPY perf/heavy.sql /perf/heavy.sql
```

`dev/perf/20-init.sql`:

```sql
-- Стенд P3: роли, база perf с расширениями и объёмная схема shop (спец. P3 §13.1). Таблицы audit и balances
-- нарочно без ANALYZE и автовакуума: ошибка оценки и heap fetches для правил рекомендаций.
CREATE ROLE glafira_diag LOGIN PASSWORD 'glafira-secret';
GRANT pg_monitor, pg_signal_backend, pg_read_all_data TO glafira_diag;
CREATE ROLE glafira_app LOGIN PASSWORD 'app-secret';

CREATE DATABASE perf;
\connect perf

CREATE EXTENSION pg_stat_statements;
CREATE EXTENSION pg_stat_kcache;
CREATE EXTENSION pg_wait_sampling;
CREATE EXTENSION hypopg;

CREATE SCHEMA shop;
CREATE TABLE shop.customers (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  email text NOT NULL,
  region text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE shop.orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  customer_id bigint NOT NULL,
  status text NOT NULL,
  total numeric(12, 2) NOT NULL,
  created_at timestamptz NOT NULL
);
CREATE INDEX orders_customer_idx ON shop.orders (customer_id);
CREATE TABLE shop.order_items (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  order_id bigint NOT NULL,
  product_id int NOT NULL,
  qty int NOT NULL,
  price numeric(12, 2) NOT NULL
);
CREATE INDEX order_items_order_idx ON shop.order_items (order_id);
CREATE INDEX order_items_product_idx ON shop.order_items (product_id);
CREATE TABLE shop.events (id bigint NOT NULL, happened_at timestamptz NOT NULL, kind text NOT NULL, payload jsonb)
  PARTITION BY RANGE (happened_at);
DO $$
DECLARE m date := date '2025-01-01';
BEGIN
  FOR i IN 0..23 LOOP
    EXECUTE format('CREATE TABLE shop.events_%s PARTITION OF shop.events FOR VALUES FROM (%L) TO (%L)',
                   to_char(m, 'YYYY_MM'), m, m + interval '1 month');
    m := m + interval '1 month';
  END LOOP;
END $$;
-- Скошенные данные без статистики: 99 % строк относятся к заказу 1, планировщик оценивает соединение неверно.
CREATE TABLE shop.audit (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  order_id bigint NOT NULL,
  actor text NOT NULL,
  note text
) WITH (autovacuum_enabled = off);
CREATE INDEX audit_order_idx ON shop.audit (order_id);
-- Массовое обновление без VACUUM: карта видимости грязная, Index Only Scan ходит в heap.
CREATE TABLE shop.balances (customer_id bigint PRIMARY KEY, balance numeric(12, 2) NOT NULL)
  WITH (autovacuum_enabled = off);

INSERT INTO shop.customers (email, region)
  SELECT 'user' || g || '@example.com', (ARRAY['eu', 'us', 'apac', 'latam'])[1 + g % 4] FROM generate_series(1, 200000) g;
INSERT INTO shop.orders (customer_id, status, total, created_at)
  SELECT 1 + (random() * 199999)::int,
         CASE WHEN g % 1000 = 0 THEN 'disputed' WHEN g % 10 = 0 THEN 'new' ELSE 'done' END,
         (random() * 1000)::numeric(12, 2),
         timestamptz '2025-01-01' + (random() * 700) * interval '1 day'
  FROM generate_series(1, 500000) g;
INSERT INTO shop.order_items (order_id, product_id, qty, price)
  SELECT 1 + (random() * 499999)::int, 1 + (random() * 4999)::int, 1 + (random() * 5)::int, (random() * 200)::numeric(12, 2)
  FROM generate_series(1, 1500000) g;
INSERT INTO shop.events (id, happened_at, kind, payload)
  SELECT g, timestamptz '2025-01-01' + (random() * 729) * interval '1 day', (ARRAY['view', 'cart', 'pay'])[1 + g % 3],
         jsonb_build_object('k', g % 100)
  FROM generate_series(1, 1000000) g;
INSERT INTO shop.audit (order_id, actor, note)
  SELECT CASE WHEN g % 100 = 0 THEN 1 + (random() * 499999)::int ELSE 1 END, 'app', repeat('x', 20)
  FROM generate_series(1, 200000) g;
INSERT INTO shop.balances SELECT id, 0 FROM shop.customers;
UPDATE shop.balances SET balance = balance + 1;
ANALYZE shop.customers, shop.orders, shop.order_items, shop.events;

GRANT USAGE ON SCHEMA shop TO glafira_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA shop TO glafira_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA shop TO glafira_app;
```

`dev/perf/heavy.sql` (сценарий `pgbench`, по запросу на каждое правило §9.1 и эвристику §6 спеки):

```sql
-- Нагрузка pg-perf: «плохие» запросы для топа, эвристик и правил рекомендаций (спец. P3 §13.1).
-- Запуск: docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300
\set cid random(1, 200000)
\set oid random(1, 500000)
\set pid random(1, 4990)
\set day random(0, 729)
-- N+1: точечный выбор покупателя — огромные calls при крошечном времени
SELECT * FROM shop.customers WHERE id = :cid;
SELECT * FROM shop.customers WHERE id = :cid + 1;
SELECT * FROM shop.customers WHERE id = :cid + 2;
-- Seq Scan с большим отсевом: status без индекса
SELECT count(*) FROM shop.orders WHERE status = 'disputed';
-- Nested Loop по неверной оценке: audit без статистики, 99 % строк у заказа 1
SELECT o.id, a.note FROM shop.audit a JOIN shop.orders o ON o.id = a.order_id WHERE a.actor = 'app' AND a.order_id = 1;
-- Sort под Limit без индекса и сортировка на диске (work_mem = 1MB)
SELECT * FROM shop.orders ORDER BY total DESC LIMIT 10;
SELECT customer_id, sum(total) FROM shop.orders GROUP BY customer_id ORDER BY 2 DESC;
-- Index Only Scan с heap fetches
SELECT count(*) FROM shop.balances WHERE customer_id BETWEEN :cid AND :cid + 1000;
-- SubPlan на строку: коррелированный подзапрос на ~3000 строк
SELECT oi.id FROM shop.order_items oi
 WHERE oi.product_id BETWEEN :pid AND :pid + 10
   AND oi.price > (SELECT avg(x.price) FROM shop.order_items x WHERE x.order_id = oi.order_id);
-- Append без pruning: фильтр не по ключу партиционирования
SELECT count(*) FROM shop.events WHERE kind = 'pay' AND payload ->> 'k' = '7';
-- Параллельная агрегация (воркеров меньше плана) и JIT
SELECT customer_id % 1000, sum(total * 1.07 + 3), avg(total) FROM shop.orders GROUP BY 1;
-- Temp-файлы и чтения с диска: хеш-соединение больших таблиц
SELECT count(*) FROM shop.order_items oi JOIN shop.orders o ON o.id = oi.order_id
 WHERE o.created_at >= timestamptz '2025-01-01' + :day * interval '1 day' - interval '30 days';
```

`dev/perf/README.md`:

```markdown
# pg-perf — стенд тяжёлых запросов (P3)

PostgreSQL 17 (`gp-pg-perf`, `127.0.0.1:5471`) с `pg_stat_statements` (`track_planning = on`), `pg_stat_kcache`,
`pg_wait_sampling` и `hypopg`; база `perf`, схема `shop` (~3 млн строк), роли `glafira_diag` (`pg_monitor`,
`pg_read_all_data`) и `glafira_app` (пароли — как у остального стенда). Параметры сервера нарочно тесные:
`work_mem = 1MB`, `max_parallel_workers = 2`, низкие пороги JIT.

Поднять на уже работающем стенде (остальные контейнеры не трогаются):

    docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf

Нагрузка — сценарий `pgbench` с запросами под каждую эвристику топа и правило рекомендаций:

    docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300

Сбросить статистику: `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT pg_stat_statements_reset()"`.
Интеграционные тесты: `GPG_IT_PERF=localhost:5471`.
```

- [ ] **Step 4: compose, wait-stand, CI, README, common**

`dev/docker-compose.yml` — сервис после `pg-lab` (перед `bastion`):

```yaml
  # Стенд P3: тяжёлые запросы. PG 17 с pg_stat_statements (track_planning), hypopg, pg_stat_kcache и pg_wait_sampling,
  # объёмная схема perf.shop и сценарий pgbench /perf/heavy.sql (dev/perf/README.md). Параметры нарочно тесные:
  # work_mem=1MB — сортировки на диске, max_parallel_workers=2 при per_gather=4 — нехватка воркеров, низкие пороги
  # JIT — JIT на простых запросах. Init-скрипты запечены в образ (perf/Dockerfile).
  pg-perf:
    build:
      context: .
      dockerfile: perf/Dockerfile
    image: glafira-pg-dev/perf:17
    container_name: gp-pg-perf
    hostname: pg-perf
    environment:
      POSTGRES_PASSWORD: postgres-secret
    command:
      - postgres
      - -c
      - shared_preload_libraries=pg_stat_statements,pg_stat_kcache,pg_wait_sampling
      - -c
      - pg_stat_statements.track_planning=on
      - -c
      - pg_stat_statements.max=5000
      - -c
      - track_io_timing=on
      - -c
      - compute_query_id=on
      - -c
      - shared_buffers=128MB
      - -c
      - work_mem=1MB
      - -c
      - max_parallel_workers=2
      - -c
      - max_parallel_workers_per_gather=4
      - -c
      - jit_above_cost=1000
      - -c
      - jit_inline_above_cost=1000
      - -c
      - jit_optimize_above_cost=1000
    ports: ["127.0.0.1:5471:5432"]
```

`scripts/wait-stand.sh` — в список контейнеров `for c in …` добавить `gp-pg-perf` после `gp-pg-lab`; в конец файла:

```bash
wait_for docker exec gp-pg-perf psql -U postgres -d perf -tAc "select count(*) from pg_extension where extname = 'hypopg'"
```

`.github/workflows/ci.yml` — в `env` шага «Тесты Rust» после `GPG_IT_LAB` строка `GPG_IT_PERF: localhost:5471`.

`README.md` — в таблицу стенда строка `| Стенд тяжёлых запросов P3 (\`pg-perf\`, PG 17, hypopg, kcache, wait_sampling) | \`localhost:5471\` |` и после фразы о `pg-lab` (строка 69): «Так же поднимается `pg-perf`: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf` (нагрузка — `dev/perf/README.md`).»

`src-tauri/tests/common/mod.rs` — после `lab_addr`:

```rust
/// Стенд тяжёлых запросов P3 (`GPG_IT_PERF=localhost:5471`, PG 17 с hypopg, pg_stat_kcache, pg_wait_sampling).
pub fn perf_addr() -> Option<String> {
    env("GPG_IT_PERF")
}
```

- [ ] **Step 5: Поднять сервис и прогнать тест**

Run: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf && scripts/wait-stand.sh`
Expected: образ собирается (пакеты PGDG ставятся), контейнер `gp-pg-perf` поднят, init занимает 1–2 минуты, `wait-stand.sh` завершается без ошибок; остальные контейнеры не пересозданы (`docker ps` — те же `CREATED`).

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_stand perf_has -- --test-threads=1`
Expected: PASS.

Run (прогрев для следующих задач, в фоне, 5 минут): `docker exec -d gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300`

- [ ] **Step 6: Commit**

```bash
git add dev/perf dev/docker-compose.yml scripts/wait-stand.sh .github/workflows/ci.yml README.md src-tauri/tests/common/mod.rs src-tauri/tests/it_stand.rs
git commit -m "feat(stand): сервис pg-perf — PG 17 с hypopg, pg_stat_kcache, pg_wait_sampling, объёмная схема и сценарий pgbench

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: `pg-advisor` — версии pgss, модель топа, цель и ссылка находок, пороги

**Files:**
- Create: `crates/pg-advisor/src/pgss.rs`, `crates/pg-advisor/src/queries/mod.rs`, `crates/pg-advisor/src/queries/rules.rs` (пустой модуль с заголовком — наполняется в задаче 3)
- Modify: `crates/pg-advisor/src/lib.rs` (`pub mod pgss; pub mod queries;`), `crates/pg-advisor/src/findings.rs`
- Test: unit-тесты в тех же файлах

**Interfaces:**
- Produces: `pgss::PgssVersion { major, minor }` (`parse`, константы `V1_8…V1_12`, `Ord`); `pgss::PgssCaps { version, toplevel, info, jit, temp_io_time, shared_io_names, stats_since, parallel }` и `PgssCaps::from_version(&str) -> Option<PgssCaps>` (`None` — старше 1.8 или не разобралась).
  `queries::QueryKey { query_id: i64 (JSON — строка), dbid: u32, userid: u32, toplevel: bool }`; `queries::Counters` (аддитивные счётчики, `changed_since`, `decreased_from`, `delta`, `add`); `queries::Cumulative { key, datname, rolname, counters, min_exec_ms, max_exec_ms, stddev_exec_ms }`; `queries::Totals { calls, total_exec_ms, rows }` и `queries::totals(iter)`; `queries::QueryStat` (строка отчёта, спец. §4.4) и `queries::stat(key, datname, rolname, &Counters, minmax: Option<(f64, f64, f64)>, &Totals) -> QueryStat`; `queries::QuerySort` (`ascending`, `value`, `available(&PgssCaps, window)`), `queries::top(&mut Vec<QueryStat>, QuerySort, limit)`.
  `findings::FindingTarget::Query { query_id: String, database: String }`, `findings::FindingLink::OpenQuery { query_id: String, database: String, member: Option<String> }`, `Finding::link_query(query_id, database, member)`; пороги `thresholds::QUERY_*`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/pgss.rs` — только тесты (реализация в шаге 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(PgssVersion::parse("1.10"), Some(PgssVersion { major: 1, minor: 10 }));
        assert_eq!(PgssVersion::parse(" 1.8 "), Some(PgssVersion::V1_8));
        assert_eq!(PgssVersion::parse("1"), None);
        assert_eq!(PgssVersion::parse("x.y"), None);
        assert!(PgssVersion::V1_9 < PgssVersion::V1_10, "1.10 новее 1.9 — сравнение не строковое");
    }

    #[test]
    fn caps_follow_the_extension_version() {
        assert!(PgssCaps::from_version("1.7").is_none(), "старше 1.8 — нет *_exec_time");
        let c8 = PgssCaps::from_version("1.8").unwrap();
        assert!(!c8.toplevel && !c8.info && !c8.jit && !c8.temp_io_time && !c8.shared_io_names && !c8.parallel);
        let c9 = PgssCaps::from_version("1.9").unwrap();
        assert!(c9.toplevel && c9.info && !c9.jit);
        let c10 = PgssCaps::from_version("1.10").unwrap();
        assert!(c10.jit && c10.temp_io_time && !c10.shared_io_names);
        let c11 = PgssCaps::from_version("1.11").unwrap();
        assert!(c11.shared_io_names && c11.stats_since && !c11.parallel);
        let c12 = PgssCaps::from_version("1.12").unwrap();
        assert!(c12.parallel && c12.jit && c12.toplevel);
    }
}
```

`crates/pg-advisor/src/queries/mod.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgss::PgssCaps;

    fn key(id: i64) -> QueryKey {
        QueryKey { query_id: id, dbid: 5, userid: 10, toplevel: true }
    }

    fn counters(calls: i64, total_ms: f64, shared_hit: i64, shared_read: i64, temp_written: i64) -> Counters {
        Counters { calls, total_exec_ms: total_ms, rows: calls * 2, shared_hit, shared_read, temp_written, ..Counters::default() }
    }

    #[test]
    fn query_key_serializes_the_id_as_a_string() {
        let v = serde_json::to_value(key(-9_223_372_036_854_775_000)).unwrap();
        assert_eq!(v["queryId"], "-9223372036854775000");
        assert_eq!((v["dbid"].as_u64(), v["toplevel"].as_bool()), (Some(5), Some(true)));
        let back: QueryKey = serde_json::from_value(v).unwrap();
        assert_eq!(back, key(-9_223_372_036_854_775_000));
    }

    #[test]
    fn counters_delta_add_and_decrease() {
        let a = counters(10, 100.0, 50, 5, 0);
        let b = counters(15, 160.0, 70, 9, 3);
        let d = b.delta(&a);
        assert_eq!((d.calls, d.total_exec_ms, d.shared_hit, d.shared_read, d.temp_written, d.rows), (5, 60.0, 20, 4, 3, 10));
        assert!(b.changed_since(&a) && !a.changed_since(&a));
        assert!(a.decreased_from(&b) && !b.decreased_from(&a));
        let mut s = a;
        s.add(&d);
        assert_eq!(s, b);
        // Option-колонки: None − Some остаётся None, Some + None — Some.
        let with_jit = Counters { jit_ms: Some(4.0), ..Counters::default() };
        assert_eq!(with_jit.delta(&Counters::default()).jit_ms, Some(4.0));
        assert_eq!(Counters::default().delta(&with_jit).jit_ms, None);
        let mut acc = Counters::default();
        acc.add(&with_jit);
        assert_eq!(acc.jit_ms, Some(4.0));
    }

    #[test]
    fn stat_computes_shares_ratios_and_means() {
        let t = Totals { calls: 100, total_exec_ms: 1000.0, rows: 0 };
        let s = stat(key(1), Some("perf".into()), Some("app".into()), &counters(10, 250.0, 90, 10, 0), Some((1.0, 50.0, 7.5)), &t);
        assert_eq!((s.mean_exec_ms, s.share_pct, s.rows_per_call), (25.0, 25.0, 2.0));
        assert_eq!(s.hit_ratio, Some(0.9));
        assert_eq!((s.min_exec_ms, s.max_exec_ms, s.stddev_exec_ms), (Some(1.0), Some(50.0), Some(7.5)));
        assert_eq!((s.text, s.text_truncated), (None, false));
        // Без чтений hit ratio не определён; без вызовов среднее — 0; без итогов доля — 0.
        let z = stat(key(2), None, None, &Counters::default(), None, &Totals::default());
        assert_eq!((z.hit_ratio, z.mean_exec_ms, z.share_pct, z.rows_per_call, z.min_exec_ms), (None, 0.0, 0.0, 0.0, None));
    }

    #[test]
    fn totals_sum_the_counters() {
        let t = totals([&counters(1, 10.0, 0, 0, 0), &counters(2, 5.0, 0, 0, 0)]);
        assert_eq!((t.calls, t.total_exec_ms, t.rows), (3, 15.0, 6));
    }

    #[test]
    fn top_sorts_descending_puts_missing_values_last_and_truncates() {
        let t = Totals { calls: 10, total_exec_ms: 100.0, rows: 0 };
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 10.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 50.0, 0, 0, 0), None, &t),
            stat(key(3), None, None, &counters(1, 40.0, 0, 0, 0), None, &t),
        ];
        top(&mut rows, QuerySort::TotalTime, 2);
        assert_eq!(rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [2, 3]);
        // Hit ratio — по возрастанию, строки без чтений (None) — в конце.
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 1.0, 50, 50, 0), None, &t),
            stat(key(3), None, None, &counters(1, 1.0, 99, 1, 0), None, &t),
        ];
        top(&mut rows, QuerySort::HitRatio, 10);
        assert_eq!(rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [2, 3, 1]);
        // В режиме окна stddev недоступен → строки без значения остаются в исходном порядке.
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
        ];
        top(&mut rows, QuerySort::StddevTime, 10);
        assert_eq!(rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn sort_availability_depends_on_caps_and_mode() {
        let c8 = PgssCaps::from_version("1.8").unwrap();
        let c10 = PgssCaps::from_version("1.10").unwrap();
        assert!(QuerySort::TotalTime.available(&c8, true));
        assert!(!QuerySort::StddevTime.available(&c10, true) && QuerySort::StddevTime.available(&c10, false));
        assert!(!QuerySort::MaxTime.available(&c10, true));
        assert!(!QuerySort::JitTime.available(&c8, false) && QuerySort::JitTime.available(&c10, false));
        assert!(QuerySort::HitRatio.ascending() && !QuerySort::Calls.ascending());
        assert_eq!(serde_json::to_value(QuerySort::RowsPerCall).unwrap(), "rowsPerCall");
    }
}
```

`crates/pg-advisor/src/findings.rs` — в `mod tests` добавить:

```rust
    #[test]
    fn query_target_and_link_serialize_with_string_ids() {
        let f = Finding::new("query_temp", Severity::Warning)
            .target(FindingTarget::Query { query_id: "123".into(), database: "perf".into() })
            .link_query("123", "perf", Some("pg-1"));
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["target"], json!({"kind": "query", "queryId": "123", "database": "perf"}));
        assert_eq!(v["links"][0], json!({"kind": "openQuery", "queryId": "123", "database": "perf", "member": "pg-1"}));
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml`
Expected: ошибки компиляции (`pgss`, `queries`, `FindingTarget::Query`, `link_query` нет).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/lib.rs` — добавить `pub mod pgss;` и `pub mod queries;` (по алфавиту).

`crates/pg-advisor/src/pgss.rs` (перед тестами шага 1):

```rust
//! Возможности установленной версии `pg_stat_statements` (спец. P3 §3.3): набор колонок определяется
//! `pg_extension.extversion`, а не версией сервера — после `pg_upgrade` без `ALTER EXTENSION … UPDATE` сервер 17
//! может работать с pgss 1.10.

/// Версия расширения `major.minor`; сравнение числовое (1.10 новее 1.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PgssVersion {
    pub major: u32,
    pub minor: u32,
}

impl PgssVersion {
    pub const V1_8: Self = Self { major: 1, minor: 8 };
    pub const V1_9: Self = Self { major: 1, minor: 9 };
    pub const V1_10: Self = Self { major: 1, minor: 10 };
    pub const V1_11: Self = Self { major: 1, minor: 11 };
    pub const V1_12: Self = Self { major: 1, minor: 12 };

    pub fn parse(s: &str) -> Option<Self> {
        let (major, minor) = s.trim().split_once('.')?;
        Some(Self {
            major: major.parse().ok()?,
            minor: minor.parse().ok()?,
        })
    }
}

/// Какие колонки есть у установленной версии (спец. §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PgssCaps {
    pub version: PgssVersion,
    /// 1.9: колонка `toplevel`.
    pub toplevel: bool,
    /// 1.9: представление `pg_stat_statements_info` (`dealloc`, `stats_reset`).
    pub info: bool,
    /// 1.10: `jit_*`.
    pub jit: bool,
    /// 1.10: `temp_blk_read_time`, `temp_blk_write_time`.
    pub temp_io_time: bool,
    /// 1.11: `shared_blk_read_time` / `shared_blk_write_time` вместо `blk_read_time` / `blk_write_time`; `local_blk_*_time`;
    /// `jit_deform_time`.
    pub shared_io_names: bool,
    /// 1.11: `stats_since`, `minmax_stats_since`.
    pub stats_since: bool,
    /// 1.12: `parallel_workers_to_launch`, `parallel_workers_launched`, `generic_plan_calls`, `custom_plan_calls`.
    pub parallel: bool,
}

impl PgssCaps {
    /// `None` — версия старше 1.8 (нет `*_exec_time`, планирования и WAL) или строка не разобралась.
    pub fn from_version(s: &str) -> Option<Self> {
        let v = PgssVersion::parse(s)?;
        if v < PgssVersion::V1_8 {
            return None;
        }
        Some(Self {
            version: v,
            toplevel: v >= PgssVersion::V1_9,
            info: v >= PgssVersion::V1_9,
            jit: v >= PgssVersion::V1_10,
            temp_io_time: v >= PgssVersion::V1_10,
            shared_io_names: v >= PgssVersion::V1_11,
            stats_since: v >= PgssVersion::V1_11,
            parallel: v >= PgssVersion::V1_12,
        })
    }
}
```

`crates/pg-advisor/src/queries/mod.rs` (перед тестами):

```rust
//! Топ запросов (спец. P3 §4): ключ строки `pg_stat_statements`, аддитивные счётчики, строка отчёта, итоги и
//! сортировки. Эвристики без плана — `rules`.

pub mod rules;

use serde::{Deserialize, Serialize};

use crate::pgss::PgssCaps;

/// Ключ строки pgss. `query_id` в JSON — десятичная строка: 64-битный хеш не помещается в число JS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryKey {
    #[serde(with = "crate::serde_util::i64_string")]
    pub query_id: i64,
    pub dbid: u32,
    pub userid: u32,
    pub toplevel: bool,
}

/// Аддитивные счётчики строки pgss: накопительные с момента сброса или дельта за тик / окно. `Option` — колонки,
/// которых нет у версии расширения (`PgssCaps`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub calls: i64,
    pub total_exec_ms: f64,
    pub plans: i64,
    pub total_plan_ms: f64,
    pub rows: i64,
    pub shared_hit: i64,
    pub shared_read: i64,
    pub shared_dirtied: i64,
    pub shared_written: i64,
    pub local_hit: i64,
    pub local_read: i64,
    pub local_dirtied: i64,
    pub local_written: i64,
    pub temp_read: i64,
    pub temp_written: i64,
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    pub temp_io_ms: Option<f64>,
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: i64,
    pub jit_functions: Option<i64>,
    pub jit_ms: Option<f64>,
    pub parallel_planned: Option<i64>,
    pub parallel_launched: Option<i64>,
}

fn sub_opt<T: std::ops::Sub<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a - b),
        (a, None) => a,
        (None, Some(_)) => None,
    }
}

fn add_opt<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, None) => a,
        (None, b) => b,
    }
}

impl Counters {
    /// Строка изменилась с прошлого тика: были вызовы или время.
    pub fn changed_since(&self, prev: &Counters) -> bool {
        self.calls != prev.calls || self.total_exec_ms != prev.total_exec_ms
    }

    /// Счётчики уменьшились: строка вытеснена и вернулась или статистика сброшена — нужна новая база.
    pub fn decreased_from(&self, prev: &Counters) -> bool {
        self.calls < prev.calls || self.total_exec_ms < prev.total_exec_ms
    }

    /// `self − prev` по всем полям.
    pub fn delta(&self, p: &Counters) -> Counters {
        Counters {
            calls: self.calls - p.calls,
            total_exec_ms: self.total_exec_ms - p.total_exec_ms,
            plans: self.plans - p.plans,
            total_plan_ms: self.total_plan_ms - p.total_plan_ms,
            rows: self.rows - p.rows,
            shared_hit: self.shared_hit - p.shared_hit,
            shared_read: self.shared_read - p.shared_read,
            shared_dirtied: self.shared_dirtied - p.shared_dirtied,
            shared_written: self.shared_written - p.shared_written,
            local_hit: self.local_hit - p.local_hit,
            local_read: self.local_read - p.local_read,
            local_dirtied: self.local_dirtied - p.local_dirtied,
            local_written: self.local_written - p.local_written,
            temp_read: self.temp_read - p.temp_read,
            temp_written: self.temp_written - p.temp_written,
            io_read_ms: self.io_read_ms - p.io_read_ms,
            io_write_ms: self.io_write_ms - p.io_write_ms,
            temp_io_ms: sub_opt(self.temp_io_ms, p.temp_io_ms),
            wal_records: self.wal_records - p.wal_records,
            wal_fpi: self.wal_fpi - p.wal_fpi,
            wal_bytes: self.wal_bytes - p.wal_bytes,
            jit_functions: sub_opt(self.jit_functions, p.jit_functions),
            jit_ms: sub_opt(self.jit_ms, p.jit_ms),
            parallel_planned: sub_opt(self.parallel_planned, p.parallel_planned),
            parallel_launched: sub_opt(self.parallel_launched, p.parallel_launched),
        }
    }

    pub fn add(&mut self, o: &Counters) {
        self.calls += o.calls;
        self.total_exec_ms += o.total_exec_ms;
        self.plans += o.plans;
        self.total_plan_ms += o.total_plan_ms;
        self.rows += o.rows;
        self.shared_hit += o.shared_hit;
        self.shared_read += o.shared_read;
        self.shared_dirtied += o.shared_dirtied;
        self.shared_written += o.shared_written;
        self.local_hit += o.local_hit;
        self.local_read += o.local_read;
        self.local_dirtied += o.local_dirtied;
        self.local_written += o.local_written;
        self.temp_read += o.temp_read;
        self.temp_written += o.temp_written;
        self.io_read_ms += o.io_read_ms;
        self.io_write_ms += o.io_write_ms;
        self.temp_io_ms = add_opt(self.temp_io_ms, o.temp_io_ms);
        self.wal_records += o.wal_records;
        self.wal_fpi += o.wal_fpi;
        self.wal_bytes += o.wal_bytes;
        self.jit_functions = add_opt(self.jit_functions, o.jit_functions);
        self.jit_ms = add_opt(self.jit_ms, o.jit_ms);
        self.parallel_planned = add_opt(self.parallel_planned, o.parallel_planned);
        self.parallel_launched = add_opt(self.parallel_launched, o.parallel_launched);
    }
}

/// Строка pgss с накопительными значениями — как прочитана с сервера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cumulative {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub counters: Counters,
    pub min_exec_ms: f64,
    pub max_exec_ms: f64,
    pub stddev_exec_ms: f64,
}

/// Итоги набора — знаменатели долей.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub calls: i64,
    pub total_exec_ms: f64,
    pub rows: i64,
}

pub fn totals<'a>(rows: impl IntoIterator<Item = &'a Counters>) -> Totals {
    let mut t = Totals::default();
    for c in rows {
        t.calls += c.calls;
        t.total_exec_ms += c.total_exec_ms;
        t.rows += c.rows;
    }
    t
}

/// Строка отчёта (спец. §4.4). `min` / `max` / `stddev` есть только у накопительных данных: дельты их не дают.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStat {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub calls: i64,
    pub total_exec_ms: f64,
    pub mean_exec_ms: f64,
    pub min_exec_ms: Option<f64>,
    pub max_exec_ms: Option<f64>,
    pub stddev_exec_ms: Option<f64>,
    pub rows: i64,
    pub rows_per_call: f64,
    pub plans: i64,
    pub total_plan_ms: f64,
    pub shared_hit: i64,
    pub shared_read: i64,
    pub shared_dirtied: i64,
    pub shared_written: i64,
    pub local_hit: i64,
    pub local_read: i64,
    pub local_dirtied: i64,
    pub local_written: i64,
    pub temp_read: i64,
    pub temp_written: i64,
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    pub temp_io_ms: Option<f64>,
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: i64,
    pub jit_functions: Option<i64>,
    pub jit_ms: Option<f64>,
    pub parallel_planned: Option<i64>,
    pub parallel_launched: Option<i64>,
    /// Доля в `totals.total_exec_ms`, проценты.
    pub share_pct: f64,
    /// `hit / (hit + read)`; `None` — блоков не читали.
    pub hit_ratio: Option<f64>,
    /// Текст из кеша; `None` — ещё не запрашивался.
    pub text: Option<String>,
    pub text_truncated: bool,
}

/// Строка отчёта из счётчиков; `minmax` — `(min, max, stddev)` только у накопительных данных.
pub fn stat(
    key: QueryKey,
    datname: Option<String>,
    rolname: Option<String>,
    c: &Counters,
    minmax: Option<(f64, f64, f64)>,
    totals: &Totals,
) -> QueryStat {
    let per_call = |v: f64| if c.calls > 0 { v / c.calls as f64 } else { 0.0 };
    let blocks = c.shared_hit + c.shared_read;
    QueryStat {
        key,
        datname,
        rolname,
        calls: c.calls,
        total_exec_ms: c.total_exec_ms,
        mean_exec_ms: per_call(c.total_exec_ms),
        min_exec_ms: minmax.map(|m| m.0),
        max_exec_ms: minmax.map(|m| m.1),
        stddev_exec_ms: minmax.map(|m| m.2),
        rows: c.rows,
        rows_per_call: per_call(c.rows as f64),
        plans: c.plans,
        total_plan_ms: c.total_plan_ms,
        shared_hit: c.shared_hit,
        shared_read: c.shared_read,
        shared_dirtied: c.shared_dirtied,
        shared_written: c.shared_written,
        local_hit: c.local_hit,
        local_read: c.local_read,
        local_dirtied: c.local_dirtied,
        local_written: c.local_written,
        temp_read: c.temp_read,
        temp_written: c.temp_written,
        io_read_ms: c.io_read_ms,
        io_write_ms: c.io_write_ms,
        temp_io_ms: c.temp_io_ms,
        wal_records: c.wal_records,
        wal_fpi: c.wal_fpi,
        wal_bytes: c.wal_bytes,
        jit_functions: c.jit_functions,
        jit_ms: c.jit_ms,
        parallel_planned: c.parallel_planned,
        parallel_launched: c.parallel_launched,
        share_pct: if totals.total_exec_ms > 0.0 {
            c.total_exec_ms / totals.total_exec_ms * 100.0
        } else {
            0.0
        },
        hit_ratio: (blocks > 0).then(|| c.shared_hit as f64 / blocks as f64),
        text: None,
        text_truncated: false,
    }
}

/// Сортировки топа (спец. §4.3). `hitRatio` — по возрастанию (худшие сверху), остальные — по убыванию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuerySort {
    TotalTime,
    MeanTime,
    MaxTime,
    StddevTime,
    Calls,
    RowsPerCall,
    SharedRead,
    HitRatio,
    TempWritten,
    WalBytes,
    IoTime,
    PlanTime,
    JitTime,
}

impl QuerySort {
    pub fn ascending(self) -> bool {
        matches!(self, Self::HitRatio)
    }

    /// Ключ сортировки строки; `None` — значения нет (колонки нет в версии или режиме): такие строки в конце.
    pub fn value(self, s: &QueryStat) -> Option<f64> {
        match self {
            Self::TotalTime => Some(s.total_exec_ms),
            Self::MeanTime => Some(s.mean_exec_ms),
            Self::MaxTime => s.max_exec_ms,
            Self::StddevTime => s.stddev_exec_ms,
            Self::Calls => Some(s.calls as f64),
            Self::RowsPerCall => Some(s.rows_per_call),
            Self::SharedRead => Some(s.shared_read as f64),
            Self::HitRatio => s.hit_ratio,
            Self::TempWritten => Some(s.temp_written as f64),
            Self::WalBytes => Some(s.wal_bytes as f64),
            Self::IoTime => Some(s.io_read_ms + s.io_write_ms),
            Self::PlanTime => Some(s.total_plan_ms),
            Self::JitTime => s.jit_ms,
        }
    }

    /// Доступна ли сортировка: `min` / `max` / `stddev` нет в режиме окна, JIT — только с pgss 1.10.
    pub fn available(self, caps: &PgssCaps, window: bool) -> bool {
        match self {
            Self::MaxTime | Self::StddevTime => !window,
            Self::JitTime => caps.jit,
            _ => true,
        }
    }
}

/// Сортировка и обрезка до `limit`; строки без значения — в конце в исходном порядке (сортировка устойчива).
pub fn top(rows: &mut Vec<QueryStat>, sort: QuerySort, limit: usize) {
    use std::cmp::Ordering;
    rows.sort_by(|a, b| match (sort.value(a), sort.value(b)) {
        (Some(x), Some(y)) => {
            let o = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
            if sort.ascending() { o } else { o.reverse() }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    });
    rows.truncate(limit);
}
```

`crates/pg-advisor/src/queries/rules.rs` (заголовок; наполняется в задаче 3):

```rust
//! Эвристики топа без плана (спец. P3 §6) — задача 3 плана P3a.
```

`crates/pg-advisor/src/findings.rs`:
- в `FindingTarget` после `Database`:

```rust
    /// Запрос pgss: `query_id` десятичной строкой и база (личность запроса — `queryid` в пределах базы).
    #[serde(rename_all = "camelCase")]
    Query {
        query_id: String,
        database: String,
    },
```

- в `FindingLink` после `Action`:

```rust
    /// Карточка запроса (вкладка `query`); без узла интерфейс кнопки не даёт.
    #[serde(rename_all = "camelCase")]
    OpenQuery {
        query_id: String,
        database: String,
        member: Option<String>,
    },
```

- в `impl Finding` после `link_action`:

```rust
    pub fn link_query(mut self, query_id: &str, database: &str, member: Option<&str>) -> Self {
        self.links.push(FindingLink::OpenQuery {
            query_id: query_id.to_string(),
            database: database.to_string(),
            member: member.map(str::to_string),
        });
        self
    }
```

- в `pub mod thresholds` в конце:

```rust
    /// Топ запросов (спец. P3 §6): N+1 — доля вызовов, среднее (мс) и минимум вызовов.
    pub const QUERY_N1_CALLS_SHARE: f64 = 0.20;
    pub const QUERY_N1_MEAN_MS: f64 = 1.0;
    pub const QUERY_N1_MIN_CALLS: i64 = 1000;
    /// Нестабильный план: stddev во столько раз больше среднего; среднее не меньше (мс); минимум вызовов.
    pub const QUERY_UNSTABLE_STDDEV_RATIO: f64 = 2.0;
    pub const QUERY_UNSTABLE_MIN_MEAN_MS: f64 = 1.0;
    pub const QUERY_UNSTABLE_MIN_CALLS: i64 = 50;
    /// Низкий hit ratio: прочитано не меньше блоков и доля попаданий ниже.
    pub const QUERY_HIT_MIN_READ_BLOCKS: i64 = 10_000;
    pub const QUERY_HIT_RATIO_LOW: f64 = 0.90;
    /// Temp-файлы: записано блоков на вызов (128 блоков — 1 МБ); минимум вызовов.
    pub const QUERY_TEMP_BLOCKS_PER_CALL: i64 = 128;
    pub const QUERY_TEMP_MIN_CALLS: i64 = 10;
    /// Планирование дольше выполнения: минимум планирований.
    pub const QUERY_PLANNING_MIN_PLANS: i64 = 100;
    /// Блок PostgreSQL, байты.
    pub const BLOCK_BYTES: i64 = 8192;
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`
Expected: все тесты PASS, clippy и fmt чистые.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor
git commit -m "feat(pg-advisor): версии pg_stat_statements, модель топа запросов, цель и ссылка находок Query

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: `pg-advisor` — эвристики топа без плана

**Files:**
- Modify: `crates/pg-advisor/src/queries/rules.rs`
- Test: unit-тесты в том же файле

**Interfaces:**
- Consumes: `queries::{QueryStat, Totals}`, `findings::{Finding, FindingTarget, Severity, thresholds}`.
- Produces: `rules::Mode { SinceReset, Window }`; `rules::PgssState { track_planning: bool, dealloc: Option<i64>, dealloc_grew: bool, max: i64 }`; `rules::evaluate(member: Option<&str>, rows: &[QueryStat], totals: &Totals, pgss: &PgssState, mode: Mode) -> Vec<Finding>`. Параметры находок: `callsSharePct`, `meanMs`, `calls`, `stddevMs`, `hitRatioPct`, `readBytes`, `tempBytes`, `planMs`, `execMs`, `dealloc`, `max`.

- [ ] **Step 1: Падающие тесты**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{Counters, QueryKey, stat};

    fn key(id: i64) -> QueryKey {
        QueryKey { query_id: id, dbid: 1, userid: 1, toplevel: true }
    }

    fn row(id: i64, c: Counters, minmax: Option<(f64, f64, f64)>, totals: &Totals) -> QueryStat {
        stat(key(id), Some("perf".into()), Some("app".into()), &c, minmax, totals)
    }

    fn pgss() -> PgssState {
        PgssState { track_planning: true, dealloc: Some(0), dealloc_grew: false, max: 5000 }
    }

    fn ids(f: &[Finding]) -> Vec<&str> {
        f.iter().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn n_plus_one_needs_share_tiny_mean_and_volume() {
        let totals = Totals { calls: 10_000, total_exec_ms: 10_000.0, rows: 0 };
        let hot = Counters { calls: 3000, total_exec_ms: 1500.0, ..Counters::default() }; // 30 %, 0,5 мс
        let cold = Counters { calls: 3000, total_exec_ms: 9000.0, ..Counters::default() }; // 3 мс
        let few = Counters { calls: 900, total_exec_ms: 10.0, ..Counters::default() }; // 9 %
        let f = evaluate(Some("pg-1"), &[row(1, hot, None, &totals), row(2, cold, None, &totals), row(3, few, None, &totals)], &totals, &pgss(), Mode::Window);
        assert_eq!(ids(&f), ["query_n_plus_one"]);
        let n1 = &f[0];
        assert_eq!(n1.severity, Severity::Warning);
        assert_eq!(n1.target, Some(FindingTarget::Query { query_id: "1".into(), database: "perf".into() }));
        assert_eq!(n1.params["callsSharePct"], 30.0);
        assert_eq!(n1.params["calls"], 3000);
        assert!(matches!(&n1.links[0], crate::findings::FindingLink::OpenQuery { query_id, member: Some(m), .. } if query_id == "1" && m == "pg-1"));
    }

    #[test]
    fn unstable_plan_only_with_cumulative_stddev() {
        let totals = Totals { calls: 100, total_exec_ms: 1000.0, rows: 0 };
        let c = Counters { calls: 100, total_exec_ms: 1000.0, ..Counters::default() }; // mean 10
        let f = evaluate(None, &[row(1, c, Some((1.0, 500.0, 25.0)), &totals)], &totals, &pgss(), Mode::SinceReset);
        assert_eq!(ids(&f), ["query_unstable"]);
        assert_eq!((f[0].params["stddevMs"].as_f64(), f[0].params["meanMs"].as_f64()), (Some(25.0), Some(10.0)));
        assert!(evaluate(None, &[row(1, c, Some((1.0, 500.0, 19.0)), &totals)], &totals, &pgss(), Mode::SinceReset).is_empty());
        // За окно stddev нет: правило молчит, даже если бы minmax передали.
        assert!(evaluate(None, &[row(1, c, Some((1.0, 500.0, 25.0)), &totals)], &totals, &pgss(), Mode::Window).is_empty());
    }

    #[test]
    fn low_hit_ratio_and_temp_files() {
        let totals = Totals { calls: 100, total_exec_ms: 100.0, rows: 0 };
        let cold = Counters { calls: 100, total_exec_ms: 100.0, shared_hit: 10_000, shared_read: 40_000, ..Counters::default() }; // 20 %
        let small = Counters { calls: 100, total_exec_ms: 100.0, shared_hit: 10, shared_read: 9_000, ..Counters::default() }; // мало чтений
        let temp = Counters { calls: 10, total_exec_ms: 100.0, temp_written: 1_280, ..Counters::default() }; // 128 блоков на вызов
        let temp_few = Counters { calls: 9, total_exec_ms: 100.0, temp_written: 9_000, ..Counters::default() };
        let f = evaluate(None, &[row(1, cold, None, &totals), row(2, small, None, &totals), row(3, temp, None, &totals), row(4, temp_few, None, &totals)], &totals, &pgss(), Mode::Window);
        assert_eq!(ids(&f), ["query_low_hit_ratio", "query_temp"]);
        assert_eq!(f[0].params["hitRatioPct"], 20.0);
        assert_eq!(f[0].params["readBytes"], 40_000 * 8192);
        assert_eq!(f[1].params["tempBytes"], 128 * 8192);
    }

    #[test]
    fn planning_heavier_than_execution_needs_track_planning() {
        // 999 вызовов: при 1000 сработало бы и правило N+1 (доля 100 %, среднее 0,1 мс), а тест проверяет только планирование.
        let totals = Totals { calls: 999, total_exec_ms: 100.0, rows: 0 };
        let c = Counters { calls: 999, total_exec_ms: 100.0, plans: 999, total_plan_ms: 150.0, ..Counters::default() };
        let f = evaluate(None, &[row(1, c, None, &totals)], &totals, &pgss(), Mode::SinceReset);
        assert_eq!(ids(&f), ["query_planning_heavy"]);
        assert_eq!(f[0].severity, Severity::Info);
        let off = PgssState { track_planning: false, ..pgss() };
        assert!(evaluate(None, &[row(1, c, None, &totals)], &totals, &off, Mode::SinceReset).is_empty());
    }

    #[test]
    fn incomplete_top_depends_on_mode() {
        let totals = Totals::default();
        let since = PgssState { dealloc: Some(3), dealloc_grew: false, ..pgss() };
        let f = evaluate(Some("pg-1"), &[], &totals, &since, Mode::SinceReset);
        assert_eq!(ids(&f), ["queries_incomplete"]);
        assert_eq!(f[0].target, Some(FindingTarget::Member { name: "pg-1".into() }));
        assert_eq!((f[0].params["dealloc"].as_i64(), f[0].params["max"].as_i64()), (Some(3), Some(5000)));
        // За окно важен рост, а не накопленное значение.
        assert!(evaluate(None, &[], &totals, &since, Mode::Window).is_empty());
        let grew = PgssState { dealloc: Some(3), dealloc_grew: true, ..pgss() };
        assert_eq!(ids(&evaluate(None, &[], &totals, &grew, Mode::Window)), ["queries_incomplete"]);
        assert!(evaluate(None, &[], &totals, &PgssState { dealloc: None, ..pgss() }, Mode::SinceReset).is_empty());
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml queries::rules`
Expected: ошибки компиляции (`Mode`, `PgssState`, `evaluate` нет).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/queries/rules.rs` (над тестами):

```rust
//! Эвристики топа без плана (спец. P3 §6): считаются по строкам отчёта (уже обрезанным до предела) в выбранном
//! режиме. Критических находок нет — это подсказки «куда смотреть», а не аварии.

use crate::findings::{Finding, FindingTarget, Severity, thresholds as th};
use crate::queries::{QueryStat, Totals};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    SinceReset,
    Window,
}

/// Состояние pgss для правила «топ неполный» и для планирования.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PgssState {
    /// `pg_stat_statements.track_planning = on`: иначе колонки планирования нулевые и правило молчит.
    pub track_planning: bool,
    /// `pg_stat_statements_info.dealloc` накопительно (1.9+).
    pub dealloc: Option<i64>,
    /// `dealloc` вырос за окно отчёта.
    pub dealloc_grew: bool,
    /// `pg_stat_statements.max`.
    pub max: i64,
}

pub fn evaluate(
    member: Option<&str>,
    rows: &[QueryStat],
    totals: &Totals,
    pgss: &PgssState,
    mode: Mode,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in rows {
        let qid = r.key.query_id.to_string();
        let db = r.datname.clone().unwrap_or_default();
        let finding = |id: &str, severity: Severity| {
            Finding::new(id, severity)
                .member(member)
                .target(FindingTarget::Query {
                    query_id: qid.clone(),
                    database: db.clone(),
                })
                .link_query(&qid, &db, member)
        };
        if totals.calls > 0 {
            let share = r.calls as f64 / totals.calls as f64;
            if share >= th::QUERY_N1_CALLS_SHARE
                && r.mean_exec_ms <= th::QUERY_N1_MEAN_MS
                && r.calls >= th::QUERY_N1_MIN_CALLS
            {
                out.push(
                    finding("query_n_plus_one", Severity::Warning)
                        .param("callsSharePct", share * 100.0)
                        .param("meanMs", r.mean_exec_ms)
                        .param("calls", r.calls),
                );
            }
        }
        if mode == Mode::SinceReset
            && let Some(stddev) = r.stddev_exec_ms
            && r.mean_exec_ms >= th::QUERY_UNSTABLE_MIN_MEAN_MS
            && stddev >= th::QUERY_UNSTABLE_STDDEV_RATIO * r.mean_exec_ms
            && r.calls >= th::QUERY_UNSTABLE_MIN_CALLS
        {
            out.push(
                finding("query_unstable", Severity::Warning)
                    .param("stddevMs", stddev)
                    .param("meanMs", r.mean_exec_ms)
                    .param("calls", r.calls),
            );
        }
        if r.shared_read >= th::QUERY_HIT_MIN_READ_BLOCKS
            && let Some(hit) = r.hit_ratio
            && hit < th::QUERY_HIT_RATIO_LOW
        {
            out.push(
                finding("query_low_hit_ratio", Severity::Warning)
                    .param("hitRatioPct", hit * 100.0)
                    .param("readBytes", r.shared_read * th::BLOCK_BYTES),
            );
        }
        if r.calls >= th::QUERY_TEMP_MIN_CALLS
            && r.temp_written / r.calls >= th::QUERY_TEMP_BLOCKS_PER_CALL
        {
            out.push(
                finding("query_temp", Severity::Warning)
                    .param("tempBytes", r.temp_written / r.calls * th::BLOCK_BYTES)
                    .param("calls", r.calls),
            );
        }
        if pgss.track_planning
            && r.plans >= th::QUERY_PLANNING_MIN_PLANS
            && r.total_plan_ms > r.total_exec_ms
        {
            out.push(
                finding("query_planning_heavy", Severity::Info)
                    .param("planMs", r.total_plan_ms)
                    .param("execMs", r.total_exec_ms),
            );
        }
    }
    let incomplete = match mode {
        Mode::SinceReset => pgss.dealloc.is_some_and(|d| d > 0),
        Mode::Window => pgss.dealloc_grew,
    };
    if incomplete {
        let mut f = Finding::new("queries_incomplete", Severity::Warning)
            .member(member)
            .param("dealloc", pgss.dealloc.unwrap_or(0))
            .param("max", pgss.max);
        if let Some(m) = member {
            f = f.target(FindingTarget::Member { name: m.to_string() });
        }
        out.push(f);
    }
    out
}
```

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor/src/queries/rules.rs
git commit -m "feat(pg-advisor): эвристики топа запросов без плана — N+1, нестабильный план, hit ratio, temp, планирование, неполный топ

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Инструмент «Запросы» в Rust — поиск pgss, SQL по версии, режим «с момента сброса», тексты

**Files:**
- Create: `src-tauri/src/diag/queries.rs`, `src-tauri/src/commands/queries.rs`, `src-tauri/tests/it_queries.rs`
- Modify: `src-tauri/src/diag/mod.rs` (`pub mod queries;`, `Tool::Queries`, `DiagRequest.queries`, `DiagData::Queries`, `validate`, `run`), `src-tauri/src/commands/mod.rs` (`pub mod queries;`), `src-tauri/src/lib.rs` (команда `query_texts`), `src/locales/en.json`, `src/locales/ru.json`
- Test: unit-тесты в `diag/queries.rs`; `it_queries.rs` (со стендом)

**Interfaces:**
- Consumes: `pg_advisor::pgss::PgssCaps`, `pg_advisor::queries::{QueryKey, Counters, Cumulative, QueryStat, QuerySort, Totals, stat, totals, top}`, `pg_advisor::queries::rules::{Mode, PgssState, evaluate}`, `diag::{head, on_member, make_report, Head, Unavailable}`, `diag::sessions::QUERY_LIMIT`, `catalog::ddl::quote_ident`.
- Produces (`diag::queries`): `PgssInstalled { schema, version, caps }`; `Detect { Installed(PgssInstalled), Missing, TooOld(String) }`; `detect(&Client) -> AppResult<Detect>`; `statements_sql(ns, caps, where_clause) -> String` (35 колонок, порядок — в `row_from`); `info_sql(ns, caps)`; `texts_sql(ns, caps)`; `PgssInfo { stats_reset_ms, dealloc, max, track_planning }`; `Snapshot { rows: Vec<Cumulative>, info: PgssInfo, hidden: bool }`; `read_cumulative(&Client, &PgssInstalled) -> AppResult<Snapshot>`; `QueriesMode` (тег `kind`: `sinceReset` | `window { s }` | `between { from, to }`), `QueriesParams { mode, sort, limit, database, toplevel_only }`, `validate(&QueriesParams)`; `PgssMeta`, `QueriesData { mode, coverage_s, ticks, pgss: Option<PgssMeta>, totals, rows }` и `QueriesData::empty(mode)`; `stats_since_reset(&[Cumulative], &QueriesParams) -> (Vec<QueryStat>, Totals)`; `attach_texts(&mut [QueryStat], lookup)`; `report(st, id, target, &QueriesParams) -> AppResult<DiagReport>`; `QueryText { key, text, truncated }`, `texts(st, id, member, keys) -> AppResult<Vec<QueryText>>`; пометки `missing()`, `too_old(v)`, `hidden_rows()`, `window_starting()`.
  `diag::Tool::Queries`, `DiagRequest.queries: Option<QueriesParams>`, `DiagData::Queries(QueriesData)`. Команда `query_texts(id, member, keys) -> Vec<QueryText>`.

- [ ] **Step 1: Падающие тесты**

Unit-тесты `src-tauri/src/diag/queries.rs` (в конец файла):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::pgss::PgssCaps;

    fn caps(v: &str) -> PgssCaps {
        PgssCaps::from_version(v).unwrap()
    }

    fn cumulative(id: i64, toplevel: bool, db: &str, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: QueryKey { query_id: id, dbid: 1, userid: 1, toplevel },
            datname: Some(db.into()),
            rolname: Some("app".into()),
            counters: Counters { calls, total_exec_ms: ms, ..Counters::default() },
            min_exec_ms: 0.0,
            max_exec_ms: 0.0,
            stddev_exec_ms: 0.0,
        }
    }

    fn params(mode: QueriesMode) -> QueriesParams {
        QueriesParams { mode, sort: QuerySort::TotalTime, limit: 50, database: None, toplevel_only: true }
    }

    #[test]
    fn sql_follows_the_extension_version() {
        let old = statements_sql("public", caps("1.8"), "");
        assert!(old.contains("true, d.datname") && !old.contains("s.toplevel"), "{old}");
        assert!(old.contains("s.blk_read_time, s.blk_write_time, NULL::float8"), "{old}");
        assert!(old.contains("NULL::bigint, NULL::float8, NULL::bigint, NULL::bigint"), "{old}");
        assert!(old.contains("FROM public.pg_stat_statements(false) s"));
        let v10 = statements_sql("public", caps("1.10"), "");
        assert!(v10.contains("s.toplevel") && v10.contains("s.temp_blk_read_time + s.temp_blk_write_time"), "{v10}");
        assert!(v10.contains("s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time"), "{v10}");
        let v11 = statements_sql("\"my schema\"", caps("1.11"), " WHERE s.queryid = $1 AND d.datname = $2");
        assert!(v11.contains("s.shared_blk_read_time, s.shared_blk_write_time"), "{v11}");
        assert!(v11.contains("FROM \"my schema\".pg_stat_statements(false) s") && v11.ends_with("WHERE s.queryid = $1 AND d.datname = $2"));
        let v12 = statements_sql("public", caps("1.12"), "");
        assert!(v12.contains("s.parallel_workers_to_launch, s.parallel_workers_launched"), "{v12}");
        assert!(v12.contains("s.wal_bytes::bigint"), "numeric приводится к bigint");
        assert!(info_sql("public", caps("1.8")).starts_with("SELECT NULL::bigint, NULL::bigint,"));
        assert!(info_sql("public", caps("1.9")).contains("FROM public.pg_stat_statements_info"));
        assert!(texts_sql("public", caps("1.9")).contains("pg_stat_statements(true) s WHERE s.queryid = ANY($1)"));
        assert!(texts_sql("public", caps("1.8")).contains("s.userid, true, left(s.query"));
    }

    #[test]
    fn since_reset_filters_sorts_and_sums() {
        let rows = vec![
            cumulative(1, true, "perf", 10, 100.0),
            cumulative(2, false, "perf", 10, 900.0),
            cumulative(3, true, "other", 10, 500.0),
            cumulative(4, true, "perf", 10, 300.0),
        ];
        let (out, t) = stats_since_reset(&rows, &params(QueriesMode::SinceReset));
        // Только toplevel, обе базы: 1, 3, 4 — по убыванию времени.
        assert_eq!(out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [3, 4, 1]);
        assert_eq!((t.calls, t.total_exec_ms), (30, 900.0));
        assert_eq!(out[0].share_pct, 500.0 / 900.0 * 100.0);
        assert_eq!(out[0].stddev_exec_ms, Some(0.0), "накопительные данные несут min/max/stddev");
        let mut p = params(QueriesMode::SinceReset);
        p.database = Some("perf".into());
        p.toplevel_only = false;
        p.limit = 50;
        let (out, t) = stats_since_reset(&rows, &p);
        assert_eq!(out.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [2, 4, 1]);
        assert_eq!(t.total_exec_ms, 1300.0);
    }

    #[test]
    fn attach_texts_uses_the_lookup() {
        let rows = vec![cumulative(1, true, "perf", 1, 1.0), cumulative(2, true, "perf", 1, 1.0)];
        let (mut out, _) = stats_since_reset(&rows, &params(QueriesMode::SinceReset));
        attach_texts(&mut out, |k| (k.query_id == 1).then(|| (std::sync::Arc::<str>::from("SELECT 1"), true)));
        assert_eq!((out[0].text.as_deref(), out[0].text_truncated), (Some("SELECT 1"), true));
        assert_eq!((out[1].text.as_deref(), out[1].text_truncated), (None, false));
    }

    #[test]
    fn params_are_validated() {
        assert!(validate(&params(QueriesMode::SinceReset)).is_ok());
        assert!(validate(&params(QueriesMode::Window { s: 900 })).is_ok());
        assert_eq!(validate(&params(QueriesMode::Window { s: 60 })).unwrap_err().key, "errors.diag.badWindow");
        let mut p = params(QueriesMode::SinceReset);
        p.limit = 70;
        assert_eq!(validate(&p).unwrap_err().key, "errors.queries.badLimit");
        let between = params(QueriesMode::Between { from: "a".into(), to: "b".into() });
        let e = validate(&between).unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::FeatureUnavailable, "errors.queries.betweenUnsupported"));
    }

    #[test]
    fn request_and_data_json() {
        let r: crate::diag::DiagRequest = serde_json::from_value(serde_json::json!({
            "tool": "queries", "member": null,
            "queries": {"mode": {"kind": "window", "s": 300}, "sort": "hitRatio", "limit": 100, "database": "perf", "toplevelOnly": false}
        }))
        .unwrap();
        let q = r.queries.unwrap();
        assert_eq!((q.mode, q.sort, q.limit, q.database.as_deref(), q.toplevel_only), (QueriesMode::Window { s: 300 }, QuerySort::HitRatio, 100, Some("perf"), false));
        let r: crate::diag::DiagRequest = serde_json::from_value(serde_json::json!({"tool": "queries", "queries": {"mode": {"kind": "sinceReset"}, "sort": "calls", "limit": 50, "database": null}})).unwrap();
        assert!(r.queries.unwrap().toplevel_only, "toplevelOnly по умолчанию включён");
        let v = serde_json::to_value(crate::diag::DiagData::Queries(QueriesData::empty(QueriesMode::SinceReset))).unwrap();
        assert_eq!((&v["kind"], &v["mode"]["kind"], &v["rows"], &v["pgss"]), (&"queries".into(), &"sinceReset".into(), &serde_json::json!([]), &serde_json::Value::Null));
        assert_eq!(v["totals"]["calls"], 0);
    }

    #[test]
    fn queries_tool_needs_params() {
        let req = crate::diag::DiagRequest { tool: crate::diag::Tool::Queries, member: None, window_s: None, queries: None };
        assert_eq!(crate::diag::validate(&req, crate::conn::profile::ConnectionKind::Server).unwrap_err().key, "errors.queries.paramsRequired");
        assert!(!crate::diag::Tool::Queries.is_cluster() && !crate::diag::Tool::Queries.patroni_only());
    }
}
```

`src-tauri/tests/it_queries.rs`:

```rust
//! Инструмент «Запросы» на стенде: топ «с момента сброса» на pg-perf (`GPG_IT_PERF`), SQL pgss на матрице
//! 13–18 (`GPG_IT_MATRIX`), отсутствие расширения на pg-single, скрытые строки под ролью без pg_read_all_stats.
//! Запускать с `--test-threads=1`; перед тестами pg-perf должен был поработать под `heavy.sql` (dev/perf/README.md).

mod common;

use glafira_pg_lib::diag::queries::{self, Detect, QueriesMode, QueriesParams};
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use pg_advisor::queries::QuerySort;

fn since_reset(sort: QuerySort) -> QueriesParams {
    QueriesParams { mode: QueriesMode::SinceReset, sort, limit: 50, database: None, toplevel_only: true }
}

fn request(p: QueriesParams) -> DiagRequest {
    DiagRequest { tool: Tool::Queries, member: None, window_s: None, queries: Some(p) }
}

#[tokio::test]
async fn top_since_reset_on_perf() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime))).await.unwrap();
    let DiagData::Queries(d) = &r.data else { panic!("не queries: {:?}", r.data) };
    let pgss = d.pgss.as_ref().expect("pgss установлен в perf");
    assert!(pgss.version.starts_with("1."), "{}", pgss.version);
    assert!(pgss.track_planning && pgss.max == 5000);
    assert!(pgss.stats_reset_ms.is_some() && pgss.dealloc.is_some(), "pgss 1.9+: info есть");
    assert!(!d.rows.is_empty() && d.totals.calls > 0);
    assert!(d.rows.windows(2).all(|w| w[0].total_exec_ms >= w[1].total_exec_ms), "по убыванию времени");
    assert!(d.rows.iter().all(|x| x.key.toplevel && x.datname.is_some() && x.text.is_none()));
    assert!(r.unavailable.iter().all(|u| u.part != "statements" && u.part != "others"), "{:?}", r.unavailable);
    // Тексты — отдельной командой по ключам.
    let keys: Vec<_> = d.rows.iter().take(3).map(|x| x.key).collect();
    let texts = queries::texts(&st, "it", None, keys.clone()).await.unwrap();
    assert_eq!(texts.len(), 3);
    assert!(texts.iter().all(|t| keys.contains(&t.key) && t.text.as_deref().is_some_and(|s| !s.is_empty())));
    // Сортировка по hit ratio — по возрастанию, без чтений — в конце.
    let r = diag::run(&st, "it", request(since_reset(QuerySort::HitRatio))).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    let ratios: Vec<_> = d.rows.iter().map(|x| x.hit_ratio).collect();
    let known: Vec<f64> = ratios.iter().flatten().copied().collect();
    assert!(known.windows(2).all(|w| w[0] <= w[1]), "{ratios:?}");
    assert!(ratios.iter().skip_while(|r| r.is_some()).all(|r| r.is_none()), "None в конце: {ratios:?}");
}

#[tokio::test]
async fn missing_extension_is_unavailable_not_an_error() {
    let Some(single) = common::env("GPG_IT_SINGLE") else { return };
    let (_dir, st) = common::open_server(&single).await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime))).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert!(d.rows.is_empty() && d.pgss.is_none());
    assert!(r.unavailable.iter().any(|u| u.part == "statements"), "{:?}", r.unavailable);
}

#[tokio::test]
async fn role_without_read_all_stats_sees_hidden_rows() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_as(&perf, "perf", common::APP_USER, common::APP_PASSWORD).await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime))).await.unwrap();
    assert!(r.unavailable.iter().any(|u| u.part == "others"), "{:?}", r.unavailable);
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert!(d.rows.iter().all(|x| x.rolname.as_deref() == Some(common::APP_USER)), "видны только свои строки");
}

#[tokio::test]
async fn statements_sql_runs_on_every_version() {
    let mut addrs = common::matrix_addrs();
    addrs.extend(common::perf_addr().map(|a| a));
    addrs.extend(common::lab_addr());
    for addr in addrs {
        let db = if common::perf_addr().as_deref() == Some(addr.as_str()) { "perf" } else { "postgres" };
        let c = common::connect_as(&addr, db, common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let Detect::Installed(pgss) = queries::detect(&c).await.unwrap() else { panic!("{addr}: pgss не найден") };
        let version = diag::head(&c).await.unwrap().version_num;
        // Версия расширения растёт вместе с сервером: 13 → 1.8, 14 → 1.9, 15/16 → 1.10, 17 → 1.11, 18 → 1.12.
        let expected = match version / 10_000 { 13 => "1.8", 14 => "1.9", 15 | 16 => "1.10", 17 => "1.11", _ => "1.12" };
        assert_eq!(pgss.version, expected, "{addr}");
        let snap = queries::read_cumulative(&c, &pgss).await.unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(!snap.rows.is_empty(), "{addr}: хотя бы запрос самого detect");
        assert!(snap.info.max > 0, "{addr}");
        assert_eq!(snap.info.dealloc.is_some(), pgss.caps.info, "{addr}");
        let keys: Vec<_> = snap.rows.iter().take(2).map(|r| r.key).collect();
        let texts = c.query(&queries::texts_sql(&pgss.schema, pgss.caps), &[&keys.iter().map(|k| k.query_id).collect::<Vec<i64>>()]).await.unwrap();
        assert!(!texts.is_empty(), "{addr}");
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib diag::queries`
Expected: ошибка компиляции (`diag::queries` нет).

- [ ] **Step 3: Реализация `diag/queries.rs`**

```rust
//! Инструмент «Запросы» (спец. P3 §4): поиск `pg_stat_statements`, SQL по версии расширения, режим «с момента
//! сброса» и ленивые тексты. Режим «за окно» — из буфера сэмплера pgss (`sampler::statements`).

use std::sync::Arc;

use pg_advisor::pgss::PgssCaps;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{Counters, Cumulative, QueryKey, QuerySort, QueryStat, Totals, stat, top, totals};
use serde::{Deserialize, Serialize};
use tokio_postgres::{Client, Row};

use crate::catalog::ddl::quote_ident;
use crate::conn::errors::from_pg;
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::{DiagData, DiagReport, Tool, Unavailable, WINDOWS_S, head, make_report, on_member};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::topology::runtime::DiagTarget;

/// Пределы строк отчёта (спец. §4.2).
pub const LIMITS: [usize; 3] = [50, 100, 200];

/// Установленное расширение: схема (для вызова функции и представления), версия и её возможности.
#[derive(Debug, Clone, PartialEq)]
pub struct PgssInstalled {
    pub schema: String,
    pub version: String,
    pub caps: PgssCaps,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Detect {
    Installed(PgssInstalled),
    Missing,
    /// Версия старше 1.8: нет `*_exec_time`, планирования и WAL.
    TooOld(String),
}

const DETECT_SQL: &str = "SELECT e.extversion::text, n.nspname::text FROM pg_extension e \
    JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_stat_statements'";

pub async fn detect(c: &Client) -> AppResult<Detect> {
    let Some(r) = c.query_opt(DETECT_SQL, &[]).await.map_err(|e| from_pg(&e))? else {
        return Ok(Detect::Missing);
    };
    let (version, schema): (String, String) = (r.get(0), r.get(1));
    Ok(match PgssCaps::from_version(&version) {
        Some(caps) => Detect::Installed(PgssInstalled {
            schema: quote_ident(&schema),
            version,
            caps,
        }),
        None => Detect::TooOld(version),
    })
}

/// Строки pgss без текстов (`showtext := false`): файл текстов не читается. `where_clause` — пусто или
/// ` WHERE s.queryid = $1 AND d.datname = $2` (карточка). Порядок колонок — `row_from`.
pub fn statements_sql(ns: &str, caps: PgssCaps, where_clause: &str) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    let (read_ms, write_ms) = if caps.shared_io_names {
        ("s.shared_blk_read_time", "s.shared_blk_write_time")
    } else {
        ("s.blk_read_time", "s.blk_write_time")
    };
    let temp_io = if caps.temp_io_time {
        "s.temp_blk_read_time + s.temp_blk_write_time"
    } else {
        "NULL::float8"
    };
    // `jit_deform_time` (1.11) входит в `jit_generation_time` и отдельно не прибавляется.
    let (jit_functions, jit_ms) = if caps.jit {
        (
            "s.jit_functions",
            "s.jit_generation_time + s.jit_inlining_time + s.jit_optimization_time + s.jit_emission_time",
        )
    } else {
        ("NULL::bigint", "NULL::float8")
    };
    let (par_planned, par_launched) = if caps.parallel {
        ("s.parallel_workers_to_launch", "s.parallel_workers_launched")
    } else {
        ("NULL::bigint", "NULL::bigint")
    };
    format!(
        "SELECT s.userid, s.dbid, s.queryid, {toplevel}, d.datname::text, r.rolname::text, \
         s.calls, s.total_exec_time, s.min_exec_time, s.max_exec_time, s.mean_exec_time, s.stddev_exec_time, s.rows, \
         s.plans, s.total_plan_time, \
         s.shared_blks_hit, s.shared_blks_read, s.shared_blks_dirtied, s.shared_blks_written, \
         s.local_blks_hit, s.local_blks_read, s.local_blks_dirtied, s.local_blks_written, \
         s.temp_blks_read, s.temp_blks_written, {read_ms}, {write_ms}, {temp_io}, \
         s.wal_records, s.wal_fpi, s.wal_bytes::bigint, {jit_functions}, {jit_ms}, {par_planned}, {par_launched} \
         FROM {ns}.pg_stat_statements(false) s \
         LEFT JOIN pg_database d ON d.oid = s.dbid \
         LEFT JOIN pg_roles r ON r.oid = s.userid{where_clause}"
    )
}

/// `stats_reset` (мс эпохи) и `dealloc` из `pg_stat_statements_info` (1.9+), предел строк и `track_planning`.
pub fn info_sql(ns: &str, caps: PgssCaps) -> String {
    let info = if caps.info {
        format!(
            "(SELECT (extract(epoch FROM stats_reset) * 1000)::bigint FROM {ns}.pg_stat_statements_info), \
             (SELECT dealloc FROM {ns}.pg_stat_statements_info)"
        )
    } else {
        "NULL::bigint, NULL::bigint".to_string()
    };
    format!(
        "SELECT {info}, current_setting('pg_stat_statements.max')::bigint, \
         current_setting('pg_stat_statements.track_planning', true)"
    )
}

/// Тексты по ключам (`showtext := true`, `$1` — массив `queryid`): не длиннее `QUERY_LIMIT` и признак обрезки.
pub fn texts_sql(ns: &str, caps: PgssCaps) -> String {
    let toplevel = if caps.toplevel { "s.toplevel" } else { "true" };
    format!(
        "SELECT s.queryid, s.dbid, s.userid, {toplevel}, left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} \
         FROM {ns}.pg_stat_statements(true) s WHERE s.queryid = ANY($1)"
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PgssInfo {
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
    pub max: i64,
    pub track_planning: bool,
}

/// Чтение pgss: строки и состояние. `hidden` — были строки с `queryid = NULL` (чужие без `pg_read_all_stats`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub rows: Vec<Cumulative>,
    pub info: PgssInfo,
    pub hidden: bool,
}

/// Строка `statements_sql`; `None` — `queryid` скрыт правами.
pub fn row_from(r: &Row) -> Option<Cumulative> {
    let query_id: i64 = r.get::<_, Option<i64>>(2)?;
    Some(Cumulative {
        key: QueryKey {
            query_id,
            dbid: r.get(1),
            userid: r.get(0),
            toplevel: r.get::<_, Option<bool>>(3).unwrap_or(true),
        },
        datname: r.get(4),
        rolname: r.get(5),
        counters: Counters {
            calls: r.get(6),
            total_exec_ms: r.get(7),
            rows: r.get(12),
            plans: r.get(13),
            total_plan_ms: r.get(14),
            shared_hit: r.get(15),
            shared_read: r.get(16),
            shared_dirtied: r.get(17),
            shared_written: r.get(18),
            local_hit: r.get(19),
            local_read: r.get(20),
            local_dirtied: r.get(21),
            local_written: r.get(22),
            temp_read: r.get(23),
            temp_written: r.get(24),
            io_read_ms: r.get(25),
            io_write_ms: r.get(26),
            temp_io_ms: r.get(27),
            wal_records: r.get(28),
            wal_fpi: r.get(29),
            wal_bytes: r.get(30),
            jit_functions: r.get(31),
            jit_ms: r.get(32),
            parallel_planned: r.get(33),
            parallel_launched: r.get(34),
        },
        min_exec_ms: r.get(8),
        max_exec_ms: r.get(9),
        stddev_exec_ms: r.get(11),
    })
}

pub async fn read_cumulative(c: &Client, pgss: &PgssInstalled) -> AppResult<Snapshot> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let raw = c
        .query(&statements_sql(&pgss.schema, pgss.caps, ""), &[])
        .await
        .map_err(pg)?;
    let mut hidden = false;
    let rows = raw
        .iter()
        .filter_map(|r| {
            let row = row_from(r);
            hidden |= row.is_none();
            row
        })
        .collect();
    let i = c
        .query_one(&info_sql(&pgss.schema, pgss.caps), &[])
        .await
        .map_err(pg)?;
    let info = PgssInfo {
        stats_reset_ms: i.get(0),
        dealloc: i.get(1),
        max: i.get(2),
        track_planning: i.get::<_, Option<String>>(3).as_deref() == Some("on"),
    };
    Ok(Snapshot { rows, info, hidden })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum QueriesMode {
    SinceReset,
    Window { s: u32 },
    /// Между снимками — P3b; в P3a отклоняется `validate`.
    Between { from: String, to: String },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueriesParams {
    pub mode: QueriesMode,
    pub sort: QuerySort,
    pub limit: usize,
    pub database: Option<String>,
    #[serde(default = "default_true")]
    pub toplevel_only: bool,
}

pub fn validate(p: &QueriesParams) -> AppResult<()> {
    if !LIMITS.contains(&p.limit) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.queries.badLimit",
            "The row limit must be 50, 100 or 200"
        ));
    }
    match &p.mode {
        QueriesMode::SinceReset => Ok(()),
        QueriesMode::Window { s } if WINDOWS_S.contains(s) => Ok(()),
        QueriesMode::Window { .. } => Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.diag.badWindow",
            "The window must be 300, 900 or 3600 seconds"
        )),
        QueriesMode::Between { .. } => Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.queries.betweenUnsupported",
            "Comparing snapshots is not available yet"
        )),
    }
}

/// Состояние расширения в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PgssMeta {
    pub version: String,
    pub max: i64,
    pub track_planning: bool,
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
}

impl PgssMeta {
    pub fn of(pgss: &PgssInstalled, info: &PgssInfo) -> Self {
        Self {
            version: pgss.version.clone(),
            max: info.max,
            track_planning: info.track_planning,
            stats_reset_ms: info.stats_reset_ms,
            dealloc: info.dealloc,
        }
    }
}

/// Данные инструмента (спец. §4.5). `coverage_s` и `ticks` — только у режима окна.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueriesData {
    pub mode: QueriesMode,
    pub coverage_s: Option<u32>,
    pub ticks: Option<u32>,
    pub pgss: Option<PgssMeta>,
    pub totals: Totals,
    pub rows: Vec<QueryStat>,
}

impl QueriesData {
    pub fn empty(mode: QueriesMode) -> Self {
        Self {
            mode,
            coverage_s: None,
            ticks: None,
            pgss: None,
            totals: Totals::default(),
            rows: Vec::new(),
        }
    }
}

/// Фильтр строк по параметрам: база и верхний уровень.
pub fn keep(c: &Cumulative, p: &QueriesParams) -> bool {
    (!p.toplevel_only || c.key.toplevel)
        && p
            .database
            .as_deref()
            .is_none_or(|d| c.datname.as_deref() == Some(d))
}

/// Строки отчёта «с момента сброса»: фильтр, итоги по отфильтрованному набору, сортировка и обрезка.
pub fn stats_since_reset(rows: &[Cumulative], p: &QueriesParams) -> (Vec<QueryStat>, Totals) {
    let kept: Vec<&Cumulative> = rows.iter().filter(|c| keep(c, p)).collect();
    let t = totals(kept.iter().map(|c| &c.counters));
    let mut out: Vec<QueryStat> = kept
        .iter()
        .map(|c| {
            stat(
                c.key,
                c.datname.clone(),
                c.rolname.clone(),
                &c.counters,
                Some((c.min_exec_ms, c.max_exec_ms, c.stddev_exec_ms)),
                &t,
            )
        })
        .collect();
    top(&mut out, p.sort, p.limit);
    (out, t)
}

/// Подставляет тексты из кеша: `lookup` отдаёт текст и признак обрезки.
pub fn attach_texts(rows: &mut [QueryStat], lookup: impl Fn(&QueryKey) -> Option<(Arc<str>, bool)>) {
    for r in rows {
        if let Some((text, truncated)) = lookup(&r.key) {
            r.text = Some(text.to_string());
            r.text_truncated = truncated;
        }
    }
}

pub fn missing() -> Unavailable {
    Unavailable {
        part: "statements".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsMissing",
            "{{part}}: pg_stat_statements is not installed in this database",
            part = "statements"
        ),
    }
}

pub fn too_old(version: &str) -> Unavailable {
    Unavailable {
        part: "statements".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsOld",
            "{{part}}: pg_stat_statements {{version}} is older than 1.8",
            part = "statements",
            version = version
        ),
    }
}

pub fn hidden_rows() -> Unavailable {
    Unavailable {
        part: "others".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsHidden",
            "{{part}}: needs the pg_read_all_stats role",
            part = "others"
        ),
    }
}

pub fn window_starting() -> Unavailable {
    Unavailable {
        part: "window".into(),
        member: None,
        reason: ltext!(
            "diag.unavailable.statementsStarting",
            "{{part}}: the statements sampler is starting",
            part = "window"
        ),
    }
}

fn sort_unavailable() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.queries.sortUnavailable",
        "This sort order is not available for this version or mode"
    )
}

fn member_name(target: &DiagTarget) -> AppResult<String> {
    target
        .member
        .as_ref()
        .map(|m| m.name.clone())
        .ok_or_else(|| AppError::internal_with("queries without a member"))
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget, p: &QueriesParams) -> AppResult<DiagReport> {
    validate(p)?;
    match &p.mode {
        QueriesMode::SinceReset => report_since_reset(st, id, target, p).await,
        QueriesMode::Window { s } => report_window(st, id, target, p, *s).await,
        QueriesMode::Between { .. } => Err(AppError::internal_with("between mode passed validation")),
    }
}

async fn report_since_reset(st: &AppState, id: &str, target: &DiagTarget, p: &QueriesParams) -> AppResult<DiagReport> {
    let member = member_name(target)?;
    let (h, detected, snapshot) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let d = detect(c).await?;
            let snap = match &d {
                Detect::Installed(pgss) => Some(read_cumulative(c, pgss).await?),
                _ => None,
            };
            Ok((h, d, snap))
        })
    })
    .await?;
    let mut unavailable = Vec::new();
    let (data, findings) = match (detected, snapshot) {
        (Detect::Installed(pgss), Some(snap)) => {
            if !p.sort.available(&pgss.caps, false) {
                return Err(sort_unavailable());
            }
            let (mut rows, t) = stats_since_reset(&snap.rows, p);
            attach_texts(&mut rows, |k| cached_text(st, id, &member, k));
            if snap.hidden {
                unavailable.push(hidden_rows());
            }
            let state = PgssState {
                track_planning: snap.info.track_planning,
                dealloc: snap.info.dealloc,
                dealloc_grew: false,
                max: snap.info.max,
            };
            let findings = rules::evaluate(Some(&member), &rows, &t, &state, Mode::SinceReset);
            (
                QueriesData {
                    mode: p.mode.clone(),
                    coverage_s: None,
                    ticks: None,
                    pgss: Some(PgssMeta::of(&pgss, &snap.info)),
                    totals: t,
                    rows,
                },
                findings,
            )
        }
        (Detect::TooOld(v), _) => {
            unavailable.push(too_old(&v));
            (QueriesData::empty(p.mode.clone()), Vec::new())
        }
        _ => {
            unavailable.push(missing());
            (QueriesData::empty(p.mode.clone()), Vec::new())
        }
    };
    Ok(make_report(Tool::Queries, target, Some(&h), DiagData::Queries(data), findings, unavailable))
}

/// Режим окна: буфер сэмплера pgss появляется в задаче 6; без него — пустой отчёт с пометкой «сэмплер запускается».
async fn report_window(st: &AppState, id: &str, target: &DiagTarget, p: &QueriesParams, _window_s: u32) -> AppResult<DiagReport> {
    let h = on_member(st, id, target, |c| Box::pin(head(c))).await?;
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(QueriesData::empty(p.mode.clone())),
        Vec::new(),
        vec![window_starting()],
    ))
}

/// Текст из кеша узла; кеш живёт в буфере сэмплера pgss (задача 6) — до него текстов в кеше нет.
pub fn cached_text(_st: &AppState, _id: &str, _member: &str, _key: &QueryKey) -> Option<(Arc<str>, bool)> {
    None
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryText {
    pub key: QueryKey,
    pub text: Option<String>,
    pub truncated: bool,
}

/// Тексты запросов по ключам — одним запросом (`showtext := true`); ключи, которых в pgss уже нет, пропускаются.
pub async fn texts(st: &AppState, id: &str, member: Option<&str>, keys: Vec<QueryKey>) -> AppResult<Vec<QueryText>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let target = st.runtimes.diag_target(id, member, false)?;
    let ids: Vec<i64> = keys.iter().map(|k| k.query_id).collect();
    let rows = on_member(st, id, &target, |c| {
        Box::pin(async move {
            let Detect::Installed(pgss) = detect(c).await? else {
                return Ok(Vec::new());
            };
            let rows = c
                .query(&texts_sql(&pgss.schema, pgss.caps), &[&ids])
                .await
                .map_err(|e| from_pg(&e))?;
            Ok(rows
                .iter()
                .filter_map(|r| {
                    let key = QueryKey {
                        query_id: r.get::<_, Option<i64>>(0)?,
                        dbid: r.get(1),
                        userid: r.get(2),
                        toplevel: r.get::<_, Option<bool>>(3).unwrap_or(true),
                    };
                    Some(QueryText {
                        key,
                        text: r.get(4),
                        truncated: r.get::<_, Option<bool>>(5).unwrap_or(false),
                    })
                })
                .filter(|t| keys.contains(&t.key))
                .collect())
        })
    })
    .await?;
    Ok(rows)
}
```

- [ ] **Step 4: Каркас диагностики**

`src-tauri/src/diag/mod.rs`:
- `pub mod queries;` (по алфавиту, после `pulse`); импорт `use queries::{QueriesData, QueriesParams};`.
- в `enum Tool` после `Health`: `Queries,`.
- в `DiagRequest` после `window_s`:

```rust
    /// Только `queries`: режим, сортировка, предел, фильтры (спец. P3 §4.5).
    #[serde(default)]
    pub queries: Option<QueriesParams>,
```

- в `enum DiagData` после `Health`: `Queries(QueriesData),`.
- в `validate` перед `Ok(())`:

```rust
    if req.tool == Tool::Queries && req.queries.is_none() {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.queries.paramsRequired",
            "The queries tool needs its parameters"
        ));
    }
```

- в `run` после `Tool::Health`:

```rust
        Tool::Queries => {
            let p = req.queries.as_ref().ok_or_else(|| AppError::internal_with("queries without params"))?;
            queries::report(st, id, &target, p).await
        }
```

- в тестах `diag/mod.rs` все литералы `DiagRequest { tool, member, window_s }` получают `queries: None` (функция `req` в `request_validation`, тест `request_and_report_json`: `assert_eq!((r.tool, r.member, r.window_s), …)` остаётся). В `src-tauri/tests/it_diag.rs` и `it_sampler.rs` литералы `DiagRequest { … }` — тоже `queries: None` (grep `window_s: None`).

`src-tauri/src/commands/queries.rs`:

```rust
//! Команды инструмента «Запросы» и карточки запроса (спец. P3 §10).

use pg_advisor::queries::QueryKey;
use tauri::State;

use crate::diag::queries::QueryText;
use crate::error::AppResult;
use crate::state::AppState;

/// Тексты запросов по ключам (ленивая подгрузка видимых строк).
#[tauri::command]
pub async fn query_texts(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    keys: Vec<QueryKey>,
) -> AppResult<Vec<QueryText>> {
    crate::diag::queries::texts(&state, &id, member.as_deref(), keys).await
}
```

`src-tauri/src/commands/mod.rs` — `pub mod queries;`; `src-tauri/src/lib.rs` — в `generate_handler!` после `commands::diag::stop_session`: `commands::queries::query_texts,`.

Словари (`src/locales/en.json`; в `ru.json` — перевод):
- `errors.queries`: `{ "paramsRequired": "The queries tool needs its parameters", "badLimit": "The row limit must be 50, 100 or 200", "betweenUnsupported": "Comparing snapshots is not available yet", "sortUnavailable": "This sort order is not available for this version or mode" }`
  ru: `{ "paramsRequired": "Инструменту «Запросы» нужны его параметры", "badLimit": "Предел строк — 50, 100 или 200", "betweenUnsupported": "Сравнение снимков пока недоступно", "sortUnavailable": "Эта сортировка недоступна для этой версии или режима" }`
- `diag.unavailable`: `"statementsMissing": "{{part}}: pg_stat_statements is not installed in this database"`, `"statementsOld": "{{part}}: pg_stat_statements {{version}} is older than 1.8"`, `"statementsHidden": "{{part}}: needs the pg_read_all_stats role"`, `"statementsStarting": "{{part}}: the statements sampler is starting"`; в `diag.unavailable.parts`: `"statements": "Query statistics (pg_stat_statements)"`, `"others": "Other roles' queries"`, `"window": "Window data"`.
  ru: `"statementsMissing": "{{part}}: pg_stat_statements не установлен в этой базе"`, `"statementsOld": "{{part}}: pg_stat_statements {{version}} старше 1.8"`, `"statementsHidden": "{{part}}: нужна роль pg_read_all_stats"`, `"statementsStarting": "{{part}}: сэмплер запросов запускается"`; parts: `"statements": "Статистика запросов (pg_stat_statements)"`, `"others": "Запросы других ролей"`, `"window": "Данные окна"`.

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS (включая старые тесты `diag` и `it_*` с `queries: None`).

Run: `GPG_IT_PERF=localhost:5471 GPG_IT_SINGLE=localhost:5460 GPG_IT_LAB=localhost:5470 GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 cargo test --manifest-path src-tauri/Cargo.toml --test it_queries -- --test-threads=1`
Expected: 4 теста PASS (матрица поднята `--profile matrix up -d --no-recreate`; pg-perf поработал под `heavy.sql`).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/diag src-tauri/src/commands src-tauri/src/lib.rs src-tauri/tests src/locales
git commit -m "feat(queries): инструмент «Запросы» — поиск pg_stat_statements, SQL по версии расширения, режим «с момента сброса», тексты по ключам

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Реестр сэмплеров — обобщение трейтом `Sampling`

**Files:**
- Modify: `src-tauri/src/sampler/registry.rs`, `src-tauri/src/sampler/mod.rs`, `src-tauri/src/sampler/query.rs`, `src-tauri/src/commands/diag.rs`, `src-tauri/src/state.rs`, `src-tauri/tests/it_sampler.rs`
- Test: существующие unit-тесты `registry.rs` и `sampler/mod.rs` (меняются только сигнатуры), `it_sampler.rs`

**Interfaces:**
- Produces (`sampler::registry`): `trait Sampling { type Sample; type Buffer: Default; type Event: Clone; fn interval(&self) -> Duration; fn push(buffer: &mut Buffer, at_ms: i64, sample: Sample) -> Vec<Event>; fn error_event(AppError) -> Event; fn stopped_event() -> Event; fn fatal(&AppError) -> bool }`; `struct Activity { interval: Duration }` (`impl Sampling`: `Vec<RawSample>` / `Buffer` / `SamplerEvent`); `type ActivityRegistry = SamplerRegistry<Activity>`; `SamplerRegistry<S: Sampling>::new(sampling: S, sessions) -> Arc<Self>`; `trait SamplerSink<E> { fn send(&self, e: E) -> bool }`; `trait SampleSource<T> { fn sample(&self) -> BoxFuture<AppResult<T>>; fn limit(&self) -> Duration }`; методы `subscribe`, `unsubscribe`, `forget_empty_node`, `buffer() -> Option<Arc<Mutex<S::Buffer>>>`, `is_running` — как в P2.
  `sampler::start_with<S: Sampling>(st, id, member, sink, registry: &Arc<SamplerRegistry<S>>, kind: SessionKind, source: impl FnOnce(&AppState, &str, &str) -> Arc<dyn SampleSource<S::Sample>>) -> AppResult<String>`; `sampler::start` — обёртка для активности. `AppState.samplers: Arc<ActivityRegistry>`.

- [ ] **Step 1: Переписать `registry.rs` (вне тестов)**

Заменить всё до `#[cfg(test)]` на:

```rust
//! Реестр сэмплеров (спец. P2 §6, P3 §5): задача на узел, пока подписана хоть одна вкладка; буфер — до закрытия
//! подключения или исчезновения участника. Реестр обобщён трейтом `Sampling`: активность (раз в секунду) и
//! `pg_stat_statements` (раз в минуту) — два экземпляра с разными сэмплами, буферами и событиями.
//!
//! Порядок блокировок. `nodes` — std-мьютекс, держится без `await` и без вызова чужого кода: подписчикам события
//! шлются после того, как блокировка отпущена, реестр сессий (`SessionRegistry::forget`) — тоже вне её.
//! `unsubscribe` вызывается из остановки сессии (`SessionRegistry` её зовёт вне своей блокировки), слушатель
//! закрытия — под блокировкой `entries` `Runtimes`; к `Runtimes` реестр не обращается. Вложенных блокировок
//! `nodes` и реестра сессий нет ни в каком порядке.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::Serialize;
use tokio::task::{AbortHandle, Id};

use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::buffer::{Buffer, RawSample};
use crate::session::SessionRegistry;
use crate::sync::MutexExt;
use crate::topology::runtime::CloseListener;

/// Вид сэмплера: что читается с узла, где копится и какие события получают вкладки.
pub trait Sampling: Send + Sync + 'static {
    type Sample: Send + 'static;
    type Buffer: Default + Send + 'static;
    type Event: Clone + Send + 'static;

    fn interval(&self) -> Duration;
    /// Записывает сэмпл в буфер; события уходят подписчикам по порядку (например, `gap`, затем `tick`).
    fn push(buffer: &mut Self::Buffer, at_ms: i64, sample: Self::Sample) -> Vec<Self::Event>;
    fn error_event(error: AppError) -> Self::Event;
    fn stopped_event() -> Self::Event;
    /// Ошибка, после которой сэмплер останавливается, а не повторяет с паузой. Отказ во входе — у всех видов:
    /// повтор подбирал бы пароль (LDAP, fail2ban).
    fn fatal(error: &AppError) -> bool {
        error.code == ErrorCode::AuthFailed
    }
}

/// Сэмплер активности P2: `pg_stat_activity` раз в секунду.
pub struct Activity {
    pub interval: Duration,
}

impl Sampling for Activity {
    type Sample = Vec<RawSample>;
    type Buffer = Buffer;
    type Event = SamplerEvent;

    fn interval(&self) -> Duration {
        self.interval
    }

    fn push(buffer: &mut Buffer, at_ms: i64, rows: Vec<RawSample>) -> Vec<SamplerEvent> {
        let active = rows.len() as u32;
        let waiting = rows
            .iter()
            .filter(|r| r.wait_type.as_deref() == Some("Lock"))
            .count() as u32;
        let mut out = Vec::with_capacity(2);
        if let Some((from_ms, to_ms)) = buffer.push(at_ms, rows) {
            out.push(SamplerEvent::Gap { from_ms, to_ms });
        }
        out.push(SamplerEvent::Tick {
            at_ms,
            active,
            waiting,
        });
        out
    }

    fn error_event(error: AppError) -> SamplerEvent {
        SamplerEvent::Error { error }
    }

    fn stopped_event() -> SamplerEvent {
        SamplerEvent::Stopped
    }
}

pub type ActivityRegistry = SamplerRegistry<Activity>;

/// Событие канала вкладки сэмплера активности: `tick` — раз в секунду (индикатор «живой»), `gap` — промежуток в
/// истории, `error` — сэмпл не удался (сэмплер продолжает с паузой), `stopped` — сэмплер узла остановлен не
/// вкладкой (отказ во входе, подключение закрыто, участник исчез).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SamplerEvent {
    #[serde(rename_all = "camelCase")]
    Tick {
        at_ms: i64,
        active: u32,
        waiting: u32,
    },
    #[serde(rename_all = "camelCase")]
    Gap {
        from_ms: i64,
        to_ms: i64,
    },
    Error {
        error: AppError,
    },
    Stopped,
}

/// Канал вкладки. `false` — канал закрыт (вкладка закрыта, вебвью перезагружен): подписка снимается.
pub trait SamplerSink<E>: Send + Sync {
    fn send(&self, e: E) -> bool;
}

/// Откуда берутся сэмплы: в приложении — запрос к узлу, в тестах — подмена.
pub trait SampleSource<T>: Send + Sync {
    fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<T>>;

    /// Предел одного сэмпла целиком (подключение и запрос), которым источник ограничивает себя сам и по
    /// которому закрывает соединение. Задача сэмплера ждёт на секунду дольше — только как страховка.
    fn limit(&self) -> Duration {
        SAMPLE_TIMEOUT
    }
}

/// Паузы после неудачных сэмплов подряд, секунды.
const BACKOFF_S: [u64; 5] = [1, 2, 5, 10, 30];
/// Предел самого запроса сэмпла (без подключения): молчащий узел не держит задачу.
pub(crate) const SAMPLE_TIMEOUT: Duration = Duration::from_secs(5);
/// Запас внешнего предела над `SampleSource::limit`. Источник сам ограничен своим пределом и по таймауту
/// закрывает соединение (`on_catalog_node_within`); внешний предел — страховка для источника без своего, и он
/// не должен сработать раньше: брошенный на полуслове запрос вернул бы соединение в пул с недочитанным ответом.
const SAMPLE_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

type Key = (String, String);

struct Node<S: Sampling> {
    /// Номер открытия подключения (`Entry::generation`): буфер прошлого открытия не продолжается.
    generation: u64,
    buffer: Arc<Mutex<S::Buffer>>,
    /// Подписки вкладок: id сессии → канал.
    subscribers: HashMap<String, Arc<dyn SamplerSink<S::Event>>>,
    /// Задача сэмплера; `None` — не идёт.
    task: Option<AbortHandle>,
}

impl<S: Sampling> Node<S> {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            buffer: Arc::new(Mutex::new(S::Buffer::default())),
            subscribers: HashMap::new(),
            task: None,
        }
    }

    /// Задача `id` — текущая задача узла. Прерванная задача может ещё дойти до конца синхронного участка
    /// (прерывание срабатывает на `await`): ни событий, ни изменений узла от неё быть не должно.
    fn runs(&self, id: Id) -> bool {
        self.task.as_ref().is_some_and(|t| t.id() == id)
    }
}

pub struct SamplerRegistry<S: Sampling> {
    nodes: Mutex<HashMap<Key, Node<S>>>,
    sampling: S,
    sessions: Arc<SessionRegistry>,
}

impl<S: Sampling> SamplerRegistry<S> {
    pub fn new(sampling: S, sessions: Arc<SessionRegistry>) -> Arc<Self> {
        Arc::new(Self {
            nodes: Mutex::new(HashMap::new()),
            sampling,
            sessions,
        })
    }

    /// Подписка вкладки на сэмплер узла: узла нет или он от более раннего открытия подключения — новый буфер;
    /// задача сэмплера запускается, если не идёт. Подписка от более раннего открытия, чем у узла, отклоняется:
    /// узел не трогается, вкладке — `stopped`, её сессия забывается. Нужен рантайм tokio (`tokio::spawn`).
    pub fn subscribe(
        self: &Arc<Self>,
        connection: &str,
        generation: u64,
        member: &str,
        session_id: &str,
        sink: Arc<dyn SamplerSink<S::Event>>,
        source: Arc<dyn SampleSource<S::Sample>>,
    ) {
        let key = (connection.to_string(), member.to_string());
        let stale = {
            let mut nodes = self.nodes.lock_safe();
            if nodes.get(&key).is_some_and(|n| n.generation > generation) {
                drop(nodes);
                // Подписка запоздала: подключение уже переоткрыто, и узел принадлежит новому открытию.
                sink.send(S::stopped_event());
                self.sessions.forget(session_id);
                return;
            }
            let node = nodes
                .entry(key.clone())
                .or_insert_with(|| Node::new(generation));
            let stale = (node.generation != generation)
                .then(|| std::mem::replace(node, Node::new(generation)));
            node.subscribers.insert(session_id.to_string(), sink);
            if node.task.is_none() {
                let (weak, buffer) = (Arc::downgrade(self), node.buffer.clone());
                node.task = Some(
                    tokio::spawn(run(weak, key, source, buffer, self.sampling.interval()))
                        .abort_handle(),
                );
            }
            stale
        };
        self.stop_nodes(stale.into_iter().collect());
    }

    /// Убирает узел открытия `generation`, если подписчиков у него не осталось (`true` — убран): так
    /// отменённый запуск не оставляет узел с буфером закрытого подключения. Узел другого открытия или с
    /// подписчиками не трогается; задача сэмплера пустого узла уже снята отпиской.
    pub fn forget_empty_node(&self, connection: &str, member: &str, generation: u64) -> bool {
        let mut nodes = self.nodes.lock_safe();
        let key = (connection.to_string(), member.to_string());
        let gone = nodes
            .get(&key)
            .is_some_and(|n| n.generation == generation && n.subscribers.is_empty());
        if gone
            && let Some(n) = nodes.remove(&key)
            && let Some(t) = n.task
        {
            t.abort();
        }
        gone
    }

    /// Отписка вкладки; последняя — задача прерывается, буфер остаётся.
    pub fn unsubscribe(&self, connection: &str, member: &str, session_id: &str) {
        let mut nodes = self.nodes.lock_safe();
        if let Some(n) = nodes.get_mut(&(connection.to_string(), member.to_string())) {
            n.subscribers.remove(session_id);
            if n.subscribers.is_empty()
                && let Some(t) = n.task.take()
            {
                t.abort();
            }
        }
    }

    pub fn buffer(&self, connection: &str, member: &str) -> Option<Arc<Mutex<S::Buffer>>> {
        self.nodes
            .lock_safe()
            .get(&(connection.to_string(), member.to_string()))
            .map(|n| n.buffer.clone())
    }

    pub fn is_running(&self, connection: &str, member: &str) -> bool {
        self.nodes
            .lock_safe()
            .get(&(connection.to_string(), member.to_string()))
            .is_some_and(|n| n.task.is_some())
    }

    /// Рассылка подписчикам от задачи `me`; закрытые каналы отписываются (и забываются в реестре сессий).
    /// Без подписчиков задача снимается с узла — `false`; задача уже не текущая — тоже `false`, без рассылки.
    fn broadcast(&self, key: &Key, me: Id, e: &S::Event) -> bool {
        let sinks: Vec<(String, Arc<dyn SamplerSink<S::Event>>)> = {
            let nodes = self.nodes.lock_safe();
            match nodes.get(key).filter(|n| n.runs(me)) {
                Some(n) => n
                    .subscribers
                    .iter()
                    .map(|(id, s)| (id.clone(), s.clone()))
                    .collect(),
                None => return false,
            }
        };
        let closed: Vec<String> = sinks
            .into_iter()
            .filter(|(_, s)| !s.send(e.clone()))
            .map(|(id, _)| id)
            .collect();
        let alive = {
            let mut nodes = self.nodes.lock_safe();
            match nodes.get_mut(key).filter(|n| n.runs(me)) {
                Some(n) => {
                    for id in &closed {
                        n.subscribers.remove(id);
                    }
                    // Решение «подписчиков нет — задача снята» — под той же блокировкой, что и подписка:
                    // вкладка, подписавшаяся сразу после, запустит новую задачу, а не повиснет без неё.
                    if n.subscribers.is_empty() {
                        n.task = None;
                    }
                    n.task.is_some()
                }
                None => false,
            }
        };
        for id in &closed {
            self.sessions.forget(id);
        }
        alive
    }

    /// Задача `me` остановилась сама (отказ во входе): узел без задачи, подписчикам — `stopped`, их сессии
    /// закончены. Буфер остаётся до закрытия подключения.
    fn finished(&self, key: &Key, me: Id) {
        let subscribers = {
            let mut nodes = self.nodes.lock_safe();
            match nodes.get_mut(key).filter(|n| n.runs(me)) {
                Some(n) => {
                    n.task = None;
                    std::mem::take(&mut n.subscribers)
                }
                None => return,
            }
        };
        self.stop_subscribers(subscribers);
    }

    fn stop_subscribers(&self, subscribers: HashMap<String, Arc<dyn SamplerSink<S::Event>>>) {
        for (id, sink) in subscribers {
            sink.send(S::stopped_event());
            self.sessions.forget(&id);
        }
    }

    /// Узлы удалены из реестра: задачи прерываются, подписчикам — `stopped`.
    fn stop_nodes(&self, gone: Vec<Node<S>>) {
        for n in gone {
            if let Some(t) = n.task {
                t.abort();
            }
            self.stop_subscribers(n.subscribers);
        }
    }
}

/// Закрытие подключения и исчезновение участника: задачи прерываются, буферы удаляются, подписчикам —
/// `stopped`. Регистрируется в `Runtimes` раньше реестра сессий: иначе остановка сессий успела бы отписать
/// вкладки, и `stopped` никто бы не получил.
impl<S: Sampling> CloseListener for SamplerRegistry<S> {
    fn connection_closed(&self, connection_id: &str) {
        let gone: Vec<Node<S>> = {
            let mut nodes = self.nodes.lock_safe();
            let keys: Vec<Key> = nodes
                .keys()
                .filter(|k| k.0 == connection_id)
                .cloned()
                .collect();
            keys.iter().filter_map(|k| nodes.remove(k)).collect()
        };
        self.stop_nodes(gone);
    }

    fn member_removed(&self, connection_id: &str, member: &str) {
        let gone = self
            .nodes
            .lock_safe()
            .remove(&(connection_id.to_string(), member.to_string()));
        self.stop_nodes(gone.into_iter().collect());
    }
}

/// Задача сэмплера узла: тик раз в `interval`, сэмпл, буфер, события подписчикам.
async fn run<S: Sampling>(
    weak: Weak<SamplerRegistry<S>>,
    key: Key,
    source: Arc<dyn SampleSource<S::Sample>>,
    buffer: Arc<Mutex<S::Buffer>>,
    interval: Duration,
) {
    let me = tokio::task::id();
    let mut tick = tokio::time::interval(interval);
    // Запрос дольше интервала: пропущенные тики не догоняются — запросы не накладываются.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut failures = 0usize;
    loop {
        tick.tick().await;
        // Предел источника может быть больше `SAMPLE_TIMEOUT` (подключение по медленной сети), внешний — за ним.
        let limit = source.limit();
        let result = tokio::time::timeout(limit + SAMPLE_TIMEOUT_GRACE, source.sample()).await;
        let failed = !matches!(result, Ok(Ok(_)));
        let Some(reg) = weak.upgrade() else { return };
        let alive = match result {
            Ok(Ok(sample)) => {
                failures = 0;
                // Время сэмпла — часы приложения (уточнение 10 плана P2a): тики, промежутки и окно отчёта
                // считаются одними часами.
                let at_ms = crate::topology::model::now_ms();
                let events = {
                    let mut b = buffer.lock_safe();
                    S::push(&mut b, at_ms, sample)
                };
                let mut alive = true;
                for e in &events {
                    if !alive {
                        break;
                    }
                    alive = reg.broadcast(&key, me, e);
                }
                alive
            }
            Ok(Err(e)) if S::fatal(&e) => {
                reg.broadcast(&key, me, &S::error_event(e));
                reg.finished(&key, me);
                return;
            }
            Ok(Err(e)) => reg.broadcast(&key, me, &S::error_event(e)),
            Err(_) => reg.broadcast(
                &key,
                me,
                &S::error_event(crate::topology::probe::no_answer(limit)),
            ),
        };
        if !alive {
            reg.finished(&key, me);
            return;
        }
        if failed {
            drop(reg);
            tokio::time::sleep(Duration::from_secs(
                BACKOFF_S[failures.min(BACKOFF_S.len() - 1)],
            ))
            .await;
            failures += 1;
        }
    }
}
```

В тестах `registry.rs` (`#[cfg(test)] mod tests`): `impl SampleSource for Fake` → `impl SampleSource<Vec<RawSample>> for Fake`, то же для `Slow`; `impl SamplerSink for Sink` → `impl SamplerSink<SamplerEvent> for Sink`; `fn reg() -> Arc<SamplerRegistry>` → `Arc<ActivityRegistry>`; каждое `SamplerRegistry::new(Duration::from_millis(20), x)` → `SamplerRegistry::new(Activity { interval: Duration::from_millis(20) }, x)`, `SamplerRegistry::new(Duration::from_secs(1), x)` → `SamplerRegistry::new(Activity { interval: Duration::from_secs(1) }, x)`. Логика тестов не меняется.

- [ ] **Step 2: Остальные пользователи реестра**

`src-tauri/src/sampler/mod.rs`:

```rust
//! Сэмплеры (спец. P2 §6, P3 §5): активность раз в секунду и `pg_stat_statements` раз в минуту — в памяти,
//! пока подписана вкладка.

pub mod buffer;
pub mod query;
pub mod registry;
pub mod statements;

use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::session::{SessionKind, SessionRegistry};
use crate::state::AppState;
use crate::topology::runtime::not_connected;
use registry::{ActivityRegistry, SampleSource, SamplerEvent, SamplerRegistry, SamplerSink, Sampling};

/// Подписка на сэмплер активности узла (`member = None` — «за лидером» на момент подписки). Возвращает id
/// сессии для `stop_session`. Нужен рантайм tokio: задача сэмплера запускается здесь же.
pub fn start(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    sink: Arc<dyn SamplerSink<SamplerEvent>>,
) -> AppResult<String> {
    start_with(st, id, member, sink, &st.samplers, SessionKind::Sampler, |st, id, m| {
        Arc::new(query::PoolSampleSource::new(st.clone(), id, m))
    })
}

/// Подписка на сэмплер вида `S` в реестре `registry`: сессия вида `kind`, источник — `source(st, id, member)`.
pub fn start_with<S: Sampling>(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    sink: Arc<dyn SamplerSink<S::Event>>,
    registry: &Arc<SamplerRegistry<S>>,
    kind: SessionKind,
    source: impl FnOnce(&AppState, &str, &str) -> Arc<dyn SampleSource<S::Sample>>,
) -> AppResult<String> {
    let target = st.runtimes.diag_target(id, member, false)?;
    let m = target
        .member
        .map(|m| m.name)
        .ok_or_else(|| AppError::internal_with("sampler without a member"))?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let source = source(st, id, &m);
    // Остановка сессии только отписывает её (std-мьютекс реестра сэмплеров): годится и для слушателя закрытия.
    st.sessions.insert(
        session_id.clone(),
        id,
        kind,
        unsubscribe_on_stop(registry, id, &m, &session_id),
    );
    registry.subscribe(id, target.generation, &m, &session_id, sink, source);
    // Подключение могли закрыть (или переоткрыть) между выбором узла и подпиской: слушатели закрытия тогда
    // не видели ни сессии, ни подписки. Проверка после подписки — под той же блокировкой `Runtimes`, что и
    // рассылка закрытий: закрытие либо уже было (видно здесь), либо будет и найдёт сессию. Если оно уже было,
    // запись сессии могла уйти раньше подписки, и её остановка подписку не снимет: отписка здесь явная.
    let current = st.runtimes.diag_target(id, Some(&m), false);
    if current.as_ref().map(|t| t.generation).ok() != Some(target.generation) {
        withdraw_late(&st.sessions, registry, id, &m, &session_id, target.generation);
        return Err(current.err().unwrap_or_else(not_connected));
    }
    Ok(session_id)
}
```

`unsubscribe_on_stop` и `withdraw_late` становятся обобщёнными: `fn unsubscribe_on_stop<S: Sampling>(samplers: &Arc<SamplerRegistry<S>>, …)` и `fn withdraw_late<S: Sampling>(sessions: &SessionRegistry, samplers: &SamplerRegistry<S>, …)` — тела без изменений. В тестах `sampler/mod.rs`: `impl SampleSource for Calls` → `impl SampleSource<Vec<RawSample>> for Calls`, `impl SamplerSink for Events` → `impl SamplerSink<SamplerEvent> for Events`, `SamplerRegistry::new(Duration::from_millis(20), …)` → `SamplerRegistry::new(registry::Activity { interval: Duration::from_millis(20) }, …)`. Файл `sampler/statements.rs` создаётся в задаче 6 — до неё строка `pub mod statements;` не добавляется (иначе сборка упадёт): добавить её в задаче 6.

`src-tauri/src/sampler/query.rs`: `impl SampleSource<Vec<RawSample>> for PoolSampleSource`.

`src-tauri/src/commands/diag.rs`: `impl SamplerSink<SamplerEvent> for ChannelSink`.

`src-tauri/src/state.rs`: `use crate::sampler::registry::{Activity, ActivityRegistry, SamplerRegistry};`; поле `pub samplers: Arc<ActivityRegistry>,`; в `new_with_logs`: `let samplers = SamplerRegistry::new(Activity { interval: std::time::Duration::from_secs(1) }, sessions.clone());`.

`src-tauri/tests/it_sampler.rs`: `impl SamplerSink<SamplerEvent> for Events`.

- [ ] **Step 3: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib sampler && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check`
Expected: все прежние тесты реестра и `sampler::tests` PASS без изменений логики.

Run: `GPG_IT_SINGLE=localhost:5460 GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 cargo test --manifest-path src-tauri/Cargo.toml --test it_sampler -- --test-threads=1`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/sampler src-tauri/src/commands/diag.rs src-tauri/src/state.rs src-tauri/tests/it_sampler.rs
git commit -m "refactor(sampler): реестр сэмплеров обобщён трейтом Sampling — вид сэмпла, буфера и событий задаёт экземпляр

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Сэмплер `pg_stat_statements` — буфер дельт, источник, подписка, режим «за окно», кеш текстов

**Files:**
- Create: `src-tauri/src/sampler/statements.rs`
- Modify: `src-tauri/src/sampler/mod.rs` (`pub mod statements;`), `src-tauri/src/session.rs` (`SessionKind::Statements`), `src-tauri/src/state.rs` (`statements`), `src-tauri/src/diag/queries.rs` (`report_window`, `cached_text`, кеш в `texts`), `src-tauri/src/commands/queries.rs` (`start_statements_sampler`), `src-tauri/src/lib.rs`, `src-tauri/tests/it_queries.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `statements.rs`; `it_queries.rs`

**Interfaces:**
- Consumes: `registry::{Sampling, SamplerRegistry, SampleSource, SamplerSink, SAMPLE_TIMEOUT}`, `diag::queries::{detect, read_cumulative, Detect, PgssInstalled, PgssInfo, Snapshot}`, `diag::{head, Head}`, `on_catalog_node_split`, `catalog_limit`.
- Produces (`sampler::statements`): `INTERVAL = 60 с`, `MAX_ENTRIES = 200_000`, `MAX_AGE_MS = 3_600_000`, `GAP_AFTER_MS = 150_000`, `MAX_TEXTS = 2000`; `StatementsSample { head, pgss, snapshot }`; `StatementsEvent` (тег `kind`: `tick { atMs, coverageS, ticks, changed }` | `gap { fromMs, toMs }` | `error { error }` | `stopped`); `HistoryPoint { at_ms, calls, total_exec_ms, mean_exec_ms }`; `WindowSum { since_ms, until_ms, ticks, coverage_s, dealloc_grew, sums: HashMap<QueryKey, Counters> }`; `StatementsBuffer` (`Default`, `with_limits(max_entries, max_age_ms)`, `push(at_ms, sample) -> Vec<StatementsEvent>`, `window(window_s) -> Option<WindowSum>`, `names(&QueryKey) -> (Option<String>, Option<String>)`, `latest_for(query_id, dbid) -> Vec<&Cumulative>`, `history(keys, from_ms) -> Vec<HistoryPoint>`, `gaps(from_ms)`, `last_head()`, `info()`, `pgss()`, `hidden()`, `text(&QueryKey) -> Option<(Arc<str>, bool)>`, `remember_texts(items)`); `Statements` (`impl Sampling`), `type StatementsRegistry = SamplerRegistry<Statements>`; `PoolStatementsSource`; `start(st, id, member, sink) -> AppResult<String>`. `AppState.statements: Arc<StatementsRegistry>`; `SessionKind::Statements`; команда `start_statements_sampler(id, member, channel) -> SessionId`.

- [ ] **Step 1: Падающие unit-тесты**

`src-tauri/src/sampler/statements.rs` — тесты (реализация в шаге 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::pgss::PgssCaps;
    use pg_advisor::queries::Counters;

    fn key(id: i64) -> QueryKey {
        QueryKey { query_id: id, dbid: 1, userid: 1, toplevel: true }
    }

    fn row(id: i64, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: key(id),
            datname: Some("perf".into()),
            rolname: Some("app".into()),
            counters: Counters { calls, total_exec_ms: ms, rows: calls, ..Counters::default() },
            min_exec_ms: 0.0,
            max_exec_ms: 0.0,
            stddev_exec_ms: 0.0,
        }
    }

    fn sample(rows: Vec<Cumulative>, reset_ms: i64, dealloc: i64) -> StatementsSample {
        StatementsSample {
            head: Head { version_num: 170_000, in_recovery: false, now_ms: 0 },
            pgss: PgssInstalled { schema: "public".into(), version: "1.11".into(), caps: PgssCaps::from_version("1.11").unwrap() },
            snapshot: Snapshot {
                rows,
                info: PgssInfo { stats_reset_ms: Some(reset_ms), dealloc: Some(dealloc), max: 5000, track_planning: true },
                hidden: false,
            },
        }
    }

    const M: i64 = 60_000;

    fn ticks(e: &[StatementsEvent]) -> Vec<(u32, u32)> {
        e.iter()
            .filter_map(|e| match e {
                StatementsEvent::Tick { ticks, changed, .. } => Some((*ticks, *changed)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn first_tick_is_a_baseline_and_deltas_follow() {
        let mut b = StatementsBuffer::default();
        let e = b.push(M, sample(vec![row(1, 10, 100.0), row(2, 5, 50.0)], 1, 0));
        assert_eq!(ticks(&e), [(0, 0)], "первый тик — база без дельт");
        assert!(b.window(300).is_none());
        // Второй тик: изменился только ключ 1; ключ 3 появился — дельта равна накопительному.
        let e = b.push(2 * M, sample(vec![row(1, 14, 160.0), row(2, 5, 50.0), row(3, 2, 20.0)], 1, 0));
        assert_eq!(ticks(&e), [(1, 2)]);
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.coverage_s, w.dealloc_grew), (1, 120, false));
        assert_eq!(w.sums[&key(1)].calls, 4);
        assert_eq!(w.sums[&key(1)].total_exec_ms, 60.0);
        assert_eq!(w.sums[&key(3)].calls, 2);
        assert!(!w.sums.contains_key(&key(2)));
        assert_eq!(b.names(&key(1)), (Some("perf".into()), Some("app".into())));
        assert_eq!(b.last_head().map(|h| h.version_num), Some(170_000));
        assert_eq!(b.info().and_then(|i| i.stats_reset_ms), Some(1));
    }

    #[test]
    fn decreased_counters_and_reset_start_a_new_base() {
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 100, 1000.0), row(2, 100, 1000.0)], 1, 0));
        // Ключ 1 вытеснен и вернулся с малыми счётчиками: дельта — его накопительное значение, не отрицательная.
        let e = b.push(2 * M, sample(vec![row(1, 3, 30.0), row(2, 101, 1010.0)], 1, 1));
        assert_eq!(ticks(&e), [(1, 2)]);
        let w = b.window(300).unwrap();
        assert_eq!((w.sums[&key(1)].calls, w.sums[&key(2)].calls, w.dealloc_grew), (3, 1, true));
        // Сброс статистики: новая база для всех ключей, прошлые тики остаются.
        let e = b.push(3 * M, sample(vec![row(1, 2, 20.0), row(2, 7, 70.0)], 2, 0));
        assert_eq!(ticks(&e), [(2, 2)]);
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.sums[&key(1)].calls, w.sums[&key(2)].calls), (2, 5, 8));
        assert!(!w.dealloc_grew, "dealloc после сброса не сравнивается со старым");
    }

    #[test]
    fn long_pause_is_a_gap_and_a_new_base() {
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 10, 100.0)], 1, 0));
        b.push(2 * M, sample(vec![row(1, 12, 120.0)], 1, 0));
        let e = b.push(10 * M, sample(vec![row(1, 500, 5000.0)], 1, 0));
        assert!(matches!(e[0], StatementsEvent::Gap { from_ms, to_ms } if from_ms == 3 * M && to_ms == 10 * M), "{e:?}");
        assert_eq!(ticks(&e), [(0, 0)], "после промежутка — база: активность за паузу не приписывается одному тику");
        assert_eq!(b.gaps(0), [(3 * M, 10 * M)]);
        assert!(b.window(300).is_none(), "в окне 5 мин тиков с дельтами нет");
        assert_eq!(b.window(3600).unwrap().ticks, 1, "старый тик виден в часовом окне");
    }

    #[test]
    fn window_covers_only_recent_ticks_and_coverage_is_capped() {
        let mut b = StatementsBuffer::default();
        for i in 0..8 {
            b.push((i + 1) * M, sample(vec![row(1, 10 * (i + 1), 100.0 * (i + 1) as f64)], 1, 0));
        }
        // Окно 5 мин от последнего тика (8 мин): тики 4…8 — пять тиков с дельтами, покрытие 5 мин.
        let w = b.window(300).unwrap();
        assert_eq!((w.ticks, w.coverage_s, w.sums[&key(1)].calls), (5, 300, 50));
        let w = b.window(3600).unwrap();
        assert_eq!((w.ticks, w.coverage_s, w.sums[&key(1)].calls), (7, 480, 70));
        let h = b.history(&[key(1)], 0);
        assert_eq!(h.len(), 7);
        assert_eq!((h[0].at_ms, h[0].calls, h[0].mean_exec_ms), (2 * M, 10, 10.0));
    }

    #[test]
    fn eviction_drops_whole_old_ticks() {
        let mut b = StatementsBuffer::with_limits(5, MAX_AGE_MS);
        b.push(M, sample((1..=4).map(|i| row(i, 1, 1.0)).collect(), 1, 0));
        b.push(2 * M, sample((1..=4).map(|i| row(i, 2, 2.0)).collect(), 1, 0)); // 4 записи
        b.push(3 * M, sample((1..=4).map(|i| row(i, 3, 3.0)).collect(), 1, 0)); // 8 > 5: первый тик дельт вытеснен
        let w = b.window(3600).unwrap();
        assert_eq!((w.ticks, w.sums[&key(1)].calls), (1, 1));
        // Возраст: тик старше часа уходит целиком.
        let mut b = StatementsBuffer::default();
        b.push(M, sample(vec![row(1, 1, 1.0)], 1, 0));
        b.push(2 * M, sample(vec![row(1, 2, 2.0)], 1, 0));
        b.push(2 * M + MAX_AGE_MS + 1, sample(vec![row(1, 3, 3.0)], 1, 0));
        assert_eq!(b.window(3600).map(|w| w.ticks), None, "после промежутка новая база, старый тик вытеснен возрастом");
    }

    #[test]
    fn texts_cache_is_bounded() {
        let mut b = StatementsBuffer::default();
        b.remember_texts([(key(1), "SELECT 1".to_string(), false)]);
        assert_eq!(b.text(&key(1)).map(|(t, tr)| (t.to_string(), tr)), Some(("SELECT 1".into(), false)));
        assert!(b.text(&key(2)).is_none());
        b.remember_texts((2..=(MAX_TEXTS as i64 + 1)).map(|i| (key(i), format!("q{i}"), false)));
        assert!(b.text(&key(1)).is_none(), "переполнение очищает кеш целиком");
        assert!(b.text(&key(MAX_TEXTS as i64 + 1)).is_some(), "новые тексты записаны");
    }

    #[test]
    fn statements_events_json_and_fatal_errors() {
        let v = serde_json::to_value(StatementsEvent::Tick { at_ms: 1, coverage_s: 120, ticks: 2, changed: 7 }).unwrap();
        assert_eq!(v, serde_json::json!({"kind": "tick", "atMs": 1, "coverageS": 120, "ticks": 2, "changed": 7}));
        assert!(Statements::fatal(&app_err!(ErrorCode::FeatureUnavailable, "errors.queries.pgssMissing", "pg_stat_statements is not installed in this database")));
        assert!(Statements::fatal(&app_err!(ErrorCode::AuthFailed, "errors.pg.authFailed", "x")));
        assert!(!Statements::fatal(&app_err!(ErrorCode::Timeout, "errors.pg.statementTimeout", "x")));
        assert_eq!(Statements.interval(), INTERVAL);
    }
}
```

`src-tauri/tests/it_queries.rs` — добавить:

```rust
use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::sampler::registry::SamplerSink;
use glafira_pg_lib::sampler::statements::{self, StatementsEvent};

#[derive(Default)]
struct Events(Mutex<Vec<StatementsEvent>>);
impl SamplerSink<StatementsEvent> for Events {
    fn send(&self, e: StatementsEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

/// Сэмплер pgss на pg-perf: первый тик — база, второй (через минуту) — дельты; окно считается из буфера без
/// сервера; тексты, прочитанные по ключам, попадают в кеш и возвращаются в строках отчёта.
#[tokio::test]
async fn statements_sampler_fills_the_window_and_caches_texts() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let events = Arc::new(Events::default());
    let sid = statements::start(&st, "it", None, events.clone()).unwrap();
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    common::wait_until("первый тик сэмплера pgss", Duration::from_secs(20), async || {
        events.0.lock().unwrap().iter().any(|e| matches!(e, StatementsEvent::Tick { ticks: 0, .. }))
    })
    .await;
    // Окно до второго тика: пусто с пометкой.
    let window = QueriesParams { mode: QueriesMode::Window { s: 300 }, ..since_reset(QuerySort::TotalTime) };
    let r = diag::run(&st, "it", request(window.clone())).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert!(d.rows.is_empty() && d.coverage_s.is_none());
    assert!(r.unavailable.iter().any(|u| u.part == "window"));
    // Тексты по ключам — в кеш узла.
    let since = diag::run(&st, "it", request(since_reset(QuerySort::Calls))).await.unwrap();
    let DiagData::Queries(d) = &since.data else { unreachable!() };
    let keys: Vec<_> = d.rows.iter().take(2).map(|x| x.key).collect();
    queries::texts(&st, "it", None, keys.clone()).await.unwrap();
    let again = diag::run(&st, "it", request(since_reset(QuerySort::Calls))).await.unwrap();
    let DiagData::Queries(d) = &again.data else { unreachable!() };
    assert!(d.rows.iter().filter(|x| keys.contains(&x.key)).all(|x| x.text.is_some()), "тексты из кеша");
    // Нагрузка между тиками, затем второй тик (до 75 с).
    let app = common::superuser(&perf, "perf").await;
    for _ in 0..50 {
        app.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'disputed'").await.unwrap();
    }
    common::wait_until("второй тик сэмплера pgss", Duration::from_secs(75), async || {
        events.0.lock().unwrap().iter().any(|e| matches!(e, StatementsEvent::Tick { ticks: 1, .. }))
    })
    .await;
    let r = diag::run(&st, "it", request(window)).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert_eq!(d.ticks, Some(1));
    assert!(d.coverage_s.is_some_and(|c| c >= 60 && c <= 300), "{:?}", d.coverage_s);
    assert!(d.rows.iter().any(|x| x.calls >= 50), "дельты за минуту видны: {:?}", d.rows.iter().map(|x| x.calls).collect::<Vec<_>>());
    assert!(d.rows.iter().all(|x| x.stddev_exec_ms.is_none()), "за окно stddev нет");
    assert!(d.pgss.is_some() && r.server_version_num.is_some());
    assert!(st.statements.is_running("it", &member));
    assert!(st.sessions.stop(&sid));
    assert!(!st.statements.is_running("it", &member), "последний подписчик — задача снята");
    assert!(st.statements.buffer("it", &member).is_some(), "буфер остаётся");
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib sampler::statements`
Expected: ошибка компиляции (`sampler::statements` нет).

- [ ] **Step 3: Реализация `sampler/statements.rs`**

```rust
//! Сэмплер `pg_stat_statements` (спец. P3 §5): раз в минуту `pg_stat_statements(showtext := false)` на соединении
//! пула `Sampler`, буфер дельт на 60 мин; живёт, пока подписаны инструмент «Запросы» или карточки запроса узла.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::future::BoxFuture;
use pg_advisor::queries::{Counters, Cumulative, QueryKey};
use serde::Serialize;

use crate::commands::catalog::{catalog_limit, on_catalog_node_split};
use crate::conn::pool::PoolPurpose;
use crate::diag::queries::{Detect, PgssInfo, PgssInstalled, Snapshot, detect, read_cumulative};
use crate::diag::{Head, head};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::registry::{SAMPLE_TIMEOUT, SampleSource, SamplerRegistry, SamplerSink, Sampling};
use crate::session::SessionKind;
use crate::state::AppState;

pub const INTERVAL: Duration = Duration::from_secs(60);
/// Записей `(ключ, дельта)` на узел — порядка 50 МБ.
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_AGE_MS: i64 = 3_600_000;
/// Пауза между тиками дольше — промежуток: сэмплер стоял (вкладка была закрыта), и следующий тик — новая база.
pub const GAP_AFTER_MS: i64 = 150_000;
/// Кеш текстов узла; при переполнении очищается целиком.
pub const MAX_TEXTS: usize = 2000;

/// Сэмпл: шапка узла, установленное расширение и строки pgss.
pub struct StatementsSample {
    pub head: Head,
    pub pgss: PgssInstalled,
    pub snapshot: Snapshot,
}

/// Событие канала вкладки: `tick` — раз в минуту (`ticks` — тиков с дельтами в часовом окне, `changed` — ключей с
/// дельтами в этом тике), `gap`, `error`, `stopped` — как у сэмплера активности.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StatementsEvent {
    #[serde(rename_all = "camelCase")]
    Tick {
        at_ms: i64,
        coverage_s: u32,
        ticks: u32,
        changed: u32,
    },
    #[serde(rename_all = "camelCase")]
    Gap {
        from_ms: i64,
        to_ms: i64,
    },
    Error {
        error: AppError,
    },
    Stopped,
}

/// Точка истории карточки: дельты тика по ключам запроса.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPoint {
    pub at_ms: i64,
    pub calls: i64,
    pub total_exec_ms: f64,
    pub mean_exec_ms: f64,
}

struct Tick {
    at_ms: i64,
    dealloc_delta: i64,
    deltas: Vec<(QueryKey, Counters)>,
}

/// Сумма дельт за окно.
pub struct WindowSum {
    pub since_ms: i64,
    pub until_ms: i64,
    pub ticks: u32,
    pub coverage_s: u32,
    pub dealloc_grew: bool,
    pub sums: HashMap<QueryKey, Counters>,
}

pub struct StatementsBuffer {
    latest: HashMap<QueryKey, Cumulative>,
    ticks: VecDeque<Tick>,
    entries: usize,
    gaps: Vec<(i64, i64)>,
    last_at_ms: Option<i64>,
    last_head: Option<Head>,
    info: Option<PgssInfo>,
    pgss: Option<PgssInstalled>,
    hidden: bool,
    texts: HashMap<QueryKey, (Arc<str>, bool)>,
    max_entries: usize,
    max_age_ms: i64,
}

impl Default for StatementsBuffer {
    fn default() -> Self {
        Self::with_limits(MAX_ENTRIES, MAX_AGE_MS)
    }
}

impl StatementsBuffer {
    pub fn with_limits(max_entries: usize, max_age_ms: i64) -> Self {
        Self {
            latest: HashMap::new(),
            ticks: VecDeque::new(),
            entries: 0,
            gaps: Vec::new(),
            last_at_ms: None,
            last_head: None,
            info: None,
            pgss: None,
            hidden: false,
            texts: HashMap::new(),
            max_entries,
            max_age_ms,
        }
    }

    /// Тик: дельты изменившихся ключей против прошлых накопительных значений. Первый тик и тик после промежутка —
    /// только база (дельт нет). Ключ с уменьшившимися счётчиками (вытеснен и вернулся) и все ключи после смены
    /// `stats_reset` получают новую базу: дельта равна накопительному значению.
    pub fn push(&mut self, at_ms: i64, s: StatementsSample) -> Vec<StatementsEvent> {
        let mut out = Vec::with_capacity(2);
        let gap = self
            .last_at_ms
            .filter(|last| at_ms - last > GAP_AFTER_MS)
            .map(|last| (last + INTERVAL.as_millis() as i64, at_ms));
        if let Some((from_ms, to_ms)) = gap {
            self.gaps.push((from_ms, to_ms));
            out.push(StatementsEvent::Gap { from_ms, to_ms });
        }
        let baseline = self.last_at_ms.is_none() || gap.is_some();
        let reset = self
            .info
            .is_some_and(|i| i.stats_reset_ms != s.snapshot.info.stats_reset_ms);
        let mut deltas = Vec::new();
        if !baseline {
            for row in &s.snapshot.rows {
                let delta = match self.latest.get(&row.key) {
                    Some(prev) if !reset && !row.counters.decreased_from(&prev.counters) => {
                        if !row.counters.changed_since(&prev.counters) {
                            continue;
                        }
                        row.counters.delta(&prev.counters)
                    }
                    _ => row.counters,
                };
                if delta.calls == 0 && delta.total_exec_ms == 0.0 {
                    continue;
                }
                deltas.push((row.key, delta));
            }
        }
        let dealloc_delta = match (self.info.and_then(|i| i.dealloc), s.snapshot.info.dealloc) {
            (Some(prev), Some(now)) if !baseline && !reset && now > prev => now - prev,
            _ => 0,
        };
        self.latest = s.snapshot.rows.into_iter().map(|r| (r.key, r)).collect();
        self.hidden = s.snapshot.hidden;
        self.info = Some(s.snapshot.info);
        self.last_head = Some(s.head);
        self.pgss = Some(s.pgss);
        self.last_at_ms = Some(at_ms);
        let changed = deltas.len() as u32;
        if !baseline {
            self.entries += deltas.len();
            self.ticks.push_back(Tick { at_ms, dealloc_delta, deltas });
        }
        self.evict(at_ms);
        let (coverage_s, ticks) = self
            .window((self.max_age_ms / 1000) as u32)
            .map_or((0, 0), |w| (w.coverage_s, w.ticks));
        out.push(StatementsEvent::Tick { at_ms, coverage_s, ticks, changed });
        out
    }

    /// Старое вытесняется целыми тиками: по возрасту и по пределу записей.
    fn evict(&mut self, now_ms: i64) {
        while let Some(t) = self.ticks.front()
            && (self.entries > self.max_entries || now_ms - t.at_ms > self.max_age_ms)
        {
            let t = self.ticks.pop_front().expect("front checked");
            self.entries -= t.deltas.len();
        }
        self.gaps.retain(|(_, to)| now_ms - to <= self.max_age_ms);
    }

    /// Сумма дельт по тикам в `(последний − window_s, последний]`; `None` — тиков с дельтами в окне нет.
    /// Покрытие — от первого тика окна до последнего плюс минута первого тика, не больше окна.
    pub fn window(&self, window_s: u32) -> Option<WindowSum> {
        let until_ms = self.last_at_ms?;
        let since_ms = until_ms - i64::from(window_s) * 1000;
        let mut sums: HashMap<QueryKey, Counters> = HashMap::new();
        let (mut ticks, mut first_at, mut dealloc_grew) = (0u32, None, false);
        for t in self.ticks.iter().filter(|t| t.at_ms > since_ms) {
            ticks += 1;
            first_at.get_or_insert(t.at_ms);
            dealloc_grew |= t.dealloc_delta > 0;
            for (k, d) in &t.deltas {
                sums.entry(*k).or_default().add(d);
            }
        }
        let first_at = first_at?;
        let span_s = ((until_ms - first_at) / 1000) as u32 + INTERVAL.as_secs() as u32;
        Some(WindowSum {
            since_ms,
            until_ms,
            ticks,
            coverage_s: span_s.min(window_s),
            dealloc_grew,
            sums,
        })
    }

    pub fn names(&self, key: &QueryKey) -> (Option<String>, Option<String>) {
        self.latest
            .get(key)
            .map(|c| (c.datname.clone(), c.rolname.clone()))
            .unwrap_or_default()
    }

    /// Накопительные строки запроса в базе (по ролям и `toplevel`).
    pub fn latest_for(&self, query_id: i64, dbid: u32) -> Vec<&Cumulative> {
        let mut v: Vec<&Cumulative> = self
            .latest
            .values()
            .filter(|c| c.key.query_id == query_id && c.key.dbid == dbid)
            .collect();
        v.sort_by_key(|c| (c.key.userid, !c.key.toplevel));
        v
    }

    /// История по тикам с `from_ms`: сумма дельт ключей `keys` в каждом тике, где они были.
    pub fn history(&self, keys: &[QueryKey], from_ms: i64) -> Vec<HistoryPoint> {
        self.ticks
            .iter()
            .filter(|t| t.at_ms >= from_ms)
            .filter_map(|t| {
                let mut sum = Counters::default();
                let mut any = false;
                for (k, d) in &t.deltas {
                    if keys.contains(k) {
                        sum.add(d);
                        any = true;
                    }
                }
                any.then(|| HistoryPoint {
                    at_ms: t.at_ms,
                    calls: sum.calls,
                    total_exec_ms: sum.total_exec_ms,
                    mean_exec_ms: if sum.calls > 0 { sum.total_exec_ms / sum.calls as f64 } else { 0.0 },
                })
            })
            .collect()
    }

    pub fn gaps(&self, from_ms: i64) -> Vec<(i64, i64)> {
        self.gaps.iter().copied().filter(|(_, to)| *to >= from_ms).collect()
    }

    pub fn last_head(&self) -> Option<Head> {
        self.last_head
    }

    pub fn info(&self) -> Option<PgssInfo> {
        self.info
    }

    pub fn pgss(&self) -> Option<&PgssInstalled> {
        self.pgss.as_ref()
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    pub fn text(&self, key: &QueryKey) -> Option<(Arc<str>, bool)> {
        self.texts.get(key).cloned()
    }

    pub fn remember_texts(&mut self, items: impl IntoIterator<Item = (QueryKey, String, bool)>) {
        let items: Vec<(QueryKey, String, bool)> = items.into_iter().collect();
        if self.texts.len() + items.len() > MAX_TEXTS {
            self.texts.clear();
        }
        for (k, t, truncated) in items {
            self.texts.insert(k, (Arc::from(t.as_str()), truncated));
        }
    }
}

/// Вид сэмплера pgss для реестра.
pub struct Statements;

impl Sampling for Statements {
    type Sample = StatementsSample;
    type Buffer = StatementsBuffer;
    type Event = StatementsEvent;

    fn interval(&self) -> Duration {
        INTERVAL
    }

    fn push(buffer: &mut StatementsBuffer, at_ms: i64, sample: StatementsSample) -> Vec<StatementsEvent> {
        buffer.push(at_ms, sample)
    }

    fn error_event(error: AppError) -> StatementsEvent {
        StatementsEvent::Error { error }
    }

    fn stopped_event() -> StatementsEvent {
        StatementsEvent::Stopped
    }

    /// Расширение исчезло или слишком старое — остановка, а не повторы с паузой.
    fn fatal(error: &AppError) -> bool {
        matches!(error.code, ErrorCode::AuthFailed | ErrorCode::FeatureUnavailable)
    }
}

pub type StatementsRegistry = SamplerRegistry<Statements>;

pub fn pgss_missing() -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.queries.pgssMissing",
        "pg_stat_statements is not installed in this database"
    )
}

pub fn pgss_too_old(version: &str) -> AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.queries.pgssTooOld",
        "pg_stat_statements {{version}} is older than 1.8",
        version = version
    )
}

/// Сэмпл pgss узла через пул `Sampler`: шапка, расширение (ищется один раз), строки без текстов.
pub struct PoolStatementsSource {
    st: AppState,
    connection_id: String,
    member: String,
    pgss: OnceLock<PgssInstalled>,
}

impl PoolStatementsSource {
    pub fn new(st: AppState, connection_id: &str, member: &str) -> Self {
        Self {
            st,
            connection_id: connection_id.to_string(),
            member: member.to_string(),
            pgss: OnceLock::new(),
        }
    }

    /// Предел получения соединения — как у каталога (таймаут подключения профиля + 10 с).
    fn acquire_limit(&self) -> Duration {
        self.st
            .runtimes
            .target(&self.connection_id, Some(&self.member))
            .map(|(_, profile)| catalog_limit(&profile))
            .unwrap_or(SAMPLE_TIMEOUT)
    }
}

impl SampleSource<StatementsSample> for PoolStatementsSource {
    fn sample<'a>(&'a self) -> BoxFuture<'a, AppResult<StatementsSample>> {
        Box::pin(async move {
            let known = self.pgss.get().cloned();
            let (sample, pgss) = on_catalog_node_split(
                self.acquire_limit(),
                SAMPLE_TIMEOUT,
                self.st.client_with(
                    &self.connection_id,
                    Some(&self.member),
                    None,
                    PoolPurpose::Sampler,
                ),
                move |c| {
                    Box::pin(async move {
                        let head = head(c).await?;
                        let pgss = match known {
                            Some(p) => p,
                            None => match detect(c).await? {
                                Detect::Installed(p) => p,
                                Detect::Missing => return Err(pgss_missing()),
                                Detect::TooOld(v) => return Err(pgss_too_old(&v)),
                            },
                        };
                        let snapshot = read_cumulative(c, &pgss).await?;
                        Ok((StatementsSample { head, pgss: pgss.clone(), snapshot }, pgss))
                    })
                },
            )
            .await?;
            let _ = self.pgss.set(pgss);
            Ok(sample)
        })
    }
}

/// Подписка на сэмплер pgss узла (`member = None` — «за лидером» на момент подписки); id сессии — для `stop_session`.
pub fn start(
    st: &AppState,
    id: &str,
    member: Option<&str>,
    sink: Arc<dyn SamplerSink<StatementsEvent>>,
) -> AppResult<String> {
    crate::sampler::start_with(st, id, member, sink, &st.statements, SessionKind::Statements, |st, id, m| {
        Arc::new(PoolStatementsSource::new(st.clone(), id, m))
    })
}
```

- [ ] **Step 4: Реестр в состоянии, сессии, команда, режим окна и кеш**

`src-tauri/src/sampler/mod.rs` — `pub mod statements;` (шапка модуля уже упоминает pgss, задача 5).

`src-tauri/src/session.rs` — в `enum SessionKind` добавить вариант `Statements,` (после `Sampler`).

`src-tauri/src/state.rs`: `use crate::sampler::statements::{Statements, StatementsRegistry};`; поле

```rust
    /// Сэмплеры `pg_stat_statements` узлов и их буферы дельт (P3).
    pub statements: Arc<StatementsRegistry>,
```

в `new_with_logs` после `samplers`: `let statements = SamplerRegistry::new(Statements, sessions.clone());` и `runtimes.add_close_listener(statements.clone());` **до** `runtimes.add_close_listener(sessions.clone());` (тот же порядок, что у активности); в литерале `Self { … }` — `statements,`.

`src-tauri/src/commands/queries.rs` — добавить:

```rust
use std::sync::Arc;

use tauri::ipc::Channel;

use crate::sampler::registry::SamplerSink;
use crate::sampler::statements::StatementsEvent;

struct StatementsChannelSink(Channel<StatementsEvent>);

impl SamplerSink<StatementsEvent> for StatementsChannelSink {
    fn send(&self, e: StatementsEvent) -> bool {
        self.0.send(e).is_ok()
    }
}

/// Подписка вкладки на сэмплер pg_stat_statements узла; итог — id сессии для `stop_session`.
#[tauri::command]
pub async fn start_statements_sampler(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
    channel: Channel<StatementsEvent>,
) -> AppResult<String> {
    crate::sampler::statements::start(&state, &id, member.as_deref(), Arc::new(StatementsChannelSink(channel)))
}
```

`src-tauri/src/lib.rs` — `commands::queries::start_statements_sampler,`.

`src-tauri/src/diag/queries.rs` — заменить `report_window` и `cached_text`, расширить `texts`:

```rust
/// Срез буфера под блокировкой: всё, что нужно отчёту, копируется наружу.
struct WindowSlice {
    pgss: PgssInstalled,
    info: PgssInfo,
    head: Option<Head>,
    hidden: bool,
    window: Option<crate::sampler::statements::WindowSum>,
    names: std::collections::HashMap<QueryKey, (Option<String>, Option<String>)>,
}

fn window_slice(st: &AppState, id: &str, member: &str, window_s: u32) -> Option<WindowSlice> {
    let buffer = st.statements.buffer(id, member)?;
    let b = buffer.lock_safe();
    let pgss = b.pgss()?.clone();
    let window = b.window(window_s);
    let names = window
        .as_ref()
        .map(|w| w.sums.keys().map(|k| (*k, b.names(k))).collect())
        .unwrap_or_default();
    Some(WindowSlice {
        pgss,
        info: b.info().unwrap_or_default(),
        head: b.last_head(),
        hidden: b.hidden(),
        window,
        names,
    })
}

/// Режим окна — из буфера сэмплера pgss, без обращения к серверу (спец. §4.2). Буфера или тиков с дельтами ещё
/// нет — пустой отчёт с пометкой «сэмплер запускается».
async fn report_window(st: &AppState, id: &str, target: &DiagTarget, p: &QueriesParams, window_s: u32) -> AppResult<DiagReport> {
    let member = member_name(target)?;
    let slice = window_slice(st, id, &member, window_s);
    let head_of = |h: Option<Head>| async move {
        match h {
            Some(h) => Ok(h),
            None => on_member(st, id, target, |c| Box::pin(head(c))).await,
        }
    };
    let Some(WindowSlice { pgss, info, head: h, hidden, window: Some(w), names }) = slice else {
        let h = head_of(slice.as_ref().and_then(|s| s.head)).await?;
        return Ok(make_report(
            Tool::Queries,
            target,
            Some(&h),
            DiagData::Queries(QueriesData::empty(p.mode.clone())),
            Vec::new(),
            vec![window_starting()],
        ));
    };
    if !p.sort.available(&pgss.caps, true) {
        return Err(sort_unavailable());
    }
    let h = head_of(h).await?;
    let kept: Vec<(QueryKey, Counters, Option<String>, Option<String>)> = w
        .sums
        .iter()
        .map(|(k, c)| {
            let (datname, rolname) = names.get(k).cloned().unwrap_or_default();
            (*k, *c, datname, rolname)
        })
        .filter(|(k, _, datname, _)| {
            (!p.toplevel_only || k.toplevel)
                && p.database.as_deref().is_none_or(|d| datname.as_deref() == Some(d))
        })
        .collect();
    let t = totals(kept.iter().map(|(_, c, _, _)| c));
    let mut rows: Vec<QueryStat> = kept
        .into_iter()
        .map(|(k, c, datname, rolname)| stat(k, datname, rolname, &c, None, &t))
        .collect();
    top(&mut rows, p.sort, p.limit);
    attach_texts(&mut rows, |k| cached_text(st, id, &member, k));
    let mut unavailable = Vec::new();
    if hidden {
        unavailable.push(hidden_rows());
    }
    let state = PgssState {
        track_planning: info.track_planning,
        dealloc: info.dealloc,
        dealloc_grew: w.dealloc_grew,
        max: info.max,
    };
    let findings = rules::evaluate(Some(&member), &rows, &t, &state, Mode::Window);
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(QueriesData {
            mode: p.mode.clone(),
            coverage_s: Some(w.coverage_s),
            ticks: Some(w.ticks),
            pgss: Some(PgssMeta::of(&pgss, &info)),
            totals: t,
            rows,
        }),
        findings,
        unavailable,
    ))
}

/// Текст из кеша узла (буфер сэмплера pgss, спец. §4.6).
pub fn cached_text(st: &AppState, id: &str, member: &str, key: &QueryKey) -> Option<(Arc<str>, bool)> {
    st.statements.buffer(id, member).and_then(|b| b.lock_safe().text(key))
}
```

(добавить `use crate::sync::MutexExt;` и `use crate::diag::Head;` в импорты). В `texts` после получения `rows` — запомнить в кеше узла:

```rust
    if let Some(m) = target.member.as_ref().map(|m| m.name.clone())
        && let Some(b) = st.statements.buffer(id, &m)
    {
        b.lock_safe().remember_texts(
            rows.iter()
                .filter_map(|t| t.text.clone().map(|text| (t.key, text, t.truncated))),
        );
    }
    Ok(rows)
```

Словари: `errors.queries.pgssMissing`: en `"pg_stat_statements is not installed in this database"`, ru `"pg_stat_statements не установлен в этой базе"`; `errors.queries.pgssTooOld`: en `"pg_stat_statements {{version}} is older than 1.8"`, ru `"pg_stat_statements {{version}} старше 1.8"`.

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS (7 новых тестов буфера).

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_queries statements_sampler -- --test-threads=1`
Expected: PASS за ~70–90 с (ожидание второго тика).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src src-tauri/tests src/locales
git commit -m "feat(queries): сэмплер pg_stat_statements — буфер дельт на 60 мин, режим «за окно» из буфера, кеш текстов узла

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Карточка запроса в Rust — `query_detail`

**Files:**
- Create: `src-tauri/src/diag/query_detail.rs`
- Modify: `src-tauri/src/diag/mod.rs` (`pub mod query_detail;`), `src-tauri/src/commands/queries.rs` (`query_detail`), `src-tauri/src/lib.rs`, `src-tauri/tests/it_queries.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты в `query_detail.rs`; `it_queries.rs`

**Interfaces:**
- Consumes: `diag::queries::{detect, Detect, statements_sql, row_from, info_sql, PgssInfo}`, `diag::sessions::QUERY_LIMIT`, `diag::sql::BACKEND_START_US`, `diag::caps::{Caps, V14}`, `sampler::statements::{HistoryPoint, WindowSum}`, `pg_advisor::ash::Sample`, `pg_advisor::queries::{Cumulative, Counters, QueryKey, QueryStat, Totals, stat, totals}`, `rules::{evaluate, Mode, PgssState}`.
- Produces (`diag::query_detail`): `RunningInstance { pid, backend_start_us, state, started_ms, duration_s, wait_event_type, wait_event, usename, application_name, client_addr, query, truncated }`; `WaitItem { event_type, event, count, share_pct }`; `QueryWaits { source: "sampler", since_ms, total, items }`; `ByUser { rolname, toplevel, stat }`; `WindowStat { window_s, stat: Option<QueryStat> }`; `Gap { from_ms, to_ms }`; `QueryDetail { text, text_truncated, since_reset, windows: Vec<WindowStat>, totals, by_user, history: Vec<HistoryPoint>, gaps, running, waits, findings, unavailable, server_version_num, in_recovery, pgss_version }`; `detail(st, id, member, database, query_id) -> AppResult<QueryDetail>`; чистые `aggregate(&[Cumulative]) -> Option<(Counters, (f64, f64, f64))>`, `window_stat(&WindowSum, &[QueryKey]) -> Option<QueryStat>`, `waits_of(&[Sample], query_id, since_ms) -> Option<QueryWaits>`, `running_sql()`. Команда `query_detail(id, member, database, queryId: string) -> QueryDetail`.

Уточнение: окна в карточке — список `windows: [{ windowS: 300 | 900 | 3600, stat }]`, а не словарь (спец. §7.2 называла `Record`): serde и типы проще, порядок фиксирован.

- [ ] **Step 1: Падающие тесты**

Unit-тесты `query_detail.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::ash::Sample;
    use pg_advisor::queries::Counters;

    fn key(id: i64, userid: u32, toplevel: bool) -> QueryKey {
        QueryKey { query_id: id, dbid: 1, userid, toplevel }
    }

    fn row(userid: u32, toplevel: bool, calls: i64, ms: f64, min: f64, max: f64, sd: f64) -> Cumulative {
        Cumulative {
            key: key(7, userid, toplevel),
            datname: Some("perf".into()),
            rolname: Some(format!("r{userid}")),
            counters: Counters { calls, total_exec_ms: ms, ..Counters::default() },
            min_exec_ms: min,
            max_exec_ms: max,
            stddev_exec_ms: sd,
        }
    }

    #[test]
    fn aggregate_sums_counters_and_merges_minmax() {
        assert!(aggregate(&[]).is_none());
        let (c, (min, max, sd)) = aggregate(&[row(1, true, 10, 100.0, 1.0, 50.0, 5.0), row(2, false, 30, 60.0, 0.5, 20.0, 9.0)]).unwrap();
        assert_eq!((c.calls, c.total_exec_ms), (40, 160.0));
        assert_eq!((min, max), (0.5, 50.0));
        assert_eq!(sd, 9.0, "stddev — у строки с наибольшим числом вызовов");
    }

    #[test]
    fn window_stat_sums_only_the_query_keys() {
        let mut sums = std::collections::HashMap::new();
        sums.insert(key(7, 1, true), Counters { calls: 3, total_exec_ms: 30.0, ..Counters::default() });
        sums.insert(key(7, 2, true), Counters { calls: 1, total_exec_ms: 10.0, ..Counters::default() });
        sums.insert(key(8, 1, true), Counters { calls: 100, total_exec_ms: 1000.0, ..Counters::default() });
        let w = crate::sampler::statements::WindowSum { since_ms: 0, until_ms: 60_000, ticks: 1, coverage_s: 60, dealloc_grew: false, sums };
        let s = window_stat(&w, &[key(7, 1, true), key(7, 2, true)]).unwrap();
        assert_eq!((s.calls, s.total_exec_ms, s.mean_exec_ms), (4, 40.0, 10.0));
        assert_eq!(s.stddev_exec_ms, None);
        assert!((s.share_pct - 40.0 / 1040.0 * 100.0).abs() < 1e-9, "доля — от всего окна");
        assert!(window_stat(&w, &[key(9, 1, true)]).is_none());
    }

    #[test]
    fn waits_group_samples_of_the_query() {
        let s = |at_ms: i64, qid: Option<i64>, wt: Option<&'static str>, we: Option<&'static str>| Sample {
            at_ms, pid: 1, leader_pid: None, wait_type: wt, wait_event: we, query_id: qid,
            query: "", application: "", user: "", database: "", client: "",
        };
        let samples = [
            s(10, Some(7), Some("IO"), Some("DataFileRead")),
            s(11, Some(7), Some("IO"), Some("DataFileRead")),
            s(12, Some(7), None, None),
            s(13, Some(8), Some("Lock"), Some("relation")),
            s(5, Some(7), Some("Lock"), Some("relation")),
        ];
        let w = waits_of(&samples, 7, 10).unwrap();
        assert_eq!((w.total, w.since_ms, w.source), (3, 10, WaitsSource::Sampler));
        assert_eq!(w.items[0].event_type, "IO");
        assert_eq!((w.items[0].count, w.items[0].share_pct), (2, 200.0 / 3.0));
        assert_eq!((w.items[1].event_type.as_str(), w.items[1].event, w.items[1].count), ("CPU", None, 1));
        assert!(waits_of(&samples, 99, 0).is_none(), "сэмплов запроса нет — None");
    }

    #[test]
    fn running_sql_filters_by_query_id_and_database() {
        let sql = running_sql();
        assert!(sql.contains("a.query_id = $1") && sql.contains("a.datname = $2") && sql.contains("pg_backend_pid()"), "{sql}");
        assert!(sql.contains("track_activity_query_size"));
    }

    #[test]
    fn detail_json_shape() {
        let d = QueryDetail::unavailable_pgss(170_000, false);
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["windows"], serde_json::json!([{"windowS": 300, "stat": null}, {"windowS": 900, "stat": null}, {"windowS": 3600, "stat": null}]));
        assert_eq!(v["unavailable"][0]["part"], "statements");
        assert_eq!((&v["text"], &v["sinceReset"], &v["waits"]), (&serde_json::Value::Null, &serde_json::Value::Null, &serde_json::Value::Null));
    }
}
```

`src-tauri/tests/it_queries.rs` — добавить:

```rust
use glafira_pg_lib::diag::query_detail;

#[tokio::test]
async fn query_detail_on_perf_and_on_pg13() {
    if let Some(perf) = common::perf_addr() {
        let (_dir, st) = common::open_server_in(&perf, "perf").await;
        let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
        let r = diag::run(&st, "it", request(since_reset(QuerySort::Calls))).await.unwrap();
        let DiagData::Queries(d) = &r.data else { unreachable!() };
        let top = &d.rows[0];
        let detail = query_detail::detail(&st, "it", &member, "perf", top.key.query_id).await.unwrap();
        assert!(detail.text.as_deref().is_some_and(|t| !t.is_empty()));
        let since = detail.since_reset.as_ref().expect("строка pgss есть");
        assert!(since.calls >= top.calls, "карточка суммирует роли и toplevel");
        assert!(since.stddev_exec_ms.is_some());
        assert!(!detail.by_user.is_empty() && detail.totals.calls >= since.calls);
        assert_eq!(detail.windows.iter().map(|w| w.window_s).collect::<Vec<_>>(), [300, 900, 3600]);
        assert!(detail.unavailable.iter().all(|u| u.part != "running"), "PG 17: экземпляры читаются");
        assert!(detail.server_version_num >= 170_000 && !detail.in_recovery);
        assert_eq!(detail.pgss_version.as_deref().map(|v| v.starts_with("1.")), Some(true));
        // Запроса нет в pgss — карточка без текста и статистики, но без ошибки.
        let gone = query_detail::detail(&st, "it", &member, "perf", 1).await.unwrap();
        assert!(gone.text.is_none() && gone.since_reset.is_none() && gone.by_user.is_empty());
    }
    if let Some(pg13) = common::matrix_addrs().into_iter().find(|a| a.ends_with(":5513")) {
        let (_dir, st) = common::open_server(&pg13).await;
        let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
        let detail = query_detail::detail(&st, "it", &member, "postgres", 1).await.unwrap();
        assert!(detail.unavailable.iter().any(|u| u.part == "running"), "{:?}", detail.unavailable);
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib diag::query_detail`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`src-tauri/src/diag/query_detail.rs`:

```rust
//! Карточка запроса (спец. P3 §7.2): накопительные строки pgss по ролям и `toplevel`, окна и история из буфера
//! сэмплера pgss, выполняющиеся экземпляры из `pg_stat_activity` (14+), ожидания из буфера сэмплера активности.

use std::collections::HashMap;

use pg_advisor::ash::Sample;
use pg_advisor::findings::Finding;
use pg_advisor::queries::rules::{self, Mode, PgssState};
use pg_advisor::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat, totals};
use serde::Serialize;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14};
use crate::diag::queries::{Detect, PgssInfo, PgssInstalled, detect, info_sql, missing, row_from, statements_sql, too_old};
use crate::diag::sessions::QUERY_LIMIT;
use crate::diag::sql::BACKEND_START_US;
use crate::diag::{Head, Unavailable, WINDOWS_S, head, on_member};
use crate::error::{AppError, AppResult};
use crate::sampler::statements::{HistoryPoint, WindowSum};
use crate::state::AppState;
use crate::sync::MutexExt;

/// История и ожидания — за последний час.
const LOOKBACK_MS: i64 = 3_600_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningInstance {
    pub pid: i32,
    pub backend_start_us: i64,
    pub state: Option<String>,
    pub started_ms: Option<i64>,
    pub duration_s: Option<f64>,
    pub wait_event_type: Option<String>,
    pub wait_event: Option<String>,
    pub usename: Option<String>,
    pub application_name: Option<String>,
    pub client_addr: Option<String>,
    pub query: String,
    /// Текст упёрся в `track_activity_query_size`: литералы могут быть обрезаны, для плана такой текст не годится.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitsSource {
    Sampler,
    /// P3b: `pg_wait_sampling`.
    PgWaitSampling,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitItem {
    pub event_type: String,
    pub event: Option<String>,
    pub count: u64,
    pub share_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryWaits {
    pub source: WaitsSource,
    pub since_ms: i64,
    pub total: u64,
    pub items: Vec<WaitItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ByUser {
    pub rolname: Option<String>,
    pub toplevel: bool,
    pub stat: QueryStat,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStat {
    pub window_s: u32,
    pub stat: Option<QueryStat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub from_ms: i64,
    pub to_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryDetail {
    pub text: Option<String>,
    pub text_truncated: bool,
    /// Сумма строк pgss запроса в базе; `None` — запроса в pgss уже нет.
    pub since_reset: Option<QueryStat>,
    pub windows: Vec<WindowStat>,
    pub totals: Totals,
    pub by_user: Vec<ByUser>,
    pub history: Vec<HistoryPoint>,
    pub gaps: Vec<Gap>,
    pub running: Vec<RunningInstance>,
    /// `None` — буфера сэмплера активности на узле нет (вкладка «Сессии» не открывалась).
    pub waits: Option<QueryWaits>,
    pub findings: Vec<Finding>,
    pub unavailable: Vec<Unavailable>,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub pgss_version: Option<String>,
}

impl QueryDetail {
    fn empty(h: &Head) -> Self {
        Self {
            text: None,
            text_truncated: false,
            since_reset: None,
            windows: WINDOWS_S.iter().map(|w| WindowStat { window_s: *w, stat: None }).collect(),
            totals: Totals::default(),
            by_user: Vec::new(),
            history: Vec::new(),
            gaps: Vec::new(),
            running: Vec::new(),
            waits: None,
            findings: Vec::new(),
            unavailable: Vec::new(),
            server_version_num: h.version_num,
            in_recovery: h.in_recovery,
            pgss_version: None,
        }
    }

    /// Карточка без pgss: только шапка и пометка.
    pub fn unavailable_pgss(version_num: i32, in_recovery: bool) -> Self {
        let mut d = Self::empty(&Head { version_num, in_recovery, now_ms: 0 });
        d.unavailable.push(missing());
        d
    }
}

/// Сумма строк запроса (по ролям и `toplevel`): счётчики складываются, `min` — наименьший, `max` — наибольший,
/// `stddev` — у строки с наибольшим числом вызовов (объединить стандартные отклонения точно нельзя).
pub fn aggregate(rows: &[Cumulative]) -> Option<(Counters, (f64, f64, f64))> {
    let first = rows.first()?;
    let mut c = Counters::default();
    let (mut min, mut max) = (first.min_exec_ms, first.max_exec_ms);
    let mut busiest = first;
    for r in rows {
        c.add(&r.counters);
        min = min.min(r.min_exec_ms);
        max = max.max(r.max_exec_ms);
        if r.counters.calls > busiest.counters.calls {
            busiest = r;
        }
    }
    Some((c, (min, max, busiest.stddev_exec_ms)))
}

/// Строка окна по ключам запроса; доля — от всего окна. `None` — дельт у этих ключей в окне нет.
pub fn window_stat(w: &WindowSum, keys: &[QueryKey]) -> Option<QueryStat> {
    let mut sum = Counters::default();
    let mut any = false;
    for k in keys {
        if let Some(c) = w.sums.get(k) {
            sum.add(c);
            any = true;
        }
    }
    if !any {
        return None;
    }
    let t = totals(w.sums.values());
    let key = keys[0];
    Some(stat(key, None, None, &sum, None, &t))
}

/// Ожидания запроса из сэмплов активности: группировка по типу и событию; активные без ожидания — `CPU`.
pub fn waits_of(samples: &[Sample<'_>], query_id: i64, since_ms: i64) -> Option<QueryWaits> {
    let mut groups: HashMap<(String, Option<String>), u64> = HashMap::new();
    let mut total = 0u64;
    for s in samples.iter().filter(|s| s.at_ms >= since_ms && s.query_id == Some(query_id)) {
        let key = match s.wait_type {
            Some(t) => (t.to_string(), s.wait_event.map(str::to_string)),
            None => ("CPU".to_string(), None),
        };
        *groups.entry(key).or_insert(0) += 1;
        total += 1;
    }
    if total == 0 {
        return None;
    }
    let mut items: Vec<WaitItem> = groups
        .into_iter()
        .map(|((event_type, event), count)| WaitItem {
            event_type,
            event,
            count,
            share_pct: count as f64 / total as f64 * 100.0,
        })
        .collect();
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.event_type.cmp(&b.event_type)));
    Some(QueryWaits { source: WaitsSource::Sampler, since_ms, total, items })
}

/// Выполняющиеся экземпляры (14+): `$1` — `query_id`, `$2` — база.
pub fn running_sql() -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.state, (extract(epoch FROM a.query_start) * 1000)::bigint, \
         extract(epoch FROM now() - a.query_start)::float8, a.wait_event_type, a.wait_event, a.usename::text, \
         a.application_name, host(a.client_addr), left(a.query, {QUERY_LIMIT}), \
         length(a.query) >= current_setting('track_activity_query_size')::int - 1 \
         FROM pg_stat_activity a \
         WHERE a.query_id = $1 AND a.datname = $2 AND a.pid <> pg_backend_pid() ORDER BY a.query_start"
    )
}

fn text_sql(ns: &str) -> String {
    format!(
        "SELECT left(s.query, {QUERY_LIMIT}), length(s.query) > {QUERY_LIMIT} FROM {ns}.pg_stat_statements(true) s \
         JOIN pg_database d ON d.oid = s.dbid WHERE s.queryid = $1 AND d.datname = $2 LIMIT 1"
    )
}

fn totals_sql(ns: &str) -> String {
    format!(
        "SELECT COALESCE(sum(calls), 0)::bigint, COALESCE(sum(total_exec_time), 0)::float8, COALESCE(sum(rows), 0)::bigint \
         FROM {ns}.pg_stat_statements(false) WHERE queryid IS NOT NULL"
    )
}

/// Что читается с сервера за одно соединение.
struct Fetched {
    head: Head,
    pgss: Option<PgssInstalled>,
    rows: Vec<Cumulative>,
    text: Option<(String, bool)>,
    totals: Totals,
    info: PgssInfo,
    running: Vec<RunningInstance>,
    unavailable: Vec<Unavailable>,
}

async fn fetch(c: &Client, database: &str, query_id: i64) -> AppResult<Fetched> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let h = head(c).await?;
    let caps = Caps::new(h.version_num);
    let mut unavailable = Vec::new();
    let (pgss, rows, text, t, info) = match detect(c).await? {
        Detect::Installed(pgss) => {
            let raw = c
                .query(&statements_sql(&pgss.schema, pgss.caps, " WHERE s.queryid = $1 AND d.datname = $2"), &[&query_id, &database])
                .await
                .map_err(pg)?;
            let rows: Vec<Cumulative> = raw.iter().filter_map(row_from).collect();
            let text = c
                .query_opt(&text_sql(&pgss.schema), &[&query_id, &database])
                .await
                .map_err(pg)?
                .map(|r| (r.get::<_, String>(0), r.get::<_, Option<bool>>(1).unwrap_or(false)));
            let tr = c.query_one(&totals_sql(&pgss.schema), &[]).await.map_err(pg)?;
            let t = Totals { calls: tr.get(0), total_exec_ms: tr.get(1), rows: tr.get(2) };
            let i = c.query_one(&info_sql(&pgss.schema, pgss.caps), &[]).await.map_err(pg)?;
            let info = PgssInfo {
                stats_reset_ms: i.get(0),
                dealloc: i.get(1),
                max: i.get(2),
                track_planning: i.get::<_, Option<String>>(3).as_deref() == Some("on"),
            };
            (Some(pgss), rows, text, t, info)
        }
        Detect::TooOld(v) => {
            unavailable.push(too_old(&v));
            (None, Vec::new(), None, Totals::default(), PgssInfo::default())
        }
        Detect::Missing => {
            unavailable.push(missing());
            (None, Vec::new(), None, Totals::default(), PgssInfo::default())
        }
    };
    let running = if caps.at_least(V14) {
        c.query(&running_sql(), &[&query_id, &database])
            .await
            .map_err(pg)?
            .iter()
            .map(|r| RunningInstance {
                pid: r.get(0),
                backend_start_us: r.get(1),
                state: r.get(2),
                started_ms: r.get(3),
                duration_s: r.get(4),
                wait_event_type: r.get(5),
                wait_event: r.get(6),
                usename: r.get(7),
                application_name: r.get(8),
                client_addr: r.get(9),
                query: r.get::<_, Option<String>>(10).unwrap_or_default(),
                truncated: r.get::<_, Option<bool>>(11).unwrap_or(false),
            })
            .collect()
    } else {
        unavailable.extend(caps.missing(V14, "running"));
        Vec::new()
    };
    Ok(Fetched { head: h, pgss, rows, text, totals: t, info, running, unavailable })
}

pub async fn detail(st: &AppState, id: &str, member: &str, database: &str, query_id: i64) -> AppResult<QueryDetail> {
    let target = st.runtimes.diag_target(id, Some(member), false)?;
    let f = on_member(st, id, &target, |c| Box::pin(fetch(c, database, query_id))).await?;
    let mut d = QueryDetail::empty(&f.head);
    d.unavailable = f.unavailable;
    d.running = f.running;
    d.totals = f.totals;
    d.pgss_version = f.pgss.as_ref().map(|p| p.version.clone());
    if let Some((text, truncated)) = f.text {
        d.text = Some(text);
        d.text_truncated = truncated;
    }
    if let Some((c, minmax)) = aggregate(&f.rows) {
        let key = f.rows[0].key;
        let s = stat(key, f.rows[0].datname.clone(), None, &c, Some(minmax), &f.totals);
        let state = PgssState {
            track_planning: f.info.track_planning,
            dealloc: f.info.dealloc,
            dealloc_grew: false,
            max: f.info.max,
        };
        d.findings = rules::evaluate(Some(member), std::slice::from_ref(&s), &f.totals, &state, Mode::SinceReset)
            .into_iter()
            .filter(|x| x.id != "queries_incomplete")
            .collect();
        d.since_reset = Some(s);
        d.by_user = f
            .rows
            .iter()
            .map(|r| ByUser {
                rolname: r.rolname.clone(),
                toplevel: r.key.toplevel,
                stat: stat(r.key, r.datname.clone(), r.rolname.clone(), &r.counters, Some((r.min_exec_ms, r.max_exec_ms, r.stddev_exec_ms)), &f.totals),
            })
            .collect();
    }
    let keys: Vec<QueryKey> = f.rows.iter().map(|r| r.key).collect();
    let now_ms = crate::topology::model::now_ms();
    if !keys.is_empty()
        && let Some(b) = st.statements.buffer(id, member)
    {
        let b = b.lock_safe();
        d.windows = WINDOWS_S
            .iter()
            .map(|w| WindowStat { window_s: *w, stat: b.window(*w).and_then(|sum| window_stat(&sum, &keys)) })
            .collect();
        d.history = b.history(&keys, now_ms - LOOKBACK_MS);
        d.gaps = b.gaps(now_ms - LOOKBACK_MS).into_iter().map(|(from_ms, to_ms)| Gap { from_ms, to_ms }).collect();
    }
    if let Some(b) = st.samplers.buffer(id, member) {
        let b = b.lock_safe();
        d.waits = waits_of(&b.samples(now_ms - LOOKBACK_MS), query_id, now_ms - LOOKBACK_MS);
    }
    Ok(d)
}

pub fn parse_query_id(s: &str) -> AppResult<i64> {
    s.trim().parse().map_err(|_| {
        app_err!(
            crate::error::ErrorCode::InvalidInput,
            "errors.queries.badQueryId",
            "Query id must be a 64-bit integer"
        )
    })
}

#[allow(dead_code)]
fn _assert_error_type(_: AppError) {}
```

(последние две строки — убрать `_assert_error_type`, он не нужен; `AppError` импортируется только если используется — иначе удалить из `use`.)

`src-tauri/src/diag/mod.rs` — `pub mod query_detail;`.

`src-tauri/src/commands/queries.rs`:

```rust
use crate::diag::query_detail::{self, QueryDetail};

/// Карточка запроса: статистика, окна и история, выполняющиеся экземпляры, ожидания.
#[tauri::command]
pub async fn query_detail(
    state: State<'_, AppState>,
    id: String,
    member: String,
    database: String,
    query_id: String,
) -> AppResult<QueryDetail> {
    let qid = query_detail::parse_query_id(&query_id)?;
    query_detail::detail(&state, &id, &member, &database, qid).await
}
```

`src-tauri/src/lib.rs` — `commands::queries::query_detail,`. Словари: `errors.queries.badQueryId`: en `"Query id must be a 64-bit integer"`, ru `"Идентификатор запроса — 64-битное целое"`; `diag.unavailable.parts.running`: en `"Running instances (query_id)"`, ru `"Выполняющиеся экземпляры (query_id)"`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS.

Run: `GPG_IT_PERF=localhost:5471 GPG_IT_MATRIX=localhost:5513 cargo test --manifest-path src-tauri/Cargo.toml --test it_queries query_detail -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src src-tauri/tests src/locales
git commit -m "feat(queries): карточка запроса — строки pgss по ролям, окна и история из буфера, выполняющиеся экземпляры, ожидания из сэмплов

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: `pg-advisor` — модель плана, парсер JSON, текстовый рендер, фикстуры

**Files:**
- Create: `crates/pg-advisor/src/plan/mod.rs`, `crates/pg-advisor/src/plan/json.rs`, `crates/pg-advisor/src/plan/text_out.rs`, `crates/pg-advisor/tests/plans.rs`, `crates/pg-advisor/tests/fixtures/plans/json/*.json` (с pg-perf)
- Modify: `crates/pg-advisor/src/lib.rs` (`pub mod plan;`)
- Test: unit-тесты в `plan/json.rs` и `plan/text_out.rs`; `tests/plans.rs` по фикстурам

**Interfaces:**
- Produces (`pg_advisor::plan`): `PlanTree { root: PlanNode, planning_ms, execution_ms, triggers: Vec<Trigger>, jit: Option<Jit>, settings: Vec<Setting>, total_ms }`; `PlanNode` (спец. §8.5: `id`, `node_type`, `parent_relationship`, `subplan_name`, `relation: Option<Relation { schema, name, alias }>`, `index_name`, `strategy`, `join_type`, `parallel_aware`, `startup_cost`, `total_cost`, `plan_rows`, `plan_width`, `actual: Option<Actual { startup_ms, total_ms, rows, loops }>`, `never_executed`, `workers_planned`, `workers_launched`, `rows_removed_by_filter`, `rows_removed_by_index_recheck`, `rows_removed_by_join_filter`, `heap_fetches`, `filter`, `index_cond`, `recheck_cond`, `hash_cond`, `join_filter`, `merge_cond`, `sort_key`, `group_key`, `sort_method`, `sort_space_kb`, `sort_space_type`, `hash_batches`, `hash_buckets`, `peak_memory_kb`, `exact_heap_blocks`, `lossy_heap_blocks`, `subplans_removed`, `buffers: Option<Buffers>`, `io_read_ms`, `io_write_ms`, `output`, `extra: Map<String, Value>`, `inclusive_ms`, `exclusive_ms`, `share_pct`, `estimate_ratio`, `hot`, `misestimated`, `children`); `Buffers` (10 счётчиков); `Jit { functions, generation_ms, inlining_ms, optimization_ms, emission_ms, total_ms }`; `Trigger { name, relation, time_ms, calls }`; `Setting { name, value }`; `PlanError { message, line: Option<usize> }`.
  `plan::json::from_json(&Value) -> Result<PlanTree, PlanError>`; `plan::json::from_psql_text(&str) -> Result<Value, PlanError>` (снимает обрамление psql); `plan::relations(&PlanTree) -> Vec<RelationRef { schema, name }>` (без повторов, в порядке обхода); `plan::has_modify_table(&PlanTree) -> bool`; `plan::text_out::to_text(&PlanTree) -> String`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/plan/json.rs` — тесты (внизу файла):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Ручная фикстура: Hash Join над Seq Scan и Index Scan, с ANALYZE и буферами.
    fn fixture() -> serde_json::Value {
        json!([{
          "Plan": {
            "Node Type": "Hash Join", "Parallel Aware": false, "Join Type": "Inner",
            "Startup Cost": 10.0, "Total Cost": 100.0, "Plan Rows": 1000, "Plan Width": 8,
            "Actual Startup Time": 1.0, "Actual Total Time": 50.0, "Actual Rows": 10, "Actual Loops": 1,
            "Output": ["o.id"], "Hash Cond": "(o.customer_id = c.id)",
            "Shared Hit Blocks": 100, "Shared Read Blocks": 20, "Shared Dirtied Blocks": 0, "Shared Written Blocks": 0,
            "Local Hit Blocks": 0, "Local Read Blocks": 0, "Local Dirtied Blocks": 0, "Local Written Blocks": 0,
            "Temp Read Blocks": 0, "Temp Written Blocks": 0,
            "Plans": [
              { "Node Type": "Seq Scan", "Parent Relationship": "Outer", "Parallel Aware": false,
                "Relation Name": "orders", "Schema": "shop", "Alias": "o",
                "Startup Cost": 0.0, "Total Cost": 60.0, "Plan Rows": 500, "Plan Width": 8,
                "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": 5, "Actual Loops": 1,
                "Filter": "(o.status = 'disputed'::text)", "Rows Removed by Filter": 499995,
                "Shared Hit Blocks": 80, "Shared Read Blocks": 20, "Shared Dirtied Blocks": 0, "Shared Written Blocks": 0,
                "Local Hit Blocks": 0, "Local Read Blocks": 0, "Local Dirtied Blocks": 0, "Local Written Blocks": 0,
                "Temp Read Blocks": 0, "Temp Written Blocks": 0, "Mystery Field": 42 },
              { "Node Type": "Hash", "Parent Relationship": "Inner", "Parallel Aware": false,
                "Startup Cost": 5.0, "Total Cost": 5.0, "Plan Rows": 100, "Plan Width": 8,
                "Actual Startup Time": 0.5, "Actual Total Time": 0.5, "Actual Rows": 100, "Actual Loops": 1,
                "Hash Buckets": 1024, "Original Hash Buckets": 1024, "Hash Batches": 1, "Original Hash Batches": 1, "Peak Memory Usage": 12,
                "Plans": [
                  { "Node Type": "Index Scan", "Parent Relationship": "Outer", "Parallel Aware": false,
                    "Scan Direction": "Forward", "Index Name": "customers_pkey", "Relation Name": "customers", "Schema": "shop", "Alias": "c",
                    "Startup Cost": 0.3, "Total Cost": 4.0, "Plan Rows": 100, "Plan Width": 8,
                    "Actual Startup Time": 0.01, "Actual Total Time": 0.4, "Actual Rows": 100, "Actual Loops": 1,
                    "Index Cond": "(c.id < 100)" }
                ] }
            ]
          },
          "Planning Time": 0.5,
          "Triggers": [],
          "JIT": { "Functions": 3, "Options": {"Inlining": false}, "Timing": {"Generation": 1.0, "Inlining": 0.0, "Optimization": 2.0, "Emission": 3.0, "Total": 6.0} },
          "Settings": { "work_mem": "1MB" },
          "Execution Time": 55.0
        }])
    }

    #[test]
    fn parses_nodes_relations_actuals_and_extra() {
        let t = from_json(&fixture()).unwrap();
        assert_eq!((t.planning_ms, t.execution_ms, t.total_ms), (Some(0.5), Some(55.0), Some(55.0)));
        assert_eq!(t.settings, vec![Setting { name: "work_mem".into(), value: "1MB".into() }]);
        assert_eq!(t.jit.as_ref().map(|j| (j.functions, j.total_ms)), Some((3, 6.0)));
        let root = &t.root;
        assert_eq!((root.id, root.node_type.as_str(), root.join_type.as_deref()), (0, "Hash Join", Some("Inner")));
        assert_eq!(root.hash_cond.as_deref(), Some("(o.customer_id = c.id)"));
        assert_eq!(root.buffers.as_ref().map(|b| (b.shared_hit, b.shared_read)), Some((100, 20)));
        let seq = &root.children[0];
        assert_eq!((seq.id, seq.node_type.as_str()), (1, "Seq Scan"));
        assert_eq!(seq.relation.as_ref().map(|r| (r.schema.as_deref(), r.name.as_str(), r.alias.as_deref())), Some((Some("shop"), "orders", Some("o"))));
        assert_eq!((seq.rows_removed_by_filter, seq.filter.as_deref()), (Some(499_995), Some("(o.status = 'disputed'::text)")));
        assert_eq!(seq.extra.get("Mystery Field"), Some(&json!(42)), "нераспознанное поле сохранено");
        assert!(!seq.extra.contains_key("Filter"), "распознанное поле в extra не дублируется");
        let hash = &root.children[1];
        assert_eq!((hash.id, hash.hash_batches, hash.peak_memory_kb), (2, Some(1), Some(12)));
        let idx = &hash.children[0];
        assert_eq!((idx.id, idx.index_name.as_deref(), idx.index_cond.as_deref()), (3, Some("customers_pkey"), Some("(c.id < 100)")));
        assert_eq!(idx.actual.as_ref().map(|a| (a.rows, a.loops)), Some((100.0, 1)));
    }

    #[test]
    fn computes_times_shares_estimates_and_hot_nodes() {
        let t = from_json(&fixture()).unwrap();
        let root = &t.root;
        // Включённое время — actual total × loops; собственное — минус дети; доли — от executionMs.
        assert_eq!(root.inclusive_ms, Some(50.0));
        assert_eq!(root.exclusive_ms, Some(50.0 - 30.0 - 0.5));
        assert!((root.share_pct.unwrap() - (19.5 / 55.0 * 100.0)).abs() < 1e-9);
        let seq = &root.children[0];
        assert_eq!((seq.inclusive_ms, seq.exclusive_ms), (Some(30.0), Some(30.0)));
        // Оценка строк: 500 против факта 5 — ошибка ×100.
        assert_eq!(seq.estimate_ratio, Some(0.01));
        assert!(seq.misestimated && !root.children[1].children[0].misestimated);
        // Горячие — три узла с наибольшим собственным временем: Seq Scan (30), корень (19,5), Index Scan (0,4).
        let hot: Vec<i32> = [root, seq, &root.children[1], &root.children[1].children[0]].iter().map(|n| n.hot as i32).collect();
        assert_eq!(hot, [1, 1, 0, 1]);
        assert!(!seq.never_executed);
    }

    #[test]
    fn plain_explain_without_analyze_uses_costs() {
        let mut v = fixture();
        fn strip(n: &mut serde_json::Value) {
            let o = n.as_object_mut().unwrap();
            for k in ["Actual Startup Time", "Actual Total Time", "Actual Rows", "Actual Loops"] {
                o.remove(k);
            }
            if let Some(ps) = o.get_mut("Plans").and_then(|p| p.as_array_mut()) {
                ps.iter_mut().for_each(strip);
            }
        }
        strip(&mut v[0]["Plan"]);
        v[0].as_object_mut().unwrap().remove("Execution Time");
        let t = from_json(&v).unwrap();
        assert!(t.root.actual.is_none() && t.root.inclusive_ms.is_none() && t.root.share_pct.is_none());
        assert_eq!(t.execution_ms, None);
        // Без факта «горячие» — по стоимости без детей: Seq Scan (60), корень (100 − 60 − 5 = 35), Hash (5 − 4 = 1).
        assert!(t.root.children[0].hot && t.root.hot && t.root.children[1].hot);
        assert!(!t.root.children[1].children[0].hot);
        assert_eq!(t.root.estimate_ratio, None);
    }

    #[test]
    fn accepts_a_bare_object_and_rejects_garbage() {
        let v = fixture();
        assert!(from_json(&v[0]).is_ok(), "объект без массива");
        let e = from_json(&json!({"foo": 1})).unwrap_err();
        assert!(e.message.contains("Plan"), "{e:?}");
        assert!(from_json(&json!([])).is_err());
    }

    #[test]
    fn never_executed_and_loops_are_handled() {
        let v = json!({"Plan": {"Node Type": "Result", "Startup Cost": 0.0, "Total Cost": 0.01, "Plan Rows": 1, "Plan Width": 4,
            "Actual Startup Time": 0.0, "Actual Total Time": 0.0, "Actual Rows": 0, "Actual Loops": 0}});
        let t = from_json(&v).unwrap();
        assert!(t.root.never_executed && t.root.inclusive_ms == Some(0.0));
        let v = json!({"Plan": {"Node Type": "Index Scan", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4,
            "Actual Startup Time": 0.0, "Actual Total Time": 0.02, "Actual Rows": 1, "Actual Loops": 1000}, "Execution Time": 40.0});
        let t = from_json(&v).unwrap();
        assert_eq!(t.root.inclusive_ms, Some(20.0), "0,02 мс × 1000 циклов");
        assert!((t.root.share_pct.unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn psql_framing_is_stripped() {
        let text = "                      QUERY PLAN                       \n\
                    -------------------------------------------------------\n\
                    [                                                     +\n\
                      {                                                   +\n\
                        \"Plan\": {                                        +\n\
                          \"Node Type\": \"Result\",                        +\n\
                          \"Startup Cost\": 0.00,                          +\n\
                          \"Total Cost\": 0.01,                            +\n\
                          \"Plan Rows\": 1,                                +\n\
                          \"Plan Width\": 4                                +\n\
                        }                                                 +\n\
                      }                                                   +\n\
                    ]\n\
                    (1 row)\n";
        let v = from_psql_text(text).unwrap();
        assert_eq!(from_json(&v).unwrap().root.node_type, "Result");
        assert!(from_psql_text("[{\"Plan\": {\"Node Type\": \"Result\", \"Startup Cost\": 0, \"Total Cost\": 0, \"Plan Rows\": 1, \"Plan Width\": 1}}]").is_ok(), "чистый JSON тоже принимается");
        let e = from_psql_text("not json at all").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn relations_and_modify_table() {
        let t = from_json(&fixture()).unwrap();
        let rels: Vec<String> = crate::plan::relations(&t).iter().map(|r| format!("{}.{}", r.schema.as_deref().unwrap_or(""), r.name)).collect();
        assert_eq!(rels, ["shop.orders", "shop.customers"]);
        assert!(!crate::plan::has_modify_table(&t));
        let upd = json!({"Plan": {"Node Type": "ModifyTable", "Operation": "Update", "Relation Name": "balances", "Schema": "shop",
            "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4,
            "Plans": [{"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "balances", "Schema": "shop", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}]}});
        let t = from_json(&upd).unwrap();
        assert!(crate::plan::has_modify_table(&t));
        assert_eq!(crate::plan::relations(&t).len(), 1, "без повторов");
        // ModifyTable внутри CTE (`WITH d AS (DELETE …) SELECT …`) тоже считается записью.
        let cte = json!({"Plan": {"Node Type": "CTE Scan", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4,
            "Plans": [{"Node Type": "ModifyTable", "Operation": "Delete", "Parent Relationship": "InitPlan", "Subplan Name": "CTE d", "Relation Name": "t", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}]}});
        assert!(crate::plan::has_modify_table(&from_json(&cte).unwrap()));
    }
}
```

`crates/pg-advisor/src/plan/text_out.rs` — тест:

```rust
#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use crate::plan::text_out::to_text;
    use serde_json::json;

    #[test]
    fn renders_like_psql() {
        let v = json!([{"Plan": {"Node Type": "Hash Join", "Join Type": "Inner", "Startup Cost": 10.0, "Total Cost": 100.0, "Plan Rows": 1000, "Plan Width": 8,
            "Actual Startup Time": 1.0, "Actual Total Time": 50.0, "Actual Rows": 10, "Actual Loops": 1, "Hash Cond": "(o.customer_id = c.id)",
            "Shared Hit Blocks": 100, "Shared Read Blocks": 20, "Shared Dirtied Blocks": 0, "Shared Written Blocks": 0,
            "Local Hit Blocks": 0, "Local Read Blocks": 0, "Local Dirtied Blocks": 0, "Local Written Blocks": 0, "Temp Read Blocks": 0, "Temp Written Blocks": 0,
            "Plans": [
              {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 60.0, "Plan Rows": 500, "Plan Width": 8,
               "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": 5, "Actual Loops": 1, "Filter": "(o.status = 'disputed'::text)", "Rows Removed by Filter": 499995},
              {"Node Type": "Index Scan", "Parent Relationship": "Inner", "Index Name": "customers_pkey", "Relation Name": "customers", "Schema": "shop", "Alias": "c",
               "Startup Cost": 0.3, "Total Cost": 4.0, "Plan Rows": 100, "Plan Width": 8, "Actual Startup Time": 0.01, "Actual Total Time": 0.4, "Actual Rows": 100, "Actual Loops": 0, "Index Cond": "(c.id < 100)"}
            ]}, "Planning Time": 0.5, "Execution Time": 55.0}]);
        let text = to_text(&from_json(&v).unwrap());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Hash Join  (cost=10.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)");
        assert_eq!(lines[1], "  Hash Cond: (o.customer_id = c.id)");
        assert_eq!(lines[2], "  Buffers: shared hit=100 read=20");
        assert_eq!(lines[3], "  ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)");
        assert_eq!(lines[4], "        Filter: (o.status = 'disputed'::text)");
        assert_eq!(lines[5], "        Rows Removed by Filter: 499995");
        assert_eq!(lines[6], "  ->  Index Scan using customers_pkey on shop.customers c  (cost=0.30..4.00 rows=100 width=8) (never executed)");
        assert_eq!(lines[7], "        Index Cond: (c.id < 100)");
        assert_eq!(lines[8], "Planning Time: 0.500 ms");
        assert_eq!(lines[9], "Execution Time: 55.000 ms");
    }
}
```

`crates/pg-advisor/tests/plans.rs` (фикстуры со стенда — шаг 4):

```rust
//! Планы с pg-perf (PG 17) и матрицы: каждая фикстура `tests/fixtures/plans/json/*.json` разбирается без потерь,
//! вычисляемые поля согласованы. Фикстуры снимаются скриптом из шага 4 задачи 8 плана P3a.

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanNode, relations};

fn walk<'a>(n: &'a PlanNode, out: &mut Vec<&'a PlanNode>) {
    out.push(n);
    for c in &n.children {
        walk(c, out);
    }
}

#[test]
fn every_fixture_parses_consistently() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/json");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("каталог фикстур") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        count += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let t = from_json(&v).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
        let mut nodes = Vec::new();
        walk(&t.root, &mut nodes);
        assert!(nodes.iter().enumerate().all(|(i, n)| n.id == i as u32), "{path:?}: id — порядок обхода");
        assert!(nodes.iter().filter(|n| n.hot).count() <= 3, "{path:?}");
        assert!(nodes.iter().all(|n| n.exclusive_ms.is_none_or(|x| x >= 0.0)), "{path:?}");
        let share: f64 = nodes.iter().filter_map(|n| n.share_pct).sum();
        assert!(share <= 100.5, "{path:?}: сумма долей {share}");
        assert!(nodes.iter().all(|n| n.relation.as_ref().is_none_or(|r| !r.name.is_empty())), "{path:?}");
        if path.file_name().unwrap().to_str().unwrap().contains("analyze") {
            assert!(t.execution_ms.is_some() && t.root.actual.is_some(), "{path:?}");
        }
        if path.file_name().unwrap().to_str().unwrap().contains("partitions") {
            assert!(relations(&t).len() >= 10, "{path:?}: Append по партициям");
        }
    }
    assert!(count >= 10, "фикстур мало: {count}");
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml plan`
Expected: ошибка компиляции (`plan` нет).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/lib.rs` — `pub mod plan;`.

`crates/pg-advisor/src/plan/mod.rs`:

```rust
//! Модель плана выполнения (спец. P3 §8.5): дерево узлов из `EXPLAIN (FORMAT JSON)` (P3b — и из текста),
//! вычисляемые поля (собственное время, доля, ошибка оценки, горячие узлы) и текстовый рендер.

pub mod json;
pub mod text_out;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    pub message: String,
    /// Строка исходного текста (с 1), если известна.
    pub line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relation {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

/// Отношение без псевдонима — для списка таблиц запроса и DDL.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationRef {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actual {
    pub startup_ms: f64,
    pub total_ms: f64,
    /// Строк на цикл — как отдаёт сервер (на 18 дробное).
    pub rows: f64,
    pub loops: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Buffers {
    pub shared_hit: i64,
    pub shared_read: i64,
    pub shared_dirtied: i64,
    pub shared_written: i64,
    pub local_hit: i64,
    pub local_read: i64,
    pub local_dirtied: i64,
    pub local_written: i64,
    pub temp_read: i64,
    pub temp_written: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Jit {
    pub functions: i64,
    pub generation_ms: f64,
    pub inlining_ms: f64,
    pub optimization_ms: f64,
    pub emission_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trigger {
    pub name: String,
    pub relation: Option<String>,
    pub time_ms: f64,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    /// Порядок обхода в глубину, с нуля.
    pub id: u32,
    pub node_type: String,
    pub parent_relationship: Option<String>,
    pub subplan_name: Option<String>,
    pub relation: Option<Relation>,
    pub index_name: Option<String>,
    pub strategy: Option<String>,
    pub join_type: Option<String>,
    pub parallel_aware: bool,
    pub startup_cost: f64,
    pub total_cost: f64,
    pub plan_rows: f64,
    pub plan_width: i64,
    pub actual: Option<Actual>,
    /// `Actual Loops = 0`: узел не выполнялся.
    pub never_executed: bool,
    pub workers_planned: Option<i64>,
    pub workers_launched: Option<i64>,
    pub rows_removed_by_filter: Option<i64>,
    pub rows_removed_by_index_recheck: Option<i64>,
    pub rows_removed_by_join_filter: Option<i64>,
    pub heap_fetches: Option<i64>,
    pub filter: Option<String>,
    pub index_cond: Option<String>,
    pub recheck_cond: Option<String>,
    pub hash_cond: Option<String>,
    pub join_filter: Option<String>,
    pub merge_cond: Option<String>,
    pub sort_key: Vec<String>,
    pub group_key: Vec<String>,
    pub sort_method: Option<String>,
    pub sort_space_kb: Option<i64>,
    pub sort_space_type: Option<String>,
    pub hash_batches: Option<i64>,
    pub hash_buckets: Option<i64>,
    pub peak_memory_kb: Option<i64>,
    pub exact_heap_blocks: Option<i64>,
    pub lossy_heap_blocks: Option<i64>,
    pub subplans_removed: Option<i64>,
    pub buffers: Option<Buffers>,
    pub io_read_ms: Option<f64>,
    pub io_write_ms: Option<f64>,
    pub output: Vec<String>,
    /// Нераспознанные поля узла — как есть.
    pub extra: Map<String, Value>,
    /// `actual.total_ms × loops`; без ANALYZE — `None`.
    pub inclusive_ms: Option<f64>,
    /// Включённое минус включённое детей, не меньше нуля.
    pub exclusive_ms: Option<f64>,
    /// Доля собственного времени в `PlanTree::total_ms`, проценты.
    pub share_pct: Option<f64>,
    /// `actual.rows / plan_rows`; `None` без ANALYZE или при `plan_rows = 0` и `actual.rows = 0`.
    pub estimate_ratio: Option<f64>,
    /// Три узла с наибольшим собственным временем (без ANALYZE — по стоимости без детей).
    pub hot: bool,
    /// Ошибка оценки ×10 и больше при `max(план, факт) ≥ 100` строк.
    pub misestimated: bool,
    pub children: Vec<PlanNode>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanTree {
    pub root: PlanNode,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
    pub triggers: Vec<Trigger>,
    pub jit: Option<Jit>,
    pub settings: Vec<Setting>,
    /// Знаменатель долей: `execution_ms`, иначе включённое время корня; без ANALYZE — `None`.
    pub total_ms: Option<f64>,
}

pub const HOT_NODES: usize = 3;
pub const MISESTIMATE_RATIO: f64 = 10.0;
pub const MISESTIMATE_MIN_ROWS: f64 = 100.0;

impl PlanTree {
    /// Вычисляемые поля после разбора: id, времена, доли, ошибки оценки, горячие узлы.
    pub fn finish(&mut self) {
        let mut next = 0u32;
        number(&mut self.root, &mut next);
        compute_times(&mut self.root);
        self.total_ms = self.execution_ms.or(self.root.inclusive_ms);
        if let Some(total) = self.total_ms.filter(|t| *t > 0.0) {
            shares(&mut self.root, total);
        }
        let with_actual = self.root.actual.is_some();
        let mut weights: Vec<(f64, u32)> = Vec::new();
        collect_weights(&self.root, with_actual, &mut weights);
        weights.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let hot: Vec<u32> = weights.iter().take(HOT_NODES).filter(|(w, _)| *w > 0.0).map(|(_, id)| *id).collect();
        mark_hot(&mut self.root, &hot);
    }
}

fn number(n: &mut PlanNode, next: &mut u32) {
    n.id = *next;
    *next += 1;
    for c in &mut n.children {
        number(c, next);
    }
}

fn compute_times(n: &mut PlanNode) {
    for c in &mut n.children {
        compute_times(c);
    }
    if let Some(a) = n.actual {
        let inclusive = a.total_ms * a.loops as f64;
        let children: f64 = n.children.iter().filter_map(|c| c.inclusive_ms).sum();
        n.inclusive_ms = Some(inclusive);
        n.exclusive_ms = Some((inclusive - children).max(0.0));
        n.estimate_ratio = if n.plan_rows > 0.0 {
            Some(a.rows / n.plan_rows)
        } else if a.rows > 0.0 {
            Some(f64::INFINITY)
        } else {
            None
        };
        n.misestimated = n.estimate_ratio.is_some_and(|r| {
            (r >= MISESTIMATE_RATIO || r <= 1.0 / MISESTIMATE_RATIO) && n.plan_rows.max(a.rows) >= MISESTIMATE_MIN_ROWS
        });
    }
}

fn shares(n: &mut PlanNode, total: f64) {
    n.share_pct = n.exclusive_ms.map(|x| x / total * 100.0);
    for c in &mut n.children {
        shares(c, total);
    }
}

/// Вес узла для «горячих»: собственное время, без ANALYZE — стоимость без детей.
fn collect_weights(n: &PlanNode, with_actual: bool, out: &mut Vec<(f64, u32)>) {
    let w = if with_actual {
        n.exclusive_ms.unwrap_or(0.0)
    } else {
        (n.total_cost - n.children.iter().map(|c| c.total_cost).sum::<f64>()).max(0.0)
    };
    out.push((w, n.id));
    for c in &n.children {
        collect_weights(c, with_actual, out);
    }
}

fn mark_hot(n: &mut PlanNode, hot: &[u32]) {
    n.hot = hot.contains(&n.id);
    for c in &mut n.children {
        mark_hot(c, hot);
    }
}

/// Отношения плана без повторов, в порядке обхода.
pub fn relations(t: &PlanTree) -> Vec<RelationRef> {
    fn walk(n: &PlanNode, out: &mut Vec<RelationRef>) {
        if let Some(r) = &n.relation {
            let rr = RelationRef { schema: r.schema.clone(), name: r.name.clone() };
            if !out.contains(&rr) {
                out.push(rr);
            }
        }
        for c in &n.children {
            walk(c, out);
        }
    }
    let mut out = Vec::new();
    walk(&t.root, &mut out);
    out
}

/// В дереве есть `ModifyTable` (INSERT / UPDATE / DELETE / MERGE, в том числе внутри CTE).
pub fn has_modify_table(t: &PlanTree) -> bool {
    fn walk(n: &PlanNode) -> bool {
        n.node_type == "ModifyTable" || n.children.iter().any(walk)
    }
    walk(&t.root)
}
```

`crates/pg-advisor/src/plan/json.rs`:

```rust
//! Разбор `EXPLAIN (FORMAT JSON)`: массив из одного объекта или сам объект; незнакомые поля узла — в `extra`.

use serde_json::{Map, Value};

use crate::plan::{Actual, Buffers, Jit, PlanError, PlanNode, PlanTree, Relation, Setting, Trigger};

fn err(message: impl Into<String>) -> PlanError {
    PlanError { message: message.into(), line: None }
}

/// Снимает обрамление psql (заголовок `QUERY PLAN`, разделитель, `(N rows)`, переносы `+` и рамки `|`) и разбирает JSON.
pub fn from_psql_text(text: &str) -> Result<Value, PlanError> {
    let mut body = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t == "QUERY PLAN" || t.chars().all(|c| c == '-' || c == '+') {
            continue;
        }
        if t.starts_with('(') && t.ends_with(')') && (t.ends_with(" row)") || t.ends_with(" rows)")) {
            continue;
        }
        let t = t.trim_matches('|').trim();
        let t = t.strip_suffix('+').map(str::trim_end).unwrap_or(t);
        body.push_str(t);
        body.push('\n');
    }
    serde_json::from_str(&body).map_err(|e| PlanError {
        message: e.to_string(),
        line: Some(e.line().max(1)),
    })
}

pub fn from_json(v: &Value) -> Result<PlanTree, PlanError> {
    let top = match v {
        Value::Array(items) => items.first().ok_or_else(|| err("empty plan array"))?,
        other => other,
    };
    let obj = top.as_object().ok_or_else(|| err("plan is not an object"))?;
    let plan = obj.get("Plan").ok_or_else(|| err("no \"Plan\" key"))?;
    let root = node(plan)?;
    let triggers = obj
        .get("Triggers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(Trigger {
                        name: t.get("Trigger Name")?.as_str()?.to_string(),
                        relation: t.get("Relation").and_then(Value::as_str).map(str::to_string),
                        time_ms: t.get("Time").and_then(Value::as_f64).unwrap_or(0.0),
                        calls: t.get("Calls").and_then(Value::as_i64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let jit = obj.get("JIT").and_then(Value::as_object).map(|j| {
        let timing = j.get("Timing").and_then(Value::as_object);
        let t = |k: &str| timing.and_then(|t| t.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
        Jit {
            functions: j.get("Functions").and_then(Value::as_i64).unwrap_or(0),
            generation_ms: t("Generation"),
            inlining_ms: t("Inlining"),
            optimization_ms: t("Optimization"),
            emission_ms: t("Emission"),
            total_ms: t("Total"),
        }
    });
    let settings = obj
        .get("Settings")
        .and_then(Value::as_object)
        .map(|s| {
            s.iter()
                .map(|(k, v)| Setting {
                    name: k.clone(),
                    value: v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    let mut tree = PlanTree {
        root,
        planning_ms: obj.get("Planning Time").and_then(Value::as_f64),
        execution_ms: obj.get("Execution Time").and_then(Value::as_f64),
        triggers,
        jit,
        settings,
        total_ms: None,
    };
    tree.finish();
    Ok(tree)
}

fn take_str(m: &mut Map<String, Value>, k: &str) -> Option<String> {
    m.remove(k).and_then(|v| v.as_str().map(str::to_string))
}

fn take_f64(m: &mut Map<String, Value>, k: &str) -> Option<f64> {
    m.remove(k).and_then(|v| v.as_f64())
}

fn take_i64(m: &mut Map<String, Value>, k: &str) -> Option<i64> {
    m.remove(k).and_then(|v| v.as_i64())
}

fn take_bool(m: &mut Map<String, Value>, k: &str) -> Option<bool> {
    m.remove(k).and_then(|v| v.as_bool())
}

/// Строка или массив строк (`Sort Key`, `Output`, `Group Key`).
fn take_strs(m: &mut Map<String, Value>, k: &str) -> Vec<String> {
    match m.remove(k) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    }
}

fn node(v: &Value) -> Result<PlanNode, PlanError> {
    let mut m = v.as_object().ok_or_else(|| err("plan node is not an object"))?.clone();
    let node_type = take_str(&mut m, "Node Type").ok_or_else(|| err("node without \"Node Type\""))?;
    let children = match m.remove("Plans") {
        Some(Value::Array(ps)) => ps.iter().map(node).collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    let relation = take_str(&mut m, "Relation Name").map(|name| Relation {
        schema: take_str(&mut m, "Schema"),
        name,
        alias: take_str(&mut m, "Alias"),
    });
    let actual = match (take_f64(&mut m, "Actual Total Time"), take_f64(&mut m, "Actual Rows"), take_i64(&mut m, "Actual Loops")) {
        (Some(total_ms), Some(rows), Some(loops)) => Some(Actual {
            startup_ms: take_f64(&mut m, "Actual Startup Time").unwrap_or(0.0),
            total_ms,
            rows,
            loops: loops.max(0) as u64,
        }),
        // Без TIMING времени нет, но строки и циклы есть.
        (None, Some(rows), Some(loops)) => Some(Actual { startup_ms: 0.0, total_ms: 0.0, rows, loops: loops.max(0) as u64 }),
        _ => None,
    };
    let never_executed = actual.is_some_and(|a| a.loops == 0);
    let has_buffers = m.contains_key("Shared Hit Blocks");
    let buffers = has_buffers.then(|| Buffers {
        shared_hit: take_i64(&mut m, "Shared Hit Blocks").unwrap_or(0),
        shared_read: take_i64(&mut m, "Shared Read Blocks").unwrap_or(0),
        shared_dirtied: take_i64(&mut m, "Shared Dirtied Blocks").unwrap_or(0),
        shared_written: take_i64(&mut m, "Shared Written Blocks").unwrap_or(0),
        local_hit: take_i64(&mut m, "Local Hit Blocks").unwrap_or(0),
        local_read: take_i64(&mut m, "Local Read Blocks").unwrap_or(0),
        local_dirtied: take_i64(&mut m, "Local Dirtied Blocks").unwrap_or(0),
        local_written: take_i64(&mut m, "Local Written Blocks").unwrap_or(0),
        temp_read: take_i64(&mut m, "Temp Read Blocks").unwrap_or(0),
        temp_written: take_i64(&mut m, "Temp Written Blocks").unwrap_or(0),
    });
    // До 17 — `I/O Read Time`, с 17 — `Shared I/O Read Time` (локальные и temp — отдельно, остаются в extra).
    let io_read_ms = take_f64(&mut m, "I/O Read Time").or_else(|| take_f64(&mut m, "Shared I/O Read Time"));
    let io_write_ms = take_f64(&mut m, "I/O Write Time").or_else(|| take_f64(&mut m, "Shared I/O Write Time"));
    Ok(PlanNode {
        id: 0,
        node_type,
        parent_relationship: take_str(&mut m, "Parent Relationship"),
        subplan_name: take_str(&mut m, "Subplan Name"),
        relation,
        index_name: take_str(&mut m, "Index Name"),
        strategy: take_str(&mut m, "Strategy"),
        join_type: take_str(&mut m, "Join Type"),
        parallel_aware: take_bool(&mut m, "Parallel Aware").unwrap_or(false),
        startup_cost: take_f64(&mut m, "Startup Cost").unwrap_or(0.0),
        total_cost: take_f64(&mut m, "Total Cost").unwrap_or(0.0),
        plan_rows: take_f64(&mut m, "Plan Rows").unwrap_or(0.0),
        plan_width: take_i64(&mut m, "Plan Width").unwrap_or(0),
        actual,
        never_executed,
        workers_planned: take_i64(&mut m, "Workers Planned"),
        workers_launched: take_i64(&mut m, "Workers Launched"),
        rows_removed_by_filter: take_i64(&mut m, "Rows Removed by Filter"),
        rows_removed_by_index_recheck: take_i64(&mut m, "Rows Removed by Index Recheck"),
        rows_removed_by_join_filter: take_i64(&mut m, "Rows Removed by Join Filter"),
        heap_fetches: take_i64(&mut m, "Heap Fetches"),
        filter: take_str(&mut m, "Filter"),
        index_cond: take_str(&mut m, "Index Cond"),
        recheck_cond: take_str(&mut m, "Recheck Cond"),
        hash_cond: take_str(&mut m, "Hash Cond"),
        join_filter: take_str(&mut m, "Join Filter"),
        merge_cond: take_str(&mut m, "Merge Cond"),
        sort_key: take_strs(&mut m, "Sort Key"),
        group_key: take_strs(&mut m, "Group Key"),
        sort_method: take_str(&mut m, "Sort Method"),
        sort_space_kb: take_i64(&mut m, "Sort Space Used"),
        sort_space_type: take_str(&mut m, "Sort Space Type"),
        hash_batches: take_i64(&mut m, "Hash Batches"),
        hash_buckets: take_i64(&mut m, "Hash Buckets"),
        peak_memory_kb: take_i64(&mut m, "Peak Memory Usage"),
        exact_heap_blocks: take_i64(&mut m, "Exact Heap Blocks"),
        lossy_heap_blocks: take_i64(&mut m, "Lossy Heap Blocks"),
        subplans_removed: take_i64(&mut m, "Subplans Removed"),
        buffers,
        io_read_ms,
        io_write_ms,
        output: take_strs(&mut m, "Output"),
        extra: m,
        inclusive_ms: None,
        exclusive_ms: None,
        share_pct: None,
        estimate_ratio: None,
        hot: false,
        misestimated: false,
        children,
    })
}
```

`crates/pg-advisor/src/plan/text_out.rs`:

```rust
//! Текстовый рендер дерева в стиле psql — для «Скопировать для LLM» и кнопки «Как текст».

use std::fmt::Write;

use crate::plan::{PlanNode, PlanTree};

pub fn to_text(t: &PlanTree) -> String {
    let mut out = String::new();
    node(&t.root, 0, &mut out);
    if let Some(p) = t.planning_ms {
        let _ = writeln!(out, "Planning Time: {p:.3} ms");
    }
    for tr in &t.triggers {
        let _ = writeln!(out, "Trigger {}: time={:.3} calls={}", tr.name, tr.time_ms, tr.calls);
    }
    if let Some(j) = &t.jit {
        let _ = writeln!(out, "JIT:");
        let _ = writeln!(out, "  Functions: {}", j.functions);
        let _ = writeln!(
            out,
            "  Timing: Generation {:.3} ms, Inlining {:.3} ms, Optimization {:.3} ms, Emission {:.3} ms, Total {:.3} ms",
            j.generation_ms, j.inlining_ms, j.optimization_ms, j.emission_ms, j.total_ms
        );
    }
    if let Some(e) = t.execution_ms {
        let _ = writeln!(out, "Execution Time: {e:.3} ms");
    }
    out
}

fn head(n: &PlanNode) -> String {
    let mut s = n.node_type.clone();
    if let Some(st) = &n.strategy
        && st != "Plain"
    {
        s = format!("{st}{s}");
    }
    if let Some(i) = &n.index_name {
        let _ = write!(s, " using {i}");
    }
    if let Some(r) = &n.relation {
        let _ = write!(s, " on ");
        if let Some(sc) = &r.schema {
            let _ = write!(s, "{sc}.");
        }
        let _ = write!(s, "{}", r.name);
        if let Some(a) = &r.alias
            && *a != r.name
        {
            let _ = write!(s, " {a}");
        }
    }
    let _ = write!(
        s,
        "  (cost={:.2}..{:.2} rows={} width={})",
        n.startup_cost, n.total_cost, n.plan_rows.round() as i64, n.plan_width
    );
    if let Some(a) = n.actual {
        if n.never_executed {
            s.push_str(" (never executed)");
        } else {
            let _ = write!(
                s,
                " (actual time={:.3}..{:.3} rows={} loops={})",
                a.startup_ms, a.total_ms, a.rows.round() as i64, a.loops
            );
        }
    }
    s
}

fn node(n: &PlanNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth * 3);
    let (first, rest) = if depth == 0 {
        (String::new(), "  ".to_string())
    } else {
        (format!("{}  ->  ", "  ".repeat(depth * 3 - 3)), format!("{indent}  "))
    };
    let _ = writeln!(out, "{first}{}", head(n));
    let mut line = |label: &str, v: &str| {
        let _ = writeln!(out, "{rest}{label}: {v}");
    };
    if let Some(name) = &n.subplan_name {
        line("Subplan Name", name);
    }
    if let Some(v) = &n.index_cond {
        line("Index Cond", v);
    }
    if let Some(v) = &n.recheck_cond {
        line("Recheck Cond", v);
    }
    if let Some(v) = &n.hash_cond {
        line("Hash Cond", v);
    }
    if let Some(v) = &n.merge_cond {
        line("Merge Cond", v);
    }
    if let Some(v) = &n.join_filter {
        line("Join Filter", v);
    }
    if let Some(v) = &n.filter {
        line("Filter", v);
    }
    if let Some(v) = n.rows_removed_by_filter {
        line("Rows Removed by Filter", &v.to_string());
    }
    if let Some(v) = n.rows_removed_by_index_recheck {
        line("Rows Removed by Index Recheck", &v.to_string());
    }
    if let Some(v) = n.rows_removed_by_join_filter {
        line("Rows Removed by Join Filter", &v.to_string());
    }
    if !n.sort_key.is_empty() {
        line("Sort Key", &n.sort_key.join(", "));
    }
    if !n.group_key.is_empty() {
        line("Group Key", &n.group_key.join(", "));
    }
    if let Some(m) = &n.sort_method {
        let space = match (n.sort_space_type.as_deref(), n.sort_space_kb) {
            (Some(t), Some(kb)) => format!("  {t}: {kb}kB"),
            _ => String::new(),
        };
        line("Sort Method", &format!("{m}{space}"));
    }
    if let Some(b) = n.hash_batches {
        line(
            "Buckets",
            &format!("{}  Batches: {}  Memory Usage: {}kB", n.hash_buckets.unwrap_or(0), b, n.peak_memory_kb.unwrap_or(0)),
        );
    }
    if n.exact_heap_blocks.is_some() || n.lossy_heap_blocks.is_some() {
        line("Heap Blocks", &format!("exact={} lossy={}", n.exact_heap_blocks.unwrap_or(0), n.lossy_heap_blocks.unwrap_or(0)));
    }
    if let Some(h) = n.heap_fetches {
        line("Heap Fetches", &h.to_string());
    }
    if let (Some(p), Some(l)) = (n.workers_planned, n.workers_launched) {
        line("Workers Planned", &p.to_string());
        line("Workers Launched", &l.to_string());
    }
    if let Some(s) = n.subplans_removed {
        line("Subplans Removed", &s.to_string());
    }
    if let Some(b) = &n.buffers {
        let mut parts = Vec::new();
        let mut group = |name: &str, hit: i64, read: i64, dirtied: i64, written: i64| {
            let mut g = Vec::new();
            if hit > 0 { g.push(format!("hit={hit}")); }
            if read > 0 { g.push(format!("read={read}")); }
            if dirtied > 0 { g.push(format!("dirtied={dirtied}")); }
            if written > 0 { g.push(format!("written={written}")); }
            if !g.is_empty() { parts.push(format!("{name} {}", g.join(" "))); }
        };
        group("shared", b.shared_hit, b.shared_read, b.shared_dirtied, b.shared_written);
        group("local", b.local_hit, b.local_read, b.local_dirtied, b.local_written);
        if b.temp_read > 0 || b.temp_written > 0 {
            parts.push(format!("temp read={} written={}", b.temp_read, b.temp_written));
        }
        if !parts.is_empty() {
            line("Buffers", &parts.join(", "));
        }
    }
    if n.io_read_ms.is_some() || n.io_write_ms.is_some() {
        line("I/O Timings", &format!("read={:.3} write={:.3}", n.io_read_ms.unwrap_or(0.0), n.io_write_ms.unwrap_or(0.0)));
    }
    for c in &n.children {
        node(c, depth + 1, out);
    }
}
```

- [ ] **Step 4: Фикстуры со стенда**

Скрипт (выполнить один раз; pg-perf поработал под `heavy.sql`):

```bash
mkdir -p crates/pg-advisor/tests/fixtures/plans/json
P=crates/pg-advisor/tests/fixtures/plans/json
X() { docker exec gp-pg-perf psql -U postgres -d perf -XqAt -c "EXPLAIN ($2) $3" > "$P/$1.json"; }
X seq_scan_filter_analyze       "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT count(*) FROM shop.orders WHERE status = 'disputed'"
X nested_loop_misestimate_analyze "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT o.id, a.note FROM shop.audit a JOIN shop.orders o ON o.id = a.order_id WHERE a.actor = 'app' AND a.order_id = 1"
X sort_limit_analyze            "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT * FROM shop.orders ORDER BY total DESC LIMIT 10"
X sort_spill_analyze            "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT customer_id, sum(total) FROM shop.orders GROUP BY customer_id ORDER BY 2 DESC"
X index_only_heap_fetches_analyze "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT count(*) FROM shop.balances WHERE customer_id BETWEEN 1000 AND 2000"
X subplan_per_row_analyze       "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT oi.id FROM shop.order_items oi WHERE oi.product_id BETWEEN 100 AND 110 AND oi.price > (SELECT avg(x.price) FROM shop.order_items x WHERE x.order_id = oi.order_id)"
X append_partitions_analyze     "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT count(*) FROM shop.events WHERE kind = 'pay' AND payload ->> 'k' = '7'"
X parallel_jit_analyze          "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT customer_id % 1000, sum(total * 1.07 + 3), avg(total) FROM shop.orders GROUP BY 1"
X hash_join_temp_analyze        "ANALYZE, BUFFERS, VERBOSE, FORMAT JSON" "SELECT count(*) FROM shop.order_items oi JOIN shop.orders o ON o.id = oi.order_id WHERE o.created_at >= timestamptz '2025-06-01'"
X update_modify_table_plain     "VERBOSE, FORMAT JSON" "UPDATE shop.balances SET balance = balance + 1 WHERE customer_id = 1"
X generic_plan_plain            "VERBOSE, SETTINGS, GENERIC_PLAN, FORMAT JSON" "SELECT * FROM shop.customers WHERE id = \$1"
X cte_delete_plain              "VERBOSE, FORMAT JSON" "WITH d AS (DELETE FROM shop.audit WHERE id = 1 RETURNING id) SELECT count(*) FROM d"
for v in 13 14 15 16 18; do
  docker exec gp-pg$v psql -U postgres -d orders -XqAt -c "EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT JSON) SELECT c.email, count(*) FROM shop.orders o JOIN shop.customers c ON c.id = o.customer_id GROUP BY c.email ORDER BY 2 DESC LIMIT 5" > "$P/pg${v}_join_group_analyze.json"
done
```

(в `psql -At` вывод JSON идёт без обрамления; файлы — чистый JSON.) Проверить: `ls $P | wc -l` — 17 файлов.

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`
Expected: PASS, включая `tests/plans.rs` по 17 фикстурам.

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor
git commit -m "feat(pg-advisor): модель плана, парсер EXPLAIN JSON с вычисляемыми полями, текстовый рендер, фикстуры PG 13–18

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: EXPLAIN и EXPLAIN ANALYZE в два яруса

**Files:**
- Create: `src-tauri/src/explain/mod.rs`, `src-tauri/src/explain/params.rs`, `src-tauri/src/explain/tier.rs`, `src-tauri/src/explain/run.rs`, `src-tauri/src/commands/explain.rs`, `src-tauri/tests/it_explain.rs`
- Modify: `src-tauri/Cargo.toml` (`tokio-postgres` с `with-serde_json-1`), `src-tauri/src/lib.rs` (`pub mod explain;`, команды), `src-tauri/src/conn/pg.rs` (`EXPLAIN_SESSION_OPTIONS`, `for_explain`), `src-tauri/src/actions/conn.rs` (`open_with`, `open_explain`), `src-tauri/src/error.rs` (`ExplainWrites`), `src-tauri/src/actions/journal.rs` (`ExplainRecord`, `ActionRecord.explain`), `src-tauri/src/actions/{sessions,slots,patroni}.rs` (`explain: None` в литералах `ActionRecord`), `src-tauri/src/commands/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты `params.rs`, `tier.rs`, `mod.rs`; `it_explain.rs`

**Interfaces:**
- Consumes: `pg_advisor::plan::{PlanTree, RelationRef, json::from_json, relations, has_modify_table}`, `actions::conn::ActionClient`, `guard::ensure_mutation_allowed`, `actions::journal::{ActionJournal, ActionRecord, RecordConnection, Outcome, now_ms, code_name, cut}`, `diag::{head, Head}`, `diag::caps::V16`, `conn::errors::{from_pg, chain}`.
- Produces (`explain`): `Mode { Plan, Analyze }`; `Params` (тег `kind`: `none` | `generic` | `values { values: Vec<Option<String>> }`); `ExplainRequest { member, database, sql, mode, params, timeout_s: u32, search_path: Option<String>, confirmation: Confirmation, force_write: bool }`; `Tier { Read, Write }`; `ExplainResult { mode, tier, plan: PlanTree, raw: Value, relations: Vec<RelationRef>, elapsed_ms: u64, warnings: Vec<String> }`; `validate(&ExplainRequest) -> AppResult<String>` (нормализованный SQL); `params::{count_params, first_word, valid_search_path, normalize}`; `tier::tier_of(&PlanTree, sql) -> Tier`; `run::run(st, id, &ExplainRequest) -> AppResult<ExplainResult>`; константы `PROBE_TIMEOUT_S = 15`, `MAX_SQL_BYTES = 262_144`, `TIMEOUT_RANGE = 5..=600`, `DEFAULT_TIMEOUT_S = 30`.
  `conn::pg::EXPLAIN_SESSION_OPTIONS`, `ConnectSpec::for_explain()`; `actions::conn::open_with(st, id, member, database, options: fn(ConnectSpec) -> ConnectSpec)`, `actions::conn::open_explain(st, id, member, database)`; `ErrorCode::ExplainWrites` (`EXPLAIN_WRITES`); `journal::ExplainRecord { database, tables: Vec<String>, timeout_s, sql_head }`, `ActionRecord.explain: Option<ExplainRecord>`; команды `explain(id, request) -> ExplainResult`, `parse_plan(text) -> PlanTree`.

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/explain/params.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_largest_parameter_number() {
        assert_eq!(count_params("SELECT 1"), 0);
        assert_eq!(count_params("SELECT * FROM t WHERE a = $1 AND b = $2"), 2);
        assert_eq!(count_params("SELECT $3, $1"), 3);
        assert_eq!(count_params("SELECT $$text$$, $1"), 1, "долларовые кавычки — не параметр");
        assert_eq!(count_params("SELECT '$5'"), 5, "литерал в консоли даёт лишнее поле формы, не ошибку");
        assert_eq!(count_params("SELECT $"), 0);
    }

    #[test]
    fn first_word_skips_comments_and_parentheses() {
        assert_eq!(first_word("  SELECT 1"), "SELECT");
        assert_eq!(first_word("-- comment\n/* block */ (select 1)"), "select");
        assert_eq!(first_word("/* a */ CREATE TABLE x AS SELECT 1"), "CREATE");
        assert_eq!(first_word("with d as (delete from t) select 1"), "with");
        assert_eq!(first_word("   "), "");
    }

    #[test]
    fn normalizes_trailing_semicolons_and_validates_search_path() {
        assert_eq!(normalize("  SELECT 1 ;; \n"), "SELECT 1");
        assert_eq!(normalize("SELECT 1; SELECT 2"), "SELECT 1; SELECT 2", "внутренняя точка с запятой остаётся — её отвергнет сервер");
        assert!(valid_search_path("\"$user\", public, shop"));
        assert!(valid_search_path("pg_catalog"));
        assert!(!valid_search_path("public; DROP TABLE x"));
        assert!(!valid_search_path(""));
    }
}
```

`src-tauri/src/explain/tier.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::plan::json::from_json;
    use serde_json::json;

    fn tree(node_type: &str) -> pg_advisor::plan::PlanTree {
        from_json(&json!({"Plan": {"Node Type": node_type, "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}})).unwrap()
    }

    #[test]
    fn modify_table_or_create_is_write() {
        assert_eq!(tier_of(&tree("Seq Scan"), "SELECT 1"), Tier::Read);
        assert_eq!(tier_of(&tree("ModifyTable"), "UPDATE t SET a = 1"), Tier::Write);
        assert_eq!(tier_of(&tree("Seq Scan"), "/* x */ create table t2 as select 1"), Tier::Write);
        assert_eq!(tier_of(&tree("Seq Scan"), "SELECT nextval('s')"), Tier::Read, "побочный эффект ловит READ ONLY");
        assert_eq!(serde_json::to_value(Tier::Write).unwrap(), "write");
    }
}
```

`src-tauri/src/explain/mod.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Confirmation;
    use crate::error::ErrorCode;

    fn req(sql: &str, mode: Mode, params: Params) -> ExplainRequest {
        ExplainRequest {
            member: "pg-1".into(),
            database: "perf".into(),
            sql: sql.into(),
            mode,
            params,
            timeout_s: DEFAULT_TIMEOUT_S,
            search_path: None,
            confirmation: Confirmation { confirm: None, typed_name: None },
            force_write: false,
        }
    }

    #[test]
    fn request_json_shape() {
        let r: ExplainRequest = serde_json::from_value(serde_json::json!({
            "member": "pg-1", "database": "perf", "sql": "SELECT $1", "mode": "analyze",
            "params": {"kind": "values", "values": ["1", null]}, "timeoutS": 60, "searchPath": null,
            "confirmation": {"confirm": null, "typedName": null}
        }))
        .unwrap();
        assert_eq!((r.mode, r.timeout_s), (Mode::Analyze, 60));
        assert_eq!(r.params, Params::Values { values: vec![Some("1".into()), None] });
        let r: ExplainRequest = serde_json::from_value(serde_json::json!({"member": "m", "database": "d", "sql": "SELECT 1", "mode": "plan", "params": {"kind": "generic"}})).unwrap();
        assert_eq!((r.params, r.timeout_s, r.search_path, r.force_write), (Params::Generic, DEFAULT_TIMEOUT_S, None, false), "срок, search_path и forceWrite по умолчанию");
    }

    #[test]
    fn validation() {
        assert_eq!(validate(&req("  SELECT 1; ", Mode::Plan, Params::None)).unwrap(), "SELECT 1");
        assert_eq!(validate(&req("   ", Mode::Plan, Params::None)).unwrap_err().key, "errors.explain.emptySql");
        let long = "SELECT ".to_string() + &"1,".repeat(MAX_SQL_BYTES / 2);
        assert_eq!(validate(&req(&long, Mode::Plan, Params::None)).unwrap_err().key, "errors.explain.sqlTooLong");
        let e = validate(&req("SELECT $1, $2", Mode::Plan, Params::Values { values: vec![Some("1".into())] })).unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::InvalidInput, "errors.explain.paramsCount"));
        assert_eq!((e.params["expected"].as_str(), e.params["got"].as_str()), ("2", "1"));
        assert_eq!(validate(&req("SELECT $1", Mode::Plan, Params::None)).unwrap_err().key, "errors.explain.paramsCount");
        assert!(validate(&req("SELECT $1", Mode::Plan, Params::Generic)).is_ok());
        assert_eq!(validate(&req("SELECT $1", Mode::Analyze, Params::Generic)).unwrap_err().key, "errors.explain.genericAnalyze");
        let mut r = req("SELECT 1", Mode::Analyze, Params::None);
        r.timeout_s = 4;
        assert_eq!(validate(&r).unwrap_err().key, "errors.explain.badTimeout");
        r.timeout_s = 600;
        r.search_path = Some("public; drop".into());
        assert_eq!(validate(&r).unwrap_err().key, "errors.explain.badSearchPath");
        r.search_path = Some("public".into());
        assert!(validate(&r).is_ok());
    }

    #[test]
    fn explain_writes_code_serializes() {
        assert_eq!(serde_json::to_value(ErrorCode::ExplainWrites).unwrap(), "EXPLAIN_WRITES");
        assert!(!crate::error::AppError::localized(ErrorCode::ExplainWrites, ltext!("errors.explain.writes", "x")).retryable);
    }
}
```

`src-tauri/tests/it_explain.rs`:

```rust
//! EXPLAIN и EXPLAIN ANALYZE на стенде (спец. P3 §8): ярусы, параметры, GENERIC_PLAN, журнал, защита read-only
//! и prod, таймаут, вторая инструкция, реплика. `GPG_IT_PERF` — pg-perf, `GPG_IT_MATRIX` — PG 15 для отказа
//! GENERIC_PLAN, `GPG_IT_REPLICA` — реплика пары. Запускать с `--test-threads=1`.

mod common;

use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::journal::Outcome;
use glafira_pg_lib::conn::profile::Environment;
use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::explain::{self, ExplainRequest, Mode, Params, Tier};

fn req(member: &str, database: &str, sql: &str, mode: Mode, params: Params) -> ExplainRequest {
    ExplainRequest {
        member: member.into(),
        database: database.into(),
        sql: sql.into(),
        mode,
        params,
        timeout_s: 30,
        search_path: None,
        confirmation: Confirmation { confirm: None, typed_name: None },
        force_write: false,
    }
}

fn values(v: &[&str]) -> Params {
    Params::Values { values: v.iter().map(|s| Some((*s).to_string())).collect() }
}

async fn member_of(st: &glafira_pg_lib::state::AppState) -> String {
    st.runtimes.view("it").topology.unwrap().members[0].name.clone()
}

#[tokio::test]
async fn plan_and_analyze_read_tier_on_perf() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    // Пробный план с параметром-значением: тип выводит сервер.
    let r = explain::run::run(&st, "it", &req(&m, "perf", "SELECT count(*) FROM shop.orders WHERE status = $1", Mode::Plan, values(&["disputed"]))).await.unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Plan, Tier::Read));
    assert!(r.plan.root.actual.is_none());
    assert!(r.relations.iter().any(|x| x.schema.as_deref() == Some("shop") && x.name == "orders"), "{:?}", r.relations);
    assert!(r.raw.is_array());
    // GENERIC_PLAN на 17.
    let r = explain::run::run(&st, "it", &req(&m, "perf", "SELECT * FROM shop.customers WHERE id = $1", Mode::Plan, Params::Generic)).await.unwrap();
    assert!(r.warnings.iter().any(|w| w == "genericPlan"));
    assert_eq!(r.plan.root.node_type, "Index Scan");
    // ANALYZE чтения — без подтверждения и без строки в журнале.
    let r = explain::run::run(&st, "it", &req(&m, "perf", "SELECT count(*) FROM shop.customers WHERE id < $1", Mode::Analyze, values(&["100"]))).await.unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Analyze, Tier::Read));
    assert!(r.plan.root.actual.is_some() && r.plan.execution_ms.is_some());
    assert!(r.plan.root.buffers.is_some(), "BUFFERS");
    assert!(st.actions.list(Some("it"), 10).is_empty(), "чтение в журнал не пишется");
    // Побочный эффект в ярусе чтения: nextval в READ ONLY → EXPLAIN_WRITES.
    let e = explain::run::run(&st, "it", &req(&m, "perf", "SELECT nextval('shop.customers_id_seq')", Mode::Analyze, Params::None)).await.unwrap_err();
    assert_eq!((e.code, e.key.as_str()), (ErrorCode::ExplainWrites, "errors.explain.writes"));
    // «Выполнить как запись»: тот же запрос с forceWrite идёт ярусом записи (dev — без подтверждения) и пишется в журнал.
    let mut forced = req(&m, "perf", "SELECT nextval('shop.customers_id_seq')", Mode::Analyze, Params::None);
    forced.force_write = true;
    let r = explain::run::run(&st, "it", &forced).await.unwrap();
    assert_eq!(r.tier, Tier::Write);
    assert_eq!(st.actions.list(Some("it"), 10).len(), 1, "ярус записи — строка журнала");
    // Вторая инструкция и таймаут.
    let e = explain::run::run(&st, "it", &req(&m, "perf", "SELECT 1; SELECT 2", Mode::Plan, Params::None)).await.unwrap_err();
    assert_eq!((e.code, e.key.as_str()), (ErrorCode::InvalidInput, "errors.explain.oneStatement"));
    let mut slow = req(&m, "perf", "SELECT pg_sleep(10)", Mode::Analyze, Params::None);
    slow.timeout_s = 5;
    let e = explain::run::run(&st, "it", &slow).await.unwrap_err();
    assert_eq!((e.code, e.key.as_str()), (ErrorCode::Timeout, "errors.explain.timeout"));
    // Синтаксическая ошибка — сообщение сервера и позиция.
    let e = explain::run::run(&st, "it", &req(&m, "perf", "SELEC 1", Mode::Plan, Params::None)).await.unwrap_err();
    assert_eq!((e.code, e.key.as_str()), (ErrorCode::InvalidInput, "errors.explain.server"));
    assert!(e.details.as_deref().is_some_and(|d| d.contains("position")), "{e:?}");
}

#[tokio::test]
async fn write_tier_rolls_back_journals_and_respects_guard() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    let su = common::superuser(&perf, "perf").await;
    let before: String = su.query_one("SELECT balance::text FROM shop.balances WHERE customer_id = 1", &[]).await.unwrap().get(0);
    let sql = "UPDATE shop.balances SET balance = balance + 100 WHERE customer_id = $1";
    // Пробный план показывает ярус записи без выполнения.
    let r = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Plan, values(&["1"]))).await.unwrap();
    assert_eq!(r.tier, Tier::Write);
    assert_eq!(r.relations.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["balances"]);
    // Dev-профиль: подтверждения не нужно; выполнено и откатано; строка журнала.
    let r = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Analyze, values(&["1"]))).await.unwrap();
    assert_eq!((r.mode, r.tier), (Mode::Analyze, Tier::Write));
    assert!(r.plan.root.actual.is_some());
    let after: String = su.query_one("SELECT balance::text FROM shop.balances WHERE customer_id = 1", &[]).await.unwrap().get(0);
    assert_eq!(before, after, "изменения откатаны");
    let records = st.actions.list(Some("it"), 10);
    assert_eq!(records.len(), 1);
    let rec = &records[0];
    assert_eq!((rec.action.as_str(), rec.outcome, rec.member.as_deref()), ("explainAnalyze", Outcome::Ok, Some(m.as_str())));
    let ex = rec.explain.as_ref().expect("поле explain");
    assert_eq!((ex.database.as_str(), ex.tables.as_slice(), ex.timeout_s), ("perf", &["shop.balances".to_string()][..], 30));
    assert!(ex.sql_head.starts_with("UPDATE shop.balances"));
    // Prod без имени подключения — отказ до сервера; с именем — проходит.
    let mut p = st.profile("it").unwrap();
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let e = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Analyze, values(&["1"]))).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ConfirmationRequired);
    let mut ok = req(&m, "perf", sql, Mode::Analyze, values(&["1"]));
    ok.confirmation.confirm = Some("it".into());
    let r = explain::run::run(&st, "it", &ok).await.unwrap();
    assert_eq!(r.tier, Tier::Write);
    assert!(st.actions.list(Some("it"), 10)[0].prod_confirmed);
    // Read-only профиль: ярус записи закрыт, ярус чтения работает.
    p.environment = Environment::Dev;
    p.read_only = true;
    st.connections.upsert(p).unwrap();
    let e = explain::run::run(&st, "it", &req(&m, "perf", sql, Mode::Analyze, values(&["1"]))).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ReadOnly);
    assert!(explain::run::run(&st, "it", &req(&m, "perf", "SELECT 1", Mode::Analyze, Params::None)).await.is_ok());
    assert_eq!(st.actions.list(Some("it"), 10).len(), 2, "отказы guard в журнал не пишутся");
}

#[tokio::test]
async fn generic_plan_is_refused_before_16_and_writes_are_refused_on_a_replica() {
    if let Some(pg15) = common::matrix_addrs().into_iter().find(|a| a.ends_with(":5515")) {
        let (_dir, st) = common::open_server(&pg15).await;
        let m = member_of(&st).await;
        let e = explain::run::run(&st, "it", &req(&m, "postgres", "SELECT $1::int", Mode::Plan, Params::Generic)).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::FeatureUnavailable, "errors.explain.genericUnsupported"));
        assert!(explain::run::run(&st, "it", &req(&m, "postgres", "SELECT $1::int", Mode::Plan, values(&["1"]))).await.is_ok());
    }
    if let Some(replica) = common::env("GPG_IT_REPLICA") {
        let (_dir, st) = common::open_server_in(&replica, "orders").await;
        let m = member_of(&st).await;
        let sql = "UPDATE shop.orders SET status = status WHERE id = 1";
        let r = explain::run::run(&st, "it", &req(&m, "orders", sql, Mode::Plan, Params::None)).await.unwrap();
        assert_eq!(r.tier, Tier::Write, "пробный план строится и на реплике");
        let e = explain::run::run(&st, "it", &req(&m, "orders", sql, Mode::Analyze, Params::None)).await.unwrap_err();
        assert_eq!((e.code, e.key.as_str()), (ErrorCode::FeatureUnavailable, "errors.explain.writeOnReplica"));
        assert!(explain::run::run(&st, "it", &req(&m, "orders", "SELECT count(*) FROM shop.orders", Mode::Analyze, Params::None)).await.is_ok());
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib explain`
Expected: ошибка компиляции.

- [ ] **Step 3: Соединение, ошибка, журнал**

`src-tauri/Cargo.toml`: `tokio-postgres = { version = "0.7", features = ["with-serde_json-1"] }` (колонка `json` ответа EXPLAIN читается как `serde_json::Value`; нового крейта нет).

`src-tauri/src/conn/pg.rs` — после `ACTION_SESSION_OPTIONS`:

```rust
/// Параметры сессии EXPLAIN (спец. P3 §8.3): без `search_path=pg_catalog` (запрос пользователя должен видеть его
/// схемы) и без `default_transaction_read_only` (ярусы задают режим транзакции сами).
pub const EXPLAIN_SESSION_OPTIONS: &str = "-c statement_timeout=5000 -c lock_timeout=1000 -c DateStyle=ISO";
```

и метод рядом с `for_action`:

```rust
    pub fn for_explain(mut self) -> Self {
        self.options = EXPLAIN_SESSION_OPTIONS;
        self
    }
```

В тесте `conn/pg.rs`, где проверяется `for_action`, добавить: `assert_eq!(spec.clone().for_explain().pg_config().get_options(), Some(EXPLAIN_SESSION_OPTIONS)); assert!(!EXPLAIN_SESSION_OPTIONS.contains("search_path"));` (если `ConnectSpec` не `Clone` — построить спеку второй раз тем же вызовом).

`src-tauri/src/actions/conn.rs` — `open` переименовать в `open_with` с параметром `options: fn(ConnectSpec) -> ConnectSpec` (вместо `.for_action()` — `options(spec)`), и добавить:

```rust
/// Соединение действия (спец. P2 §8.2): параметры `ACTION_SESSION_OPTIONS`.
pub async fn open(st: &AppState, id: &str, member: &str, database: Option<&str>) -> AppResult<ActionClient> {
    open_with(st, id, member, database, ConnectSpec::for_action).await
}

/// Соединение EXPLAIN (спец. P3 §8.3): `EXPLAIN_SESSION_OPTIONS`, вне пулов, на один запрос.
pub async fn open_explain(st: &AppState, id: &str, member: &str, database: Option<&str>) -> AppResult<ActionClient> {
    open_with(st, id, member, database, ConnectSpec::for_explain).await
}
```

`src-tauri/src/error.rs` — в `enum ErrorCode` после `ConfirmationRequired`: `/// Запрос, признанный чтением, пытается писать (SQLSTATE 25006 в `READ ONLY`).` `ExplainWrites,`. В тесте `guard_codes_serialize_for_frontend` добавить `assert_eq!(serde_json::to_value(ErrorCode::ExplainWrites).unwrap(), "EXPLAIN_WRITES");`.

`src-tauri/src/actions/journal.rs` — после `PatroniRecord`:

```rust
/// EXPLAIN ANALYZE яруса записи (спец. P3 §8.4): база, затронутые таблицы `schema.name`, срок и начало текста.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRecord {
    pub database: String,
    pub tables: Vec<String>,
    pub timeout_s: u32,
    pub sql_head: String,
}
```

в `ActionRecord` после `patroni`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<ExplainRecord>,
```

и в комментарии к `action` добавить `| \`explainAnalyze\``. Во всех литералах `ActionRecord { … }` (`actions/sessions.rs`, `actions/slots.rs`, `actions/patroni.rs`, тесты `journal.rs`; найти `grep -rn "patroni: " src-tauri/src/actions`) добавить `explain: None,`.

- [ ] **Step 4: Модуль `explain`**

`src-tauri/src/explain/mod.rs`:

```rust
//! EXPLAIN и EXPLAIN ANALYZE (спец. P3 §8): запрос, результат, проверки до сервера. Ход — `run`, параметры —
//! `params`, ярус — `tier`, данные для LLM — `llm`.

pub mod params;
pub mod run;
pub mod tier;

use pg_advisor::plan::{PlanTree, RelationRef};
use serde::{Deserialize, Serialize};

use crate::actions::Confirmation;
use crate::error::{AppResult, ErrorCode};

pub const PROBE_TIMEOUT_S: u32 = 15;
pub const DEFAULT_TIMEOUT_S: u32 = 30;
pub const TIMEOUT_RANGE: std::ops::RangeInclusive<u32> = 5..=600;
pub const MAX_SQL_BYTES: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Plan,
    Analyze,
}

/// Параметры `$N`: нет, `GENERIC_PLAN` (16+, только без ANALYZE) или значения (`None` — NULL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Params {
    None,
    Generic,
    Values { values: Vec<Option<String>> },
}

fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT_S
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainRequest {
    pub member: String,
    pub database: String,
    pub sql: String,
    pub mode: Mode,
    pub params: Params,
    /// Только ANALYZE, секунды; пробный план всегда в `PROBE_TIMEOUT_S`.
    #[serde(default = "default_timeout")]
    pub timeout_s: u32,
    /// `None` — серверный по умолчанию для роли.
    #[serde(default)]
    pub search_path: Option<String>,
    #[serde(default)]
    pub confirmation: Confirmation,
    /// Выполнить как запись, хотя пробный план — чтение (кнопка после `EXPLAIN_WRITES`).
    #[serde(default)]
    pub force_write: bool,
}

pub use tier::Tier;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainResult {
    pub mode: Mode,
    pub tier: Tier,
    pub plan: PlanTree,
    /// JSON сервера как есть — кнопка «JSON».
    pub raw: serde_json::Value,
    pub relations: Vec<RelationRef>,
    pub elapsed_ms: u64,
    /// Ключи предупреждений интерфейса: `genericPlan`, `probeWithoutReadOnly`.
    pub warnings: Vec<String>,
}

/// Проверки до сервера; возвращает нормализованный SQL (без хвостовых `;`).
pub fn validate(req: &ExplainRequest) -> AppResult<String> {
    let sql = params::normalize(&req.sql);
    if sql.is_empty() {
        return Err(app_err!(ErrorCode::InvalidInput, "errors.explain.emptySql", "Enter a query"));
    }
    if sql.len() > MAX_SQL_BYTES {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.sqlTooLong",
            "The query is longer than {{max}} bytes",
            max = MAX_SQL_BYTES
        ));
    }
    let expected = params::count_params(&sql);
    let got = match &req.params {
        Params::Values { values } => values.len(),
        Params::Generic => expected,
        Params::None => 0,
    };
    if got != expected {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.paramsCount",
            "The query has {{expected}} parameters, {{got}} values given",
            expected = expected,
            got = got
        ));
    }
    if req.params == Params::Generic && req.mode == Mode::Analyze {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.genericAnalyze",
            "GENERIC_PLAN cannot be combined with ANALYZE: enter parameter values"
        ));
    }
    if req.mode == Mode::Analyze && !TIMEOUT_RANGE.contains(&req.timeout_s) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.badTimeout",
            "The timeout must be between 5 and 600 seconds"
        ));
    }
    if let Some(sp) = &req.search_path
        && !params::valid_search_path(sp)
    {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.badSearchPath",
            "search_path may contain only identifiers, quotes, commas and spaces"
        ));
    }
    Ok(sql)
}
```

(`Confirmation` должен быть `Default` — добавить `#[derive(Default)]` к структуре в `actions/mod.rs`, если его нет.)

`src-tauri/src/explain/params.rs`:

```rust
//! Параметры `$N`, первое слово инструкции, `search_path` — разбор без парсера SQL (решение владельца: `pg_query` — P4).

/// Наибольший номер `$N` — число параметров. Текст pgss литералов не содержит; в консоли литерал с `$1` внутри
/// строки даст лишнее поле формы, а не ошибку. `$$` долларовых кавычек — не параметр.
pub fn count_params(sql: &str) -> usize {
    let b = sql.as_bytes();
    let (mut max, mut i) = (0usize, 0usize);
    while i < b.len() {
        if b[i] == b'$' {
            let mut j = i + 1;
            let mut n = 0usize;
            while j < b.len() && b[j].is_ascii_digit() && n < 100_000 {
                n = n * 10 + usize::from(b[j] - b'0');
                j += 1;
            }
            if j > i + 1 && n > max {
                max = n;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    max
}

/// Первое слово инструкции без ведущих комментариев (`--`, `/* */`) и открывающих скобок; пусто — слов нет.
pub fn first_word(sql: &str) -> String {
    let mut s = sql;
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |(_, r)| r);
        } else {
            break;
        }
    }
    s.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect()
}

/// Хвостовые `;` и пробелы снимаются; внутренняя `;` остаётся — её отвергнет сервер (одна инструкция).
pub fn normalize(sql: &str) -> String {
    let mut s = sql.trim();
    while let Some(t) = s.strip_suffix(';') {
        s = t.trim_end();
    }
    s.to_string()
}

/// `search_path` для `SET LOCAL`: идентификаторы, кавычки, `$user`, запятые, пробелы, точки и дефисы.
pub fn valid_search_path(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 1024
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '"' | ',' | ' ' | '.' | '-'))
}
```

`src-tauri/src/explain/tier.rs`:

```rust
//! Ярус EXPLAIN ANALYZE (спец. P3 §8.3): запись — `ModifyTable` в плане (в том числе внутри CTE) или инструкция
//! `CREATE … AS`; иначе чтение. Ошибку яруса страхует транзакция `READ ONLY`.

use pg_advisor::plan::{PlanTree, has_modify_table};
use serde::Serialize;

use crate::explain::params::first_word;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Read,
    Write,
}

pub fn tier_of(tree: &PlanTree, sql: &str) -> Tier {
    if has_modify_table(tree) || first_word(sql).eq_ignore_ascii_case("create") {
        Tier::Write
    } else {
        Tier::Read
    }
}
```

`src-tauri/src/explain/run.rs`:

```rust
//! Ход EXPLAIN (спец. P3 §8.3): соединение `for_explain`, пробный план в `READ ONLY`, ярус, выполнение, журнал.

use std::time::Instant;

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanTree, relations};
use tokio_postgres::error::SqlState;
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::Client;

use crate::actions::journal::{self, ActionRecord, ExplainRecord, RecordConnection};
use crate::conn::errors::{chain, from_pg};
use crate::conn::profile::Environment;
use crate::diag::caps::V16;
use crate::diag::head;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::tier::{Tier, tier_of};
use crate::explain::{ExplainRequest, ExplainResult, Mode, PROBE_TIMEOUT_S, Params, validate};
use crate::guard::ensure_mutation_allowed;
use crate::state::AppState;

const PROBE_OPTIONS: &str = "FORMAT JSON, VERBOSE, SETTINGS";
const ANALYZE_OPTIONS: &str = "ANALYZE, BUFFERS, WAL, VERBOSE, SETTINGS, FORMAT JSON";

/// Ошибка одного EXPLAIN: сырая ошибка сервера нужна до перевода в `AppError` (25006, «вторая инструкция»).
enum Step {
    Pg(tokio_postgres::Error),
    App(AppError),
}

impl From<tokio_postgres::Error> for Step {
    fn from(e: tokio_postgres::Error) -> Self {
        Step::Pg(e)
    }
}

fn is_read_only_violation(e: &tokio_postgres::Error) -> bool {
    e.code() == Some(&SqlState::READ_ONLY_SQL_TRANSACTION)
}

/// Перевод ошибки сервера: вторая инструкция, синтаксис с позицией, 57014, 42501 и прочие — через `from_pg`.
fn classify(e: &tokio_postgres::Error) -> AppError {
    let db = e.as_db_error();
    let message = db.map(|d| d.message().to_string()).unwrap_or_else(|| chain(e));
    if e.code() == Some(&SqlState::SYNTAX_ERROR) {
        if message.contains("multiple commands") {
            return app_err!(ErrorCode::InvalidInput, "errors.explain.oneStatement", "Enter exactly one statement");
        }
        let position = db
            .and_then(|d| d.position())
            .map(|p| match p {
                tokio_postgres::error::ErrorPosition::Original(n) => *n,
                tokio_postgres::error::ErrorPosition::Internal { position, .. } => *position,
            });
        let err = app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.server",
            "The server rejected the query: {{message}}",
            message = message
        );
        return match position {
            Some(p) => err.with_details(format!("position {p}")),
            None => err,
        };
    }
    if e.code() == Some(&SqlState::QUERY_CANCELED) {
        return app_err!(
            ErrorCode::Timeout,
            "errors.explain.timeout",
            "The plan was not produced in time; execution was cancelled, nothing changed"
        );
    }
    if e.code() == Some(&SqlState::INSUFFICIENT_PRIVILEGE) {
        return app_err!(
            ErrorCode::NotAuthorized,
            "errors.explain.notAuthorized",
            "Not enough privileges on the query's tables: grant pg_read_all_data (14+) or SELECT"
        )
        .with_details(message);
    }
    if e.code() == Some(&SqlState::READ_ONLY_SQL_TRANSACTION) {
        return explain_writes();
    }
    from_pg(e)
}

fn explain_writes() -> AppError {
    app_err!(
        ErrorCode::ExplainWrites,
        "errors.explain.writes",
        "The query writes data (a volatile function or trigger): run it as a write with confirmation"
    )
}

/// Один EXPLAIN внутри транзакции: `BEGIN [READ ONLY]`, `SET LOCAL`, EXPLAIN расширенным протоколом, `ROLLBACK`.
async fn explain_in_tx(
    c: &Client,
    read_only: bool,
    timeout_s: u32,
    search_path: Option<&str>,
    options: &str,
    sql: &str,
    params: &Params,
) -> Result<serde_json::Value, Step> {
    let mut begin = format!(
        "BEGIN{}; SET LOCAL statement_timeout = '{timeout_s}s'; SET LOCAL lock_timeout = '1s'",
        if read_only { " READ ONLY" } else { "" }
    );
    if let Some(sp) = search_path {
        begin.push_str(&format!("; SET LOCAL search_path = {sp}"));
    }
    c.batch_execute(&begin).await?;
    let text = format!("EXPLAIN ({options}) {sql}");
    let result = match params {
        Params::Values { values } => {
            let types = vec![Type::UNKNOWN; values.len()];
            match c.prepare_typed(&text, &types).await {
                Ok(stmt) => {
                    let refs: Vec<&(dyn ToSql + Sync)> = values.iter().map(|v| v as &(dyn ToSql + Sync)).collect();
                    c.query(&stmt, &refs).await
                }
                Err(e) => Err(e),
            }
        }
        _ => c.query(&text, &[]).await,
    };
    // Откат — страховка: при ошибке транзакция уже прервана сервером, при обрыве соединение закроется.
    let _ = c.batch_execute("ROLLBACK").await;
    let rows = result?;
    let row = rows
        .first()
        .ok_or_else(|| Step::App(AppError::internal_with("EXPLAIN returned no rows")))?;
    row.try_get::<_, serde_json::Value>(0)
        .map_err(|e| Step::App(AppError::internal_with(format!("EXPLAIN output is not JSON: {e}"))))
}

fn probe_options(params: &Params) -> String {
    match params {
        Params::Generic => format!("{PROBE_OPTIONS}, GENERIC_PLAN"),
        _ => PROBE_OPTIONS.to_string(),
    }
}

fn result(mode: Mode, tier: Tier, tree: PlanTree, raw: serde_json::Value, started: Instant, warnings: Vec<String>) -> ExplainResult {
    let relations = relations(&tree);
    ExplainResult {
        mode,
        tier,
        plan: tree,
        raw,
        relations,
        elapsed_ms: started.elapsed().as_millis() as u64,
        warnings,
    }
}

fn parse(raw: &serde_json::Value) -> AppResult<PlanTree> {
    from_json(raw).map_err(|e| AppError::internal_with(format!("plan JSON: {}", e.message)))
}

pub async fn run(st: &AppState, id: &str, req: &ExplainRequest) -> AppResult<ExplainResult> {
    let sql = validate(req)?;
    let profile = st.profile(id)?;
    let started = Instant::now();
    let client = crate::actions::conn::open_explain(st, id, &req.member, Some(&req.database)).await?;
    let c = &client.client;
    let h = head(c).await?;
    if req.params == Params::Generic && h.version_num < V16 {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.explain.genericUnsupported",
            "GENERIC_PLAN needs PostgreSQL 16 or newer: enter parameter values"
        ));
    }
    let mut warnings = Vec::new();
    if req.params == Params::Generic {
        warnings.push("genericPlan".to_string());
    }
    let sp = req.search_path.as_deref();
    // 1. Пробный план в READ ONLY; отвергнут 25006 (CREATE … AS на отдельных версиях) — повтор без READ ONLY,
    //    выполнения всё равно нет (уточнение 12 плана).
    let probe_opts = probe_options(&req.params);
    let raw = match explain_in_tx(c, true, PROBE_TIMEOUT_S, sp, &probe_opts, &sql, &req.params).await {
        Ok(v) => v,
        Err(Step::Pg(e)) if is_read_only_violation(&e) => {
            warnings.push("probeWithoutReadOnly".to_string());
            match explain_in_tx(c, false, PROBE_TIMEOUT_S, sp, &probe_opts, &sql, &req.params).await {
                Ok(v) => v,
                Err(Step::Pg(e)) => return Err(classify(&e)),
                Err(Step::App(e)) => return Err(e),
            }
        }
        Err(Step::Pg(e)) => return Err(classify(&e)),
        Err(Step::App(e)) => return Err(e),
    };
    let tree = parse(&raw)?;
    let tier = if req.force_write { Tier::Write } else { tier_of(&tree, &sql) };
    if req.mode == Mode::Plan {
        return Ok(result(Mode::Plan, tier, tree, raw, started, warnings));
    }
    // 3–4. ANALYZE.
    let read_only = match tier {
        Tier::Read => true,
        Tier::Write => {
            ensure_mutation_allowed(&profile, req.confirmation.confirm.as_deref())?;
            if h.in_recovery {
                return Err(app_err!(
                    ErrorCode::FeatureUnavailable,
                    "errors.explain.writeOnReplica",
                    "A write cannot be executed on a replica: choose the primary"
                ));
            }
            false
        }
    };
    let analyzed = explain_in_tx(c, read_only, req.timeout_s, sp, ANALYZE_OPTIONS, &sql, &req.params).await;
    let outcome = match &analyzed {
        Ok(_) => (journal::Outcome::Ok, None),
        Err(Step::Pg(e)) => {
            let err = classify(e);
            (journal::Outcome::Error, Some(journal::code_name(err.code)))
        }
        Err(Step::App(e)) => (journal::Outcome::Error, Some(journal::code_name(e.code))),
    };
    if tier == Tier::Write {
        let tables = relations(&tree)
            .iter()
            .map(|r| match &r.schema {
                Some(s) => format!("{s}.{}", r.name),
                None => r.name.clone(),
            })
            .collect();
        let record = ActionRecord {
            ts: journal::now_ms(),
            action: "explainAnalyze".into(),
            connection: RecordConnection::of(&profile),
            member: Some(req.member.clone()),
            prod_confirmed: profile.environment == Environment::Prod,
            outcome: outcome.0,
            code: outcome.1.clone(),
            ms: started.elapsed().as_millis() as u64,
            sessions: vec![],
            slot: None,
            patroni: None,
            explain: Some(ExplainRecord {
                database: req.database.clone(),
                tables,
                timeout_s: req.timeout_s,
                sql_head: journal::cut(&sql, 200),
            }),
        };
        let j = st.actions.clone();
        let _ = crate::conn::run_blocking(move || Ok(j.append(&record))).await;
    }
    match analyzed {
        Ok(raw) => {
            let tree = parse(&raw)?;
            Ok(result(Mode::Analyze, tier, tree, raw, started, warnings))
        }
        Err(Step::Pg(e)) => Err(classify(&e)),
        Err(Step::App(e)) => Err(e),
    }
}
```

`classify` принимает ссылку: `tokio_postgres::Error` не клонируется, а для журнала код ошибки нужен до возврата самой ошибки (`match &analyzed`).

`src-tauri/src/commands/explain.rs`:

```rust
//! Команды планов (спец. P3 §10): EXPLAIN на узле и разбор вставленного плана без сервера.

use pg_advisor::plan::PlanTree;
use pg_advisor::plan::json::{from_json, from_psql_text};
use tauri::State;

use crate::error::{AppResult, ErrorCode};
use crate::explain::{ExplainRequest, ExplainResult};
use crate::state::AppState;

#[tauri::command]
pub async fn explain(state: State<'_, AppState>, id: String, request: ExplainRequest) -> AppResult<ExplainResult> {
    crate::explain::run::run(&state, &id, &request).await
}

/// Вставленный план: JSON, в том числе с обрамлением psql (P3b — и текст). Без сервера.
#[tauri::command]
pub fn parse_plan(text: String) -> AppResult<PlanTree> {
    let value = from_psql_text(&text).map_err(|e| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.plan.unparsed",
            "The plan could not be parsed (line {{line}}): {{message}}",
            line = e.line.unwrap_or(1),
            message = e.message
        )
    })?;
    from_json(&value).map_err(|e| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.plan.unparsed",
            "The plan could not be parsed (line {{line}}): {{message}}",
            line = e.line.unwrap_or(1),
            message = e.message
        )
    })
}
```

`src-tauri/src/lib.rs` — `pub mod explain;` и команды `commands::explain::explain, commands::explain::parse_plan,`; `src-tauri/src/commands/mod.rs` — `pub mod explain;`.

Словари (`errors.explain`, en / ru):
`emptySql`: "Enter a query" / "Введите запрос"; `sqlTooLong`: "The query is longer than {{max}} bytes" / "Запрос длиннее {{max}} байт"; `paramsCount`: "The query has {{expected}} parameters, {{got}} values given" / "В запросе {{expected}} параметров, значений передано {{got}}"; `genericAnalyze`: "GENERIC_PLAN cannot be combined with ANALYZE: enter parameter values" / "GENERIC_PLAN несовместим с ANALYZE: введите значения параметров"; `badTimeout`: "The timeout must be between 5 and 600 seconds" / "Срок — от 5 до 600 секунд"; `badSearchPath`: "search_path may contain only identifiers, quotes, commas and spaces" / "search_path — только идентификаторы, кавычки, запятые и пробелы"; `genericUnsupported`: "GENERIC_PLAN needs PostgreSQL 16 or newer: enter parameter values" / "GENERIC_PLAN требует PostgreSQL 16 и новее: введите значения параметров"; `oneStatement`: "Enter exactly one statement" / "Введите ровно одну инструкцию"; `server`: "The server rejected the query: {{message}}" / "Сервер отверг запрос: {{message}}"; `timeout`: "The plan was not produced in time; execution was cancelled, nothing changed" / "План не получен за отведённое время; выполнение прервано, изменений нет"; `notAuthorized`: "Not enough privileges on the query's tables: grant pg_read_all_data (14+) or SELECT" / "Не хватает прав на таблицы запроса: выдайте pg_read_all_data (14+) или SELECT"; `writes`: "The query writes data (a volatile function or trigger): run it as a write with confirmation" / "Запрос пишет данные (volatile-функция или триггер): выполните как запись с подтверждением"; `writeOnReplica`: "A write cannot be executed on a replica: choose the primary" / "Запись на реплике невозможна: выберите primary". `errors.plan.unparsed`: "The plan could not be parsed (line {{line}}): {{message}}" / "План не разобран (строка {{line}}): {{message}}". `actions.kind.explainAnalyze`: "EXPLAIN ANALYZE (write)" / "EXPLAIN ANALYZE (запись)".

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS.

Run: `GPG_IT_PERF=localhost:5471 GPG_IT_REPLICA=localhost:5452 GPG_IT_MATRIX=localhost:5515 cargo test --manifest-path src-tauri/Cargo.toml --test it_explain -- --test-threads=1`
Expected: 3 теста PASS.

- [ ] **Step 6: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(explain): EXPLAIN и EXPLAIN ANALYZE в два яруса — пробный план, READ ONLY без подтверждения, запись с guard и журналом, parse_plan

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Данные для «Скопировать для LLM» — `query_llm_bundle`

**Files:**
- Create: `src-tauri/src/explain/llm.rs`
- Modify: `src-tauri/src/explain/mod.rs` (`pub mod llm;`), `src-tauri/src/commands/queries.rs` (`query_llm_bundle`), `src-tauri/src/lib.rs`, `src-tauri/tests/it_explain.rs`
- Test: unit-тесты `llm.rs`; `it_explain.rs`

**Interfaces:**
- Consumes: `diag::query_detail::{detail, QueryDetail}`, `explain::run::run`, `explain::params::count_params`, `pg_advisor::plan::{PlanTree, relations, text_out::to_text}`, `catalog::table::get`, `commands::catalog::{on_catalog_node_within, catalog_limit}`.
- Produces (`explain::llm`): `LlmSource` (тег `kind`: `query { queryId: string }` | `sql { sql }`); `LlmTable { schema, name, ddl, indexes: Vec<String> }`; `LlmStats { since_reset: Option<QueryStat>, window_3600: Option<QueryStat> }`; `LlmBundle { server_version_num, pgss_version, in_recovery, member, database, text, text_truncated, stats, plan_text, tables, tables_omitted: usize, warnings: Vec<String> }`; `bundle(st, id, member, database, source, plan: Option<PlanTree>) -> AppResult<LlmBundle>`; `MAX_TABLES = 10`, `MAX_DDL_BYTES = 65_536`; чистая `pick_tables(relations: &[RelationRef]) -> (Vec<RelationRef>, usize)`, `fit_ddl(tables: Vec<LlmTable>) -> (Vec<LlmTable>, usize)`. Команда `query_llm_bundle(id, member, database, source, plan) -> LlmBundle`.

- [ ] **Step 1: Падающие тесты**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn rel(n: &str) -> RelationRef {
        RelationRef { schema: Some("shop".into()), name: n.into() }
    }

    #[test]
    fn at_most_ten_tables_and_the_rest_is_counted() {
        let rels: Vec<RelationRef> = (0..12).map(|i| rel(&format!("t{i}"))).collect();
        let (picked, omitted) = pick_tables(&rels);
        assert_eq!((picked.len(), omitted), (MAX_TABLES, 2));
        assert_eq!(picked[0].name, "t0");
    }

    #[test]
    fn ddl_budget_drops_whole_tables_from_the_end() {
        let big = "x".repeat(MAX_DDL_BYTES / 2 + 1);
        let tables = vec![
            LlmTable { schema: "shop".into(), name: "a".into(), ddl: big.clone(), indexes: vec![] },
            LlmTable { schema: "shop".into(), name: "b".into(), ddl: big.clone(), indexes: vec![] },
            LlmTable { schema: "shop".into(), name: "c".into(), ddl: "small".into(), indexes: vec!["CREATE INDEX i ON c (x)".into()] },
        ];
        let (kept, omitted) = fit_ddl(tables);
        assert_eq!((kept.len(), omitted), (1, 2), "вторая таблица не влезла, третья тоже не берётся (порядок плана важен)");
        assert_eq!(kept[0].name, "a");
    }

    #[test]
    fn source_json() {
        let s: LlmSource = serde_json::from_value(serde_json::json!({"kind": "query", "queryId": "-5"})).unwrap();
        assert_eq!(s, LlmSource::Query { query_id: -5 });
        let s: LlmSource = serde_json::from_value(serde_json::json!({"kind": "sql", "sql": "SELECT 1"})).unwrap();
        assert_eq!(s, LlmSource::Sql { sql: "SELECT 1".into() });
    }
}
```

`src-tauri/tests/it_explain.rs` — добавить:

```rust
use glafira_pg_lib::diag::queries::{QueriesMode, QueriesParams};
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::explain::llm::{self, LlmSource};
use pg_advisor::queries::QuerySort;

#[tokio::test]
async fn llm_bundle_has_text_stats_plan_and_ddl() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = member_of(&st).await;
    // Самый частый запрос pgbench — точечный выбор покупателя с $1.
    let r = diag::run(&st, "it", DiagRequest { tool: Tool::Queries, member: None, window_s: None,
        queries: Some(QueriesParams { mode: QueriesMode::SinceReset, sort: QuerySort::Calls, limit: 50, database: None, toplevel_only: true }) }).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    let qid = d.rows.iter().find(|x| x.rolname.as_deref() == Some("postgres")).map(|x| x.key.query_id).expect("строки pgbench");
    let b = llm::bundle(&st, "it", &m, "perf", LlmSource::Query { query_id: qid }, None).await.unwrap();
    assert!(b.text.as_deref().is_some_and(|t| !t.is_empty()));
    assert!(b.stats.as_ref().is_some_and(|s| s.since_reset.is_some()));
    assert!(b.plan_text.as_deref().is_some_and(|p| p.contains("(cost=")), "{:?}", b.plan_text);
    assert!(!b.tables.is_empty() && b.tables.iter().all(|t| t.ddl.contains("CREATE TABLE")), "{:?}", b.tables);
    assert!(b.server_version_num >= 170_000 && b.pgss_version.is_some());
    // Источник — SQL консоли: статистики нет, план и таблицы есть.
    let b = llm::bundle(&st, "it", &m, "perf", LlmSource::Sql { sql: "SELECT count(*) FROM shop.orders WHERE status = 'new'".into() }, None).await.unwrap();
    assert!(b.stats.is_none() && b.plan_text.is_some());
    assert_eq!(b.tables.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["orders"]);
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib explain::llm`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация `explain/llm.rs`**

```rust
//! Данные для «Скопировать для LLM» (спец. P3 §7.4): среда, текст, статистика, текст плана и DDL таблиц.
//! Markdown с заголовками на языке интерфейса собирает фронтенд (`src/lib/llmMarkdown.ts`).

use pg_advisor::plan::text_out::to_text;
use pg_advisor::plan::{PlanTree, RelationRef, relations};
use pg_advisor::queries::QueryStat;
use serde::{Deserialize, Serialize};

use crate::actions::Confirmation;
use crate::catalog::table;
use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::diag::caps::V16;
use crate::diag::query_detail;
use crate::error::AppResult;
use crate::explain::params::count_params;
use crate::explain::{ExplainRequest, Mode, Params, run};
use crate::state::AppState;

pub const MAX_TABLES: usize = 10;
pub const MAX_DDL_BYTES: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LlmSource {
    #[serde(rename_all = "camelCase")]
    Query {
        #[serde(with = "pg_advisor::serde_util::i64_string")]
        query_id: i64,
    },
    Sql {
        sql: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTable {
    pub schema: String,
    pub name: String,
    pub ddl: String,
    pub indexes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStats {
    pub since_reset: Option<QueryStat>,
    pub window_3600: Option<QueryStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmBundle {
    pub server_version_num: i32,
    pub pgss_version: Option<String>,
    pub in_recovery: bool,
    pub member: String,
    pub database: String,
    pub text: Option<String>,
    pub text_truncated: bool,
    pub stats: Option<LlmStats>,
    pub plan_text: Option<String>,
    pub tables: Vec<LlmTable>,
    pub tables_omitted: usize,
    /// Ключи интерфейса: `paramsNeeded` (PG < 16, текст с `$N` без значений — плана нет), `queryGone`, `planFailed`.
    pub warnings: Vec<String>,
}

/// Первые `MAX_TABLES` отношений плана и сколько осталось за бортом.
pub fn pick_tables(rels: &[RelationRef]) -> (Vec<RelationRef>, usize) {
    let picked: Vec<RelationRef> = rels.iter().take(MAX_TABLES).cloned().collect();
    (picked, rels.len().saturating_sub(MAX_TABLES))
}

/// Укладывает DDL в бюджет: таблицы в порядке плана, первая не влезшая и все за ней отбрасываются.
pub fn fit_ddl(tables: Vec<LlmTable>) -> (Vec<LlmTable>, usize) {
    let mut used = 0usize;
    let mut kept = Vec::new();
    let total = tables.len();
    for t in tables {
        let size = t.ddl.len() + t.indexes.iter().map(|i| i.len() + 1).sum::<usize>();
        if used + size > MAX_DDL_BYTES {
            break;
        }
        used += size;
        kept.push(t);
    }
    let omitted = total - kept.len();
    (kept, omitted)
}

pub async fn bundle(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    source: LlmSource,
    plan: Option<PlanTree>,
) -> AppResult<LlmBundle> {
    let mut warnings = Vec::new();
    let (text, text_truncated, stats, version, in_recovery, pgss_version) = match &source {
        LlmSource::Query { query_id } => {
            let d = query_detail::detail(st, id, member, database, *query_id).await?;
            if d.text.is_none() {
                warnings.push("queryGone".to_string());
            }
            let window_3600 = d.windows.iter().find(|w| w.window_s == 3600).and_then(|w| w.stat.clone());
            (
                d.text.clone(),
                d.text_truncated,
                Some(LlmStats { since_reset: d.since_reset.clone(), window_3600 }),
                d.server_version_num,
                d.in_recovery,
                d.pgss_version.clone(),
            )
        }
        LlmSource::Sql { sql } => {
            let target = st.runtimes.diag_target(id, Some(member), false)?;
            let h = crate::diag::on_member(st, id, &target, |c| Box::pin(crate::diag::head(c))).await?;
            (Some(sql.clone()), false, None, h.version_num, h.in_recovery, None)
        }
    };
    // План: переданный, иначе пробный EXPLAIN без выполнения (GENERIC_PLAN на 16+ при `$N`).
    let plan = match (plan, &text) {
        (Some(p), _) => Some(p),
        (None, Some(sql)) => {
            let n = count_params(sql);
            let params = if n == 0 {
                Some(Params::None)
            } else if version >= V16 {
                Some(Params::Generic)
            } else {
                warnings.push("paramsNeeded".to_string());
                None
            };
            match params {
                Some(params) => {
                    let req = ExplainRequest {
                        member: member.to_string(),
                        database: database.to_string(),
                        sql: sql.clone(),
                        mode: Mode::Plan,
                        params,
                        timeout_s: crate::explain::DEFAULT_TIMEOUT_S,
                        search_path: None,
                        confirmation: Confirmation::default(),
                        force_write: false,
                    };
                    match run::run(st, id, &req).await {
                        Ok(r) => Some(r.plan),
                        Err(_) => {
                            warnings.push("planFailed".to_string());
                            None
                        }
                    }
                }
                None => None,
            }
        }
        (None, None) => None,
    };
    let (tables, tables_omitted) = match &plan {
        Some(p) => {
            let (picked, omitted_by_count) = pick_tables(&relations(p));
            let (_, profile) = st.runtimes.target(id, Some(member))?;
            let fetched: Vec<LlmTable> = on_catalog_node_within(
                catalog_limit(&profile),
                st.client(id, Some(member), Some(database)),
                |c| {
                    Box::pin(async move {
                        let mut out = Vec::new();
                        for r in &picked {
                            let schema = r.schema.clone().unwrap_or_else(|| "public".to_string());
                            // Отношение могло быть не таблицей (представление, партиция другой схемы): пропускается.
                            if let Ok(t) = table::get(c, &schema, &r.name).await {
                                out.push(LlmTable {
                                    schema,
                                    name: r.name.clone(),
                                    indexes: t.indexes.iter().map(|i| i.definition.clone()).collect(),
                                    ddl: t.ddl,
                                });
                            }
                        }
                        Ok(out)
                    })
                },
            )
            .await?;
            let (kept, omitted_by_size) = fit_ddl(fetched);
            (kept, omitted_by_count + omitted_by_size)
        }
        None => (Vec::new(), 0),
    };
    Ok(LlmBundle {
        server_version_num: version,
        pgss_version,
        in_recovery,
        member: member.to_string(),
        database: database.to_string(),
        text,
        text_truncated,
        stats,
        plan_text: plan.as_ref().map(to_text),
        tables,
        tables_omitted,
        warnings,
    })
}
```

(`on_catalog_node_within` и `catalog_limit` — `pub(crate)`, доступны внутри крейта.)

`src-tauri/src/explain/mod.rs` — `pub mod llm;`. `src-tauri/src/commands/queries.rs`:

```rust
use pg_advisor::plan::PlanTree;

use crate::explain::llm::{self, LlmBundle, LlmSource};

/// Данные для «Скопировать для LLM»: markdown собирает фронтенд.
#[tauri::command]
pub async fn query_llm_bundle(
    state: State<'_, AppState>,
    id: String,
    member: String,
    database: String,
    source: LlmSource,
    plan: Option<PlanTree>,
) -> AppResult<LlmBundle> {
    llm::bundle(&state, &id, &member, &database, source, plan).await
}
```

`src-tauri/src/lib.rs` — `commands::queries::query_llm_bundle,`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check`
Expected: PASS.

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_explain llm_bundle -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri
git commit -m "feat(explain): данные для «Скопировать для LLM» — текст, статистика, текст плана и DDL таблиц из плана

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 11: Готовность — `track_planning`, права чтения для EXPLAIN, версия pgss

**Files:**
- Modify: `crates/pg-advisor/src/readiness.rs`, `src-tauri/src/readiness/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты `readiness.rs` (pg-advisor)

**Interfaces:**
- Produces: `ReadinessFacts.pgss_version: Option<String>`, `ReadinessFacts.pgss_track_planning: Option<String>`, `ReadinessFacts.read_all_data: bool` (все с `#[serde(default)]`); пункты `pgssTrackPlanning` (есть только при установленном pgss: `Ok` при `on`, иначе `Warn`, fix `pg_stat_statements.track_planning = on`) и `readAllData` (`Ok` — суперпользователь или член `pg_read_all_data`; 14+ без роли — `Warn` с `GRANT pg_read_all_data TO <user>;`; 13 — `Warn` с `reason: version`, без fix). Оба — перед пунктами расширений.

- [ ] **Step 1: Падающие тесты**

В `crates/pg-advisor/src/readiness.rs` `mod tests`: в `facts()` добавить `pgss_version: Some("1.11".into()), pgss_track_planning: Some("on".into()), read_all_data: true,` (и в `member_of` — `"pg_read_all_data".into()`); новые тесты:

```rust
    #[test]
    fn track_planning_item_follows_the_setting_and_needs_pgss() {
        let items = evaluate(&facts());
        assert_eq!(get(&items, "pgssTrackPlanning").status, Status::Ok);
        let mut f = facts();
        f.pgss_track_planning = Some("off".into());
        let i = get(&evaluate(&f), "pgssTrackPlanning").clone();
        assert_eq!((i.status, i.fix.as_deref()), (Status::Warn, Some("pg_stat_statements.track_planning = on")));
        f.pgss_installed = false;
        f.pgss_track_planning = None;
        assert!(evaluate(&f).iter().all(|i| i.id != "pgssTrackPlanning"), "без pgss пункт не показывается");
    }

    #[test]
    fn read_all_data_item_depends_on_version_and_membership() {
        assert_eq!(get(&evaluate(&facts()), "readAllData").status, Status::Ok);
        let mut f = facts();
        f.read_all_data = false;
        let i = get(&evaluate(&f), "readAllData").clone();
        assert_eq!((i.status, i.fix.as_deref()), (Status::Warn, Some("GRANT pg_read_all_data TO \"glafira_diag\";")));
        f.server_version_num = 130_000;
        let i = get(&evaluate(&f), "readAllData").clone();
        assert_eq!((i.status, i.params.get("reason").map(String::as_str), i.fix), (Status::Warn, Some("version"), None));
        f.is_superuser = true;
        assert_eq!(get(&evaluate(&f), "readAllData").status, Status::Ok);
    }
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml readiness`
Expected: ошибка компиляции (полей нет).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/readiness.rs` — в `ReadinessFacts` после `pgss_installed`:

```rust
    /// `pg_extension.extversion` расширения (P3): колонки топа запросов зависят от неё.
    #[serde(default)]
    pub pgss_version: Option<String>,
    /// `pg_stat_statements.track_planning`; `None` — настройка не видна (библиотека не загружена).
    #[serde(default)]
    pub pgss_track_planning: Option<String>,
    /// Член `pg_read_all_data` (14+): EXPLAIN запросов приложения читает его таблицы.
    #[serde(default)]
    pub read_all_data: bool,
```

в `evaluate` перед циклом `for name in OPTIONAL_EXTENSIONS`:

```rust
    if f.pgss_installed {
        out.push(if f.pgss_track_planning.as_deref() == Some("on") {
            item("pgssTrackPlanning", Status::Ok, &[], None)
        } else {
            item(
                "pgssTrackPlanning",
                Status::Warn,
                &[],
                Some("pg_stat_statements.track_planning = on".into()),
            )
        });
    }
    out.push(if f.is_superuser || f.read_all_data {
        item("readAllData", Status::Ok, &[], None)
    } else if f.server_version_num >= 140_000 {
        item("readAllData", Status::Warn, &[], grant("pg_read_all_data"))
    } else {
        item(
            "readAllData",
            Status::Warn,
            &[("reason", "version".into())],
            None,
        )
    });
```

`src-tauri/src/readiness/mod.rs` `collect_facts`: в запрос `member_of` добавить `'pg_read_all_data'` в список `IN (...)`; `pgss_installed` заменить на чтение версии:

```rust
    let pgss_version: Option<String> = c
        .query_opt(
            "SELECT extversion::text FROM pg_extension WHERE extname = 'pg_stat_statements'",
            &[],
        )
        .await
        .map_err(pg)?
        .map(|r| r.get(0));
    let pgss_installed = pgss_version.is_some();
```

в запрос настроек добавить `'pg_stat_statements.track_planning'` и переменную `let mut pgss_track_planning = None;` с ветвью `"pg_stat_statements.track_planning" => pgss_track_planning = Some(setting),`; в литерал фактов — `pgss_version, pgss_track_planning, read_all_data: member_of.iter().any(|r| r == "pg_read_all_data"),`. Тесты `readiness/mod.rs` и фикстуры фактов в `src-tauri` (grep `pgss_installed:`) получают новые поля (`pgss_version: None, pgss_track_planning: None, read_all_data: false` — или через `..Default::default()`).

Словари `readiness` (en / ru):
- `pgssTrackPlanning`: `title` "pg_stat_statements.track_planning" / то же; `ok` "On" / "Включён"; `warn` "Off" / "Выключен"; `affects` "Planning time columns in the query top stay empty; the “planning heavier than execution” hint is silent" / "Колонки планирования в топе запросов пустые; подсказка «планирование дольше выполнения» молчит".
- `readAllData`: `title` "Reading application tables (EXPLAIN)" / "Чтение таблиц приложения (EXPLAIN)"; `ok` "Allowed" / "Разрешено"; `warn` "The role is not a member of pg_read_all_data" / "Роль не входит в pg_read_all_data"; `version` "PostgreSQL 13: grant SELECT on the tables of the queries you want to explain" / "PostgreSQL 13: выдайте SELECT на таблицы запросов, которые нужно объяснять"; `affects` "EXPLAIN of application queries fails with “permission denied”" / "EXPLAIN запросов приложения падает с «permission denied»".

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib readiness && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && pnpm i18n:check && pnpm test -- TestPanel`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor/src/readiness.rs src-tauri/src/readiness src/locales
git commit -m "feat(readiness): пункты track_planning и прав чтения для EXPLAIN, версия pg_stat_statements в фактах

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 12: Фронтенд — типы, клиент, инструмент «Запросы» (`QueriesView`), сэмплер pgss, виды вкладок

**Files:**
- Create: `src/lib/useStatementsSampler.ts`, `src/lib/useQueriesReport.ts`, `src/components/queries/QueriesView.tsx`, `src/components/queries/QueriesView.test.tsx`, `src/components/queries/queriesText.ts`
- Modify: `src/api/types.ts`, `src/api/client.ts`, `src/state/workspace.ts`, `src/components/Layout.tsx`, `src/components/help/helpChapterFor.ts`, `src/lib/format.ts` (+ `formatMs`), `src/components/diag/findingText.ts`, `src/components/diag/FindingsPanel.tsx` (`targetKey`), `src/components/diag/unavailable.ts`, `src/components/diag/DiagHeader.tsx` (`TOOL_HINTS`), `src/components/diag/DiagTab.tsx`, `src/components/connections/tree/DiagnosticsNodes.tsx`, `src/lib/useDiagnostic.ts` (`rustTool`), `src/locales/{en,ru}.json`, `docs/i18n/same-as-en.json`, `src/lib/format.test.ts`
- Test: `QueriesView.test.tsx`, `format.test.ts`, существующие тесты диагностики

**Interfaces:**
- Consumes: команды задач 4, 6 (`run_diagnostic` с `queries`, `query_texts`, `start_statements_sampler`), `DiagHeader`, `FindingsPanel`, `useWorkspace`.
- Produces: типы `QueryKey`, `QueryStat`, `QuerySort`, `QueriesMode`, `QueriesParams`, `PgssMeta`, `QueriesData`, `QueryText`, `StatementsEvent`, `RunningInstance`, `QueryWaits`, `ByUser`, `WindowStat`, `HistoryPoint`, `QueryDetail`, `PlanTree`, `PlanNode`, `Buffers`, `Jit`, `Trigger`, `Setting`, `RelationRef`, `ExplainMode`, `ExplainParams`, `ExplainRequest`, `ExplainTier`, `ExplainResult`, `LlmSource`, `LlmTable`, `LlmStats`, `LlmBundle`, `ExplainRecord`; `DiagTool` с `"queries"`; `FindingTarget` `query`, `FindingLink` `openQuery`; `ErrorCode` `EXPLAIN_WRITES`; `TabSpec` виды `query { connectionId, member, database, queryId, title? }` и `explain { connectionId, member?, database?, sql? }`; `api.{queryTexts, startStatementsSampler, queryDetail, explain, parsePlan, queryLlmBundle}`; `useStatementsSampler(connectionId, member, enabled) -> { live: StatementsLive | null, error }`; `useQueriesReport(connectionId, member, params, opts)`; `QueriesView` (пропсы `connectionId, member, active, live, members, kind, onSelectMember`); `formatMs(ms)`; `sortAvailable(sort, pgssVersion, window)`; `queryTitle(text, queryId)`.

- [ ] **Step 1: Падающие тесты**

`src/lib/format.test.ts` — добавить:

```ts
import { formatMs } from "./format";

describe("formatMs", () => {
  it("миллисекунды с точностью по порядку, секунды от 1000 мс, длительность от минуты", () => {
    expect(formatMs(0.004)).toBe("<0,01 мс");
    expect(formatMs(0.42)).toBe("0,42 мс");
    expect(formatMs(12.34)).toBe("12,3 мс");
    expect(formatMs(999)).toBe("999 мс");
    expect(formatMs(1250)).toBe("1,25 с");
    expect(formatMs(90_000)).toBe("1 мин 30 с");
  });
});
```

`src/components/queries/QueriesView.test.tsx`:

```tsx
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueriesData, QueryStat, StatementsEvent } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { diagReport } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { runDiagnostic: vi.fn(), queryTexts: vi.fn(), startStatementsSampler: vi.fn(), stopSession: vi.fn() } };
});

import { api } from "../../api/client";
import { QueriesView } from "./QueriesView";

let emit: (e: StatementsEvent) => void;

function row(id: string, patch: Partial<QueryStat> = {}): QueryStat {
  return {
    key: { queryId: id, dbid: 5, userid: 10, toplevel: true }, datname: "perf", rolname: "app",
    calls: 100, totalExecMs: 1000, meanExecMs: 10, minExecMs: 1, maxExecMs: 50, stddevExecMs: 5, rows: 100, rowsPerCall: 1,
    plans: 0, totalPlanMs: 0, sharedHit: 900, sharedRead: 100, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0,
    tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0,
    jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 50, hitRatio: 0.9, text: null, textTruncated: false, ...patch,
  };
}

function data(patch: Partial<QueriesData> = {}): { kind: "queries" } & QueriesData {
  return {
    kind: "queries", mode: { kind: "sinceReset" }, coverageS: null, ticks: null,
    pgss: { version: "1.11", max: 5000, trackPlanning: true, statsResetMs: 1_700_000_000_000, dealloc: 0 },
    totals: { calls: 200, totalExecMs: 2000, rows: 200 }, rows: [row("1"), row("2", { totalExecMs: 500, sharePct: 25, text: "select 2" })], ...patch,
  };
}

describe("QueriesView", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    emit = () => {};
    vi.mocked(api.startStatementsSampler).mockImplementation(async (_id, _m, onEvent) => { emit = onEvent; return "s-1"; });
    vi.mocked(api.stopSession).mockResolvedValue(true);
    vi.mocked(api.queryTexts).mockResolvedValue([{ key: { queryId: "1", dbid: 5, userid: 10, toplevel: true }, text: "select 1 from t", truncated: false }]);
  });

  const view = (member: string | null = null) => <QueriesView connectionId="c" member={member} active live members={null} kind="server" onSelectMember={() => {}} />;

  it("«с момента сброса»: строки, тексты подгружаются по ключам без текста, подвал с версией pgss", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    renderWithClient(view());
    expect(await screen.findByText("select 2")).toBeInTheDocument();
    await waitFor(() => expect(api.queryTexts).toHaveBeenCalledWith("c", "pg-1", [{ queryId: "1", dbid: 5, userid: 10, toplevel: true }]));
    expect(await screen.findByText("select 1 from t")).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "queries", member: null, queries: { mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true } });
    expect(screen.getByText(/pg_stat_statements 1\.11/)).toBeInTheDocument();
    expect(api.startStatementsSampler).toHaveBeenCalledWith("c", null, expect.any(Function));
  });

  it("окно: покрытие из отчёта, обновление по тику сэмплера, stddev недоступен", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ mode: { kind: "window", s: 300 }, coverageS: 120, ticks: 2, rows: [row("1", { stddevExecMs: null, maxExecMs: null })] }), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    await screen.findByRole("table", { name: "Запросы" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Режим" }), "window:300");
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ mode: { kind: "window", s: 300 } }) })));
    expect(await screen.findByText("Накоплено 2 из 5 мин")).toBeInTheDocument();
    const calls = vi.mocked(api.runDiagnostic).mock.calls.length;
    act(() => emit({ kind: "tick", atMs: 5, coverageS: 180, ticks: 3, changed: 4 }));
    await waitFor(() => expect(vi.mocked(api.runDiagnostic).mock.calls.length).toBeGreaterThan(calls));
    expect(screen.getByRole("columnheader", { name: /Stddev/ }).querySelector("button")).toBeDisabled();
    expect(screen.getByRole("columnheader", { name: /Среднее/ }).querySelector("button")).toBeEnabled();
  });

  it("клик по заголовку меняет сортировку; клик по строке открывает вкладку запроса с узлом отчёта", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data(), { member: "pg-1" }));
    const user = userEvent.setup();
    renderWithClient(view());
    const table = await screen.findByRole("table", { name: "Запросы" });
    await user.click(within(screen.getByRole("columnheader", { name: /Вызовов/ })).getByRole("button"));
    await waitFor(() => expect(api.runDiagnostic).toHaveBeenLastCalledWith("c", expect.objectContaining({ queries: expect.objectContaining({ sort: "calls" }) })));
    await user.click(within(table).getByRole("button", { name: /select 2/ }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "query", connectionId: "c", member: "pg-1", database: "perf", queryId: "2", title: "select 2" });
  });

  it("без pg_stat_statements — строка «недоступно», таблицы нет", async () => {
    vi.mocked(api.runDiagnostic).mockResolvedValue(diagReport(data({ pgss: null, rows: [] }), {
      unavailable: [{ part: "statements", reason: { key: "diag.unavailable.statementsMissing", params: { part: "statements" }, text: "statements: pg_stat_statements is not installed in this database" } }],
    }));
    renderWithClient(view());
    expect(await screen.findByText("Статистика запросов (pg_stat_statements): pg_stat_statements не установлен в этой базе")).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "Запросы" })).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- QueriesView format`
Expected: FAIL (модулей нет, `formatMs` нет).

- [ ] **Step 3: Типы и клиент**

`src/api/types.ts`:
- в `ErrorCode` перед `"INTERNAL"`: `| "EXPLAIN_WRITES"`.
- `DiagTool`: добавить `"queries"`; `RustTool` остаётся `Exclude<DiagTool, "idleTx">`.
- `FindingTarget`: `| { kind: "query"; queryId: string; database: string }`; `FindingLink`: `| { kind: "openQuery"; queryId: string; database: string; member: string | null }`.
- `DiagRequest`: `/** Только `queries`. */ queries?: QueriesParams;`.
- после `DiagRequest` — блок типов P3a:

```ts
// ---------------------------------------------------------------------------------------------------------------
// Тяжёлые запросы (P3a): зеркало `pg_advisor::{queries, plan}`, `diag::{queries, query_detail}`, `explain`, `sampler::statements`.
// `queryId` — всегда строка (64-битный хеш не помещается в number).
// ---------------------------------------------------------------------------------------------------------------

export interface QueryKey { queryId: string; dbid: number; userid: number; toplevel: boolean }
export type QuerySort = "totalTime" | "meanTime" | "maxTime" | "stddevTime" | "calls" | "rowsPerCall" | "sharedRead" | "hitRatio" | "tempWritten" | "walBytes" | "ioTime" | "planTime" | "jitTime";
export type QueriesMode = { kind: "sinceReset" } | { kind: "window"; s: 300 | 900 | 3600 } | { kind: "between"; from: string; to: string };
export interface QueriesParams { mode: QueriesMode; sort: QuerySort; limit: 50 | 100 | 200; database: string | null; toplevelOnly: boolean }
export interface QueryStat {
  key: QueryKey; datname: string | null; rolname: string | null;
  calls: number; totalExecMs: number; meanExecMs: number; minExecMs: number | null; maxExecMs: number | null; stddevExecMs: number | null;
  rows: number; rowsPerCall: number; plans: number; totalPlanMs: number;
  sharedHit: number; sharedRead: number; sharedDirtied: number; sharedWritten: number;
  localHit: number; localRead: number; localDirtied: number; localWritten: number;
  tempRead: number; tempWritten: number; ioReadMs: number; ioWriteMs: number; tempIoMs: number | null;
  walRecords: number; walFpi: number; walBytes: number;
  jitFunctions: number | null; jitMs: number | null; parallelPlanned: number | null; parallelLaunched: number | null;
  /** Доля в `totals.totalExecMs`, проценты; `hitRatio` — 0..1 или null без чтений. */
  sharePct: number; hitRatio: number | null;
  text: string | null; textTruncated: boolean;
}
export interface PgssMeta { version: string; max: number; trackPlanning: boolean; statsResetMs: number | null; dealloc: number | null }
export interface QueriesData {
  mode: QueriesMode; coverageS: number | null; ticks: number | null; pgss: PgssMeta | null;
  totals: { calls: number; totalExecMs: number; rows: number }; rows: QueryStat[];
}
export interface QueryText { key: QueryKey; text: string | null; truncated: boolean }
/** События сэмплера pg_stat_statements (канал `start_statements_sampler`). */
export type StatementsEvent =
  | { kind: "tick"; atMs: number; coverageS: number; ticks: number; changed: number }
  | { kind: "gap"; fromMs: number; toMs: number }
  | { kind: "error"; error: AppError }
  | { kind: "stopped" };

export interface RunningInstance {
  pid: number; backendStartUs: number; state: string | null; startedMs: number | null; durationS: number | null;
  waitEventType: string | null; waitEvent: string | null; usename: string | null; applicationName: string | null; clientAddr: string | null;
  query: string; truncated: boolean;
}
export interface QueryWaits { source: "sampler" | "pgWaitSampling"; sinceMs: number; total: number; items: { eventType: string; event: string | null; count: number; sharePct: number }[] }
export interface ByUser { rolname: string | null; toplevel: boolean; stat: QueryStat }
export interface WindowStat { windowS: 300 | 900 | 3600; stat: QueryStat | null }
export interface HistoryPoint { atMs: number; calls: number; totalExecMs: number; meanExecMs: number }
export interface QueryDetail {
  text: string | null; textTruncated: boolean; sinceReset: QueryStat | null; windows: WindowStat[];
  totals: { calls: number; totalExecMs: number; rows: number }; byUser: ByUser[]; history: HistoryPoint[]; gaps: { fromMs: number; toMs: number }[];
  running: RunningInstance[]; waits: QueryWaits | null; findings: Finding[]; unavailable: Unavailable[];
  serverVersionNum: number; inRecovery: boolean; pgssVersion: string | null;
}

export interface RelationRef { schema: string | null; name: string }
export interface Buffers { sharedHit: number; sharedRead: number; sharedDirtied: number; sharedWritten: number; localHit: number; localRead: number; localDirtied: number; localWritten: number; tempRead: number; tempWritten: number }
export interface Jit { functions: number; generationMs: number; inliningMs: number; optimizationMs: number; emissionMs: number; totalMs: number }
export interface Trigger { name: string; relation: string | null; timeMs: number; calls: number }
export interface Setting { name: string; value: string }
export interface PlanNode {
  id: number; nodeType: string; parentRelationship: string | null; subplanName: string | null;
  relation: { schema: string | null; name: string; alias: string | null } | null; indexName: string | null; strategy: string | null; joinType: string | null; parallelAware: boolean;
  startupCost: number; totalCost: number; planRows: number; planWidth: number;
  actual: { startupMs: number; totalMs: number; rows: number; loops: number } | null; neverExecuted: boolean;
  workersPlanned: number | null; workersLaunched: number | null;
  rowsRemovedByFilter: number | null; rowsRemovedByIndexRecheck: number | null; rowsRemovedByJoinFilter: number | null; heapFetches: number | null;
  filter: string | null; indexCond: string | null; recheckCond: string | null; hashCond: string | null; joinFilter: string | null; mergeCond: string | null;
  sortKey: string[]; groupKey: string[]; sortMethod: string | null; sortSpaceKb: number | null; sortSpaceType: string | null;
  hashBatches: number | null; hashBuckets: number | null; peakMemoryKb: number | null; exactHeapBlocks: number | null; lossyHeapBlocks: number | null; subplansRemoved: number | null;
  buffers: Buffers | null; ioReadMs: number | null; ioWriteMs: number | null; output: string[]; extra: Record<string, unknown>;
  inclusiveMs: number | null; exclusiveMs: number | null; sharePct: number | null; estimateRatio: number | null; hot: boolean; misestimated: boolean;
  children: PlanNode[];
}
export interface PlanTree { root: PlanNode; planningMs: number | null; executionMs: number | null; triggers: Trigger[]; jit: Jit | null; settings: Setting[]; totalMs: number | null }

export type ExplainMode = "plan" | "analyze";
export type ExplainParams = { kind: "none" } | { kind: "generic" } | { kind: "values"; values: (string | null)[] };
export type ExplainTier = "read" | "write";
export interface ExplainRequest {
  member: string; database: string; sql: string; mode: ExplainMode; params: ExplainParams;
  /** Только ANALYZE, 5…600 с. */ timeoutS: number; searchPath: string | null; confirmation: Confirmation;
  /** Выполнить как запись, хотя план — чтение (после `EXPLAIN_WRITES`). */ forceWrite: boolean;
}
export interface ExplainResult { mode: ExplainMode; tier: ExplainTier; plan: PlanTree; raw: unknown; relations: RelationRef[]; elapsedMs: number; warnings: string[] }

export type LlmSource = { kind: "query"; queryId: string } | { kind: "sql"; sql: string };
export interface LlmTable { schema: string; name: string; ddl: string; indexes: string[] }
export interface LlmStats { sinceReset: QueryStat | null; window3600: QueryStat | null }
export interface LlmBundle {
  serverVersionNum: number; pgssVersion: string | null; inRecovery: boolean; member: string; database: string;
  text: string | null; textTruncated: boolean; stats: LlmStats | null; planText: string | null; tables: LlmTable[]; tablesOmitted: number; warnings: string[];
}
export interface ExplainRecord { database: string; tables: string[]; timeoutS: number; sqlHead: string }
```

- в объединение `DiagData` добавить `| ({ kind: "queries" } & QueriesData)`; в `ActionRecord` — `explain?: ExplainRecord;` (после `patroni`); тип `Confirmation` объявлен ниже — TypeScript допускает ссылку вперёд.

`src/api/client.ts` — после `stopSession`:

```ts
  queryTexts: (id: string, member: string | null, keys: QueryKey[]) => call<QueryText[]>("query_texts", { id, member, keys }),
  /** Подписка на сэмплер pg_stat_statements узла; итог — id сессии для `stopSession`. */
  startStatementsSampler: (id: string, member: string | null, onEvent: (e: StatementsEvent) => void) => {
    const channel = new Channel<StatementsEvent>();
    channel.onmessage = onEvent;
    return call<string>("start_statements_sampler", { id, member, channel });
  },
  queryDetail: (id: string, member: string, database: string, queryId: string) => call<QueryDetail>("query_detail", { id, member, database, queryId }),
  explain: (id: string, request: ExplainRequest) => call<ExplainResult>("explain", { id, request }),
  parsePlan: (text: string) => call<PlanTree>("parse_plan", { text }),
  queryLlmBundle: (id: string, member: string, database: string, source: LlmSource, plan: PlanTree | null) =>
    call<LlmBundle>("query_llm_bundle", { id, member, database, source, plan }),
```

(импорты типов — дополнить.)

- [ ] **Step 4: Вкладки, формат, находки, недоступное**

`src/state/workspace.ts` — в `TabSpec` после `diag`:

```ts
  /** Карточка запроса pgss: узел, база и queryId; title — начало текста для заголовка вкладки (не часть id). */
  | { kind: "query"; connectionId: string; member: string; database: string; queryId: string; title?: string }
  /** Консоль EXPLAIN — одна на подключение; member/database/sql — подстановка при открытии из карточки. */
  | { kind: "explain"; connectionId: string; member?: string | null; database?: string; sql?: string }
```

в `tabId`: `case "query": return \`query:${spec.connectionId}:${spec.member}:${spec.database}:${spec.queryId}\`;` и `case "explain": return \`explain:${spec.connectionId}\`;`.

`src/components/Layout.tsx` `tabTitle`: `case "query": return i18n.t("tabs.query", { text: tab.title ?? tab.queryId, conn });` и `case "explain": return i18n.t("tabs.explain", { conn });`.

`src/components/help/helpChapterFor.ts`: `case "query": case "explain":` → `"diagnostics"` (глава «Запросы» появится в задаче 16 и переключит на `"queries"`); `helpAnchorFor`: для `query` и `explain` — `"queries"`.

`src/lib/format.ts` — добавить:

```ts
/** Миллисекунды: до 10 — два знака, до 1000 — один, дальше секунды с двумя знаками, от минуты — длительность. */
export function formatMs(ms: number): string {
  if (ms >= 60_000) return formatDuration(ms / 1000);
  if (ms >= 1000) return `${num(ms / 1000, 2)} ${i18n.t("diag.duration.secShort")}`;
  if (ms >= 100) return `${num(ms, 0)} ${i18n.t("diag.duration.msShort")}`;
  if (ms >= 10) return `${num(ms, 1)} ${i18n.t("diag.duration.msShort")}`;
  if (ms >= 0.01) return `${num(ms, 2)} ${i18n.t("diag.duration.msShort")}`;
  return `<0,01 ${i18n.t("diag.duration.msShort")}`.replace("0,01", num(0.01, 2));
}
```

(словари `diag.duration.msShort`: en `"ms"`, ru `"мс"`; `diag.duration.secShort`: en `"s"`, ru `"с"`; `formatDuration(90)` даёт «1 мин 30 с» по существующим ключам).

`src/components/diag/findingText.ts` — в цикле параметров перед веткой `endsWith("S")`: `else if (typeof v === "number" && k.endsWith("Ms")) out[k] = formatMs(v);` (импорт `formatMs`); после `pid`: `else if (f.target?.kind === "query") { out.queryId = f.target.queryId; out.database = f.target.database; }` — и условие `else if (f.target && !("name" in out)) out.name = f.target.name;` ограничить целями с `name` (`f.target.kind !== "query"`).

`src/components/diag/FindingsPanel.tsx` — `targetKey`: `t.kind === "session" ? … : t.kind === "query" ? \`${t.database}:${t.queryId}\` : t.name`; фильтр `links` и кнопки `openQuery` — задача 14 (вкладка `query` появляется там); пока ссылка `openQuery` исключается из `links` фильтром `l.kind !== "openQuery"`, чтобы код компилировался и ничего не рисовал.

`src/components/diag/unavailable.ts` — в `UNAVAILABLE_PARTS`: `statements: "diag.unavailable.parts.statements", others: "diag.unavailable.parts.others", window: "diag.unavailable.parts.window", running: "diag.unavailable.parts.running"`.

`src/components/diag/DiagHeader.tsx` — `TOOL_HINTS.queries: "hints.diag.queries"`.

`src/components/connections/tree/DiagnosticsNodes.tsx` — в `common` после `"slots"`: `"queries"`.

- [ ] **Step 5: Хуки и `QueriesView`**

`src/lib/useStatementsSampler.ts`:

```ts
import { useEffect, useState } from "react";
import { api, toAppError } from "../api/client";
import type { AppError } from "../api/types";
import { useConnectionEpoch } from "../state/connectionEpoch";
import { RESUBSCRIBE_DELAY_MS } from "./useSampler";

/** Последний тик сэмплера pgss: покрытие часового окна и число тиков с дельтами. */
export interface StatementsLive {
  atMs: number;
  coverageS: number;
  ticks: number;
}

function stop(session: string) {
  void Promise.resolve(api.stopSession(session)).catch(() => {});
}

/**
 * Подписка на сэмплер pg_stat_statements узла (спец. P3 §5): та же дисциплина, что у `useSampler` (держится, пока
 * вкладка смонтирована; после `stopped` не по отказу во входе — повтор через паузу; переподключение перезапускает).
 * `member = null` — «за лидером»: узел выбирает Rust.
 */
export function useStatementsSampler(connectionId: string, member: string | null, enabled: boolean): { live: StatementsLive | null; error: AppError | null } {
  const [live, setLive] = useState<StatementsLive | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [round, setRound] = useState(0);
  const epoch = useConnectionEpoch(connectionId);
  useEffect(() => {
    if (!enabled) return;
    let session: string | null = null;
    let cancelled = false;
    let stopped = false;
    let lastError: AppError | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    setLive(null);
    setError(null);
    api
      .startStatementsSampler(connectionId, member, (e) => {
        if (cancelled || stopped) return;
        if (e.kind === "tick") {
          setLive({ atMs: e.atMs, coverageS: e.coverageS, ticks: e.ticks });
          lastError = null;
          setError(null);
        } else if (e.kind === "error") {
          lastError = e.error;
          setError(e.error);
        } else if (e.kind === "stopped") {
          stopped = true;
          if (lastError?.code !== "AUTH_FAILED" && lastError?.code !== "FEATURE_UNAVAILABLE") retry = setTimeout(() => setRound((n) => n + 1), RESUBSCRIBE_DELAY_MS);
        }
      })
      .then((id) => {
        if (cancelled) stop(id);
        else session = id;
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(toAppError(e));
      });
    return () => {
      cancelled = true;
      clearTimeout(retry);
      if (session) stop(session);
    };
  }, [connectionId, member, enabled, round, epoch]);
  return { live, error };
}
```

`src/lib/useQueriesReport.ts`:

```ts
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "../api/client";
import type { QueriesData, QueriesParams, DiagReport } from "../api/types";
import { diagRefetchInterval } from "./useDiagnostic";

export type QueriesReport = DiagReport<{ kind: "queries" } & QueriesData>;

/** Ключ запроса отчёта: первый элемент `diag` — его сбрасывает `refreshAfterConnect`. */
export const queriesKey = (connectionId: string, member: string | null, params: QueriesParams) => ["diag", connectionId, "queries", member ?? "", params] as const;

/**
 * Отчёт «Запросы» (спец. P3 §3.2): «с момента сброса» перечитывается по интервалу шапки, «за окно» — по тику сэмплера
 * (`tickAtMs`), потому что без нового тика буфер не меняется.
 */
export function useQueriesReport(
  connectionId: string,
  member: string | null,
  params: QueriesParams,
  opts: { active: boolean; live: boolean; intervalS: number | null; tickAtMs: number | null },
) {
  const window = params.mode.kind === "window";
  const q = useQuery({
    queryKey: queriesKey(connectionId, member, params),
    queryFn: async () => (await api.runDiagnostic(connectionId, { tool: "queries", member, queries: params })) as QueriesReport,
    enabled: opts.live,
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (window ? false : diagRefetchInterval(opts.active, opts.intervalS, query.state.error)),
  });
  const { refetch } = q;
  useEffect(() => {
    if (window && opts.live && opts.active && opts.tickAtMs !== null) void refetch({ cancelRefetch: false });
  }, [window, opts.live, opts.active, opts.tickAtMs, refetch]);
  return q;
}
```

`src/components/queries/queriesText.ts`:

```ts
import type { PgssMeta, QuerySort } from "../../api/types";

/** Заголовок вкладки и первая строка таблицы: начало текста одной строкой; без текста — queryId. */
export function queryTitle(text: string | null, queryId: string, max = 60): string {
  if (!text) return queryId;
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

/** Версия pgss как пара чисел; `null` — не разобралась. */
export function pgssVersion(v: string | null | undefined): [number, number] | null {
  const m = /^(\d+)\.(\d+)/.exec(v ?? "");
  return m ? [Number(m[1]), Number(m[2])] : null;
}

/** Зеркало `QuerySort::available`: min/max/stddev нет в окне, JIT — только с pgss 1.10. */
export function sortAvailable(sort: QuerySort, pgss: PgssMeta | null, window: boolean): boolean {
  if (sort === "maxTime" || sort === "stddevTime") return !window;
  if (sort === "jitTime") {
    const v = pgssVersion(pgss?.version);
    return v !== null && (v[0] > 1 || v[1] >= 10);
  }
  return true;
}

export const QUERY_KEY_ID = (k: { queryId: string; dbid: number; userid: number; toplevel: boolean }) => `${k.queryId}:${k.dbid}:${k.userid}:${k.toplevel ? 1 : 0}`;
```

`src/components/queries/QueriesView.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { ConnectionKind, Member, QueriesParams, QueryKey, QuerySort, QueryStat } from "../../api/types";
import { formatBytes, formatCount, formatMs, formatPercent, formatTimestamp } from "../../lib/format";
import { useQueriesReport, type QueriesReport, queriesKey } from "../../lib/useQueriesReport";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { DiagHeader } from "../diag/DiagHeader";
import { FindingsPanel } from "../diag/FindingsPanel";
import { unavailableText } from "../diag/unavailable";
import { QUERY_KEY_ID, queryTitle, sortAvailable } from "./queriesText";

/** Интервал обновления режима «с момента сброса» по умолчанию, с. */
const DEFAULT_INTERVAL_S = 10;
const LIMITS = [50, 100, 200] as const;

interface Col {
  id: string;
  sort: QuerySort | null;
  labelKey: string;
  cell(r: QueryStat): string;
  title?(r: QueryStat): string | undefined;
}

const COLS: Col[] = [
  { id: "share", sort: "totalTime", labelKey: "diag.queries.columns.share", cell: (r) => formatPercent(r.sharePct) },
  { id: "total", sort: "totalTime", labelKey: "diag.queries.columns.total", cell: (r) => formatMs(r.totalExecMs) },
  { id: "mean", sort: "meanTime", labelKey: "diag.queries.columns.mean", cell: (r) => formatMs(r.meanExecMs) },
  { id: "calls", sort: "calls", labelKey: "diag.queries.columns.calls", cell: (r) => formatCount(r.calls) },
  { id: "rowsPerCall", sort: "rowsPerCall", labelKey: "diag.queries.columns.rowsPerCall", cell: (r) => formatCount(r.rowsPerCall, 1) },
  { id: "hit", sort: "hitRatio", labelKey: "diag.queries.columns.hitRatio", cell: (r) => (r.hitRatio === null ? "—" : formatPercent(r.hitRatio * 100)) },
  { id: "read", sort: "sharedRead", labelKey: "diag.queries.columns.read", cell: (r) => formatBytes(r.sharedRead * 8192) },
  { id: "temp", sort: "tempWritten", labelKey: "diag.queries.columns.temp", cell: (r) => formatBytes(r.tempWritten * 8192) },
  { id: "wal", sort: "walBytes", labelKey: "diag.queries.columns.wal", cell: (r) => formatBytes(r.walBytes) },
  { id: "io", sort: "ioTime", labelKey: "diag.queries.columns.io", cell: (r) => formatMs(r.ioReadMs + r.ioWriteMs) },
  { id: "plan", sort: "planTime", labelKey: "diag.queries.columns.plan", cell: (r) => formatMs(r.totalPlanMs) },
  { id: "jit", sort: "jitTime", labelKey: "diag.queries.columns.jit", cell: (r) => (r.jitMs === null ? "—" : formatMs(r.jitMs)) },
  { id: "stddev", sort: "stddevTime", labelKey: "diag.queries.columns.stddev", cell: (r) => (r.stddevExecMs === null ? "—" : formatMs(r.stddevExecMs)) },
  { id: "max", sort: "maxTime", labelKey: "diag.queries.columns.max", cell: (r) => (r.maxExecMs === null ? "—" : formatMs(r.maxExecMs)) },
];

const modeValue = (m: QueriesParams["mode"]) => (m.kind === "window" ? `window:${m.s}` : m.kind);
const parseMode = (v: string): QueriesParams["mode"] => (v.startsWith("window:") ? { kind: "window", s: Number(v.slice(7)) as 300 | 900 | 3600 } : { kind: "sinceReset" });

/**
 * Инструмент «Запросы» (спец. P3 §4.7): общая шапка диагностики, режим и фильтры, таблица топа, находки. Отчёт «за
 * окно» обновляется по тику сэмплера pgss, на который вкладка подписана, пока смонтирована; тексты строк подгружаются
 * по ключам один раз на набор строк.
 */
export function QueriesView({
  connectionId,
  member,
  active,
  live,
  members,
  kind,
  onSelectMember,
}: {
  connectionId: string;
  member: string | null;
  active: boolean;
  live: boolean;
  members: Member[] | null;
  kind: ConnectionKind;
  onSelectMember: (m: string | null) => void;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const open = useWorkspace((s) => s.open);
  const [params, setParams] = useState<QueriesParams>({ mode: { kind: "sinceReset" }, sort: "totalTime", limit: 50, database: null, toplevelOnly: true });
  const [intervalS, setIntervalS] = useState<number | null>(DEFAULT_INTERVAL_S);
  const sampler = useStatementsSampler(connectionId, member, live);
  const q = useQueriesReport(connectionId, member, params, { active, live, intervalS, tickAtMs: sampler.live?.atMs ?? null });
  const report = q.data;
  const data = report?.data;
  const window = params.mode.kind === "window";
  const reportMember = report?.member ?? null;

  // Тексты — по ключам без текста, один запрос на ключ; при смене узла набор запрошенных ключей сбрасывается.
  const requested = useRef(new Set<string>());
  useEffect(() => {
    requested.current.clear();
  }, [connectionId, reportMember]);
  useEffect(() => {
    if (!data || reportMember === null) return;
    const keys = data.rows.filter((r) => r.text === null && !requested.current.has(QUERY_KEY_ID(r.key))).map((r) => r.key);
    if (keys.length === 0) return;
    keys.forEach((k) => requested.current.add(QUERY_KEY_ID(k)));
    void api.queryTexts(connectionId, reportMember, keys).then((texts) => {
      const byKey = new Map(texts.map((x) => [QUERY_KEY_ID(x.key), x]));
      qc.setQueriesData<QueriesReport>({ queryKey: ["diag", connectionId, "queries"] }, (old) =>
        old
          ? { ...old, data: { ...old.data, rows: old.data.rows.map((r) => { const x = byKey.get(QUERY_KEY_ID(r.key)); return x && x.text !== null ? { ...r, text: x.text, textTruncated: x.truncated } : r; }) } }
          : old,
      );
    });
  }, [data, connectionId, reportMember, qc]);

  const databases = useMemo(() => Array.from(new Set((data?.rows ?? []).map((r) => r.datname).filter((d): d is string => d !== null))).sort(), [data]);
  const setSort = (sort: QuerySort) => setParams((p) => ({ ...p, sort }));
  const openQuery = (r: QueryStat) => {
    if (reportMember === null || r.datname === null) return;
    open({ kind: "query", connectionId, member: reportMember, database: r.datname, queryId: r.key.queryId, title: queryTitle(r.text, r.key.queryId) });
  };
  const coverage = () => {
    if (!window || !data) return null;
    const want = params.mode.kind === "window" ? params.mode.s / 60 : 0;
    if (data.coverageS === null) return t("diag.queries.firstTick");
    return t("diag.queries.coverage", { have: Math.round(data.coverageS / 60), want });
  };

  return (
    <>
      <DiagHeader
        tool="queries"
        controls={{
          members, kind, selected: member, leader: report?.followsLeader ? report.member : null, onSelect: onSelectMember,
          intervalS, onInterval: setIntervalS, windowS: null, onWindow: () => {}, onRefresh: () => void q.refetch(),
          updatedAtMs: q.dataUpdatedAt > 0 ? q.dataUpdatedAt : null, stale: q.isError && report !== undefined,
        }}
      />
      <div className="toolbar diag-sub">
        <label className="inline">
          <span>{t("diag.queries.mode")}</span>
          <select aria-label={t("diag.queries.mode")} value={modeValue(params.mode)} onChange={(e) => setParams((p) => ({ ...p, mode: parseMode(e.target.value), sort: sortAvailable(p.sort, data?.pgss ?? null, e.target.value.startsWith("window:")) ? p.sort : "totalTime" }))}>
            <option value="sinceReset">{t("diag.queries.modes.sinceReset")}</option>
            {[300, 900, 3600].map((s) => (
              <option key={s} value={`window:${s}`}>{t("diag.queries.modes.window", { min: s / 60 })}</option>
            ))}
          </select>
        </label>
        {coverage() && <span className="kind-badge" role="status">{coverage()}</span>}
        <label className="inline">
          <span>{t("diag.queries.database")}</span>
          <select aria-label={t("diag.queries.database")} value={params.database ?? ""} onChange={(e) => setParams((p) => ({ ...p, database: e.target.value || null }))}>
            <option value="">{t("diag.queries.allDatabases")}</option>
            {databases.map((d) => <option key={d} value={d}>{d}</option>)}
          </select>
        </label>
        <label className="inline">
          <input type="checkbox" checked={params.toplevelOnly} onChange={(e) => setParams((p) => ({ ...p, toplevelOnly: e.target.checked }))} />
          <span>{t("diag.queries.toplevelOnly")}</span>
        </label>
        <label className="inline">
          <span>{t("diag.queries.limit")}</span>
          <select aria-label={t("diag.queries.limit")} value={params.limit} onChange={(e) => setParams((p) => ({ ...p, limit: Number(e.target.value) as 50 | 100 | 200 }))}>
            {LIMITS.map((l) => <option key={l} value={l}>{l}</option>)}
          </select>
        </label>
      </div>
      {q.error && <ErrorBanner error={q.error} />}
      {sampler.error && <ErrorBanner error={sampler.error} />}
      {report && data ? (
        <div className={q.isPlaceholderData ? "diag-body diag-pending" : "diag-body"} aria-busy={q.isPlaceholderData}>
          <FindingsPanel connectionId={connectionId} findings={report.findings} tool="queries" member={report.member} />
          {report.unavailable.length > 0 && (
            <div className="diag-unavailable">
              {report.unavailable.map((u, i) => <p key={`${u.part}:${i}`} className="muted">{unavailableText(u)}</p>)}
            </div>
          )}
          {data.pgss && (
            <div className="table-scroll">
              <table className="diag-table" aria-label={t("diag.tools.queries")}>
                <thead>
                  <tr>
                    <th scope="col">{t("diag.queries.columns.query")}</th>
                    {COLS.map((c) => {
                      const enabled = c.sort !== null && sortAvailable(c.sort, data.pgss, window);
                      const current = c.sort === params.sort;
                      return (
                        <th key={c.id} scope="col" className="num" aria-sort={current ? (params.sort === "hitRatio" ? "ascending" : "descending") : undefined}>
                          <button type="button" className="link" disabled={!enabled} title={enabled ? undefined : t("diag.queries.sortUnavailable")} onClick={() => c.sort && setSort(c.sort)}>
                            {t(c.labelKey)}{current ? " ▾" : ""}
                          </button>
                        </th>
                      );
                    })}
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((r) => (
                    <tr key={QUERY_KEY_ID(r.key)}>
                      <td className="diag-query-cell" title={r.text ?? undefined}>
                        <button type="button" className="link mono" onClick={() => openQuery(r)} disabled={reportMember === null || r.datname === null}>
                          {queryTitle(r.text, r.key.queryId, 90)}
                        </button>
                        {r.textTruncated && <span className="muted"> {t("diag.queries.truncated")}</span>}
                      </td>
                      {COLS.map((c) => (
                        <td key={c.id} className="num">{c.cell(r)}</td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
              {data.rows.length === 0 && <p className="empty">{window ? t("diag.queries.emptyWindow") : t("diag.queries.empty")}</p>}
            </div>
          )}
          {data.pgss && (
            <p className="muted diag-note">
              {t("diag.queries.pgss", { version: data.pgss.version, reset: data.pgss.statsResetMs === null ? "—" : formatTimestamp(data.pgss.statsResetMs) })}
              {!data.pgss.trackPlanning && ` · ${t("diag.queries.noTrackPlanning")}`}
            </p>
          )}
        </div>
      ) : (
        !q.error && <p className="empty">{t("common.loading")}</p>
      )}
    </>
  );
}
```

CSS (`src/styles.css`): `.diag-query-cell { max-width: 480px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; } .mono { font-family: var(--mono, ui-monospace, monospace); }`.

- [ ] **Step 6: `DiagTab` и словари**

`src/components/diag/DiagTab.tsx`:
- `import { QueriesView } from "../queries/QueriesView";`
- после `const cluster = …`: `const isQueries = tool === "queries";`; в `useDiagnostic(…, { …, live: live && !isQueries })`.
- в `ToolBody` в `switch (report.data.kind)` добавить `case "queries": return null;` (инструмент рисуется отдельной ветвью ниже; вариант нужен для исчерпывающего объединения).
- после ветки `if (!live) { … }` и перед `return (<div className="page diag-tab"> <DiagHeader …`:

```tsx
  if (isQueries) {
    return (
      <div className="page diag-tab">
        <QueriesView connectionId={connectionId} member={member} active={active} live={live} members={topo && topo.kind !== "server" ? topo.members : null} kind={topo?.kind ?? "server"} onSelectMember={pick} />
      </div>
    );
  }
```

(`topo` вычисляется до этого места: перенести строку `const topo = view.data.topology;` выше ветки.)

Словари en (ru — перевод рядом):
- `diag.tools.queries`: "Queries" / "Запросы"; `hints.diag.queries`: "Top queries from pg_stat_statements: since the last reset or over the last 5, 15 or 60 minutes (the sampler reads the statistics once a minute while the tab is open). Click a row to open the query card." / "Топ запросов по pg_stat_statements: с момента сброса или за последние 5, 15 или 60 минут (сэмплер читает статистику раз в минуту, пока вкладка открыта). Строка открывает карточку запроса."
- `tabs.query`: "{{text}} — {{conn}}" (ru то же, в `same-as-en`); `tabs.explain`: "EXPLAIN — {{conn}}" (ru то же).
- `diag.queries`: `mode` "Mode"/"Режим"; `modes.sinceReset` "Since reset"/"С момента сброса"; `modes.window` "Last {{min}} min"/"За {{min}} мин"; `coverage` "{{have}} of {{want}} min collected"/"Накоплено {{have}} из {{want}} мин"; `firstTick` "Waiting for the first tick of the sampler…"/"Идёт первый тик сэмплера…"; `database` "Database"/"База"; `allDatabases` "All databases"/"Все базы"; `toplevelOnly` "Top level only"/"Только верхний уровень"; `limit` "Rows"/"Строк"; `sortUnavailable` "Not available for this version or mode"/"Недоступно для этой версии или режима"; `truncated` "(truncated)"/"(обрезан)"; `empty` "pg_stat_statements has no rows"/"В pg_stat_statements нет строк"; `emptyWindow` "No deltas in the window yet"/"В окне ещё нет дельт"; `pgss` "pg_stat_statements {{version}}, statistics reset {{reset}}"/"pg_stat_statements {{version}}, сброс статистики {{reset}}"; `noTrackPlanning` "track_planning is off: planning columns are empty"/"track_planning выключен: колонки планирования пустые"; `columns`: `query` "Query"/"Запрос", `share` "Share"/"Доля", `total` "Total"/"Всего", `mean` "Mean"/"Среднее", `calls` "Calls"/"Вызовов", `rowsPerCall` "Rows/call"/"Строк/вызов", `hitRatio` "Hit %"/"Hit %", `read` "Read"/"Чтений", `temp` "Temp"/"Temp", `wal` "WAL"/"WAL", `io` "I/O"/"I/O", `plan` "Planning"/"Планирование", `jit` "JIT"/"JIT", `stddev` "Stddev"/"Stddev", `max` "Max"/"Max".
- `findings`: `query_n_plus_one` {title "{{callsSharePct}} of all calls, {{meanMs}} each ({{calls}} calls)" / "{{callsSharePct}} всех вызовов по {{meanMs}} ({{calls}} вызовов)", fix "Looks like N+1: the application repeats a tiny query in a loop. Fetch the rows in one query with a JOIN or an IN (…) list." / "Похоже на N+1: приложение повторяет крошечный запрос в цикле. Получайте строки одним запросом — JOIN или список IN (…)."}; `query_unstable` {title "Unstable execution time: stddev {{stddevMs}} at mean {{meanMs}}" / "Нестабильное время: stddev {{stddevMs}} при среднем {{meanMs}}", fix "The plan or the data volume differs between calls: compare EXPLAIN ANALYZE with different parameter values; check for generic vs custom plan flips and skewed data." / "План или объём данных различаются между вызовами: сравните EXPLAIN ANALYZE с разными значениями параметров; проверьте смену generic/custom плана и скошенные данные."}; `query_low_hit_ratio` {title "Cache hit {{hitRatioPct}}: {{readBytes}} read from disk" / "Попадание в кэш {{hitRatioPct}}: {{readBytes}} прочитано с диска", fix "The working set does not fit shared_buffers or the query scans too much: look for Seq Scan in the plan and consider an index; check shared_buffers." / "Рабочий набор не помещается в shared_buffers или запрос читает слишком много: поищите Seq Scan в плане и подумайте об индексе; проверьте shared_buffers."}; `query_temp` {title "Temp files: {{tempBytes}} per call" / "Temp-файлы: {{tempBytes}} на вызов", fix "Sorts or hashes spill to disk: raise work_mem for the session or role, or add an index that avoids the sort." / "Сортировки или хеши уходят на диск: поднимите work_mem для сессии или роли либо добавьте индекс, избавляющий от сортировки."}; `query_planning_heavy` {title "Planning {{planMs}} takes longer than execution {{execMs}}" / "Планирование {{planMs}} дольше выполнения {{execMs}}", fix "Use prepared statements (plan_cache_mode), touch fewer partitions or simplify the query." / "Используйте подготовленные запросы (plan_cache_mode), затрагивайте меньше партиций или упростите запрос."}; `queries_incomplete` {title "The top is incomplete: {{dealloc}} evictions from pg_stat_statements" / "Топ неполный: {{dealloc}} вытеснений из pg_stat_statements", fix "Raise pg_stat_statements.max (now {{max}}) or reset statistics less often: rarely seen queries have been evicted." / "Увеличьте pg_stat_statements.max (сейчас {{max}}) или сбрасывайте статистику реже: редкие запросы вытеснены."}.
- `diag.duration.msShort` "ms"/"мс", `diag.duration.secShort` "s"/"с".
Ключи с ru == en (`tabs.query`, `tabs.explain`, `diag.queries.columns.{hitRatio,temp,wal,io,jit,stddev,max}`) — в `docs/i18n/same-as-en.json`.

- [ ] **Step 7: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS; существующие тесты `DiagTab`, `FindingsPanel`, `DiagnosticsNodes` зелёные (новый инструмент в дереве — проверить, что тест списка инструментов, если он перечисляет их, дополнен `Запросы`).

- [ ] **Step 8: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): инструмент «Запросы» — типы P3, клиент, подписка на сэмплер pgss, таблица топа с режимами и сортировками, виды вкладок query/explain

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 13: Фронтенд — дерево плана (`PlanTree`, `PlanNodeDetails`, `PlanSummary`)

**Files:**
- Create: `src/components/queries/plan/PlanTree.tsx`, `src/components/queries/plan/PlanNodeDetails.tsx`, `src/components/queries/plan/PlanSummary.tsx`, `src/components/queries/plan/planText.ts`, `src/components/queries/plan/PlanTree.test.tsx`, `src/test/plans.ts` (фикстура дерева)
- Modify: `src/locales/{en,ru}.json`, `src/styles.css`
- Test: `PlanTree.test.tsx`

**Interfaces:**
- Consumes: типы `PlanTree`, `PlanNode`, `ExplainTier` (задача 12), `formatMs`, `formatCount`, `formatBytes`, `formatPercent`, `CopyButton`, `KeyValue`.
- Produces: `PlanTreeView({ tree, selectedId, onSelect })` (`role="treegrid"`-таблица с развёрткой), `PlanNodeDetails({ node })`, `PlanSummary({ tree, tier?, warnings?, raw?, elapsedMs? })` с кнопками «JSON» и «Как текст»; `planText.ts`: `nodeLabel(node)`, `flatten(root, collapsed) -> { node, depth }[]`, `marks(node) -> string[]` (ключи пометок), `renderText(tree)` — зеркало `plan::to_text` для кнопки «Как текст» (заголовок узла, условия, буферы, итоги); фикстура `src/test/plans.ts`: `planTree(patch)`, `planNode(patch)`.

- [ ] **Step 1: Падающие тесты**

`src/test/plans.ts`:

```ts
import type { PlanNode, PlanTree } from "../api/types";

export function planNode(patch: Partial<PlanNode> = {}): PlanNode {
  return {
    id: 0, nodeType: "Seq Scan", parentRelationship: null, subplanName: null, relation: { schema: "shop", name: "orders", alias: "o" }, indexName: null, strategy: null, joinType: null, parallelAware: false,
    startupCost: 0, totalCost: 60, planRows: 500, planWidth: 8, actual: { startupMs: 0.1, totalMs: 30, rows: 5, loops: 1 }, neverExecuted: false,
    workersPlanned: null, workersLaunched: null, rowsRemovedByFilter: 499_995, rowsRemovedByIndexRecheck: null, rowsRemovedByJoinFilter: null, heapFetches: null,
    filter: "(o.status = 'disputed'::text)", indexCond: null, recheckCond: null, hashCond: null, joinFilter: null, mergeCond: null, sortKey: [], groupKey: [],
    sortMethod: null, sortSpaceKb: null, sortSpaceType: null, hashBatches: null, hashBuckets: null, peakMemoryKb: null, exactHeapBlocks: null, lossyHeapBlocks: null, subplansRemoved: null,
    buffers: { sharedHit: 80, sharedRead: 20, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0 },
    ioReadMs: null, ioWriteMs: null, output: [], extra: {}, inclusiveMs: 30, exclusiveMs: 30, sharePct: 54.5, estimateRatio: 0.01, hot: true, misestimated: true, children: [], ...patch,
  };
}

/** Hash Join над Seq Scan и Index Scan, с ANALYZE. */
export function planTree(patch: Partial<PlanTree> = {}): PlanTree {
  const seq = planNode({ id: 1, parentRelationship: "Outer" });
  const idx = planNode({
    id: 2, nodeType: "Index Scan", parentRelationship: "Inner", indexName: "customers_pkey", relation: { schema: "shop", name: "customers", alias: "c" },
    totalCost: 4, planRows: 100, actual: { startupMs: 0.01, totalMs: 0.4, rows: 100, loops: 1 }, rowsRemovedByFilter: null, filter: null, indexCond: "(c.id < 100)",
    buffers: null, inclusiveMs: 0.4, exclusiveMs: 0.4, sharePct: 0.7, estimateRatio: 1, hot: false, misestimated: false,
  });
  const root = planNode({
    id: 0, nodeType: "Hash Join", joinType: "Inner", relation: null, totalCost: 100, planRows: 1000, actual: { startupMs: 1, totalMs: 50, rows: 10, loops: 1 },
    rowsRemovedByFilter: null, filter: null, hashCond: "(o.customer_id = c.id)", buffers: null, inclusiveMs: 50, exclusiveMs: 19.6, sharePct: 35.6, estimateRatio: 0.01, hot: true, misestimated: false, children: [seq, idx],
  });
  return { root, planningMs: 0.5, executionMs: 55, triggers: [], jit: null, settings: [{ name: "work_mem", value: "1MB" }], totalMs: 55, ...patch };
}
```

`src/components/queries/plan/PlanTree.test.tsx`:

```tsx
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { planNode, planTree } from "../../../test/plans";
import { PlanSummary } from "./PlanSummary";
import { PlanTreeView } from "./PlanTree";
import { renderText } from "./planText";

describe("PlanTreeView", () => {
  it("строки дерева с отступом, горячие и ошибочные оценки помечены, выбор строки", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(<PlanTreeView tree={planTree()} selectedId={null} onSelect={onSelect} />);
    const rows = screen.getAllByRole("row").slice(1);
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent("Hash Join");
    expect(rows[1]).toHaveTextContent("Seq Scan on shop.orders o");
    expect(rows[1]).toHaveAttribute("aria-level", "2");
    expect(rows[1]).toHaveClass("plan-hot");
    expect(within(rows[1]).getByText("×100")).toHaveClass("plan-misestimate");
    expect(rows[1]).toHaveTextContent("500 → 5");
    expect(rows[2]).not.toHaveClass("plan-hot");
    await user.click(within(rows[1]).getByRole("button", { name: /Seq Scan/ }));
    expect(onSelect).toHaveBeenCalledWith(1);
  });

  it("сворачивание скрывает детей; «never executed» и пометки", async () => {
    const user = userEvent.setup();
    const tree = planTree();
    tree.root.children[1] = planNode({ ...tree.root.children[1], neverExecuted: true, sortMethod: "external merge", sortSpaceType: "Disk", sortSpaceKb: 2048 });
    render(<PlanTreeView tree={tree} selectedId={null} onSelect={() => {}} />);
    expect(screen.getAllByRole("row")).toHaveLength(4);
    expect(screen.getByText("не выполнялся")).toBeInTheDocument();
    expect(screen.getByText("диск")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Свернуть" }));
    expect(screen.getAllByRole("row")).toHaveLength(2);
  });
});

describe("PlanSummary и текст", () => {
  it("сводка: планирование, выполнение, ярус, settings; текст плана в стиле psql", () => {
    render(<PlanSummary tree={planTree()} tier="read" warnings={["genericPlan"]} raw={[{ Plan: {} }]} elapsedMs={70} />);
    expect(screen.getByText(/Планирование: 0,50 мс/)).toBeInTheDocument();
    expect(screen.getByText(/Выполнение: 55,0 мс/)).toBeInTheDocument();
    expect(screen.getByText("Ярус: чтение")).toBeInTheDocument();
    expect(screen.getByText("work_mem = 1MB")).toBeInTheDocument();
    expect(screen.getByText("План общий (GENERIC_PLAN): без значений параметров")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать JSON" })).toBeInTheDocument();
    const text = renderText(planTree());
    expect(text.split("\n")[0]).toBe("Hash Join  (cost=0.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)");
    expect(text).toContain("  ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)");
    expect(text).toContain("        Rows Removed by Filter: 499995");
    expect(text).toContain("Execution Time: 55.000 ms");
  });
});
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- PlanTree`
Expected: FAIL (модулей нет).

- [ ] **Step 3: Реализация**

`src/components/queries/plan/planText.ts`:

```ts
import type { PlanNode, PlanTree } from "../../../api/types";

/** Заголовок узла: тип (со стратегией), индекс, отношение с псевдонимом — как в psql. */
export function nodeLabel(n: PlanNode): string {
  let s = n.strategy && n.strategy !== "Plain" ? `${n.strategy}${n.nodeType}` : n.nodeType;
  if (n.indexName) s += ` using ${n.indexName}`;
  if (n.relation) {
    s += ` on ${n.relation.schema ? `${n.relation.schema}.` : ""}${n.relation.name}`;
    if (n.relation.alias && n.relation.alias !== n.relation.name) s += ` ${n.relation.alias}`;
  }
  return s;
}

/** Строки дерева в порядке обхода с глубиной; дети свёрнутых узлов пропускаются. */
export function flatten(root: PlanNode, collapsed: ReadonlySet<number>): { node: PlanNode; depth: number }[] {
  const out: { node: PlanNode; depth: number }[] = [];
  const walk = (n: PlanNode, depth: number) => {
    out.push({ node: n, depth });
    if (!collapsed.has(n.id)) n.children.forEach((c) => walk(c, depth + 1));
  };
  walk(root, 0);
  return out;
}

/** Ключи пометок узла (`diag.plan.marks.*`). */
export function marks(n: PlanNode): string[] {
  const out: string[] = [];
  if (n.neverExecuted) out.push("neverExecuted");
  if (n.sortSpaceType === "Disk" || (n.hashBatches ?? 0) > 1) out.push("disk");
  if ((n.lossyHeapBlocks ?? 0) > 0) out.push("lossy");
  if ((n.heapFetches ?? 0) > 0) out.push("heapFetches");
  if (n.workersPlanned !== null && n.workersLaunched !== null && n.workersLaunched < n.workersPlanned) out.push("workersShort");
  if (n.parallelAware) out.push("parallel");
  return out;
}

/** Ошибка оценки как «×100» / «÷100»; `null` — не считается. */
export function estimateLabel(n: PlanNode): string | null {
  if (n.estimateRatio === null || !n.misestimated) return null;
  if (!Number.isFinite(n.estimateRatio)) return "×∞";
  return n.estimateRatio >= 1 ? `×${Math.round(n.estimateRatio)}` : `÷${Math.round(1 / n.estimateRatio)}`;
}

const f2 = (x: number) => x.toFixed(2);
const f3 = (x: number) => x.toFixed(3);

/** Зеркало `pg_advisor::plan::to_text`: текст плана в стиле psql для копирования и для LLM. */
export function renderText(t: PlanTree): string {
  const lines: string[] = [];
  const node = (n: PlanNode, depth: number) => {
    const head = depth === 0 ? "" : `${"  ".repeat(depth * 3 - 3)}  ->  `;
    const rest = depth === 0 ? "  " : `${"  ".repeat(depth * 3)}  `;
    let h = `${nodeLabel(n)}  (cost=${f2(n.startupCost)}..${f2(n.totalCost)} rows=${Math.round(n.planRows)} width=${n.planWidth})`;
    if (n.actual) h += n.neverExecuted ? " (never executed)" : ` (actual time=${f3(n.actual.startupMs)}..${f3(n.actual.totalMs)} rows=${Math.round(n.actual.rows)} loops=${n.actual.loops})`;
    lines.push(head + h);
    const line = (label: string, v: string) => lines.push(`${rest}${label}: ${v}`);
    if (n.subplanName) line("Subplan Name", n.subplanName);
    if (n.indexCond) line("Index Cond", n.indexCond);
    if (n.recheckCond) line("Recheck Cond", n.recheckCond);
    if (n.hashCond) line("Hash Cond", n.hashCond);
    if (n.mergeCond) line("Merge Cond", n.mergeCond);
    if (n.joinFilter) line("Join Filter", n.joinFilter);
    if (n.filter) line("Filter", n.filter);
    if (n.rowsRemovedByFilter !== null) line("Rows Removed by Filter", String(n.rowsRemovedByFilter));
    if (n.rowsRemovedByIndexRecheck !== null) line("Rows Removed by Index Recheck", String(n.rowsRemovedByIndexRecheck));
    if (n.rowsRemovedByJoinFilter !== null) line("Rows Removed by Join Filter", String(n.rowsRemovedByJoinFilter));
    if (n.sortKey.length) line("Sort Key", n.sortKey.join(", "));
    if (n.groupKey.length) line("Group Key", n.groupKey.join(", "));
    if (n.sortMethod) line("Sort Method", `${n.sortMethod}${n.sortSpaceType && n.sortSpaceKb !== null ? `  ${n.sortSpaceType}: ${n.sortSpaceKb}kB` : ""}`);
    if (n.hashBatches !== null) line("Buckets", `${n.hashBuckets ?? 0}  Batches: ${n.hashBatches}  Memory Usage: ${n.peakMemoryKb ?? 0}kB`);
    if (n.exactHeapBlocks !== null || n.lossyHeapBlocks !== null) line("Heap Blocks", `exact=${n.exactHeapBlocks ?? 0} lossy=${n.lossyHeapBlocks ?? 0}`);
    if (n.heapFetches !== null) line("Heap Fetches", String(n.heapFetches));
    if (n.workersPlanned !== null && n.workersLaunched !== null) {
      line("Workers Planned", String(n.workersPlanned));
      line("Workers Launched", String(n.workersLaunched));
    }
    if (n.subplansRemoved !== null) line("Subplans Removed", String(n.subplansRemoved));
    if (n.buffers) {
      const b = n.buffers;
      const g = (name: string, hit: number, read: number, dirtied: number, written: number) => {
        const parts = [hit > 0 && `hit=${hit}`, read > 0 && `read=${read}`, dirtied > 0 && `dirtied=${dirtied}`, written > 0 && `written=${written}`].filter(Boolean);
        return parts.length ? `${name} ${parts.join(" ")}` : null;
      };
      const parts = [g("shared", b.sharedHit, b.sharedRead, b.sharedDirtied, b.sharedWritten), g("local", b.localHit, b.localRead, b.localDirtied, b.localWritten), b.tempRead > 0 || b.tempWritten > 0 ? `temp read=${b.tempRead} written=${b.tempWritten}` : null].filter(Boolean);
      if (parts.length) line("Buffers", parts.join(", "));
    }
    if (n.ioReadMs !== null || n.ioWriteMs !== null) line("I/O Timings", `read=${f3(n.ioReadMs ?? 0)} write=${f3(n.ioWriteMs ?? 0)}`);
    n.children.forEach((c) => node(c, depth + 1));
  };
  node(t.root, 0);
  if (t.planningMs !== null) lines.push(`Planning Time: ${f3(t.planningMs)} ms`);
  for (const tr of t.triggers) lines.push(`Trigger ${tr.name}: time=${f3(tr.timeMs)} calls=${tr.calls}`);
  if (t.jit) lines.push("JIT:", `  Functions: ${t.jit.functions}`, `  Timing: Generation ${f3(t.jit.generationMs)} ms, Inlining ${f3(t.jit.inliningMs)} ms, Optimization ${f3(t.jit.optimizationMs)} ms, Emission ${f3(t.jit.emissionMs)} ms, Total ${f3(t.jit.totalMs)} ms`);
  if (t.executionMs !== null) lines.push(`Execution Time: ${f3(t.executionMs)} ms`);
  return `${lines.join("\n")}\n`;
}
```

`src/components/queries/plan/PlanTree.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { PlanNode, PlanTree } from "../../../api/types";
import { formatCount, formatMs } from "../../../lib/format";
import { estimateLabel, flatten, marks, nodeLabel } from "./planText";

/** Полоса доли собственного времени, ширина — проценты. */
function ShareBar({ pct }: { pct: number | null }) {
  if (pct === null) return null;
  return (
    <span className="plan-share" aria-hidden="true">
      <span style={{ width: `${Math.min(100, Math.max(0, pct))}%` }} />
    </span>
  );
}

/**
 * Дерево плана (спец. P3 §8.6): таблица-дерево с отступами и развёрткой; колонки — узел, время (включённое и
 * собственное с полосой доли), строки (оценка → факт, ошибка ×N подсвечена), циклы, буферы, пометки. Горячие узлы
 * выделены классом `plan-hot`; клик по узлу выбирает его для панели деталей.
 */
export function PlanTreeView({ tree, selectedId, onSelect }: { tree: PlanTree; selectedId: number | null; onSelect: (id: number) => void }) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState<ReadonlySet<number>>(new Set());
  const rows = flatten(tree.root, collapsed);
  const toggle = (id: number) => setCollapsed((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  const withActual = tree.root.actual !== null;
  return (
    <div className="table-scroll">
      <table className="diag-table plan-tree" role="treegrid" aria-label={t("diag.plan.tree")}>
        <thead>
          <tr>
            <th scope="col">{t("diag.plan.columns.node")}</th>
            <th scope="col" className="num">{withActual ? t("diag.plan.columns.time") : t("diag.plan.columns.cost")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.rows")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.loops")}</th>
            <th scope="col" className="num">{t("diag.plan.columns.buffers")}</th>
            <th scope="col">{t("diag.plan.columns.marks")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ node: n, depth }) => {
            const est = estimateLabel(n);
            const classes = [n.hot ? "plan-hot" : "", n.id === selectedId ? "plan-selected" : ""].filter(Boolean).join(" ") || undefined;
            return (
              <tr key={n.id} className={classes} aria-level={depth + 1} aria-expanded={n.children.length ? !collapsed.has(n.id) : undefined} aria-selected={n.id === selectedId}>
                <td style={{ paddingLeft: `${8 + depth * 18}px` }}>
                  {n.children.length > 0 && (
                    <button type="button" className="icon" aria-label={collapsed.has(n.id) ? t("diag.plan.expand") : t("diag.plan.collapse")} onClick={() => toggle(n.id)}>
                      <span aria-hidden="true">{collapsed.has(n.id) ? "▸" : "▾"}</span>
                    </button>
                  )}
                  <button type="button" className="link" onClick={() => onSelect(n.id)}>{nodeLabel(n)}</button>
                  {n.subplanName && <span className="muted"> {n.subplanName}</span>}
                </td>
                <td className="num">
                  {withActual ? (
                    n.inclusiveMs !== null && n.exclusiveMs !== null ? (
                      <>
                        <span title={t("diag.plan.inclusive")}>{formatMs(n.inclusiveMs)}</span>
                        <span className="muted"> / {formatMs(n.exclusiveMs)}</span>
                        <ShareBar pct={n.sharePct} />
                      </>
                    ) : "—"
                  ) : (
                    `${formatCount(n.startupCost, 2)}..${formatCount(n.totalCost, 2)}`
                  )}
                </td>
                <td className="num">
                  {formatCount(n.planRows)}
                  {n.actual && <> → {formatCount(n.actual.rows)}</>}
                  {est && <span className="plan-misestimate"> {est}</span>}
                </td>
                <td className="num">{n.actual ? formatCount(n.actual.loops) : "—"}</td>
                <td className="num">
                  {n.buffers ? `${formatCount(n.buffers.sharedHit)} / ${formatCount(n.buffers.sharedRead)}` : "—"}
                  {n.buffers && n.buffers.tempWritten > 0 && <span className="muted"> temp {formatCount(n.buffers.tempWritten)}</span>}
                </td>
                <td>{marks(n).map((m) => <span key={m} className="kind-badge">{t(`diag.plan.marks.${m}`)}</span>)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
```

`src/components/queries/plan/PlanNodeDetails.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { PlanNode } from "../../../api/types";
import { formatCount, formatMs } from "../../../lib/format";
import { KeyValue, type KeyValueItem } from "../../catalog/KeyValue";
import { nodeLabel } from "./planText";

/** Панель выбранного узла: все распознанные поля и `extra` как есть. */
export function PlanNodeDetails({ node: n }: { node: PlanNode }) {
  const { t } = useTranslation();
  const items: KeyValueItem[] = [
    { label: t("diag.plan.details.type"), value: nodeLabel(n) },
    { label: t("diag.plan.details.parent"), value: n.parentRelationship },
    { label: t("diag.plan.details.cost"), value: `${n.startupCost.toFixed(2)}..${n.totalCost.toFixed(2)}` },
    { label: t("diag.plan.details.planRows"), value: `${formatCount(n.planRows)} × ${n.planWidth} B` },
    { label: t("diag.plan.details.actual"), value: n.actual ? `${formatMs(n.actual.startupMs)}..${formatMs(n.actual.totalMs)}, ${formatCount(n.actual.rows)} ${t("diag.plan.details.rowsUnit")}, loops ${formatCount(n.actual.loops)}` : null },
    { label: t("diag.plan.details.exclusive"), value: n.exclusiveMs === null ? null : formatMs(n.exclusiveMs) },
    { label: t("diag.plan.details.filter"), value: n.filter },
    { label: t("diag.plan.details.indexCond"), value: n.indexCond },
    { label: t("diag.plan.details.recheckCond"), value: n.recheckCond },
    { label: t("diag.plan.details.hashCond"), value: n.hashCond },
    { label: t("diag.plan.details.joinFilter"), value: n.joinFilter },
    { label: t("diag.plan.details.mergeCond"), value: n.mergeCond },
    { label: t("diag.plan.details.removedByFilter"), value: n.rowsRemovedByFilter === null ? null : formatCount(n.rowsRemovedByFilter) },
    { label: t("diag.plan.details.removedByRecheck"), value: n.rowsRemovedByIndexRecheck === null ? null : formatCount(n.rowsRemovedByIndexRecheck) },
    { label: t("diag.plan.details.heapFetches"), value: n.heapFetches === null ? null : formatCount(n.heapFetches) },
    { label: t("diag.plan.details.sortKey"), value: n.sortKey.length ? n.sortKey.join(", ") : null },
    { label: t("diag.plan.details.sortMethod"), value: n.sortMethod ? `${n.sortMethod}${n.sortSpaceType ? ` (${n.sortSpaceType} ${formatCount(n.sortSpaceKb ?? 0)} kB)` : ""}` : null },
    { label: t("diag.plan.details.hash"), value: n.hashBatches === null ? null : `${t("diag.plan.details.batches")} ${n.hashBatches}, ${formatCount(n.peakMemoryKb ?? 0)} kB` },
    { label: t("diag.plan.details.workers"), value: n.workersPlanned === null ? null : `${n.workersLaunched ?? "—"} / ${n.workersPlanned}` },
    { label: t("diag.plan.details.buffers"), value: n.buffers ? `shared hit ${formatCount(n.buffers.sharedHit)}, read ${formatCount(n.buffers.sharedRead)}, dirtied ${formatCount(n.buffers.sharedDirtied)}, written ${formatCount(n.buffers.sharedWritten)}; temp read ${formatCount(n.buffers.tempRead)}, written ${formatCount(n.buffers.tempWritten)}` : null },
    { label: t("diag.plan.details.io"), value: n.ioReadMs === null && n.ioWriteMs === null ? null : `${formatMs(n.ioReadMs ?? 0)} / ${formatMs(n.ioWriteMs ?? 0)}` },
    { label: t("diag.plan.details.output"), value: n.output.length ? n.output.join(", ") : null },
  ].filter((i) => i.value !== null && i.value !== undefined);
  const extra = Object.entries(n.extra);
  return (
    <section className="plan-details" aria-label={t("diag.plan.details.title")}>
      <h3>{t("diag.plan.details.title")}</h3>
      <KeyValue items={items} />
      {extra.length > 0 && (
        <details>
          <summary>{t("diag.plan.details.extra", { count: extra.length })}</summary>
          <pre>{JSON.stringify(n.extra, null, 2)}</pre>
        </details>
      )}
    </section>
  );
}
```

`src/components/queries/plan/PlanSummary.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { ExplainTier, PlanTree } from "../../../api/types";
import { formatMs } from "../../../lib/format";
import { CopyButton } from "../../common/CopyButton";
import { renderText } from "./planText";

/** Ключи предупреждений результата EXPLAIN — литералами (правило словарей). */
const WARNING_KEYS: Record<string, string> = {
  genericPlan: "diag.plan.warnings.genericPlan",
  probeWithoutReadOnly: "diag.plan.warnings.probeWithoutReadOnly",
};

/** Сводка плана: планирование, выполнение, ярус, JIT, триггеры, settings, предупреждения; «JSON» и «Как текст». */
export function PlanSummary({ tree, tier, warnings = [], raw, elapsedMs }: { tree: PlanTree; tier?: ExplainTier; warnings?: string[]; raw?: unknown; elapsedMs?: number }) {
  const { t } = useTranslation();
  const parts = [
    tree.planningMs !== null && t("diag.plan.summary.planning", { ms: formatMs(tree.planningMs) }),
    tree.executionMs !== null && t("diag.plan.summary.execution", { ms: formatMs(tree.executionMs) }),
    tier && t("diag.plan.summary.tier", { tier: t(`diag.plan.tiers.${tier}`) }),
    elapsedMs !== undefined && t("diag.plan.summary.elapsed", { ms: formatMs(elapsedMs) }),
    tree.jit && t("diag.plan.summary.jit", { functions: tree.jit.functions, ms: formatMs(tree.jit.totalMs) }),
  ].filter((p): p is string => typeof p === "string");
  return (
    <section className="plan-summary">
      <p>{parts.map((p) => <span key={p} className="plan-summary-part">{p}</span>)}</p>
      {tree.triggers.length > 0 && <p className="muted">{tree.triggers.map((tr) => `${tr.name}: ${formatMs(tr.timeMs)} × ${tr.calls}`).join("; ")}</p>}
      {tree.settings.length > 0 && <p className="muted">{tree.settings.map((s) => <code key={s.name} className="plan-setting">{s.name} = {s.value}</code>)}</p>}
      {warnings.map((w) => <p key={w} className="diag-note" role="status">{WARNING_KEYS[w] ? t(WARNING_KEYS[w]) : w}</p>)}
      <div className="toolbar">
        {raw !== undefined && <CopyButton text={JSON.stringify(raw, null, 2)} label={t("diag.plan.copyJson")} />}
        <CopyButton text={renderText(tree)} label={t("diag.plan.copyText")} />
      </div>
    </section>
  );
}
```

CSS: `.plan-tree tr.plan-hot td:first-child { font-weight: 600; } .plan-tree tr.plan-selected { background: var(--selection, rgba(79,140,255,.12)); } .plan-misestimate { color: var(--warn, #c77700); font-weight: 600; } .plan-share { display: inline-block; width: 60px; height: 6px; margin-left: 6px; background: var(--border); vertical-align: middle; } .plan-share > span { display: block; height: 100%; background: var(--accent, #4f8cff); } .plan-summary-part + .plan-summary-part::before { content: " · "; color: var(--muted); } .plan-setting { margin-right: 8px; } .plan-details pre { max-height: 240px; overflow: auto; }`.

Словари `diag.plan` (en / ru): `tree` "Plan tree"/"Дерево плана"; `columns`: `node` "Node"/"Узел", `time` "Time (incl. / own)"/"Время (вкл. / собств.)", `cost` "Cost"/"Стоимость", `rows` "Rows (plan → actual)"/"Строки (оценка → факт)", `loops` "Loops"/"Циклы", `buffers` "Buffers (hit / read)"/"Буферы (hit / read)", `marks` "Marks"/"Пометки"; `expand` "Expand"/"Развернуть"; `collapse` "Collapse"/"Свернуть"; `inclusive` "Inclusive time × loops"/"Включённое время × циклы"; `marks`: `neverExecuted` "never executed"/"не выполнялся", `disk` "disk"/"диск", `lossy` "lossy"/"lossy", `heapFetches` "heap fetches"/"heap fetches", `workersShort` "fewer workers"/"меньше воркеров", `parallel` "parallel"/"параллельно"; `details`: `title` "Node details"/"Узел", `type` "Node"/"Узел", `parent` "Relationship"/"Связь", `cost` "Cost"/"Стоимость", `planRows` "Plan rows × width"/"Оценка строк × ширина", `actual` "Actual"/"Факт", `rowsUnit` "rows"/"строк", `exclusive` "Own time"/"Собственное время", `filter` "Filter"/"Filter", `indexCond` "Index Cond"/"Index Cond", `recheckCond` "Recheck Cond"/"Recheck Cond", `hashCond` "Hash Cond"/"Hash Cond", `joinFilter` "Join Filter"/"Join Filter", `mergeCond` "Merge Cond"/"Merge Cond", `removedByFilter` "Rows removed by filter"/"Отсеяно фильтром", `removedByRecheck` "Rows removed by recheck"/"Отсеяно перепроверкой", `heapFetches` "Heap fetches"/"Heap fetches", `sortKey` "Sort key"/"Ключ сортировки", `sortMethod` "Sort method"/"Метод сортировки", `hash` "Hash"/"Хеш", `batches` "batches"/"батчей", `workers` "Workers launched / planned"/"Воркеров запущено / план", `buffers` "Buffers"/"Буферы", `io` "I/O read / write"/"I/O чтение / запись", `output` "Output"/"Output", `extra` "Other fields ({{count}})"/"Прочие поля ({{count}})"; `summary`: `planning` "Planning: {{ms}}"/"Планирование: {{ms}}", `execution` "Execution: {{ms}}"/"Выполнение: {{ms}}", `tier` "Tier: {{tier}}"/"Ярус: {{tier}}", `elapsed` "Round trip: {{ms}}"/"Запрос: {{ms}}", `jit` "JIT: {{functions}} functions, {{ms}}"/"JIT: функций {{functions}}, {{ms}}"; `tiers`: `read` "read"/"чтение", `write` "write"/"запись"; `warnings`: `genericPlan` "Generic plan (GENERIC_PLAN): no parameter values"/"План общий (GENERIC_PLAN): без значений параметров", `probeWithoutReadOnly` "The probe plan was built outside READ ONLY (the server refused it there)"/"Пробный план построен вне READ ONLY (сервер отверг его там)"; `copyJson` "Copy JSON"/"Копировать JSON"; `copyText` "Copy as text"/"Копировать как текст".

- [ ] **Step 4: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test -- PlanTree && pnpm i18n:check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): дерево плана — строки с отступом и развёрткой, горячие узлы, ошибка оценки, панель узла, сводка, текст как в psql

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 14: Фронтенд — карточка запроса (`QueryTab`), `PlanRunner`, форма параметров, подтверждение яруса записи, «Скопировать для LLM»

**Files:**
- Create: `src/components/queries/QueryTab.tsx`, `src/components/queries/QueryOverview.tsx`, `src/components/queries/QueryPlan.tsx`, `src/components/queries/plan/PlanRunner.tsx`, `src/components/queries/plan/ParamsForm.tsx`, `src/components/queries/plan/AnalyzeConfirm.tsx`, `src/components/queries/CopyLlmButton.tsx`, `src/components/queries/Sparkline.tsx`, `src/lib/useQueryDetail.ts`, `src/lib/useExplain.ts`, `src/lib/llmMarkdown.ts`, `src/components/queries/QueryTab.test.tsx`, `src/components/queries/plan/PlanRunner.test.tsx`, `src/lib/llmMarkdown.test.ts`
- Modify: `src/components/TabContent.tsx` (`query`), `src/components/diag/FindingsPanel.tsx` (кнопка `openQuery`), `src/lib/useConnectionEvents.ts` (`PG_QUERY_KEYS` + `queryDetail`), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: перечисленные тесты

**Interfaces:**
- Consumes: `api.{queryDetail, explain, queryLlmBundle}`, `PlanTreeView`, `PlanNodeDetails`, `PlanSummary`, `ConfirmDialog`, `useConnectionProfile`, `useTopology`, `useStatementsSampler`, `SubTabs`, `KeyValue`, `FindingsPanel`.
- Produces: `QueryTab({ connectionId, member, database, queryId, active })`; `QueryOverview({ detail, onPlan, onConsole })`; `QueryPlan({ connectionId, member, database, detail, onPlanResult })`; `PlanRunner({ connectionId, member, database, sql, serverVersionNum, inRecovery, members?, onResult? })` — состояние запуска (параметры, срок, `search_path`, история, подтверждение), возвращает `ExplainResult` через `onResult`; `ParamsForm({ count, values, onChange, generic, canGeneric, onGeneric })`; `AnalyzeConfirm({ connection, relations, onConfirm, onCancel, busy })`; `CopyLlmButton({ connectionId, member, database, source, plan })`; `Sparkline({ points, label })` (SVG); `useQueryDetail(connectionId, member, database, queryId, { active, tickAtMs })`; `useExplain()` — мутация `explain`; `llmMarkdown(bundle, t) -> string`; `countParams(sql)` (зеркало Rust) в `useExplain.ts`.

Уточнение: подтверждение яруса записи — `ConfirmDialog` P2 с детьми (список таблиц и предупреждение) и `danger`; на не-prod без `objectName` диалог всё равно показывается (кнопка «Выполнить» без полей ввода) — `needsConfirmation` P2 для него не используется: подтверждение нужно всегда (спец. §8.3 п. 4).

- [ ] **Step 1: Падающие тесты**

`src/lib/llmMarkdown.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { LlmBundle } from "../api/types";
import { i18n } from "../i18n";
import { llmMarkdown } from "./llmMarkdown";

const bundle: LlmBundle = {
  serverVersionNum: 170_004, pgssVersion: "1.11", inRecovery: false, member: "pg-1", database: "perf",
  text: "SELECT * FROM shop.customers WHERE id = $1", textTruncated: false,
  stats: { sinceReset: { key: { queryId: "1", dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 1000, totalExecMs: 500, meanExecMs: 0.5, minExecMs: 0.1, maxExecMs: 3, stddevExecMs: 0.2, rows: 1000, rowsPerCall: 1, plans: 0, totalPlanMs: 0, sharedHit: 4000, sharedRead: 10, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 12.5, hitRatio: 0.9975, text: null, textTruncated: false }, window3600: null },
  planText: "Index Scan using customers_pkey on shop.customers  (cost=0.42..8.44 rows=1 width=40)\n", tables: [{ schema: "shop", name: "customers", ddl: "CREATE TABLE shop.customers (...);", indexes: ["CREATE UNIQUE INDEX customers_pkey ON shop.customers USING btree (id)"] }], tablesOmitted: 2, warnings: ["paramsNeeded"],
};

describe("llmMarkdown", () => {
  it("разделы на языке интерфейса, статистика таблицей, план и DDL в блоках кода, предупреждения и «ещё N таблиц»", () => {
    const md = llmMarkdown(bundle, i18n.t.bind(i18n));
    expect(md).toContain("## Среда");
    expect(md).toContain("PostgreSQL 17.4");
    expect(md).toContain("pg_stat_statements 1.11");
    expect(md).toContain("```sql\nSELECT * FROM shop.customers WHERE id = $1\n```");
    expect(md).toContain("| Вызовов | 1 000 |");
    expect(md).toContain("## План\n\n```\nIndex Scan using customers_pkey");
    expect(md).toContain("### shop.customers\n\n```sql\nCREATE TABLE shop.customers (...);\nCREATE UNIQUE INDEX customers_pkey");
    expect(md).toContain("… ещё 2 таблиц");
    expect(md).toContain("план требует значений параметров");
    expect(md).not.toMatch(/\{\{/);
  });
});
```

`src/components/queries/plan/PlanRunner.test.tsx`:

```tsx
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ExplainResult } from "../../../api/types";
import { connection } from "../../../test/fixtures";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), listConnections: vi.fn() } };
});

import { api } from "../../../api/client";
import { PlanRunner } from "./PlanRunner";

const result = (patch: Partial<ExplainResult> = {}): ExplainResult => ({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [{ schema: "shop", name: "orders" }], elapsedMs: 12, warnings: [], ...patch });

describe("PlanRunner", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", environment: "dev", readOnly: false })]);
  });

  it("форма из $N, GENERIC_PLAN на 16+, «План» шлёт значения; ANALYZE чтения — без подтверждения", async () => {
    vi.mocked(api.explain).mockResolvedValue(result());
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="SELECT * FROM t WHERE a = $1 AND b = $2" serverVersionNum={170_000} inRecovery={false} />);
    expect(screen.getByRole("textbox", { name: "$1" })).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "$2" })).toBeInTheDocument();
    const generic = screen.getByRole("checkbox", { name: "GENERIC_PLAN (без значений)" });
    expect(generic).toBeChecked();
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT * FROM t WHERE a = $1 AND b = $2", mode: "plan", params: { kind: "generic" }, member: "pg-1", database: "perf" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    await user.click(generic);
    await user.type(screen.getByRole("textbox", { name: "$1" }), "7");
    vi.mocked(api.explain).mockResolvedValue(result({ mode: "analyze" }));
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", params: { kind: "values", values: ["7", null] }, timeoutS: 30, forceWrite: false })));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByText(/История/)).toBeInTheDocument();
  });

  it("ярус записи: подтверждение со списком таблиц, затем ANALYZE; EXPLAIN_WRITES предлагает выполнить как запись", async () => {
    vi.mocked(api.explain).mockResolvedValueOnce(result({ tier: "write", relations: [{ schema: "shop", name: "balances" }] })).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-1" database="perf" sql="UPDATE shop.balances SET balance = 0" serverVersionNum={170_000} inRecovery={false} />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("shop.balances")).toBeInTheDocument();
    expect(within(dialog).getByText(/Строки изменятся и откатятся/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", confirmation: { confirm: null, typedName: null } })));
    // Запрос-чтение с побочным эффектом: сервер отвечает EXPLAIN_WRITES → кнопка «Выполнить как запись».
    vi.mocked(api.explain).mockResolvedValueOnce(result()).mockRejectedValueOnce({ code: "EXPLAIN_WRITES", message: "writes", retryable: false, key: "errors.explain.writes" });
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    const asWrite = await screen.findByRole("button", { name: "Выполнить как запись" });
    vi.mocked(api.explain).mockResolvedValueOnce(result({ mode: "analyze", tier: "write" }));
    await user.click(asWrite);
    await user.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Выполнить и откатить" }));
    await waitFor(() => expect(api.explain).toHaveBeenLastCalledWith("c", expect.objectContaining({ mode: "analyze", forceWrite: true })));
  });

  it("на реплике ярус записи закрыт подсказкой; read-only профиль — то же", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", readOnly: true })]);
    vi.mocked(api.explain).mockResolvedValue(result({ tier: "write" }));
    const user = userEvent.setup();
    renderWithClient(<PlanRunner connectionId="c" member="pg-2" database="perf" sql="UPDATE t SET a = 1" serverVersionNum={170_000} inRecovery />);
    await user.click(screen.getByRole("button", { name: "План с выполнением" }));
    expect(await screen.findByText("Запись недоступна: подключение только для чтения")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});
```

`src/components/queries/QueryTab.test.tsx`:

```tsx
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { QueryDetail } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { connection, member, topology, topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { queryDetail: vi.fn(), explain: vi.fn(), queryLlmBundle: vi.fn(), startStatementsSampler: vi.fn(), stopSession: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn() } };
});

import { api } from "../../api/client";
import { QueryTab } from "./QueryTab";

const stat = { key: { queryId: "7", dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 1200, totalExecMs: 600, meanExecMs: 0.5, minExecMs: 0.1, maxExecMs: 4, stddevExecMs: 0.3, rows: 1200, rowsPerCall: 1, plans: 0, totalPlanMs: 0, sharedHit: 5000, sharedRead: 0, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 30, hitRatio: 1, text: null, textTruncated: false };

const detail = (patch: Partial<QueryDetail> = {}): QueryDetail => ({
  text: "SELECT * FROM shop.customers WHERE id = $1", textTruncated: false, sinceReset: stat,
  windows: [{ windowS: 300, stat: null }, { windowS: 900, stat: null }, { windowS: 3600, stat: { ...stat, calls: 60 } }],
  totals: { calls: 4000, totalExecMs: 2000, rows: 0 }, byUser: [{ rolname: "app", toplevel: true, stat }],
  history: [{ atMs: 1, calls: 10, totalExecMs: 5, meanExecMs: 0.5 }, { atMs: 60_001, calls: 20, totalExecMs: 10, meanExecMs: 0.5 }], gaps: [],
  running: [{ pid: 42, backendStartUs: 1, state: "active", startedMs: 1, durationS: 3.5, waitEventType: "IO", waitEvent: "DataFileRead", usename: "app", applicationName: "api", clientAddr: "10.0.0.5", query: "SELECT * FROM shop.customers WHERE id = 15", truncated: false }],
  waits: { source: "sampler", sinceMs: 0, total: 10, items: [{ eventType: "IO", event: "DataFileRead", count: 7, sharePct: 70 }, { eventType: "CPU", event: null, count: 3, sharePct: 30 }] },
  findings: [], unavailable: [], serverVersionNum: 170_000, inRecovery: false, pgssVersion: "1.11", ...patch,
});

describe("QueryTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.startStatementsSampler).mockResolvedValue("s-1");
    vi.mocked(api.stopSession).mockResolvedValue(true);
  });

  it("обзор: текст, метрики в две колонки, разбивка, экземпляры, ожидания; таблицы — после плана", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("SELECT * FROM shop.customers WHERE id = $1")).toBeInTheDocument();
    expect(api.queryDetail).toHaveBeenCalledWith("c", "pg-1", "perf", "7");
    expect(screen.getByRole("table", { name: "Метрики" })).toHaveTextContent("1 200");
    expect(screen.getByRole("table", { name: "Метрики" })).toHaveTextContent("60");
    expect(screen.getByRole("table", { name: "Выполняющиеся экземпляры" })).toHaveTextContent("42");
    expect(screen.getByRole("table", { name: "Ожидания" })).toHaveTextContent("IO:DataFileRead");
    expect(screen.getByText("Таблицы появятся после первого плана")).toBeInTheDocument();
    expect(api.startStatementsSampler).toHaveBeenCalledWith("c", "pg-1", expect.any(Function));
    // «Открыть в консоли» подставляет текст в консоль подключения.
    await user.click(screen.getByRole("button", { name: "Открыть в консоли" }));
    expect(useWorkspace.getState().tabs.at(-1)).toMatchObject({ kind: "explain", connectionId: "c", member: "pg-1", database: "perf", sql: "SELECT * FROM shop.customers WHERE id = $1" });
  });

  it("подвкладка «План»: источник — текст экземпляра, пробный план даёт таблицы в обзоре", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail());
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: (await import("../../test/plans")).planTree(), raw: [], relations: [{ schema: "shop", name: "customers" }], elapsedMs: 5, warnings: [] });
    const user = userEvent.setup();
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    await screen.findByText("SELECT * FROM shop.customers WHERE id = $1");
    await user.click(screen.getByRole("tab", { name: "План" }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Текст запроса" }), "running:42");
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ sql: "SELECT * FROM shop.customers WHERE id = 15", params: { kind: "none" } })));
    await user.click(screen.getByRole("tab", { name: "Обзор" }));
    expect(await screen.findByRole("button", { name: "shop.customers" })).toBeInTheDocument();
  });

  it("запрос исчез из pgss — пометка без ошибки; на PG 13 экземпляры недоступны", async () => {
    vi.mocked(api.queryDetail).mockResolvedValue(detail({ text: null, sinceReset: null, byUser: [], running: [], serverVersionNum: 130_000, unavailable: [{ part: "running", reason: { key: "diag.unavailable.version", params: { part: "running", version: "14" }, text: "running: PostgreSQL 14+" } }] }));
    renderWithClient(<QueryTab connectionId="c" member="pg-1" database="perf" queryId="7" active />);
    expect(await screen.findByText("Запроса больше нет в pg_stat_statements")).toBeInTheDocument();
    expect(screen.getByText("Выполняющиеся экземпляры (query_id): PostgreSQL 14+")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- QueryTab PlanRunner llmMarkdown`
Expected: FAIL.

- [ ] **Step 3: Хуки и markdown**

`src/lib/useQueryDetail.ts`:

```ts
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "../api/client";

/** Экземпляры перечитываются раз в 5 с, пока вкладка видна (спец. P3 §7.3). */
const RUNNING_INTERVAL_MS = 5000;

export const queryDetailKey = (connectionId: string, member: string, database: string, queryId: string) => ["queryDetail", connectionId, member, database, queryId] as const;

/** Карточка запроса: перечитывается по интервалу (экземпляры) и по тику сэмплера pgss (`tickAtMs` — история и окна). */
export function useQueryDetail(connectionId: string, member: string, database: string, queryId: string, opts: { active: boolean; tickAtMs: number | null }) {
  const q = useQuery({
    queryKey: queryDetailKey(connectionId, member, database, queryId),
    queryFn: () => api.queryDetail(connectionId, member, database, queryId),
    placeholderData: keepPreviousData,
    refetchInterval: opts.active ? RUNNING_INTERVAL_MS : false,
  });
  const { refetch } = q;
  useEffect(() => {
    if (opts.tickAtMs !== null && opts.active) void refetch({ cancelRefetch: false });
  }, [opts.tickAtMs, opts.active, refetch]);
  return q;
}
```

`src/lib/useExplain.ts`:

```ts
import { useMutation } from "@tanstack/react-query";
import { api } from "../api/client";
import type { ExplainRequest, ExplainResult } from "../api/types";

/** Зеркало `explain::params::count_params`: наибольший номер `$N`. */
export function countParams(sql: string): number {
  let max = 0;
  for (const m of sql.matchAll(/\$(\d+)/g)) max = Math.max(max, Number(m[1]));
  return max;
}

/** Мутация EXPLAIN: один запуск за раз; ошибки — `AppError` из `toAppError` на месте показа. */
export function useExplain(connectionId: string) {
  return useMutation({ mutationFn: (request: ExplainRequest): Promise<ExplainResult> => api.explain(connectionId, request) });
}
```

`src/lib/llmMarkdown.ts`:

```ts
import type { TFunction } from "i18next";
import type { LlmBundle, QueryStat } from "../api/types";
import { formatBytes, formatCount, formatMs, formatPercent } from "./format";

/** `170004` → `17.4`; `130012` → `13.12`. */
export const versionText = (num: number) => `${Math.floor(num / 10_000)}.${num % 10_000}`;

function statsTable(s: QueryStat, t: TFunction): string {
  const rows: [string, string][] = [
    [t("llm.stats.calls"), formatCount(s.calls)],
    [t("llm.stats.total"), formatMs(s.totalExecMs)],
    [t("llm.stats.mean"), formatMs(s.meanExecMs)],
    [t("llm.stats.minMax"), s.minExecMs === null || s.maxExecMs === null ? "—" : `${formatMs(s.minExecMs)} / ${formatMs(s.maxExecMs)}`],
    [t("llm.stats.stddev"), s.stddevExecMs === null ? "—" : formatMs(s.stddevExecMs)],
    [t("llm.stats.rowsPerCall"), formatCount(s.rowsPerCall, 1)],
    [t("llm.stats.hitRatio"), s.hitRatio === null ? "—" : formatPercent(s.hitRatio * 100)],
    [t("llm.stats.read"), formatBytes(s.sharedRead * 8192)],
    [t("llm.stats.temp"), formatBytes(s.tempWritten * 8192)],
    [t("llm.stats.wal"), formatBytes(s.walBytes)],
    [t("llm.stats.planning"), formatMs(s.totalPlanMs)],
  ];
  return [`| ${t("llm.stats.metric")} | ${t("llm.stats.value")} |`, "|---|---|", ...rows.map(([k, v]) => `| ${k} | ${v} |`)].join("\n");
}

/** Markdown для LLM (спец. P3 §7.4): заголовки — на языке интерфейса; данные — как есть; промптов нет. */
export function llmMarkdown(b: LlmBundle, t: TFunction): string {
  const out: string[] = [`# ${t("llm.title")}`, ""];
  out.push(`## ${t("llm.env")}`, "", `- PostgreSQL ${versionText(b.serverVersionNum)}${b.inRecovery ? ` (${t("llm.replica")})` : ""}`);
  if (b.pgssVersion) out.push(`- pg_stat_statements ${b.pgssVersion}`);
  out.push(`- ${t("llm.node")}: ${b.member}, ${t("llm.database")}: ${b.database}`, "");
  if (b.text) out.push(`## ${t("llm.query")}`, "", "```sql", b.text, "```", b.textTruncated ? `_${t("llm.truncated")}_` : "", "");
  if (b.stats) {
    out.push(`## ${t("llm.statistics")}`, "");
    if (b.stats.sinceReset) out.push(`### ${t("llm.sinceReset")}`, "", statsTable(b.stats.sinceReset, t), "");
    if (b.stats.window3600) out.push(`### ${t("llm.lastHour")}`, "", statsTable(b.stats.window3600, t), "");
  }
  if (b.planText) out.push(`## ${t("llm.plan")}`, "", "```", b.planText.trimEnd(), "```", "");
  if (b.tables.length > 0 || b.tablesOmitted > 0) {
    out.push(`## ${t("llm.tables")}`, "");
    for (const tb of b.tables) out.push(`### ${tb.schema}.${tb.name}`, "", "```sql", [tb.ddl.trimEnd(), ...tb.indexes].join("\n"), "```", "");
    if (b.tablesOmitted > 0) out.push(`_${t("llm.moreTables", { count: b.tablesOmitted })}_`, "");
  }
  if (b.warnings.length > 0) out.push(`## ${t("llm.notes")}`, "", ...b.warnings.map((w) => `- ${t(`llm.warnings.${w}`)}`), "");
  return `${out.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd()}\n`;
}
```

- [ ] **Step 4: Компоненты плана**

`src/components/queries/plan/ParamsForm.tsx`:

```tsx
import { useTranslation } from "react-i18next";

/** Поля значений `$1…$N`; пустое поле — NULL. На 16+ без ANALYZE можно выбрать GENERIC_PLAN вместо значений. */
export function ParamsForm({ count, values, onChange, generic, canGeneric, onGeneric }: { count: number; values: (string | null)[]; onChange: (v: (string | null)[]) => void; generic: boolean; canGeneric: boolean; onGeneric: (g: boolean) => void }) {
  const { t } = useTranslation();
  if (count === 0) return null;
  return (
    <fieldset className="params-form">
      <legend>{t("diag.plan.params.title", { count })}</legend>
      {canGeneric && (
        <label className="inline">
          <input type="checkbox" checked={generic} onChange={(e) => onGeneric(e.target.checked)} />
          <span>{t("diag.plan.params.generic")}</span>
        </label>
      )}
      {!generic &&
        Array.from({ length: count }, (_, i) => (
          <label key={i} className="inline">
            <span>${i + 1}</span>
            <input type="text" aria-label={`$${i + 1}`} value={values[i] ?? ""} placeholder={t("diag.plan.params.nullPlaceholder")} onChange={(e) => { const next = [...values]; next[i] = e.target.value === "" ? null : e.target.value; onChange(next); }} />
          </label>
        ))}
    </fieldset>
  );
}
```

`src/components/queries/plan/AnalyzeConfirm.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { Confirmation, ConnectionProfile, RelationRef } from "../../../api/types";
import { ConfirmDialog } from "../../common/ConfirmDialog";

/** Подтверждение яруса записи (спец. P3 §8.3 п. 4): таблицы из пробного плана и что откат не отменяет. */
export function AnalyzeConfirm({ connection, relations, busy, onConfirm, onCancel }: { connection: ConnectionProfile; relations: RelationRef[]; busy?: boolean; onConfirm: (c: Confirmation) => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <ConfirmDialog connection={connection} title={t("diag.plan.confirm.title")} confirmLabel={t("diag.plan.confirm.run")} danger busy={busy} onConfirm={onConfirm} onCancel={onCancel}>
      <p>{t("diag.plan.confirm.rollback")}</p>
      <ul>
        {relations.map((r) => <li key={`${r.schema ?? ""}.${r.name}`}>{r.schema ? `${r.schema}.${r.name}` : r.name}</li>)}
      </ul>
      <p className="muted">{t("diag.plan.confirm.sideEffects")}</p>
    </ConfirmDialog>
  );
}
```

`src/components/queries/plan/PlanRunner.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toAppError } from "../../../api/client";
import type { AppError, Confirmation, ExplainMode, ExplainParams, ExplainResult } from "../../../api/types";
import { formatMs, formatTime } from "../../../lib/format";
import { countParams, useExplain } from "../../../lib/useExplain";
import { canAct, useConnectionProfile } from "../../../lib/useConnectionProfile";
import { ErrorBanner } from "../../common/ErrorBanner";
import { AnalyzeConfirm } from "./AnalyzeConfirm";
import { ParamsForm } from "./ParamsForm";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanSummary } from "./PlanSummary";
import { PlanTreeView } from "./PlanTree";

const TIMEOUTS_S = [5, 15, 30, 60, 300, 600] as const;
/** Прошлых планов в истории вкладки. */
const HISTORY = 10;

interface Run {
  id: number;
  at: number;
  result: ExplainResult;
}

/**
 * Запуск EXPLAIN (спец. P3 §8): параметры (`$N` → поля или GENERIC_PLAN на 16+), срок, `search_path`, «План» и «План с
 * выполнением». Ярус записи: пробный план → подтверждение со списком таблиц → ANALYZE. `EXPLAIN_WRITES` даёт кнопку
 * «Выполнить как запись» (`forceWrite`). История запусков — в пределах компонента.
 */
export function PlanRunner({ connectionId, member, database, sql, serverVersionNum, inRecovery, onResult }: { connectionId: string; member: string; database: string; sql: string; serverVersionNum: number; inRecovery: boolean; onResult?: (r: ExplainResult) => void }) {
  const { t } = useTranslation();
  const profile = useConnectionProfile(connectionId);
  const count = countParams(sql);
  const canGeneric = serverVersionNum >= 160_000 && count > 0;
  const [generic, setGeneric] = useState(true);
  const [values, setValues] = useState<(string | null)[]>([]);
  const [timeoutS, setTimeoutS] = useState<number>(30);
  const [searchPath, setSearchPath] = useState("");
  const [runs, setRuns] = useState<Run[]>([]);
  const [shown, setShown] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<{ relations: ExplainResult["relations"]; force: boolean } | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [writeHint, setWriteHint] = useState<string | null>(null);
  const explain = useExplain(connectionId);
  const seq = useState(() => ({ n: 0 }))[0];

  const params = (mode: ExplainMode): ExplainParams => (count === 0 ? { kind: "none" } : mode === "plan" && generic && canGeneric ? { kind: "generic" } : { kind: "values", values: Array.from({ length: count }, (_, i) => values[i] ?? null) });
  const request = (mode: ExplainMode, confirmation: Confirmation, forceWrite: boolean) => ({ member, database, sql, mode, params: params(mode), timeoutS, searchPath: searchPath.trim() || null, confirmation, forceWrite });

  const remember = (result: ExplainResult) => {
    const run: Run = { id: ++seq.n, at: Date.now(), result };
    setRuns((r) => [run, ...r].slice(0, HISTORY));
    setShown(run.id);
    setSelected(null);
    onResult?.(result);
  };

  const fail = (e: unknown) => {
    const err = toAppError(e);
    setError(err);
    if (err.code === "EXPLAIN_WRITES") setWriteHint("force");
  };

  const runPlan = async () => {
    setError(null);
    setWriteHint(null);
    try {
      remember(await explain.mutateAsync(request("plan", { confirm: null, typedName: null }, false)));
    } catch (e) {
      fail(e);
    }
  };

  /** ANALYZE: ярус определяется пробным планом; запись — только с подтверждением (и только когда действия доступны). */
  const runAnalyze = async (force = false) => {
    setError(null);
    setWriteHint(null);
    try {
      const probe = force ? null : await explain.mutateAsync(request("plan", { confirm: null, typedName: null }, false));
      if (probe && probe.tier === "read") {
        remember(await explain.mutateAsync(request("analyze", { confirm: null, typedName: null }, false)));
        return;
      }
      if (inRecovery) {
        setWriteHint("replica");
        return;
      }
      if (!canAct(profile)) {
        setWriteHint("readOnly");
        return;
      }
      setConfirm({ relations: probe?.relations ?? [], force });
    } catch (e) {
      fail(e);
    }
  };

  const confirmed = async (c: Confirmation) => {
    const force = confirm?.force ?? false;
    setConfirm(null);
    try {
      remember(await explain.mutateAsync(request("analyze", c, force)));
    } catch (e) {
      fail(e);
    }
  };

  const current = runs.find((r) => r.id === shown) ?? null;
  const node = current ? findNode(current.result.plan.root, selected) : null;
  const busy = explain.isPending;
  return (
    <div className="plan-runner">
      <ParamsForm count={count} values={values} onChange={setValues} generic={generic} canGeneric={canGeneric} onGeneric={setGeneric} />
      <div className="toolbar">
        <button type="button" className="primary" disabled={busy} onClick={() => void runPlan()}>{t("diag.plan.run.plan")}</button>
        <button type="button" disabled={busy} onClick={() => void runAnalyze()}>{t("diag.plan.run.analyze")}</button>
        <label className="inline">
          <span>{t("diag.plan.run.timeout")}</span>
          <select aria-label={t("diag.plan.run.timeout")} value={timeoutS} onChange={(e) => setTimeoutS(Number(e.target.value))}>
            {TIMEOUTS_S.map((s) => <option key={s} value={s}>{formatMs(s * 1000)}</option>)}
          </select>
        </label>
        <details className="inline">
          <summary>search_path</summary>
          <input type="text" aria-label="search_path" value={searchPath} placeholder={t("diag.plan.run.searchPathDefault")} onChange={(e) => setSearchPath(e.target.value)} />
        </details>
      </div>
      {error && <ErrorBanner error={error} />}
      {writeHint === "force" && (
        <p className="diag-note" role="status">
          {t("diag.plan.run.writesHint")} <button type="button" onClick={() => void runAnalyze(true)}>{t("diag.plan.run.asWrite")}</button>
        </p>
      )}
      {writeHint === "replica" && <p className="diag-note" role="status">{t("diag.plan.run.writeOnReplica")}</p>}
      {writeHint === "readOnly" && <p className="diag-note" role="status">{t("diag.plan.run.writeReadOnly")}</p>}
      {confirm && profile && <AnalyzeConfirm connection={profile} relations={confirm.relations} busy={busy} onConfirm={(c) => void confirmed(c)} onCancel={() => setConfirm(null)} />}
      {current && (
        <>
          <PlanSummary tree={current.result.plan} tier={current.result.tier} warnings={current.result.warnings} raw={current.result.raw} elapsedMs={current.result.elapsedMs} />
          <PlanTreeView tree={current.result.plan} selectedId={selected} onSelect={setSelected} />
          {node && <PlanNodeDetails node={node} />}
        </>
      )}
      {runs.length > 1 && (
        <section className="plan-history">
          <h3>{t("diag.plan.history.title")}</h3>
          <ul>
            {runs.map((r) => (
              <li key={r.id}>
                <button type="button" className="link" aria-current={r.id === shown} onClick={() => { setShown(r.id); setSelected(null); }}>
                  {t("diag.plan.history.item", { n: r.id, mode: t(`diag.plan.history.modes.${r.result.mode}`), tier: t(`diag.plan.tiers.${r.result.tier}`), time: formatTime(r.at), ms: formatMs(r.result.elapsedMs) })}
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}

function findNode(root: ExplainResult["plan"]["root"], id: number | null): ExplainResult["plan"]["root"] | null {
  if (id === null) return null;
  if (root.id === id) return root;
  for (const c of root.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}
```

- [ ] **Step 5: Карточка**

`src/components/queries/Sparkline.tsx`:

```tsx
/** Спарклайн SVG по точкам (значения ≥ 0); пустой список — пустая область с подписью. */
export function Sparkline({ points, label }: { points: { atMs: number; value: number }[]; label: string }) {
  const w = 240;
  const h = 40;
  if (points.length === 0) return <svg className="sparkline" role="img" aria-label={label} width={w} height={h} />;
  const max = Math.max(...points.map((p) => p.value), 1);
  const x = (i: number) => (points.length === 1 ? w / 2 : (i / (points.length - 1)) * (w - 2) + 1);
  const y = (v: number) => h - 1 - (v / max) * (h - 2);
  const d = points.map((p, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)},${y(p.value).toFixed(1)}`).join(" ");
  return (
    <svg className="sparkline" role="img" aria-label={label} width={w} height={h} viewBox={`0 0 ${w} ${h}`}>
      <title>{label}</title>
      <path d={d} fill="none" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}
```

`src/components/queries/CopyLlmButton.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, LlmSource, PlanTree } from "../../api/types";
import { llmMarkdown } from "../../lib/llmMarkdown";

/** «Скопировать для LLM» (спец. P3 §7.4): данные из Rust, markdown — здесь, в буфер обмена. */
export function CopyLlmButton({ connectionId, member, database, source, plan }: { connectionId: string; member: string; database: string; source: LlmSource; plan: PlanTree | null }) {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "busy" | "copied" | "failed">("idle");
  const [error, setError] = useState<AppError | null>(null);
  const copy = async () => {
    setState("busy");
    setError(null);
    try {
      const bundle = await api.queryLlmBundle(connectionId, member, database, source, plan);
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(llmMarkdown(bundle, t));
      setState("copied");
    } catch (e) {
      setError(e instanceof Error && e.message === "clipboard unavailable" ? null : toAppError(e));
      setState("failed");
    }
    window.setTimeout(() => setState("idle"), 2500);
  };
  return (
    <>
      <button type="button" disabled={state === "busy"} onClick={() => void copy()}>{t("diag.query.copyLlm")}</button>
      <span role="status" className="muted">{state === "copied" ? t("test.copied") : state === "failed" ? (error ? error.message : t("test.copyFailed")) : ""}</span>
    </>
  );
}
```

`src/components/queries/QueryOverview.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { QueryDetail, QueryStat, RelationRef } from "../../api/types";
import { formatBytes, formatCount, formatDuration, formatMs, formatPercent } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { FindingsPanel } from "../diag/FindingsPanel";
import { unavailableText } from "../diag/unavailable";
import { Sparkline } from "./Sparkline";

const METRICS: { key: string; of: (s: QueryStat) => string }[] = [
  { key: "calls", of: (s) => formatCount(s.calls) },
  { key: "total", of: (s) => formatMs(s.totalExecMs) },
  { key: "mean", of: (s) => formatMs(s.meanExecMs) },
  { key: "minMax", of: (s) => (s.minExecMs === null || s.maxExecMs === null ? "—" : `${formatMs(s.minExecMs)} / ${formatMs(s.maxExecMs)}`) },
  { key: "stddev", of: (s) => (s.stddevExecMs === null ? "—" : formatMs(s.stddevExecMs)) },
  { key: "rowsPerCall", of: (s) => formatCount(s.rowsPerCall, 1) },
  { key: "hitRatio", of: (s) => (s.hitRatio === null ? "—" : formatPercent(s.hitRatio * 100)) },
  { key: "read", of: (s) => `${formatBytes(s.sharedRead * 8192)} / ${formatBytes(s.sharedWritten * 8192)}` },
  { key: "temp", of: (s) => formatBytes(s.tempWritten * 8192) },
  { key: "wal", of: (s) => formatBytes(s.walBytes) },
  { key: "planning", of: (s) => formatMs(s.totalPlanMs) },
  { key: "jit", of: (s) => (s.jitMs === null ? "—" : formatMs(s.jitMs)) },
];

/** Обзор карточки (спец. P3 §7.3): текст, метрики (с момента сброса и за час), история, разбивка, экземпляры, ожидания, таблицы. */
export function QueryOverview({ connectionId, member, database, detail, relations, onPlan, onConsole }: { connectionId: string; member: string; database: string; detail: QueryDetail; relations: RelationRef[] | null; onPlan: () => void; onConsole: () => void }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const hour = detail.windows.find((w) => w.windowS === 3600)?.stat ?? null;
  return (
    <div className="query-overview">
      <section>
        {detail.text === null ? (
          <p className="muted">{t("diag.query.gone")}</p>
        ) : (
          <pre className="query-text" tabIndex={0} aria-label={t("diag.query.text")}><code>{detail.text}</code>{detail.textTruncated && <span className="muted"> {t("diag.queries.truncated")}</span>}</pre>
        )}
        <div className="toolbar">
          <button type="button" onClick={onPlan}>{t("diag.query.plan")}</button>
          <button type="button" onClick={onConsole}>{t("diag.query.openConsole")}</button>
        </div>
      </section>
      <FindingsPanel connectionId={connectionId} findings={detail.findings} member={member} />
      {detail.unavailable.map((u, i) => <p key={`${u.part}:${i}`} className="muted">{unavailableText(u)}</p>)}
      {detail.sinceReset && (
        <section>
          <h3>{t("diag.query.metrics")}</h3>
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.metrics")}>
              <thead><tr><th scope="col" /><th scope="col" className="num">{t("diag.query.sinceReset")}</th><th scope="col" className="num">{t("diag.query.lastHour")}</th></tr></thead>
              <tbody>
                {METRICS.map((m) => (
                  <tr key={m.key}><th scope="row">{t(`diag.query.metric.${m.key}`)}</th><td className="num">{m.of(detail.sinceReset!)}</td><td className="num">{hour ? m.of(hour) : "—"}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
      <section className="query-history">
        <h3>{t("diag.query.history")}</h3>
        <Sparkline points={detail.history.map((p) => ({ atMs: p.atMs, value: p.calls }))} label={t("diag.query.historyCalls")} />
        <Sparkline points={detail.history.map((p) => ({ atMs: p.atMs, value: p.meanExecMs }))} label={t("diag.query.historyMean")} />
        {detail.history.length === 0 && <p className="muted">{t("diag.query.noHistory")}</p>}
        {detail.gaps.length > 0 && <p className="muted">{t("diag.query.gaps", { count: detail.gaps.length })}</p>}
      </section>
      {detail.byUser.length > 1 && (
        <section>
          <h3>{t("diag.query.byUser")}</h3>
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.byUser")}>
              <thead><tr><th scope="col">{t("diag.query.role")}</th><th scope="col">{t("diag.query.toplevel")}</th><th scope="col" className="num">{t("diag.query.metric.calls")}</th><th scope="col" className="num">{t("diag.query.metric.total")}</th><th scope="col" className="num">{t("diag.query.metric.mean")}</th></tr></thead>
              <tbody>
                {detail.byUser.map((u) => (
                  <tr key={`${u.rolname ?? ""}:${u.toplevel}`}><td>{u.rolname ?? "—"}</td><td>{u.toplevel ? t("common.yes") : t("common.no")}</td><td className="num">{formatCount(u.stat.calls)}</td><td className="num">{formatMs(u.stat.totalExecMs)}</td><td className="num">{formatMs(u.stat.meanExecMs)}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}
      <section>
        <h3>{t("diag.query.running")}</h3>
        {detail.running.length === 0 ? (
          <p className="muted">{t("diag.query.noRunning")}</p>
        ) : (
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.running")}>
              <thead><tr><th scope="col">PID</th><th scope="col">{t("diag.query.state")}</th><th scope="col" className="num">{t("diag.query.duration")}</th><th scope="col">{t("diag.query.wait")}</th><th scope="col">{t("diag.query.who")}</th></tr></thead>
              <tbody>
                {detail.running.map((r) => (
                  <tr key={`${r.pid}:${r.backendStartUs}`}><td>{r.pid}</td><td>{r.state ?? ""}</td><td className="num">{formatDuration(r.durationS)}</td><td>{r.waitEventType ? `${r.waitEventType}${r.waitEvent ? `:${r.waitEvent}` : ""}` : ""}</td><td>{[r.usename, r.applicationName, r.clientAddr].filter(Boolean).join(" · ")}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <button type="button" className="link" onClick={() => open({ kind: "diag", connectionId, tool: "sessions", member })}>{t("diag.query.openSessions")}</button>
      </section>
      <section>
        <h3>{t("diag.query.waits")}</h3>
        {detail.waits === null ? (
          <p className="muted">{t("diag.query.noWaitsBuffer")}</p>
        ) : (
          <div className="table-scroll">
            <table className="diag-table" aria-label={t("diag.query.waits")}>
              <thead><tr><th scope="col">{t("diag.query.waitEvent")}</th><th scope="col" className="num">{t("diag.query.share")}</th><th scope="col" className="num">{t("diag.query.samples")}</th></tr></thead>
              <tbody>
                {detail.waits.items.map((w) => (
                  <tr key={`${w.eventType}:${w.event ?? ""}`}><td>{w.event ? `${w.eventType}:${w.event}` : w.eventType}</td><td className="num">{formatPercent(w.sharePct)}</td><td className="num">{formatCount(w.count)}</td></tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <section>
        <h3>{t("diag.query.tables")}</h3>
        {relations === null ? (
          <p className="muted">{t("diag.query.tablesAfterPlan")} <button type="button" className="link" onClick={onPlan}>{t("diag.query.getPlan")}</button></p>
        ) : relations.length === 0 ? (
          <p className="muted">{t("diag.query.noTables")}</p>
        ) : (
          <ul>
            {relations.map((r) => (
              <li key={`${r.schema ?? ""}.${r.name}`}>
                <button type="button" className="link" onClick={() => open({ kind: "table", connectionId, database, schema: r.schema ?? "public", name: r.name })}>{r.schema ? `${r.schema}.${r.name}` : r.name}</button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
```

`src/components/queries/QueryPlan.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainResult, QueryDetail } from "../../api/types";
import { PlanRunner } from "./plan/PlanRunner";

/** Источник текста: нормализованный pgss или текст выполняющегося экземпляра (с литералами; обрезанный — нельзя). */
export function QueryPlan({ connectionId, member, database, detail, onResult }: { connectionId: string; member: string; database: string; detail: QueryDetail; onResult: (r: ExplainResult) => void }) {
  const { t } = useTranslation();
  const [source, setSource] = useState("pgss");
  const running = detail.running.find((r) => `running:${r.pid}` === source);
  const sql = running ? running.query : detail.text;
  return (
    <div className="query-plan">
      <label className="inline">
        <span>{t("diag.query.source")}</span>
        <select aria-label={t("diag.query.source")} value={source} onChange={(e) => setSource(e.target.value)}>
          <option value="pgss">{t("diag.query.sourcePgss")}</option>
          {detail.running.map((r) => (
            <option key={r.pid} value={`running:${r.pid}`} disabled={r.truncated}>
              {t("diag.query.sourceRunning", { pid: r.pid })}{r.truncated ? ` — ${t("diag.query.sourceTruncated")}` : ""}
            </option>
          ))}
        </select>
      </label>
      {sql ? (
        <>
          <pre className="query-text"><code>{sql}</code></pre>
          <PlanRunner key={source} connectionId={connectionId} member={member} database={database} sql={sql} serverVersionNum={detail.serverVersionNum} inRecovery={detail.inRecovery} onResult={onResult} />
        </>
      ) : (
        <p className="muted">{t("diag.query.gone")}</p>
      )}
    </div>
  );
}
```

`src/components/queries/QueryTab.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainResult } from "../../api/types";
import { useStatementsSampler } from "../../lib/useStatementsSampler";
import { useQueryDetail } from "../../lib/useQueryDetail";
import { useTopology } from "../../lib/useConnectionEvents";
import { useWorkspace } from "../../state/workspace";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { QueryOverview } from "./QueryOverview";
import { QueryPlan } from "./QueryPlan";
import { queryTitle } from "./queriesText";

type Section = "overview" | "plan";

/**
 * Вкладка запроса (спец. P3 §7): шапка с текстом и узлом, подвкладки «Обзор» и «План». Подписана на сэмплер pgss узла
 * (история копится, пока открыта хотя бы одна вкладка); таблицы запроса — из последнего плана, хранятся в состоянии вкладки.
 */
export function QueryTab({ connectionId, member, database, queryId, active }: { connectionId: string; member: string; database: string; queryId: string; active: boolean }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const view = useTopology(connectionId);
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const sampler = useStatementsSampler(connectionId, member, live);
  const q = useQueryDetail(connectionId, member, database, queryId, { active: active && live, tickAtMs: sampler.live?.atMs ?? null });
  const [section, setSection] = useState<Section>("overview");
  const [last, setLast] = useState<ExplainResult | null>(null);
  const detail = q.data;
  const members = view.data?.topology && view.data.topology.kind !== "server" ? view.data.topology.members : null;
  const switchMember = (m: string) => open({ kind: "query", connectionId, member: m, database, queryId, title: queryTitle(detail?.text ?? null, queryId) });
  if (!live) return <div className="page"><p className="muted">{t("diag.closed")}</p></div>;
  return (
    <div className="page query-tab">
      <div className="toolbar">
        <h2 className="mono">{queryTitle(detail?.text ?? null, queryId, 80)}</h2>
        <span className="muted">{database} · {queryId}</span>
        {members && (
          <label className="inline">
            <span>{t("diag.header.node")}</span>
            <select aria-label={t("diag.header.node")} value={member} onChange={(e) => switchMember(e.target.value)}>
              {members.map((m) => <option key={m.name} value={m.name}>{m.name}</option>)}
            </select>
          </label>
        )}
        <CopyLlmButton connectionId={connectionId} member={member} database={database} source={{ kind: "query", queryId }} plan={last?.plan ?? null} />
      </div>
      {q.error && <ErrorBanner error={q.error} onRetry={() => void q.refetch()} />}
      {sampler.error && <ErrorBanner error={sampler.error} />}
      {detail && (
        <SubTabs<Section> label={t("diag.query.sections")} tabs={[{ id: "overview", label: t("diag.query.overview") }, { id: "plan", label: t("diag.query.plan") }]} value={section} onChange={setSection}>
          {(s) =>
            s === "overview" ? (
              <QueryOverview connectionId={connectionId} member={member} database={database} detail={detail} relations={last?.relations ?? null} onPlan={() => setSection("plan")} onConsole={() => open({ kind: "explain", connectionId, member, database, sql: detail.text ?? "" })} />
            ) : (
              <QueryPlan connectionId={connectionId} member={member} database={database} detail={detail} onResult={setLast} />
            )
          }
        </SubTabs>
      )}
      {!detail && !q.error && <p className="empty">{t("common.loading")}</p>}
    </div>
  );
}
```

- [ ] **Step 6: Включение в приложение**

`src/components/TabContent.tsx`: `import { QueryTab } from "./queries/QueryTab";` и `case "query": return <QueryTab connectionId={tab.connectionId} member={tab.member} database={tab.database} queryId={tab.queryId} active={active} />;`.

`src/lib/useConnectionEvents.ts`: `PG_QUERY_KEYS = ["catalog", "memberFacts", "diag", "queryDetail"]`.

`src/components/diag/FindingsPanel.tsx`: убрать временный фильтр `l.kind !== "openQuery"` (задача 12); в фильтре `links` — `openQuery` остаётся, если известен узел (`(l.member ?? member ?? null) !== null`); в разметке ссылок третья ветвь:

```tsx
                      ) : link.kind === "openQuery" ? (
                        <button key={`query:${link.database}:${link.queryId}`} type="button" onClick={() => { const node = link.member ?? member ?? null; if (node !== null) open({ kind: "query", connectionId, member: node, database: link.database, queryId: link.queryId }); }}>
                          {t("diag.findings.openQuery")}
                        </button>
                      ) : (
```

CSS: `.query-text { max-height: 200px; overflow: auto; white-space: pre-wrap; } .query-overview section, .plan-runner section { margin-top: 12px; } .params-form { display: flex; flex-wrap: wrap; gap: 8px; } .sparkline { color: var(--accent, #4f8cff); margin-right: 12px; }`.

Словари (en / ru): `diag.findings.openQuery` "Open query"/"Открыть запрос"; `diag.query`: `sections` "Query sections"/"Разделы запроса"; `overview` "Overview"/"Обзор"; `plan` "Plan"/"План"; `text` "Query text"/"Текст запроса"; `gone` "The query is no longer in pg_stat_statements"/"Запроса больше нет в pg_stat_statements"; `openConsole` "Open in console"/"Открыть в консоли"; `copyLlm` "Copy for LLM"/"Скопировать для LLM"; `metrics` "Metrics"/"Метрики"; `sinceReset` "Since reset"/"С момента сброса"; `lastHour` "Last 60 min"/"За 60 мин"; `metric`: `calls` "Calls"/"Вызовов", `total` "Total time"/"Общее время", `mean` "Mean"/"Среднее", `minMax` "Min / max"/"Min / max", `stddev` "Stddev"/"Stddev", `rowsPerCall` "Rows per call"/"Строк на вызов", `hitRatio` "Cache hit"/"Попадание в кэш", `read` "Read / written"/"Прочитано / записано", `temp` "Temp written"/"Temp записано", `wal` "WAL"/"WAL", `planning` "Planning"/"Планирование", `jit` "JIT"/"JIT"; `history` "History (60 min)"/"История (60 мин)"; `historyCalls` "Calls per minute"/"Вызовов в минуту"; `historyMean` "Mean time per minute"/"Среднее время по минутам"; `noHistory` "No ticks yet: the sampler collects history while the tab is open"/"Тиков ещё нет: сэмплер копит историю, пока вкладка открыта"; `gaps` "Gaps in history: {{count}}"/"Промежутков в истории: {{count}}"; `byUser` "By role and level"/"По ролям и уровню"; `role` "Role"/"Роль"; `toplevel` "Top level"/"Верхний уровень"; `running` "Running instances"/"Выполняющиеся экземпляры"; `noRunning` "Not running now"/"Сейчас не выполняется"; `state` "State"/"Состояние"; `duration` "Duration"/"Длительность"; `wait` "Wait"/"Ожидание"; `who` "Role · application · client"/"Роль · приложение · клиент"; `openSessions` "Open in Sessions"/"Открыть в сессиях"; `waits` "Waits"/"Ожидания"; `noWaitsBuffer` "Open the Sessions tab to collect waits for this query"/"Откройте «Сессии», чтобы собирать ожидания этого запроса"; `waitEvent` "Event"/"Событие"; `share` "Share"/"Доля"; `samples` "Samples"/"Сэмплов"; `tables` "Tables"/"Таблицы"; `tablesAfterPlan` "Tables appear after the first plan"/"Таблицы появятся после первого плана"; `getPlan` "Get plan"/"Получить план"; `noTables` "The plan touches no tables"/"План не касается таблиц"; `source` "Query text"/"Текст запроса"; `sourcePgss` "Normalized text (pg_stat_statements)"/"Нормализованный текст (pg_stat_statements)"; `sourceRunning` "Running instance PID {{pid}}"/"Выполняющийся экземпляр PID {{pid}}"; `sourceTruncated` "truncated"/"обрезан". `diag.plan.params`: `title` "Parameters ({{count}})"/"Параметры ({{count}})", `generic` "GENERIC_PLAN (no values)"/"GENERIC_PLAN (без значений)", `nullPlaceholder` "NULL"/"NULL"; `diag.plan.run`: `plan` "Plan"/"План", `analyze` "Plan with execution"/"План с выполнением", `timeout` "Timeout"/"Срок", `searchPathDefault` "server default"/"по умолчанию сервера", `writesHint` "The query writes data in a read-only transaction."/"Запрос пишет данные в транзакции только для чтения.", `asWrite` "Run as a write"/"Выполнить как запись", `writeOnReplica` "A write cannot run on a replica: choose the primary"/"Запись на реплике невозможна: выберите primary", `writeReadOnly` "Writes are unavailable: the connection is read-only"/"Запись недоступна: подключение только для чтения"; `diag.plan.confirm`: `title` "Execute the query and roll back"/"Выполнить запрос и откатить", `run` "Execute and roll back"/"Выполнить и откатить", `rollback` "Rows will change and be rolled back. Tables:"/"Строки изменятся и откатятся. Таблицы:", `sideEffects` "Rollback does not undo fired triggers, sequence advances, NOTIFY and external calls; locks are held while the query runs."/"Откат не отменяет сработавшие триггеры, сдвиг последовательностей, NOTIFY и внешние вызовы; блокировки держатся на время выполнения."; `diag.plan.history`: `title` "History"/"История", `item` "#{{n}} {{mode}}, {{tier}}, {{time}}, {{ms}}"/то же, `modes.plan` "plan"/"план", `modes.analyze` "with execution"/"с выполнением". `llm`: `title` "Query context from Glafira for PostgreSQL"/"Контекст запроса из Glafira for PostgreSQL", `env` "Environment"/"Среда", `replica` "replica"/"реплика", `node` "Node"/"Узел", `database` "Database"/"База", `query` "Query"/"Запрос", `truncated` "The text is truncated to 16 384 characters"/"Текст обрезан до 16 384 символов", `statistics` "Statistics"/"Статистика", `sinceReset` "Since reset"/"С момента сброса", `lastHour` "Last 60 minutes"/"За 60 минут", `plan` "Plan"/"План", `tables` "Tables"/"Таблицы", `moreTables` "… {{count}} more tables"/"… ещё {{count}} таблиц", `notes` "Notes"/"Примечания", `warnings.paramsNeeded` "the plan needs parameter values (PostgreSQL < 16)"/"план требует значений параметров (PostgreSQL < 16)", `warnings.queryGone` "the query is no longer in pg_stat_statements"/"запроса больше нет в pg_stat_statements", `warnings.planFailed` "the plan could not be built"/"план не удалось построить", `stats.metric` "Metric"/"Метрика", `stats.value` "Value"/"Значение", `stats.calls` "Calls"/"Вызовов", `stats.total` "Total time"/"Общее время", `stats.mean` "Mean"/"Среднее", `stats.minMax` "Min / max"/"Min / max", `stats.stddev` "Stddev"/"Stddev", `stats.rowsPerCall` "Rows per call"/"Строк на вызов", `stats.hitRatio` "Cache hit"/"Попадание в кэш", `stats.read` "Read from disk"/"Прочитано с диска", `stats.temp` "Temp written"/"Temp записано", `stats.wal` "WAL"/"WAL", `stats.planning` "Planning"/"Планирование". `common.yes`/`common.no` — если ещё нет: "Yes"/"Да", "No"/"Нет".

- [ ] **Step 7: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): карточка запроса — обзор, подвкладка «План» с формой параметров и подтверждением яруса записи, «Скопировать для LLM», ссылка находки на запрос

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 15: Фронтенд — консоль EXPLAIN (`ExplainTab`), вставка плана, пункт в дереве, журнал

**Files:**
- Create: `src/components/queries/ExplainTab.tsx`, `src/components/queries/ExplainTab.test.tsx`
- Modify: `src/components/TabContent.tsx` (`explain`), `src/components/connections/tree/DiagnosticsNodes.tsx` (пункт «Консоль EXPLAIN»), `src/components/actions/ActionsJournal.tsx` (`explainAnalyze`), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: `ExplainTab.test.tsx`, `ActionsJournal.test.tsx` (строка `explainAnalyze`)

**Interfaces:**
- Consumes: `api.{parsePlan, listDatabases}`, `useTopology`, `PlanRunner`, `PlanTreeView`, `PlanNodeDetails`, `PlanSummary`, `CopyLlmButton`, `useWorkspace`.
- Produces: `ExplainTab({ connectionId, member, database, sql, active })` — вкладка `explain`: узел, база, SQL (`textarea`), режим «Запрос» / «Вставить план», `PlanRunner` для запроса, разбор вставленного JSON без сервера; пункт дерева «Консоль EXPLAIN»; в журнале действие `explainAnalyze` с таблицами в колонке «Объект».

- [ ] **Step 1: Падающие тесты**

`src/components/queries/ExplainTab.test.tsx`:

```tsx
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { connection, member, topology, topologyView } from "../../test/fixtures";
import { planTree } from "../../test/plans";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { explain: vi.fn(), parsePlan: vi.fn(), listDatabases: vi.fn(), getTopology: vi.fn(), listConnections: vi.fn(), queryLlmBundle: vi.fn() } };
});

import { api } from "../../api/client";
import { ExplainTab } from "./ExplainTab";

describe("ExplainTab", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    useWorkspace.setState({ tabs: [], activeId: null });
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.listDatabases).mockResolvedValue([{ name: "postgres", sizeBytes: 1 }, { name: "perf", sizeBytes: 1 }]);
  });

  it("узел и база из спеки, SQL подставлен, «План» уходит на выбранный узел и базу", async () => {
    vi.mocked(api.explain).mockResolvedValue({ mode: "plan", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 3, warnings: [] });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-2" database="perf" sql="SELECT 1" active />);
    expect(await screen.findByRole("combobox", { name: "База" })).toHaveValue("perf");
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
    await user.click(screen.getByRole("button", { name: "План" }));
    await waitFor(() => expect(api.explain).toHaveBeenCalledWith("c", expect.objectContaining({ member: "pg-2", database: "perf", sql: "SELECT 1", mode: "plan" })));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
  });

  it("без узла в спеке берётся лидер; «Вставить план» разбирает JSON без сервера", async () => {
    vi.mocked(api.parsePlan).mockResolvedValue(planTree());
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member={null} database={undefined} sql={undefined} active />);
    expect(await screen.findByRole("combobox", { name: "Узел" })).toHaveValue("pg-1");
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("postgres");
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "[[{{]");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    await waitFor(() => expect(api.parsePlan).toHaveBeenCalledWith("[[{]"));
    expect(await screen.findByRole("treegrid", { name: "Дерево плана" })).toBeInTheDocument();
    expect(api.explain).not.toHaveBeenCalled();
  });

  it("ошибка разбора показывается баннером", async () => {
    vi.mocked(api.parsePlan).mockRejectedValue({ code: "INVALID_INPUT", key: "errors.plan.unparsed", params: { line: "1", message: "x" }, message: "The plan could not be parsed (line 1): x", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ExplainTab connectionId="c" member="pg-1" database="perf" sql={undefined} active />);
    await screen.findByRole("combobox", { name: "Узел" });
    await user.click(screen.getByRole("tab", { name: "Вставить план" }));
    await user.type(screen.getByRole("textbox", { name: "План (JSON)" }), "oops");
    await user.click(screen.getByRole("button", { name: "Разобрать" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("План не разобран (строка 1): x");
  });
});
```

В `src/components/actions/ActionsJournal.test.tsx` добавить проверку: запись `{ action: "explainAnalyze", explain: { database: "perf", tables: ["shop.balances"], timeoutS: 30, sqlHead: "UPDATE …" } }` показывает в колонке действия «EXPLAIN ANALYZE (запись)» и в колонке объекта `shop.balances`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- ExplainTab ActionsJournal`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`src/components/queries/ExplainTab.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, PlanTree } from "../../api/types";
import { useTopology } from "../../lib/useConnectionEvents";
import { SubTabs } from "../catalog/SubTabs";
import { ErrorBanner } from "../common/ErrorBanner";
import { CopyLlmButton } from "./CopyLlmButton";
import { PlanNodeDetails } from "./plan/PlanNodeDetails";
import { PlanRunner } from "./plan/PlanRunner";
import { PlanSummary } from "./plan/PlanSummary";
import { PlanTreeView } from "./plan/PlanTree";

type Source = "sql" | "paste";

function findNode(root: PlanTree["root"], id: number | null): PlanTree["root"] | null {
  if (id === null) return null;
  if (root.id === id) return root;
  for (const c of root.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}

/**
 * Консоль EXPLAIN (спец. P3 §8.7): узел, база, SQL и запуск планов через `PlanRunner`; режим «Вставить план» разбирает
 * JSON (в том числе с обрамлением psql) без сервера. Одна на подключение: «Открыть в консоли» подставляет текст сюда.
 */
export function ExplainTab({ connectionId, member, database, sql: initialSql, active }: { connectionId: string; member: string | null | undefined; database: string | undefined; sql: string | undefined; active: boolean }) {
  const { t } = useTranslation();
  const view = useTopology(connectionId);
  const topo = view.data?.topology ?? null;
  const live = view.data?.state === "connected" || view.data?.state === "degraded";
  const dbs = useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId), enabled: live });
  const leader = topo?.members.find((m) => m.role === "primary")?.name ?? topo?.members[0]?.name ?? null;
  const [node, setNode] = useState<string | null>(member ?? null);
  const [db, setDb] = useState<string | null>(database ?? null);
  const [sql, setSql] = useState(initialSql ?? "");
  const [source, setSource] = useState<Source>("sql");
  const [pasted, setPasted] = useState("");
  const [parsed, setParsed] = useState<PlanTree | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [parseError, setParseError] = useState<AppError | null>(null);
  // Повторное «Открыть в консоли» меняет спеку вкладки: новые узел, база и текст подставляются.
  useEffect(() => {
    if (member !== undefined) setNode(member);
    if (database !== undefined) setDb(database);
    if (initialSql !== undefined) setSql(initialSql);
  }, [member, database, initialSql]);
  const nodeName = node ?? leader;
  const dbName = db ?? dbs.data?.[0]?.name ?? null;
  const memberInfo = topo?.members.find((m) => m.name === nodeName);
  const inRecovery = memberInfo ? memberInfo.role !== "primary" : false;
  const serverVersionNum = view.data?.topology?.serverVersionNum ?? 0;
  const parse = async () => {
    setParseError(null);
    try {
      setParsed(await api.parsePlan(pasted));
      setSelected(null);
    } catch (e) {
      setParseError(toAppError(e));
    }
  };
  if (!live) return <div className="page"><p className="muted">{t("diag.closed")}</p></div>;
  return (
    <div className="page explain-tab">
      <div className="toolbar">
        <h2>{t("diag.explain.title")}</h2>
        {topo && topo.kind !== "server" && (
          <label className="inline">
            <span>{t("diag.header.node")}</span>
            <select aria-label={t("diag.header.node")} value={nodeName ?? ""} onChange={(e) => setNode(e.target.value)}>
              {topo.members.map((m) => <option key={m.name} value={m.name}>{m.name}</option>)}
            </select>
          </label>
        )}
        <label className="inline">
          <span>{t("diag.explain.database")}</span>
          <select aria-label={t("diag.explain.database")} value={dbName ?? ""} onChange={(e) => setDb(e.target.value)}>
            {(dbs.data ?? []).map((d) => <option key={d.name} value={d.name}>{d.name}</option>)}
          </select>
        </label>
      </div>
      <SubTabs<Source> label={t("diag.explain.source")} tabs={[{ id: "sql", label: t("diag.explain.sourceSql") }, { id: "paste", label: t("diag.explain.sourcePaste") }]} value={source} onChange={setSource}>
        {(s) =>
          s === "sql" ? (
            <>
              <textarea className="mono explain-sql" aria-label="SQL" rows={8} value={sql} onChange={(e) => setSql(e.target.value)} placeholder={t("diag.explain.placeholder")} />
              {nodeName && dbName && (
                <>
                  <PlanRunner key={`${nodeName}:${dbName}`} connectionId={connectionId} member={nodeName} database={dbName} sql={sql} serverVersionNum={serverVersionNum} inRecovery={inRecovery} />
                  {sql.trim() && <CopyLlmButton connectionId={connectionId} member={nodeName} database={dbName} source={{ kind: "sql", sql }} plan={null} />}
                </>
              )}
            </>
          ) : (
            <>
              <textarea className="mono explain-sql" aria-label={t("diag.explain.pasteLabel")} rows={12} value={pasted} onChange={(e) => setPasted(e.target.value)} placeholder={t("diag.explain.pastePlaceholder")} />
              <div className="toolbar">
                <button type="button" className="primary" disabled={!pasted.trim()} onClick={() => void parse()}>{t("diag.explain.parse")}</button>
              </div>
              {parseError && <ErrorBanner error={parseError} />}
              {parsed && (
                <>
                  <PlanSummary tree={parsed} />
                  <PlanTreeView tree={parsed} selectedId={selected} onSelect={setSelected} />
                  {findNode(parsed.root, selected) && <PlanNodeDetails node={findNode(parsed.root, selected)!} />}
                </>
              )}
            </>
          )
        }
      </SubTabs>
      {!active && null}
    </div>
  );
}
```

(`Topology.serverVersionNum` — если поля в типе топологии нет, версия берётся из `memberInfo?.serverVersionNum ?? 0`; проверить `Member`/`Topology` в `types.ts` и взять имеющееся поле версии; без версии `canGeneric` в `PlanRunner` будет ложным — GENERIC_PLAN недоступен до первого ответа, что допустимо.)

`src/components/TabContent.tsx`: `case "explain": return <ExplainTab connectionId={tab.connectionId} member={tab.member} database={tab.database} sql={tab.sql} active={active} />;`.

`src/components/connections/tree/DiagnosticsNodes.tsx` — перед пунктом журнала:

```tsx
        <li key="explain">
          <button type="button" className="link" onClick={() => open({ kind: "explain", connectionId })}>
            {t("diag.explain.title")}
          </button>
        </li>,
```

`src/components/actions/ActionsJournal.tsx`: `ACTION_KEYS` — `["explainAnalyze", "actions.kind.explainAnalyze"]`; `objectText`: первой строкой `if (r.explain) return r.explain.tables.join(", ") || r.explain.database;`.

CSS: `.explain-sql { width: 100%; min-height: 120px; resize: vertical; }`.

Словари (en / ru): `diag.explain`: `title` "EXPLAIN console"/"Консоль EXPLAIN", `database` "Database"/"База", `source` "Source"/"Источник", `sourceSql` "Query"/"Запрос", `sourcePaste` "Paste a plan"/"Вставить план", `placeholder` "SELECT … — one statement; $1, $2 become parameter fields"/"SELECT … — одна инструкция; $1, $2 станут полями параметров", `pasteLabel` "Plan (JSON)"/"План (JSON)", `pastePlaceholder` "Paste EXPLAIN (FORMAT JSON) output, psql framing is fine"/"Вставьте вывод EXPLAIN (FORMAT JSON), обрамление psql не мешает", `parse` "Parse"/"Разобрать".

- [ ] **Step 4: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): консоль EXPLAIN — запрос или вставленный JSON-план, пункт в дереве, EXPLAIN ANALYZE в журнале действий

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 16: Документация — глава «Запросы», F1, admin.md, смоук, правки спеки

**Files:**
- Create: `docs/guide/ru/queries.md`, `docs/guide/en/queries.md`, `docs/smoke/p3a.md`
- Modify: `src/guide/chapters.ts` (`GUIDE_CHAPTERS` + `"queries"`), `src/components/help/helpChapterFor.ts` (`query`/`explain` → `queries`, `diag` с `tool: "queries"` → `queries`), `docs/guide/{ru,en}/diagnostics.md` (ссылка на главу), `docs/admin.md`, `README.md` (строка стенда уже есть — проверить), `docs/superpowers/specs/2026-10-29-glafira-postgres-p3-design.md` (уточнения плана), `src/guide/parse.test.ts` или тест списка глав (если перечисляет главы)
- Test: `pnpm test -- guide help`, `pnpm i18n:check`, `pnpm build`

- [ ] **Step 1: Глава руководства (ru)**

`docs/guide/ru/queries.md`:

```markdown
# Запросы

Инструмент [[ui:diag.tools.queries]] стоит в группе [[ui:sidebar.diagnostics]] после слотов репликации. Он показывает, какие запросы грузят узел, по расширению `pg_stat_statements`, открывает карточку любого запроса с историей и планом и даёт консоль для EXPLAIN произвольного SQL. Приложение читает статистику и строит планы; данные меняет только [план с выполнением](queries.md#analyze) для запросов, которые пишут, — и то в транзакции, которая всегда откатывается.

Что нужно на сервере: расширение `pg_stat_statements`, созданное в базе подключения (пункт чек-листа «Расширение pg_stat_statements»); роль с `pg_monitor` (иначе видны только собственные запросы роли); для планов — право читать таблицы приложения (`pg_read_all_data` на PostgreSQL 14 и новее, пункт чек-листа «Чтение таблиц приложения»). Колонки планирования заполняются только при `pg_stat_statements.track_planning = on`.

## Топ запросов {#top}

Таблица — строка на запрос (одинаковые запросы с разными литералами уже объединены самим расширением: в тексте вместо значений стоят `$1`, `$2`). Клик по заголовку колонки сортирует: общее время (по умолчанию), среднее, вызовы, строк на вызов, hit %, чтения, temp, WAL, I/O, планирование, JIT, stddev и max. Недоступная сортировка серая: max и stddev есть только в режиме «с момента сброса», JIT — с pg_stat_statements 1.10 (PostgreSQL 15). Фильтры: база, [[ui:diag.queries.toplevelOnly]] (без вложенных инструкций из функций), число строк.

Режим ([[ui:diag.queries.mode]]):

- [[ui:diag.queries.modes.sinceReset]] — накопленная статистика с последнего `pg_stat_statements_reset()`. Доступен сразу, обновляется по интервалу шапки.
- За 5, 15 или 60 минут — что происходило недавно. Пока вкладка открыта, приложение раз в минуту читает статистику (без текстов запросов) и запоминает прирост; окно считается из этих приростов и не ходит на сервер. Значок рядом с режимом показывает, сколько минут уже накоплено: сразу после открытия окно пустое. История хранится час и живёт, пока открыто подключение; закрытие вкладки её останавливает, повторное открытие продолжает с промежутком.

Внизу — версия расширения и время последнего сброса статистики. Если расширение вытесняло редкие запросы (`dealloc`), находка «Топ неполный» советует поднять `pg_stat_statements.max`.

Находки топа (блок над таблицей): N+1 — крошечный запрос с огромным числом вызовов; нестабильное время (stddev много больше среднего); низкое попадание в кэш; temp-файлы на вызов; планирование дольше выполнения. У каждой — кнопка «Открыть запрос».

## Карточка запроса {#card}

Строка таблицы открывает вкладку запроса. Запрос в карточке — один `queryid` в одной базе: строки по ролям и по признаку верхнего уровня суммируются, разбивка — отдельной таблицей. Переключатель узла в шапке позволяет сравнить тот же запрос на primary и реплике (на физических репликах идентификаторы совпадают).

Подвкладка [[ui:diag.query.overview]]: текст запроса, метрики с момента сброса и за последний час рядом, два спарклайна истории (вызовов в минуту и среднее время) по тикам сэмплера, выполняющиеся сейчас экземпляры из `pg_stat_activity` (PostgreSQL 14 и новее; кнопка открывает «Сессии»), ожидания запроса по сэмплам вкладки «Сессии» (пока она открыта, иначе подсказка), таблицы запроса — они появляются после первого плана. Кнопки: [[ui:diag.query.plan]], [[ui:diag.query.openConsole]], [[ui:diag.query.copyLlm]].

[[ui:diag.query.copyLlm]] собирает в буфер обмена markdown: версия сервера и расширения, узел и база, текст запроса, статистика, план в текстовом виде и DDL таблиц с индексами (не больше десяти таблиц и 64 КБ). Текст берётся нормализованный, без значений параметров; план строится без выполнения. Это данные для вопроса модели, промптов в тексте нет.

## План и EXPLAIN {#plan}

Подвкладка [[ui:diag.query.plan]] строит план на выбранном узле. Текст: нормализованный из `pg_stat_statements` или текст выполняющегося экземпляра с его литералами ([[ui:diag.query.source]]). У нормализованного текста параметры `$1…$N` надо задать: на PostgreSQL 16 и новее по умолчанию стоит `GENERIC_PLAN` — план без значений; иначе заполните поля (пустое поле — NULL). Срок выполнения — от 5 секунд до 10 минут, по умолчанию 30 секунд; `search_path` при необходимости задаётся в свёрнутом поле (по умолчанию серверный для роли).

[[ui:diag.plan.run.plan]] — `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS)`: запрос не выполняется. [[ui:diag.plan.run.analyze]] — `EXPLAIN (ANALYZE, BUFFERS, WAL)`, запрос выполняется по-настоящему. {#analyze}

Два яруса выполнения. Сначала приложение строит обычный план и смотрит, пишет ли запрос:

- **Чтение** (нет `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `CREATE … AS`): выполняется без подтверждения в транзакции только для чтения (`BEGIN READ ONLY`), которая откатывается. Если запрос всё же попробует писать (функция с побочным эффектом, триггер), сервер откажет, и приложение предложит выполнить его как запись.
- **Запись**: нужны включённые действия на подключении (на «только чтение» недоступно, на prod вводится имя подключения), показывается окно со списком таблиц и предупреждением, затем запрос выполняется в транзакции и откатывается. Строки изменятся и откатятся, но откат не отменяет сработавшие триггеры, сдвиг последовательностей, `NOTIFY` и внешние вызовы; на время выполнения запрос держит блокировки. Такое выполнение записывается в [журнал действий](diagnostics.md#journal) как «EXPLAIN ANALYZE (запись)». На реплике запись невозможна — выберите primary.

Срок истёк — выполнение прервано сервером, изменений нет. Две инструкции в тексте не принимаются: EXPLAIN строится ровно для одной.

Дерево плана: строки с отступом, развёртка узлов; время включённое и собственное (без детей) с полосой доли; строки «оценка → факт» — ошибка оценки в десять раз и больше подсвечена; циклы; буферы (попадания / чтения); пометки: диск, lossy, heap fetches, воркеров меньше плана, не выполнялся. Три узла с наибольшим собственным временем выделены. Клик по узлу открывает панель со всеми полями. Над деревом — сводка: планирование, выполнение, ярус, JIT, триггеры, settings; кнопки [[ui:diag.plan.copyJson]] и [[ui:diag.plan.copyText]] (текст в стиле psql). Прошлые запуски — в списке [[ui:diag.plan.history.title]].

## Консоль EXPLAIN {#console}

[[ui:diag.explain.title]] открывается из группы диагностики и из карточки. Выберите узел и базу, введите одну инструкцию SQL и стройте планы так же, как в карточке; `$1…$N` в тексте становятся полями параметров. Вторая подвкладка — [[ui:diag.explain.sourcePaste]]: вставьте вывод `EXPLAIN (FORMAT JSON)` (обрамление psql с `QUERY PLAN` и `+` не мешает) и нажмите [[ui:diag.explain.parse]] — дерево строится без обращения к серверу. Текстовый формат плана psql и auto_explain появится в следующей части.

## Права и нагрузка {#load}

Роль диагностики читает чужие запросы с `pg_read_all_stats` (входит в `pg_monitor`); без него видны только собственные, и инструмент пишет об этом. Для EXPLAIN запросов приложения роли нужны права чтения их таблиц. Сэмплер читает `pg_stat_statements` без текстов раз в минуту на узел, пока открыт инструмент или карточка; тексты запрашиваются по ключам видимых строк. EXPLAIN выполняется на отдельном соединении, которое закрывается после запроса. Подробности — руководство администратора, раздел «Диагностика».
```


- [ ] **Step 2: Глава руководства (en)**

`docs/guide/en/queries.md` — перевод главы по абзацам с теми же якорями `{#top}`, `{#card}`, `{#plan}`, `{#analyze}`, `{#console}`, `{#load}` и теми же `[[ui:…]]`-ссылками; заголовок `# Queries`. Текст:

```markdown
# Queries

The [[ui:diag.tools.queries]] tool sits in the [[ui:sidebar.diagnostics]] group after replication slots. It shows which queries load the node according to the `pg_stat_statements` extension, opens a card for any query with its history and plan, and gives a console for EXPLAIN of arbitrary SQL. The app reads statistics and builds plans; the only thing that changes data is the [plan with execution](queries.md#analyze) of a writing query, and even that runs in a transaction that is always rolled back.

What the server needs: the `pg_stat_statements` extension created in the connection's database (the checklist item “pg_stat_statements extension”); a role with `pg_monitor` (otherwise only the role's own queries are visible); for plans, the right to read the application's tables (`pg_read_all_data` on PostgreSQL 14 and newer, the checklist item “Reading application tables”). Planning columns are filled only with `pg_stat_statements.track_planning = on`.

## Top queries {#top}

The table has a row per query (identical queries with different literals are already merged by the extension: the text has `$1`, `$2` instead of values). Click a column header to sort: total time (default), mean, calls, rows per call, hit %, reads, temp, WAL, I/O, planning, JIT, stddev and max. A grayed-out sort is unavailable: max and stddev exist only in “since reset” mode, JIT needs pg_stat_statements 1.10 (PostgreSQL 15). Filters: database, [[ui:diag.queries.toplevelOnly]] (without statements nested in functions), the number of rows.

Mode ([[ui:diag.queries.mode]]):

- [[ui:diag.queries.modes.sinceReset]] — statistics accumulated since the last `pg_stat_statements_reset()`. Available at once, refreshed on the header interval.
- Last 5, 15 or 60 minutes — what happened recently. While the tab is open the app reads the statistics once a minute (without query texts) and remembers the growth; the window is computed from that growth without touching the server. The badge next to the mode shows how many minutes have been collected: right after opening, the window is empty. The history is kept for an hour while the connection is open; closing the tab stops it, reopening continues with a gap.

At the bottom are the extension version and the last statistics reset. If the extension evicted rare queries (`dealloc`), the “The top is incomplete” finding suggests raising `pg_stat_statements.max`.

Findings of the top (the block above the table): N+1, a tiny query with a huge number of calls; unstable time (stddev much larger than the mean); low cache hit; temp files per call; planning longer than execution. Each has an “Open query” button.

## Query card {#card}

A table row opens the query tab. A query in the card is one `queryid` in one database: rows per role and per top-level flag are summed, the breakdown is a separate table. The node selector in the header compares the same query on the primary and a replica (identifiers match on physical replicas).

The [[ui:diag.query.overview]] sub-tab: the query text, metrics since reset and for the last hour side by side, two history sparklines (calls per minute and mean time) by sampler ticks, instances running right now from `pg_stat_activity` (PostgreSQL 14 and newer; a button opens Sessions), the query's waits from the samples of the Sessions tab (while it is open, otherwise a hint), the query's tables, which appear after the first plan. Buttons: [[ui:diag.query.plan]], [[ui:diag.query.openConsole]], [[ui:diag.query.copyLlm]].

[[ui:diag.query.copyLlm]] puts markdown on the clipboard: server and extension versions, node and database, the query text, statistics, the plan as text and the DDL of the tables with indexes (at most ten tables and 64 KB). The text is the normalized one, without parameter values; the plan is built without execution. This is data for a question to a model; there are no prompts in it.

## Plan and EXPLAIN {#plan}

The [[ui:diag.query.plan]] sub-tab builds the plan on the selected node. The text: normalized from `pg_stat_statements` or the text of a running instance with its literals ([[ui:diag.query.source]]). For the normalized text the parameters `$1…$N` must be given: on PostgreSQL 16 and newer `GENERIC_PLAN` is the default, a plan without values; otherwise fill in the fields (an empty field is NULL). The execution timeout is 5 seconds to 10 minutes, 30 seconds by default; `search_path` can be set in the collapsed field if needed (the server default of the role otherwise).

[[ui:diag.plan.run.plan]] is `EXPLAIN (FORMAT JSON, VERBOSE, SETTINGS)`: the query is not executed. [[ui:diag.plan.run.analyze]] is `EXPLAIN (ANALYZE, BUFFERS, WAL)`: the query really runs. {#analyze}

Two execution tiers. The app first builds a plain plan and checks whether the query writes:

- **Read** (no `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `CREATE … AS`): runs without confirmation in a read-only transaction (`BEGIN READ ONLY`) that is rolled back. If the query tries to write anyway (a function with a side effect, a trigger), the server refuses and the app offers to run it as a write.
- **Write**: needs actions enabled on the connection (unavailable on read-only, the connection name is typed on prod), shows a dialog with the list of tables and a warning, then runs the query in a transaction and rolls it back. Rows change and are rolled back, but rollback does not undo fired triggers, sequence advances, `NOTIFY` and external calls; the query holds locks while it runs. Such an execution is recorded in the [actions journal](diagnostics.md#journal) as “EXPLAIN ANALYZE (write)”. A write cannot run on a replica: choose the primary.

When the timeout expires the server cancels the execution and nothing changes. Two statements in the text are not accepted: EXPLAIN is built for exactly one.

The plan tree: indented rows with expandable nodes; inclusive and own time (without children) with a share bar; rows “estimate → actual” with a tenfold estimate error highlighted; loops; buffers (hits / reads); marks: disk, lossy, heap fetches, fewer workers than planned, never executed. The three nodes with the largest own time are highlighted. Clicking a node opens a panel with all its fields. Above the tree is the summary: planning, execution, tier, JIT, triggers, settings; the [[ui:diag.plan.copyJson]] and [[ui:diag.plan.copyText]] buttons (psql-style text). Past runs are in the [[ui:diag.plan.history.title]] list.

## EXPLAIN console {#console}

[[ui:diag.explain.title]] opens from the diagnostics group and from the card. Pick a node and a database, enter one SQL statement and build plans as in the card; `$1…$N` in the text become parameter fields. The second sub-tab is [[ui:diag.explain.sourcePaste]]: paste the output of `EXPLAIN (FORMAT JSON)` (psql framing with `QUERY PLAN` and `+` is fine) and press [[ui:diag.explain.parse]]: the tree is built without the server. The text format of psql and auto_explain plans comes in the next part.

## Rights and load {#load}

The diagnostics role reads other roles' queries with `pg_read_all_stats` (part of `pg_monitor`); without it only its own are visible, and the tool says so. EXPLAIN of application queries needs the right to read their tables. The sampler reads `pg_stat_statements` without texts once a minute per node while the tool or a card is open; texts are requested by the keys of the visible rows. EXPLAIN runs on a separate connection that is closed after the query. Details: the administrator guide, section “Diagnostics”.
```

- [ ] **Step 3: Главы в коде, F1, ссылки**

`src/guide/chapters.ts`: `GUIDE_CHAPTERS = ["getting-started", "connections", "objects", "diagnostics", "queries", "updates"]`. `src/components/help/helpChapterFor.ts`: `case "query": case "explain": return "queries";` и в `case "diag"` — `return tab.tool === "queries" ? "queries" : "diagnostics";` (переписать `switch` так, чтобы `diag` обрабатывался отдельной ветвью с проверкой инструмента); `helpAnchorFor`: `query` → `"card"`, `explain` → `"console"`, `diag` с `queries` → `"top"`. Тесты `helpChapterFor.test.ts` (если есть) и тест списка глав — дополнить. В `docs/guide/{ru,en}/diagnostics.md` первый абзац: после перечисления инструментов — «Топ запросов и планы описаны в главе [Запросы](queries.md)» / “Top queries and plans are described in the [Queries](queries.md) chapter”. В словарях `help` — заголовок главы, если главы перечислены ключами (`help.chapters.queries`: "Queries"/"Запросы"; проверить `grep -n "diagnostics" src/locales/en.json` в разделе `help`).

- [ ] **Step 4: `docs/admin.md`**

В разделе «Диагностика» первый абзац дополнить: «Инструмент «Запросы» читает `pg_stat_statements`; планы (EXPLAIN) строятся на отдельном соединении; «план с выполнением» выполняет запрос в транзакции с откатом — см. ниже.»

В «Права» добавить абзац:

```markdown
Инструмент «Запросы» и карточка запроса читают `pg_stat_statements`: строки других ролей видны только с `pg_read_all_stats` (входит в `pg_monitor`), без него сервер скрывает их `queryid` и текст, и инструмент пишет «Запросы других ролей: нужна роль pg_read_all_stats». Расширение должно быть создано в базе подключения (`CREATE EXTENSION pg_stat_statements`; пункт чек-листа «Расширение pg_stat_statements»); колонки планирования заполняются при `pg_stat_statements.track_planning = on` (пункт «pg_stat_statements.track_planning»). EXPLAIN запроса приложения выполняется под ролью подключения и требует права читать таблицы запроса: на PostgreSQL 14 и новее — `GRANT pg_read_all_data TO glafira_diag;` (пункт чек-листа «Чтение таблиц приложения»), на 13 — `SELECT` на нужные таблицы. Ярус записи EXPLAIN ANALYZE (запросы с `INSERT`/`UPDATE`/`DELETE`/`MERGE`) требует ещё и права на запись — он предназначен для отдельной роли действий (см. «Отдельная роль для действий»).
```

В «Нагрузка» добавить пункты:

```markdown
- Сэмплер запросов — одно чтение `pg_stat_statements(showtext := false)` раз в минуту на узел, пока открыты инструмент «Запросы» или карточка запроса (в том числе скрытые); тексты запросов читаются отдельно по ключам видимых строк. Чтение идёт на соединении сэмплера (общее с сэмплером профиля ожиданий). История дельт хранится в памяти (60 минут, не больше 200 000 записей на узел, порядка 50 МБ) и пропадает при закрытии подключения.
- EXPLAIN и EXPLAIN ANALYZE выполняются на отдельном соединении (как действие), которое открывается на один запрос и закрывается после него; параметры сессии — `statement_timeout = 5s` (внутри транзакции заменяется сроком запроса: 15 с у пробного плана, 5–600 с у выполнения), `lock_timeout = 1s`, без `search_path = pg_catalog` (запрос должен видеть схемы приложения).
```

В «Действия» после списка «Что можно сделать» добавить пункт «- EXPLAIN ANALYZE запроса, который пишет данные: выполнение в транзакции с откатом (ярус записи, см. ниже).» и абзац:

```markdown
**EXPLAIN ANALYZE в два яруса.** «План с выполнением» сначала строит обычный план и по нему определяет ярус. Чтение (нет `ModifyTable` в плане и инструкция не `CREATE … AS`) выполняется в транзакции `BEGIN READ ONLY … ROLLBACK` без подтверждения и в журнал не пишется: сервер сам отвергнет попытку записи (SQLSTATE 25006), и приложение предложит выполнить запрос как запись. Запись требует включённых действий (на подключении «только чтение» недоступна, на prod вводится имя подключения), подтверждения со списком таблиц и выполняется в `BEGIN … ROLLBACK`; на реплике недоступна. Откат не отменяет сработавшие триггеры, сдвиг последовательностей, `NOTIFY` и внешние вызовы (`dblink`, `http`), а на время выполнения запрос держит блокировки — предупреждение об этом стоит в окне подтверждения. Каждое выполнение яруса записи записывается в `actions.jsonl` действием `explainAnalyze` с базой, списком таблиц, сроком и первыми 200 символами текста (литералы остаются).
```

В «Данные и логи»: после описания `actions.jsonl` — «в записях `explainAnalyze` — начало текста запроса с литералами».

- [ ] **Step 5: Смоук и спека**

`docs/smoke/p3a.md`:

```markdown
# Смоук P3a «Видеть» — топ запросов, карточка, EXPLAIN, консоль

Сборка: `pnpm tauri dev`. Стенд: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf` и нагрузка
`docker exec -d gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 600` (dev/perf/README.md).
Подключение: сервер `localhost:5471`, база `perf`, роль `glafira_diag` / `glafira-secret`, окружение `dev`.

1. **Готовность.** Форма подключения → «Проверить»: пункты «pg_stat_statements.track_planning» — Включён, «Чтение таблиц приложения» — Разрешено, расширения hypopg, pg_stat_kcache, pg_wait_sampling — установлены.
2. **Топ «с момента сброса».** Дерево → Диагностика → Запросы: таблица с строками pgbench, тексты появляются через секунду, сортировка по «Вызовов» ставит точечный выбор покупателя наверх; находки «N+1», «Temp-файлы» и «Попадание в кэш» есть; подвал — `pg_stat_statements 1.11`.
3. **Окно.** Режим «За 5 мин»: значок «Идёт первый тик…», через минуту — «Накоплено 1 из 5 мин» и строки с дельтами; заголовки «Stddev» и «Max» неактивны. Закрыть вкладку, подождать минуту, открыть снова — таблица продолжает с прежними данными (промежуток в истории карточки).
4. **Карточка.** Клик по строке «SELECT count(*) FROM shop.orders WHERE status = $1»: текст, метрики в две колонки, спарклайны растут по минутам, экземпляры (при работающем pgbench) и ожидания после открытия «Сессий»; «Таблицы появятся после первого плана».
5. **План.** Подвкладка «План»: GENERIC_PLAN включён; «План» → дерево с Seq Scan on shop.orders и горячим узлом; в «Обзоре» появилась таблица shop.orders со ссылкой на вкладку таблицы. Снять GENERIC_PLAN, ввести `disputed`, «План с выполнением» — факт, буферы, ярус «чтение», без подтверждения; в журнале действий записи нет.
6. **Запись.** Консоль EXPLAIN (из дерева): `UPDATE shop.balances SET balance = balance + 1 WHERE customer_id = 1`, «План с выполнением» → окно со списком `shop.balances` и предупреждением → «Выполнить и откатить» → дерево с ModifyTable, ярус «запись»; журнал действий — строка «EXPLAIN ANALYZE (запись)» с `shop.balances`; `SELECT balance FROM shop.balances WHERE customer_id = 1` в psql — значение не изменилось.
7. **Защита.** Перевести подключение в «только чтение»: кнопка «План с выполнением» для UPDATE даёт подсказку «Запись недоступна…», SELECT выполняется. Окружение `prod`: окно подтверждения просит имя подключения.
8. **Побочный эффект.** Консоль: `SELECT nextval('shop.customers_id_seq')` → «План с выполнением» → сообщение про запись и кнопка «Выполнить как запись» → подтверждение → ярус «запись».
9. **Ошибки.** `SELECT 1; SELECT 2` → «Введите ровно одну инструкцию»; `SELECT pg_sleep(10)` со сроком 5 с → «План не получен…»; `SELEC 1` → ошибка сервера с позицией.
10. **Вставка плана.** «Вставить план» ← вывод `docker exec gp-pg-perf psql -U postgres -d perf -c "EXPLAIN (FORMAT JSON) SELECT 1"` целиком (с `QUERY PLAN` и `+`) → «Разобрать» → дерево «Result».
11. **Для LLM.** В карточке «Скопировать для LLM» → в буфере markdown с разделами «Среда», «Запрос», «Статистика», «План», «Таблицы» и DDL `shop.orders`.
12. **pg-single.** Подключение к `localhost:5460`: инструмент «Запросы» показывает «Статистика запросов (pg_stat_statements): pg_stat_statements не установлен в этой базе», таблицы нет.
13. **Справка.** F1 на вкладке «Запросы» открывает главу «Запросы» (раздел «Топ запросов»), на карточке — раздел «Карточка запроса», в консоли — «Консоль EXPLAIN».
```

Спека `docs/superpowers/specs/2026-10-29-glafira-postgres-p3-design.md` — правки по уточнениям плана:
- §7.1: «вкладка сохраняется в workspace и восстанавливается при запуске» → «живёт, как остальные вкладки, до закрытия приложения (workspace между запусками не сохраняется)»;
- §7.2: `windows: Record<…>` → `windows: { windowS: 300 | 900 | 3600; stat: QueryStat | null }[]`;
- §7.4 и §10: `query_llm_markdown(…)` → `query_llm_bundle(id, member, database, source, plan) -> LlmBundle` (данные), markdown собирает фронтенд (`src/lib/llmMarkdown.ts`);
- §8.1: в `ExplainRequest` добавить `forceWrite: boolean` с пояснением («Выполнить как запись» после `EXPLAIN_WRITES`); §11 строка `EXPLAIN_WRITES` — «кнопка повторяет запрос с `forceWrite`»;
- §4.1: убрать `[+ s.jit_deform_time]` (входит в `jit_generation_time`);
- §3.1 / §12: `explain/llm.rs` остаётся; добавить `diag/query_detail.rs` уже есть; `commands/queries.rs` — команды `query_texts`, `start_statements_sampler`, `query_detail`, `query_llm_bundle`.

- [ ] **Step 6: Проверка**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS (главы грузятся, `GUIDE_CHAPTERS` содержит `queries`, F1 открывает главу). Пройти смоук `docs/smoke/p3a.md` на стенде; расхождения — исправить в соответствующих задачах.

- [ ] **Step 7: Commit**

```bash
git add docs src/guide src/components/help src/locales README.md
git commit -m "docs(queries): глава «Запросы» руководства (ru/en), F1, admin.md — права, нагрузка и ярусы EXPLAIN ANALYZE, смоук P3a, правки спеки по решениям плана

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Самопроверка плана

- **Покрытие части P3a спецификации:**
  - §1 п. 1 (стенд `pg-perf`) — задача 1; п. 2 (сэмплер pgss) — 5, 6; п. 3 (инструмент «Запросы») — 2, 4, 6, 12; п. 4 (находки без плана, `Query`/`OpenQuery`) — 2, 3, 12, 14; п. 5 (карточка, «для LLM») — 7, 10, 14; п. 6 (EXPLAIN в два яруса, парсер JSON, дерево, консоль с JSON) — 8, 9, 13, 14, 15; п. 10 (готовность: `track_planning`, права чтения) — 11; п. 11 (руководство, admin.md) — 16.
  - §2 (что берём): `Tool`/`DiagRequest`/`DiagData` — 4; `Caps` — 7; реестр и буфер сэмплера — 5, 6; `ActionClient`/`for_explain` — 9; `guard` — 9; журнал — 9, 15; `ConfirmDialog` — 14; `FindingTarget::Query` — 2; `table::get` — 10; готовность — 11; `write_private` — P3b; `TabSpec` — 12; `DdlView`/`CopyButton` — 13, 14.
  - §3.1 модули — карта файлов; §3.2 поток данных — 6, 12, 14; §3.3 версии pgss — 2, 4 (тест матрицы); §3.4 соединения — 6 (пул `Sampler`), 9 (`for_explain`).
  - §4.1–4.7 — 4, 6, 12; §5 — 5, 6; §6 — 3 (пороги в 2); §7.1–7.4 — 7, 10, 12, 14; §8.1–8.7 — 8, 9, 13, 14, 15; §9 — план P3b; §10 (IPC P3a) — 4, 6, 7, 9, 10; §11 (ошибки P3a: `EXPLAIN_WRITES`, `FEATURE_UNAVAILABLE`, `NOT_AUTHORIZED`, `TIMEOUT`, `INVALID_INPUT`, `READ_ONLY`, `CONFIRMATION_REQUIRED`) — 4, 6, 9; §12 структура — карта файлов; §13.1 стенд — 1; §13.2 автотесты — в каждой задаче, интеграционные — 1, 4, 6, 7, 9, 10; §13.3 CI — 1 (`GPG_IT_PERF`), без новых нативных зависимостей — Global Constraints; §14 документация — 16; §15 отступления — учтены (без `pg_query`, SELECT без подтверждения, текст экземпляра целиком, подсветка без переформатирования, сэмплер пока открыт потребитель, общее соединение `Sampler`); §16 критерии P3a: 1 — 4, 6, 12 и смоук; 2 — 4 (`it_queries`), 12; 3 — 7, 14; 4 — 9 (`it_explain`), 14; 5 — 9, 14, 15; 6 — 15, 10, 14; 7 — 6; 8 — 12–16 и CI.
  - Не входит (P3b): рекомендации, hypopg, снимки и «между снимками» (`validate` отвергает), текстовый парсер, `pg_stat_kcache`, `pg_wait_sampling`, подсказки к расширениям.
- **Типы между задачами:** `PgssCaps` (2) → SQL (4) и источник (6); `QueryKey`/`Counters`/`Cumulative`/`QueryStat`/`Totals`/`QuerySort`/`stat`/`top` (2) → 4, 6, 7, TS-типы (12); `rules::{evaluate, Mode, PgssState}` (3) → 4, 6, 7; `Snapshot`/`PgssInfo`/`PgssInstalled`/`detect`/`read_cumulative`/`statements_sql`/`row_from`/`info_sql` (4) → 6, 7; `Sampling`/`SamplerRegistry<S>`/`SampleSource<T>`/`SamplerSink<E>`/`start_with` (5) → 6; `StatementsBuffer::{window, names, history, gaps, text, remember_texts, last_head, info, pgss, hidden}`, `WindowSum`, `HistoryPoint`, `StatementsEvent` (6) → 4 (`report_window`), 7, TS (12); `QueryDetail` (7) → 10 (`bundle`), TS (12), 14; `PlanTree`/`PlanNode`/`RelationRef`/`from_json`/`from_psql_text`/`relations`/`has_modify_table`/`to_text` (8) → 9, 10, TS (12), 13; `ExplainRequest { …, force_write }`/`ExplainResult`/`Tier`/`run` (9) → 10, TS (12), 14; `LlmBundle`/`LlmSource` (10) → TS (12), `llmMarkdown` (14); `ExplainRecord` (9) → TS (12), журнал (15); `ReadinessFacts` новые поля (11) → `collect_facts` (11); `TabSpec` `query`/`explain` (12) → 12 (`openQuery` строки), 14 (`TabContent`, `FindingsPanel`), 15; `PlanTreeView`/`PlanNodeDetails`/`PlanSummary`/`renderText` (13) → 14, 15; `PlanRunner`/`CopyLlmButton` (14) → 15; `formatMs` (12) → 13, 14; `queryTitle`/`sortAvailable` (12) → 12, 14.
- **Плейсхолдеры:** `report_window` в задаче 4 — настоящее поведение без буфера (пустой отчёт с пометкой), заменяется в 6; `cached_text` в 4 возвращает `None` до появления кеша в 6; `openQuery` в `FindingsPanel` фильтруется в 12 до появления вкладки в 14; `helpChapterFor` для `query`/`explain` ведёт в «Диагностику» до появления главы в 16 — всё это рабочие промежуточные состояния, не заглушки.
- **Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12 → 13 → 14 → 15 → 16. Задача 6 зависит от 4 и 5; 7 — от 6; 9 — от 8; 10 — от 7, 8, 9; 12 — от 4, 6, 7, 9, 10 (типы); 13 — от 12; 14 — от 13; 15 — от 14; 16 — последняя.
- **Риски, названные заранее:** сборка образа `pg-perf` тянет пакеты PGDG (сеть на CI); init `perf` — 1–2 минуты в `wait-stand`; тест второго тика сэмплера ждёт до 75 с; `tokio-postgres` с `with-serde_json-1` — новая feature, не крейт; `GENERIC_PLAN` в тесте матрицы — только узел `:5515`; `pg_read_all_data` на стенде выдан только в `pg-perf` (на остальных узлах EXPLAIN запросов `shop` под `glafira_diag` даст `NOT_AUTHORIZED` — это ожидаемое поведение, проверяется подсказкой); обобщение реестра сэмплеров меняет сигнатуры тестов P2 без изменения логики.
