//! Планы с pg-perf (PG 17) и матрицы: каждая фикстура `tests/fixtures/plans/json/*.json` разбирается без потерь,
//! вычисляемые поля согласованы. Фикстуры снимаются скриптом из шага 4 задачи 8 плана P3a.

use pg_advisor::plan::json::from_json;
use pg_advisor::plan::text_out::to_text;
use pg_advisor::plan::{PlanNode, PlanTree, relations};

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
        let v: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?}: {e}"));
        let t = from_json(&v).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
        let mut nodes = Vec::new();
        walk(&t.root, &mut nodes);
        assert!(
            nodes.iter().enumerate().all(|(i, n)| n.id == i as u32),
            "{path:?}: id — порядок обхода"
        );
        assert!(nodes.iter().filter(|n| n.hot).count() <= 3, "{path:?}");
        assert!(
            nodes
                .iter()
                .all(|n| n.exclusive_ms.is_none_or(|x| x >= 0.0)),
            "{path:?}"
        );
        let share: f64 = nodes.iter().filter_map(|n| n.share_pct).sum();
        assert!(share <= 100.5, "{path:?}: сумма долей {share}");
        assert!(
            nodes
                .iter()
                .all(|n| n.relation.as_ref().is_none_or(|r| !r.name.is_empty())),
            "{path:?}"
        );
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("analyze")
        {
            assert!(
                t.execution_ms.is_some() && t.root.actual.is_some(),
                "{path:?}"
            );
        }
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("partitions")
        {
            assert!(relations(&t).len() >= 10, "{path:?}: Append по партициям");
        }
    }
    assert!(count >= 10, "фикстур мало: {count}");
}

/// Разобран ли ключ узла в поле модели без потери значения: `Some(true)` — да, `Some(false)` — ключ разбирается, но
/// значение потеряно (например, неверного типа), `None` — модель ключ не разбирает (он должен остаться в `extra`).
fn kept(n: &PlanNode, key: &str, v: &serde_json::Value) -> Option<bool> {
    use serde_json::Value;
    let rel = n.relation.as_ref();
    let actual = n.actual.as_ref();
    let strs = |got: &Vec<String>| match v {
        Value::Array(a) => got.len() == a.len(),
        Value::String(_) => got.len() == 1,
        _ => false,
    };
    Some(match key {
        "Node Type" => v.as_str() == Some(n.node_type.as_str()),
        "Plans" => v.as_array().map(Vec::len) == Some(n.children.len()),
        "Relation Name" => rel.is_some(),
        "Schema" => rel.is_some_and(|r| r.schema.is_some()),
        "Alias" => rel.is_some_and(|r| r.alias.is_some()),
        "Actual Startup Time" => actual.is_some_and(|a| a.startup_ms.is_some()),
        "Actual Total Time" => actual.is_some_and(|a| a.total_ms.is_some()),
        "Actual Rows" | "Actual Loops" => actual.is_some(),
        "Shared Hit Blocks"
        | "Shared Read Blocks"
        | "Shared Dirtied Blocks"
        | "Shared Written Blocks"
        | "Local Hit Blocks"
        | "Local Read Blocks"
        | "Local Dirtied Blocks"
        | "Local Written Blocks"
        | "Temp Read Blocks"
        | "Temp Written Blocks" => n.buffers.is_some() && v.is_i64(),
        "I/O Read Time" | "Shared I/O Read Time" => n.io_read_ms.is_some(),
        "I/O Write Time" | "Shared I/O Write Time" => n.io_write_ms.is_some(),
        "Parent Relationship" => n.parent_relationship.is_some(),
        "Subplan Name" => n.subplan_name.is_some(),
        "Index Name" => n.index_name.is_some(),
        "Strategy" => n.strategy.is_some(),
        "Join Type" => n.join_type.is_some(),
        "Parallel Aware" => v.as_bool() == Some(n.parallel_aware),
        "Startup Cost" => v.as_f64() == Some(n.startup_cost),
        "Total Cost" => v.as_f64() == Some(n.total_cost),
        "Plan Rows" => v.as_f64() == Some(n.plan_rows),
        "Plan Width" => v.as_i64() == Some(n.plan_width),
        "Workers Planned" => n.workers_planned.is_some(),
        "Workers Launched" => n.workers_launched.is_some(),
        "Rows Removed by Filter" => n.rows_removed_by_filter.is_some(),
        "Rows Removed by Index Recheck" => n.rows_removed_by_index_recheck.is_some(),
        "Rows Removed by Join Filter" => n.rows_removed_by_join_filter.is_some(),
        "Heap Fetches" => n.heap_fetches.is_some(),
        "Filter" => n.filter.is_some(),
        "Index Cond" => n.index_cond.is_some(),
        "Recheck Cond" => n.recheck_cond.is_some(),
        "Hash Cond" => n.hash_cond.is_some(),
        "Join Filter" => n.join_filter.is_some(),
        "Merge Cond" => n.merge_cond.is_some(),
        "Sort Key" => strs(&n.sort_key),
        "Group Key" => strs(&n.group_key),
        "Output" => strs(&n.output),
        "Sort Method" => n.sort_method.is_some(),
        "Sort Space Used" => n.sort_space_kb.is_some(),
        "Sort Space Type" => n.sort_space_type.is_some(),
        "Hash Batches" => n.hash_batches.is_some(),
        "Hash Buckets" => n.hash_buckets.is_some(),
        "Peak Memory Usage" => n.peak_memory_kb.is_some(),
        "Exact Heap Blocks" => n.exact_heap_blocks.is_some(),
        "Lossy Heap Blocks" => n.lossy_heap_blocks.is_some(),
        "Subplans Removed" => n.subplans_removed.is_some(),
        _ => return None,
    })
}

/// Узел JSON и узел модели — параллельным обходом: каждый ключ исходного узла либо разобран в поле, либо лежит в
/// `extra` как есть.
fn assert_lossless(path: &std::path::Path, v: &serde_json::Value, n: &PlanNode) {
    let obj = v.as_object().expect("узел — объект");
    for (k, x) in obj {
        if n.extra.get(k) == Some(x) {
            continue;
        }
        match kept(n, k, x) {
            Some(true) => {}
            Some(false) => panic!(
                "{path:?}: узел {} — значение ключа «{k}» потеряно: {x}",
                n.id
            ),
            None => panic!(
                "{path:?}: узел {} — ключа «{k}» нет ни в полях, ни в extra",
                n.id
            ),
        }
    }
    let plans = obj
        .get("Plans")
        .and_then(serde_json::Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    assert_eq!(plans.len(), n.children.len(), "{path:?}: узел {}", n.id);
    for (cv, cn) in plans.iter().zip(&n.children) {
        assert_lossless(path, cv, cn);
    }
}

/// Разбор без потерь: ни один ключ узла фикстуры не пропадает.
#[test]
fn every_fixture_node_keeps_every_key() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/json");
    let mut nodes = 0;
    for entry in std::fs::read_dir(&dir).expect("каталог фикстур") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let t = from_json(&v).unwrap_or_else(|e| panic!("{path:?}: {e:?}"));
        let top = match &v {
            serde_json::Value::Array(a) => &a[0],
            other => other,
        };
        assert_lossless(&path, &top["Plan"], &t.root);
        let mut all = Vec::new();
        walk(&t.root, &mut all);
        nodes += all.len();
    }
    assert!(nodes >= 50, "узлов в фикстурах мало: {nodes}");
}

fn load(name: &str) -> PlanTree {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/plans/json")
        .join(format!("{name}.json"));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    from_json(&v).unwrap_or_else(|e| panic!("{path:?}: {e:?}"))
}

#[test]
fn jit_generation_total_is_read_from_pg17_object() {
    // PG 17: `Timing.Generation` — объект `{Deform, Total}`; в модель идёт `Total`.
    let j = load("seq_scan_filter_analyze").jit.expect("JIT");
    assert_eq!(
        (j.functions, j.generation_ms, j.total_ms),
        (14, Some(0.469), Some(158.756))
    );
    let j = load("parallel_jit_analyze").jit.expect("JIT");
    assert_eq!(j.generation_ms, Some(0.319));
}

#[test]
fn to_text_uses_psql_node_names_on_fixtures() {
    let text = to_text(&load("pg16_join_group_analyze"));
    assert!(text.contains("->  HashAggregate  (cost="), "{text}");
    assert!(!text.contains("HashedAggregate"), "{text}");
    let text = to_text(&load("update_modify_table_plain"));
    assert!(
        text.starts_with("Update on shop.balances  (cost="),
        "{text}"
    );
    let text = to_text(&load("seq_scan_filter_analyze"));
    assert!(text.starts_with("Finalize Aggregate  (cost="), "{text}");
    assert!(text.contains("->  Partial Aggregate  (cost="), "{text}");
    assert!(
        text.contains("->  Parallel Seq Scan on shop.orders  (cost="),
        "{text}"
    );
    // Имя CTE — отдельной строкой над узлом, как в psql.
    let text = to_text(&load("cte_delete_plain"));
    assert!(
        text.contains("\n  CTE d\n    ->  Delete on shop.audit  (cost="),
        "{text}"
    );
    assert!(text.contains("\n  ->  CTE Scan on d  (cost="), "{text}");
    // Подплан с корреляцией: SubPlan 1 над узлом.
    let text = to_text(&load("subplan_per_row_analyze"));
    assert!(
        text.contains("\n  SubPlan 1\n    ->  Aggregate  (cost="),
        "{text}"
    );
}

/// Каждая фикстура `*_analyze` со стенда даёт хотя бы одну рекомендацию из ожидаемого набора (задача 2 плана P3b).
/// Фикстуры `index_only_heap_fetches_analyze`, `parallel_jit_analyze` и `append_partitions_analyze` пересняты на pg-perf
/// (PG 17) под правила: прежние давали Bitmap Heap Scan без `Heap Fetches`, агрегат без Gather и Append без пустых
/// партиций (фильтр не по ключу секционирования — читать все партиции нужно, правило верно молчит). Новые запросы:
/// - `SET enable_bitmapscan = off; … SELECT count(*) FROM shop.balances WHERE customer_id BETWEEN 1000 AND 20000`
///   (у `balances` не проходил VACUUM — карта видимости пуста);
/// - `SET max_parallel_workers_per_gather = 4; SET min_parallel_table_scan_size = '64kB'; SET parallel_setup_cost = 0;
///   SET parallel_tuple_cost = 0; … SELECT customer_id % 1000, sum(total * 1.07 + 3), avg(total) FROM shop.orders GROUP BY 1`
///   (на стенде `max_parallel_workers = 2`: запланировано 4, запущено 2; JIT по всем процессам — больше половины выполнения);
/// - `SELECT count(*) FROM shop.events WHERE happened_at::date = date '2026-03-05'` (приведение ключа мешает pruning).
#[test]
fn analyze_fixtures_trigger_expected_recommendations() {
    let expected: [(&str, &[&str]); 8] = [
        ("seq_scan_filter_analyze", &["plan_seq_scan_filter"]),
        (
            "nested_loop_misestimate_analyze",
            &["plan_misestimate", "plan_nested_loop_seq"],
        ),
        ("sort_limit_analyze", &["plan_sort_limit"]),
        ("sort_spill_analyze", &["plan_spill"]),
        ("index_only_heap_fetches_analyze", &["plan_heap_fetches"]),
        ("subplan_per_row_analyze", &["plan_subplan_per_row"]),
        ("append_partitions_analyze", &["plan_no_pruning"]),
        (
            "parallel_jit_analyze",
            &["plan_workers_short", "plan_jit_heavy"],
        ),
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/json");
    for (name, ids) in expected {
        let text = std::fs::read_to_string(dir.join(format!("{name}.json"))).unwrap();
        let t = from_json(&serde_json::from_str(&text).unwrap()).unwrap();
        let got: Vec<String> = pg_advisor::plan_rules::evaluate(&t)
            .recommendations
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert!(
            ids.iter().any(|id| got.iter().any(|g| g == id)),
            "{name}: ждали одно из {ids:?}, получили {got:?}"
        );
    }
}

/// Решения по итогам задачи 2: сканирования партиций одного Append — одна рекомендация; JIT параллельного плана
/// делится на число процессов; приведение `::date` к колонке — выражение (индекса-гипотезы нет).
#[test]
fn partition_grouping_and_parallel_jit_on_fixtures() {
    let ids = |name: &str| pg_advisor::plan_rules::evaluate(&load(name)).recommendations;
    let append = ids("append_partitions_analyze");
    let seq: Vec<_> = append
        .iter()
        .filter(|r| r.id == "plan_seq_scan_filter")
        .collect();
    assert_eq!(
        seq.len(),
        1,
        "одна рекомендация на Append, а не по партиции"
    );
    assert_eq!(seq[0].params["partitions"], 24);
    assert_eq!(seq[0].node_ids.len(), 24);
    assert!(seq[0].hypo.is_none() && seq[0].params.get("columns").is_none());
    assert!(append.iter().any(|r| r.id == "plan_no_pruning"));
    // 121.8 мс JIT по 3 процессам (2 воркера + ведущий) — 40.6 мс из 182.9 мс выполнения: меньше половины.
    let par = ids("parallel_jit_analyze");
    assert!(!par.iter().any(|r| r.id == "plan_jit_heavy"), "{par:?}");
    assert!(par.iter().any(|r| r.id == "plan_workers_short"));
}

/// Поправки по смоуку P3b (замечания 4–10) на планах pg-perf:
/// - `bitmap_filter_analyze` — `SELECT count(*) FROM shop.audit WHERE order_id = 1 AND actor = 'ops'`;
/// - `bitmap_misestimate_analyze` — `SELECT o.id, a.note FROM shop.audit a JOIN shop.orders o ON o.id = a.order_id
///   WHERE a.actor = 'app' AND a.order_id = 1`.
#[test]
fn hot_nodes_skip_tiny_shares_on_fixtures() {
    // Aggregate над Bitmap Heap Scan: собственное время 0,02 мс — не горячий; горячий только Bitmap Heap Scan.
    let t = load("bitmap_filter_analyze");
    let heap = &t.root.children[0];
    assert!(!t.root.hot && heap.hot && !heap.children[0].hot);
}

/// Подсветка ошибки оценки в дереве: под Limit (ранняя остановка) «переоценки» нет; партиции без отсечения с оценкой
/// ~125 строк и фактом 0 — не «×∞».
#[test]
fn misestimate_flag_skips_early_stop_and_small_zero_estimates() {
    let t = load("sort_limit_analyze");
    let gm = &t.root.children[0];
    assert!(!gm.misestimated && !gm.children[0].misestimated);
    let t = load("append_partitions_analyze");
    let mut nodes = Vec::new();
    walk(&t.root, &mut nodes);
    let zero: Vec<_> = nodes
        .iter()
        .filter(|n| n.relation.is_some() && n.actual.is_some_and(|a| a.rows == 0.0))
        .collect();
    assert!(zero.len() >= 20, "{}", zero.len());
    assert!(zero.iter().all(|n| !n.misestimated));
}

/// index_filter у Bitmap Heap Scan: имя индекса — из дочернего Bitmap Index Scan (доказательство не начинается с «: »).
#[test]
fn bitmap_index_filter_names_the_child_index() {
    let recs = pg_advisor::plan_rules::evaluate(&load("bitmap_filter_analyze")).recommendations;
    let r = recs
        .iter()
        .find(|r| r.id == "plan_index_filter")
        .expect("plan_index_filter");
    assert_eq!(r.params["index"], "audit_order_idx");
    assert_eq!(r.params["rowsRemoved"], 198_000);
}

/// misestimate у Bitmap Heap Scan: колонки из Recheck Cond (order_id) и Filter (actor) вместе — расширенная статистика.
#[test]
fn misestimate_takes_columns_from_recheck_cond_and_filter() {
    let recs =
        pg_advisor::plan_rules::evaluate(&load("bitmap_misestimate_analyze")).recommendations;
    let r = recs
        .iter()
        .find(|r| r.id == "plan_misestimate" && r.params["table"] == "shop.audit")
        .expect("plan_misestimate по shop.audit");
    let cmd = r.command.as_deref().unwrap();
    assert!(
        cmd.contains("ALTER COLUMN \"order_id\" SET STATISTICS 1000;")
            && cmd.contains("ALTER COLUMN \"actor\" SET STATISTICS 1000;"),
        "{cmd}"
    );
    assert!(
        cmd.contains(
            "CREATE STATISTICS \"shop\".\"audit_order_id_actor_stat\" ON \"order_id\", \"actor\" FROM \"shop\".\"audit\";"
        ),
        "{cmd}"
    );
}
