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
