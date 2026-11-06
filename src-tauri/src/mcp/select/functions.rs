//! Сверка функций запроса `run_select` с `pg_proc` (спец. P4 §7.3, шаг 4, с решением владельца P4b): имена дерева
//! разбора — до пробы плана, имена VERBOSE-плана — после неё; отказ — `user_function` (тело функции не проверить) или
//! `forbidden_function` (схема из `DENIED_SCHEMAS`: внешние вызовы).

use std::collections::HashSet;

use glafira_mcp_core::error::ToolError;
use pg_advisor::functions::{DENIED_SCHEMAS, calls_in_plan};
use pg_advisor::sqlcheck::FuncRef;
use serde_json::Value;
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::AppResult;
use crate::mcp::access::{err, tool_error};

/// Шаблон «любое имя» для `calls_in_plan`: все вызовы VERBOSE-плана.
const ANY_FUNCTION: &str = "*";

/// Все имена вызовов VERBOSE-плана пробы (без схемы: план печатает её, только если схема не видна), в порядке плана, без
/// повторов. Функции из представлений и запись через атрибут (`r.f` = `f(r)`) план печатает обычными вызовами; ключевые
/// слова перед скобкой (`ANY (`, `numeric(12,2)`) тоже попадают сюда — в `pg_proc` вне `pg_catalog` их обычно нет.
pub fn plan_functions(raw: &Value) -> Vec<String> {
    calls_in_plan(raw, &[ANY_FUNCTION])
}

/// Имена для сверки с `pg_proc` одним запросом (массивы `unnest`): из дерева разбора — в своей схеме (`schemas`) или в
/// `search_path` (`""`), из плана — в любой схеме (`anywhere`). Повторы не уходят на сервер.
#[derive(Debug, Default, PartialEq)]
pub struct Lookups {
    pub schemas: Vec<String>,
    pub names: Vec<String>,
    pub anywhere: Vec<bool>,
}

impl Lookups {
    pub fn new(tree: &[FuncRef], plan: &[String]) -> Self {
        let mut l = Self::default();
        // Повторы — по множеству: длинный текст с тысячами вызовов не должен стоить O(n²).
        let mut seen = HashSet::new();
        let refs = tree
            .iter()
            .map(|f| (f.schema.clone().unwrap_or_default(), f.name.clone(), false))
            .chain(plan.iter().map(|n| (String::new(), n.clone(), true)));
        for (schema, name, anywhere) in refs {
            if seen.insert((schema.clone(), name.clone(), anywhere)) {
                l.schemas.push(schema);
                l.names.push(name);
                l.anywhere.push(anywhere);
            }
        }
        l
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// Функция запроса, которую `run_select` отвергает: найдена в `pg_proc` вне `pg_catalog` и `information_schema` и либо
/// не принадлежит расширению (`pg_depend.deptype = 'e'`), либо лежит в схеме из `DENIED_SCHEMAS`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncHit {
    pub schema: String,
    pub name: String,
    /// Схема из `DENIED_SCHEMAS` (`net`, `aws_s3`, `aws_lambda`: внешние вызовы) — кто бы ни владел функцией.
    pub denied_schema: bool,
}

pub(super) const USER_FUNCTIONS_SQL: &str = marked_sql!(
    "SELECT DISTINCT n.nspname::text, p.proname::text, lower(n.nspname::text) = ANY ($4::text[]) \
     FROM unnest($1::text[], $2::text[], $3::bool[]) AS f(schema, name, anywhere) \
     JOIN pg_proc p ON p.proname = f.name \
     JOIN pg_namespace n ON n.oid = p.pronamespace \
     WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') \
       AND (f.anywhere OR CASE WHEN f.schema = '' THEN n.nspname = ANY (current_schemas(false)) \
            ELSE n.nspname = f.schema END) \
       AND (lower(n.nspname::text) = ANY ($4::text[]) OR NOT EXISTS (SELECT 1 FROM pg_depend d \
         WHERE d.classid = 'pg_proc'::regclass AND d.objid = p.oid \
           AND d.refclassid = 'pg_extension'::regclass AND d.deptype = 'e')) \
     ORDER BY 3 DESC, 1, 2"
);

/// Шаг 4 §7.3 (с решением владельца P4b): функции запроса, которые `run_select` отвергает (`FuncHit`). Тело
/// пользовательской функции наш разбор не видит. Сверка осторожная: функция дерева разбора без схемы ищется во всех
/// схемах `search_path`, имя из плана — во всех схемах; одноимённая пользовательская перегрузка тоже даёт отказ.
pub async fn user_functions(
    c: &Client,
    tree: &[FuncRef],
    plan: &[String],
) -> AppResult<Vec<FuncHit>> {
    let l = Lookups::new(tree, plan);
    if l.is_empty() {
        return Ok(Vec::new());
    }
    let rows = c
        .query(
            USER_FUNCTIONS_SQL,
            &[&l.schemas, &l.names, &l.anywhere, &DENIED_SCHEMAS],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(rows
        .iter()
        .map(|r| FuncHit {
            schema: r.get(0),
            name: r.get(1),
            denied_schema: r.get(2),
        })
        .collect())
}

/// Отказ по найденным функциям (первая — из запрещённой схемы, если есть): схема из `DENIED_SCHEMAS` —
/// `forbidden_function`, иначе — `user_function` (спец. §7.3, шаг 4). `from_plan` — имя из плана: вызов может быть
/// внутри представления.
pub fn function_rejection(hits: &[FuncHit], from_plan: bool) -> Option<ToolError> {
    let hit = hits
        .iter()
        .find(|h| h.denied_schema)
        .or_else(|| hits.first())?;
    let name = format!("{}.{}", hit.schema, hit.name);
    Some(if hit.denied_schema {
        err(
            "forbidden_function",
            format!(
                "Function {name} is not allowed: functions of schema {} call outside the database",
                hit.schema
            ),
        )
    } else if from_plan {
        err(
            "user_function",
            format!(
                "User-defined function {name} is not allowed in run_select: its body cannot be checked (the query calls it, possibly inside a view)"
            ),
        )
    } else {
        err(
            "user_function",
            format!(
                "User-defined function {name} is not allowed in run_select: its body cannot be checked"
            ),
        )
    })
}

/// Сверка с `pg_proc` и отказ, если есть что отвергнуть.
pub(super) async fn reject_functions(
    c: &Client,
    tree: &[FuncRef],
    plan: &[String],
) -> Result<(), ToolError> {
    let hits = user_functions(c, tree, plan).await.map_err(tool_error)?;
    match function_rejection(&hits, !plan.is_empty()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Имена из дерева разбора ищутся в своей схеме или в `search_path`, имена из плана — в любой схеме (план печатает
    /// их без схемы, если она видна); точные повторы не уходят на сервер.
    #[test]
    fn lookups_scope_tree_names_and_spread_plan_names() {
        let tree = vec![
            FuncRef {
                schema: Some("shop".into()),
                name: "order_count".into(),
            },
            FuncRef {
                schema: None,
                name: "lower".into(),
            },
            FuncRef {
                schema: None,
                name: "lower".into(),
            },
        ];
        let plan = vec![
            "order_total".to_string(),
            "lower".to_string(),
            "order_total".to_string(),
        ];
        let l = Lookups::new(&tree, &plan);
        assert_eq!(l.schemas, ["shop", "", "", ""]);
        assert_eq!(l.names, ["order_count", "lower", "order_total", "lower"]);
        assert_eq!(
            l.anywhere,
            [false, false, true, true],
            "повторы убраны, имя плана — своей областью"
        );
        let only_plan = Lookups::new(&[], &plan);
        assert_eq!(only_plan.anywhere, [true, true]);
        assert!(Lookups::new(&[], &[]).is_empty());
    }

    /// Все вызовы VERBOSE-плана (`calls_in_plan` с шаблоном `*`): функции из представлений и запись через атрибут
    /// (`r.f` — это `f(r)`) план печатает вызовами; имя в литерале — не вызов.
    #[test]
    fn plan_functions_are_every_call_of_the_verbose_plan() {
        let raw = json!([{"Plan": {
            "Node Type": "Seq Scan",
            "Relation Name": "orders",
            "Output": [
                "shop.order_total(o.*)",
                "lower(o.note)",
                "\"MixedCase\"(o.id)",
                "'evil(1)'::text"
            ],
            "Filter": "(o.id = ANY ('{1,2}'::integer[]))",
            "Plans": [{"Node Type": "Function Scan", "Function Call": "net.http_get('http://x'::text)"}]
        }}]);
        let names = plan_functions(&raw);
        for name in ["order_total", "lower", "MixedCase", "http_get"] {
            assert!(names.contains(&name.to_string()), "{name}: {names:?}");
        }
        assert!(!names.contains(&"evil".to_string()), "{names:?}");
    }

    fn hit(schema: &str, name: &str, denied_schema: bool) -> FuncHit {
        FuncHit {
            schema: schema.into(),
            name: name.into(),
            denied_schema,
        }
    }

    /// Решение по найденному в `pg_proc` (без базы): функция из схемы `DENIED_SCHEMAS` (pg_net, aws_s3, aws_lambda) —
    /// `forbidden_function`, кто бы ей ни владел; функция вне `pg_catalog`, не принадлежащая расширению, —
    /// `user_function` (её тело не проверить). Сервер отдаёт только такие функции (расширения вне запрещённых схем —
    /// нет), запрещённая схема — первой.
    #[test]
    fn found_functions_are_rejected_with_the_reason() {
        assert!(function_rejection(&[], false).is_none());
        let e = function_rejection(&[hit("shop", "order_count", false)], false).unwrap();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "user_function",
                "User-defined function shop.order_count is not allowed in run_select: its body cannot be checked"
            )
        );
        let e = function_rejection(&[hit("shop", "order_total", false)], true).unwrap();
        assert_eq!(e.code, "user_function");
        assert!(
            e.message
                .starts_with("User-defined function shop.order_total is not allowed")
                && e.message.contains("view"),
            "из плана — может быть внутри представления: {}",
            e.message
        );
        let e = function_rejection(
            &[hit("net", "http_get", true), hit("shop", "f", false)],
            true,
        )
        .unwrap();
        assert_eq!(e.code, "forbidden_function");
        assert!(e.message.contains("net.http_get"), "{}", e.message);
        for (schema, name) in [("aws_s3", "query_export_to_s3"), ("aws_lambda", "invoke")] {
            let e = function_rejection(&[hit("shop", "f", false), hit(schema, name, true)], true)
                .unwrap();
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                (
                    "forbidden_function",
                    format!(
                        "Function {schema}.{name} is not allowed: functions of schema {schema} call outside the database"
                    )
                    .as_str()
                )
            );
        }
    }
}
