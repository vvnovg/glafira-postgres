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
    /// Времён нет при `TIMING OFF` — тогда `None`.
    pub startup_ms: Option<f64>,
    pub total_ms: Option<f64>,
    /// Строк на цикл — как отдаёт сервер (на 18 дробное).
    pub rows: f64,
    /// Число циклов; у не выполнявшегося узла — 0, у повторно запущенных узлов — сумма по запускам и процессам.
    pub loops: f64,
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
    /// Включённое время, мс; без ANALYZE и при `TIMING OFF` — `None`. Вне параллельной части плана — `actual.total_ms × loops`.
    /// Под `Gather` / `Gather Merge` время узла в `actual.total_ms` — среднее на процесс (так его отдаёт сервер), а `loops` —
    /// число процессов, выполнявших узел (воркеры и ведущий, если он участвует), умноженное на внутренние циклы; поэтому
    /// `total_ms × loops` там — суммарное время всех процессов, а не время по часам. Его делят на число процессов:
    /// `total_ms × loops / процессов` — вклад узла в время выполнения. Число процессов — `loops` внешнего потомка Gather,
    /// делённое на `loops` самого Gather: при повторном запуске Gather (на внутренней стороне Nested Loop, под коррелированным
    /// SubPlan) сервер накапливает инструментацию воркеров по всем запускам, и у потомка `loops` = запуски × процессы.
    /// InitPlan и SubPlan самого Gather в параллельную часть не входят.
    pub inclusive_ms: Option<f64>,
    /// Включённое минус включённое детей (оба — в пересчёте на время по часам, см. `inclusive_ms`), не меньше нуля.
    pub exclusive_ms: Option<f64>,
    /// Доля собственного времени в `PlanTree::total_ms`, проценты; сумма долей узлов плана не превышает 100.
    pub share_pct: Option<f64>,
    /// `actual.rows / plan_rows`; `None` без ANALYZE, у не выполнявшегося узла (сервер отдаёт для него 0 строк) и при
    /// `plan_rows = 0` и `actual.rows = 0`.
    pub estimate_ratio: Option<f64>,
    /// Три узла с наибольшим собственным временем (без ANALYZE и при `TIMING OFF` — по стоимости без детей).
    pub hot: bool,
    /// Ошибка оценки ×10 и больше при `max(план, факт) ≥ 100` строк; у не выполнявшегося узла всегда `false`.
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
    /// При `TIMING OFF` доли узлов остаются `None` (нет времён по узлам), даже если `execution_ms` есть.
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
        compute_times(&mut self.root, 1.0);
        self.total_ms = self.execution_ms.or(self.root.inclusive_ms);
        if let Some(total) = self.total_ms.filter(|t| *t > 0.0) {
            shares(&mut self.root, total);
        }
        // Времена по узлам есть только при ANALYZE с TIMING; иначе горячие узлы — по стоимости, как у обычного EXPLAIN.
        let with_time = self.root.actual.is_some_and(|a| a.total_ms.is_some());
        let mut weights: Vec<(f64, u32)> = Vec::new();
        collect_weights(&self.root, with_time, &mut weights);
        weights.sort_by(|a, b| b.0.total_cmp(&a.0));
        let hot: Vec<u32> = weights
            .iter()
            .take(HOT_NODES)
            .filter(|(w, _)| *w > 0.0)
            .map(|(_, id)| *id)
            .collect();
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

/// `procs` — число процессов, выполняющих поддерево (1 вне Gather): время узла `total_ms × loops` делится на него.
fn compute_times(n: &mut PlanNode, procs: f64) {
    // Под Gather внешний потомок выполняют воркеры и ведущий; InitPlan / SubPlan самого Gather выполняются вне параллельной части.
    let is_gather = matches!(n.node_type.as_str(), "Gather" | "Gather Merge");
    let gather_procs = if is_gather {
        // Повторно запущенный Gather (loops = N) копит инструментацию воркеров по запускам: у потомка loops = N × процессы.
        let gather_loops = n
            .actual
            .map(|a| a.loops)
            .filter(|l| *l > 0.0)
            .unwrap_or(1.0);
        let child_loops = n
            .children
            .iter()
            .find(|c| c.parent_relationship.as_deref() == Some("Outer"))
            .or_else(|| n.children.iter().find(|c| !is_subplan(c)))
            .and_then(|c| c.actual)
            .map_or(1.0, |a| a.loops);
        (child_loops / gather_loops).max(1.0)
    } else {
        procs
    };
    for c in &mut n.children {
        let child_procs = if is_gather && !is_subplan(c) {
            gather_procs
        } else {
            procs
        };
        compute_times(c, child_procs);
    }
    if let Some(a) = n.actual {
        if let Some(total_ms) = a.total_ms {
            let inclusive = total_ms * a.loops / procs;
            let children: f64 = n.children.iter().filter_map(|c| c.inclusive_ms).sum();
            n.inclusive_ms = Some(inclusive);
            n.exclusive_ms = Some((inclusive - children).max(0.0));
        }
        // У не выполнявшегося узла сервер отдаёт 0 строк: сравнивать с оценкой нечего.
        n.estimate_ratio = if n.never_executed {
            None
        } else if n.plan_rows > 0.0 {
            Some(a.rows / n.plan_rows)
        } else if a.rows > 0.0 {
            Some(f64::INFINITY)
        } else {
            None
        };
        n.misestimated = n.estimate_ratio.is_some_and(|r| {
            (r >= MISESTIMATE_RATIO || r <= 1.0 / MISESTIMATE_RATIO)
                && n.plan_rows.max(a.rows) >= MISESTIMATE_MIN_ROWS
        });
    }
}

fn is_subplan(n: &PlanNode) -> bool {
    matches!(
        n.parent_relationship.as_deref(),
        Some("InitPlan" | "SubPlan")
    )
}

fn shares(n: &mut PlanNode, total: f64) {
    n.share_pct = n.exclusive_ms.map(|x| x / total * 100.0);
    for c in &mut n.children {
        shares(c, total);
    }
}

/// Вес узла для «горячих»: собственное время, без ANALYZE и при `TIMING OFF` — стоимость без детей.
fn collect_weights(n: &PlanNode, with_time: bool, out: &mut Vec<(f64, u32)>) {
    let w = if with_time {
        n.exclusive_ms.unwrap_or(0.0)
    } else {
        (n.total_cost - n.children.iter().map(|c| c.total_cost).sum::<f64>()).max(0.0)
    };
    out.push((w, n.id));
    for c in &n.children {
        collect_weights(c, with_time, out);
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
            let rr = RelationRef {
                schema: r.schema.clone(),
                name: r.name.clone(),
            };
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

#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use serde_json::json;

    fn share_sum(n: &crate::plan::PlanNode) -> f64 {
        n.share_pct.unwrap_or(0.0) + n.children.iter().map(share_sum).sum::<f64>()
    }

    #[test]
    fn time_under_gather_is_divided_by_process_count() {
        // Limit → Gather (2 воркера + ведущий) → Parallel Seq Scan: loops = 3, время — среднее на процесс.
        let v = json!({"Plan": {
            "Node Type": "Limit", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 10, "Plan Width": 4,
            "Actual Startup Time": 0.0, "Actual Total Time": 105.0, "Actual Rows": 10, "Actual Loops": 1,
            "Plans": [{
                "Node Type": "Gather", "Parent Relationship": "Outer", "Workers Planned": 2, "Workers Launched": 2,
                "Startup Cost": 0.0, "Total Cost": 9.0, "Plan Rows": 10, "Plan Width": 4,
                "Actual Startup Time": 0.0, "Actual Total Time": 100.0, "Actual Rows": 10, "Actual Loops": 1,
                "Plans": [{
                    "Node Type": "Seq Scan", "Parent Relationship": "Outer", "Parallel Aware": true,
                    "Relation Name": "orders", "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 100, "Plan Width": 4,
                    "Actual Startup Time": 0.0, "Actual Total Time": 90.0, "Actual Rows": 100, "Actual Loops": 3
                }]
            }]
        }, "Execution Time": 106.0});
        let t = from_json(&v).unwrap();
        let gather = &t.root.children[0];
        let scan = &gather.children[0];
        // Узел вне Gather — как раньше (total × loops); под Gather — total × loops / 3 процесса.
        assert_eq!(t.root.inclusive_ms, Some(105.0));
        assert_eq!(gather.inclusive_ms, Some(100.0));
        assert_eq!(scan.inclusive_ms, Some(90.0 * 3.0 / 3.0));
        assert_eq!(scan.exclusive_ms, Some(90.0));
        assert_eq!(gather.exclusive_ms, Some(10.0));
        assert_eq!(t.root.exclusive_ms, Some(5.0));
        assert!((scan.share_pct.unwrap() - 90.0 / 106.0 * 100.0).abs() < 1e-9);
        let sum = share_sum(&t.root);
        assert!(sum <= 100.0, "сумма долей {sum}");
        // Горячие — по скорректированному собственному времени: Scan (90), Gather (10), Limit (5).
        assert!(scan.hot && gather.hot && t.root.hot);
    }

    #[test]
    fn gather_initplan_runs_outside_the_parallel_part() {
        // InitPlan идёт в `Plans` раньше внешнего потомка и выполняется ведущим один раз: на число процессов не делится.
        let v = json!({"Plan": {
            "Node Type": "Gather", "Workers Planned": 1, "Workers Launched": 1,
            "Startup Cost": 0.0, "Total Cost": 9.0, "Plan Rows": 10, "Plan Width": 4,
            "Actual Startup Time": 0.0, "Actual Total Time": 50.0, "Actual Rows": 10, "Actual Loops": 1,
            "Plans": [
                {"Node Type": "Aggregate", "Strategy": "Plain", "Parent Relationship": "InitPlan", "Subplan Name": "InitPlan 1",
                 "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4,
                 "Actual Startup Time": 4.0, "Actual Total Time": 4.0, "Actual Rows": 1, "Actual Loops": 1},
                {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Parallel Aware": true, "Relation Name": "orders",
                 "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 100, "Plan Width": 4,
                 "Actual Startup Time": 0.0, "Actual Total Time": 40.0, "Actual Rows": 100, "Actual Loops": 2}
            ]
        }, "Execution Time": 51.0});
        let t = from_json(&v).unwrap();
        assert_eq!(
            t.root.children[0].inclusive_ms,
            Some(4.0),
            "InitPlan вне параллельной части"
        );
        assert_eq!(
            t.root.children[1].inclusive_ms,
            Some(40.0),
            "40 мс × 2 цикла / 2 процесса"
        );
        assert_eq!(t.root.exclusive_ms, Some(6.0));
        assert!(share_sum(&t.root) <= 100.0);
    }

    #[test]
    fn rescanned_gather_divides_by_processes_not_loops() {
        // Gather на внутренней стороне Nested Loop запускается 10 раз: у потомка loops = 10 запусков × 3 процесса = 30.
        let v = json!({"Plan": {
            "Node Type": "Nested Loop", "Join Type": "Inner", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 10, "Plan Width": 4,
            "Actual Startup Time": 0.0, "Actual Total Time": 105.0, "Actual Rows": 10, "Actual Loops": 1,
            "Plans": [
                {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "customers",
                 "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 10, "Plan Width": 4,
                 "Actual Startup Time": 0.0, "Actual Total Time": 1.0, "Actual Rows": 10, "Actual Loops": 1},
                {"Node Type": "Gather", "Parent Relationship": "Inner", "Workers Planned": 2, "Workers Launched": 2,
                 "Startup Cost": 0.0, "Total Cost": 9.0, "Plan Rows": 1, "Plan Width": 4,
                 "Actual Startup Time": 0.0, "Actual Total Time": 10.0, "Actual Rows": 1, "Actual Loops": 10,
                 "Plans": [{
                    "Node Type": "Seq Scan", "Parent Relationship": "Outer", "Parallel Aware": true, "Relation Name": "orders",
                    "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 1, "Plan Width": 4,
                    "Actual Startup Time": 0.0, "Actual Total Time": 9.0, "Actual Rows": 1, "Actual Loops": 30
                 }]}
            ]
        }, "Execution Time": 106.0});
        let t = from_json(&v).unwrap();
        let gather = &t.root.children[1];
        let scan = &gather.children[0];
        // 9 мс × 30 / (30 / 10) = 90; делить на все 30 циклов нельзя — вышло бы 9 и собственное время Gather 91.
        assert!(
            (scan.inclusive_ms.unwrap() - 90.0).abs() < 1e-9,
            "{:?}",
            scan.inclusive_ms
        );
        assert!((gather.inclusive_ms.unwrap() - 100.0).abs() < 1e-9);
        assert!(
            (gather.exclusive_ms.unwrap() - 10.0).abs() < 1e-9,
            "{:?}",
            gather.exclusive_ms
        );
        assert!(share_sum(&t.root) <= 100.0);
    }

    #[test]
    fn never_executed_node_is_not_misestimated() {
        // Узел не выполнялся (loops = 0): сервер отдаёт 0 строк, а план обещал 500 — это не ошибка оценки.
        let node = |loops: u32| {
            json!({"Plan": {"Node Type": "Seq Scan", "Relation Name": "orders",
                "Startup Cost": 0.0, "Total Cost": 8.0, "Plan Rows": 500, "Plan Width": 4,
                "Actual Startup Time": 0.0, "Actual Total Time": 0.0, "Actual Rows": 0, "Actual Loops": loops}})
        };
        let skipped = from_json(&node(0)).unwrap().root;
        assert!(skipped.never_executed);
        assert_eq!(skipped.estimate_ratio, None);
        assert!(!skipped.misestimated);
        // Выполнившийся узел с теми же цифрами — настоящая ошибка оценки: план 500, факт 0.
        let ran = from_json(&node(1)).unwrap().root;
        assert_eq!(ran.estimate_ratio, Some(0.0));
        assert!(ran.misestimated);
    }
}
