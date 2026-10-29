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
        (14, 0.469, 158.756)
    );
    let j = load("parallel_jit_analyze").jit.expect("JIT");
    assert_eq!(j.generation_ms, 0.226);
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
