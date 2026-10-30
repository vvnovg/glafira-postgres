# P3b «Советовать и сравнивать» — рекомендации по плану, hypopg, снимки, текстовые планы, расширения: план реализации

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Реализовать часть P3b спецификации `docs/superpowers/specs/2026-10-29-glafira-postgres-p3-design.md` (§1, пункты 7–10 и остаток пункта 11; §9 целиком): 11 правил рекомендаций по плану с доказательством, командой, эффектом и рисками и проверкой гипотезы через hypopg; снимки `pg_stat_statements` на диске, сравнение и режим топа «между снимками»; парсер текстовых планов psql / auto_explain; `pg_stat_kcache` (CPU и диск) и `pg_wait_sampling` (точный профиль ожиданий запроса); подсказки готовности к расширениям; разделы руководства и admin.md.

**Architecture:** Правила — чистые функции `pg_advisor::plan_rules` над `PlanTree` P3a; `explain::run` добавляет их в `ExplainResult.recommendations`. Проверка гипотезы — `explain/hypopg.rs` на соединении `for_explain()` в одной транзакции `READ ONLY` (виртуальный индекс → EXPLAIN → `hypopg_reset()`). Снимки — файлы `data_dir/snapshots/<подключение>/<узел>/<время>.json` (0600) через `storage::fs::write_private`; сравнение — `pg_advisor::queries::compare`; режим «между снимками» — дельты двух файлов в том же `DiagData::Queries`. Текстовый парсер — `plan::from_text` в тот же `PlanTree`, `parse_plan` принимает и текст. Расширения — join к pgss (`pg_stat_kcache`) и запрос профиля (`pg_wait_sampling`) только при установленном расширении. Интерфейс — блок `Recommendations` под сводкой плана, `HypoCheck`, `SnapshotsPanel`/`SnapshotCompare` в инструменте «Запросы», колонки CPU/диск, источник ожиданий в карточке.

**Tech Stack:** как у P3a (Tauri 2, Rust 2024, tokio-postgres с `with-serde_json-1`, serde; React 19 + TS + TanStack Query + Zustand + i18next; Vitest). Новых крейтов нет (`time` для имён файлов уже в зависимостях).

**Образец кода:** корень репозитория — `$P` (`/Users/vvnovg/postgres-patroni-diag`). Опора — код P3a: `crates/pg-advisor/src/{plan,queries}`, `src-tauri/src/{explain,diag/queries.rs,diag/query_detail.rs,sampler/statements.rs}`, `src/components/queries/**`. Имена и сигнатуры P3a — по плану `docs/superpowers/plans/2026-10-29-glafira-postgres-p3a.md` (блоки Interfaces).

## Поправки по итогам P3a

Код P3a ушёл от того, что предполагают задачи ниже; задачи не переписаны, исполнитель применяет эти поправки по месту. Номера строк — по тексту плана до вставки этого раздела.

1. **Тип `Actual` (`crates/pg-advisor/src/plan/mod.rs`):** `startup_ms: Option<f64>`, `total_ms: Option<f64>` (`None` при `TIMING OFF`), `rows: f64`, **`loops: f64`** (было `loops: u64`, времена `f64`). Последствия для кода задач:
   - Задача 1, `scan.rs` (стр. 554–563, 599–608): `(returned + removed) * a.loops as f64` и `returned * a.loops as f64` — приведение к `f64` теперь лишнее, `-D warnings` отвергнет его (`clippy::unnecessary_cast`); `a.loops < th::PLAN_NESTED_LOOP_LOOPS` — типы разные, константа должна быть `f64` или сравнение идёт с приведением константы; `.param("loops", a.loops)` — приводить к целому (`a.loops as u64`): `f64` ляжет в JSON как `2000.0`, и `assert_eq!(rec.params["loops"], 2000)` (стр. 204) не сойдётся — `Value` из `f64` не равен целому литералу.
   - Задача 2, `exec.rs` (стр. 847, 878, 901, 933–938) и тест (стр. 789): `a.rows * a.loops as f64` — без приведения; `a.loops == 0` → `a.loops == 0.0`; `a.loops < th::PLAN_SUBPLAN_LOOPS` и `.param("loops", a.loops)` — как выше; `assert_eq!(r.params["loops"], 3000)` — после приведения параметра к целому.
   - Задача 5, `text_in.rs` (стр. 1731–1747) и тест (стр. 1578): литералы `Actual { startup_ms: …, total_ms: …, rows, loops }` — времена в `Some(..)` (`None`, когда в тексте нет `time=…`, то есть `TIMING OFF`), `loops: f64` (переменная разбора `loops=` — `f64`, без `as u64`); у `never executed` — `loops: 0.0`; проверка `root.actual.map(|a| (a.total_ms, a.rows, a.loops))` ждёт `Some((Some(50.0), 10.0, 1.0))`.
2. **Сигнатуры `explain/run.rs` для `hypopg.rs` (задача 3, стр. 986, 1084, 1100, 1157–1159, 3402):** `explain_in_tx(c, ctx: Context, timeout_s, search_path, options, sql, params)` принимает `Context { origin: Option<Origin>, write: bool, read_only: bool }` вместо `read_only: bool`, а `classify(&e, ctx)` — второй аргумент вместо `classify(&e)`. `Context`, `Origin` (`Origin::new(options, original_sql)`), `explain_prefix`/`PROBE_OPTIONS` и `Step` сейчас приватны: сделать `pub(crate)` вместе с `explain_in_tx` и `classify`. Проба гипотезы: `Context { origin: Some(Origin::new(options, sql)), write: false, read_only: true }`, где `options` — та же строка опций, что уходит в `explain_in_tx`: пробный план P3a идёт с `FORMAT JSON, VERBOSE, SETTINGS` (`PROBE_OPTIONS`), а `plan_cost` ниже передаёт `FORMAT JSON, VERBOSE` — взять общую константу или передать в `Origin::new` ту же строку, иначе позиция ошибки в тексте пользователя сместится.
3. **Фикстуры P3a для проверки правил (стр. 965):** `index_only_heap_fetches_analyze` на деле — Aggregate над Bitmap Heap Scan с Bitmap Index Scan и без `Heap Fetches` (правило `plan_heap_fetches` не сработает), а `parallel_jit_analyze` — Aggregate над Seq Scan без Gather и воркеров (`plan_workers_short` не сработает, `plan_jit_heavy` зависит от доли JIT). Пересобрать их другими запросами на `pg-perf` (Index Only Scan по `balances` и параллельная агрегация при `max_parallel_workers = 2`) или поправить ожидаемый набор правил в проверке и записать причину в комментарий теста.
4. **`estimate_ratio` (`PlanNode`):** `None` у невыполнявшегося узла и когда и оценка, и факт нулевые; `Some(f64::INFINITY)` при оценке 0 и факте > 0 (в JSON такое значение сериализуется как `null`). Правило `misestimate` (стр. 617) опирается на флаг `n.misestimated`, а не на `?` по `estimate_ratio`; параметр `ratio` берёт конечным (например, `a.rows.max(1.0) / n.plan_rows.max(1.0)`), иначе `.param("ratio", f64::INFINITY)` станет `null` и текст доказательства потеряет число.
5. **JIT `Generation` на PG 17+** — объект `{"Deform": …, "Total": …}`, разбор P3a берёт `Total` (спека §8.5); тест `jit_heavy` (стр. 790) строит `"Generation": 20.0` числом и PG 17+ не покрывает — добавить вариант с объектом. В текстовом парсере (задача 5, стр. 1994) сверить формат строки `Timing:` на текстовой фикстуре PG 17+.
6. **`coverageS`:** в P3a покрытие окна — от первого тика до последнего плюс минута, оно охватывает и промежутки сбора (спека §4.2, открытый вопрос §15 п. 12). Режим «между снимками» (задача 6, стр. 2709) задаёт `coverage_s` разницей времён снятия; решение по п. 12 принять до реализации, чтобы значок покрытия значил одно и то же в обоих режимах.
7. **`StatementsBuffer::window_for(keys, window_s)`** (P3a, после RT4) — один проход по тикам окна с суммами только по `keys` и итогом по всем дельтам; если сравнению снимков или карточке понадобится окно по нескольким ключам, брать его, а не `window()` (тот строит карту сумм по всем ключам, до 200 000 записей).
8. **К сведению из ревью P3a:** `to_text` теперь печатает строку `Settings: name = 'value', …` после дерева — `from_text` (задача 5) разбирает её в `PlanTree.settings`, иначе пара «JSON / текст» не будет эквивалентной (стр. 1551–1602); `from_psql_text` P3a снимает только обрамление psql по умолчанию (граница 2, юникодовые линии, `\x auto`), другие обрамления — задача `from_any`; `serde_json` ограничивает глубину вложенности 128 (около 62 узлов плана) — нужны feature `unbounded_depth` и понятное сообщение; ложная «ошибка оценки» под `Limit` (узел читает меньше, чем оценил планировщик) — `plan_rules` могут исключать узлы под `Limit`.

## Global Constraints

- Те же, что в плане P3a: PostgreSQL 13–18, pgss 1.8–1.12 по `extversion`; пулы и `for_explain()`; без `pg_query` и без новых крейтов; `pg-advisor` без I/O; `queryId` — строка; `ltext!`/`app_err!` с текстами из `en.json`, `pnpm i18n:check`; параметры текстов — по единицам (`…S`, `…Ms`, `…Bytes`, `…Pct`, `count`, `command`); стенд `gp-*` не пересоздаётся; коммиты по-русски с трейлером `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; проверки каждой задачи — `cargo test/clippy/fmt` обоих крейтов, `pnpm test`, `pnpm exec tsc --noEmit`, `pnpm i18n:check`, `pnpm test:scripts`, `pnpm build`; интеграционные — с `GPG_IT_PERF=localhost:5471` (и остальными `GPG_IT_*`).
- Рекомендованные команды приложение **не выполняет**: только показывает и копирует. Единственное обращение к серверу по рекомендации — hypopg: виртуальный индекс в сессии, `READ ONLY`, `hypopg_reset()` до `ROLLBACK`.
- Снимки — только на диске пользователя: `data_dir/snapshots/<connectionId>/<member>/`, каталоги 0700 и файлы 0600 на unix; имя файла — время снятия `yyyy-mm-ddThh-mm-ss`; удаление — из приложения; в архив диагностики не входят.
- Пороги правил и сравнения — константы `pg_advisor::findings::thresholds::PLAN_*` и `COMPARE_*`; тексты правил — ключи `recommend.<id>.{title,evidence,effect,risks}`.
- Правила с пометкой «факт» требуют `actual` в узлах (ANALYZE); без него пропускаются и считаются в `recommendationsSkipped`.

## Уточнения спецификации (решения плана)

1. **Колонки для индекса** извлекаются из `filter` / `indexCond` / `joinFilter` без парсера: условие делится по ` AND ` верхнего уровня, в каждом конъюнкте берётся ведущий идентификатор `(alias.)?col` (в том числе внутри `(col)::type`), если за ним идёт оператор сравнения; конъюнкты с функциями пропускаются; порядок — равенства раньше диапазонов. Не разобралось ни одного — команда с шаблоном `<колонки фильтра>` и без hypopg.
2. **`Recommendation.params`** — как `Finding.params` (доказательство — цифры узла), текст доказательства — шаблон `recommend.<id>.evidence` во фронтенде.
3. **hypopg принимает `CREATE INDEX` без `CONCURRENTLY`**: в рекомендации `command` — с `CONCURRENTLY` (для копирования), `hypo.indexDdl` — без него.
4. **Файл снимка адресуется относительным путём** `<member>/<yyyy-mm-ddThh-mm-ss>.json`; команды проверяют, что путь состоит ровно из двух обычных компонентов (без `..`).
5. **Снимок хранит тексты 500 верхних запросов по времени** и все строки без текстов; сравнение берёт текст из любого из двух снимков.
6. **Режим «между снимками»**: дельта `to − from` по ключу; ключ только в `to` или с уменьшившимися счётчиками — значение `to` целиком; `stats_reset` различается — предупреждение в отчёте (`unavailable{part: between}` с причиной «статистика сбрасывалась»); `coverageS` — разница времён снятия; узел отчёта — узел снимка `to`.
7. **`pg_stat_kcache`**: join по `(queryid, userid, dbid[, top])` через `LEFT JOIN`; версия 2.2+ — колонки `exec_*` и `top`, старше — `user_time`, `system_time`, `reads`, `writes` и без `top` (тогда join без него). Времена CPU в секундах → миллисекунды. Поля `Counters` / `QueryStat`: `cpu_user_ms`, `cpu_sys_ms`, `disk_read_bytes`, `disk_write_bytes` (`Option`, `None` без расширения); сортировка `cpuTime` = user + sys.
8. **`pg_wait_sampling`** читается в `query_detail` только при установленном расширении: профиль накопительный с момента `pg_wait_sampling_reset_profile()`, поэтому `sinceMs: 0`; при `pg_wait_sampling.profile_queries = off` — `unavailable{part: waitsProfile}` и ожидания из сэмплов, как в P3a.
9. **Текстовый парсер** принимает вывод psql (`QUERY PLAN`, разделитель, `(N rows)`), «сырой» EXPLAIN и тело записи auto_explain (строки до первой строки плана — «LOG: duration: … plan:» и т. п. — пропускаются); неразобранная строка деталей — в `extra["text"]` массивом строк.
10. **Подсказки готовности к расширениям** — параметр `hint` пункта `extension` (`hypopg`, `pg_stat_kcache`, `pg_wait_sampling`); фронтенд показывает `readiness.extension.hints.<name>`, если ключ есть.

## Карта файлов

```
crates/pg-advisor/
  src/lib.rs                      + plan_rules
  src/plan_rules/mod.rs           Recommendation, HypoIndex, Skipped, evaluate, columns_of, index_ddl          (1)
  src/plan_rules/scan.rs          seq_scan_filter, index_filter, nested_loop_seq, misestimate                  (1)
  src/plan_rules/exec.rs          spill, sort_limit, heap_fetches, no_pruning, workers_short, subplan, jit     (2)
  src/plan/text_in.rs             from_text                                                                   (5)
  src/plan/json.rs                from_psql_text → from_any (JSON или текст)                                   (5)
  src/queries/compare.rs          Comparison, Change, compare                                                 (6)
  src/queries/mod.rs              Counters/QueryStat: cpu_*, disk_*, QuerySort::CpuTime                       (8)
  src/findings.rs                 thresholds PLAN_*, COMPARE_*                                                 (1, 2, 6)
  src/readiness.rs                hint у пункта extension                                                     (10)
  tests/fixtures/plans/{json,text}/  пары JSON / текст с pg-perf и матрицы                                     (5)
  tests/plans.rs                  эквивалентность пар                                                          (5)
src-tauri/src/
  explain/run.rs                  recommendations в ExplainResult                                             (3)
  explain/hypopg.rs               HypoRequest, HypoResult, check                                              (3)
  commands/explain.rs             hypo_check; parse_plan принимает текст                                      (3, 5)
  snapshots/{mod,store}.rs        Snapshot, SnapshotInfo, SnapshotStore, take/list/read/delete                (6)
  diag/queries.rs                 QueriesMode::Between — report_between; kcache в SQL и row_from              (6, 8)
  diag/query_detail.rs            pg_wait_sampling                                                            (9)
  state.rs                        snapshots: Arc<SnapshotStore>                                               (6)
  commands/snapshots.rs           snapshot_take/list/delete/compare                                           (6)
  readiness/mod.rs                без изменений (факты расширений уже собираются)                             (10)
src-tauri/tests/ it_hypopg.rs (3)  it_snapshots.rs (6)  it_queries.rs (+kcache, +wait_sampling: 8, 9)
src/
  api/types.ts, api/client.ts                                                                                 (4, 6, 8)
  components/queries/plan/{Recommendations,HypoCheck,recommendText}.tsx|ts, PlanRunner (блок)                 (4)
  components/queries/{SnapshotsPanel,SnapshotCompare}.tsx, QueriesView (режим between, кнопка «Снимки»)       (7)
  components/queries/QueriesView.tsx (колонки CPU/диск), QueryOverview (источник ожиданий)                   (8, 9)
  components/connections/TestPanel.tsx (hint)                                                                 (10)
  components/queries/ExplainTab.tsx (вставка текста)                                                          (5)
  locales/{en,ru}.json, docs/i18n/same-as-en.json
docs/guide/{en,ru}/queries.md (разделы P3b), docs/admin.md, docs/smoke/p3b.md, спека                           (11)
```

**Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11. Задача 3 зависит от 1–2 (правила); 4 — от 3 (поле `recommendations`, `hypo_check`); 6 — от P3a (`read_cumulative`, `texts_sql`, `QueriesMode::Between`); 7 — от 6; 8 и 9 — независимы; 11 — последняя.

---

### Task 1: `pg-advisor::plan_rules` — модель рекомендации, колонки из условий, правила сканирований и оценки

**Files:**
- Create: `crates/pg-advisor/src/plan_rules/mod.rs`, `crates/pg-advisor/src/plan_rules/scan.rs`
- Modify: `crates/pg-advisor/src/lib.rs` (`pub mod plan_rules;`), `crates/pg-advisor/src/findings.rs` (пороги `PLAN_*`)
- Test: unit-тесты в `plan_rules/mod.rs` и `scan.rs`

**Interfaces:**
- Consumes: `plan::{PlanTree, PlanNode}`, `findings::{Severity, thresholds}`, `rules::sessions::quote_ident`.
- Produces: `plan_rules::Recommendation { id: String, severity: Severity, node_ids: Vec<u32>, params: Map<String, Value>, command: Option<String>, hypo: Option<HypoIndex> }`; `HypoIndex { index_ddl: String }`; `Report { recommendations: Vec<Recommendation>, skipped: Vec<String> }` (пропущенные правила «факт» без ANALYZE); `evaluate(&PlanTree) -> Report`; `columns_of(cond: &str, alias: Option<&str>) -> Vec<String>`; `index_ddl(schema: Option<&str>, table: &str, cols: &[String], concurrently: bool) -> String`; `walk(&PlanNode, parent: Option<&PlanNode>, f)`; правила `scan::{seq_scan_filter, index_filter, nested_loop_seq, misestimate}`.
  Пороги: `PLAN_SEQ_SCAN_ROWS_READ = 10_000`, `PLAN_FILTER_RATIO = 10.0`, `PLAN_INDEX_FILTER_MIN_REMOVED = 1_000`, `PLAN_NESTED_LOOP_LOOPS = 1_000`, `PLAN_MISESTIMATE_RATIO = 10.0`, `PLAN_MISESTIMATE_MIN_ROWS = 1_000.0`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/plan_rules/mod.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_from_simple_conjuncts_equalities_first() {
        assert_eq!(columns_of("(o.status = 'disputed'::text)", Some("o")), ["status"]);
        assert_eq!(columns_of("((o.created_at >= '2025-01-01'::timestamptz) AND (o.status = 'new'::text))", Some("o")), ["status", "created_at"]);
        assert_eq!(columns_of("((customer_id = 5) AND (lower(email) = 'x'::text))", None), ["customer_id"], "функция пропускается");
        assert_eq!(columns_of("((o.status)::text = 'x'::text)", Some("o")), ["status"], "колонка под приведением");
        assert_eq!(columns_of("(c.id < 100)", Some("c")), ["id"]);
        assert_eq!(columns_of("(a.id = b.id)", Some("a")), ["id"], "ключ соединения со стороны псевдонима");
        assert!(columns_of("(random() > 0.5)", None).is_empty());
        assert_eq!(columns_of("(\"Order Id\" = 1)", None), ["Order Id"]);
        assert!(columns_of("(x.col = 1)", Some("o")).is_empty(), "чужой псевдоним — не колонка таблицы");
    }

    #[test]
    fn index_ddl_quotes_and_optionally_concurrently() {
        assert_eq!(index_ddl(Some("shop"), "orders", &["status".into(), "created_at".into()], true), "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\", \"created_at\");");
        assert_eq!(index_ddl(None, "t", &["a".into()], false), "CREATE INDEX ON \"t\" (\"a\")");
    }

    #[test]
    fn report_skips_fact_rules_without_analyze() {
        let t = crate::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "shop", "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 10, "Plan Width": 8, "Filter": "(status = 'x'::text)"}})).unwrap();
        let r = evaluate(&t);
        assert!(r.recommendations.is_empty());
        assert!(r.skipped.contains(&"plan_seq_scan_filter".to_string()) && r.skipped.contains(&"plan_misestimate".to_string()));
    }

    #[test]
    fn recommendation_json_shape() {
        let r = Recommendation::new("plan_seq_scan_filter", Severity::Warning, 1)
            .param("rowsRead", 500_000)
            .command("CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");")
            .hypo("CREATE INDEX ON \"shop\".\"orders\" (\"status\")");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["id"], "plan_seq_scan_filter");
        assert_eq!(v["nodeIds"], serde_json::json!([1]));
        assert_eq!(v["params"]["rowsRead"], 500_000);
        assert_eq!(v["hypo"]["indexDdl"], "CREATE INDEX ON \"shop\".\"orders\" (\"status\")");
    }
}
```

`crates/pg-advisor/src/plan_rules/scan.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use crate::plan_rules::evaluate;
    use serde_json::{Value, json};

    fn ids(v: &Value) -> Vec<String> {
        evaluate(&from_json(v).unwrap()).recommendations.iter().map(|r| r.id.clone()).collect()
    }

    fn seq(rows: f64, removed: i64, actual_rows: f64) -> Value {
        json!({"Plan": {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 100.0,
            "Plan Rows": rows, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": actual_rows, "Actual Loops": 1,
            "Filter": "(o.status = 'disputed'::text)", "Rows Removed by Filter": removed}})
    }

    #[test]
    fn seq_scan_with_heavy_filter_recommends_an_index_with_hypo() {
        let r = evaluate(&from_json(&seq(500.0, 499_500, 500.0)).unwrap());
        let rec = r.recommendations.iter().find(|r| r.id == "plan_seq_scan_filter").expect("рекомендация");
        assert_eq!(rec.node_ids, [0]);
        assert_eq!(rec.params["rowsRead"], 500_000);
        assert_eq!(rec.params["rowsReturned"], 500);
        assert_eq!(rec.params["table"], "shop.orders");
        assert_eq!(rec.command.as_deref(), Some("CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");"));
        assert_eq!(rec.hypo.as_ref().map(|h| h.index_ddl.as_str()), Some("CREATE INDEX ON \"shop\".\"orders\" (\"status\")"));
        // Мало строк или малый отсев — молчит.
        assert!(!ids(&seq(500.0, 5_000, 500.0)).contains(&"plan_seq_scan_filter".to_string()));
        assert!(!ids(&seq(500.0, 2_000, 500.0)).contains(&"plan_seq_scan_filter".to_string()));
    }

    #[test]
    fn seq_scan_with_function_filter_has_template_command_without_hypo() {
        let mut v = seq(500.0, 499_500, 500.0);
        v["Plan"]["Filter"] = json!("(lower(o.status) = 'disputed'::text)");
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r.recommendations.iter().find(|r| r.id == "plan_seq_scan_filter").unwrap();
        assert!(rec.command.as_deref().unwrap().contains("(<columns>)"), "{:?}", rec.command);
        assert!(rec.hypo.is_none());
    }

    #[test]
    fn index_scan_with_filter_recommends_a_composite_index() {
        let v = json!({"Plan": {"Node Type": "Index Scan", "Index Name": "orders_customer_idx", "Relation Name": "orders", "Schema": "shop", "Alias": "o",
            "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 10, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": 10, "Actual Loops": 1,
            "Index Cond": "(o.customer_id = 5)", "Filter": "(o.status = 'new'::text)", "Rows Removed by Filter": 5000}});
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r.recommendations.iter().find(|r| r.id == "plan_index_filter").unwrap();
        assert_eq!(rec.hypo.as_ref().unwrap().index_ddl, "CREATE INDEX ON \"shop\".\"orders\" (\"customer_id\", \"status\")");
        assert_eq!(rec.params["rowsRemoved"], 5000);
    }

    #[test]
    fn nested_loop_over_seq_scan_recommends_join_key_index() {
        let v = json!({"Plan": {"Node Type": "Nested Loop", "Join Type": "Inner", "Startup Cost": 0.0, "Total Cost": 1000.0, "Plan Rows": 100, "Plan Width": 8,
            "Actual Startup Time": 0.1, "Actual Total Time": 900.0, "Actual Rows": 100, "Actual Loops": 1, "Plans": [
              {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "audit", "Schema": "shop", "Alias": "a", "Index Name": "audit_order_idx", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 100, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 2000, "Actual Loops": 1, "Index Cond": "(a.order_id = 1)"},
              {"Node Type": "Seq Scan", "Parent Relationship": "Inner", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 5.0, "Plan Rows": 1, "Plan Width": 8, "Actual Startup Time": 0.0, "Actual Total Time": 0.4, "Actual Rows": 1, "Actual Loops": 2000, "Filter": "(o.id = a.order_id)", "Rows Removed by Filter": 499999}
            ]}});
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r.recommendations.iter().find(|r| r.id == "plan_nested_loop_seq").unwrap();
        assert_eq!(rec.node_ids, [0, 2]);
        assert_eq!(rec.params["loops"], 2000);
        assert_eq!(rec.hypo.as_ref().unwrap().index_ddl, "CREATE INDEX ON \"shop\".\"orders\" (\"id\")");
        // Оценка строк внешнего узла ошиблась в 20 раз — отдельная рекомендация ANALYZE.
        let mis = r.recommendations.iter().find(|r| r.id == "plan_misestimate").unwrap();
        assert_eq!(mis.node_ids, [1]);
        assert_eq!(mis.params["table"], "shop.audit");
        assert!(mis.command.as_deref().unwrap().starts_with("ANALYZE \"shop\".\"audit\";"));
    }

    #[test]
    fn misestimate_needs_volume_and_suggests_statistics_for_two_columns() {
        let mut v = seq(10.0, 0, 50.0);
        assert!(!ids(&v).contains(&"plan_misestimate".to_string()), "мало строк");
        v = seq(100.0, 0, 5000.0);
        v["Plan"]["Filter"] = json!("((o.status = 'new'::text) AND (o.region = 'eu'::text))");
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r.recommendations.iter().find(|r| r.id == "plan_misestimate").unwrap();
        assert_eq!(rec.params["ratio"], 50.0);
        let cmd = rec.command.as_deref().unwrap();
        assert!(cmd.contains("ANALYZE \"shop\".\"orders\";"));
        assert!(cmd.contains("ALTER TABLE \"shop\".\"orders\" ALTER COLUMN \"status\" SET STATISTICS 1000;"));
        assert!(cmd.contains("CREATE STATISTICS ON \"status\", \"region\" FROM \"shop\".\"orders\";"), "{cmd}");
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml plan_rules`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/findings.rs` — в `thresholds`:

```rust
    /// Рекомендации по плану (спец. P3 §9.1).
    pub const PLAN_SEQ_SCAN_ROWS_READ: f64 = 10_000.0;
    pub const PLAN_FILTER_RATIO: f64 = 10.0;
    pub const PLAN_INDEX_FILTER_MIN_REMOVED: f64 = 1_000.0;
    pub const PLAN_NESTED_LOOP_LOOPS: u64 = 1_000;
    pub const PLAN_MISESTIMATE_RATIO: f64 = 10.0;
    pub const PLAN_MISESTIMATE_MIN_ROWS: f64 = 1_000.0;
    pub const PLAN_WORK_MEM_FACTOR: f64 = 1.5;
    pub const PLAN_WORK_MEM_MAX_KB: i64 = 1024 * 1024;
    pub const PLAN_SORT_LIMIT_FACTOR: f64 = 100.0;
    pub const PLAN_HEAP_FETCH_SHARE: f64 = 0.1;
    pub const PLAN_HEAP_FETCH_MIN: f64 = 1_000.0;
    pub const PLAN_APPEND_MIN_CHILDREN: usize = 10;
    pub const PLAN_APPEND_EMPTY_SHARE: f64 = 0.5;
    pub const PLAN_SUBPLAN_LOOPS: u64 = 1_000;
    pub const PLAN_JIT_SHARE: f64 = 0.5;
```

`crates/pg-advisor/src/lib.rs` — `pub mod plan_rules;`.

`crates/pg-advisor/src/plan_rules/mod.rs`:

```rust
//! Рекомендации по плану (спец. P3 §9.1): правила над `PlanTree`, доказательство — цифры узла, готовая команда
//! (приложение её не выполняет), индекс-гипотеза для hypopg. Тексты — во фронтенде по `id` и `params`.

mod scan;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::findings::Severity;
use crate::plan::{PlanNode, PlanTree};
use crate::rules::sessions::quote_ident;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoIndex {
    /// `CREATE INDEX ON …` без `CONCURRENTLY` — так принимает `hypopg_create_index`.
    pub index_ddl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    /// Ключ текстов `recommend.<id>.{title,evidence,effect,risks}`.
    pub id: String,
    pub severity: Severity,
    /// Узлы плана, к которым относится рекомендация («Показать узел»).
    pub node_ids: Vec<u32>,
    pub params: Map<String, Value>,
    /// Готовая команда для копирования; `None` — совет без команды.
    pub command: Option<String>,
    pub hypo: Option<HypoIndex>,
}

impl Recommendation {
    pub fn new(id: &str, severity: Severity, node_id: u32) -> Self {
        Self {
            id: id.to_string(),
            severity,
            node_ids: vec![node_id],
            params: Map::new(),
            command: None,
            hypo: None,
        }
    }

    pub fn also_node(mut self, id: u32) -> Self {
        self.node_ids.push(id);
        self
    }

    pub fn param(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_string(), value.into());
        self
    }

    pub fn command(mut self, c: impl Into<String>) -> Self {
        self.command = Some(c.into());
        self
    }

    pub fn hypo(mut self, ddl: impl Into<String>) -> Self {
        self.hypo = Some(HypoIndex { index_ddl: ddl.into() });
        self
    }
}

/// Итог: рекомендации и правила «факт», пропущенные без ANALYZE.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub recommendations: Vec<Recommendation>,
    pub skipped: Vec<String>,
}

/// Правила, которым нужен факт выполнения (задача 2 дополняет список правилами выполнения).
pub const FACT_RULES: [&str; 4] = [
    "plan_seq_scan_filter",
    "plan_index_filter",
    "plan_nested_loop_seq",
    "plan_misestimate",
];

pub fn evaluate(t: &PlanTree) -> Report {
    let with_actual = t.root.actual.is_some();
    let mut out = Vec::new();
    walk(&t.root, None, &mut |n, _parent| {
        if with_actual {
            out.extend(scan::seq_scan_filter(n));
            out.extend(scan::index_filter(n));
            out.extend(scan::nested_loop_seq(n));
            out.extend(scan::misestimate(n));
        }
    });
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then_with(|| a.node_ids.cmp(&b.node_ids)).then_with(|| a.id.cmp(&b.id)));
    Report {
        recommendations: out,
        skipped: if with_actual { Vec::new() } else { FACT_RULES.iter().map(|s| s.to_string()).collect() },
    }
}

/// Обход в глубину с родителем.
pub fn walk<'a>(n: &'a PlanNode, parent: Option<&'a PlanNode>, f: &mut impl FnMut(&'a PlanNode, Option<&'a PlanNode>)) {
    f(n, parent);
    for c in &n.children {
        walk(c, Some(n), f);
    }
}

/// Полное имя отношения для текста и команд.
pub fn table_name(n: &PlanNode) -> Option<String> {
    n.relation.as_ref().map(|r| match &r.schema {
        Some(s) => format!("{s}.{}", r.name),
        None => r.name.clone(),
    })
}

pub fn qualified(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(table)),
        None => quote_ident(table),
    }
}

/// Делит условие на конъюнкты верхнего уровня по ` AND ` (скобки учитываются).
fn conjuncts(cond: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let b = cond.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'\'' => {
                i += 1;
                while i < b.len() && b[i] != b'\'' {
                    i += 1;
                }
            }
            _ if depth == 0 && cond[i..].starts_with(" AND ") => {
                out.push(cond[start..i].to_string());
                i += 5;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(cond[start..].to_string());
    out
}

fn strip_parens(s: &str) -> &str {
    let mut s = s.trim();
    while s.starts_with('(') && s.ends_with(')') && balanced(&s[1..s.len() - 1]) {
        s = s[1..s.len() - 1].trim();
    }
    s
}

fn balanced(s: &str) -> bool {
    let mut d = 0i32;
    for c in s.chars() {
        match c {
            '(' => d += 1,
            ')' => {
                d -= 1;
                if d < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    d == 0
}

/// Идентификатор в начале строки: простой или в кавычках; возвращает имя и остаток.
fn ident(s: &str) -> Option<(String, &str)> {
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some((rest[..end].to_string(), &rest[end + 1..]));
    }
    let end = s.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(s.len());
    if end == 0 || s.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    Some((s[..end].to_string(), &s[end..]))
}

const OPERATORS: [&str; 10] = ["=", "<>", "<=", ">=", "<", ">", "~~", "!~~", "IS", "IN"];

/// Колонки простых сравнений `col op …` (в том числе `alias.col` своего псевдонима и `(col)::type`); равенства —
/// первыми. Конъюнкты с функциями и чужими псевдонимами пропускаются.
pub fn columns_of(cond: &str, alias: Option<&str>) -> Vec<String> {
    let mut eq = Vec::new();
    let mut range = Vec::new();
    for c in conjuncts(cond) {
        let s = strip_parens(&c);
        // `(col)::type op …` — приведение снимается.
        let s = if s.starts_with('(') {
            match s.find(")::") {
                Some(i) if balanced(&s[1..i]) => &s[1..i],
                _ => s,
            }
        } else {
            s
        };
        let Some((first, rest)) = ident(s) else { continue };
        let (col, rest) = match rest.strip_prefix('.') {
            Some(r) => {
                if alias != Some(first.as_str()) {
                    continue;
                }
                match ident(r) {
                    Some(x) => x,
                    None => continue,
                }
            }
            None => (first, rest),
        };
        // После колонки — возможное приведение `)::type` (когда скобка снята выше остаток пуст) и оператор.
        let rest = rest.trim_start_matches(')');
        let rest = match rest.find(char::is_whitespace) {
            Some(0) => rest.trim_start(),
            Some(i) if rest[..i].starts_with("::") => rest[i..].trim_start(),
            _ => rest,
        };
        let Some(op) = OPERATORS.iter().find(|op| rest.starts_with(**op)) else { continue };
        if rest.contains('(') && rest[..rest.find('(').unwrap()].trim().is_empty() {
            continue;
        }
        if *op == "=" { eq.push(col) } else { range.push(col) }
    }
    eq.extend(range);
    eq.dedup();
    eq
}

/// `CREATE INDEX [CONCURRENTLY] ON "s"."t" ("a", "b")`; с `CONCURRENTLY` — команда для копирования с `;`.
pub fn index_ddl(schema: Option<&str>, table: &str, cols: &[String], concurrently: bool) -> String {
    let cols: Vec<String> = cols.iter().map(|c| quote_ident(c)).collect();
    let target = qualified(schema, table);
    if concurrently {
        format!("CREATE INDEX CONCURRENTLY ON {target} ({});", cols.join(", "))
    } else {
        format!("CREATE INDEX ON {target} ({})", cols.join(", "))
    }
}

/// Команда и гипотеза по колонкам: разобралось — индекс, иначе шаблон без hypopg.
pub fn index_recommendation(mut r: Recommendation, n: &PlanNode, cols: &[String]) -> Recommendation {
    let Some(rel) = &n.relation else { return r };
    if cols.is_empty() {
        r = r.command(format!("CREATE INDEX CONCURRENTLY ON {} (<columns>);", qualified(rel.schema.as_deref(), &rel.name)));
    } else {
        r = r
            .command(index_ddl(rel.schema.as_deref(), &rel.name, cols, true))
            .hypo(index_ddl(rel.schema.as_deref(), &rel.name, cols, false))
            .param("columns", cols.join(", "));
    }
    r
}
```

Шаблон колонок в команде без разобранных условий — `(<columns>)`: команда показывается как есть, без перевода.

`crates/pg-advisor/src/plan_rules/scan.rs`:

```rust
//! Правила сканирований и оценки (спец. P3 §9.1): Seq Scan с отсевом, Index Scan с фильтром, Nested Loop над
//! Seq Scan, ошибка оценки.

use crate::findings::{Severity, thresholds as th};
use crate::plan::PlanNode;
use crate::plan_rules::{Recommendation, columns_of, index_recommendation, qualified, table_name};
use crate::rules::sessions::quote_ident;

fn alias(n: &PlanNode) -> Option<&str> {
    n.relation.as_ref().and_then(|r| r.alias.as_deref().or(Some(r.name.as_str())))
}

/// Seq Scan: прочитано ≥ 10 000 строк (с учётом циклов), отсев ≥ 10 × возвращено.
pub fn seq_scan_filter(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Seq Scan" {
        return None;
    }
    let a = n.actual?;
    let removed = n.rows_removed_by_filter? as f64;
    let returned = a.rows;
    let read = (returned + removed) * a.loops as f64;
    if read < th::PLAN_SEQ_SCAN_ROWS_READ || removed < th::PLAN_FILTER_RATIO * returned.max(1.0) {
        return None;
    }
    let cols = n.filter.as_deref().map(|f| columns_of(f, alias(n))).unwrap_or_default();
    let r = Recommendation::new("plan_seq_scan_filter", Severity::Warning, n.id)
        .param("table", table_name(n).unwrap_or_default())
        .param("rowsRead", read as i64)
        .param("rowsReturned", (returned * a.loops as f64) as i64)
        .param("loops", a.loops);
    Some(index_recommendation(r, n, &cols))
}

/// Index Scan / Bitmap Heap Scan с фильтром: отсев ≥ 10 × rows и ≥ 1000 — составной индекс (условие индекса + фильтр).
pub fn index_filter(n: &PlanNode) -> Option<Recommendation> {
    if !matches!(n.node_type.as_str(), "Index Scan" | "Bitmap Heap Scan") {
        return None;
    }
    let a = n.actual?;
    let removed = n.rows_removed_by_filter? as f64;
    if removed < th::PLAN_INDEX_FILTER_MIN_REMOVED || removed < th::PLAN_FILTER_RATIO * a.rows.max(1.0) {
        return None;
    }
    let al = alias(n);
    let mut cols = n.index_cond.as_deref().or(n.recheck_cond.as_deref()).map(|c| columns_of(c, al)).unwrap_or_default();
    for c in n.filter.as_deref().map(|f| columns_of(f, al)).unwrap_or_default() {
        if !cols.contains(&c) {
            cols.push(c);
        }
    }
    let r = Recommendation::new("plan_index_filter", Severity::Warning, n.id)
        .param("table", table_name(n).unwrap_or_default())
        .param("index", n.index_name.clone().unwrap_or_default())
        .param("rowsRemoved", removed as i64)
        .param("rowsReturned", a.rows as i64);
    Some(index_recommendation(r, n, &cols))
}

/// Nested Loop, внутренний ребёнок — Seq Scan с loops ≥ 1000: индекс по ключу соединения.
pub fn nested_loop_seq(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Nested Loop" {
        return None;
    }
    let inner = n.children.iter().find(|c| c.parent_relationship.as_deref() == Some("Inner") && c.node_type == "Seq Scan")?;
    let a = inner.actual?;
    if a.loops < th::PLAN_NESTED_LOOP_LOOPS {
        return None;
    }
    let al = alias(inner);
    let cond = inner.filter.as_deref().or(n.join_filter.as_deref()).unwrap_or("");
    let cols = columns_of(cond, al);
    let r = Recommendation::new("plan_nested_loop_seq", Severity::Warning, n.id)
        .also_node(inner.id)
        .param("table", table_name(inner).unwrap_or_default())
        .param("loops", a.loops)
        .param("rowsPerLoop", a.rows + inner.rows_removed_by_filter.unwrap_or(0) as f64);
    Some(index_recommendation(r, inner, &cols))
}

/// Ошибка оценки ≥ ×10 при объёме ≥ 1000 строк: ANALYZE, SET STATISTICS, при двух и более колонках — CREATE STATISTICS.
pub fn misestimate(n: &PlanNode) -> Option<Recommendation> {
    let a = n.actual?;
    let rel = n.relation.as_ref()?;
    let ratio = n.estimate_ratio?;
    let big = n.plan_rows.max(a.rows) >= th::PLAN_MISESTIMATE_MIN_ROWS;
    if !big || (ratio < th::PLAN_MISESTIMATE_RATIO && ratio > 1.0 / th::PLAN_MISESTIMATE_RATIO) {
        return None;
    }
    let al = alias(n);
    let cols: Vec<String> = [n.filter.as_deref(), n.index_cond.as_deref()]
        .into_iter()
        .flatten()
        .flat_map(|c| columns_of(c, al))
        .fold(Vec::new(), |mut acc, c| {
            if !acc.contains(&c) {
                acc.push(c);
            }
            acc
        });
    let target = qualified(rel.schema.as_deref(), &rel.name);
    let mut cmd = format!("ANALYZE {target};");
    for c in &cols {
        cmd.push_str(&format!("\nALTER TABLE {target} ALTER COLUMN {} SET STATISTICS 1000;", quote_ident(c)));
    }
    if cols.len() >= 2 {
        let list: Vec<String> = cols.iter().map(|c| quote_ident(c)).collect();
        cmd.push_str(&format!("\nCREATE STATISTICS ON {} FROM {target};", list.join(", ")));
    }
    Some(
        Recommendation::new("plan_misestimate", Severity::Warning, n.id)
            .param("table", table_name(n).unwrap_or_default())
            .param("planRows", n.plan_rows as i64)
            .param("actualRows", a.rows as i64)
            .param("ratio", if ratio >= 1.0 { ratio } else { 1.0 / ratio })
            .param("direction", if ratio >= 1.0 { "under" } else { "over" })
            .command(cmd),
    )
}
```

Модуль `exec` (правила выполнения) появляется в задаче 2: она добавляет `mod exec;`, вызовы в `evaluate` (с параметром `parent`) и остальные id в `FACT_RULES`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor
git commit -m "feat(pg-advisor): рекомендации по плану — модель, колонки из условий, правила Seq Scan, Index Scan с фильтром, Nested Loop, ошибка оценки

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: `pg-advisor::plan_rules::exec` — сброс на диск, Sort под Limit, heap fetches, pruning, воркеры, SubPlan, JIT

**Files:**
- Create: `crates/pg-advisor/src/plan_rules/exec.rs`
- Modify: `crates/pg-advisor/src/plan_rules/mod.rs` (`mod exec;`, вызовы с `parent`, `FACT_RULES: [&str; 9]`)
- Test: unit-тесты в `exec.rs`; расширить `report_skips_fact_rules_without_analyze`

**Interfaces:**
- Produces: `exec::{spill, sort_limit(n, parent), heap_fetches, no_pruning(n, with_actual), workers_short, subplan_per_row, jit_heavy(tree)}` — каждое `Option<Recommendation>`; параметры: `spill` — `kind` (`sort` | `hash` | `bitmap`), `spaceKb`, `workMemKb`; `sort_limit` — `inputRows`, `limitRows`, `keys`; `heap_fetches` — `heapFetches`, `rows`, `table`; `no_pruning` — `children`, `emptyChildren`; `workers_short` — `planned`, `launched`; `subplan_per_row` — `loops`; `jit_heavy` — `jitMs`, `execMs`.

- [ ] **Step 1: Падающие тесты**

```rust
#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use crate::plan_rules::evaluate;
    use serde_json::{Value, json};

    fn rec<'a>(v: &Value, id: &str) -> Option<crate::plan_rules::Recommendation> {
        evaluate(&from_json(v).unwrap()).recommendations.into_iter().find(|r| r.id == id)
    }

    fn with_actual(mut node: Value, rows: f64, loops: i64) -> Value {
        node["Actual Startup Time"] = json!(0.1);
        node["Actual Total Time"] = json!(10.0);
        node["Actual Rows"] = json!(rows);
        node["Actual Loops"] = json!(loops);
        node
    }

    fn base(node_type: &str) -> Value {
        json!({"Node Type": node_type, "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 1000, "Plan Width": 8})
    }

    #[test]
    fn spill_on_sort_hash_and_lossy_bitmap() {
        let mut sort = with_actual(base("Sort"), 1000.0, 1);
        sort["Sort Method"] = json!("external merge");
        sort["Sort Space Type"] = json!("Disk");
        sort["Sort Space Used"] = json!(20_480);
        sort["Sort Key"] = json!(["o.total DESC"]);
        let r = rec(&json!({"Plan": sort}), "plan_spill").unwrap();
        assert_eq!((r.params["kind"].as_str(), r.params["spaceKb"].as_i64(), r.params["workMemKb"].as_i64()), (Some("sort"), Some(20_480), Some(30_720)));
        assert_eq!(r.command.as_deref(), Some("SET work_mem = '30720kB';"));
        let mut hash = with_actual(base("Hash"), 1000.0, 1);
        hash["Hash Batches"] = json!(4);
        hash["Peak Memory Usage"] = json!(1024);
        let r = rec(&json!({"Plan": hash}), "plan_spill").unwrap();
        assert_eq!(r.params["kind"], "hash");
        let mut bitmap = with_actual(base("Bitmap Heap Scan"), 1000.0, 1);
        bitmap["Lossy Heap Blocks"] = json!(300);
        bitmap["Exact Heap Blocks"] = json!(10);
        assert_eq!(rec(&json!({"Plan": bitmap}), "plan_spill").unwrap().params["kind"], "bitmap");
        // Предел work_mem — 1 ГБ.
        let mut huge = with_actual(base("Sort"), 1.0, 1);
        huge["Sort Space Type"] = json!("Disk");
        huge["Sort Space Used"] = json!(2_000_000);
        huge["Sort Method"] = json!("external merge");
        assert_eq!(rec(&json!({"Plan": huge}), "plan_spill").unwrap().params["workMemKb"], 1024 * 1024);
    }

    #[test]
    fn sort_under_limit_recommends_an_index_even_without_analyze() {
        let mut sort = base("Sort");
        sort["Sort Key"] = json!(["o.total DESC"]);
        sort["Plan Rows"] = json!(500_000);
        sort["Plans"] = json!([{"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 50.0, "Plan Rows": 500_000, "Plan Width": 8}]);
        let mut limit = base("Limit");
        limit["Plan Rows"] = json!(10);
        limit["Plans"] = json!([sort]);
        let r = rec(&json!({"Plan": limit}), "plan_sort_limit").unwrap();
        assert_eq!((r.params["inputRows"].as_i64(), r.params["limitRows"].as_i64()), (Some(500_000), Some(10)));
        assert_eq!(r.hypo.as_ref().unwrap().index_ddl, "CREATE INDEX ON \"shop\".\"orders\" (\"total\")");
        assert!(r.command.as_deref().unwrap().contains("(\"total\")"));
        // Малый вход — молчит.
        let mut small = json!({"Plan": limit});
        small["Plan"]["Plans"][0]["Plan Rows"] = json!(500);
        assert!(rec(&small, "plan_sort_limit").is_none());
    }

    #[test]
    fn heap_fetches_on_index_only_scan() {
        let mut ios = with_actual(base("Index Only Scan"), 10_000.0, 1);
        ios["Relation Name"] = json!("balances");
        ios["Schema"] = json!("shop");
        ios["Heap Fetches"] = json!(2_500);
        let r = rec(&json!({"Plan": ios}), "plan_heap_fetches").unwrap();
        assert_eq!(r.command.as_deref(), Some("VACUUM \"shop\".\"balances\";"));
        ios["Heap Fetches"] = json!(500);
        assert!(rec(&json!({"Plan": ios}), "plan_heap_fetches").is_none(), "меньше 10 %");
    }

    #[test]
    fn append_without_pruning() {
        let child = |i: i32, rows: f64| with_actual(json!({"Node Type": "Seq Scan", "Parent Relationship": "Member", "Relation Name": format!("events_{i}"), "Schema": "shop", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 10, "Plan Width": 8}), rows, 1);
        let mut append = with_actual(base("Append"), 10.0, 1);
        append["Plans"] = json!((0..12).map(|i| child(i, if i < 2 { 5.0 } else { 0.0 })).collect::<Vec<_>>());
        let r = rec(&json!({"Plan": append}), "plan_no_pruning").unwrap();
        assert_eq!((r.params["children"].as_i64(), r.params["emptyChildren"].as_i64()), (Some(12), Some(10)));
        append["Subplans Removed"] = json!(20);
        assert!(rec(&json!({"Plan": append}), "plan_no_pruning").is_none(), "pruning сработал во время выполнения");
    }

    #[test]
    fn workers_subplan_and_jit() {
        let mut gather = with_actual(base("Gather"), 1000.0, 1);
        gather["Workers Planned"] = json!(4);
        gather["Workers Launched"] = json!(1);
        let r = rec(&json!({"Plan": gather}), "plan_workers_short").unwrap();
        assert_eq!((r.params["planned"].as_i64(), r.params["launched"].as_i64()), (Some(4), Some(1)));
        let mut sub = with_actual(base("Aggregate"), 1.0, 3000);
        sub["Parent Relationship"] = json!("SubPlan");
        sub["Subplan Name"] = json!("SubPlan 1");
        let mut root = with_actual(base("Index Scan"), 3000.0, 1);
        root["Plans"] = json!([sub]);
        let r = rec(&json!({"Plan": root}), "plan_subplan_per_row").unwrap();
        assert_eq!(r.params["loops"], 3000);
        let plan = json!({"Plan": with_actual(base("Aggregate"), 1.0, 1), "Execution Time": 100.0, "JIT": {"Functions": 5, "Timing": {"Generation": 20.0, "Inlining": 20.0, "Optimization": 20.0, "Emission": 0.0, "Total": 60.0}}});
        let r = rec(&plan, "plan_jit_heavy").unwrap();
        assert_eq!(r.command.as_deref(), Some("SET jit = off;"));
        assert_eq!(r.params["jitMs"], 60.0);
    }
}
```

В `plan_rules/mod.rs` тест `report_skips_fact_rules_without_analyze` дополнить: `assert_eq!(r.skipped.len(), 9); assert!(!r.skipped.contains(&"plan_sort_limit".to_string()) && !r.skipped.contains(&"plan_no_pruning".to_string()));`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml plan_rules`
Expected: ошибка компиляции (`exec` нет).

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/plan_rules/exec.rs`:

```rust
//! Правила выполнения (спец. P3 §9.1): сброс на диск, Sort под Limit, heap fetches, Append без pruning, нехватка
//! воркеров, SubPlan на строку, тяжёлый JIT.

use crate::findings::{Severity, thresholds as th};
use crate::plan::{PlanNode, PlanTree};
use crate::plan_rules::{Recommendation, index_recommendation, qualified, table_name};

/// Sort на диске, Hash с батчами, lossy bitmap: `work_mem` ×1,5 от объёма (не больше 1 ГБ).
pub fn spill(n: &PlanNode) -> Option<Recommendation> {
    n.actual?;
    let (kind, space_kb) = if n.sort_space_type.as_deref() == Some("Disk") {
        ("sort", n.sort_space_kb.unwrap_or(0))
    } else if n.hash_batches.is_some_and(|b| b > 1) {
        ("hash", n.peak_memory_kb.unwrap_or(0) * n.hash_batches.unwrap_or(1))
    } else if n.lossy_heap_blocks.is_some_and(|l| l > 0) {
        ("bitmap", (n.lossy_heap_blocks.unwrap_or(0) + n.exact_heap_blocks.unwrap_or(0)) * 8)
    } else {
        return None;
    };
    let work_mem_kb = ((space_kb as f64 * th::PLAN_WORK_MEM_FACTOR) as i64).clamp(1024, th::PLAN_WORK_MEM_MAX_KB);
    Some(
        Recommendation::new("plan_spill", Severity::Warning, n.id)
            .param("kind", kind)
            .param("spaceKb", space_kb)
            .param("workMemKb", work_mem_kb)
            .param("keys", n.sort_key.join(", "))
            .command(format!("SET work_mem = '{work_mem_kb}kB';")),
    )
}

/// Sort под Limit: вход ≥ 100 × предел — индекс под ключ сортировки (по оценке; с фактом — по факту).
pub fn sort_limit(n: &PlanNode, parent: Option<&PlanNode>) -> Option<Recommendation> {
    if !matches!(n.node_type.as_str(), "Sort" | "Incremental Sort") || parent?.node_type != "Limit" {
        return None;
    }
    let limit_rows = parent?.actual.map(|a| a.rows).unwrap_or(parent?.plan_rows).max(1.0);
    let input = n.children.first()?;
    let input_rows = input.actual.map(|a| a.rows * a.loops as f64).unwrap_or(input.plan_rows);
    if input_rows < th::PLAN_SORT_LIMIT_FACTOR * limit_rows {
        return None;
    }
    let scan = n.children.iter().find(|c| c.relation.is_some()).unwrap_or(input);
    let cols: Vec<String> = n
        .sort_key
        .iter()
        .filter_map(|k| {
            let k = k.trim();
            let col = k.split_whitespace().next()?;
            let col = col.rsplit('.').next()?;
            (!col.contains('(')).then(|| col.trim_matches('"').to_string())
        })
        .collect();
    let r = Recommendation::new("plan_sort_limit", Severity::Info, n.id)
        .also_node(parent?.id)
        .param("inputRows", input_rows as i64)
        .param("limitRows", limit_rows as i64)
        .param("keys", n.sort_key.join(", "))
        .param("table", table_name(scan).unwrap_or_default());
    Some(if scan.relation.is_some() { index_recommendation(r, scan, &cols) } else { r })
}

/// Index Only Scan с heap fetches ≥ 10 % строк и ≥ 1000: VACUUM (карта видимости).
pub fn heap_fetches(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Index Only Scan" {
        return None;
    }
    let a = n.actual?;
    let fetches = n.heap_fetches? as f64;
    let rows = (a.rows * a.loops as f64).max(1.0);
    if fetches < th::PLAN_HEAP_FETCH_MIN || fetches < th::PLAN_HEAP_FETCH_SHARE * rows {
        return None;
    }
    let rel = n.relation.as_ref()?;
    Some(
        Recommendation::new("plan_heap_fetches", Severity::Info, n.id)
            .param("table", table_name(n).unwrap_or_default())
            .param("heapFetches", fetches as i64)
            .param("rows", rows as i64)
            .command(format!("VACUUM {};", qualified(rel.schema.as_deref(), &rel.name))),
    )
}

/// Append / Merge Append по ≥ 10 партициям без `Subplans Removed`; с фактом — ≥ 50 % детей пустые.
pub fn no_pruning(n: &PlanNode, with_actual: bool) -> Option<Recommendation> {
    if !matches!(n.node_type.as_str(), "Append" | "Merge Append") || n.subplans_removed.is_some_and(|s| s > 0) {
        return None;
    }
    let scans: Vec<&PlanNode> = n.children.iter().filter(|c| c.relation.is_some()).collect();
    if scans.len() < th::PLAN_APPEND_MIN_CHILDREN {
        return None;
    }
    let empty = scans.iter().filter(|c| c.actual.is_some_and(|a| a.rows == 0.0 || a.loops == 0)).count();
    if with_actual && (empty as f64) < th::PLAN_APPEND_EMPTY_SHARE * scans.len() as f64 {
        return None;
    }
    Some(
        Recommendation::new("plan_no_pruning", if with_actual { Severity::Warning } else { Severity::Info }, n.id)
            .param("children", scans.len() as i64)
            .param("emptyChildren", empty as i64),
    )
}

/// Воркеров запущено меньше, чем запланировано.
pub fn workers_short(n: &PlanNode) -> Option<Recommendation> {
    n.actual?;
    let (planned, launched) = (n.workers_planned?, n.workers_launched?);
    if launched >= planned {
        return None;
    }
    Some(
        Recommendation::new("plan_workers_short", Severity::Info, n.id)
            .param("planned", planned)
            .param("launched", launched)
            .command("SHOW max_parallel_workers;\nSHOW max_worker_processes;".to_string()),
    )
}

/// SubPlan, выполненный ≥ 1000 раз: переписать на JOIN / LATERAL.
pub fn subplan_per_row(n: &PlanNode) -> Option<Recommendation> {
    if n.parent_relationship.as_deref() != Some("SubPlan") {
        return None;
    }
    let a = n.actual?;
    if a.loops < th::PLAN_SUBPLAN_LOOPS {
        return None;
    }
    Some(
        Recommendation::new("plan_subplan_per_row", Severity::Warning, n.id)
            .param("loops", a.loops)
            .param("name", n.subplan_name.clone().unwrap_or_default()),
    )
}

/// JIT дольше половины выполнения.
pub fn jit_heavy(t: &PlanTree) -> Option<Recommendation> {
    let jit = t.jit.as_ref()?;
    let exec = t.execution_ms?;
    if exec <= 0.0 || jit.total_ms < th::PLAN_JIT_SHARE * exec {
        return None;
    }
    Some(
        Recommendation::new("plan_jit_heavy", Severity::Info, t.root.id)
            .param("jitMs", jit.total_ms)
            .param("execMs", exec)
            .param("functions", jit.functions)
            .command("SET jit = off;".to_string()),
    )
}
```

`plan_rules/mod.rs`: добавить `mod exec;`; в `evaluate` замыкание обхода становится `|n, parent|`, блок `if with_actual { … }` дополняется `out.extend(exec::spill(n)); out.extend(exec::heap_fetches(n)); out.extend(exec::workers_short(n)); out.extend(exec::subplan_per_row(n));`, вне блока — `out.extend(exec::sort_limit(n, parent)); out.extend(exec::no_pruning(n, with_actual));`, после обхода — `if with_actual { out.extend(exec::jit_heavy(t)); }`; `FACT_RULES: [&str; 9]` — четыре id задачи 1 плюс `plan_spill`, `plan_heap_fetches`, `plan_workers_short`, `plan_subplan_per_row`, `plan_jit_heavy`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check`
Expected: PASS; дополнительно — прогнать `evaluate` по фикстурам P3a (`tests/plans.rs`: добавить проверку «каждая фикстура `*_analyze` даёт хотя бы одну рекомендацию из ожидаемого набора»: `seq_scan_filter_analyze` → `plan_seq_scan_filter`, `nested_loop_misestimate_analyze` → `plan_misestimate` или `plan_nested_loop_seq`, `sort_limit_analyze` → `plan_sort_limit`, `sort_spill_analyze` → `plan_spill`, `index_only_heap_fetches_analyze` → `plan_heap_fetches`, `subplan_per_row_analyze` → `plan_subplan_per_row`, `append_partitions_analyze` → `plan_no_pruning`, `parallel_jit_analyze` → `plan_workers_short` или `plan_jit_heavy`; расхождение — пересмотреть фикстуру или порог и записать причину в комментарий теста).

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor
git commit -m "feat(pg-advisor): правила выполнения — сброс на диск, Sort под Limit, heap fetches, Append без pruning, воркеры, SubPlan, JIT

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Rust — рекомендации в `ExplainResult`, проверка гипотезы hypopg

**Files:**
- Create: `src-tauri/src/explain/hypopg.rs`, `src-tauri/tests/it_hypopg.rs`
- Modify: `src-tauri/src/explain/mod.rs` (`pub mod hypopg;`, поля `ExplainResult`), `src-tauri/src/explain/run.rs` (`plan_rules::evaluate`), `src-tauri/src/commands/explain.rs` (`hypo_check`), `src-tauri/src/lib.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты `hypopg.rs`; `it_hypopg.rs`; `it_explain.rs` (поле `recommendations`)

**Interfaces:**
- Consumes: `pg_advisor::plan_rules::{evaluate, Recommendation, Report}`, `actions::conn::open_explain`, `explain::run::{explain_in_tx (сделать `pub(crate)`), classify, Step}`, `explain::{Params, validate-подобные проверки}`.
- Produces: `ExplainResult.recommendations: Vec<Recommendation>`, `ExplainResult.recommendations_skipped: Vec<String>`; `hypopg::HypoRequest { member, database, sql, params: Params, search_path: Option<String>, index_ddl }`; `hypopg::HypoResult { index_name, estimated_bytes, cost_before, cost_after, used, plan_after: PlanTree }`; `hypopg::validate_ddl(&str) -> AppResult<()>`; `hypopg::check(st, id, &HypoRequest) -> AppResult<HypoResult>`; команда `hypo_check(id, request) -> HypoResult`.

- [ ] **Step 1: Падающие тесты**

`src-tauri/src/explain/hypopg.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_must_be_a_single_create_index_without_concurrently() {
        assert!(validate_ddl("CREATE INDEX ON \"shop\".\"orders\" (\"status\")").is_ok());
        assert!(validate_ddl("  create index on t (a, b)  ").is_ok());
        assert_eq!(validate_ddl("CREATE INDEX CONCURRENTLY ON t (a)").unwrap_err().key, "errors.hypo.badDdl");
        assert_eq!(validate_ddl("CREATE INDEX ON t (a); DROP TABLE t").unwrap_err().key, "errors.hypo.badDdl");
        assert_eq!(validate_ddl("CREATE UNIQUE INDEX ON t (a)").unwrap_err().key, "errors.hypo.badDdl", "уникальность hypopg не проверяет");
        assert_eq!(validate_ddl("SELECT 1").unwrap_err().key, "errors.hypo.badDdl");
    }

    #[test]
    fn uses_index_finds_the_hypothetical_name_in_the_plan() {
        let t = pg_advisor::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Index Scan", "Index Name": "<13543>btree_orders_status", "Relation Name": "orders", "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 1, "Plan Width": 4}})).unwrap();
        assert!(uses_index(&t, "<13543>btree_orders_status"));
        assert!(!uses_index(&t, "orders_pkey"));
    }
}
```

`src-tauri/tests/it_hypopg.rs`:

```rust
//! hypopg на pg-perf (`GPG_IT_PERF`): индекс-гипотеза снижает стоимость и используется планом; сессия после проверки
//! чиста; без расширения — FEATURE_UNAVAILABLE. Запускать с `--test-threads=1`.

mod common;

use glafira_pg_lib::error::ErrorCode;
use glafira_pg_lib::explain::hypopg::{self, HypoRequest};
use glafira_pg_lib::explain::Params;

#[tokio::test]
async fn hypothetical_index_changes_the_plan_and_leaves_no_trace() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let m = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let req = HypoRequest {
        member: m.clone(),
        database: "perf".into(),
        sql: "SELECT count(*) FROM shop.orders WHERE status = $1".into(),
        params: Params::Values { values: vec![Some("disputed".into())] },
        search_path: None,
        index_ddl: "CREATE INDEX ON shop.orders (status)".into(),
    };
    let r = hypopg::check(&st, "it", &req).await.unwrap();
    assert!(r.used, "план после гипотезы использует индекс: {}", r.plan_after.root.node_type);
    assert!(r.cost_after < r.cost_before, "{} < {}", r.cost_after, r.cost_before);
    assert!(r.estimated_bytes > 0 && r.index_name.contains("orders"));
    // Сессия чиста: виртуальных индексов в новом соединении нет (они живут в сессии, а она закрыта).
    let su = common::superuser(&perf, "perf").await;
    let n: i64 = su.query_one("SELECT count(*) FROM hypopg_list_indexes", &[]).await.unwrap().get(0);
    assert_eq!(n, 0);
    let bad = HypoRequest { index_ddl: "CREATE INDEX CONCURRENTLY ON shop.orders (status)".into(), ..req.clone() };
    assert_eq!(hypopg::check(&st, "it", &bad).await.unwrap_err().code, ErrorCode::InvalidInput);
    // База без hypopg (postgres) — недоступно.
    let none = HypoRequest { database: "postgres".into(), sql: "SELECT 1".into(), params: Params::None, ..req };
    let e = hypopg::check(&st, "it", &none).await.unwrap_err();
    assert_eq!((e.code, e.key.as_str()), (ErrorCode::FeatureUnavailable, "errors.hypo.notInstalled"));
}
```

В `it_explain.rs` (`plan_and_analyze_read_tier_on_perf`) после ANALYZE SELECT по `shop.orders WHERE status = $1` добавить: ANALYZE этого запроса → `r.recommendations.iter().any(|x| x.id == "plan_seq_scan_filter")` и `r.recommendations_skipped.is_empty()`; у пробного плана (без ANALYZE) — `recommendations_skipped.len() == 9`.

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib explain::hypopg`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`src-tauri/src/explain/mod.rs`: `pub mod hypopg;`; в `ExplainResult` добавить

```rust
    /// Рекомендации по плану (P3b, `pg_advisor::plan_rules`).
    pub recommendations: Vec<pg_advisor::plan_rules::Recommendation>,
    /// Правила «факт», пропущенные без ANALYZE.
    pub recommendations_skipped: Vec<String>,
```

`src-tauri/src/explain/run.rs` — в `result(...)`:

```rust
    let report = pg_advisor::plan_rules::evaluate(&tree);
    ExplainResult { mode, tier, plan: tree, raw, relations, elapsed_ms: …, warnings, recommendations: report.recommendations, recommendations_skipped: report.skipped }
```

(`relations(&tree)` считать до перемещения `tree`.) `explain_in_tx`, `classify`, `Step` — сделать `pub(crate)`.

`src-tauri/src/explain/hypopg.rs`:

```rust
//! Проверка гипотезы hypopg (спец. P3 §9.2): виртуальный индекс в сессии, план до и после, `hypopg_reset()`.
//! Всё — в одной транзакции `READ ONLY` на соединении EXPLAIN; на сервере не остаётся ничего.

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::{PlanNode, PlanTree};
use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::explain::params::{normalize, valid_search_path};
use crate::explain::run::{Step, classify, explain_in_tx};
use crate::explain::{PROBE_TIMEOUT_S, Params};
use crate::state::AppState;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoRequest {
    pub member: String,
    pub database: String,
    pub sql: String,
    pub params: Params,
    #[serde(default)]
    pub search_path: Option<String>,
    pub index_ddl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoResult {
    pub index_name: String,
    pub estimated_bytes: i64,
    pub cost_before: f64,
    pub cost_after: f64,
    pub used: bool,
    pub plan_after: PlanTree,
}

fn bad_ddl() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.hypo.badDdl",
        "The hypothesis must be a single CREATE INDEX ON … statement without CONCURRENTLY or UNIQUE"
    )
}

/// `CREATE INDEX ON …`: одна инструкция, без `CONCURRENTLY` и `UNIQUE` (hypopg их не принимает / не проверяет).
pub fn validate_ddl(ddl: &str) -> AppResult<()> {
    let s = normalize(ddl);
    let upper = s.to_ascii_uppercase();
    let words: Vec<&str> = upper.split_whitespace().collect();
    if words.len() < 4 || words[0] != "CREATE" || words[1] != "INDEX" || words[2] != "ON" || s.contains(';') {
        return Err(bad_ddl());
    }
    Ok(())
}

/// В дереве есть узел с именем индекса гипотезы (`<oid>btree_…`).
pub fn uses_index(t: &PlanTree, name: &str) -> bool {
    fn walk(n: &PlanNode, name: &str) -> bool {
        n.index_name.as_deref() == Some(name) || n.children.iter().any(|c| walk(c, name))
    }
    walk(&t.root, name)
}

const INSTALLED_SQL: &str = "SELECT 1 FROM pg_extension WHERE extname = 'hypopg'";

async fn plan_cost(c: &Client, sql: &str, params: &Params, search_path: Option<&str>) -> AppResult<(PlanTree, f64)> {
    let raw = match explain_in_tx(c, true, PROBE_TIMEOUT_S, search_path, "FORMAT JSON, VERBOSE", sql, params).await {
        Ok(v) => v,
        Err(Step::Pg(e)) => return Err(classify(&e)),
        Err(Step::App(e)) => return Err(e),
    };
    let tree = from_json(&raw).map_err(|e| AppError::internal_with(format!("plan JSON: {}", e.message)))?;
    let cost = tree.root.total_cost;
    Ok((tree, cost))
}

pub async fn check(st: &AppState, id: &str, req: &HypoRequest) -> AppResult<HypoResult> {
    validate_ddl(&req.index_ddl)?;
    let sql = normalize(&req.sql);
    if sql.is_empty() {
        return Err(app_err!(ErrorCode::InvalidInput, "errors.explain.emptySql", "Enter a query"));
    }
    if let Some(sp) = &req.search_path
        && !valid_search_path(sp)
    {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.explain.badSearchPath",
            "search_path may contain only identifiers, quotes, commas and spaces"
        ));
    }
    let client = crate::actions::conn::open_explain(st, id, &req.member, Some(&req.database)).await?;
    let c = &client.client;
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    if c.query_opt(INSTALLED_SQL, &[]).await.map_err(pg)?.is_none() {
        return Err(app_err!(
            ErrorCode::FeatureUnavailable,
            "errors.hypo.notInstalled",
            "hypopg is not installed in this database"
        ));
    }
    let sp = req.search_path.as_deref();
    // Стоимость до — отдельной транзакцией (explain_in_tx сама открывает и откатывает).
    let (_, cost_before) = plan_cost(c, &sql, &req.params, sp).await?;
    // Гипотеза живёт в сессии: индекс, его размер и план после — на том же соединении, затем hypopg_reset().
    let row = c
        .query_one("SELECT indexrelid, indexname FROM hypopg_create_index($1)", &[&normalize(&req.index_ddl)])
        .await
        .map_err(pg)?;
    let (indexrelid, index_name): (u32, String) = (row.get(0), row.get(1));
    let size: i64 = c
        .query_one("SELECT hypopg_relation_size($1)", &[&indexrelid])
        .await
        .map_err(pg)?
        .get(0);
    let after = plan_cost(c, &sql, &req.params, sp).await;
    let _ = c.batch_execute("SELECT hypopg_reset()").await;
    let (plan_after, cost_after) = after?;
    Ok(HypoResult {
        used: uses_index(&plan_after, &index_name),
        index_name,
        estimated_bytes: size,
        cost_before,
        cost_after,
        plan_after,
    })
}
```

(`hypopg_create_index` и `hypopg_relation_size` вызываются вне транзакции `READ ONLY` — виртуальный индекс создаётся в памяти сессии и не пишет на диск; `explain_in_tx` после него открывает `BEGIN READ ONLY` и видит индекс. `indexrelid` — `oid` → `u32`.)

`src-tauri/src/commands/explain.rs`:

```rust
use crate::explain::hypopg::{self, HypoRequest, HypoResult};

/// Проверка гипотезы hypopg: стоимость до и после, использован ли индекс, размер.
#[tauri::command]
pub async fn hypo_check(state: State<'_, AppState>, id: String, request: HypoRequest) -> AppResult<HypoResult> {
    hypopg::check(&state, &id, &request).await
}
```

`lib.rs` — `commands::explain::hypo_check,`. Словари `errors.hypo`: `badDdl` "The hypothesis must be a single CREATE INDEX ON … statement without CONCURRENTLY or UNIQUE" / "Гипотеза — одна инструкция CREATE INDEX ON … без CONCURRENTLY и UNIQUE"; `notInstalled` "hypopg is not installed in this database" / "hypopg не установлен в этой базе".

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS.

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_hypopg --test it_explain -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src/locales
git commit -m "feat(explain): рекомендации в результате EXPLAIN и проверка гипотезы hypopg — стоимость до/после, использование индекса, размер

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Фронтенд — блок рекомендаций и «Проверить гипотезу»

**Files:**
- Create: `src/components/queries/plan/Recommendations.tsx`, `src/components/queries/plan/HypoCheck.tsx`, `src/components/queries/plan/recommendText.ts`, `src/components/queries/plan/Recommendations.test.tsx`
- Modify: `src/api/types.ts` (`Recommendation`, `HypoIndex`, `HypoRequest`, `HypoResult`, поля `ExplainResult`), `src/api/client.ts` (`hypoCheck`), `src/components/queries/plan/PlanRunner.tsx` (блок под сводкой, `onShowNode`), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: `Recommendations.test.tsx`

**Interfaces:**
- Produces: типы `Recommendation { id; severity; nodeIds: number[]; params: Record<string, string | number | boolean | null>; command: string | null; hypo: { indexDdl: string } | null }`, `HypoRequest`, `HypoResult`; `api.hypoCheck(id, request)`; `Recommendations({ connectionId, member, database, sql, params, searchPath, result, onShowNode })`; `HypoCheck({ connectionId, request, onClose })`; `recommendText.ts`: `recommendParams(r)` (форматирование по суффиксам как у находок + `table`, `columns`, `keys`), `recommendTitle/evidence/effect/risks(r)`.

- [ ] **Step 1: Падающие тесты**

```tsx
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ExplainResult, Recommendation } from "../../../api/types";
import { planTree } from "../../../test/plans";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { hypoCheck: vi.fn() } };
});

import { api } from "../../../api/client";
import { Recommendations } from "./Recommendations";

const rec: Recommendation = {
  id: "plan_seq_scan_filter", severity: "warning", nodeIds: [1],
  params: { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" },
  command: "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");", hypo: { indexDdl: "CREATE INDEX ON \"shop\".\"orders\" (\"status\")" },
};
const result = (patch: Partial<ExplainResult> = {}): ExplainResult => ({ mode: "analyze", tier: "read", plan: planTree(), raw: [], relations: [], elapsedMs: 1, warnings: [], recommendations: [rec], recommendationsSkipped: [], ...patch });

describe("Recommendations", () => {
  beforeEach(() => vi.resetAllMocks());

  it("карточка рекомендации: заголовок с цифрами, доказательство, команда, эффект и риски, «Показать узел»", async () => {
    const onShowNode = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result()} onShowNode={onShowNode} />);
    expect(screen.getByRole("heading", { name: "Индекс по колонкам фильтра: shop.orders" })).toBeInTheDocument();
    expect(screen.getByText(/Seq Scan прочитал 500 000 строк, вернул 500/)).toBeInTheDocument();
    expect(screen.getByText(rec.command!)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать команду" })).toBeInTheDocument();
    expect(screen.getByText(/Риски/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Показать узел" }));
    expect(onShowNode).toHaveBeenCalledWith(1);
  });

  it("«Проверить гипотезу»: запрос с DDL, стоимость была/стала и использование индекса", async () => {
    vi.mocked(api.hypoCheck).mockResolvedValue({ indexName: "<1>btree_orders_status", estimatedBytes: 11_000_000, costBefore: 12_000, costAfter: 120.5, used: true, planAfter: planTree() });
    const user = userEvent.setup();
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result()} onShowNode={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Проверить гипотезу" }));
    await waitFor(() => expect(api.hypoCheck).toHaveBeenCalledWith("c", { member: "pg-1", database: "perf", sql: "SELECT 1", params: { kind: "none" }, searchPath: null, indexDdl: rec.hypo!.indexDdl }));
    const dialog = await screen.findByRole("dialog", { name: "Проверка гипотезы" });
    expect(within(dialog).getByText(/Стоимость: 12 000 → 120,5/)).toBeInTheDocument();
    expect(within(dialog).getByText("Индекс использован")).toBeInTheDocument();
    expect(within(dialog).getByText(/10,5 MB/)).toBeInTheDocument();
    expect(within(dialog).getByRole("treegrid")).toBeInTheDocument();
  });

  it("без ANALYZE — подсказка о пропущенных правилах; без рекомендаций — «Советов нет»", () => {
    renderWithClient(<Recommendations connectionId="c" member="pg-1" database="perf" sql="SELECT 1" params={{ kind: "none" }} searchPath={null} result={result({ recommendations: [], recommendationsSkipped: ["plan_spill", "plan_jit_heavy"] })} onShowNode={() => {}} />);
    expect(screen.getByText("Советов нет")).toBeInTheDocument();
    expect(screen.getByText("Выполните план с выполнением, чтобы проверить ещё 2 правила")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- Recommendations`
Expected: FAIL.

- [ ] **Step 3: Типы, клиент, тексты**

`src/api/types.ts`:

```ts
export interface HypoIndex { indexDdl: string }
export interface Recommendation {
  /** Ключ текстов `recommend.<id>.{title,evidence,effect,risks}`. */
  id: string; severity: Severity; nodeIds: number[];
  params: Record<string, string | number | boolean | null>; command: string | null; hypo: HypoIndex | null;
}
export interface HypoRequest { member: string; database: string; sql: string; params: ExplainParams; searchPath: string | null; indexDdl: string }
export interface HypoResult { indexName: string; estimatedBytes: number; costBefore: number; costAfter: number; used: boolean; planAfter: PlanTree }
```

и в `ExplainResult`: `recommendations: Recommendation[]; recommendationsSkipped: string[];`. `client.ts`: `hypoCheck: (id: string, request: HypoRequest) => call<HypoResult>("hypo_check", { id, request }),`. Фикстуры тестов P3a, строящие `ExplainResult` (`PlanRunner.test.tsx`, `QueryTab.test.tsx`, `ExplainTab.test.tsx`), получают `recommendations: [], recommendationsSkipped: []`.

`src/components/queries/plan/recommendText.ts`:

```ts
import type { Recommendation } from "../../../api/types";
import { i18n } from "../../../i18n";
import { formatBytes, formatCount, formatMs, formatPercent } from "../../../lib/format";

/** Параметры текста: числа по суффиксам (как у находок), `Kb` — килобайты как байты, остальное как есть. */
export function recommendParams(r: Recommendation): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(r.params)) {
    if (v === null) continue;
    if (typeof v === "number" && k.endsWith("Bytes")) out[k] = formatBytes(v);
    else if (typeof v === "number" && k.endsWith("Kb")) out[k] = formatBytes(v * 1024);
    else if (typeof v === "number" && k.endsWith("Pct")) out[k] = formatPercent(v);
    else if (typeof v === "number" && k.endsWith("Ms")) out[k] = formatMs(v);
    else if (typeof v === "number" && k === "ratio") out[k] = formatCount(v, v < 10 ? 1 : 0);
    else if (typeof v === "number") out[k] = formatCount(v);
    else out[k] = String(v);
  }
  if (typeof r.params.direction === "string") out.directionLabel = i18n.t(`recommend.direction.${r.params.direction}`);
  if (typeof r.params.kind === "string") out.kindLabel = i18n.t(`recommend.spillKinds.${r.params.kind}`);
  return out;
}

export const recommendTitle = (r: Recommendation) => i18n.t(`recommend.${r.id}.title`, recommendParams(r));
export const recommendEvidence = (r: Recommendation) => i18n.t(`recommend.${r.id}.evidence`, recommendParams(r));
export const recommendEffect = (r: Recommendation) => i18n.t(`recommend.${r.id}.effect`, recommendParams(r));
export const recommendRisks = (r: Recommendation) => i18n.t(`recommend.${r.id}.risks`, recommendParams(r));
```

- [ ] **Step 4: Компоненты**

`src/components/queries/plan/HypoCheck.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { HypoRequest } from "../../../api/types";
import { formatBytes, formatCount } from "../../../lib/format";
import { ErrorBanner } from "../../common/ErrorBanner";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanTreeView } from "./PlanTree";

/** Окно проверки гипотезы (спец. P3 §9.2): стоимость «была / стала», использован ли индекс, размер, план «после». */
export function HypoCheck({ connectionId, request, onClose }: { connectionId: string; request: HypoRequest; onClose: () => void }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["hypo", connectionId, request], queryFn: () => api.hypoCheck(connectionId, request), retry: false, staleTime: Infinity });
  const [selected, setSelected] = useState<number | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.focus();
  }, []);
  const r = q.data;
  const node = r && selected !== null ? find(r.planAfter.root, selected) : null;
  return (
    <div className="modal-backdrop">
      <div ref={ref} role="dialog" aria-modal="true" aria-label={t("recommend.hypo.title")} className="modal hypo-check" tabIndex={-1} onKeyDown={(e) => e.key === "Escape" && onClose()}>
        <h3>{t("recommend.hypo.title")}</h3>
        <pre className="query-text"><code>{request.indexDdl}</code></pre>
        {q.isLoading && <p className="muted">{t("common.loading")}</p>}
        {q.error && <ErrorBanner error={q.error} />}
        {r && (
          <>
            <p>
              {t("recommend.hypo.cost", { before: formatCount(r.costBefore, r.costBefore < 100 ? 1 : 0), after: formatCount(r.costAfter, r.costAfter < 100 ? 1 : 0) })}
              {" · "}
              <span className={r.used ? "ok" : "diag-warn"}>{r.used ? t("recommend.hypo.used") : t("recommend.hypo.notUsed")}</span>
              {" · "}
              {t("recommend.hypo.size", { size: formatBytes(r.estimatedBytes) })}
            </p>
            <PlanTreeView tree={r.planAfter} selectedId={selected} onSelect={setSelected} />
            {node && <PlanNodeDetails node={node} />}
          </>
        )}
        <div className="toolbar">
          <button type="button" onClick={onClose}>{t("common.close")}</button>
        </div>
      </div>
    </div>
  );
}

function find(n: import("../../../api/types").PlanNode, id: number): import("../../../api/types").PlanNode | null {
  if (n.id === id) return n;
  for (const c of n.children) {
    const f = find(c, id);
    if (f) return f;
  }
  return null;
}
```

`src/components/queries/plan/Recommendations.tsx`:

```tsx
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainParams, ExplainResult, HypoRequest, Recommendation } from "../../../api/types";
import { CopyButton } from "../../common/CopyButton";
import { HypoCheck } from "./HypoCheck";
import { recommendEffect, recommendEvidence, recommendRisks, recommendTitle } from "./recommendText";

const SEVERITY_ORDER = { critical: 0, warning: 1, info: 2 } as const;

/**
 * Блок рекомендаций под сводкой плана (спец. P3 §9.1): карточка на рекомендацию — заголовок, доказательство, команда
 * с копированием, эффект, риски, «Проверить гипотезу» (hypopg) и «Показать узел». Команды приложение не выполняет.
 */
export function Recommendations({ connectionId, member, database, sql, params, searchPath, result, onShowNode }: { connectionId: string; member: string; database: string; sql: string; params: ExplainParams; searchPath: string | null; result: ExplainResult; onShowNode: (id: number) => void }) {
  const { t } = useTranslation();
  const [hypo, setHypo] = useState<HypoRequest | null>(null);
  const recs = [...result.recommendations].sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity]);
  const check = (r: Recommendation) => r.hypo && setHypo({ member, database, sql, params: params.kind === "generic" ? { kind: "generic" } : params, searchPath, indexDdl: r.hypo.indexDdl });
  return (
    <section className="recommendations" aria-label={t("recommend.title")}>
      <h3>{t("recommend.title")}{recs.length > 0 && <span className="muted"> ({recs.length})</span>}</h3>
      {recs.length === 0 && <p className="muted">{t("recommend.none")}</p>}
      {result.recommendationsSkipped.length > 0 && <p className="diag-note">{t("recommend.skipped", { count: result.recommendationsSkipped.length })}</p>}
      <ul className="recommend-list">
        {recs.map((r, i) => (
          <li key={`${r.id}:${r.nodeIds.join(",")}:${i}`} className={`recommend sev-${r.severity}`}>
            <h4>{recommendTitle(r)}</h4>
            <p className="recommend-evidence">{recommendEvidence(r)}</p>
            {r.command && (
              <div className="diag-command">
                <pre><code>{r.command}</code></pre>
                <CopyButton text={r.command} label={t("recommend.copyCommand")} />
              </div>
            )}
            <p><strong>{t("recommend.effect")}:</strong> {recommendEffect(r)}</p>
            <p className="muted"><strong>{t("recommend.risks")}:</strong> {recommendRisks(r)}</p>
            <div className="toolbar">
              {r.hypo && <button type="button" onClick={() => check(r)}>{t("recommend.hypo.check")}</button>}
              <button type="button" className="link" onClick={() => onShowNode(r.nodeIds[0])}>{t("recommend.showNode")}</button>
            </div>
          </li>
        ))}
      </ul>
      {hypo && <HypoCheck connectionId={connectionId} request={hypo} onClose={() => setHypo(null)} />}
    </section>
  );
}
```

`PlanRunner.tsx`: после `<PlanSummary …/>` — `<Recommendations connectionId={connectionId} member={member} database={database} sql={sql} params={params(current.result.mode)} searchPath={searchPath.trim() || null} result={current.result} onShowNode={setSelected} />` (для `mode: "analyze"` параметры — значения; `params()` уже это даёт).

CSS: `.recommend-list { list-style: none; padding: 0; } .recommend { border-left: 3px solid var(--border); padding: 6px 10px; margin: 8px 0; } .recommend.sev-warning { border-color: var(--warn, #c77700); } .recommend.sev-info { border-color: var(--accent, #4f8cff); } .recommend h4 { margin: 0 0 4px; font-size: 13px; } .hypo-check { max-width: 900px; max-height: 80vh; overflow: auto; } .ok { color: var(--ok, #2a9d4a); }`.

Словари (en / ru) `recommend`: `title` "Recommendations"/"Рекомендации"; `none` "No advice"/"Советов нет"; `skipped` "Run the plan with execution to check {{count}} more rules"/"Выполните план с выполнением, чтобы проверить ещё {{count}} правила"; `copyCommand` "Copy command"/"Копировать команду"; `effect` "Effect"/"Эффект"; `risks` "Risks"/"Риски"; `showNode` "Show node"/"Показать узел"; `direction.under` "underestimated"/"занижена", `direction.over` "overestimated"/"завышена"; `spillKinds.sort` "sort"/"сортировка", `spillKinds.hash` "hash"/"хеш", `spillKinds.bitmap` "bitmap"/"bitmap"; `hypo`: `title` "Hypothesis check"/"Проверка гипотезы", `check` "Check hypothesis"/"Проверить гипотезу", `cost` "Cost: {{before}} → {{after}}"/"Стоимость: {{before}} → {{after}}", `used` "Index used"/"Индекс использован", `notUsed` "Index not used"/"Индекс не использован", `size` "estimated size {{size}}"/"оценка размера {{size}}".
Правила (en / ru; `{{…}}` — параметры):
- `plan_seq_scan_filter`: title "Index on filter columns: {{table}}"/"Индекс по колонкам фильтра: {{table}}"; evidence "Seq Scan read {{rowsRead}} rows and returned {{rowsReturned}} ({{loops}} loops)"/"Seq Scan прочитал {{rowsRead}} строк, вернул {{rowsReturned}} ({{loops}} циклов)"; effect "An index turns the full scan into an index scan of the matching rows"/"Индекс превращает полное сканирование в чтение подходящих строк"; risks "The index costs disk space and slows writes; CONCURRENTLY avoids blocking but takes longer"/"Индекс занимает место и замедляет запись; CONCURRENTLY не блокирует таблицу, но строится дольше".
- `plan_index_filter`: title "Composite index for {{table}}"/"Составной индекс для {{table}}"; evidence "{{index}} returned {{rowsReturned}} rows after removing {{rowsRemoved}} by filter"/"{{index}} вернул {{rowsReturned}} строк, отсеяв фильтром {{rowsRemoved}}"; effect "Adding the filter columns to the index avoids reading rows that are discarded"/"Колонки фильтра в индексе избавляют от чтения отбрасываемых строк"; risks "A wider index is larger; put equality columns first"/"Широкий индекс больше; колонки равенства — первыми".
- `plan_nested_loop_seq`: title "Index on the join key: {{table}}"/"Индекс по ключу соединения: {{table}}"; evidence "Seq Scan on {{table}} ran {{loops}} times, {{rowsPerLoop}} rows per loop"/"Seq Scan по {{table}} выполнен {{loops}} раз, по {{rowsPerLoop}} строк за цикл"; effect "The inner side becomes an index lookup per outer row"/"Внутренняя сторона становится поиском по индексу на каждую внешнюю строку"; risks "Check that the planner's row estimate is right: a wrong estimate may still choose a nested loop"/"Проверьте оценку строк планировщика: при ошибке он может снова выбрать Nested Loop".
- `plan_misestimate`: title "Row estimate {{directionLabel}} ×{{ratio}}: {{table}}"/"Оценка строк {{directionLabel}} в {{ratio}} раз: {{table}}"; evidence "Planned {{planRows}} rows, got {{actualRows}}"/"Оценка {{planRows}} строк, факт {{actualRows}}"; effect "Fresh statistics (ANALYZE, higher statistics target, extended statistics for correlated columns) fix the estimate and the join strategy"/"Свежая статистика (ANALYZE, больший statistics target, расширенная статистика для связанных колонок) исправит оценку и выбор соединения"; risks "ANALYZE reads a sample of the table; extended statistics need PostgreSQL 10+ and are refreshed by ANALYZE"/"ANALYZE читает выборку таблицы; расширенная статистика обновляется ANALYZE".
- `plan_spill`: title "{{kindLabel}} spills to disk"/"{{kindLabel}} уходит на диск"; evidence "{{spaceKb}} used on disk; work_mem for the session or role: {{workMemKb}}"/"На диске {{spaceKb}}; work_mem для сессии или роли: {{workMemKb}}"; effect "Enough work_mem keeps the sort or hash in memory; an index on {{keys}} removes the sort"/"Достаточный work_mem держит сортировку или хеш в памяти; индекс по {{keys}} убирает сортировку"; risks "work_mem is per operation and per session: raise it for the session or role, not globally"/"work_mem выделяется на операцию и сессию: поднимайте для сессии или роли, не глобально".
- `plan_sort_limit`: title "Index for ORDER BY … LIMIT: {{table}}"/"Индекс под ORDER BY … LIMIT: {{table}}"; evidence "{{inputRows}} rows sorted to return {{limitRows}}"/"Отсортировано {{inputRows}} строк ради {{limitRows}}"; effect "An index on {{keys}} lets the server read the first rows without sorting everything"/"Индекс по {{keys}} даёт серверу прочитать первые строки без полной сортировки"; risks "The index must match the sort order and direction"/"Индекс должен совпадать с порядком и направлением сортировки".
- `plan_heap_fetches`: title "VACUUM {{table}}: visibility map is stale"/"VACUUM {{table}}: карта видимости устарела"; evidence "Index Only Scan fetched {{heapFetches}} of {{rows}} rows from the heap"/"Index Only Scan сходил в heap за {{heapFetches}} из {{rows}} строк"; effect "VACUUM marks pages all-visible and the scan stops touching the heap"/"VACUUM пометит страницы видимыми, и скан перестанет читать heap"; risks "Check autovacuum settings for the table"/"Проверьте настройки автовакуума таблицы".
- `plan_no_pruning`: title "Partition pruning did not work"/"Отсечение партиций не сработало"; evidence "{{children}} partitions scanned, {{emptyChildren}} returned nothing"/"Просканировано {{children}} партиций, {{emptyChildren}} ничего не вернули"; effect "A condition on the partition key lets the planner skip partitions"/"Условие по ключу партиционирования позволит планировщику пропустить партиции"; risks "Pruning needs a constant or a parameter on the key, not an expression; check enable_partition_pruning"/"Отсечение требует константы или параметра по ключу, не выражения; проверьте enable_partition_pruning".
- `plan_workers_short`: title "Fewer parallel workers than planned"/"Воркеров меньше, чем запланировано"; evidence "{{launched}} of {{planned}} workers launched"/"Запущено {{launched}} из {{planned}} воркеров"; effect "Raising max_parallel_workers and max_worker_processes gives the query its planned parallelism"/"Повышение max_parallel_workers и max_worker_processes даст запросу запланированный параллелизм"; risks "Workers are shared by all sessions; more workers mean more CPU and memory"/"Воркеры общие для всех сессий; больше воркеров — больше CPU и памяти".
- `plan_subplan_per_row`: title "Subquery runs per row"/"Подзапрос выполняется на каждую строку"; evidence "{{name}} executed {{loops}} times"/"{{name}} выполнен {{loops}} раз"; effect "Rewriting as JOIN or LATERAL lets the planner choose a hash or merge strategy"/"Переписанный как JOIN или LATERAL подзапрос даст планировщику выбрать хеш или merge"; risks "The rewrite must keep NULL semantics (NOT IN vs NOT EXISTS)"/"При переписывании сохраните семантику NULL (NOT IN против NOT EXISTS)".
- `plan_jit_heavy`: title "JIT takes longer than the query"/"JIT дольше самого запроса"; evidence "JIT {{jitMs}} ({{functions}} functions) against execution {{execMs}}"/"JIT {{jitMs}} ({{functions}} функций) против выполнения {{execMs}}"; effect "jit = off for the session or role, or a higher jit_above_cost, removes the compilation cost"/"jit = off для сессии или роли либо больший jit_above_cost убирает стоимость компиляции"; risks "Long analytical queries may benefit from JIT: disable it selectively"/"Долгим аналитическим запросам JIT помогает: отключайте избирательно".
`common.close` — если нет: "Close"/"Закрыть".

- [ ] **Step 5: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): блок рекомендаций по плану с командами и «Показать узел», проверка гипотезы hypopg

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: `pg-advisor::plan::from_text` — текстовые планы psql и auto_explain

**Files:**
- Create: `crates/pg-advisor/src/plan/text_in.rs`, `crates/pg-advisor/tests/fixtures/plans/text/*.txt`
- Modify: `crates/pg-advisor/src/plan/mod.rs` (`pub mod text_in;`), `crates/pg-advisor/src/plan/json.rs` (`from_any`), `crates/pg-advisor/tests/plans.rs` (пары), `src-tauri/src/commands/explain.rs` (`parse_plan` → `from_any`), `src/components/queries/ExplainTab.tsx` и словари (подпись «План (JSON или текст)»)
- Test: unit-тесты `text_in.rs`; `tests/plans.rs`

**Interfaces:**
- Produces: `plan::text_in::from_text(&str) -> Result<PlanTree, PlanError>`; `plan::json::from_any(&str) -> Result<PlanTree, PlanError>` (JSON — через `from_psql_text`, иначе текст).

- [ ] **Step 1: Падающие тесты**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
                                                                QUERY PLAN
------------------------------------------------------------------------------------------------------------------------------
 Hash Join  (cost=10.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)
   Hash Cond: (o.customer_id = c.id)
   Buffers: shared hit=100 read=20
   ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)
         Filter: (o.status = 'disputed'::text)
         Rows Removed by Filter: 499995
   ->  Hash  (cost=5.00..5.00 rows=100 width=8) (actual time=0.500..0.500 rows=100 loops=1)
         Buckets: 1024  Batches: 1  Memory Usage: 12kB
         ->  Index Scan using customers_pkey on shop.customers c  (cost=0.30..4.00 rows=100 width=8) (never executed)
               Index Cond: (c.id < 100)
 Planning Time: 0.500 ms
 JIT:
   Functions: 3
   Timing: Generation 1.000 ms, Inlining 0.000 ms, Optimization 2.000 ms, Emission 3.000 ms, Total 6.000 ms
 Execution Time: 55.000 ms
(16 rows)
";

    #[test]
    fn parses_psql_text_into_the_same_tree() {
        let t = from_text(TEXT).unwrap();
        assert_eq!((t.planning_ms, t.execution_ms), (Some(0.5), Some(55.0)));
        assert_eq!(t.jit.as_ref().map(|j| (j.functions, j.total_ms)), Some((3, 6.0)));
        let root = &t.root;
        assert_eq!((root.node_type.as_str(), root.total_cost, root.plan_rows), ("Hash Join", 100.0, 1000.0));
        assert_eq!(root.hash_cond.as_deref(), Some("(o.customer_id = c.id)"));
        assert_eq!(root.buffers.map(|b| (b.shared_hit, b.shared_read)), Some((100, 20)));
        assert_eq!(root.actual.map(|a| (a.total_ms, a.rows, a.loops)), Some((50.0, 10.0, 1)));
        let seq = &root.children[0];
        assert_eq!(seq.relation.as_ref().map(|r| (r.schema.as_deref(), r.name.as_str(), r.alias.as_deref())), Some((Some("shop"), "orders", Some("o"))));
        assert_eq!((seq.rows_removed_by_filter, seq.filter.as_deref()), (Some(499_995), Some("(o.status = 'disputed'::text)")));
        let hash = &root.children[1];
        assert_eq!((hash.node_type.as_str(), hash.hash_batches, hash.peak_memory_kb), ("Hash", Some(1), Some(12)));
        let idx = &hash.children[0];
        assert_eq!((idx.node_type.as_str(), idx.index_name.as_deref(), idx.never_executed), ("Index Scan", Some("customers_pkey"), true));
        assert_eq!(idx.index_cond.as_deref(), Some("(c.id < 100)"));
        assert!(seq.hot && seq.misestimated, "вычисляемые поля те же, что у JSON");
    }

    #[test]
    fn handles_parallel_sort_and_unknown_lines() {
        let text = "\
Limit  (cost=1.0..2.0 rows=10 width=8) (actual time=5.000..5.010 rows=10 loops=1)
  ->  Gather Merge  (cost=1.0..2.0 rows=100 width=8) (actual time=5.000..5.005 rows=10 loops=1)
        Workers Planned: 2
        Workers Launched: 1
        ->  Sort  (cost=1.0..2.0 rows=50 width=8) (actual time=4.000..4.001 rows=10 loops=2)
              Sort Key: o.total DESC
              Sort Method: top-N heapsort  Memory: 25kB
              Worker 0:  Sort Method: top-N heapsort  Memory: 25kB
              Mystery: something new
              ->  Parallel Seq Scan on orders o  (cost=0.0..1.0 rows=25 width=8) (actual time=0.010..3.000 rows=250000 loops=2)
Planning Time: 0.100 ms
Execution Time: 6.000 ms
";
        let t = from_text(text).unwrap();
        let gm = &t.root.children[0];
        assert_eq!((gm.workers_planned, gm.workers_launched), (Some(2), Some(1)));
        let sort = &gm.children[0];
        assert_eq!(sort.sort_key, ["o.total DESC"]);
        assert_eq!((sort.sort_method.as_deref(), sort.sort_space_kb, sort.sort_space_type.as_deref()), (Some("top-N heapsort"), Some(25), Some("Memory")));
        assert_eq!(sort.extra["text"], serde_json::json!(["Worker 0:  Sort Method: top-N heapsort  Memory: 25kB", "Mystery: something new"]));
        let scan = &sort.children[0];
        assert_eq!((scan.node_type.as_str(), scan.parallel_aware), ("Seq Scan", true));
        assert_eq!(scan.relation.as_ref().map(|r| (r.schema.as_deref(), r.name.as_str())), Some((None, "orders")));
    }

    #[test]
    fn plain_explain_without_actual_and_auto_explain_prefix() {
        let text = "2026-10-29 10:00:00 UTC [123] LOG:  duration: 12.345 ms  plan:\n\tQuery Text: SELECT 1\n\tResult  (cost=0.00..0.01 rows=1 width=4)\n";
        let t = from_text(text).unwrap();
        assert_eq!((t.root.node_type.as_str(), t.root.actual), ("Result", None));
        assert_eq!(t.root.extra["queryText"], "SELECT 1");
        let e = from_text("no plan here").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn from_any_dispatches() {
        assert_eq!(crate::plan::json::from_any("[{\"Plan\": {\"Node Type\": \"Result\", \"Startup Cost\": 0, \"Total Cost\": 0, \"Plan Rows\": 1, \"Plan Width\": 1}}]").unwrap().root.node_type, "Result");
        assert_eq!(crate::plan::json::from_any("Result  (cost=0.00..0.01 rows=1 width=4)\n").unwrap().root.node_type, "Result");
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml plan::text_in`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`crates/pg-advisor/src/plan/mod.rs` — `pub mod text_in;`. `crates/pg-advisor/src/plan/json.rs`:

```rust
/// Вставленный план любого вида: JSON (в том числе с обрамлением psql) или текст EXPLAIN.
pub fn from_any(text: &str) -> Result<PlanTree, PlanError> {
    let body: String = text.lines().map(|l| l.trim_matches('|').trim()).filter(|l| !l.is_empty() && *l != "QUERY PLAN").collect::<Vec<_>>().join("\n");
    let first = body.lines().find(|l| !l.chars().all(|c| c == '-' || c == '+')).unwrap_or("");
    if first.starts_with('[') || first.starts_with('{') {
        from_json(&from_psql_text(text)?)
    } else {
        crate::plan::text_in::from_text(text)
    }
}
```

`crates/pg-advisor/src/plan/text_in.rs`:

```rust
//! Текстовый план psql / auto_explain (спец. P3 §9.4) → `PlanTree`. Дерево — по отступам и `->`; заголовок узла —
//! `Тип [using idx] on [schema.]rel [alias]  (cost=a..b rows=r width=w) [(actual time=a..b rows=r loops=l) | (never executed)]`;
//! строки деталей известных видов разбираются, прочие — в `extra["text"]`.

use serde_json::{Map, Value};

use crate::plan::{Actual, Buffers, Jit, PlanError, PlanNode, PlanTree, Relation, Setting, Trigger};

fn err(message: &str, line: usize) -> PlanError {
    PlanError { message: message.to_string(), line: Some(line) }
}

/// Строка заголовка узла: отступ (позиция `->` или первого символа), текст без стрелки.
struct Head {
    indent: usize,
    text: String,
    line: usize,
}

fn is_head(s: &str) -> bool {
    s.contains("(cost=") || s.contains("(actual ")
}

/// Снимает обрамление psql, префикс auto_explain («… plan:» и «Query Text: …») и табуляции; нумерует строки.
fn clean(text: &str) -> (Vec<(usize, String)>, Option<String>) {
    let mut out = Vec::new();
    let mut query_text = None;
    let mut started = false;
    for (i, raw) in text.lines().enumerate() {
        let l = raw.replace('\t', "    ");
        let t = l.trim_end().trim_end_matches('+').trim_end();
        let tt = t.trim();
        if tt.is_empty() || tt == "QUERY PLAN" || tt.chars().all(|c| c == '-') {
            continue;
        }
        if tt.starts_with('(') && (tt.ends_with(" row)") || tt.ends_with(" rows)")) {
            continue;
        }
        if !started {
            if let Some(q) = tt.strip_prefix("Query Text:") {
                query_text = Some(q.trim().to_string());
                continue;
            }
            if !is_head(tt) {
                continue;
            }
            started = true;
        }
        out.push((i + 1, t.trim_start_matches('|').to_string()));
    }
    (out, query_text)
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

/// `(cost=0.00..60.00 rows=500 width=8)` и факт после него.
fn parse_head(text: &str, line: usize) -> Result<PlanNode, PlanError> {
    let cost_at = text.find("(cost=").ok_or_else(|| err("node without (cost=", line))?;
    let (head, rest) = (text[..cost_at].trim(), &text[cost_at..]);
    let costs = &rest[6..rest.find(')').ok_or_else(|| err("unclosed (cost=", line))?];
    let mut it = costs.split_whitespace();
    let range = it.next().unwrap_or("");
    let (startup, total) = range.split_once("..").ok_or_else(|| err("bad cost range", line))?;
    let kv = |prefix: &str| it.clone().find_map(|p| p.strip_prefix(prefix)).and_then(num);
    let plan_rows = kv("rows=").unwrap_or(0.0);
    let plan_width = kv("width=").unwrap_or(0.0) as i64;
    let after = &rest[rest.find(')').unwrap() + 1..];
    let (actual, never_executed) = if after.contains("(never executed)") {
        (Some(Actual { startup_ms: 0.0, total_ms: 0.0, rows: 0.0, loops: 0 }), true)
    } else if let Some(a) = after.find("(actual ") {
        let inner = &after[a + 8..after[a..].find(')').map(|e| a + e).unwrap_or(after.len())];
        let mut time = None;
        let (mut rows, mut loops) = (0.0, 1u64);
        for part in inner.split_whitespace() {
            if let Some(v) = part.strip_prefix("time=") {
                let (s, e) = v.split_once("..").unwrap_or((v, v));
                time = Some((num(s).unwrap_or(0.0), num(e).unwrap_or(0.0)));
            } else if let Some(v) = part.strip_prefix("rows=") {
                rows = num(v).unwrap_or(0.0);
            } else if let Some(v) = part.strip_prefix("loops=") {
                loops = num(v).unwrap_or(1.0) as u64;
            }
        }
        let (s, e) = time.unwrap_or((0.0, 0.0));
        (Some(Actual { startup_ms: s, total_ms: e, rows, loops }), false)
    } else {
        (None, false)
    };
    // Заголовок: [Parallel ][Partial ]Тип [using idx] [on [schema.]rel [alias]]
    let mut words: Vec<&str> = head.split_whitespace().collect();
    let parallel_aware = words.first() == Some(&"Parallel");
    if parallel_aware {
        words.remove(0);
    }
    let mut strategy = None;
    if words.first() == Some(&"Partial") {
        strategy = Some("Partial".to_string());
        words.remove(0);
    }
    let on = words.iter().position(|w| *w == "on");
    let using = words.iter().position(|w| *w == "using");
    let type_end = [on, using].into_iter().flatten().min().unwrap_or(words.len());
    let node_type = words[..type_end].join(" ");
    let index_name = using.map(|u| words.get(u + 1).copied().unwrap_or("").to_string());
    let relation = on.map(|o| {
        let full = words.get(o + 1).copied().unwrap_or("");
        let (schema, name) = match full.rsplit_once('.') {
            Some((s, n)) => (Some(s.to_string()), n.to_string()),
            None => (None, full.to_string()),
        };
        let alias = words.get(o + 2).filter(|a| !a.starts_with('(')).map(|a| a.to_string());
        Relation { schema, name, alias }
    });
    Ok(PlanNode {
        id: 0,
        node_type,
        parent_relationship: None,
        subplan_name: None,
        relation,
        index_name,
        strategy,
        join_type: None,
        parallel_aware,
        startup_cost: num(startup).unwrap_or(0.0),
        total_cost: num(total).unwrap_or(0.0),
        plan_rows,
        plan_width,
        actual,
        never_executed,
        workers_planned: None,
        workers_launched: None,
        rows_removed_by_filter: None,
        rows_removed_by_index_recheck: None,
        rows_removed_by_join_filter: None,
        heap_fetches: None,
        filter: None,
        index_cond: None,
        recheck_cond: None,
        hash_cond: None,
        join_filter: None,
        merge_cond: None,
        sort_key: Vec::new(),
        group_key: Vec::new(),
        sort_method: None,
        sort_space_kb: None,
        sort_space_type: None,
        hash_batches: None,
        hash_buckets: None,
        peak_memory_kb: None,
        exact_heap_blocks: None,
        lossy_heap_blocks: None,
        subplans_removed: None,
        buffers: None,
        io_read_ms: None,
        io_write_ms: None,
        output: Vec::new(),
        extra: Map::new(),
        inclusive_ms: None,
        exclusive_ms: None,
        share_pct: None,
        estimate_ratio: None,
        hot: false,
        misestimated: false,
        children: Vec::new(),
    })
}

fn kb(s: &str) -> Option<i64> {
    s.trim().trim_end_matches("kB").trim().parse().ok()
}

/// Строка деталей узла; неизвестная — в `extra["text"]`.
fn detail(n: &mut PlanNode, line: &str) {
    let l = line.trim();
    let mut set = |key: &str, f: &mut dyn FnMut(&str)| -> bool {
        match l.strip_prefix(key) {
            Some(v) => {
                f(v.trim());
                true
            }
            None => false,
        }
    };
    if set("Filter:", &mut |v| n.filter = Some(v.to_string())) { return }
    if set("Index Cond:", &mut |v| n.index_cond = Some(v.to_string())) { return }
    if set("Recheck Cond:", &mut |v| n.recheck_cond = Some(v.to_string())) { return }
    if set("Hash Cond:", &mut |v| n.hash_cond = Some(v.to_string())) { return }
    if set("Join Filter:", &mut |v| n.join_filter = Some(v.to_string())) { return }
    if set("Merge Cond:", &mut |v| n.merge_cond = Some(v.to_string())) { return }
    if set("Rows Removed by Filter:", &mut |v| n.rows_removed_by_filter = v.parse().ok()) { return }
    if set("Rows Removed by Index Recheck:", &mut |v| n.rows_removed_by_index_recheck = v.parse().ok()) { return }
    if set("Rows Removed by Join Filter:", &mut |v| n.rows_removed_by_join_filter = v.parse().ok()) { return }
    if set("Heap Fetches:", &mut |v| n.heap_fetches = v.parse().ok()) { return }
    if set("Sort Key:", &mut |v| n.sort_key = v.split(", ").map(str::to_string).collect()) { return }
    if set("Group Key:", &mut |v| n.group_key = v.split(", ").map(str::to_string).collect()) { return }
    if set("Workers Planned:", &mut |v| n.workers_planned = v.parse().ok()) { return }
    if set("Workers Launched:", &mut |v| n.workers_launched = v.parse().ok()) { return }
    if set("Subplans Removed:", &mut |v| n.subplans_removed = v.parse().ok()) { return }
    if set("Output:", &mut |v| n.output = v.split(", ").map(str::to_string).collect()) { return }
    if let Some(v) = l.strip_prefix("Sort Method:") {
        // `top-N heapsort  Memory: 25kB` | `external merge  Disk: 2048kB`
        let (method, space) = v.split_once("  ").unwrap_or((v, ""));
        n.sort_method = Some(method.trim().to_string());
        if let Some((kind, amount)) = space.trim().split_once(':') {
            n.sort_space_type = Some(kind.trim().to_string());
            n.sort_space_kb = kb(amount);
        }
        return;
    }
    if let Some(v) = l.strip_prefix("Buckets:") {
        for part in v.split("  ") {
            let p = part.trim();
            if let Some(b) = p.strip_prefix("Batches:") {
                n.hash_batches = b.trim().parse().ok();
            } else if let Some(m) = p.strip_prefix("Memory Usage:") {
                n.peak_memory_kb = kb(m);
            } else if n.hash_buckets.is_none() {
                n.hash_buckets = p.split_whitespace().next().and_then(|x| x.parse().ok());
            }
        }
        return;
    }
    if let Some(v) = l.strip_prefix("Heap Blocks:") {
        for p in v.split_whitespace() {
            if let Some(x) = p.strip_prefix("exact=") {
                n.exact_heap_blocks = x.parse().ok();
            } else if let Some(x) = p.strip_prefix("lossy=") {
                n.lossy_heap_blocks = x.parse().ok();
            }
        }
        return;
    }
    if let Some(v) = l.strip_prefix("Buffers:") {
        let mut b = n.buffers.unwrap_or_default();
        let mut group = "";
        for p in v.split([' ', ',']).filter(|p| !p.is_empty()) {
            match p {
                "shared" | "local" | "temp" => group = p,
                _ => {
                    if let Some((k, val)) = p.split_once('=') {
                        let val: i64 = val.parse().unwrap_or(0);
                        match (group, k) {
                            ("shared", "hit") => b.shared_hit = val,
                            ("shared", "read") => b.shared_read = val,
                            ("shared", "dirtied") => b.shared_dirtied = val,
                            ("shared", "written") => b.shared_written = val,
                            ("local", "hit") => b.local_hit = val,
                            ("local", "read") => b.local_read = val,
                            ("local", "dirtied") => b.local_dirtied = val,
                            ("local", "written") => b.local_written = val,
                            ("temp", "read") => b.temp_read = val,
                            ("temp", "written") => b.temp_written = val,
                            _ => {}
                        }
                    }
                }
            }
        }
        n.buffers = Some(b);
        return;
    }
    if let Some(v) = l.strip_prefix("I/O Timings:") {
        for p in v.split_whitespace() {
            if let Some(x) = p.strip_prefix("read=") {
                n.io_read_ms = num(x);
            } else if let Some(x) = p.strip_prefix("write=") {
                n.io_write_ms = num(x);
            }
        }
        return;
    }
    let arr = n.extra.entry("text").or_insert_with(|| Value::Array(Vec::new()));
    if let Value::Array(a) = arr {
        a.push(Value::String(l.to_string()));
    }
}

pub fn from_text(text: &str) -> Result<PlanTree, PlanError> {
    let (lines, query_text) = clean(text);
    let first = lines.first().ok_or_else(|| err("no plan node found", 1))?;
    let root = parse_head(first.1.trim(), first.0)?;
    let root_indent = first.1.len() - first.1.trim_start().len();
    let mut tree = PlanTree {
        root: root.clone(),
        planning_ms: None,
        execution_ms: None,
        triggers: Vec::new(),
        jit: None,
        settings: Vec::new(),
        total_ms: None,
    };
    // Стек узлов по отступу: заголовок с бóльшим отступом — ребёнок последнего узла с меньшим.
    let mut stack: Vec<(usize, PlanNode)> = vec![(root_indent, root)];
    // `SubPlan 1` / `InitPlan 1 (returns $0)` / `CTE name` объявляются строкой над узлом: подпись ждёт следующий заголовок.
    let mut pending: Option<(String, &'static str)> = None;
    let mut in_jit = false;
    let mut in_settings = false;
    for (line_no, l) in lines.iter().skip(1) {
        let trimmed = l.trim();
        if let Some(v) = trimmed.strip_prefix("Planning Time:") {
            tree.planning_ms = num(v.trim_end_matches("ms"));
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("Execution Time:") {
            tree.execution_ms = num(v.trim_end_matches("ms"));
            continue;
        }
        if let Some(v) = trimmed.strip_prefix("Trigger ") {
            // `Trigger name [for constraint x] on rel: time=1.234 calls=5`
            let (name_part, stats) = v.split_once(':').unwrap_or((v, ""));
            let name = name_part.split(" for ").next().unwrap_or(name_part).split(" on ").next().unwrap_or(name_part).to_string();
            let relation = name_part.split(" on ").nth(1).map(str::to_string);
            let kv = |k: &str| stats.split_whitespace().find_map(|p| p.strip_prefix(k)).and_then(num).unwrap_or(0.0);
            tree.triggers.push(Trigger { name, relation, time_ms: kv("time="), calls: kv("calls=") as i64 });
            continue;
        }
        if trimmed == "JIT:" {
            in_jit = true;
            tree.jit = Some(Jit { functions: 0, generation_ms: 0.0, inlining_ms: 0.0, optimization_ms: 0.0, emission_ms: 0.0, total_ms: 0.0 });
            continue;
        }
        if in_jit && let Some(j) = tree.jit.as_mut() {
            if let Some(v) = trimmed.strip_prefix("Functions:") {
                j.functions = v.trim().parse().unwrap_or(0);
                continue;
            }
            if let Some(v) = trimmed.strip_prefix("Timing:") {
                for part in v.split(',') {
                    let mut it = part.split_whitespace();
                    let (k, val) = (it.next().unwrap_or(""), it.next().and_then(num).unwrap_or(0.0));
                    match k {
                        "Generation" => j.generation_ms = val,
                        "Inlining" => j.inlining_ms = val,
                        "Optimization" => j.optimization_ms = val,
                        "Emission" => j.emission_ms = val,
                        "Total" => j.total_ms = val,
                        _ => {}
                    }
                }
                continue;
            }
            if trimmed.starts_with("Options:") {
                continue;
            }
            in_jit = false;
        }
        if let Some(v) = trimmed.strip_prefix("Settings:") {
            in_settings = true;
            for kv in v.split(", ") {
                if let Some((k, val)) = kv.split_once(" = ") {
                    tree.settings.push(Setting { name: k.trim().to_string(), value: val.trim().trim_matches('\'').to_string() });
                }
            }
            continue;
        }
        if in_settings && !is_head(trimmed) && !trimmed.contains(':') {
            continue;
        }
        in_settings = false;
        if trimmed.starts_with("SubPlan ") || trimmed.starts_with("InitPlan ") || trimmed.starts_with("CTE ") {
            pending = Some((trimmed.to_string(), if trimmed.starts_with("InitPlan") { "InitPlan" } else { "SubPlan" }));
            continue;
        }
        if is_head(trimmed) {
            let indent = l.find("->").map(|a| a + 4).unwrap_or(l.len() - l.trim_start().len());
            let mut node = parse_head(trimmed.trim_start_matches("->").trim(), *line_no)?;
            if let Some((name, relationship)) = pending.take() {
                node.subplan_name = Some(name);
                node.parent_relationship = Some(relationship.to_string());
            }
            // Узлы с отступом не меньше нового закрываются: они — братья нового или глубже.
            while stack.len() > 1 && stack.last().is_some_and(|(i, _)| *i >= indent) {
                let (_, done) = stack.pop().expect("len > 1");
                stack.last_mut().expect("root").1.children.push(done);
            }
            stack.push((indent, node));
            continue;
        }
        if let Some((_, node)) = stack.last_mut() {
            detail(node, trimmed);
        }
    }
    while stack.len() > 1 {
        let (_, done) = stack.pop().expect("len > 1");
        stack.last_mut().expect("root").1.children.push(done);
    }
    tree.root = stack.pop().map(|(_, n)| n).expect("root");
    if let Some(q) = query_text {
        tree.root.extra.insert("queryText".to_string(), Value::String(q));
    }
    tree.finish();
    Ok(tree)
}
```

`src-tauri/src/commands/explain.rs` — `parse_plan`: `from_any(&text)` вместо `from_psql_text` + `from_json`. `ExplainTab`: подпись `diag.explain.pasteLabel` → "Plan (JSON or text)" / "План (JSON или текст)", `pastePlaceholder` → "Paste EXPLAIN output: JSON or text, psql framing and auto_explain lines are fine" / "Вставьте вывод EXPLAIN: JSON или текст, обрамление psql и строки auto_explain не мешают"; тест `ExplainTab.test.tsx` — подпись поля обновить.

- [ ] **Step 4: Фикстуры-пары и тест эквивалентности**

Снять текстовые планы тех же запросов, что JSON-фикстуры P3a (`crates/pg-advisor/tests/fixtures/plans/text/<имя>.txt`): для каждого `X name opts sql` задачи 8 плана P3a выполнить `docker exec gp-pg-perf psql -U postgres -d perf -c "EXPLAIN (<opts без FORMAT JSON>) <sql>" > $P/text/name.txt` (с обрамлением psql); для матрицы — аналогично с `gp-pg13…18`. Поскольку ANALYZE даёт разные времена при разных прогонах, пары снимаются **одним** запросом: `EXPLAIN (ANALYZE, BUFFERS, VERBOSE, FORMAT JSON)` и текстовый вариант — разные выполнения; тест сравнивает **форму и оценки**, не времена: в `tests/plans.rs`:

```rust
#[test]
fn text_and_json_fixtures_agree_on_shape_and_estimates() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans");
    for entry in std::fs::read_dir(dir.join("text")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let text = pg_advisor::plan::text_in::from_text(&std::fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let json_path = dir.join("json").join(format!("{name}.json"));
        let json = pg_advisor::plan::json::from_json(&serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap()).unwrap();
        fn shape(n: &pg_advisor::plan::PlanNode, out: &mut Vec<(String, Option<String>, f64, f64, usize)>) {
            out.push((n.node_type.clone(), n.relation.as_ref().map(|r| r.name.clone()), n.total_cost, n.plan_rows, n.children.len()));
            n.children.iter().for_each(|c| shape(c, out));
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        shape(&text.root, &mut a);
        shape(&json.root, &mut b);
        assert_eq!(a, b, "{name}: форма дерева, отношения, стоимость и оценки совпадают");
        assert_eq!(text.root.actual.is_some(), json.root.actual.is_some(), "{name}");
    }
}
```

(Стоимости и оценки детерминированы при неизменной статистике: пары снимаются подряд без `ANALYZE` между ними.)

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo clippy --manifest-path crates/pg-advisor/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path crates/pg-advisor/Cargo.toml --check && cargo test --manifest-path src-tauri/Cargo.toml --lib && pnpm test -- ExplainTab`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor src-tauri/src/commands/explain.rs src/components/queries/ExplainTab.tsx src/components/queries/ExplainTab.test.tsx src/locales
git commit -m "feat(plan): парсер текстовых планов psql и auto_explain в то же дерево, parse_plan принимает JSON и текст, пары фикстур

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Снимки pgss — хранилище, команды, сравнение, режим «между снимками»

**Files:**
- Create: `src-tauri/src/snapshots/mod.rs`, `src-tauri/src/snapshots/store.rs`, `src-tauri/src/commands/snapshots.rs`, `crates/pg-advisor/src/queries/compare.rs`, `src-tauri/tests/it_snapshots.rs`
- Modify: `crates/pg-advisor/src/queries/mod.rs` (`pub mod compare;`), `crates/pg-advisor/src/findings.rs` (`COMPARE_*`), `src-tauri/src/lib.rs` (`pub mod snapshots;`, команды), `src-tauri/src/state.rs` (`snapshots`), `src-tauri/src/diag/queries.rs` (`QueriesMode::Between` → `report_between`, `validate`), `src-tauri/src/commands/mod.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты `compare.rs`, `store.rs`; `it_snapshots.rs`

**Interfaces:**
- Consumes: `diag::queries::{detect, read_cumulative, texts_sql, stats_since_reset-подобная сборка, QueriesParams, QueriesData, PgssMeta, keep}`, `pg_advisor::queries::{Cumulative, QueryStat, Totals, stat, totals, top}`, `storage::fs::write_private`, `time` (имена файлов).
- Produces: `pg_advisor::queries::compare::{Change { key, datname, rolname, before: QueryStat, after: QueryStat, ratio }, Comparison { new: Vec<QueryStat>, gone: Vec<QueryStat>, degraded: Vec<Change>, improved: Vec<Change>, totals_from, totals_to, reset: bool }, compare(from: &Snapshotish, to: &Snapshotish) -> Comparison}` (вход — срезы `Cumulative` и `stats_reset_ms` обоих снимков), пороги `COMPARE_DEGRADED_RATIO = 2.0`, `COMPARE_IMPROVED_RATIO = 0.5`, `COMPARE_MIN_CALLS = 50`; `snapshots::{Snapshot { v: 1, taken_at_ms, connection: RecordConnection, member, server_version_num, pgss_version, stats_reset_ms, dealloc, rows: Vec<Cumulative>, texts: BTreeMap<String, String> }, SnapshotInfo { file, member, taken_at_ms, rows, bytes }, SnapshotStore::new(data_dir), take(st, id, member) -> SnapshotInfo, list(connection_id) -> Vec<SnapshotInfo>, read(connection_id, file) -> Snapshot, delete(connection_id, file) -> bool, valid_file(&str) -> bool, file_name(taken_at_ms) -> String}`; `TEXTS_IN_SNAPSHOT = 500`; команды `snapshot_take(id, member)`, `snapshot_list(id)`, `snapshot_delete(id, file)`, `snapshot_compare(id, from, to) -> Comparison`; `QueriesMode::Between { from, to }` принимается `validate`, `report_between` строит `DiagData::Queries` из дельт.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/queries/compare.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{Counters, Cumulative, QueryKey};

    fn row(id: i64, calls: i64, ms: f64) -> Cumulative {
        Cumulative { key: QueryKey { query_id: id, dbid: 1, userid: 1, toplevel: true }, datname: Some("perf".into()), rolname: Some("app".into()),
            counters: Counters { calls, total_exec_ms: ms, ..Counters::default() }, min_exec_ms: 0.0, max_exec_ms: 0.0, stddev_exec_ms: 0.0 }
    }

    #[test]
    fn classifies_new_gone_degraded_and_improved() {
        let from = [row(1, 100, 1000.0), row(2, 100, 1000.0), row(3, 100, 1000.0), row(4, 10, 100.0)];
        let to = [row(1, 200, 4000.0), row(2, 200, 2500.0), row(3, 200, 1200.0), row(4, 20, 110.0), row(5, 50, 50.0)];
        // Дельты: 1 — 100 вызовов за 3000 мс (30 мс против 10 раньше — ×3, деградация); 2 — 100 за 1500 (15 против 10 — ×1,5, в норме);
        // 3 — 100 за 200 (2 против 10 — ×0,2, ускорился); 4 — мало вызовов; 5 — новый.
        let c = compare(&from, Some(1), &to, Some(1));
        assert_eq!(c.new.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [5]);
        assert!(c.gone.is_empty());
        assert_eq!(c.degraded.iter().map(|x| (x.key.query_id, (x.ratio * 10.0).round() / 10.0)).collect::<Vec<_>>(), [(1, 3.0)]);
        assert_eq!(c.improved.iter().map(|x| x.key.query_id).collect::<Vec<_>>(), [3]);
        assert_eq!((c.totals_from.calls, c.totals_to.calls), (310, 670));
        assert!(!c.reset);
        assert_eq!(c.degraded[0].after.mean_exec_ms, 30.0, "`after` — средняя дельты, не накопительная");
    }

    #[test]
    fn gone_and_reset_are_reported() {
        let from = [row(1, 100, 1000.0), row(9, 100, 1000.0)];
        let to = [row(1, 20, 200.0)];
        let c = compare(&from, Some(1), &to, Some(2));
        assert_eq!(c.gone.iter().map(|r| r.key.query_id).collect::<Vec<_>>(), [9]);
        assert!(c.reset, "stats_reset различается");
        assert!(c.degraded.is_empty() && c.improved.is_empty(), "после сброса дельты не сравниваются");
    }

    #[test]
    fn deltas_between_snapshots_handle_decreased_counters() {
        let from = [row(1, 100, 1000.0), row(2, 100, 1000.0)];
        let to = [row(1, 150, 1500.0), row(2, 5, 50.0)];
        let d = deltas(&from, &to, false);
        assert_eq!(d.iter().map(|(k, c)| (k.query_id, c.calls)).collect::<Vec<_>>(), [(1, 50), (2, 5)], "уменьшившийся счётчик — значение целиком");
        let d = deltas(&from, &to, true);
        assert_eq!(d.iter().map(|(k, c)| (k.query_id, c.calls)).collect::<Vec<_>>(), [(1, 150), (2, 5)], "после сброса — значения `to`");
    }
}
```

`src-tauri/src/snapshots/store.rs` — тесты:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_and_validation() {
        assert_eq!(file_name(1_761_735_600_000), "2025-10-29T11-00-00.json");
        assert!(valid_file("pg-1/2025-10-29T11-00-00.json"));
        assert!(!valid_file("../x/2025-10-29T11-00-00.json"));
        assert!(!valid_file("pg-1/../../etc/passwd"));
        assert!(!valid_file("2025-10-29T11-00-00.json"), "нужен узел");
        assert!(!valid_file("pg-1/a/b.json"));
    }

    #[test]
    fn store_writes_lists_reads_and_deletes_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let snap = Snapshot {
            v: 1, taken_at_ms: 1_761_735_600_000,
            connection: crate::actions::journal::RecordConnection { id: "c1".into(), name: "C".into(), environment: crate::conn::profile::Environment::Dev },
            member: "pg-1".into(), server_version_num: 170_000, pgss_version: "1.11".into(), stats_reset_ms: Some(1), dealloc: Some(0),
            rows: vec![], texts: Default::default(),
        };
        let info = store.write("c1", &snap).unwrap();
        assert_eq!((info.file.as_str(), info.member.as_str(), info.rows), ("pg-1/2025-10-29T11-00-00.json", "pg-1", 0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("snapshots/c1").join(&info.file);
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(store.list("c1").unwrap().len(), 1);
        assert!(store.list("other").unwrap().is_empty());
        let back = store.read("c1", &info.file).unwrap();
        assert_eq!(back.member, "pg-1");
        assert!(store.read("c1", "../c1/x.json").is_err());
        assert!(store.delete("c1", &info.file).unwrap());
        assert!(!store.delete("c1", &info.file).unwrap());
        assert!(store.list("c1").unwrap().is_empty());
    }
}
```

`src-tauri/tests/it_snapshots.rs`:

```rust
//! Снимки pgss на pg-perf (`GPG_IT_PERF`): снятие, список, сравнение, режим «между снимками». `--test-threads=1`.

mod common;

use std::time::Duration;

use glafira_pg_lib::diag::queries::{QueriesMode, QueriesParams};
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::snapshots;
use pg_advisor::queries::QuerySort;

#[tokio::test]
async fn take_compare_and_between_mode() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let a = snapshots::take(&st, "it", &member).await.unwrap();
    assert!(a.rows > 0 && a.bytes > 0 && a.file.starts_with(&format!("{member}/")));
    let su = common::superuser(&perf, "perf").await;
    for _ in 0..30 {
        su.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'disputed'").await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1100)).await; // имя файла — до секунды
    let b = snapshots::take(&st, "it", &member).await.unwrap();
    assert_ne!(a.file, b.file);
    let list = st.snapshots.list("it").unwrap();
    assert_eq!(list.len(), 2);
    let cmp = snapshots::compare(&st, "it", &a.file, &b.file).await.unwrap();
    assert!(cmp.totals_to.calls > cmp.totals_from.calls);
    assert!(!cmp.reset);
    let params = QueriesParams { mode: QueriesMode::Between { from: a.file.clone(), to: b.file.clone() }, sort: QuerySort::Calls, limit: 50, database: None, toplevel_only: true };
    let r = diag::run(&st, "it", DiagRequest { tool: Tool::Queries, member: Some(member.clone()), window_s: None, queries: Some(params) }).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert!(d.rows.iter().any(|x| x.calls >= 30), "{:?}", d.rows.iter().map(|x| x.calls).collect::<Vec<_>>());
    assert!(d.coverage_s.is_some_and(|c| c >= 1));
    assert!(d.rows.iter().all(|x| x.stddev_exec_ms.is_none()));
    assert!(d.rows.iter().any(|x| x.text.is_some()), "тексты верхних запросов — из снимка");
    assert!(st.snapshots.delete("it", &a.file).unwrap() && st.snapshots.delete("it", &b.file).unwrap());
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml compare && cargo test --manifest-path src-tauri/Cargo.toml --lib snapshots`
Expected: ошибки компиляции.

- [ ] **Step 3: `compare` в pg-advisor**

`findings.rs` → `thresholds`: `pub const COMPARE_DEGRADED_RATIO: f64 = 2.0; pub const COMPARE_IMPROVED_RATIO: f64 = 0.5; pub const COMPARE_MIN_CALLS: i64 = 50;`. `queries/mod.rs` — `pub mod compare;`.

`crates/pg-advisor/src/queries/compare.rs`:

```rust
//! Сравнение двух снимков pgss (спец. P3 §9.3): новые, исчезнувшие, деградировавшие и ускорившиеся запросы по
//! дельтам между снимками; режим топа «между снимками» использует те же дельты.

use std::collections::HashMap;

use serde::Serialize;

use crate::findings::thresholds as th;
use crate::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat, totals};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    /// Средняя дельты `from` относительно предыдущего состояния неизвестна: `before` — накопительное `from`.
    pub before: QueryStat,
    /// Статистика дельты `to − from`.
    pub after: QueryStat,
    /// `after.mean / before.mean`.
    pub ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub new: Vec<QueryStat>,
    pub gone: Vec<QueryStat>,
    pub degraded: Vec<Change>,
    pub improved: Vec<Change>,
    pub totals_from: Totals,
    pub totals_to: Totals,
    /// Статистика сбрасывалась между снимками: дельты не сравниваются.
    pub reset: bool,
}

/// Дельты `to − from` по ключу: ключ только в `to` или с уменьшившимися счётчиками (и все — после сброса) —
/// значение `to` целиком.
pub fn deltas(from: &[Cumulative], to: &[Cumulative], reset: bool) -> Vec<(QueryKey, Counters)> {
    let prev: HashMap<QueryKey, &Counters> = from.iter().map(|c| (c.key, &c.counters)).collect();
    to.iter()
        .map(|c| match prev.get(&c.key) {
            Some(p) if !reset && !c.counters.decreased_from(p) => (c.key, c.counters.delta(p)),
            _ => (c.key, c.counters),
        })
        .collect()
}

pub fn compare(from: &[Cumulative], from_reset_ms: Option<i64>, to: &[Cumulative], to_reset_ms: Option<i64>) -> Comparison {
    let reset = from_reset_ms != to_reset_ms;
    let totals_from = totals(from.iter().map(|c| &c.counters));
    let totals_to = totals(to.iter().map(|c| &c.counters));
    let from_by: HashMap<QueryKey, &Cumulative> = from.iter().map(|c| (c.key, c)).collect();
    let to_by: HashMap<QueryKey, &Cumulative> = to.iter().map(|c| (c.key, c)).collect();
    let as_stat = |c: &Cumulative, t: &Totals| stat(c.key, c.datname.clone(), c.rolname.clone(), &c.counters, Some((c.min_exec_ms, c.max_exec_ms, c.stddev_exec_ms)), t);
    let mut out = Comparison { totals_from, totals_to, reset, ..Comparison::default() };
    out.new = to.iter().filter(|c| !from_by.contains_key(&c.key)).map(|c| as_stat(c, &totals_to)).collect();
    out.gone = from.iter().filter(|c| !to_by.contains_key(&c.key)).map(|c| as_stat(c, &totals_from)).collect();
    if !reset {
        let delta_totals = totals(deltas(from, to, false).iter().map(|(_, c)| c));
        for (key, d) in deltas(from, to, false) {
            let (Some(f), Some(t)) = (from_by.get(&key), to_by.get(&key)) else { continue };
            if f.counters.calls < th::COMPARE_MIN_CALLS || d.calls < th::COMPARE_MIN_CALLS || f.counters.calls == 0 || d.calls == 0 {
                continue;
            }
            let before_mean = f.counters.total_exec_ms / f.counters.calls as f64;
            let after_mean = d.total_exec_ms / d.calls as f64;
            if before_mean <= 0.0 {
                continue;
            }
            let ratio = after_mean / before_mean;
            let change = Change {
                key,
                datname: t.datname.clone(),
                rolname: t.rolname.clone(),
                before: as_stat(f, &totals_from),
                after: stat(key, t.datname.clone(), t.rolname.clone(), &d, None, &delta_totals),
                ratio,
            };
            if ratio >= th::COMPARE_DEGRADED_RATIO {
                out.degraded.push(change);
            } else if ratio <= th::COMPARE_IMPROVED_RATIO {
                out.improved.push(change);
            }
        }
    }
    let by_time = |a: &QueryStat, b: &QueryStat| b.total_exec_ms.partial_cmp(&a.total_exec_ms).unwrap_or(std::cmp::Ordering::Equal);
    out.new.sort_by(by_time);
    out.gone.sort_by(by_time);
    out.degraded.sort_by(|a, b| b.ratio.partial_cmp(&a.ratio).unwrap_or(std::cmp::Ordering::Equal));
    out.improved.sort_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap_or(std::cmp::Ordering::Equal));
    out
}
```

- [ ] **Step 4: Хранилище и команды**

`src-tauri/src/snapshots/mod.rs`:

```rust
//! Снимки `pg_stat_statements` (спец. P3 §9.3): файлы в каталоге данных, снятие, сравнение.

pub mod store;

use std::collections::BTreeMap;

use pg_advisor::queries::compare::Comparison;
use pg_advisor::queries::{Cumulative, QueryKey};
use serde::{Deserialize, Serialize};

use crate::actions::journal::RecordConnection;
use crate::conn::errors::from_pg;
use crate::diag::queries::{Detect, detect, read_cumulative, texts_sql};
use crate::diag::{head, on_member};
use crate::error::{AppResult, ErrorCode};
use crate::state::AppState;
pub use store::{SnapshotInfo, SnapshotStore, file_name, valid_file};

/// Текстов в снимке — у верхних запросов по общему времени.
pub const TEXTS_IN_SNAPSHOT: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub v: u32,
    pub taken_at_ms: i64,
    pub connection: RecordConnection,
    pub member: String,
    pub server_version_num: i32,
    pub pgss_version: String,
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
    /// Все строки pgss накопительно, без текстов.
    pub rows: Vec<Cumulative>,
    /// `queryId` → текст для верхних `TEXTS_IN_SNAPSHOT` по времени.
    pub texts: BTreeMap<String, String>,
}

fn pgss_missing() -> crate::error::AppError {
    app_err!(
        ErrorCode::FeatureUnavailable,
        "errors.queries.pgssMissing",
        "pg_stat_statements is not installed in this database"
    )
}

/// Снимок узла: строки pgss, состояние и тексты верхних запросов; файл — в каталоге подключения.
pub async fn take(st: &AppState, id: &str, member: &str) -> AppResult<SnapshotInfo> {
    let profile = st.profile(id)?;
    let target = st.runtimes.diag_target(id, Some(member), false)?;
    let member = member.to_string();
    let snapshot = on_member(st, id, &target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let Detect::Installed(pgss) = detect(c).await? else { return Err(pgss_missing()) };
            let snap = read_cumulative(c, &pgss).await?;
            let mut top: Vec<&Cumulative> = snap.rows.iter().collect();
            top.sort_by(|a, b| b.counters.total_exec_ms.partial_cmp(&a.counters.total_exec_ms).unwrap_or(std::cmp::Ordering::Equal));
            let ids: Vec<i64> = top.iter().take(TEXTS_IN_SNAPSHOT).map(|c| c.key.query_id).collect();
            let mut texts = BTreeMap::new();
            if !ids.is_empty() {
                for r in c.query(&texts_sql(&pgss.schema, pgss.caps), &[&ids]).await.map_err(|e| from_pg(&e))? {
                    if let (Some(qid), Some(text)) = (r.get::<_, Option<i64>>(0), r.get::<_, Option<String>>(4)) {
                        texts.entry(qid.to_string()).or_insert(text);
                    }
                }
            }
            Ok(Snapshot {
                v: 1,
                taken_at_ms: h.now_ms,
                connection: RecordConnection::of(&profile),
                member: member.clone(),
                server_version_num: h.version_num,
                pgss_version: pgss.version.clone(),
                stats_reset_ms: snap.info.stats_reset_ms,
                dealloc: snap.info.dealloc,
                rows: snap.rows,
                texts,
            })
        })
    })
    .await?;
    let store = st.snapshots.clone();
    let cid = id.to_string();
    crate::conn::run_blocking(move || store.write(&cid, &snapshot)).await
}

/// Сравнение двух снимков подключения (чтение файлов — в блокирующем потоке).
pub async fn compare(st: &AppState, id: &str, from: &str, to: &str) -> AppResult<Comparison> {
    let (a, b) = read_pair(st, id, from, to).await?;
    Ok(pg_advisor::queries::compare::compare(&a.rows, a.stats_reset_ms, &b.rows, b.stats_reset_ms))
}

pub async fn read_pair(st: &AppState, id: &str, from: &str, to: &str) -> AppResult<(Snapshot, Snapshot)> {
    let store = st.snapshots.clone();
    let (cid, from, to) = (id.to_string(), from.to_string(), to.to_string());
    crate::conn::run_blocking(move || Ok((store.read(&cid, &from)?, store.read(&cid, &to)?))).await
}

/// Текст запроса из любого из двух снимков.
pub fn text_of(a: &Snapshot, b: &Snapshot, key: &QueryKey) -> Option<String> {
    let k = key.query_id.to_string();
    b.texts.get(&k).or_else(|| a.texts.get(&k)).cloned()
}
```

`src-tauri/src/snapshots/store.rs`:

```rust
//! Файлы снимков: `data_dir/snapshots/<connectionId>/<member>/<yyyy-mm-ddThh-mm-ss>.json`, каталоги 0700, файлы 0600.

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use time::OffsetDateTime;
use time::macros::format_description;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::snapshots::Snapshot;
use crate::storage::fs::write_private;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    /// `<member>/<имя>.json` — адрес файла в командах.
    pub file: String,
    pub member: String,
    pub taken_at_ms: i64,
    pub rows: usize,
    pub bytes: u64,
}

pub struct SnapshotStore {
    dir: PathBuf,
}

/// Имя файла по времени снятия (UTC): `2025-10-29T11-00-00.json`.
pub fn file_name(taken_at_ms: i64) -> String {
    let t = OffsetDateTime::from_unix_timestamp(taken_at_ms / 1000).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let f = format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");
    format!("{}.json", t.format(&f).unwrap_or_else(|_| taken_at_ms.to_string()))
}

/// Ровно два обычных компонента: `<member>/<name>.json`, без `..` и абсолютных путей.
pub fn valid_file(file: &str) -> bool {
    let p = Path::new(file);
    let parts: Vec<Component> = p.components().collect();
    parts.len() == 2 && parts.iter().all(|c| matches!(c, Component::Normal(_))) && file.ends_with(".json")
}

fn bad_file() -> AppError {
    app_err!(ErrorCode::InvalidInput, "errors.snapshots.badFile", "Bad snapshot file name")
}

fn not_found() -> AppError {
    app_err!(ErrorCode::NotFound, "errors.snapshots.notFound", "The snapshot file is gone")
}

impl SnapshotStore {
    pub fn new(data_dir: &Path) -> Self {
        Self { dir: data_dir.join("snapshots") }
    }

    fn connection_dir(&self, connection_id: &str) -> PathBuf {
        self.dir.join(connection_id)
    }

    /// Каталог подключения и узла; на unix — 0700.
    fn ensure_dir(&self, connection_id: &str, member: &str) -> AppResult<PathBuf> {
        let dir = self.connection_dir(connection_id).join(member);
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)?;
        Ok(dir)
    }

    pub fn write(&self, connection_id: &str, snap: &Snapshot) -> AppResult<SnapshotInfo> {
        let dir = self.ensure_dir(connection_id, &snap.member)?;
        let name = file_name(snap.taken_at_ms);
        let bytes = serde_json::to_vec(snap)?;
        write_private(&dir.join(&name), &bytes)?;
        Ok(SnapshotInfo {
            file: format!("{}/{name}", snap.member),
            member: snap.member.clone(),
            taken_at_ms: snap.taken_at_ms,
            rows: snap.rows.len(),
            bytes: bytes.len() as u64,
        })
    }

    /// Снимки подключения, новые первыми; шапка каждого файла читается (имя файла — только адрес).
    pub fn list(&self, connection_id: &str) -> AppResult<Vec<SnapshotInfo>> {
        let root = self.connection_dir(connection_id);
        let mut out = Vec::new();
        let Ok(members) = std::fs::read_dir(&root) else { return Ok(out) };
        for m in members.flatten() {
            let Ok(files) = std::fs::read_dir(m.path()) else { continue };
            for f in files.flatten() {
                let path = f.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                let Ok(snap) = read_file::<Snapshot>(&path) else { continue };
                out.push(SnapshotInfo {
                    file: format!("{}/{}", m.file_name().to_string_lossy(), f.file_name().to_string_lossy()),
                    member: snap.member,
                    taken_at_ms: snap.taken_at_ms,
                    rows: snap.rows.len(),
                    bytes: meta.len(),
                });
            }
        }
        out.sort_by(|a, b| b.taken_at_ms.cmp(&a.taken_at_ms));
        Ok(out)
    }

    pub fn read(&self, connection_id: &str, file: &str) -> AppResult<Snapshot> {
        if !valid_file(file) {
            return Err(bad_file());
        }
        let path = self.connection_dir(connection_id).join(file);
        if !path.is_file() {
            return Err(not_found());
        }
        read_file(&path)
    }

    pub fn delete(&self, connection_id: &str, file: &str) -> AppResult<bool> {
        if !valid_file(file) {
            return Err(bad_file());
        }
        let path = self.connection_dir(connection_id).join(file);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}
```

`state.rs`: `pub snapshots: Arc<crate::snapshots::SnapshotStore>,` и `snapshots: Arc::new(crate::snapshots::SnapshotStore::new(data_dir)),`. `lib.rs`: `pub mod snapshots;`.

`src-tauri/src/commands/snapshots.rs`:

```rust
//! Команды снимков pgss (спец. P3 §10).

use pg_advisor::queries::compare::Comparison;
use tauri::State;

use crate::error::AppResult;
use crate::snapshots::{self, SnapshotInfo};
use crate::state::AppState;

#[tauri::command]
pub async fn snapshot_take(state: State<'_, AppState>, id: String, member: String) -> AppResult<SnapshotInfo> {
    snapshots::take(&state, &id, &member).await
}

#[tauri::command]
pub async fn snapshot_list(state: State<'_, AppState>, id: String) -> AppResult<Vec<SnapshotInfo>> {
    let store = state.snapshots.clone();
    crate::conn::run_blocking(move || store.list(&id)).await
}

#[tauri::command]
pub async fn snapshot_delete(state: State<'_, AppState>, id: String, file: String) -> AppResult<bool> {
    let store = state.snapshots.clone();
    crate::conn::run_blocking(move || store.delete(&id, &file)).await
}

#[tauri::command]
pub async fn snapshot_compare(state: State<'_, AppState>, id: String, from: String, to: String) -> AppResult<Comparison> {
    snapshots::compare(&state, &id, &from, &to).await
}
```

`commands/mod.rs` — `pub mod snapshots;`; `lib.rs` — четыре команды.

`src-tauri/src/diag/queries.rs`:
- `validate`: ветвь `QueriesMode::Between { from, to }` → `Ok(())`, если `crate::snapshots::valid_file(from) && valid_file(to)`, иначе `INVALID_INPUT` `errors.snapshots.badFile` (константа текста та же, что в `store.rs`); ошибка `errors.queries.betweenUnsupported` удаляется из кода и словарей.
- `report`: `QueriesMode::Between { from, to } => report_between(st, id, target, p, from, to).await`.

```rust
/// Между снимками (спец. §9.3): дельты `to − from`, узел и шапка — из снимка `to`.
async fn report_between(st: &AppState, id: &str, target: &DiagTarget, p: &QueriesParams, from: &str, to: &str) -> AppResult<DiagReport> {
    let (a, b) = crate::snapshots::read_pair(st, id, from, to).await?;
    let reset = a.stats_reset_ms != b.stats_reset_ms;
    let names: std::collections::HashMap<QueryKey, (Option<String>, Option<String>)> = b.rows.iter().map(|c| (c.key, (c.datname.clone(), c.rolname.clone()))).collect();
    let kept: Vec<(QueryKey, Counters, Option<String>, Option<String>)> = pg_advisor::queries::compare::deltas(&a.rows, &b.rows, reset)
        .into_iter()
        .map(|(k, c)| {
            let (d, r) = names.get(&k).cloned().unwrap_or_default();
            (k, c, d, r)
        })
        .filter(|(k, _, datname, _)| (!p.toplevel_only || k.toplevel) && p.database.as_deref().is_none_or(|d| datname.as_deref() == Some(d)))
        .collect();
    let t = totals(kept.iter().map(|(_, c, _, _)| c));
    let mut rows: Vec<QueryStat> = kept.into_iter().map(|(k, c, d, r)| stat(k, d, r, &c, None, &t)).collect();
    top(&mut rows, p.sort, p.limit);
    for r in &mut rows {
        if let Some(text) = crate::snapshots::text_of(&a, &b, &r.key) {
            r.text = Some(text);
        }
    }
    let caps = PgssCaps::from_version(&b.pgss_version).ok_or_else(|| AppError::internal_with("snapshot pgss version"))?;
    if !p.sort.available(&caps, true) {
        return Err(sort_unavailable());
    }
    let mut unavailable = Vec::new();
    if reset {
        unavailable.push(Unavailable {
            part: "between".into(),
            member: None,
            reason: ltext!("diag.unavailable.betweenReset", "{{part}}: statistics were reset between the snapshots", part = "between"),
        });
    }
    let state = PgssState { track_planning: true, dealloc: b.dealloc, dealloc_grew: matches!((a.dealloc, b.dealloc), (Some(x), Some(y)) if y > x && !reset), max: 0 };
    let findings = rules::evaluate(Some(&b.member), &rows, &t, &state, Mode::Window);
    let h = Head { version_num: b.server_version_num, in_recovery: false, now_ms: b.taken_at_ms };
    Ok(make_report(
        Tool::Queries,
        target,
        Some(&h),
        DiagData::Queries(QueriesData {
            mode: p.mode.clone(),
            coverage_s: Some(((b.taken_at_ms - a.taken_at_ms).max(0) / 1000) as u32),
            ticks: None,
            pgss: Some(PgssMeta { version: b.pgss_version.clone(), max: 0, track_planning: true, stats_reset_ms: b.stats_reset_ms, dealloc: b.dealloc }),
            totals: t,
            rows,
        }),
        findings,
        unavailable,
    ))
}
```

(`PgssState.max = 0` и `PgssMeta.max = 0`: предел не хранится в снимке — находка «топ неполный» между снимками опирается только на рост `dealloc`; текст находки с `max = 0` не показывать: во фронтенде `findingParams` для `max === 0` опускает параметр — добавить в `findingText.ts` условие `if (k === "max" && v === 0) continue;`.) `track_planning` в снимке не хранится → `true`, правило планирования работает по данным (нули дадут молчание).

Словари: `errors.snapshots.badFile` "Bad snapshot file name" / "Неверное имя файла снимка"; `errors.snapshots.notFound` "The snapshot file is gone" / "Файл снимка исчез"; `diag.unavailable.betweenReset` "{{part}}: statistics were reset between the snapshots" / "{{part}}: статистика сбрасывалась между снимками"; `diag.unavailable.parts.between` "Comparison" / "Сравнение". Удалить `errors.queries.betweenUnsupported` из обоих словарей (ключ больше не используется; `i18n:check` ругается на лишние ключи — проверить).

- [ ] **Step 5: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo fmt --manifest-path src-tauri/Cargo.toml --check && pnpm i18n:check`
Expected: PASS (тест `params_are_validated` P3a: ветвь `Between` теперь принимает корректные имена файлов — обновить ожидание: `between` с `"a"/"b"` → `errors.snapshots.badFile`, с `"pg-1/2025-10-29T11-00-00.json"` — `Ok`).

Run: `GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_snapshots -- --test-threads=1`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/pg-advisor src-tauri src/locales
git commit -m "feat(snapshots): снимки pg_stat_statements на диске (0600), список и удаление, сравнение и режим топа «между снимками»

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Фронтенд — панель снимков, сравнение, режим «между снимками»

**Files:**
- Create: `src/components/queries/SnapshotsPanel.tsx`, `src/components/queries/SnapshotCompare.tsx`, `src/components/queries/SnapshotsPanel.test.tsx`
- Modify: `src/api/types.ts` (`SnapshotInfo`, `Comparison`, `Change`), `src/api/client.ts`, `src/components/queries/QueriesView.tsx` (кнопка «Снимки», режим `between`), `src/components/diag/findingText.ts` (`max === 0`), `src/components/diag/unavailable.ts` (`between`), `src/locales/{en,ru}.json`, `src/styles.css`
- Test: `SnapshotsPanel.test.tsx`

**Interfaces:**
- Produces: типы `SnapshotInfo { file; member; takenAtMs; rows; bytes }`, `Change { key; datname; rolname; before: QueryStat; after: QueryStat; ratio }`, `Comparison { new; gone; degraded; improved; totalsFrom; totalsTo; reset }`; `api.{snapshotTake, snapshotList, snapshotDelete, snapshotCompare}`; `SnapshotsPanel({ connectionId, member, onBetween })` — список, «Сделать снимок», выбор двух, «Сравнить», «Удалить»; `SnapshotCompare({ comparison, onBetween })` — четыре списка; в `QueriesView` режим `between` показывает подпись «Между снимками: A → B» и кнопку выхода из режима.

- [ ] **Step 1: Падающие тесты**

```tsx
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Comparison, SnapshotInfo } from "../../api/types";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { snapshotList: vi.fn(), snapshotTake: vi.fn(), snapshotDelete: vi.fn(), snapshotCompare: vi.fn() } };
});

import { api } from "../../api/client";
import { SnapshotsPanel } from "./SnapshotsPanel";

const a: SnapshotInfo = { file: "pg-1/2026-10-29T10-00-00.json", member: "pg-1", takenAtMs: 1_761_732_000_000, rows: 120, bytes: 40_000 };
const b: SnapshotInfo = { file: "pg-1/2026-10-29T11-00-00.json", member: "pg-1", takenAtMs: 1_761_735_600_000, rows: 130, bytes: 42_000 };
const stat = (id: string, mean: number) => ({ key: { queryId: id, dbid: 1, userid: 1, toplevel: true }, datname: "perf", rolname: "app", calls: 100, totalExecMs: mean * 100, meanExecMs: mean, minExecMs: null, maxExecMs: null, stddevExecMs: null, rows: 0, rowsPerCall: 0, plans: 0, totalPlanMs: 0, sharedHit: 0, sharedRead: 0, sharedDirtied: 0, sharedWritten: 0, localHit: 0, localRead: 0, localDirtied: 0, localWritten: 0, tempRead: 0, tempWritten: 0, ioReadMs: 0, ioWriteMs: 0, tempIoMs: null, walRecords: 0, walFpi: 0, walBytes: 0, jitFunctions: null, jitMs: null, parallelPlanned: null, parallelLaunched: null, sharePct: 0, hitRatio: null, text: "select x", textTruncated: false });
const comparison: Comparison = { new: [stat("5", 1)], gone: [], degraded: [{ key: stat("1", 1).key, datname: "perf", rolname: "app", before: stat("1", 10), after: stat("1", 30), ratio: 3 }], improved: [], totalsFrom: { calls: 1, totalExecMs: 1, rows: 0 }, totalsTo: { calls: 2, totalExecMs: 2, rows: 0 }, reset: false };

describe("SnapshotsPanel", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.snapshotList).mockResolvedValue([b, a]);
    vi.mocked(api.snapshotTake).mockResolvedValue({ ...b, file: "pg-1/2026-10-29T12-00-00.json" });
    vi.mocked(api.snapshotDelete).mockResolvedValue(true);
    vi.mocked(api.snapshotCompare).mockResolvedValue(comparison);
  });

  it("список новых первыми, снятие добавляет строку, удаление спрашивает подтверждение", async () => {
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={() => {}} />);
    const rows = await screen.findAllByRole("row");
    expect(rows[1]).toHaveTextContent("130");
    await user.click(screen.getByRole("button", { name: "Сделать снимок" }));
    await waitFor(() => expect(api.snapshotTake).toHaveBeenCalledWith("c", "pg-1"));
    await waitFor(() => expect(api.snapshotList).toHaveBeenCalledTimes(2));
    await user.click(within(rows[1]).getByRole("button", { name: "Удалить" }));
    await user.click(screen.getByRole("button", { name: "Удалить снимок" }));
    await waitFor(() => expect(api.snapshotDelete).toHaveBeenCalledWith("c", b.file));
  });

  it("два выбранных снимка сравниваются; «Между снимками» передаёт пару старший → младший", async () => {
    const onBetween = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<SnapshotsPanel connectionId="c" member="pg-1" onBetween={onBetween} />);
    const rows = await screen.findAllByRole("row");
    expect(screen.getByRole("button", { name: "Сравнить" })).toBeDisabled();
    await user.click(within(rows[1]).getByRole("checkbox"));
    await user.click(within(rows[2]).getByRole("checkbox"));
    await user.click(screen.getByRole("button", { name: "Сравнить" }));
    await waitFor(() => expect(api.snapshotCompare).toHaveBeenCalledWith("c", a.file, b.file));
    expect(await screen.findByRole("heading", { name: /Деградировали \(1\)/ })).toBeInTheDocument();
    expect(screen.getByText(/×3/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /Новые \(1\)/ })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Топ между снимками" }));
    expect(onBetween).toHaveBeenCalledWith(a.file, b.file);
  });
});
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `pnpm test -- SnapshotsPanel`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`types.ts`:

```ts
export interface SnapshotInfo { file: string; member: string; takenAtMs: number; rows: number; bytes: number }
export interface Change { key: QueryKey; datname: string | null; rolname: string | null; before: QueryStat; after: QueryStat; ratio: number }
export interface Comparison { new: QueryStat[]; gone: QueryStat[]; degraded: Change[]; improved: Change[]; totalsFrom: { calls: number; totalExecMs: number; rows: number }; totalsTo: { calls: number; totalExecMs: number; rows: number }; reset: boolean }
```

`client.ts`: `snapshotTake: (id: string, member: string) => call<SnapshotInfo>("snapshot_take", { id, member })`, `snapshotList: (id: string) => call<SnapshotInfo[]>("snapshot_list", { id })`, `snapshotDelete: (id: string, file: string) => call<boolean>("snapshot_delete", { id, file })`, `snapshotCompare: (id: string, from: string, to: string) => call<Comparison>("snapshot_compare", { id, from, to })`.

`src/components/queries/SnapshotCompare.tsx`:

```tsx
import { useTranslation } from "react-i18next";
import type { Change, Comparison, QueryStat } from "../../api/types";
import { formatCount, formatMs } from "../../lib/format";
import { queryTitle } from "./queriesText";

function Plain({ title, rows }: { title: string; rows: QueryStat[] }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>{title} ({rows.length})</h4>
      {rows.length === 0 ? <p className="muted">—</p> : (
        <table className="diag-table" aria-label={title}>
          <thead><tr><th scope="col">{t("diag.queries.columns.query")}</th><th scope="col" className="num">{t("diag.queries.columns.calls")}</th><th scope="col" className="num">{t("diag.queries.columns.total")}</th></tr></thead>
          <tbody>{rows.map((r) => <tr key={r.key.queryId}><td className="mono">{queryTitle(r.text, r.key.queryId, 80)}</td><td className="num">{formatCount(r.calls)}</td><td className="num">{formatMs(r.totalExecMs)}</td></tr>)}</tbody>
        </table>
      )}
    </section>
  );
}

function Changes({ title, rows }: { title: string; rows: Change[] }) {
  const { t } = useTranslation();
  return (
    <section>
      <h4>{title} ({rows.length})</h4>
      {rows.length === 0 ? <p className="muted">—</p> : (
        <table className="diag-table" aria-label={title}>
          <thead><tr><th scope="col">{t("diag.queries.columns.query")}</th><th scope="col" className="num">{t("diag.snapshots.before")}</th><th scope="col" className="num">{t("diag.snapshots.after")}</th><th scope="col" className="num">{t("diag.snapshots.ratio")}</th></tr></thead>
          <tbody>{rows.map((c) => <tr key={c.key.queryId}><td className="mono">{queryTitle(c.after.text ?? c.before.text, c.key.queryId, 80)}</td><td className="num">{formatMs(c.before.meanExecMs)}</td><td className="num">{formatMs(c.after.meanExecMs)}</td><td className="num">×{formatCount(c.ratio, 1)}</td></tr>)}</tbody>
        </table>
      )}
    </section>
  );
}

/** Итог сравнения двух снимков (спец. P3 §9.3): четыре списка; «Топ между снимками» переводит инструмент в режим дельт. */
export function SnapshotCompare({ comparison, onBetween }: { comparison: Comparison; onBetween: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="snapshot-compare">
      {comparison.reset && <p className="diag-note">{t("diag.snapshots.reset")}</p>}
      <p className="muted">{t("diag.snapshots.totals", { from: formatCount(comparison.totalsFrom.calls), to: formatCount(comparison.totalsTo.calls) })}</p>
      <button type="button" onClick={onBetween}>{t("diag.snapshots.between")}</button>
      <Changes title={t("diag.snapshots.degraded")} rows={comparison.degraded} />
      <Changes title={t("diag.snapshots.improved")} rows={comparison.improved} />
      <Plain title={t("diag.snapshots.new")} rows={comparison.new} />
      <Plain title={t("diag.snapshots.gone")} rows={comparison.gone} />
    </div>
  );
}
```

`src/components/queries/SnapshotsPanel.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { Comparison, SnapshotInfo } from "../../api/types";
import { formatBytes, formatCount, formatTimestamp } from "../../lib/format";
import { ErrorBanner } from "../common/ErrorBanner";
import { SnapshotCompare } from "./SnapshotCompare";

/**
 * Снимки pgss подключения (спец. P3 §9.3): список (новые первыми), «Сделать снимок» для узла инструмента, выбор двух
 * и «Сравнить», удаление с подтверждением. Снимки разных узлов сравниваются (primary и реплика) с пометкой.
 */
export function SnapshotsPanel({ connectionId, member, onBetween }: { connectionId: string; member: string | null; onBetween: (from: string, to: string) => void }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const key = ["snapshots", connectionId];
  const q = useQuery({ queryKey: key, queryFn: () => api.snapshotList(connectionId) });
  const [picked, setPicked] = useState<string[]>([]);
  const [confirmDelete, setConfirmDelete] = useState<SnapshotInfo | null>(null);
  const [comparison, setComparison] = useState<Comparison | null>(null);
  const take = useMutation({ mutationFn: () => api.snapshotTake(connectionId, member!), onSuccess: () => void qc.invalidateQueries({ queryKey: key }) });
  const del = useMutation({ mutationFn: (file: string) => api.snapshotDelete(connectionId, file), onSuccess: () => { setConfirmDelete(null); setPicked([]); void qc.invalidateQueries({ queryKey: key }); } });
  const compare = useMutation({ mutationFn: ([from, to]: [string, string]) => api.snapshotCompare(connectionId, from, to), onSuccess: setComparison });
  const rows = q.data ?? [];
  const toggle = (file: string) => setPicked((p) => (p.includes(file) ? p.filter((f) => f !== file) : [...p.slice(-1), file]));
  // Старший снимок — `from`, младший — `to`.
  const pair = (): [string, string] | null => {
    if (picked.length !== 2) return null;
    const [x, y] = picked.map((f) => rows.find((r) => r.file === f)!);
    return x.takenAtMs <= y.takenAtMs ? [x.file, y.file] : [y.file, x.file];
  };
  const mixed = picked.length === 2 && new Set(picked.map((f) => rows.find((r) => r.file === f)?.member)).size > 1;
  return (
    <section className="snapshots-panel" aria-label={t("diag.snapshots.title")}>
      <div className="toolbar">
        <h3>{t("diag.snapshots.title")}</h3>
        <button type="button" disabled={member === null || take.isPending} onClick={() => take.mutate()}>{t("diag.snapshots.take")}</button>
        <button type="button" disabled={pair() === null || compare.isPending} onClick={() => { const p = pair(); if (p) compare.mutate(p); }}>{t("diag.snapshots.compare")}</button>
      </div>
      {(q.error || take.error || del.error || compare.error) && <ErrorBanner error={q.error ?? take.error ?? del.error ?? compare.error} />}
      {mixed && <p className="diag-note">{t("diag.snapshots.mixedMembers")}</p>}
      {rows.length === 0 && q.isSuccess && <p className="muted">{t("diag.snapshots.empty")}</p>}
      {rows.length > 0 && (
        <table className="diag-table" aria-label={t("diag.snapshots.title")}>
          <thead><tr><th scope="col" /><th scope="col">{t("diag.snapshots.takenAt")}</th><th scope="col">{t("diag.header.node")}</th><th scope="col" className="num">{t("diag.snapshots.rows")}</th><th scope="col" className="num">{t("diag.snapshots.size")}</th><th scope="col" /></tr></thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.file}>
                <td><input type="checkbox" aria-label={t("diag.snapshots.pick", { at: formatTimestamp(r.takenAtMs) })} checked={picked.includes(r.file)} onChange={() => toggle(r.file)} /></td>
                <td>{formatTimestamp(r.takenAtMs)}</td>
                <td>{r.member}</td>
                <td className="num">{formatCount(r.rows)}</td>
                <td className="num">{formatBytes(r.bytes)}</td>
                <td><button type="button" className="link" onClick={() => setConfirmDelete(r)}>{t("common.delete")}</button></td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {confirmDelete && (
        <div role="dialog" aria-modal="true" aria-label={t("diag.snapshots.deleteTitle")} className="modal">
          <p>{t("diag.snapshots.deleteQuestion", { at: formatTimestamp(confirmDelete.takenAtMs) })}</p>
          <div className="toolbar">
            <button type="button" className="danger" disabled={del.isPending} onClick={() => del.mutate(confirmDelete.file)}>{t("diag.snapshots.deleteConfirm")}</button>
            <button type="button" onClick={() => setConfirmDelete(null)}>{t("common.cancel")}</button>
          </div>
        </div>
      )}
      {comparison && <SnapshotCompare comparison={comparison} onBetween={() => { const p = pair(); if (p) onBetween(p[0], p[1]); }} />}
    </section>
  );
}
```

`QueriesView.tsx`: состояние `showSnapshots`; кнопка «Снимки» в подпанели (`aria-expanded`); `<SnapshotsPanel connectionId member={reportMember} onBetween={(from, to) => { setParams((p) => ({ ...p, mode: { kind: "between", from, to }, sort: sortAvailable(p.sort, data?.pgss ?? null, true) ? p.sort : "totalTime" })); setShowSnapshots(false); }} />`; в `modeValue`/`parseMode` режим `between` показывается отдельным пунктом списка «Между снимками» (значение `between`), выбор другого режима сбрасывает его; в шапке при `between` — строка «Между снимками: {{from}} → {{to}}» (`formatTimestamp` из имён файлов не извлекается — показываются имена файлов без `.json`) и покрытие `coverage()` для `between` — `t("diag.queries.betweenCoverage", { min: Math.round(coverageS/60) })`. `coverage()` для окна — без изменений. `findingText.ts`: `if (k === "max" && v === 0) continue;`. `unavailable.ts`: `between: "diag.unavailable.parts.between"`.

Словари `diag.snapshots` (en / ru): `title` "Snapshots"/"Снимки"; `take` "Take snapshot"/"Сделать снимок"; `compare` "Compare"/"Сравнить"; `empty` "No snapshots yet"/"Снимков пока нет"; `takenAt` "Taken"/"Снят"; `rows` "Rows"/"Строк"; `size` "Size"/"Размер"; `pick` "Select snapshot {{at}}"/"Выбрать снимок {{at}}"; `deleteTitle` "Delete snapshot"/"Удаление снимка"; `deleteQuestion` "Delete the snapshot taken {{at}}?"/"Удалить снимок от {{at}}?"; `deleteConfirm` "Delete snapshot"/"Удалить снимок"; `mixedMembers` "The snapshots are from different nodes: the comparison is approximate"/"Снимки с разных узлов: сравнение приблизительное"; `reset` "Statistics were reset between the snapshots: only new and gone queries are reliable"/"Между снимками статистика сбрасывалась: надёжны только новые и исчезнувшие"; `totals` "Calls: {{from}} → {{to}}"/"Вызовов: {{from}} → {{to}}"; `between` "Top between snapshots"/"Топ между снимками"; `degraded` "Degraded"/"Деградировали"; `improved` "Improved"/"Ускорились"; `new` "New"/"Новые"; `gone` "Gone"/"Исчезли"; `before` "Mean before"/"Среднее до"; `after` "Mean after"/"Среднее после"; `ratio` "Ratio"/"Во сколько"; `diag.queries.modes.between` "Between snapshots"/"Между снимками"; `diag.queries.betweenCoverage` "{{min}} min between snapshots"/"{{min}} мин между снимками"; `diag.queries.snapshots` "Snapshots"/"Снимки"; `common.delete`/`common.cancel` — если нет.

- [ ] **Step 4: Прогнать тесты**

Run: `pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check && pnpm build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src docs/i18n/same-as-en.json
git commit -m "feat(ui): панель снимков pgss — снятие, сравнение, удаление, режим топа «между снимками»

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: `pg_stat_kcache` — CPU и диск в топе, карточке и снимках

**Files:**
- Modify: `crates/pg-advisor/src/queries/mod.rs` (`Counters`, `QueryStat`, `stat`, `QuerySort::CpuTime`), `src-tauri/src/diag/queries.rs` (`KcacheCaps`, `detect`, `statements_sql`, `row_from`), `src-tauri/src/sampler/statements.rs` (без изменений логики — дельты через `Counters`), `src/api/types.ts`, `src/components/queries/QueriesView.tsx` (колонки), `src/components/queries/QueryOverview.tsx` (метрики), `src/lib/llmMarkdown.ts`, `src-tauri/tests/it_queries.rs`, `src/locales/{en,ru}.json`
- Test: unit-тесты `queries/mod.rs` (pg-advisor) и `diag/queries.rs`; `it_queries.rs`

**Interfaces:**
- Produces: `Counters.{cpu_user_ms, cpu_sys_ms, disk_read_bytes, disk_write_bytes}: Option<…>` (участвуют в `delta`/`add` через `sub_opt`/`add_opt`), те же поля в `QueryStat`, `QuerySort::CpuTime` (`available` — только при `kcache`), `PgssCaps` не меняется; `diag::queries::KcacheCaps { schema, exec_prefix: bool, has_top: bool }`, `PgssInstalled.kcache: Option<KcacheCaps>`, `statements_sql(ns, caps, kcache: Option<&KcacheCaps>, where_clause)` (35 колонок + 4 kcache), `row_from` читает колонки 35–38; `PgssMeta.kcache: bool`; TS: `QueryStat.{cpuUserMs, cpuSysMs, diskReadBytes, diskWriteBytes}`, `QuerySort` + `"cpuTime"`, `PgssMeta.kcache`.

- [ ] **Step 1: Падающие тесты**

`crates/pg-advisor/src/queries/mod.rs` — тесты:

```rust
    #[test]
    fn kcache_counters_follow_the_option_rules_and_cpu_sort() {
        let a = Counters { cpu_user_ms: Some(10.0), cpu_sys_ms: Some(1.0), disk_read_bytes: Some(4096), ..Counters::default() };
        let b = Counters { cpu_user_ms: Some(15.0), cpu_sys_ms: Some(1.5), disk_read_bytes: Some(8192), ..Counters::default() };
        let d = b.delta(&a);
        assert_eq!((d.cpu_user_ms, d.cpu_sys_ms, d.disk_read_bytes, d.disk_write_bytes), (Some(5.0), Some(0.5), Some(4096), None));
        let t = Totals::default();
        let s = stat(key(1), None, None, &b, None, &t);
        assert_eq!(QuerySort::CpuTime.value(&s), Some(16.5));
        assert_eq!(QuerySort::CpuTime.value(&stat(key(2), None, None, &Counters::default(), None, &t)), None, "без kcache — значения нет");
        assert_eq!(serde_json::to_value(QuerySort::CpuTime).unwrap(), "cpuTime");
    }
```

(`QuerySort::available` для `CpuTime` принимает дополнительный признак — см. реализацию: сигнатура меняется на `available(self, caps: &PgssCaps, window: bool, kcache: bool)`; обновить вызовы и тест `sort_availability_depends_on_caps_and_mode`: `assert!(QuerySort::CpuTime.available(&c10, false, true) && !QuerySort::CpuTime.available(&c10, false, false));`.)

`src-tauri/src/diag/queries.rs` — тест:

```rust
    #[test]
    fn kcache_join_depends_on_its_version() {
        let new = KcacheCaps { schema: "public".into(), exec_prefix: true, has_top: true };
        let sql = statements_sql("public", caps("1.11"), Some(&new), "");
        assert!(sql.contains("LEFT JOIN public.pg_stat_kcache() k ON k.queryid = s.queryid AND k.userid = s.userid AND k.dbid = s.dbid AND k.top = s.toplevel"), "{sql}");
        assert!(sql.contains("k.exec_user_time * 1000, k.exec_system_time * 1000, k.exec_reads, k.exec_writes"), "{sql}");
        let old = KcacheCaps { schema: "public".into(), exec_prefix: false, has_top: false };
        let sql = statements_sql("public", caps("1.9"), Some(&old), "");
        assert!(sql.contains("k.user_time * 1000, k.system_time * 1000, k.reads, k.writes") && !sql.contains("k.top"), "{sql}");
        let none = statements_sql("public", caps("1.9"), None, "");
        assert!(none.contains("NULL::float8, NULL::float8, NULL::bigint, NULL::bigint") && !none.contains("pg_stat_kcache"), "{none}");
    }
```

`it_queries.rs` — добавить:

```rust
#[tokio::test]
async fn kcache_columns_on_perf() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let mut p = since_reset(QuerySort::TotalTime);
    p.sort = pg_advisor::queries::QuerySort::CpuTime;
    let r = diag::run(&st, "it", request(p)).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    assert!(d.pgss.as_ref().is_some_and(|m| m.kcache));
    assert!(d.rows.iter().any(|x| x.cpu_user_ms.is_some_and(|c| c > 0.0)), "CPU заполнен");
    assert!(d.rows.windows(2).all(|w| QuerySort::CpuTime.value(&w[0]) >= QuerySort::CpuTime.value(&w[1])));
    // На узле без kcache сортировка по CPU — INVALID_INPUT, строки без полей.
    if let Some(lab) = common::lab_addr() {
        let (_dir, st) = common::open_server_in(&lab, "orders").await;
        let mut p = since_reset(QuerySort::TotalTime);
        p.sort = QuerySort::CpuTime;
        assert_eq!(diag::run(&st, "it", request(p)).await.unwrap_err().key, "errors.queries.sortUnavailable");
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml kcache && cargo test --manifest-path src-tauri/Cargo.toml --lib kcache`
Expected: ошибки компиляции.

- [ ] **Step 3: Реализация**

`pg-advisor queries/mod.rs`: в `Counters` и `QueryStat` — поля `pub cpu_user_ms: Option<f64>, pub cpu_sys_ms: Option<f64>, pub disk_read_bytes: Option<i64>, pub disk_write_bytes: Option<i64>` (в `delta` — `sub_opt`, в `add` — `add_opt`, в `stat` — копия); `QuerySort::CpuTime` с `value` = `Some(user + sys)` при обоих `Some`, иначе `None`; `available(self, caps, window, kcache)`: `Self::CpuTime => kcache`.

`src-tauri diag/queries.rs`:

```rust
/// Установленный `pg_stat_kcache`: 2.2+ — колонки `exec_*` и `top`, старше — `user_time` и без `top`.
#[derive(Debug, Clone, PartialEq)]
pub struct KcacheCaps {
    pub schema: String,
    pub exec_prefix: bool,
    pub has_top: bool,
}
```

`PgssInstalled.kcache: Option<KcacheCaps>`; в `detect` второй запрос: `SELECT e.extversion::text, n.nspname::text FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_stat_kcache'` → версия `major.minor` ≥ 2.2 → `exec_prefix = has_top = true`. `statements_sql(ns, caps, kcache: Option<&KcacheCaps>, where_clause)`:

```rust
    let (kcols, kjoin) = match kcache {
        Some(k) => {
            let (u, s, r, w) = if k.exec_prefix { ("k.exec_user_time", "k.exec_system_time", "k.exec_reads", "k.exec_writes") } else { ("k.user_time", "k.system_time", "k.reads", "k.writes") };
            let top = if k.has_top && caps.toplevel { " AND k.top = s.toplevel" } else { "" };
            (
                format!("{u} * 1000, {s} * 1000, {r}, {w}"),
                format!(" LEFT JOIN {}.pg_stat_kcache() k ON k.queryid = s.queryid AND k.userid = s.userid AND k.dbid = s.dbid{top}", k.schema),
            )
        }
        None => ("NULL::float8, NULL::float8, NULL::bigint, NULL::bigint".to_string(), String::new()),
    };
```

— колонки добавляются в конец `SELECT` (индексы 35–38), `{kjoin}` — после `LEFT JOIN pg_roles r …` и перед `{where_clause}`. `row_from`: `cpu_user_ms: r.get(35), cpu_sys_ms: r.get(36), disk_read_bytes: r.get(37), disk_write_bytes: r.get(38)`. Все вызовы `statements_sql` (P3a: `read_cumulative`, `query_detail::fetch`, тесты) получают `pgss.kcache.as_ref()`. `PgssMeta.kcache: bool` (`pgss.kcache.is_some()`); `sort.available(&pgss.caps, window, pgss.kcache.is_some())` во всех трёх отчётах (между снимками — `false`: снимок kcache не хранит; поля kcache в `Cumulative` снимка есть, но сортировка по ним недоступна). `query_detail` метрики — те же поля.

Фронтенд: `QueryStat` + 4 поля, `QuerySort` + `"cpuTime"`, `PgssMeta.kcache: boolean`; `sortAvailable(sort, pgss, window)`: `cpuTime` → `pgss?.kcache === true`; колонки `QueriesView`: `{ id: "cpu", sort: "cpuTime", labelKey: "diag.queries.columns.cpu", cell: (r) => r.cpuUserMs === null || r.cpuSysMs === null ? "—" : formatMs(r.cpuUserMs + r.cpuSysMs) }` и `{ id: "disk", sort: null, labelKey: "diag.queries.columns.disk", cell: (r) => r.diskReadBytes === null ? "—" : formatBytes(r.diskReadBytes) }` после `io`; `QueryOverview` метрики `cpu` и `disk`; `llmMarkdown` строки «CPU» и «Диск», когда поля не null. Словари: `diag.queries.columns.cpu` "CPU"/"CPU", `diag.queries.columns.disk` "Disk read"/"Чтение с диска"; `diag.query.metric.cpu` "CPU (user + sys)"/"CPU (user + sys)", `diag.query.metric.disk` "Disk read / written"/"Диск прочитано / записано"; `llm.stats.cpu`, `llm.stats.disk`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && pnpm exec tsc --noEmit && pnpm test && pnpm i18n:check`
Expected: PASS.

Run: `GPG_IT_PERF=localhost:5471 GPG_IT_LAB=localhost:5470 cargo test --manifest-path src-tauri/Cargo.toml --test it_queries kcache -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/pg-advisor src-tauri src
git commit -m "feat(queries): pg_stat_kcache — CPU и дисковые чтения в топе, карточке и markdown, сортировка по CPU

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: `pg_wait_sampling` — точный профиль ожиданий запроса

**Files:**
- Modify: `src-tauri/src/diag/query_detail.rs`, `src/components/queries/QueryOverview.tsx` (подпись источника), `src/components/diag/unavailable.ts` (`waitsProfile`), `src-tauri/tests/it_queries.rs`, `src/locales/{en,ru}.json`
- Test: unit-тест `query_detail.rs` (`waits_sql`); `it_queries.rs`

**Interfaces:**
- Produces: `query_detail::waits_sql(ns) -> String`; в `fetch` — `waits_profile: Option<QueryWaits>` (`source: PgWaitSampling`, `since_ms: 0`) при установленном расширении и `profile_queries = on`; иначе `unavailable{part: waitsProfile}` только при установленном расширении с выключенным профилем; `detail` предпочитает профиль сэмплам.

- [ ] **Step 1: Падающие тесты**

```rust
    #[test]
    fn waits_sql_groups_the_profile_by_event() {
        let sql = waits_sql("public");
        assert!(sql.contains("FROM public.pg_wait_sampling_profile") && sql.contains("WHERE queryid = $1") && sql.contains("GROUP BY 1, 2"), "{sql}");
    }

    #[test]
    fn profile_rows_become_waits() {
        let w = waits_from_profile(vec![("IO".into(), Some("DataFileRead".into()), 70), ("Client".into(), Some("ClientRead".into()), 30)]);
        let w = w.unwrap();
        assert_eq!((w.source, w.since_ms, w.total), (WaitsSource::PgWaitSampling, 0, 100));
        assert_eq!((w.items[0].event_type.as_str(), w.items[0].share_pct), ("IO", 70.0));
        assert!(waits_from_profile(vec![]).is_none());
    }
```

`it_queries.rs`:

```rust
#[tokio::test]
async fn wait_sampling_profile_on_perf() {
    let Some(perf) = common::perf_addr() else { return };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0].name.clone();
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime))).await.unwrap();
    let DiagData::Queries(d) = &r.data else { unreachable!() };
    // Самый долгий запрос pgbench успел попасть в профиль ожиданий (CPU-строки профиль не хранит, поэтому берём тяжёлый).
    let detail = query_detail::detail(&st, "it", &member, "perf", d.rows[0].key.query_id).await.unwrap();
    match &detail.waits {
        Some(w) => assert_eq!(w.source, query_detail::WaitsSource::PgWaitSampling),
        None => assert!(detail.unavailable.iter().any(|u| u.part == "waitsProfile"), "{:?}", detail.unavailable),
    }
}
```

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib query_detail`
Expected: ошибка компиляции.

- [ ] **Step 3: Реализация**

`diag/query_detail.rs`:

```rust
pub fn waits_sql(ns: &str) -> String {
    format!(
        "SELECT event_type::text, event::text, sum(count)::bigint FROM {ns}.pg_wait_sampling_profile \
         WHERE queryid = $1 GROUP BY 1, 2 ORDER BY 3 DESC"
    )
}

/// Профиль `pg_wait_sampling` (накопительный с момента сброса профиля): доли по типу и событию.
pub fn waits_from_profile(rows: Vec<(String, Option<String>, i64)>) -> Option<QueryWaits> {
    let total: u64 = rows.iter().map(|r| r.2.max(0) as u64).sum();
    if total == 0 {
        return None;
    }
    let items = rows
        .into_iter()
        .map(|(event_type, event, count)| WaitItem { event_type, event, count: count.max(0) as u64, share_pct: count.max(0) as f64 / total as f64 * 100.0 })
        .collect();
    Some(QueryWaits { source: WaitsSource::PgWaitSampling, since_ms: 0, total, items })
}

const WAIT_SAMPLING_SQL: &str = "SELECT n.nspname::text, current_setting('pg_wait_sampling.profile_queries', true) \
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'pg_wait_sampling'";
```

В `fetch` после `running`:

```rust
    let mut waits_profile = None;
    if let Some(r) = c.query_opt(WAIT_SAMPLING_SQL, &[]).await.map_err(pg)? {
        let (schema, profile_queries): (String, Option<String>) = (r.get(0), r.get(1));
        if matches!(profile_queries.as_deref(), Some("on") | Some("true") | Some("yes") | Some("1")) {
            let rows = c
                .query(&waits_sql(&crate::catalog::ddl::quote_ident(&schema)), &[&query_id])
                .await
                .map_err(pg)?
                .iter()
                .map(|r| (r.get(0), r.get(1), r.get(2)))
                .collect();
            waits_profile = waits_from_profile(rows);
        } else {
            unavailable.push(Unavailable {
                part: "waitsProfile".into(),
                member: None,
                reason: ltext!("diag.unavailable.waitsProfileOff", "{{part}}: pg_wait_sampling.profile_queries is off", part = "waitsProfile"),
            });
        }
    }
```

`Fetched.waits_profile: Option<QueryWaits>`; в `detail`: `d.waits = f.waits_profile.or_else(|| сэмплы, как в P3a)`. Фронтенд: `QueryOverview` над таблицей ожиданий — подпись `t(\`diag.query.waitsSource.${detail.waits.source}\`)` (`sampler` "from the Sessions sampler, last 60 min"/"по сэмплам «Сессий», 60 мин"; `pgWaitSampling` "pg_wait_sampling profile since its reset"/"профиль pg_wait_sampling с момента его сброса"); `unavailable.ts`: `waitsProfile: "diag.unavailable.parts.waitsProfile"` ("Wait profile (pg_wait_sampling)"/"Профиль ожиданий (pg_wait_sampling)"); словарь `diag.unavailable.waitsProfileOff`.

- [ ] **Step 4: Прогнать тесты**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && pnpm test -- QueryTab && pnpm i18n:check && GPG_IT_PERF=localhost:5471 cargo test --manifest-path src-tauri/Cargo.toml --test it_queries wait_sampling -- --test-threads=1`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri src
git commit -m "feat(queries): профиль ожиданий запроса из pg_wait_sampling с подписью источника и подсказкой при выключенном profile_queries

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Готовность — подсказки к расширениям

**Files:**
- Modify: `crates/pg-advisor/src/readiness.rs`, `src/components/connections/TestPanel.tsx`, `src/locales/{en,ru}.json`
- Test: unit-тест `readiness.rs`; `TestPanel.test.tsx`

- [ ] **Step 1: Падающие тесты**

`readiness.rs`:

```rust
    #[test]
    fn extension_items_carry_a_hint_for_p3_extensions() {
        let items = evaluate(&facts());
        assert_eq!(ext(&items, "hypopg").params.get("hint").map(String::as_str), Some("hypopg"));
        assert_eq!(ext(&items, "pgstattuple").params.get("hint"), None);
    }
```

`TestPanel.test.tsx`: пункт `extension` с `params: { name: "hypopg", hint: "hypopg" }` и статусом `missing` показывает текст «Проверка гипотез индексов («Проверить гипотезу» в плане)».

- [ ] **Step 2: Убедиться, что тесты падают**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml readiness && pnpm test -- TestPanel`
Expected: FAIL.

- [ ] **Step 3: Реализация**

`readiness.rs` — в цикле `OPTIONAL_EXTENSIONS` параметры пункта дополняются `("hint", name.into())` для `hypopg`, `pg_stat_kcache`, `pg_wait_sampling`:

```rust
    const HINTED: [&str; 3] = ["hypopg", "pg_stat_kcache", "pg_wait_sampling"];
    for name in OPTIONAL_EXTENSIONS {
        let fact = f.extensions.iter().find(|e| e.name == name);
        let mut params: Vec<(&str, String)> = vec![("name", name.into())];
        if HINTED.contains(&name) {
            params.push(("hint", name.into()));
        }
        out.push(match fact {
            Some(ExtensionFact { installed: Some(v), .. }) => {
                params.push(("version", v.clone()));
                item("extension", Status::Ok, &params, None)
            }
            Some(ExtensionFact { available: true, .. }) => item("extension", Status::Warn, &params, Some(format!("CREATE EXTENSION IF NOT EXISTS {name};"))),
            _ => item("extension", Status::Missing, &params, None),
        });
    }
```

`TestPanel.tsx` `itemText`: `affects: item.status === "ok" ? null : item.params.hint ? t(\`readiness.extension.hints.${item.params.hint}\`) : t(\`readiness.${item.id}.affects\`, item.params)`. Словари `readiness.extension.hints`: `hypopg` "Index hypothesis checks (“Check hypothesis” in a plan)"/"Проверка гипотез индексов («Проверить гипотезу» в плане)"; `pg_stat_kcache` "CPU and disk per query in the Queries tool"/"CPU и диск по запросам в инструменте «Запросы»"; `pg_wait_sampling` "Exact wait profile of a query in its card"/"Точный профиль ожиданий запроса в карточке".

- [ ] **Step 4: Прогнать тесты и commit**

Run: `cargo test --manifest-path crates/pg-advisor/Cargo.toml && pnpm test -- TestPanel && pnpm i18n:check`
Expected: PASS.

```bash
git add crates/pg-advisor src/components/connections/TestPanel.tsx src/locales
git commit -m "feat(readiness): подсказки, что даёт каждое расширение P3 — hypopg, pg_stat_kcache, pg_wait_sampling

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 11: Документация — разделы P3b главы «Запросы», admin.md, смоук, спека

**Files:**
- Modify: `docs/guide/ru/queries.md`, `docs/guide/en/queries.md`, `docs/admin.md`, `docs/superpowers/specs/2026-10-29-glafira-postgres-p3-design.md`
- Create: `docs/smoke/p3b.md`

- [ ] **Step 1: Глава (ru) — новые разделы перед «Права и нагрузка»**

```markdown
## Рекомендации по плану {#recommendations}

Под сводкой плана — блок [[ui:recommend.title]]. Каждая рекомендация: заголовок с цифрами, доказательство из узла плана (строки, отсев, циклы, объём, время), готовая команда с кнопкой [[ui:recommend.copyCommand]], ожидаемый эффект и риски, кнопка [[ui:recommend.showNode]], которая подсвечивает узел в дереве. Приложение команды не выполняет — скопируйте и выполните сами, когда убедитесь, что совет уместен.

Правила: Seq Scan с большим отсевом — индекс по колонкам фильтра; Index Scan с фильтром — составной индекс; Nested Loop над Seq Scan — индекс по ключу соединения; ошибка оценки строк в десять раз и больше — `ANALYZE`, `SET STATISTICS`, при двух колонках — `CREATE STATISTICS`; сортировка или хеш на диске, lossy bitmap — `work_mem` для сессии или роли и индекс под `ORDER BY`; сортировка всей таблицы ради `LIMIT` — индекс под ключ сортировки; Index Only Scan с heap fetches — `VACUUM`; `Append` по всем партициям — не сработало отсечение; воркеров запущено меньше плана — `max_parallel_workers`; подзапрос на каждую строку — `JOIN` или `LATERAL`; JIT дольше половины выполнения — `jit = off`. Большинству правил нужен факт выполнения: без [[ui:diag.plan.run.analyze]] блок пишет, сколько правил не проверено.

[[ui:recommend.hypo.check]] доступна у рекомендаций с индексом, если в базе установлен `hypopg`. Приложение создаёт виртуальный индекс в своей сессии, строит план без выполнения и сразу удаляет гипотезу (`hypopg_reset()`); окно показывает стоимость плана до и после, использовал ли планировщик индекс и оценку его размера. На сервере ничего не остаётся.

## Снимки {#snapshots}

Кнопка [[ui:diag.queries.snapshots]] в инструменте открывает список снимков `pg_stat_statements` подключения. [[ui:diag.snapshots.take]] сохраняет на вашем компьютере все строки статистики узла и тексты пятисот самых долгих запросов (каталог данных приложения, `snapshots/`, файл доступен только владельцу). Выберите два снимка и нажмите [[ui:diag.snapshots.compare]]: списки деградировавших (среднее время выросло вдвое и больше при достаточном числе вызовов), ускорившихся, новых и исчезнувших запросов. [[ui:diag.snapshots.between]] переводит таблицу в режим [[ui:diag.queries.modes.between]]: дельты между двумя снимками, как окно, но за любой промежуток. Если между снимками статистика сбрасывалась, сравнение об этом предупреждает. Снимки удаляются из списка; в архив диагностики они не входят.

## Текстовые планы {#text-plans}

Консоль принимает не только JSON: в [[ui:diag.explain.sourcePaste]] можно вставить текстовый план из psql (с заголовком `QUERY PLAN` и рамкой) или тело записи auto_explain из лога — строки `LOG: … plan:` и `Query Text:` пропускаются. Дерево, доли и рекомендации строятся так же, как для JSON; строки, которые приложение не распознало, показаны в панели узла как есть.

## Расширения {#extensions}

Если в базе установлен `pg_stat_kcache`, в топе и карточке появляются колонки CPU (время процессора в пользовательском и системном режиме) и чтение с диска по данным операционной системы — в отличие от `shared read`, это настоящие дисковые чтения, а не промахи кэша PostgreSQL; доступна сортировка по CPU. Если установлен `pg_wait_sampling` с `profile_queries = on`, ожидания запроса в карточке берутся из его профиля (точные, с момента сброса профиля), а не из сэмплов вкладки «Сессии». Чек-лист подключения подсказывает, что даёт каждое расширение.
```

(в «Права и нагрузка» убрать фразу «Текстовый формат плана … появится в следующей части» из раздела «Консоль EXPLAIN» и дополнить: «Снимки лежат в каталоге данных и принадлежат пользователю, который их снял».)

- [ ] **Step 2: Глава (en)**

```markdown
## Plan recommendations {#recommendations}

Below the plan summary is the [[ui:recommend.title]] block. Each recommendation has a title with numbers, evidence from the plan node (rows, filtered rows, loops, volume, time), a ready command with a [[ui:recommend.copyCommand]] button, the expected effect and risks, and a [[ui:recommend.showNode]] button that highlights the node in the tree. The app does not run the commands: copy and run them yourself once you are sure the advice fits.

Rules: Seq Scan with heavy filtering — an index on the filter columns; Index Scan with a filter — a composite index; Nested Loop over a Seq Scan — an index on the join key; a tenfold row estimate error — `ANALYZE`, `SET STATISTICS`, with two columns `CREATE STATISTICS`; a sort or hash on disk, lossy bitmap — `work_mem` for the session or role and an index for `ORDER BY`; sorting a whole table for `LIMIT` — an index on the sort key; Index Only Scan with heap fetches — `VACUUM`; `Append` over all partitions — pruning did not work; fewer workers than planned — `max_parallel_workers`; a subquery per row — `JOIN` or `LATERAL`; JIT longer than half of execution — `jit = off`. Most rules need execution facts: without [[ui:diag.plan.run.analyze]] the block says how many rules were not checked.

[[ui:recommend.hypo.check]] is available on recommendations with an index when `hypopg` is installed in the database. The app creates a virtual index in its own session, builds the plan without execution and drops the hypothesis at once (`hypopg_reset()`); the dialog shows the plan cost before and after, whether the planner used the index and its estimated size. Nothing stays on the server.

## Snapshots {#snapshots}

The [[ui:diag.queries.snapshots]] button in the tool opens the list of `pg_stat_statements` snapshots of the connection. [[ui:diag.snapshots.take]] saves on your computer all statistics rows of the node and the texts of the five hundred longest queries (the app's data directory, `snapshots/`, readable by the owner only). Select two snapshots and press [[ui:diag.snapshots.compare]]: lists of degraded queries (mean time doubled or worse with enough calls), improved, new and gone. [[ui:diag.snapshots.between]] switches the table to [[ui:diag.queries.modes.between]]: deltas between two snapshots, like a window, but over any interval. If statistics were reset between the snapshots the comparison warns about it. Snapshots are deleted from the list; they are not part of the diagnostics archive.

## Text plans {#text-plans}

The console accepts more than JSON: in [[ui:diag.explain.sourcePaste]] you can paste a text plan from psql (with the `QUERY PLAN` header and frame) or the body of an auto_explain log record — the `LOG: … plan:` and `Query Text:` lines are skipped. The tree, shares and recommendations are built as for JSON; lines the app did not recognize are shown in the node panel as they are.

## Extensions {#extensions}

When `pg_stat_kcache` is installed in the database, the top and the card gain CPU columns (processor time in user and system mode) and disk reads reported by the operating system — unlike `shared read`, these are real disk reads, not PostgreSQL cache misses; sorting by CPU is available. When `pg_wait_sampling` is installed with `profile_queries = on`, the query's waits in the card come from its profile (exact, since the profile reset) rather than from the Sessions tab samples. The connection checklist tells what each extension gives.
```

- [ ] **Step 3: admin.md, смоук, спека**

`docs/admin.md`: в «Нагрузка» — «`pg_stat_kcache` читается тем же запросом, что `pg_stat_statements` (join); профиль `pg_wait_sampling` — один запрос при открытии и обновлении карточки. Проверка гипотезы hypopg — отдельное соединение EXPLAIN на время проверки: виртуальный индекс живёт в сессии и удаляется `hypopg_reset()`, на диск ничего не пишется.»; в «Данные и логи» — «`snapshots/<id подключения>/<узел>/<время>.json` — снимки `pg_stat_statements` (все строки статистики и тексты 500 самых долгих запросов с нормализованными литералами `$N`); каталоги 0700, файлы 0600 на unix; удаляются из приложения; в архив диагностики не входят.» В «Права» — «`hypopg_create_index` доступна любой роли (функция работает в памяти сессии); `pg_stat_kcache` и `pg_wait_sampling` показывают чужие запросы с `pg_read_all_stats`.»

`docs/smoke/p3b.md`:

```markdown
# Смоук P3b «Советовать и сравнивать»

Сборка и стенд — как в `docs/smoke/p3a.md` (pg-perf под `heavy.sql`).

1. **Рекомендации.** Карточка «SELECT count(*) FROM shop.orders WHERE status = $1» → «План» → значение `disputed` → «План с выполнением»: под сводкой блок «Рекомендации» с «Индекс по колонкам фильтра: shop.orders», команда `CREATE INDEX CONCURRENTLY ON "shop"."orders" ("status");`, «Копировать команду» копирует её; «Показать узел» подсвечивает Seq Scan.
2. **hypopg.** «Проверить гипотезу» → окно: стоимость уменьшилась, «Индекс использован», оценка размера; дерево «после» с Index Scan. В psql `SELECT * FROM hypopg_list_indexes` пуст.
3. **Остальные правила.** Консоль, запросы из `heavy.sql` с выполнением: сортировка на диске → «уходит на диск» с `SET work_mem`; `ORDER BY total DESC LIMIT 10` → «Индекс под ORDER BY … LIMIT»; `balances` → «VACUUM … карта видимости»; `events` без ключа → «Отсечение партиций не сработало»; агрегация → «Воркеров меньше, чем запланировано» и/или «JIT дольше самого запроса»; коррелированный подзапрос → «Подзапрос выполняется на каждую строку»; соединение с `audit` → «Оценка строк занижена».
4. **Без ANALYZE.** Обычный «План»: блок пишет «Выполните план с выполнением, чтобы проверить ещё 9 правил».
5. **Снимки.** Инструмент «Запросы» → «Снимки» → «Сделать снимок»; подождать минуту под нагрузкой → второй снимок; выбрать оба → «Сравнить»: списки деградировавших/ускорившихся/новых/исчезнувших; «Топ между снимками» → режим «Между снимками» с покрытием «1 мин между снимками». Файл в каталоге данных `snapshots/<id>/<узел>/…json` с правами 0600. «Удалить» → подтверждение → строка исчезает.
6. **Текстовый план.** Консоль → «Вставить план» ← вывод `docker exec gp-pg-perf psql -U postgres -d perf -c "EXPLAIN (ANALYZE) SELECT count(*) FROM shop.orders WHERE status = 'disputed'"` → дерево Seq Scan с фактом и рекомендация.
7. **kcache и wait_sampling.** Топ: колонки «CPU» и «Чтение с диска» заполнены, сортировка по CPU работает; карточка тяжёлого запроса: блок «Ожидания» с подписью «профиль pg_wait_sampling…». На `localhost:5470` (pg-lab) колонки CPU — прочерки, сортировка серая.
8. **Готовность.** Форма подключения → «Проверить»: у пунктов hypopg, pg_stat_kcache, pg_wait_sampling — подсказки, что они дают.
```

Спека: §9.1 — колонки из условий (уточнение 1), `Recommendation.params` (2), `hypo.indexDdl` без CONCURRENTLY (3); §9.3 — адрес файла `<member>/<время>.json`, `reset` в сравнении, `max` отсутствует в снимке; §9.5 — kcache через `KcacheCaps` в `PgssInstalled`, байты; `pg_wait_sampling` — `sinceMs: 0`, `unavailable{waitsProfile}`; §9.6 — параметр `hint`; §10 — команды P3b как реализованы (`snapshot_*`, `hypo_check`); §13.2 — `it_hypopg`, `it_snapshots`.

- [ ] **Step 4: Проверка и commit**

Run: `pnpm test -- guide help && pnpm i18n:check && pnpm build`; пройти смоук `docs/smoke/p3b.md`.

```bash
git add docs
git commit -m "docs(queries): разделы P3b главы «Запросы» (рекомендации, hypopg, снимки, текстовые планы, расширения), admin.md, смоук P3b, правки спеки

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Самопроверка плана

- **Покрытие части P3b спецификации:** §1 п. 7 (11 правил, hypopg) — задачи 1, 2, 3, 4; п. 8 (снимки, сравнение, «между снимками») — 6, 7; п. 9 (текстовый парсер) — 5; п. 10 (`pg_stat_kcache`, `pg_wait_sampling`, подсказки готовности) — 8, 9, 10; п. 11 (документация) — 11. §9.1 таблица правил — 1, 2 (пороги в `thresholds`, тексты в 4); §9.2 — 3, 4; §9.3 — 6, 7; §9.4 — 5; §9.5 — 8, 9; §9.6 — 10; §10 (IPC P3b) — 3, 5, 6; §11 (`FEATURE_UNAVAILABLE` hypopg, `INVALID_INPUT` `badDdl`/`badFile`, `NOT_FOUND` снимка) — 3, 6; §13.2 P3b (положительная/отрицательная фикстура на правило — 1, 2; пары JSON/текст — 5; сравнение — 6; `it_hypopg`, `it_snapshots`, kcache — 3, 6, 8); §16 критерии P3b: 1 — 1, 2 (проверка по фикстурам P3a) и смоук; 2 — 3, 4; 3 — 6, 7; 4 — 5; 5 — 8, 9; 6 — 10 и CI.
- **Типы между задачами:** `Recommendation`/`Report`/`HypoIndex` (1) → `ExplainResult` (3) → TS (4); `columns_of`/`index_recommendation` (1) → правила 2; `explain_in_tx`/`classify`/`Step` `pub(crate)` (3) → `hypopg.rs` (3); `from_any` (5) → `parse_plan` (5); `compare::{deltas, compare, Comparison}` (6) → `report_between` (6) → TS `Comparison` (7); `SnapshotStore`/`valid_file` (6) → `validate` (6), команды (6), TS `SnapshotInfo` (7); `KcacheCaps`/`statements_sql` с четвёртым параметром (8) → все вызовы P3a (8); `WaitsSource::PgWaitSampling` (P3a) → 9; `readiness` `hint` (10) → `TestPanel` (10).
- **Плейсхолдеры:** нет; шаблон `(<columns>)` в команде — осознанный текст для пользователя, когда колонки не разобраны.
- **Порядок:** 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11; 3 зависит от 1–2, 4 — от 3, 7 — от 6, 11 — последняя; 5, 8, 9, 10 независимы между собой и могут идти параллельно после 4.
- **Риски:** пары JSON/текст фикстур должны сниматься без ANALYZE таблиц между ними (оценки совпадают только при одной статистике); `hypopg_create_index` вне транзакции `READ ONLY` — виртуальный индекс живёт в сессии и удаляется `hypopg_reset()` даже при ошибке плана (вызов — до возврата ошибки); сортировка по CPU между снимками недоступна (признак `kcache` в снимке не хранится); `profile_queries` у `pg_wait_sampling` по умолчанию может быть выключен — тогда карточка честно показывает сэмплы и подсказку.
