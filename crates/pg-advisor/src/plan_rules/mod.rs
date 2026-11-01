//! Рекомендации по плану (спец. P3 §9.1): правила над `PlanTree`, доказательство — цифры узла, готовая команда
//! (приложение её не выполняет), индекс-гипотеза для hypopg. Тексты — во фронтенде по `id` и `params`.

mod scan;

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::findings::Severity;
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

/// Правила, которым нужен факт выполнения (задача 2 дополняет список правилами выполнения).
pub const FACT_RULES: [&str; 4] = [
    "plan_seq_scan_filter",
    "plan_index_filter",
    "plan_nested_loop_seq",
    "plan_misestimate",
];

pub fn evaluate(t: &PlanTree) -> Report {
    let with_actual = t.root.actual.is_some();
    let early = early_stop_nodes(&t.root);
    let mut out = Vec::new();
    walk(&t.root, None, &mut |n, _parent| {
        if with_actual {
            out.extend(scan::seq_scan_filter(n));
            out.extend(scan::index_filter(n));
            out.extend(scan::nested_loop_seq(n));
            out.extend(scan::misestimate(n, early.contains(&n.id)));
        }
    });
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

/// Обход в глубину с родителем.
pub fn walk<'a>(
    n: &'a PlanNode,
    parent: Option<&'a PlanNode>,
    f: &mut impl FnMut(&'a PlanNode, Option<&'a PlanNode>),
) {
    f(n, parent);
    for c in &n.children {
        walk(c, Some(n), f);
    }
}

/// Узлы, которые `Limit` может остановить раньше конца (поправка 8 плана P3b): они читают меньше оценки планировщика,
/// и «переоценка» у них не ошибка статистики. Блокирующий узел (Sort, Hash, агрегат не по сортировке) читает вход
/// целиком — ниже него остановки нет; InitPlan / SubPlan выполняются отдельно.
pub fn early_stop_nodes(root: &PlanNode) -> HashSet<u32> {
    fn go(n: &PlanNode, under: bool, out: &mut HashSet<u32>) {
        let under = under
            && !matches!(
                n.parent_relationship.as_deref(),
                Some("InitPlan" | "SubPlan")
            );
        if under {
            out.insert(n.id);
        }
        let blocking = matches!(n.node_type.as_str(), "Sort" | "Hash" | "HashSetOp")
            || (n.node_type == "Aggregate" && n.strategy.as_deref() != Some("Sorted"));
        let next = (under || n.node_type == "Limit") && !blocking;
        for c in &n.children {
            go(c, next, out);
        }
    }
    let mut out = HashSet::new();
    go(root, false, &mut out);
    out
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

/// Колонка конъюнкта `col op …` (в том числе `alias.col` своего псевдонима и `(col)::type`) и признак равенства.
fn conjunct_column(c: &str, alias: Option<&str>) -> Option<(String, bool)> {
    let s = strip_parens(c);
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
    if tok.starts_with("::") {
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
        }
    }
    OPERATORS.contains(&tok).then(|| (col, tok == "="))
}

/// Колонки простых сравнений `col op …` (в том числе `alias.col` своего псевдонима и `(col)::type`); равенства —
/// первыми. Конъюнкты с функциями и чужими псевдонимами пропускаются.
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
}
