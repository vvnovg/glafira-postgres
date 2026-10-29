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
            (r >= MISESTIMATE_RATIO || r <= 1.0 / MISESTIMATE_RATIO)
                && n.plan_rows.max(a.rows) >= MISESTIMATE_MIN_ROWS
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
