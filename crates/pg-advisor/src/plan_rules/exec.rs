//! Правила выполнения (спец. P3 §9.1): сброс на диск, Sort под Limit, heap fetches, Append без pruning, нехватка
//! воркеров, SubPlan на строку, тяжёлый JIT.

use crate::findings::{Severity, thresholds as th};
use crate::plan::{PlanNode, PlanTree};
use crate::plan_rules::{
    Recommendation, column_ref, index_recommendation, qualified, scan::alias, table_name,
};

/// Sort на диске, Hash с батчами: `work_mem` ×1,5 от объёма (не меньше 1 МБ и не больше 1 ГБ). Lossy bitmap: объём
/// битовой карты по страницам кучи не оценить — доказательство в числах страниц, команда — шаблон `<work_mem>`. «Факт».
pub fn spill(n: &PlanNode) -> Option<Recommendation> {
    n.actual?;
    let (kind, space_kb) = if n.sort_space_type.as_deref() == Some("Disk") {
        ("sort", n.sort_space_kb.unwrap_or(0))
    } else if n.hash_batches.is_some_and(|b| b > 1) {
        (
            "hash",
            n.peak_memory_kb.unwrap_or(0) * n.hash_batches.unwrap_or(1),
        )
    } else {
        let lossy = n.lossy_heap_blocks.filter(|l| *l > 0)?;
        return Some(
            Recommendation::new("plan_spill", Severity::Warning, n.id)
                .param("kind", "bitmap")
                .param("lossyBlocks", lossy)
                .param("exactBlocks", n.exact_heap_blocks.unwrap_or(0))
                .command("SET work_mem = '<work_mem>';"),
        );
    };
    let work_mem_kb =
        ((space_kb as f64 * th::PLAN_WORK_MEM_FACTOR) as i64).clamp(1024, th::PLAN_WORK_MEM_MAX_KB);
    Some(
        Recommendation::new("plan_spill", Severity::Warning, n.id)
            .param("kind", kind)
            .param("spaceKb", space_kb)
            .param("workMemKb", work_mem_kb)
            .param("keys", n.sort_key.join(", "))
            .command(format!("SET work_mem = '{work_mem_kb}kB';")),
    )
}

/// Ключ сортировки `[alias.]col [ASC|DESC]` своего псевдонима: колонка и признак DESC. Выражения, `NULLS …`,
/// `COLLATE` и чужие псевдонимы — `None`.
fn sort_column(key: &str, alias: Option<&str>) -> Option<(String, bool)> {
    let (col, rest) = column_ref(key.trim(), alias)?;
    match rest.trim() {
        "" | "ASC" => Some((col, false)),
        "DESC" => Some((col, true)),
        _ => None,
    }
}

/// Sort под Limit (между ними допускается Gather Merge): вход ≥ 100 × предел — индекс под ключ сортировки (по оценке;
/// с фактом — по факту). Вызывается на узле `Limit`; узлы рекомендации — [Sort, Limit].
pub fn sort_limit(limit: &PlanNode) -> Option<Recommendation> {
    if limit.node_type != "Limit" {
        return None;
    }
    let mut n = limit.children.first()?;
    if n.node_type == "Gather Merge" {
        n = n.children.first()?;
    }
    if !matches!(n.node_type.as_str(), "Sort" | "Incremental Sort") {
        return None;
    }
    let limit_rows = limit
        .actual
        .map(|a| a.rows)
        .unwrap_or(limit.plan_rows)
        .max(1.0);
    let input = n.children.first()?;
    // Под Gather Merge факт входа — сумма по процессам (`loops` — число процессов).
    let input_rows = input
        .actual
        .map(|a| a.rows * a.loops)
        .unwrap_or(input.plan_rows);
    if input_rows < th::PLAN_SORT_LIMIT_FACTOR * limit_rows {
        return None;
    }
    let scan = n
        .children
        .iter()
        .find(|c| c.relation.is_some())
        .unwrap_or(input);
    // Индекс служит сортировке, если ключи — префикс одного направления (обратный проход индекса даёт DESC); на первом
    // неразобранном ключе или смене направления список обрывается.
    let mut cols: Vec<String> = Vec::new();
    let mut dir = None;
    for k in &n.sort_key {
        let Some((col, desc)) = sort_column(k, alias(scan)) else {
            break;
        };
        if *dir.get_or_insert(desc) != desc {
            break;
        }
        if !cols.contains(&col) {
            cols.push(col);
        }
    }
    let r = Recommendation::new("plan_sort_limit", Severity::Info, n.id)
        .also_node(limit.id)
        .param("inputRows", input_rows.round() as i64)
        .param("limitRows", limit_rows.round() as i64)
        .param("keys", n.sort_key.join(", "))
        .param("table", table_name(scan).unwrap_or_default());
    Some(if scan.relation.is_some() {
        index_recommendation(r, scan, &cols)
    } else {
        r
    })
}

/// Index Only Scan с heap fetches ≥ 10 % строк и ≥ 1000: VACUUM (карта видимости). «Факт».
pub fn heap_fetches(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Index Only Scan" {
        return None;
    }
    let a = n.actual?;
    let fetches = n.heap_fetches? as f64;
    let rows = (a.rows * a.loops).max(1.0);
    if fetches < th::PLAN_HEAP_FETCH_MIN || fetches < th::PLAN_HEAP_FETCH_SHARE * rows {
        return None;
    }
    let rel = n.relation.as_ref()?;
    Some(
        Recommendation::new("plan_heap_fetches", Severity::Info, n.id)
            .param("table", table_name(n).unwrap_or_default())
            .param("heapFetches", fetches as i64)
            .param("rows", rows.round() as i64)
            .command(format!(
                "VACUUM {};",
                qualified(rel.schema.as_deref(), &rel.name)
            )),
    )
}

/// Append / Merge Append по ≥ 10 партициям без `Subplans Removed`; с фактом — ≥ 50 % детей пустые.
pub fn no_pruning(n: &PlanNode, with_actual: bool) -> Option<Recommendation> {
    if !matches!(n.node_type.as_str(), "Append" | "Merge Append")
        || n.subplans_removed.is_some_and(|s| s > 0)
    {
        return None;
    }
    let scans: Vec<&PlanNode> = n.children.iter().filter(|c| c.relation.is_some()).collect();
    if scans.len() < th::PLAN_APPEND_MIN_CHILDREN {
        return None;
    }
    let empty = scans
        .iter()
        .filter(|c| c.actual.is_some_and(|a| a.rows == 0.0 || a.loops == 0.0))
        .count();
    if with_actual && (empty as f64) < th::PLAN_APPEND_EMPTY_SHARE * scans.len() as f64 {
        return None;
    }
    Some(
        Recommendation::new(
            "plan_no_pruning",
            if with_actual {
                Severity::Warning
            } else {
                Severity::Info
            },
            n.id,
        )
        .param("children", scans.len() as i64)
        .param("emptyChildren", empty as i64),
    )
}

/// Воркеров запущено меньше, чем запланировано. «Факт».
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
            .command("SHOW max_parallel_workers;\nSHOW max_worker_processes;"),
    )
}

/// SubPlan, выполненный ≥ 1000 раз: переписать на JOIN / LATERAL. «Факт».
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
            .param("loops", a.loops.round() as u64)
            .param("name", n.subplan_name.clone().unwrap_or_default()),
    )
}

/// Число процессов, выполнявших план: запущенные воркеры всех Gather / Gather Merge плюс ведущий.
fn processes(n: &PlanNode) -> i64 {
    let own = if matches!(n.node_type.as_str(), "Gather" | "Gather Merge") {
        n.workers_launched.unwrap_or(0)
    } else {
        0
    };
    own + n.children.iter().map(|c| processes(c) - 1).sum::<i64>() + 1
}

/// JIT не меньше половины выполнения. Сервер суммирует время JIT по процессам, а время выполнения — по часам, поэтому
/// JIT делится на число процессов (воркеры Gather / Gather Merge + ведущий); `jitMs` — сумма, как у сервера.
/// «Факт»: времена JIT есть только при ANALYZE с TIMING.
pub fn jit_heavy(t: &PlanTree) -> Option<Recommendation> {
    let jit = t.jit.as_ref()?;
    let jit_ms = jit.total_ms?;
    let exec = t.execution_ms?;
    let procs = processes(&t.root).max(1);
    if exec <= 0.0 || jit_ms / (procs as f64) < th::PLAN_JIT_SHARE * exec {
        return None;
    }
    Some(
        Recommendation::new("plan_jit_heavy", Severity::Info, t.root.id)
            .param("jitMs", jit_ms)
            .param("execMs", exec)
            .param("processes", procs)
            .param("functions", jit.functions)
            .command("SET jit = off;"),
    )
}

#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use crate::plan_rules::{Recommendation, evaluate};
    use serde_json::{Value, json};

    fn rec(v: &Value, id: &str) -> Option<Recommendation> {
        evaluate(&from_json(v).unwrap())
            .recommendations
            .into_iter()
            .find(|r| r.id == id)
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
        assert_eq!(
            (
                r.params["kind"].as_str(),
                r.params["spaceKb"].as_i64(),
                r.params["workMemKb"].as_i64()
            ),
            (Some("sort"), Some(20_480), Some(30_720))
        );
        assert_eq!(r.command.as_deref(), Some("SET work_mem = '30720kB';"));
        let mut hash = with_actual(base("Hash"), 1000.0, 1);
        hash["Hash Batches"] = json!(4);
        hash["Peak Memory Usage"] = json!(1024);
        let r = rec(&json!({"Plan": hash}), "plan_spill").unwrap();
        assert_eq!(r.params["kind"], "hash");
        let mut bitmap = with_actual(base("Bitmap Heap Scan"), 1000.0, 1);
        bitmap["Lossy Heap Blocks"] = json!(300);
        bitmap["Exact Heap Blocks"] = json!(10);
        let r = rec(&json!({"Plan": bitmap}), "plan_spill").unwrap();
        assert_eq!(
            (
                r.params["kind"].as_str(),
                r.params["lossyBlocks"].as_i64(),
                r.params["exactBlocks"].as_i64()
            ),
            (Some("bitmap"), Some(300), Some(10))
        );
        assert!(r.params.get("workMemKb").is_none() && r.params.get("spaceKb").is_none());
        assert_eq!(r.command.as_deref(), Some("SET work_mem = '<work_mem>';"));
        // Предел work_mem — 1 ГБ.
        let mut huge = with_actual(base("Sort"), 1.0, 1);
        huge["Sort Space Type"] = json!("Disk");
        huge["Sort Space Used"] = json!(2_000_000);
        huge["Sort Method"] = json!("external merge");
        assert_eq!(
            rec(&json!({"Plan": huge}), "plan_spill").unwrap().params["workMemKb"],
            1024 * 1024
        );
        // Сортировка в памяти и один батч — молчит.
        let mut mem = with_actual(base("Sort"), 1.0, 1);
        mem["Sort Space Type"] = json!("Memory");
        mem["Sort Space Used"] = json!(25);
        assert!(rec(&json!({"Plan": mem}), "plan_spill").is_none());
    }

    fn sort_under_limit() -> Value {
        let mut sort = base("Sort");
        sort["Sort Key"] = json!(["o.total DESC"]);
        sort["Plan Rows"] = json!(500_000);
        sort["Parent Relationship"] = json!("Outer");
        sort["Plans"] = json!([{"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 50.0, "Plan Rows": 500_000, "Plan Width": 8}]);
        let mut limit = base("Limit");
        limit["Plan Rows"] = json!(10);
        limit["Plans"] = json!([sort]);
        json!({ "Plan": limit })
    }

    #[test]
    fn sort_under_limit_recommends_an_index_even_without_analyze() {
        let v = sort_under_limit();
        let r = rec(&v, "plan_sort_limit").unwrap();
        assert_eq!(
            (
                r.params["inputRows"].as_i64(),
                r.params["limitRows"].as_i64()
            ),
            (Some(500_000), Some(10))
        );
        assert_eq!(r.node_ids, [1, 0]);
        assert_eq!(
            r.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"total\")"
        );
        assert!(r.command.as_deref().unwrap().contains("(\"total\")"));
        // Малый вход — молчит.
        let mut small = v.clone();
        small["Plan"]["Plans"][0]["Plans"][0]["Plan Rows"] = json!(500);
        assert!(rec(&small, "plan_sort_limit").is_none());
    }

    #[test]
    fn sort_limit_through_gather_merge_and_key_prefix() {
        // Limit → Gather Merge → Sort: факт входа — строки × процессы.
        let mut v = sort_under_limit();
        let mut sort = v["Plan"]["Plans"][0].clone();
        sort["Sort Key"] = json!(["o.total DESC", "o.id DESC", "o.created_at", "o.x"]);
        sort["Plans"][0] = with_actual(sort["Plans"][0].clone(), 166_667.0, 3);
        let mut gm = base("Gather Merge");
        gm["Parent Relationship"] = json!("Outer");
        gm["Workers Planned"] = json!(2);
        gm["Plans"] = json!([sort]);
        v["Plan"]["Plans"] = json!([gm]);
        let r = rec(&v, "plan_sort_limit").unwrap();
        assert_eq!(r.node_ids, [2, 0]);
        assert_eq!(r.params["inputRows"], 500_001);
        assert_eq!(
            r.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"total\", \"id\")",
            "смена направления обрывает ключи"
        );
        // Выражение первым ключом — шаблон без гипотезы.
        v["Plan"]["Plans"][0]["Plans"][0]["Sort Key"] = json!(["(o.total * 2) DESC", "o.id"]);
        let r = rec(&v, "plan_sort_limit").unwrap();
        assert!(r.hypo.is_none());
        assert!(r.command.as_deref().unwrap().contains("(<columns>)"));
    }

    #[test]
    fn heap_fetches_on_index_only_scan() {
        let mut ios = with_actual(base("Index Only Scan"), 10_000.0, 1);
        ios["Relation Name"] = json!("balances");
        ios["Schema"] = json!("shop");
        ios["Heap Fetches"] = json!(2_500);
        let r = rec(&json!({"Plan": ios}), "plan_heap_fetches").unwrap();
        assert_eq!(r.command.as_deref(), Some("VACUUM \"shop\".\"balances\";"));
        assert_eq!(
            (r.params["heapFetches"].as_i64(), r.params["rows"].as_i64()),
            (Some(2_500), Some(10_000))
        );
        ios["Heap Fetches"] = json!(500);
        assert!(
            rec(&json!({"Plan": ios}), "plan_heap_fetches").is_none(),
            "меньше 10 %"
        );
        ios["Actual Rows"] = json!(100.0);
        ios["Heap Fetches"] = json!(900);
        assert!(
            rec(&json!({"Plan": ios}), "plan_heap_fetches").is_none(),
            "меньше 1000"
        );
    }

    fn append_of(empty_from: i32) -> Value {
        let child = |i: i32, rows: f64| {
            with_actual(
                json!({"Node Type": "Seq Scan", "Parent Relationship": "Member", "Relation Name": format!("events_{i}"), "Schema": "shop", "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 10, "Plan Width": 8}),
                rows,
                1,
            )
        };
        let mut append = with_actual(base("Append"), 10.0, 1);
        append["Plans"] = json!(
            (0..12)
                .map(|i| child(i, if i < empty_from { 5.0 } else { 0.0 }))
                .collect::<Vec<_>>()
        );
        append
    }

    #[test]
    fn append_without_pruning() {
        let mut append = append_of(2);
        let r = rec(&json!({"Plan": append}), "plan_no_pruning").unwrap();
        assert_eq!(
            (
                r.params["children"].as_i64(),
                r.params["emptyChildren"].as_i64()
            ),
            (Some(12), Some(10))
        );
        append["Subplans Removed"] = json!(20);
        assert!(
            rec(&json!({"Plan": append}), "plan_no_pruning").is_none(),
            "pruning сработал во время выполнения"
        );
        // Пустых детей меньше половины — чтение всех партиций оправдано.
        assert!(rec(&json!({"Plan": append_of(7)}), "plan_no_pruning").is_none());
    }

    #[test]
    fn workers_subplan_and_jit() {
        let mut gather = with_actual(base("Gather"), 1000.0, 1);
        gather["Workers Planned"] = json!(4);
        gather["Workers Launched"] = json!(1);
        let r = rec(&json!({"Plan": gather}), "plan_workers_short").unwrap();
        assert_eq!(
            (r.params["planned"].as_i64(), r.params["launched"].as_i64()),
            (Some(4), Some(1))
        );
        gather["Workers Launched"] = json!(4);
        assert!(rec(&json!({"Plan": gather}), "plan_workers_short").is_none());
        let mut sub = with_actual(base("Aggregate"), 1.0, 3000);
        sub["Parent Relationship"] = json!("SubPlan");
        sub["Subplan Name"] = json!("SubPlan 1");
        let mut root = with_actual(base("Index Scan"), 3000.0, 1);
        root["Plans"] = json!([sub]);
        let r = rec(&json!({"Plan": root}), "plan_subplan_per_row").unwrap();
        assert_eq!(r.params["loops"], 3000);
        assert_eq!(r.params["name"], "SubPlan 1");
        let plan = json!({"Plan": with_actual(base("Aggregate"), 1.0, 1), "Execution Time": 100.0, "JIT": {"Functions": 5, "Timing": {"Generation": 20.0, "Inlining": 20.0, "Optimization": 20.0, "Emission": 0.0, "Total": 60.0}}});
        let r = rec(&plan, "plan_jit_heavy").unwrap();
        assert_eq!(r.command.as_deref(), Some("SET jit = off;"));
        assert_eq!(r.params["jitMs"], 60.0);
        // PG 17+: `Generation` — объект; JIT меньше половины — молчит.
        let pg17 = |total: f64| json!({"Plan": with_actual(base("Aggregate"), 1.0, 1), "Execution Time": 100.0, "JIT": {"Functions": 5, "Timing": {"Generation": {"Deform": 0.1, "Total": 0.5}, "Inlining": 20.0, "Optimization": 20.0, "Emission": 10.0, "Total": total}}});
        assert_eq!(
            rec(&pg17(50.5), "plan_jit_heavy").unwrap().params["jitMs"],
            50.5
        );
        assert!(rec(&pg17(30.0), "plan_jit_heavy").is_none());
        assert_eq!(r.params["processes"], 1);
        // Параллельный план: 120 мс JIT на 3 процесса — 40 мс из 100, молчит; на 2 процесса — 60 мс, срабатывает.
        let parallel = |launched: i64| {
            let mut g = with_actual(base("Gather"), 1.0, 1);
            g["Parent Relationship"] = json!("Outer");
            g["Workers Planned"] = json!(launched);
            g["Workers Launched"] = json!(launched);
            let mut root = with_actual(base("Aggregate"), 1.0, 1);
            root["Plans"] = json!([g]);
            json!({"Plan": root, "Execution Time": 100.0, "JIT": {"Functions": 5, "Timing": {"Generation": 20.0, "Inlining": 50.0, "Optimization": 50.0, "Emission": 0.0, "Total": 120.0}}})
        };
        assert!(rec(&parallel(2), "plan_jit_heavy").is_none());
        let r = rec(&parallel(1), "plan_jit_heavy").unwrap();
        assert_eq!(
            (r.params["jitMs"].as_f64(), r.params["processes"].as_i64()),
            (Some(120.0), Some(2))
        );
    }
}
