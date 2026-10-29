//! Текстовый рендер дерева в стиле psql — для «Скопировать для LLM» и кнопки «Как текст».

use std::fmt::Write;

use crate::plan::{PlanNode, PlanTree};

pub fn to_text(t: &PlanTree) -> String {
    let mut out = String::new();
    node(&t.root, 0, &mut out);
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
        let _ = writeln!(
            out,
            "  Timing: Generation {:.3} ms, Inlining {:.3} ms, Optimization {:.3} ms, Emission {:.3} ms, Total {:.3} ms",
            j.generation_ms, j.inlining_ms, j.optimization_ms, j.emission_ms, j.total_ms
        );
    }
    if let Some(e) = t.execution_ms {
        let _ = writeln!(out, "Execution Time: {e:.3} ms");
    }
    out
}

fn head(n: &PlanNode) -> String {
    let mut s = n.node_type.clone();
    if let Some(st) = &n.strategy
        && st != "Plain"
    {
        s = format!("{st}{s}");
    }
    if let Some(i) = &n.index_name {
        let _ = write!(s, " using {i}");
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
    }
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
            let _ = write!(
                s,
                " (actual time={:.3}..{:.3} rows={} loops={})",
                a.startup_ms,
                a.total_ms,
                a.rows.round() as i64,
                a.loops
            );
        }
    }
    s
}

fn node(n: &PlanNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth * 3);
    let (first, rest) = if depth == 0 {
        (String::new(), "  ".to_string())
    } else {
        (
            format!("{}  ->  ", "  ".repeat(depth * 3 - 3)),
            format!("{indent}  "),
        )
    };
    let _ = writeln!(out, "{first}{}", head(n));
    let mut line = |label: &str, v: &str| {
        let _ = writeln!(out, "{rest}{label}: {v}");
    };
    if let Some(name) = &n.subplan_name {
        line("Subplan Name", name);
    }
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
    if let (Some(p), Some(l)) = (n.workers_planned, n.workers_launched) {
        line("Workers Planned", &p.to_string());
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
        node(c, depth + 1, out);
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
}
