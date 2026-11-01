//! Текстовые планы с pg-perf (PG 17): каждый `tests/fixtures/plans/text/<имя>.txt` (вывод psql, план одной строкой с `+`,
//! запись auto_explain из журнала сервера и её вид у клиента) разбирается в то же дерево, что и `<имя>.json` — тот же запрос
//! в `FORMAT JSON`. Это два разных выполнения: сравниваются структура, имена, цели, оценки, условия и наличие фактов, а
//! строки и циклы — там, где они от выполнения не зависят (вне параллельной части). Плюс круговой путь
//! `from_json → to_text → from_text` по всем JSON-фикстурам — там совпадать должны и значения фактов.

use pg_advisor::plan::json::{from_any, from_json};
use pg_advisor::plan::text_in::from_text;
use pg_advisor::plan::text_out::to_text;
use pg_advisor::plan::{PlanNode, PlanTree};

fn fixtures(dir: &str, ext: &str) -> Vec<std::path::PathBuf> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let mut out: Vec<_> = std::fs::read_dir(&dir)
        .expect("каталог фикстур")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        .collect();
    out.sort();
    out
}

fn json_tree(path: &std::path::Path) -> PlanTree {
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    from_json(&v).unwrap_or_else(|e| panic!("{path:?}: {e:?}"))
}

/// Ключи `extra`, которые текстовый разбор восстанавливает из имени узла.
const NAME_EXTRA: [&str; 5] = [
    "Partial Mode",
    "Operation",
    "Scan Direction",
    "CTE Name",
    "Function Name",
];

/// `exact` — факты из одного выполнения (круговой путь): сравниваются и значения.
fn same(t: &PlanNode, j: &PlanNode, path: &str, exact: bool, parallel: bool) {
    let p = format!("{path}/{}", j.node_type);
    assert_eq!(
        (&t.node_type, &t.strategy, &t.join_type, t.parallel_aware),
        (&j.node_type, &j.strategy, &j.join_type, j.parallel_aware),
        "{p}"
    );
    assert_eq!(
        (&t.parent_relationship, &t.subplan_name),
        (&j.parent_relationship, &j.subplan_name),
        "{p}"
    );
    assert_eq!(
        (&t.relation, &t.index_name),
        (&j.relation, &j.index_name),
        "{p}"
    );
    assert_eq!(
        (t.startup_cost, t.total_cost, t.plan_width),
        (j.startup_cost, j.total_cost, j.plan_width),
        "{p}"
    );
    assert_eq!(t.plan_rows, j.plan_rows.round(), "{p}");
    assert_eq!(
        (&t.filter, &t.index_cond, &t.recheck_cond),
        (&j.filter, &j.index_cond, &j.recheck_cond),
        "{p}"
    );
    assert_eq!(
        (&t.hash_cond, &t.join_filter, &t.merge_cond),
        (&j.hash_cond, &j.join_filter, &j.merge_cond),
        "{p}"
    );
    assert_eq!(
        (&t.sort_key, &t.group_key),
        (&j.sort_key, &j.group_key),
        "{p}"
    );
    assert_eq!(
        (&t.sort_method, &t.sort_space_type),
        (&j.sort_method, &j.sort_space_type),
        "{p}"
    );
    assert_eq!(
        (
            t.workers_planned,
            t.subplans_removed,
            t.hash_buckets,
            t.hash_batches
        ),
        (
            j.workers_planned,
            j.subplans_removed,
            j.hash_buckets,
            j.hash_batches
        ),
        "{p}"
    );
    assert_eq!(t.never_executed, j.never_executed, "{p}");
    assert_eq!(
        t.actual
            .map(|a| (a.startup_ms.is_some(), a.total_ms.is_some())),
        j.actual
            .map(|a| (a.startup_ms.is_some(), a.total_ms.is_some())),
        "{p}: факт и времена — есть в обоих или нет"
    );
    assert_eq!(t.buffers.is_some(), j.buffers.is_some(), "{p}");
    for k in NAME_EXTRA {
        assert_eq!(t.extra.get(k), j.extra.get(k), "{p}: extra[{k}]");
    }
    if exact {
        // Один источник: всё, что печатает `to_text`, восстанавливается без потерь (строки — с округлением, как в тексте).
        let round = |a: Option<pg_advisor::plan::Actual>| {
            a.map(|a| (a.startup_ms, a.total_ms, a.rows.round(), a.loops.round()))
        };
        assert_eq!(round(t.actual), round(j.actual), "{p}");
        assert_eq!((t.buffers, t.io_read_ms), (j.buffers, j.io_read_ms), "{p}");
        assert_eq!(
            (t.sort_space_kb, t.heap_fetches),
            (j.sort_space_kb, j.heap_fetches),
            "{p}"
        );
        // Память `to_text` печатает только у Hash (строка `Buckets:`), у HashAggregate и Memoize — нет.
        if j.hash_batches.is_some() {
            assert_eq!(t.peak_memory_kb, j.peak_memory_kb, "{p}");
        }
        assert_eq!(
            (t.exact_heap_blocks, t.lossy_heap_blocks, t.workers_launched),
            (j.exact_heap_blocks, j.lossy_heap_blocks, j.workers_launched),
            "{p}"
        );
        assert_eq!(
            (
                t.rows_removed_by_filter,
                t.rows_removed_by_index_recheck,
                t.rows_removed_by_join_filter
            ),
            (
                j.rows_removed_by_filter,
                j.rows_removed_by_index_recheck,
                j.rows_removed_by_join_filter
            ),
            "{p}"
        );
    } else {
        // Разные выполнения: вывод VERBOSE и число запущенных воркеров стабильны, строки — вне параллельной части.
        assert_eq!(t.output, j.output, "{p}");
        assert_eq!(
            t.workers_launched.is_some(),
            j.workers_launched.is_some(),
            "{p}"
        );
        if !parallel {
            assert_eq!(
                t.actual.map(|a| (a.rows, a.loops)),
                j.actual.map(|a| (a.rows, a.loops)),
                "{p}"
            );
        }
    }
    assert_eq!(t.children.len(), j.children.len(), "{p}: детей");
    let parallel = parallel || matches!(j.node_type.as_str(), "Gather" | "Gather Merge");
    for (tc, jc) in t.children.iter().zip(&j.children) {
        same(tc, jc, &p, exact, parallel);
    }
}

fn same_tree(t: &PlanTree, j: &PlanTree, name: &str, exact: bool) {
    same(&t.root, &j.root, name, exact, false);
    // Порядок параметров: в тексте — как у сервера, в JSON-объекте — по алфавиту.
    let sorted = |t: &PlanTree| {
        let mut s: Vec<(String, String)> = t
            .settings
            .iter()
            .map(|s| (s.name.clone(), s.value.clone()))
            .collect();
        s.sort();
        s
    };
    assert_eq!(sorted(t), sorted(j), "{name}");
    assert_eq!(
        t.jit.map(|x| (x.functions, x.total_ms.is_some())),
        j.jit.map(|x| (x.functions, x.total_ms.is_some())),
        "{name}: JIT"
    );
    assert_eq!(t.triggers.len(), j.triggers.len(), "{name}");
    if exact {
        assert_eq!(
            (t.planning_ms, t.execution_ms, t.jit, &t.triggers),
            (j.planning_ms, j.execution_ms, j.jit, &j.triggers),
            "{name}"
        );
        assert_eq!(t.total_ms, j.total_ms, "{name}");
    } else if !name.starts_with("auto_explain") {
        // auto_explain не печатает Planning Time и Execution Time.
        assert_eq!(
            (t.planning_ms.is_some(), t.execution_ms.is_some()),
            (j.planning_ms.is_some(), j.execution_ms.is_some()),
            "{name}"
        );
    }
}

#[test]
fn text_fixtures_parse_into_the_same_tree_as_json() {
    let mut count = 0;
    for path in fixtures("tests/fixtures/plans/text", "txt") {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        let t = from_text(&text).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        // Команда разбора вставленного плана определяет текст сама.
        assert_eq!(from_any(&text).unwrap(), t, "{name}: from_any");
        let j = json_tree(&path.with_extension("json"));
        same_tree(&t, &j, &name, false);
        count += 1;
    }
    assert!(count >= 10, "фикстур мало: {count}");
}

#[test]
fn text_fixtures_keep_their_numbers() {
    let dir = "tests/fixtures/plans/text";
    let load = |n: &str| {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("{dir}/{n}.txt"));
        from_text(&std::fs::read_to_string(path).unwrap()).unwrap()
    };
    // ANALYZE + BUFFERS: факты, буферы, I/O (формат PG 17 `shared read=…`), планирование и JIT с временами.
    let t = load("analyze_buffers_join");
    let a = t.root.actual.unwrap();
    assert_eq!(
        (a.startup_ms, a.total_ms, a.rows, a.loops),
        (Some(47.556), Some(47.558), 10.0, 1.0)
    );
    assert_eq!(
        t.root.buffers.map(|b| (b.shared_hit, b.shared_read)),
        Some((6010, 17))
    );
    assert_eq!(t.root.io_read_ms, Some(0.057));
    assert_eq!((t.planning_ms, t.execution_ms), (Some(0.307), Some(54.859)));
    let j = t.jit.unwrap();
    assert_eq!(
        (j.functions, j.generation_ms, j.total_ms),
        (10, Some(0.153), Some(45.677))
    );
    // TIMING OFF: строки и циклы без времён, JIT без времён.
    let t = load("timing_off");
    assert_eq!(
        t.root.actual.map(|a| (a.total_ms, a.rows, a.loops)),
        Some((None, 3.0, 1.0))
    );
    assert_eq!(t.jit.map(|j| (j.functions, j.total_ms)), Some((51, None)));
    assert!(t.root.share_pct.is_none() && t.execution_ms == Some(93.559));
    // Не выполнявшийся узел.
    let t = load("never_executed");
    assert!(t.root.children[1].never_executed);
    // Параллельный план с VERBOSE и SETTINGS: воркеры, блоки `Worker N:` — в extra, параметры, без JSON-ключей лишнего.
    let t = load("parallel_verbose_settings");
    let gm = &t.root.children[0].children[0];
    assert_eq!(
        (gm.workers_planned, gm.workers_launched),
        (Some(2), Some(2))
    );
    let sort = &gm.children[0];
    assert_eq!(
        sort.extra["Worker 0"][0],
        serde_json::json!("actual time=78.123..78.123 rows=3 loops=1")
    );
    assert_eq!(t.settings.len(), 7);
    assert_eq!(t.settings[6].value, "shop, public");
    // Без ANALYZE: JIT без времён.
    let t = load("psql_plain");
    assert!(t.root.actual.is_none() && t.execution_ms.is_none());
    assert_eq!(t.jit.map(|j| (j.functions, j.total_ms)), Some((6, None)));
    // WAL — в extra под ключами JSON.
    let t = load("wal_update");
    assert_eq!(t.root.extra["WAL Records"], serde_json::json!(404));
    assert_eq!(t.root.extra["Operation"], serde_json::json!("Update"));
    // auto_explain: текст запроса в extra корня.
    let t = load("auto_explain");
    assert!(
        t.root.extra["Query Text"]
            .as_str()
            .is_some_and(|q| q.starts_with("SELECT c.region") && q.ends_with("GROUP BY c.region;"))
    );
}

#[test]
fn json_fixtures_survive_the_text_round_trip() {
    let mut count = 0;
    for path in fixtures("tests/fixtures/plans/json", "json") {
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let j = json_tree(&path);
        let text = to_text(&j);
        let t = from_text(&text).unwrap_or_else(|e| panic!("{name}: {e:?}\n{text}"));
        same_tree(&t, &j, &name, true);
        count += 1;
    }
    assert!(count >= 10, "фикстур мало: {count}");
}
