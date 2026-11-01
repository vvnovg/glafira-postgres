//! Правила сканирований и оценки (спец. P3 §9.1): Seq Scan с отсевом, Index Scan с фильтром, Nested Loop над
//! Seq Scan, ошибка оценки. Все — «факт»: без `actual` молчат.

use crate::findings::{Severity, thresholds as th};
use crate::plan::PlanNode;
use crate::plan_rules::{Recommendation, columns_of, index_recommendation, qualified, table_name};
use crate::rules::sessions::quote_ident;

/// Псевдоним отношения в условиях узла: `Alias`, иначе имя таблицы.
pub(super) fn alias(n: &PlanNode) -> Option<&str> {
    n.relation
        .as_ref()
        .map(|r| r.alias.as_deref().unwrap_or(r.name.as_str()))
}

/// Колонки из нескольких условий без повторов, в порядке условий.
fn columns_from(conds: &[Option<&str>], alias: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in conds.iter().flatten().flat_map(|c| columns_of(c, alias)) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Seq Scan: прочитано ≥ 10 000 строк (с учётом циклов), отсев ≥ 10 × возвращено.
pub fn seq_scan_filter(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Seq Scan" || n.never_executed {
        return None;
    }
    let a = n.actual?;
    // Сервер отдаёт строки и отсев на цикл.
    let removed = n.rows_removed_by_filter? as f64;
    let returned = a.rows;
    let read = (returned + removed) * a.loops;
    if read < th::PLAN_SEQ_SCAN_ROWS_READ || removed < th::PLAN_FILTER_RATIO * returned.max(1.0) {
        return None;
    }
    let cols = columns_from(&[n.filter.as_deref()], alias(n));
    let r = Recommendation::new("plan_seq_scan_filter", Severity::Warning, n.id)
        .param("table", table_name(n).unwrap_or_default())
        .param("rowsRead", read.round() as i64)
        .param("rowsReturned", (returned * a.loops).round() as i64)
        .param("loops", a.loops.round() as u64);
    Some(index_recommendation(r, n, &cols))
}

/// Index Scan / Bitmap Heap Scan с фильтром: отсев ≥ 10 × rows и ≥ 1000 — составной индекс (условие индекса + фильтр).
pub fn index_filter(n: &PlanNode) -> Option<Recommendation> {
    if !matches!(n.node_type.as_str(), "Index Scan" | "Bitmap Heap Scan") || n.never_executed {
        return None;
    }
    let a = n.actual?;
    let removed = n.rows_removed_by_filter? as f64;
    if removed < th::PLAN_INDEX_FILTER_MIN_REMOVED
        || removed < th::PLAN_FILTER_RATIO * a.rows.max(1.0)
    {
        return None;
    }
    let index_cond = n.index_cond.as_deref().or(n.recheck_cond.as_deref());
    let cols = columns_from(&[index_cond, n.filter.as_deref()], alias(n));
    let r = Recommendation::new("plan_index_filter", Severity::Warning, n.id)
        .param("table", table_name(n).unwrap_or_default())
        .param("index", n.index_name.clone().unwrap_or_default())
        .param("rowsRemoved", removed.round() as i64)
        .param("rowsReturned", a.rows.round() as i64)
        .param("loops", a.loops.round() as u64);
    Some(index_recommendation(r, n, &cols))
}

/// Nested Loop, внутренний ребёнок — Seq Scan с loops ≥ 1000: индекс по ключу соединения.
pub fn nested_loop_seq(n: &PlanNode) -> Option<Recommendation> {
    if n.node_type != "Nested Loop" {
        return None;
    }
    let inner = n
        .children
        .iter()
        .find(|c| c.parent_relationship.as_deref() == Some("Inner") && c.node_type == "Seq Scan")?;
    let a = inner.actual?;
    if a.loops < th::PLAN_NESTED_LOOP_LOOPS {
        return None;
    }
    let al = alias(inner);
    let mut cols = columns_from(&[inner.filter.as_deref()], al);
    if cols.is_empty() {
        cols = columns_from(&[n.join_filter.as_deref()], al);
    }
    let r = Recommendation::new("plan_nested_loop_seq", Severity::Warning, n.id)
        .also_node(inner.id)
        .param("table", table_name(inner).unwrap_or_default())
        .param("loops", a.loops.round() as u64)
        .param(
            "rowsPerLoop",
            (a.rows + inner.rows_removed_by_filter.unwrap_or(0) as f64).round() as i64,
        );
    Some(index_recommendation(r, inner, &cols))
}

/// Ошибка оценки ≥ ×10 при объёме ≥ 1000 строк у узла с отношением: ANALYZE, SET STATISTICS по колонкам условий, при
/// двух и более колонках — CREATE STATISTICS. `early_stop` — узел под `Limit` без блокирующего узла между ними: там
/// факт меньше оценки — не ошибка статистики (поправка 8 плана P3b), такую «переоценку» правило пропускает.
pub fn misestimate(n: &PlanNode, early_stop: bool) -> Option<Recommendation> {
    // Порог отношения — флаг P3a `misestimated` (×`plan::MISESTIMATE_RATIO`, равен `PLAN_MISESTIMATE_RATIO`; тест
    // `misestimate_threshold_matches_p3a_flag`): он же исключает не выполнявшийся узел и нулевые оценку и факт
    // (поправка 4). При объёме ≥ `PLAN_MISESTIMATE_MIN_ROWS` (≥ 1000 > 10) конечное `ratio` ниже не меньше отношения флага.
    if !n.misestimated {
        return None;
    }
    let a = n.actual?;
    let rel = n.relation.as_ref()?;
    if n.plan_rows.max(a.rows) < th::PLAN_MISESTIMATE_MIN_ROWS {
        return None;
    }
    let under = a.rows >= n.plan_rows;
    if early_stop && !under {
        return None;
    }
    // Конечное отношение (оценка 0 даёт бесконечность, а она в JSON — `null`).
    let ratio = a.rows.max(1.0) / n.plan_rows.max(1.0);
    let ratio = if ratio >= 1.0 { ratio } else { 1.0 / ratio };
    let cols = columns_from(&[n.filter.as_deref(), n.index_cond.as_deref()], alias(n));
    let target = qualified(rel.schema.as_deref(), &rel.name);
    let mut cmd = format!("ANALYZE {target};");
    for c in &cols {
        cmd.push_str(&format!(
            "\nALTER TABLE {target} ALTER COLUMN {} SET STATISTICS 1000;",
            quote_ident(c)
        ));
    }
    if cols.len() >= 2 {
        let list: Vec<String> = cols.iter().map(|c| quote_ident(c)).collect();
        // Имя статистики обязательно до PG 16 (`CREATE STATISTICS ON …` без имени — синтаксическая ошибка на 13–15);
        // схема — как у таблицы, иначе объект ляжет в первую схему `search_path`.
        let name = qualified(rel.schema.as_deref(), &statistics_name(&rel.name, &cols));
        cmd.push_str(&format!(
            "\nCREATE STATISTICS {name} ON {} FROM {target};",
            list.join(", ")
        ));
    }
    Some(
        Recommendation::new("plan_misestimate", Severity::Warning, n.id)
            .param("table", table_name(n).unwrap_or_default())
            .param("planRows", n.plan_rows.round() as i64)
            .param("actualRows", a.rows.round() as i64)
            .param("ratio", (ratio * 10.0).round() / 10.0)
            .param("direction", if under { "under" } else { "over" })
            .command(cmd),
    )
}

/// Имя расширенной статистики `<таблица>_<кол1>_<кол2>…_stat`, не длиннее `NAMEDATALEN - 1` = 63 байт: длинная
/// основа обрезается по границе символа, суффикс `_stat` остаётся.
pub(crate) fn statistics_name(table: &str, cols: &[String]) -> String {
    const MAX: usize = 63;
    const SUFFIX: &str = "_stat";
    let mut base = table.to_string();
    for c in cols {
        base.push('_');
        base.push_str(c);
    }
    let mut cut = base.len().min(MAX - SUFFIX.len());
    while !base.is_char_boundary(cut) {
        cut -= 1;
    }
    base.truncate(cut);
    base + SUFFIX
}

#[cfg(test)]
mod tests {
    use crate::plan::json::from_json;
    use crate::plan_rules::evaluate;
    use serde_json::{Value, json};

    fn ids(v: &Value) -> Vec<String> {
        evaluate(&from_json(v).unwrap())
            .recommendations
            .iter()
            .map(|r| r.id.clone())
            .collect()
    }

    fn seq(rows: f64, removed: i64, actual_rows: f64) -> Value {
        json!({"Plan": {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 100.0,
            "Plan Rows": rows, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": actual_rows, "Actual Loops": 1,
            "Filter": "(o.status = 'disputed'::text)", "Rows Removed by Filter": removed}})
    }

    #[test]
    fn seq_scan_with_heavy_filter_recommends_an_index_with_hypo() {
        let r = evaluate(&from_json(&seq(500.0, 499_500, 500.0)).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_seq_scan_filter")
            .expect("рекомендация");
        assert_eq!(rec.node_ids, [0]);
        assert_eq!(rec.params["rowsRead"], 500_000);
        assert_eq!(rec.params["rowsReturned"], 500);
        assert_eq!(rec.params["table"], "shop.orders");
        assert_eq!(
            rec.command.as_deref(),
            Some("CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");")
        );
        assert_eq!(
            rec.hypo.as_ref().map(|h| h.index_ddl.as_str()),
            Some("CREATE INDEX ON \"shop\".\"orders\" (\"status\")")
        );
        // Мало строк или малый отсев — молчит.
        assert!(!ids(&seq(500.0, 5_000, 500.0)).contains(&"plan_seq_scan_filter".to_string()));
        assert!(!ids(&seq(500.0, 2_000, 500.0)).contains(&"plan_seq_scan_filter".to_string()));
    }

    #[test]
    fn seq_scan_with_function_filter_has_template_command_without_hypo() {
        let mut v = seq(500.0, 499_500, 500.0);
        v["Plan"]["Filter"] = json!("(lower(o.status) = 'disputed'::text)");
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_seq_scan_filter")
            .unwrap();
        assert!(
            rec.command.as_deref().unwrap().contains("(<columns>)"),
            "{:?}",
            rec.command
        );
        assert!(rec.hypo.is_none());
    }

    #[test]
    fn index_scan_with_filter_recommends_a_composite_index() {
        let v = json!({"Plan": {"Node Type": "Index Scan", "Index Name": "orders_customer_idx", "Relation Name": "orders", "Schema": "shop", "Alias": "o",
            "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 10, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": 10, "Actual Loops": 1,
            "Index Cond": "(o.customer_id = 5)", "Filter": "(o.status = 'new'::text)", "Rows Removed by Filter": 5000}});
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_index_filter")
            .unwrap();
        assert_eq!(
            rec.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"customer_id\", \"status\")"
        );
        assert_eq!(rec.params["rowsRemoved"], 5000);
    }

    #[test]
    fn nested_loop_over_seq_scan_recommends_join_key_index() {
        let v = json!({"Plan": {"Node Type": "Nested Loop", "Join Type": "Inner", "Startup Cost": 0.0, "Total Cost": 1000.0, "Plan Rows": 100, "Plan Width": 8,
        "Actual Startup Time": 0.1, "Actual Total Time": 900.0, "Actual Rows": 100, "Actual Loops": 1, "Plans": [
          {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "audit", "Schema": "shop", "Alias": "a", "Index Name": "audit_order_idx", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 100, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 2000, "Actual Loops": 1, "Index Cond": "(a.order_id = 1)"},
          {"Node Type": "Seq Scan", "Parent Relationship": "Inner", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 5.0, "Plan Rows": 1, "Plan Width": 8, "Actual Startup Time": 0.0, "Actual Total Time": 0.4, "Actual Rows": 1, "Actual Loops": 2000, "Filter": "(o.id = a.order_id)", "Rows Removed by Filter": 499999}
        ]}});
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_nested_loop_seq")
            .unwrap();
        assert_eq!(rec.node_ids, [0, 2]);
        assert_eq!(rec.params["loops"], 2000);
        assert_eq!(
            rec.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"id\")"
        );
        // Оценка строк внешнего узла ошиблась в 20 раз — отдельная рекомендация ANALYZE.
        let mis = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_misestimate")
            .unwrap();
        assert_eq!(mis.node_ids, [1]);
        assert_eq!(mis.params["table"], "shop.audit");
        assert!(
            mis.command
                .as_deref()
                .unwrap()
                .starts_with("ANALYZE \"shop\".\"audit\";")
        );
    }

    #[test]
    fn misestimate_needs_volume_and_suggests_statistics_for_two_columns() {
        let mut v = seq(10.0, 0, 50.0);
        assert!(
            !ids(&v).contains(&"plan_misestimate".to_string()),
            "мало строк"
        );
        v = seq(100.0, 0, 5000.0);
        v["Plan"]["Filter"] = json!("((o.status = 'new'::text) AND (o.region = 'eu'::text))");
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_misestimate")
            .unwrap();
        assert_eq!(rec.params["ratio"], 50.0);
        let cmd = rec.command.as_deref().unwrap();
        assert!(cmd.contains("ANALYZE \"shop\".\"orders\";"));
        assert!(cmd.contains(
            "ALTER TABLE \"shop\".\"orders\" ALTER COLUMN \"status\" SET STATISTICS 1000;"
        ));
        assert!(
            cmd.contains(
                "CREATE STATISTICS \"shop\".\"orders_status_region_stat\" ON \"status\", \"region\" FROM \"shop\".\"orders\";"
            ),
            "{cmd}"
        );
    }

    #[test]
    fn statistics_name_is_capped_at_63_bytes_on_a_char_boundary() {
        use super::statistics_name;
        assert_eq!(
            statistics_name("t", &["a".into(), "b".into()]),
            "t_a_b_stat"
        );
        let long = "ж".repeat(40);
        let name = statistics_name(&long, &["col".into(), "other".into()]);
        assert!(name.len() <= 63, "{} байт", name.len());
        assert!(name.ends_with("_stat"), "{name}");
        assert!(name.starts_with("жж"), "{name}");
    }

    #[test]
    fn misestimate_over_under_limit_is_skipped_but_under_is_kept() {
        let limit = |plan: f64, actual: f64| {
            json!({"Plan": {"Node Type": "Limit", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 10, "Plan Width": 8,
                "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 10, "Actual Loops": 1, "Plans": [
                {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "orders", "Schema": "shop", "Alias": "o",
                 "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": plan, "Plan Width": 8, "Actual Startup Time": 0.1,
                 "Actual Total Time": 1.0, "Actual Rows": actual, "Actual Loops": 1}]}})
        };
        assert!(
            !ids(&limit(100_000.0, 10.0)).contains(&"plan_misestimate".to_string()),
            "Limit остановил чтение"
        );
        assert!(
            ids(&limit(10.0, 5_000.0)).contains(&"plan_misestimate".to_string()),
            "недооценка видна и под Limit"
        );
    }

    #[test]
    fn misestimate_with_zero_estimate_has_finite_ratio() {
        let mut v = seq(0.0, 0, 5000.0);
        v["Plan"]["Filter"] = json!("(o.status = 'x'::text)");
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_misestimate")
            .unwrap();
        assert_eq!(rec.params["ratio"], 5000.0);
        assert_eq!(rec.params["direction"], "under");
        assert!(
            !rec.command
                .as_deref()
                .unwrap()
                .contains("CREATE STATISTICS"),
            "одна колонка"
        );
    }

    #[test]
    fn misestimate_boundary_follows_the_p3a_flag() {
        // Оценка 1000: факт 9900 — ×9,9, молчит; 10 000 — ×10, срабатывает; переоценка ×10 (вне Limit) — тоже.
        assert!(!ids(&seq(1000.0, 0, 9_900.0)).contains(&"plan_misestimate".to_string()));
        let r = evaluate(&from_json(&seq(1000.0, 0, 10_000.0)).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_misestimate")
            .unwrap();
        assert_eq!(
            (
                rec.params["ratio"].as_f64(),
                rec.params["direction"].as_str()
            ),
            (Some(10.0), Some("under"))
        );
        assert!(!ids(&seq(9_900.0, 0, 1000.0)).contains(&"plan_misestimate".to_string()));
        assert!(ids(&seq(10_000.0, 0, 1000.0)).contains(&"plan_misestimate".to_string()));
    }

    #[test]
    fn misestimate_threshold_matches_p3a_flag() {
        assert_eq!(
            crate::findings::thresholds::PLAN_MISESTIMATE_RATIO,
            crate::plan::MISESTIMATE_RATIO
        );
        const {
            assert!(
                crate::findings::thresholds::PLAN_MISESTIMATE_MIN_ROWS
                    >= crate::plan::MISESTIMATE_MIN_ROWS
            )
        };
    }

    #[test]
    fn nested_loop_join_filter_key_on_the_right_side() {
        let v = json!({"Plan": {"Node Type": "Nested Loop", "Join Type": "Inner", "Startup Cost": 0.0, "Total Cost": 1000.0, "Plan Rows": 100, "Plan Width": 8,
        "Actual Startup Time": 0.1, "Actual Total Time": 900.0, "Actual Rows": 2000, "Actual Loops": 1, "Join Filter": "(a.order_id = o.id)", "Rows Removed by Join Filter": 999_000, "Plans": [
          {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "audit", "Schema": "shop", "Alias": "a", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 2000, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 2000, "Actual Loops": 1},
          {"Node Type": "Seq Scan", "Parent Relationship": "Inner", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 5.0, "Plan Rows": 500, "Plan Width": 8, "Actual Startup Time": 0.0, "Actual Total Time": 0.4, "Actual Rows": 500, "Actual Loops": 2000}
        ]}});
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_nested_loop_seq")
            .unwrap();
        assert_eq!(
            rec.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"id\")"
        );
        assert_eq!(rec.params["rowsPerLoop"], 500);
    }

    fn bitmap(removed: i64, rows: f64) -> Value {
        json!({"Plan": {"Node Type": "Bitmap Heap Scan", "Relation Name": "orders", "Schema": "shop", "Alias": "o",
            "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": rows, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 30.0, "Actual Rows": rows, "Actual Loops": 1,
            "Recheck Cond": "(o.customer_id = 5)", "Filter": "(o.status = 'new'::text)", "Rows Removed by Filter": removed, "Plans": [
            {"Node Type": "Bitmap Index Scan", "Parent Relationship": "Outer", "Index Name": "orders_customer_idx", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 5000, "Plan Width": 0,
             "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 5000, "Actual Loops": 1, "Index Cond": "(o.customer_id = 5)"}]}})
    }

    #[test]
    fn bitmap_heap_scan_filter_uses_recheck_cond_and_thresholds() {
        let r = evaluate(&from_json(&bitmap(5000, 10.0)).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_index_filter")
            .unwrap();
        assert_eq!(
            rec.hypo.as_ref().unwrap().index_ddl,
            "CREATE INDEX ON \"shop\".\"orders\" (\"customer_id\", \"status\")"
        );
        assert!(
            !ids(&bitmap(900, 10.0)).contains(&"plan_index_filter".to_string()),
            "отсев меньше 1000"
        );
        assert!(
            !ids(&bitmap(5000, 600.0)).contains(&"plan_index_filter".to_string()),
            "отсев меньше 10 × rows"
        );
    }

    #[test]
    fn never_executed_node_is_silent() {
        // Без проверки `never_executed` index_filter сработал бы: отсев не умножается на циклы.
        let mut v = bitmap(5000, 0.0);
        v["Plan"]["Actual Loops"] = json!(0);
        v["Plan"]["Plans"][0]["Actual Loops"] = json!(0);
        assert!(ids(&v).is_empty(), "{:?}", ids(&v));
    }

    #[test]
    fn seq_scan_rows_read_multiply_by_loops() {
        // 5 циклов по 3000 прочитанных строк: 15 000 ≥ 10 000, хотя один цикл — меньше порога.
        let mut v = seq(100.0, 2_900, 100.0);
        v["Plan"]["Actual Loops"] = json!(5);
        let r = evaluate(&from_json(&v).unwrap());
        let rec = r
            .recommendations
            .iter()
            .find(|r| r.id == "plan_seq_scan_filter")
            .unwrap();
        assert_eq!(
            (
                rec.params["rowsRead"].as_i64(),
                rec.params["rowsReturned"].as_i64(),
                rec.params["loops"].as_u64()
            ),
            (Some(15_000), Some(500), Some(5))
        );
        v["Plan"]["Actual Loops"] = json!(1);
        assert!(!ids(&v).contains(&"plan_seq_scan_filter".to_string()));
    }

    #[test]
    fn nested_loop_with_few_loops_is_silent() {
        let v = json!({"Plan": {"Node Type": "Nested Loop", "Join Type": "Inner", "Startup Cost": 0.0, "Total Cost": 1000.0, "Plan Rows": 10, "Plan Width": 8,
        "Actual Startup Time": 0.1, "Actual Total Time": 9.0, "Actual Rows": 10, "Actual Loops": 1, "Plans": [
          {"Node Type": "Index Scan", "Parent Relationship": "Outer", "Relation Name": "audit", "Schema": "shop", "Alias": "a", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 10, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 1.0, "Actual Rows": 10, "Actual Loops": 1},
          {"Node Type": "Seq Scan", "Parent Relationship": "Inner", "Relation Name": "orders", "Schema": "shop", "Alias": "o", "Startup Cost": 0.0, "Total Cost": 5.0, "Plan Rows": 1, "Plan Width": 8, "Actual Startup Time": 0.0, "Actual Total Time": 0.4, "Actual Rows": 1, "Actual Loops": 10, "Filter": "(o.id = a.order_id)", "Rows Removed by Filter": 99}
        ]}});
        assert!(!ids(&v).contains(&"plan_nested_loop_seq".to_string()));
    }
}
