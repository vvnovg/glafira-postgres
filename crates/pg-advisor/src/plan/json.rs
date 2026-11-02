//! Разбор `EXPLAIN (FORMAT JSON)`: массив из одного объекта или сам объект; незнакомые поля узла — в `extra`.

use serde_json::{Map, Value};

use crate::plan::{
    Actual, Buffers, Jit, PlanError, PlanNode, PlanTree, Relation, Setting, Trigger,
};

fn err(message: impl Into<String>) -> PlanError {
    PlanError {
        message: message.into(),
        line: None,
    }
}

/// Снимает обрамление psql (заголовок `QUERY PLAN`, разделитель, `(N rows)`, переносы `+` и рамки `|`) и разбирает JSON.
/// Снятые и пустые строки остаются пустыми: номер строки ошибки — в исходном тексте.
pub fn from_psql_text(text: &str) -> Result<Value, PlanError> {
    let mut body = String::new();
    for line in text.lines() {
        let t = line.trim();
        let footer = t.starts_with('(')
            && t.ends_with(')')
            && (t.ends_with(" row)") || t.ends_with(" rows)"));
        if !(t.is_empty() || t == "QUERY PLAN" || t.chars().all(|c| c == '-' || c == '+') || footer)
        {
            let t = t.trim_matches('|').trim();
            body.push_str(t.strip_suffix('+').map(str::trim_end).unwrap_or(t));
        }
        body.push('\n');
    }
    serde_json::from_str(&body).map_err(|e| PlanError {
        message: e.to_string(),
        line: Some(e.line().max(1)),
    })
}

/// Начало JSON-плана: строка `[` / `{` (у psql — с хвостом `+`), `[{…` / `{"…` (одной строкой, auto_explain с
/// `format = json`).
fn starts_json(line: &str) -> bool {
    let t = line.trim().trim_start_matches('|').trim_start();
    let mut chars = t.chars();
    matches!(chars.next(), Some('[' | '{'))
        && chars
            .find(|c| !c.is_whitespace())
            .is_none_or(|c| matches!(c, '{' | '"' | '+'))
}

/// Номер строки, где закрывается первая скобка JSON (скобки внутри строк JSON не считаются); не закрылась — `None`.
/// За записью auto_explain в выдержке журнала идут другие записи — они в JSON не входят.
fn json_end(lines: &[&str]) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (i, l) in lines.iter().enumerate() {
        for c in l.chars() {
            if in_str {
                match c {
                    _ if escaped => escaped = false,
                    '\\' => escaped = true,
                    '"' => in_str = false,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '[' | '{' => depth += 1,
                ']' | '}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Вставленный план любого вида: JSON (с обрамлением psql или без, в том числе после префикса записи auto_explain)
/// или текстовый EXPLAIN (`text_in`). JSON — если строка с `[` / `{` встречается раньше заголовка узла текстового плана
/// и раньше строки `Query Text:` текстовой записи auto_explain (в тексте запроса бывают строки `{` и `[`).
pub fn from_any(text: &str) -> Result<PlanTree, PlanError> {
    let lines: Vec<&str> = text.lines().collect();
    let json_at = lines.iter().position(|l| starts_json(l));
    let head_at = lines.iter().position(|l| {
        l.contains("(cost=")
            || l.contains("(actual ")
            || l.trim_start().starts_with("->")
            || l.trim_start().starts_with("Query Text:")
    });
    match json_at {
        Some(j) if head_at.is_none_or(|h| j < h) => {
            // Строки до JSON — заголовок psql или префикс журнала; номер строки ошибки — в исходном тексте.
            let end = json_end(&lines[j..]).map_or(lines.len(), |e| j + e + 1);
            let body = lines[j..end].join("\n");
            from_psql_text(&body)
                .and_then(|v| from_json(&v))
                .map_err(|e| PlanError {
                    line: e.line.map(|l| l + j),
                    ..e
                })
        }
        _ => crate::plan::text_in::from_text(text),
    }
}

pub fn from_json(v: &Value) -> Result<PlanTree, PlanError> {
    let top = match v {
        Value::Array(items) => items.first().ok_or_else(|| err("empty plan array"))?,
        other => other,
    };
    let obj = top
        .as_object()
        .ok_or_else(|| err("plan is not an object"))?;
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
                        relation: t
                            .get("Relation")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        time_ms: t.get("Time").and_then(Value::as_f64).unwrap_or(0.0),
                        calls: t.get("Calls").and_then(Value::as_i64).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let jit = obj.get("JIT").and_then(Value::as_object).map(|j| {
        let timing = j.get("Timing").and_then(Value::as_object);
        // С PG 17 `Generation` — объект `{"Deform": …, "Total": …}` (Deform входит в Total); раньше — число.
        // Без ANALYZE и при `TIMING OFF` блока `Timing` нет: времена — `None`, не ноль.
        let t = |k: &str| {
            timing.and_then(|t| t.get(k)).map(|v| {
                match v {
                    Value::Object(o) => o.get("Total").and_then(Value::as_f64),
                    other => other.as_f64(),
                }
                .unwrap_or(0.0)
            })
        };
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
                    value: v
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string()),
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
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(s)) => vec![s],
        _ => Vec::new(),
    }
}

fn node(v: &Value) -> Result<PlanNode, PlanError> {
    let mut m = v
        .as_object()
        .ok_or_else(|| err("plan node is not an object"))?
        .clone();
    let node_type =
        take_str(&mut m, "Node Type").ok_or_else(|| err("node without \"Node Type\""))?;
    let children = match m.remove("Plans") {
        Some(Value::Array(ps)) => ps.iter().map(node).collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    let relation = take_str(&mut m, "Relation Name").map(|name| Relation {
        schema: take_str(&mut m, "Schema"),
        name,
        alias: take_str(&mut m, "Alias"),
    });
    // Времена отсутствуют при `TIMING OFF`, но строки и циклы есть. Ключи снимаем всегда, чтобы они не попали в `extra`.
    let startup_ms = take_f64(&mut m, "Actual Startup Time");
    let total_ms = take_f64(&mut m, "Actual Total Time");
    let actual = match (
        take_f64(&mut m, "Actual Rows"),
        take_f64(&mut m, "Actual Loops"),
    ) {
        (Some(rows), Some(loops)) => Some(Actual {
            startup_ms,
            total_ms,
            rows,
            loops: loops.max(0.0),
        }),
        _ => None,
    };
    let never_executed = actual.is_some_and(|a| a.loops <= 0.0);
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
    let io_read_ms =
        take_f64(&mut m, "I/O Read Time").or_else(|| take_f64(&mut m, "Shared I/O Read Time"));
    let io_write_ms =
        take_f64(&mut m, "I/O Write Time").or_else(|| take_f64(&mut m, "Shared I/O Write Time"));
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
        assert_eq!(
            (t.planning_ms, t.execution_ms, t.total_ms),
            (Some(0.5), Some(55.0), Some(55.0))
        );
        assert_eq!(
            t.settings,
            vec![Setting {
                name: "work_mem".into(),
                value: "1MB".into()
            }]
        );
        assert_eq!(
            t.jit.as_ref().map(|j| (j.functions, j.total_ms)),
            Some((3, Some(6.0)))
        );
        let root = &t.root;
        assert_eq!(
            (root.id, root.node_type.as_str(), root.join_type.as_deref()),
            (0, "Hash Join", Some("Inner"))
        );
        assert_eq!(root.hash_cond.as_deref(), Some("(o.customer_id = c.id)"));
        assert_eq!(
            root.buffers.as_ref().map(|b| (b.shared_hit, b.shared_read)),
            Some((100, 20))
        );
        let seq = &root.children[0];
        assert_eq!((seq.id, seq.node_type.as_str()), (1, "Seq Scan"));
        assert_eq!(
            seq.relation.as_ref().map(|r| (
                r.schema.as_deref(),
                r.name.as_str(),
                r.alias.as_deref()
            )),
            Some((Some("shop"), "orders", Some("o")))
        );
        assert_eq!(
            (seq.rows_removed_by_filter, seq.filter.as_deref()),
            (Some(499_995), Some("(o.status = 'disputed'::text)"))
        );
        assert_eq!(
            seq.extra.get("Mystery Field"),
            Some(&json!(42)),
            "нераспознанное поле сохранено"
        );
        assert!(
            !seq.extra.contains_key("Filter"),
            "распознанное поле в extra не дублируется"
        );
        let hash = &root.children[1];
        assert_eq!(
            (hash.id, hash.hash_batches, hash.peak_memory_kb),
            (2, Some(1), Some(12))
        );
        let idx = &hash.children[0];
        assert_eq!(
            (idx.id, idx.index_name.as_deref(), idx.index_cond.as_deref()),
            (3, Some("customers_pkey"), Some("(c.id < 100)"))
        );
        assert_eq!(
            idx.actual.as_ref().map(|a| (a.rows, a.loops)),
            Some((100.0, 1.0))
        );
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
        assert_eq!(
            (seq.inclusive_ms, seq.exclusive_ms),
            (Some(30.0), Some(30.0))
        );
        // Оценка строк: 500 против факта 5 — ошибка ×100.
        assert_eq!(seq.estimate_ratio, Some(0.01));
        assert!(seq.misestimated && !root.children[1].children[0].misestimated);
        // Горячие — до трёх узлов с наибольшим собственным временем и долей ≥ 10 %: Seq Scan (30 из 55), корень (19,5);
        // Index Scan (0,4) — третий по времени, но меньше 10 %.
        let hot: Vec<i32> = [root, seq, &root.children[1], &root.children[1].children[0]]
            .iter()
            .map(|n| n.hot as i32)
            .collect();
        assert_eq!(hot, [1, 1, 0, 0]);
        assert!(!seq.never_executed);
    }

    #[test]
    fn plain_explain_without_analyze_uses_costs() {
        let mut v = fixture();
        fn strip(n: &mut serde_json::Value) {
            let o = n.as_object_mut().unwrap();
            for k in [
                "Actual Startup Time",
                "Actual Total Time",
                "Actual Rows",
                "Actual Loops",
            ] {
                o.remove(k);
            }
            if let Some(ps) = o.get_mut("Plans").and_then(|p| p.as_array_mut()) {
                ps.iter_mut().for_each(strip);
            }
        }
        strip(&mut v[0]["Plan"]);
        v[0].as_object_mut().unwrap().remove("Execution Time");
        let t = from_json(&v).unwrap();
        assert!(
            t.root.actual.is_none() && t.root.inclusive_ms.is_none() && t.root.share_pct.is_none()
        );
        assert_eq!(t.execution_ms, None);
        // Без факта «горячие» — по стоимости без детей: Seq Scan (60), корень (100 − 60 − 5 = 35); Index Scan (4 − 0 = 4)
        // и Hash (5 − 4 = 1) — меньше 10 % суммы и не горячие.
        assert!(t.root.children[0].hot && t.root.hot);
        assert!(!t.root.children[1].hot && !t.root.children[1].children[0].hot);
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
    fn from_any_detects_json_and_text() {
        let json = "[{\"Plan\": {\"Node Type\": \"Result\", \"Startup Cost\": 0, \"Total Cost\": 0, \"Plan Rows\": 1, \"Plan Width\": 1}}]";
        assert_eq!(from_any(json).unwrap().root.node_type, "Result");
        let psql = "   QUERY PLAN\n--------------\n [           +\n   {\"Plan\": {\"Node Type\": \"Limit\"}}+\n ]\n(1 row)\n";
        assert_eq!(from_any(psql).unwrap().root.node_type, "Limit");
        // auto_explain с `format = json`: префикс журнала, затем объект.
        let log = "2026-11-01 10:00:00 UTC [1] LOG:  duration: 1.000 ms  plan:\n\t{\n\t  \"Query Text\": \"SELECT 1\",\n\t  \"Plan\": {\"Node Type\": \"Result\"}\n\t}\n";
        assert_eq!(from_any(log).unwrap().root.node_type, "Result");
        let text =
            " QUERY PLAN\n------------\n Result  (cost=0.00..0.01 rows=1 width=4)\n(1 row)\n";
        assert_eq!(from_any(text).unwrap().root.node_type, "Result");
        // Ошибка JSON — с номером строки исходного текста (после двух строк заголовка).
        let e = from_any(" QUERY PLAN\n----\n [\n   {\"Plan\": oops}\n ]\n").unwrap_err();
        assert_eq!(e.line, Some(4), "{e:?}");
        let e = from_any("garbage").unwrap_err();
        assert_eq!(e.line, Some(1));
    }

    #[test]
    fn relations_and_modify_table() {
        let t = from_json(&fixture()).unwrap();
        let rels: Vec<String> = crate::plan::relations(&t)
            .iter()
            .map(|r| format!("{}.{}", r.schema.as_deref().unwrap_or(""), r.name))
            .collect();
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

    #[test]
    fn jit_generation_object_of_pg17_uses_total() {
        // PG 17+: `Generation` — объект, `Deform` уже входит в `Total` (не складывать).
        let v = json!({"Plan": {"Node Type": "Result", "Startup Cost": 0.0, "Total Cost": 0.01, "Plan Rows": 1, "Plan Width": 4},
            "JIT": {"Functions": 5, "Timing": {"Generation": {"Deform": 0.109, "Total": 0.469}, "Inlining": 1.0, "Optimization": 2.0, "Emission": 3.0, "Total": 6.469}}});
        let j = from_json(&v).unwrap().jit.unwrap();
        assert_eq!(
            (j.functions, j.generation_ms, j.total_ms),
            (5, Some(0.469), Some(6.469))
        );
        assert_eq!(
            (j.inlining_ms, j.optimization_ms, j.emission_ms),
            (Some(1.0), Some(2.0), Some(3.0))
        );
    }

    #[test]
    fn timing_off_keeps_rows_and_loops_but_not_times() {
        // `EXPLAIN (ANALYZE, TIMING OFF)`: у узлов нет Actual Startup / Total Time, строки и циклы есть.
        let v = json!([{"Plan": {
            "Node Type": "Hash Join", "Join Type": "Inner", "Startup Cost": 10.0, "Total Cost": 100.0, "Plan Rows": 1000, "Plan Width": 8,
            "Actual Rows": 10, "Actual Loops": 1,
            "Plans": [
              {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders",
               "Startup Cost": 0.0, "Total Cost": 60.0, "Plan Rows": 500, "Plan Width": 8, "Actual Rows": 5, "Actual Loops": 1},
              {"Node Type": "Index Scan", "Parent Relationship": "Inner", "Relation Name": "customers", "Index Name": "customers_pkey",
               "Startup Cost": 0.3, "Total Cost": 4.0, "Plan Rows": 100, "Plan Width": 8, "Actual Rows": 100, "Actual Loops": 1}
            ]}, "Planning Time": 0.5, "Execution Time": 55.0}]);
        let t = from_json(&v).unwrap();
        let seq = &t.root.children[0];
        let a = seq.actual.unwrap();
        assert_eq!(
            (a.startup_ms, a.total_ms, a.rows, a.loops),
            (None, None, 5.0, 1.0)
        );
        assert!(seq.extra.is_empty(), "{:?}", seq.extra);
        // Времён по узлам нет — нет и производных; Execution Time у сервера остаётся.
        assert_eq!(t.execution_ms, Some(55.0));
        assert!(
            t.root.inclusive_ms.is_none()
                && t.root.exclusive_ms.is_none()
                && t.root.share_pct.is_none()
        );
        assert!(seq.inclusive_ms.is_none() && seq.share_pct.is_none());
        // Оценка строк считается и без времён: 500 → 5, ошибка ×100.
        assert_eq!(seq.estimate_ratio, Some(0.01));
        assert!(seq.misestimated);
        // Горячие — по стоимости без детей, как у обычного EXPLAIN: Seq Scan (60), корень (100 − 64 = 36); Index Scan (4) —
        // меньше 10 %.
        assert!(seq.hot && t.root.hot && !t.root.children[1].hot);
    }

    #[test]
    fn query_text_lines_do_not_switch_to_json() {
        // Многострочный текст запроса auto_explain со строкой `{` — всё равно текстовый план.
        let text = "2026-11-01 10:00:00 UTC [1] LOG:  duration: 1.000 ms  plan:\n\
            \tQuery Text: SELECT '\n\
            \t{\n\
            \t\"a\": 1}'::jsonb\n\
            \tResult  (cost=0.00..0.01 rows=1 width=32)\n";
        let t = from_any(text).unwrap();
        assert_eq!(t.root.node_type, "Result");
        assert_eq!(
            t.root.extra["Query Text"],
            json!("SELECT '\n{\n\"a\": 1}'::jsonb")
        );
    }

    #[test]
    fn json_error_lines_count_blank_lines() {
        let e = from_any("[\n\n\n  {\"Plan\": oops}\n]").unwrap_err();
        assert_eq!(e.line, Some(4), "{e:?}");
        let e = from_psql_text("\n\n{\"Plan\": oops}").unwrap_err();
        assert_eq!(e.line, Some(3), "{e:?}");
    }

    #[test]
    fn auto_explain_json_entry_ends_where_brackets_balance() {
        // За записью auto_explain в выдержке журнала — следующие записи; строки со скобками внутри строк JSON не считаются.
        let log = "2026-11-01 10:00:00 UTC [1] LOG:  duration: 1.000 ms  plan:\n\
            \t{\n\
            \t  \"Query Text\": \"SELECT '}{' \\\"x\\\"\",\n\
            \t  \"Plan\": {\"Node Type\": \"Result\"}\n\
            \t}\n\
            2026-11-01 10:00:01 UTC [1] LOG:  statement: SELECT 2\n";
        let t = from_any(log).unwrap();
        // Скобки внутри строки `Query Text` конец JSON не сдвигают: иначе разбор упал бы на обрезанном объекте.
        assert_eq!(t.root.node_type, "Result");
    }
}
