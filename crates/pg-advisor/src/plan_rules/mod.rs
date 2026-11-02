//! Рекомендации по плану (спец. P3 §9.1): правила над `PlanTree`, доказательство — цифры узла, готовая команда
//! (приложение её не выполняет), индекс-гипотеза для hypopg. Тексты — во фронтенде по `id` и `params`.

mod exec;
mod scan;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::findings::Severity;
pub use crate::plan::early_stop_nodes;
use crate::plan::{PlanNode, PlanTree};
use crate::rules::sessions::quote_ident;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HypoIndex {
    /// `CREATE INDEX ON …` без `CONCURRENTLY` — так принимает `hypopg_create_index`.
    pub index_ddl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    /// Ключ текстов `recommend.<id>.{title,evidence,effect,risks}`.
    pub id: String,
    pub severity: Severity,
    /// Узлы плана, к которым относится рекомендация («Показать узел»).
    pub node_ids: Vec<u32>,
    pub params: Map<String, Value>,
    /// Готовая команда для копирования; `None` — совет без команды.
    pub command: Option<String>,
    pub hypo: Option<HypoIndex>,
}

impl Recommendation {
    pub fn new(id: &str, severity: Severity, node_id: u32) -> Self {
        Self {
            id: id.to_string(),
            severity,
            node_ids: vec![node_id],
            params: Map::new(),
            command: None,
            hypo: None,
        }
    }

    pub fn also_node(mut self, id: u32) -> Self {
        self.node_ids.push(id);
        self
    }

    pub fn param(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.params.insert(key.to_string(), value.into());
        self
    }

    pub fn command(mut self, c: impl Into<String>) -> Self {
        self.command = Some(c.into());
        self
    }

    pub fn hypo(mut self, ddl: impl Into<String>) -> Self {
        self.hypo = Some(HypoIndex {
            index_ddl: ddl.into(),
        });
        self
    }
}

/// Итог: рекомендации и правила «факт», пропущенные без ANALYZE.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub recommendations: Vec<Recommendation>,
    pub skipped: Vec<String>,
}

/// Правила, которым нужен факт выполнения; `plan_sort_limit` и `plan_no_pruning` работают и по оценке.
pub const FACT_RULES: [&str; 9] = [
    "plan_seq_scan_filter",
    "plan_index_filter",
    "plan_nested_loop_seq",
    "plan_misestimate",
    "plan_spill",
    "plan_heap_fetches",
    "plan_workers_short",
    "plan_subplan_per_row",
    "plan_jit_heavy",
];

pub fn evaluate(t: &PlanTree) -> Report {
    let with_actual = t.root.actual.is_some();
    let mut out = Vec::new();
    walk(&t.root, &mut |n| {
        if with_actual {
            out.extend(scan::seq_scan_filter(n));
            out.extend(scan::index_filter(n));
            out.extend(scan::nested_loop_seq(n));
            out.extend(scan::misestimate(n));
            out.extend(exec::spill(n));
            out.extend(exec::heap_fetches(n));
            out.extend(exec::workers_short(n));
            out.extend(exec::subplan_per_row(n));
        }
        out.extend(exec::sort_limit(n));
        out.extend(exec::no_pruning(n, with_actual));
    });
    if with_actual {
        out.extend(exec::jit_heavy(t));
    }
    let mut out = group_partitions(&t.root, out);
    out.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.node_ids.cmp(&b.node_ids))
            .then_with(|| a.id.cmp(&b.id))
    });
    Report {
        recommendations: out,
        skipped: if with_actual {
            Vec::new()
        } else {
            FACT_RULES.iter().map(|s| s.to_string()).collect()
        },
    }
}

/// Правила по партициям, которые группируются под одним Append / Merge Append.
const PARTITION_GROUPED: [&str; 1] = ["plan_seq_scan_filter"];

/// Одинаковые рекомендации по партициям одного Append / Merge Append — одна рекомендация: узлы — все сканирования,
/// `partitions` — их число, строки суммируются, команды индексов — по строке на партицию (без повторов). Гипотеза
/// hypopg — одна DDL, поэтому у группы её нет. `plan_no_pruning` остаётся отдельной рекомендацией.
fn group_partitions(root: &PlanNode, recs: Vec<Recommendation>) -> Vec<Recommendation> {
    let mut groups: Vec<HashSet<u32>> = Vec::new();
    walk(root, &mut |n| {
        if matches!(n.node_type.as_str(), "Append" | "Merge Append") {
            groups.push(
                n.children
                    .iter()
                    .filter(|c| c.relation.is_some())
                    .map(|c| c.id)
                    .collect(),
            );
        }
    });
    let mut recs: Vec<Option<Recommendation>> = recs.into_iter().map(Some).collect();
    let mut out = Vec::new();
    for members in &groups {
        for rule in PARTITION_GROUPED {
            let idx: Vec<usize> = recs
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.as_ref().is_some_and(|r| {
                        r.id == rule && r.node_ids.len() == 1 && members.contains(&r.node_ids[0])
                    })
                })
                .map(|(i, _)| i)
                .collect();
            if idx.len() < 2 {
                continue;
            }
            let parts: Vec<Recommendation> = idx.iter().filter_map(|&i| recs[i].take()).collect();
            out.push(merge_partitions(parts));
        }
    }
    out.extend(recs.into_iter().flatten());
    out
}

fn merge_partitions(parts: Vec<Recommendation>) -> Recommendation {
    let first = &parts[0];
    let severity = parts
        .iter()
        .map(|p| p.severity)
        .max()
        .unwrap_or(first.severity);
    let mut r = Recommendation::new(&first.id, severity, first.node_ids[0]);
    r.node_ids = parts.iter().map(|p| p.node_ids[0]).collect();
    r.node_ids.sort();
    let sum = |key: &str| -> i64 {
        parts
            .iter()
            .filter_map(|p| p.params.get(key)?.as_i64())
            .sum()
    };
    let tables: Vec<&str> = parts
        .iter()
        .filter_map(|p| p.params.get("table")?.as_str())
        .collect();
    r = r
        .param("partitions", parts.len() as i64)
        .param("table", tables.first().copied().unwrap_or_default())
        .param("tables", tables.join(", "))
        .param("rowsRead", sum("rowsRead"))
        .param("rowsReturned", sum("rowsReturned"))
        .param(
            "loops",
            parts
                .iter()
                .filter_map(|p| p.params.get("loops")?.as_u64())
                .max()
                .unwrap_or(0),
        );
    if let Some(cols) = parts.iter().find_map(|p| p.params.get("columns")) {
        r = r.param("columns", cols.clone());
    }
    let mut cmds: Vec<&str> = Vec::new();
    for c in parts.iter().filter_map(|p| p.command.as_deref()) {
        if !cmds.contains(&c) {
            cmds.push(c);
        }
    }
    if !cmds.is_empty() {
        r = r.command(cmds.join("\n"));
    }
    r
}

/// Обход в глубину.
pub fn walk<'a>(n: &'a PlanNode, f: &mut impl FnMut(&'a PlanNode)) {
    f(n);
    for c in &n.children {
        walk(c, f);
    }
}

/// Полное имя отношения для текста и команд.
pub fn table_name(n: &PlanNode) -> Option<String> {
    n.relation.as_ref().map(|r| match &r.schema {
        Some(s) => format!("{s}.{}", r.name),
        None => r.name.clone(),
    })
}

pub fn qualified(schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(table)),
        None => quote_ident(table),
    }
}

/// Позиция за закрывающей кавычкой `q`, начиная с `i` (за открывающей); удвоенная кавычка — экранирование.
fn skip_quoted(b: &[u8], mut i: usize, q: u8) -> usize {
    while i < b.len() {
        if b[i] == q {
            if b.get(i + 1) == Some(&q) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Делит условие на конъюнкты по ` AND ` вне скобок и кавычек; внешние скобки и вложенные группы `AND` раскрываются.
fn conjuncts(cond: &str) -> Vec<&str> {
    let s = strip_parens(cond);
    let b = s.as_bytes();
    let mut parts = Vec::new();
    let (mut depth, mut start, mut i) = (0i32, 0usize, 0usize);
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            q @ (b'\'' | b'"') => {
                i = skip_quoted(b, i + 1, q);
                continue;
            }
            _ if depth == 0 && s[i..].starts_with(" AND ") => {
                parts.push(&s[start..i]);
                i += 5;
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    if parts.len() == 1 {
        return parts;
    }
    parts.into_iter().flat_map(conjuncts).collect()
}

/// Снимает внешние скобки, охватывающие всю строку.
fn strip_parens(s: &str) -> &str {
    let mut s = s.trim();
    while s.starts_with('(') && closing(s) == Some(s.len() - 1) {
        s = s[1..s.len() - 1].trim();
    }
    s
}

/// Индекс скобки, закрывающей открывающую в начале строки (кавычки учитываются).
fn closing(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0i32, 0usize);
    while i < b.len() {
        match b[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            q @ (b'\'' | b'"') => {
                i = skip_quoted(b, i + 1, q);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Идентификатор в начале строки: простой или в кавычках; возвращает имя и остаток.
fn ident(s: &str) -> Option<(String, &str)> {
    if s.starts_with('"') {
        let end = skip_quoted(s.as_bytes(), 1, b'"');
        if end < 2 || s.as_bytes()[end - 1] != b'"' {
            return None;
        }
        return Some((s[1..end - 1].replace("\"\"", "\""), &s[end..]));
    }
    let first = s.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(s.len());
    Some((s[..end].to_string(), &s[end..]))
}

/// `[alias.]col` своего псевдонима: чужой квалификатор — `None`.
fn column_ref<'s>(s: &'s str, alias: Option<&str>) -> Option<(String, &'s str)> {
    let (first, rest) = ident(s)?;
    match rest.strip_prefix('.') {
        Some(r) if alias == Some(first.as_str()) => ident(r),
        Some(_) => None,
        None => Some((first, rest)),
    }
}

const OPERATORS: [&str; 13] = [
    "=", "<>", "!=", "<=", ">=", "<", ">", "~~", "!~~", "~~*", "!~~*", "IS", "IN",
];

/// Колонка конъюнкта `col op …` (в том числе `alias.col` своего псевдонима и `(col)::type`) и признак равенства; для
/// ключа соединения `other.col op alias.col` — колонка правой стороны.
fn conjunct_column(c: &str, alias: Option<&str>) -> Option<(String, bool)> {
    let s = strip_parens(c);
    leading_column(s, alias).or_else(|| join_key_rhs(s, alias?))
}

/// Ключ соединения `other.col op alias.col[::type]`: слева — колонка чужого псевдонима, справа — своего (так сервер
/// печатает `Join Filter`, когда внутренняя сторона стоит справа).
fn join_key_rhs(s: &str, alias: &str) -> Option<(String, bool)> {
    let (first, rest) = ident(s)?;
    if first == alias {
        return None;
    }
    let (_, rest) = ident(rest.strip_prefix('.')?)?;
    let (op, rhs) = rest.trim_start().split_once(' ')?;
    if !OPERATORS.contains(&op) || matches!(op, "IS" | "IN") {
        return None;
    }
    let rhs = strip_parens(rhs);
    // `(alias.col)::type` — колонка в скобках, приведение после.
    let (rhs, after) = match closing(rhs).filter(|_| rhs.starts_with('(')) {
        Some(j) => (strip_parens(&rhs[1..j]), &rhs[j + 1..]),
        None => (rhs, ""),
    };
    let (q, r) = ident(rhs)?;
    if q != alias {
        return None;
    }
    let (col, tail) = ident(r.strip_prefix('.')?)?;
    if !after.is_empty() && !tail.trim().is_empty() {
        return None;
    }
    let tail = if after.is_empty() {
        tail.trim()
    } else {
        after.trim()
    };
    let cast_only = tail.strip_prefix("::").is_some_and(text_like);
    (tail.is_empty() || cast_only).then(|| (col, op == "="))
}

/// Ведущая колонка конъюнкта `col op …` своего псевдонима.
fn leading_column(s: &str, alias: Option<&str>) -> Option<(String, bool)> {
    let (col, rest) = if s.starts_with('(') {
        // `(col)::type op …` — приведение снимается.
        let j = closing(s)?;
        let (col, tail) = column_ref(strip_parens(&s[1..j]), alias)?;
        if !tail.trim().is_empty() || !s[j + 1..].starts_with("::") {
            return None;
        }
        (col, &s[j + 1..])
    } else {
        column_ref(s, alias)?
    };
    // Приведение после колонки: `::type` (тип может быть из нескольких слов) — до первого оператора.
    let mut tokens = rest.split_whitespace();
    let mut tok = tokens.next()?;
    if let Some(first) = tok.strip_prefix("::") {
        let mut ty = first.to_string();
        loop {
            tok = tokens.next()?;
            if OPERATORS.contains(&tok) {
                break;
            }
            if !tok
                .chars()
                .all(|c| c.is_alphanumeric() || "_[]\"().,".contains(c))
            {
                return None;
            }
            ty.push(' ');
            ty.push_str(tok);
        }
        if !text_like(&ty) {
            return None;
        }
    }
    OPERATORS.contains(&tok).then(|| (col, tok == "="))
}

/// Приведение к строковому типу (`text`, `character varying`/`varchar`, `bpchar`/`character`, `name`) — неявное
/// приведение планировщика, индекс по колонке ему служит. Приведение к любому другому типу (`date`, `integer`, …)
/// делает конъюнкт выражением: колонки нет.
fn text_like(ty: &str) -> bool {
    let ty = ty.trim().replace('"', "").to_lowercase();
    let ty = match ty.find('(') {
        Some(i) => ty[..i].trim_end(),
        None => ty.as_str(),
    };
    matches!(
        ty,
        "text" | "character varying" | "varchar" | "bpchar" | "character" | "name"
    )
}

/// Колонки простых сравнений `col op …` (в том числе `alias.col` своего псевдонима и `(col)::type` строкового типа);
/// равенства — первыми. Конъюнкты с функциями, нестроковыми приведениями и чужими псевдонимами пропускаются.
pub fn columns_of(cond: &str, alias: Option<&str>) -> Vec<String> {
    let mut eq: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for c in conjuncts(cond) {
        if let Some((col, is_eq)) = conjunct_column(c, alias) {
            if is_eq { &mut eq } else { &mut other }.push(col);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for c in eq.into_iter().chain(other) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// `CREATE INDEX [CONCURRENTLY] ON "s"."t" ("a", "b")`; с `CONCURRENTLY` — команда для копирования с `;`.
pub fn index_ddl(schema: Option<&str>, table: &str, cols: &[String], concurrently: bool) -> String {
    let cols: Vec<String> = cols.iter().map(|c| quote_ident(c)).collect();
    let target = qualified(schema, table);
    if concurrently {
        format!(
            "CREATE INDEX CONCURRENTLY ON {target} ({});",
            cols.join(", ")
        )
    } else {
        format!("CREATE INDEX ON {target} ({})", cols.join(", "))
    }
}

/// Команда и гипотеза по колонкам: разобралось — индекс, иначе шаблон `(<columns>)` без hypopg.
pub fn index_recommendation(
    mut r: Recommendation,
    n: &PlanNode,
    cols: &[String],
) -> Recommendation {
    let Some(rel) = &n.relation else { return r };
    if cols.is_empty() {
        r = r.command(format!(
            "CREATE INDEX CONCURRENTLY ON {} (<columns>);",
            qualified(rel.schema.as_deref(), &rel.name)
        ));
    } else {
        r = r
            .command(index_ddl(rel.schema.as_deref(), &rel.name, cols, true))
            .hypo(index_ddl(rel.schema.as_deref(), &rel.name, cols, false))
            .param("columns", cols.join(", "));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_from_simple_conjuncts_equalities_first() {
        assert_eq!(
            columns_of("(o.status = 'disputed'::text)", Some("o")),
            ["status"]
        );
        assert_eq!(
            columns_of(
                "((o.created_at >= '2025-01-01'::timestamptz) AND (o.status = 'new'::text))",
                Some("o")
            ),
            ["status", "created_at"]
        );
        assert_eq!(
            columns_of("((customer_id = 5) AND (lower(email) = 'x'::text))", None),
            ["customer_id"],
            "функция пропускается"
        );
        assert_eq!(
            columns_of("((o.status)::text = 'x'::text)", Some("o")),
            ["status"],
            "колонка под приведением"
        );
        assert_eq!(columns_of("(c.id < 100)", Some("c")), ["id"]);
        assert_eq!(
            columns_of("(a.id = b.id)", Some("a")),
            ["id"],
            "ключ соединения со стороны псевдонима"
        );
        assert!(columns_of("(random() > 0.5)", None).is_empty());
        assert_eq!(columns_of("(\"Order Id\" = 1)", None), ["Order Id"]);
        assert!(
            columns_of("(x.col = 1)", Some("o")).is_empty(),
            "чужой псевдоним — не колонка таблицы"
        );
    }

    #[test]
    fn index_ddl_quotes_and_optionally_concurrently() {
        assert_eq!(
            index_ddl(
                Some("shop"),
                "orders",
                &["status".into(), "created_at".into()],
                true
            ),
            "CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\", \"created_at\");"
        );
        assert_eq!(
            index_ddl(None, "t", &["a".into()], false),
            "CREATE INDEX ON \"t\" (\"a\")"
        );
    }

    #[test]
    fn report_skips_fact_rules_without_analyze() {
        let t = crate::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Seq Scan", "Relation Name": "orders", "Schema": "shop", "Startup Cost": 0.0, "Total Cost": 100.0, "Plan Rows": 10, "Plan Width": 8, "Filter": "(status = 'x'::text)"}})).unwrap();
        let r = evaluate(&t);
        assert!(r.recommendations.is_empty());
        assert!(
            r.skipped.contains(&"plan_seq_scan_filter".to_string())
                && r.skipped.contains(&"plan_misestimate".to_string())
        );
        assert_eq!(r.skipped.len(), 9);
        assert!(
            !r.skipped.contains(&"plan_sort_limit".to_string())
                && !r.skipped.contains(&"plan_no_pruning".to_string())
        );
    }

    #[test]
    fn recommendation_json_shape() {
        let r = Recommendation::new("plan_seq_scan_filter", Severity::Warning, 1)
            .param("rowsRead", 500_000)
            .command("CREATE INDEX CONCURRENTLY ON \"shop\".\"orders\" (\"status\");")
            .hypo("CREATE INDEX ON \"shop\".\"orders\" (\"status\")");
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["id"], "plan_seq_scan_filter");
        assert_eq!(v["nodeIds"], serde_json::json!([1]));
        assert_eq!(v["params"]["rowsRead"], 500_000);
        assert_eq!(
            v["hypo"]["indexDdl"],
            "CREATE INDEX ON \"shop\".\"orders\" (\"status\")"
        );
    }

    #[test]
    fn columns_handle_nested_groups_casts_and_quotes() {
        assert_eq!(
            columns_of(
                "(((o.a = 1) AND (o.b > 2)) AND ((o.c)::character varying = 'x'::text))",
                Some("o")
            ),
            ["a", "c", "b"]
        );
        assert_eq!(
            columns_of("((o.note = 'x AND (y'::text) AND (o.d IS NULL))", Some("o")),
            ["note", "d"]
        );
        assert_eq!(columns_of("(\"we\"\"ird\" = 1)", None), ["we\"ird"]);
        assert_eq!(
            columns_of("((o.a = 1) AND (o.a = 2))", Some("o")),
            ["a"],
            "без повторов"
        );
        assert!(columns_of("(NOT o.flag)", Some("o")).is_empty());
        assert!(
            columns_of("((o.a = 1) OR (o.b = 2))", Some("o")).is_empty(),
            "OR — не конъюнкция"
        );
    }

    #[test]
    fn early_stop_nodes_stop_at_blocking_nodes() {
        let t = crate::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Limit", "Plans": [
            {"Node Type": "Nested Loop", "Parent Relationship": "Outer", "Plans": [
                {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "a"},
                {"Node Type": "Sort", "Parent Relationship": "Inner", "Plans": [
                    {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "b"}
                ]}
            ]}
        ]}})).unwrap();
        let mut ids: Vec<u32> = early_stop_nodes(&t.root).into_iter().collect();
        ids.sort();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn partition_scans_of_one_append_are_grouped() {
        let scan = |i: u32| {
            serde_json::json!({"Node Type": "Seq Scan", "Parent Relationship": "Member", "Relation Name": format!("events_{i}"), "Schema": "shop", "Alias": format!("events_{i}"),
                "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 10, "Plan Width": 8, "Actual Startup Time": 0.1, "Actual Total Time": 5.0, "Actual Rows": 0, "Actual Loops": 1,
                "Filter": format!("(events_{i}.kind = 'pay'::text)"), "Rows Removed by Filter": 20_000})
        };
        let v = serde_json::json!({"Plan": {"Node Type": "Append", "Startup Cost": 0.0, "Total Cost": 10.0, "Plan Rows": 30, "Plan Width": 8,
            "Actual Startup Time": 0.1, "Actual Total Time": 20.0, "Actual Rows": 0, "Actual Loops": 1, "Plans": [scan(0), scan(1), scan(2)]}});
        let r = evaluate(&crate::plan::json::from_json(&v).unwrap());
        let seq: Vec<&Recommendation> = r
            .recommendations
            .iter()
            .filter(|r| r.id == "plan_seq_scan_filter")
            .collect();
        assert_eq!(seq.len(), 1);
        let g = seq[0];
        assert_eq!(g.node_ids, [1, 2, 3]);
        assert_eq!(g.params["partitions"], 3);
        assert_eq!(g.params["rowsRead"], 60_000);
        assert_eq!(
            g.params["tables"],
            "shop.events_0, shop.events_1, shop.events_2"
        );
        assert!(g.hypo.is_none());
        assert_eq!(g.command.as_deref().unwrap().lines().count(), 3);
        assert!(
            g.command
                .as_deref()
                .unwrap()
                .contains("CREATE INDEX CONCURRENTLY ON \"shop\".\"events_1\" (\"kind\");")
        );
    }

    #[test]
    fn hashed_set_op_blocks_early_stop() {
        let plan = |strategy: &str| {
            crate::plan::json::from_json(&serde_json::json!({"Plan": {"Node Type": "Limit", "Plans": [
                {"Node Type": "SetOp", "Strategy": strategy, "Parent Relationship": "Outer", "Plans": [
                    {"Node Type": "Seq Scan", "Parent Relationship": "Outer", "Relation Name": "a"}
                ]}
            ]}}))
            .unwrap()
        };
        let mut ids: Vec<u32> = early_stop_nodes(&plan("Hashed").root).into_iter().collect();
        assert_eq!(ids, [1], "HashSetOp читает вход целиком");
        ids = early_stop_nodes(&plan("Sorted").root).into_iter().collect();
        ids.sort();
        assert_eq!(ids, [1, 2]);
    }

    #[test]
    fn only_text_like_casts_are_stripped() {
        assert_eq!(
            columns_of(
                "(((o.code)::character varying(10) = 'x'::text) AND ((o.kind)::bpchar = 'a'::bpchar) AND ((o.n)::name = 'n'::name))",
                Some("o")
            ),
            ["code", "kind", "n"]
        );
        assert!(
            columns_of("((e.happened_at)::date = '2026-03-05'::date)", Some("e")).is_empty(),
            "приведение к date — выражение, индекс по колонке не поможет"
        );
        assert!(columns_of("((o.total)::numeric > 5)", Some("o")).is_empty());
        assert!(columns_of("((o.id)::integer = 5)", Some("o")).is_empty());
        assert!(columns_of("((o.tags)::text[] = '{}'::text[])", Some("o")).is_empty());
    }

    #[test]
    fn join_key_is_taken_from_the_right_side() {
        assert_eq!(columns_of("(a.order_id = o.id)", Some("o")), ["id"]);
        assert_eq!(
            columns_of("((a.x = 1) AND (a.code < (o.code)::text))", Some("o")),
            ["code"]
        );
        assert!(
            columns_of("(a.order_id = (o.id)::bigint)", Some("o")).is_empty(),
            "приведение к нестроковому типу — выражение"
        );
        assert!(
            columns_of("(a.order_id = b.id)", Some("o")).is_empty(),
            "обе стороны чужие"
        );
        assert!(
            columns_of("(a.order_id = (o.id + 1))", Some("o")).is_empty(),
            "выражение справа"
        );
    }
}
