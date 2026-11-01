//! Текстовый рендер дерева в стиле psql — для «Скопировать для LLM» и кнопки «Как текст».

use std::fmt::Write;

use serde_json::Value;

use crate::plan::{PlanNode, PlanTree};

pub fn to_text(t: &PlanTree) -> String {
    let mut out = String::new();
    node(&t.root, 0, &mut out);
    // `SETTINGS`: параметры, отличные от значений по умолчанию, — одной строкой после дерева (`ExplainPrintSettings`).
    if !t.settings.is_empty() {
        let list: Vec<String> = t
            .settings
            .iter()
            .map(|s| format!("{} = '{}'", s.name, s.value))
            .collect();
        let _ = writeln!(out, "Settings: {}", list.join(", "));
    }
    if let Some(p) = t.planning_ms {
        let _ = writeln!(out, "Planning Time: {p:.3} ms");
    }
    for tr in &t.triggers {
        let _ = writeln!(
            out,
            "Trigger {}: time={:.3} calls={}",
            tr.name, tr.time_ms, tr.calls
        );
    }
    if let Some(j) = &t.jit {
        let _ = writeln!(out, "JIT:");
        let _ = writeln!(out, "  Functions: {}", j.functions);
        // Строка времён — только когда они есть (ANALYZE с TIMING), как в psql.
        if j.total_ms.is_some() {
            let ms = |v: Option<f64>| v.unwrap_or(0.0);
            let _ = writeln!(
                out,
                "  Timing: Generation {:.3} ms, Inlining {:.3} ms, Optimization {:.3} ms, Emission {:.3} ms, Total {:.3} ms",
                ms(j.generation_ms),
                ms(j.inlining_ms),
                ms(j.optimization_ms),
                ms(j.emission_ms),
                ms(j.total_ms)
            );
        }
    }
    if let Some(e) = t.execution_ms {
        let _ = writeln!(out, "Execution Time: {e:.3} ms");
    }
    out
}

/// Строковое поле из `extra` (ключи, которые модель не разбирает: `Partial Mode`, `Operation`, `CTE Name` …).
fn extra_str<'a>(n: &'a PlanNode, key: &str) -> Option<&'a str> {
    n.extra.get(key).and_then(Value::as_str)
}

/// Имя соединения как в `ExplainNode`: тип вставляется перед «Join» (`Hash Left Join`), у внутреннего соединения
/// Nested Loop слова «Join» нет.
fn join_name(n: &PlanNode) -> String {
    let base = match n.node_type.as_str() {
        "Hash Join" => "Hash",
        "Merge Join" => "Merge",
        _ => "Nested Loop",
    };
    match n.join_type.as_deref() {
        Some(jt) if jt != "Inner" => format!("{base} {jt} Join"),
        _ if base != "Nested Loop" => format!("{base} Join"),
        _ => base.to_string(),
    }
}

/// Имя узла, как его печатает psql: стратегия агрегата и SetOp, `Partial` / `Finalize`, `Async` / `Parallel`,
/// тип соединения, операция ModifyTable.
fn node_name(n: &PlanNode) -> String {
    let strategy = n.strategy.as_deref();
    let mut name = match n.node_type.as_str() {
        "Aggregate" => match strategy {
            Some("Sorted") => "GroupAggregate".to_string(),
            Some("Hashed") => "HashAggregate".to_string(),
            Some("Mixed") => "MixedAggregate".to_string(),
            _ => "Aggregate".to_string(),
        },
        "SetOp" if strategy == Some("Hashed") => "HashSetOp".to_string(),
        "ModifyTable" => extra_str(n, "Operation")
            .unwrap_or("ModifyTable")
            .to_string(),
        "Hash Join" | "Merge Join" | "Nested Loop" => join_name(n),
        other => other.to_string(),
    };
    if n.node_type == "Aggregate"
        && let Some(mode @ ("Partial" | "Finalize")) = extra_str(n, "Partial Mode")
    {
        name = format!("{mode} {name}");
    }
    if n.extra.get("Async Capable").and_then(Value::as_bool) == Some(true) {
        name = format!("Async {name}");
    }
    if n.parallel_aware {
        name = format!("Parallel {name}");
    }
    name
}

/// Цель узла: индекс (`using idx`, `Backward using idx`, у Bitmap Index Scan — `on idx`) и отношение (`on schema.rel alias`);
/// у CTE / подзапроса / функции — их имя из `extra`.
fn target(n: &PlanNode, s: &mut String) {
    if let Some(i) = &n.index_name {
        if n.node_type == "Bitmap Index Scan" {
            let _ = write!(s, " on {i}");
        } else {
            if extra_str(n, "Scan Direction") == Some("Backward") {
                s.push_str(" Backward");
            }
            let _ = write!(s, " using {i}");
        }
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
        return;
    }
    let alias = extra_str(n, "Alias");
    let object = match n.node_type.as_str() {
        "CTE Scan" | "WorkTable Scan" => extra_str(n, "CTE Name"),
        "Function Scan" => extra_str(n, "Function Name"),
        _ => None,
    };
    match (object, alias) {
        (Some(o), Some(a)) if o != a => {
            let _ = write!(s, " on {o} {a}");
        }
        (Some(o), _) => {
            let _ = write!(s, " on {o}");
        }
        (None, Some(a))
            if matches!(
                n.node_type.as_str(),
                "Subquery Scan" | "Values Scan" | "Table Function Scan" | "Named Tuplestore Scan"
            ) =>
        {
            let _ = write!(s, " on {a}");
        }
        _ => {}
    }
}

fn head(n: &PlanNode) -> String {
    let mut s = node_name(n);
    target(n, &mut s);
    let _ = write!(
        s,
        "  (cost={:.2}..{:.2} rows={} width={})",
        n.startup_cost,
        n.total_cost,
        n.plan_rows.round() as i64,
        n.plan_width
    );
    if let Some(a) = n.actual {
        if n.never_executed {
            s.push_str(" (never executed)");
        } else {
            let rows = a.rows.round() as i64;
            let loops = a.loops.round() as i64;
            match (a.startup_ms, a.total_ms) {
                (Some(startup), Some(total)) => {
                    let _ = write!(
                        s,
                        " (actual time={startup:.3}..{total:.3} rows={rows} loops={loops})"
                    );
                }
                // `TIMING OFF`: времён нет, psql печатает только строки и циклы.
                _ => {
                    let _ = write!(s, " (actual rows={rows} loops={loops})");
                }
            }
        }
    }
    s
}

/// `indent` — отступ в парах пробелов, как `es->indent` в `ExplainNode`: у корня 0.
fn node(n: &PlanNode, indent: usize, out: &mut String) {
    let mut indent = indent;
    // Имя вложенного плана (`SubPlan 1`, `InitPlan 1`, `CTE d`) — отдельной строкой над узлом.
    if let Some(name) = &n.subplan_name {
        let _ = writeln!(out, "{}{name}", "  ".repeat(indent));
        indent += 1;
    }
    if indent > 0 {
        let _ = write!(out, "{}->  ", "  ".repeat(indent));
        indent += 2;
    }
    let _ = writeln!(out, "{}", head(n));
    indent += 1;
    let rest = "  ".repeat(indent);
    let mut line = |label: &str, v: &str| {
        let _ = writeln!(out, "{rest}{label}: {v}");
    };
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
            &format!(
                "{}  Batches: {}  Memory Usage: {}kB",
                n.hash_buckets.unwrap_or(0),
                b,
                n.peak_memory_kb.unwrap_or(0)
            ),
        );
    }
    if n.exact_heap_blocks.is_some() || n.lossy_heap_blocks.is_some() {
        line(
            "Heap Blocks",
            &format!(
                "exact={} lossy={}",
                n.exact_heap_blocks.unwrap_or(0),
                n.lossy_heap_blocks.unwrap_or(0)
            ),
        );
    }
    if let Some(h) = n.heap_fetches {
        line("Heap Fetches", &h.to_string());
    }
    if let Some(p) = n.workers_planned {
        line("Workers Planned", &p.to_string());
    }
    if let Some(l) = n.workers_launched {
        line("Workers Launched", &l.to_string());
    }
    if let Some(s) = n.subplans_removed {
        line("Subplans Removed", &s.to_string());
    }
    if let Some(b) = &n.buffers {
        let mut parts = Vec::new();
        let mut group = |name: &str, hit: i64, read: i64, dirtied: i64, written: i64| {
            let mut g = Vec::new();
            if hit > 0 {
                g.push(format!("hit={hit}"));
            }
            if read > 0 {
                g.push(format!("read={read}"));
            }
            if dirtied > 0 {
                g.push(format!("dirtied={dirtied}"));
            }
            if written > 0 {
                g.push(format!("written={written}"));
            }
            if !g.is_empty() {
                parts.push(format!("{name} {}", g.join(" ")));
            }
        };
        group(
            "shared",
            b.shared_hit,
            b.shared_read,
            b.shared_dirtied,
            b.shared_written,
        );
        group(
            "local",
            b.local_hit,
            b.local_read,
            b.local_dirtied,
            b.local_written,
        );
        if b.temp_read > 0 || b.temp_written > 0 {
            parts.push(format!(
                "temp read={} written={}",
                b.temp_read, b.temp_written
            ));
        }
        if !parts.is_empty() {
            line("Buffers", &parts.join(", "));
        }
    }
    if n.io_read_ms.is_some() || n.io_write_ms.is_some() {
        line(
            "I/O Timings",
            &format!(
                "read={:.3} write={:.3}",
                n.io_read_ms.unwrap_or(0.0),
                n.io_write_ms.unwrap_or(0.0)
            ),
        );
    }
    for c in &n.children {
        node(c, indent, out);
    }
}

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
        assert_eq!(
            lines[0],
            "Hash Join  (cost=10.00..100.00 rows=1000 width=8) (actual time=1.000..50.000 rows=10 loops=1)"
        );
        assert_eq!(lines[1], "  Hash Cond: (o.customer_id = c.id)");
        assert_eq!(lines[2], "  Buffers: shared hit=100 read=20");
        assert_eq!(
            lines[3],
            "  ->  Seq Scan on shop.orders o  (cost=0.00..60.00 rows=500 width=8) (actual time=0.100..30.000 rows=5 loops=1)"
        );
        assert_eq!(lines[4], "        Filter: (o.status = 'disputed'::text)");
        assert_eq!(lines[5], "        Rows Removed by Filter: 499995");
        assert_eq!(
            lines[6],
            "  ->  Index Scan using customers_pkey on shop.customers c  (cost=0.30..4.00 rows=100 width=8) (never executed)"
        );
        assert_eq!(lines[7], "        Index Cond: (c.id < 100)");
        assert_eq!(lines[8], "Planning Time: 0.500 ms");
        assert_eq!(lines[9], "Execution Time: 55.000 ms");
    }

    /// Минимальный узел для проверки заголовков: `extra` — дополнительные поля `(ключ, значение)`.
    fn n(
        node_type: &str,
        extra: serde_json::Value,
        plans: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut v = json!({"Node Type": node_type, "Startup Cost": 1.0, "Total Cost": 2.0, "Plan Rows": 3, "Plan Width": 4});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        if !plans.is_empty() {
            v["Plans"] = json!(plans);
        }
        v
    }

    fn heads(plan: serde_json::Value) -> Vec<String> {
        let text = to_text(&from_json(&json!({"Plan": plan})).unwrap());
        text.lines()
            .map(|l| l.split("  (cost=").next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn node_names_follow_explain_node() {
        let h = heads(n(
            "Hash Join",
            json!({"Join Type": "Left"}),
            vec![
                n(
                    "Nested Loop",
                    json!({"Join Type": "Anti", "Parent Relationship": "Outer"}),
                    vec![n(
                        "Nested Loop",
                        json!({"Join Type": "Inner", "Parent Relationship": "Outer"}),
                        vec![],
                    )],
                ),
                n(
                    "Merge Join",
                    json!({"Join Type": "Full", "Parent Relationship": "Inner"}),
                    vec![
                        n(
                            "Hash Join",
                            json!({"Join Type": "Inner", "Parent Relationship": "Outer"}),
                            vec![],
                        ),
                        n(
                            "Hash Join",
                            json!({"Join Type": "Right Anti", "Parent Relationship": "Inner"}),
                            vec![],
                        ),
                    ],
                ),
            ],
        ));
        assert_eq!(
            h,
            [
                "Hash Left Join",
                "  ->  Nested Loop Anti Join",
                "        ->  Nested Loop",
                "  ->  Merge Full Join",
                "        ->  Hash Join",
                "        ->  Hash Right Anti Join",
            ]
        );
        // Агрегаты и SetOp: стратегия задаёт имя, Partial Mode — приставку; Parallel — приставку к любому узлу.
        let h = heads(n(
            "Aggregate",
            json!({"Strategy": "Plain", "Partial Mode": "Finalize"}),
            vec![
                n(
                    "Gather",
                    json!({"Parent Relationship": "Outer", "Workers Planned": 2}),
                    vec![n(
                        "Aggregate",
                        json!({"Strategy": "Hashed", "Partial Mode": "Partial", "Parent Relationship": "Outer"}),
                        vec![n(
                            "Seq Scan",
                            json!({"Parallel Aware": true, "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Parent Relationship": "Outer"}),
                            vec![],
                        )],
                    )],
                ),
                n(
                    "Aggregate",
                    json!({"Strategy": "Sorted", "Partial Mode": "Simple", "Parent Relationship": "Outer"}),
                    vec![n(
                        "Aggregate",
                        json!({"Strategy": "Mixed", "Parent Relationship": "Outer"}),
                        vec![n(
                            "SetOp",
                            json!({"Strategy": "Hashed", "Parent Relationship": "Outer"}),
                            vec![n(
                                "SetOp",
                                json!({"Strategy": "Sorted", "Parent Relationship": "Outer"}),
                                vec![],
                            )],
                        )],
                    )],
                ),
            ],
        ));
        assert_eq!(
            h,
            [
                "Finalize Aggregate",
                "  ->  Gather",
                "        Workers Planned: 2",
                "        ->  Partial HashAggregate",
                "              ->  Parallel Seq Scan on shop.orders o",
                "  ->  GroupAggregate",
                "        ->  MixedAggregate",
                "              ->  HashSetOp",
                "                    ->  SetOp",
            ]
        );
    }

    #[test]
    fn index_scan_targets_and_subplan_headers() {
        let h = heads(n(
            "Index Scan",
            json!({"Scan Direction": "Backward", "Index Name": "orders_pkey", "Relation Name": "orders", "Alias": "o"}),
            vec![
                n(
                    "Bitmap Index Scan",
                    json!({"Index Name": "orders_idx", "Parent Relationship": "Outer"}),
                    vec![],
                ),
                n(
                    "Aggregate",
                    json!({"Strategy": "Plain", "Parent Relationship": "SubPlan", "Subplan Name": "SubPlan 1"}),
                    vec![n(
                        "Index Only Scan",
                        json!({"Scan Direction": "Forward", "Index Name": "i2", "Relation Name": "t", "Parent Relationship": "Outer"}),
                        vec![],
                    )],
                ),
            ],
        ));
        // Имя вложенного плана — отдельная строка над узлом, сам узел и его дети сдвинуты на два пробела.
        assert_eq!(
            h,
            [
                "Index Scan Backward using orders_pkey on orders o",
                "  ->  Bitmap Index Scan on orders_idx",
                "  SubPlan 1",
                "    ->  Aggregate",
                "          ->  Index Only Scan using i2 on t",
            ]
        );
    }

    #[test]
    fn workers_planned_without_launched_is_printed() {
        // Обычный EXPLAIN: воркеров запущено ещё нет, но план их заказывает.
        let text = to_text(
            &from_json(&json!({"Plan": n("Gather", json!({"Workers Planned": 2}), vec![])}))
                .unwrap(),
        );
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "Gather  (cost=1.00..2.00 rows=3 width=4)",
                "  Workers Planned: 2"
            ]
        );
    }

    /// `SETTINGS`: изменённые параметры — одной строкой после дерева и до времени планирования, как в psql; без них
    /// строки нет.
    #[test]
    fn settings_are_printed_like_psql() {
        let v = json!({"Plan": n("Result", json!({}), vec![]), "Settings": {"work_mem": "64MB", "search_path": "shop, public"}, "Planning Time": 0.1});
        let text = to_text(&from_json(&v).unwrap());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].starts_with("Result  (cost="), "{text}");
        assert!(
            lines[1] == "Settings: search_path = 'shop, public', work_mem = '64MB'"
                || lines[1] == "Settings: work_mem = '64MB', search_path = 'shop, public'",
            "{text}"
        );
        assert_eq!(lines[2], "Planning Time: 0.100 ms");
        let v = json!({"Plan": n("Result", json!({}), vec![]), "Settings": {}});
        assert!(!to_text(&from_json(&v).unwrap()).contains("Settings"));
    }

    /// Времена JIT есть только при ANALYZE с TIMING: без них psql печатает одну строку `Functions: N`.
    #[test]
    fn jit_timing_line_only_when_timings_exist() {
        let plain = json!({"Plan": n("Result", json!({}), vec![]),
            "JIT": {"Functions": 6, "Options": {"Inlining": true, "Optimization": true, "Expressions": true, "Deforming": true}}});
        let t = from_json(&plain).unwrap();
        let j = t.jit.unwrap();
        assert_eq!((j.functions, j.total_ms, j.generation_ms), (6, None, None));
        let text = to_text(&t);
        assert!(text.ends_with("JIT:\n  Functions: 6\n"), "{text}");
        assert!(!text.contains("Timing"), "{text}");
        // `ANALYZE, TIMING OFF`: Execution Time есть, времён JIT нет.
        let off = json!({"Plan": n("Result", json!({"Actual Rows": 1, "Actual Loops": 1}), vec![]),
            "JIT": {"Functions": 2, "Options": {"Inlining": false}}, "Execution Time": 3.0});
        let text = to_text(&from_json(&off).unwrap());
        assert!(
            text.contains("JIT:\n  Functions: 2\nExecution Time: 3.000 ms"),
            "{text}"
        );
        // С временами — строка `Timing:`, нулевые времена печатаются как нули.
        let on = json!({"Plan": n("Result", json!({}), vec![]),
            "JIT": {"Functions": 2, "Timing": {"Generation": 0.0, "Inlining": 0.0, "Optimization": 0.0, "Emission": 0.0, "Total": 0.0}}});
        let t = from_json(&on).unwrap();
        assert_eq!(t.jit.unwrap().total_ms, Some(0.0));
        assert!(to_text(&t).contains("  Timing: Generation 0.000 ms, Inlining 0.000 ms, Optimization 0.000 ms, Emission 0.000 ms, Total 0.000 ms"));
    }

    #[test]
    fn timing_off_prints_rows_and_loops_only() {
        let v = json!({"Plan": n("Seq Scan", json!({"Relation Name": "orders", "Actual Rows": 5, "Actual Loops": 2}), vec![])});
        let text = to_text(&from_json(&v).unwrap());
        assert_eq!(
            text.lines().next().unwrap(),
            "Seq Scan on orders  (cost=1.00..2.00 rows=3 width=4) (actual rows=5 loops=2)"
        );
    }
}
