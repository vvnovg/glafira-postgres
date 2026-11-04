//! Ярус EXPLAIN ANALYZE (спец. P3 §8.3): запись — `ModifyTable` в плане (в том числе внутри CTE), инструкция
//! `CREATE … AS` или вызов административной функции из `ADMIN_FUNCTIONS` в VERBOSE-плане пробы; иначе чтение.
//! Ошибку яруса страхует транзакция `READ ONLY`, но административные функции она не останавливает (спец. §15 п. 10).
//! Список и поиск вызовов — `pg_advisor::functions` (спец. P4 §7.2: тот же список входит в запрещённые для SQL модели).

use pg_advisor::functions::calls_in_plan;
use pg_advisor::plan::{PlanTree, has_modify_table};
use serde::Serialize;

use crate::explain::params::first_word;

pub use pg_advisor::functions::ADMIN_FUNCTIONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Read,
    Write,
}

/// Административные функции, вызванные в VERBOSE-плане пробы (`Output`, условия, `Function Call`…): все строки JSON.
pub fn admin_calls(raw: &serde_json::Value) -> Vec<String> {
    calls_in_plan(raw, ADMIN_FUNCTIONS)
}

pub fn tier_of(tree: &PlanTree, sql: &str, raw: &serde_json::Value) -> Tier {
    if has_modify_table(tree)
        || first_word(sql).eq_ignore_ascii_case("create")
        || !admin_calls(raw).is_empty()
    {
        Tier::Write
    } else {
        Tier::Read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::plan::json::from_json;
    use serde_json::json;

    fn tree(node_type: &str) -> pg_advisor::plan::PlanTree {
        from_json(&json!({"Plan": {"Node Type": node_type, "Startup Cost": 0.0, "Total Cost": 1.0, "Plan Rows": 1, "Plan Width": 4}})).unwrap()
    }

    #[test]
    fn modify_table_or_create_is_write() {
        let none = json!({});
        assert_eq!(tier_of(&tree("Seq Scan"), "SELECT 1", &none), Tier::Read);
        assert_eq!(
            tier_of(&tree("ModifyTable"), "UPDATE t SET a = 1", &none),
            Tier::Write
        );
        assert_eq!(
            tier_of(
                &tree("Seq Scan"),
                "/* x */ create table t2 as select 1",
                &none
            ),
            Tier::Write
        );
        assert_eq!(
            tier_of(&tree("Seq Scan"), "SELECT nextval('s')", &none),
            Tier::Read,
            "побочный эффект ловит READ ONLY"
        );
        assert_eq!(serde_json::to_value(Tier::Write).unwrap(), "write");
    }

    #[test]
    fn admin_function_calls_in_the_verbose_plan_are_a_write() {
        let raw = json!([{"Plan": {
            "Node Type": "Seq Scan",
            "Output": ["pg_terminate_backend(pg_stat_activity.pid)", "pid"],
            "Filter": "(pg_stat_activity.usename = 'app'::name)",
            "Plans": [{"Node Type": "Function Scan", "Function Call": "public.dblink_exec('x'::text, 'y'::text)"}]
        }, "Settings": {"search_path": "pg_catalog"}}]);
        assert_eq!(
            admin_calls(&raw),
            vec!["pg_terminate_backend", "dblink_exec"]
        );
        assert_eq!(tier_of(&tree("Seq Scan"), "SELECT 1", &raw), Tier::Write);
        // Имя внутри другого идентификатора, без скобки или в строке-литерале без вызова — не вызов.
        let quiet = json!({"Output": [
            "my_pg_terminate_backend(1)",
            "pg_terminate_backend_count",
            "pg_reload_conf_x(1)",
            "'pg_switch_wal'::text"
        ], "Relation Name": "dblink"});
        assert!(admin_calls(&quiet).is_empty(), "{:?}", admin_calls(&quiet));
        // Схема перед именем и пробел перед скобкой — вызов.
        assert_eq!(
            admin_calls(&json!({"Output": ["pg_catalog.pg_reload_conf ()"]})),
            vec!["pg_reload_conf"]
        );
    }
}
