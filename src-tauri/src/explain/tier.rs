//! Ярус EXPLAIN ANALYZE (спец. P3 §8.3): запись — `ModifyTable` в плане (в том числе внутри CTE), инструкция
//! `CREATE … AS` или вызов административной функции из `ADMIN_FUNCTIONS` в VERBOSE-плане пробы; иначе чтение.
//! Ошибку яруса страхует транзакция `READ ONLY`, но административные функции она не останавливает (спец. §15 п. 10).

use pg_advisor::plan::{PlanTree, has_modify_table};
use serde::Serialize;

use crate::explain::params::first_word;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Read,
    Write,
}

/// Функции, которые выполняются и внутри `BEGIN READ ONLY`: завершают сессии, сбрасывают статистику, перечитывают
/// конфигурацию, удаляют и создают слоты, переключают WAL, повышают реплику, ходят наружу. EXPLAIN ANALYZE такого
/// `SELECT` выполнил бы функцию ярусом чтения — без подтверждения и журнала. Список закрывает случай частично: вызовы
/// внутри тела другой функции или триггера в плане не видны.
pub const ADMIN_FUNCTIONS: &[&str] = &[
    "pg_cancel_backend",
    "pg_terminate_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
    "pg_stat_reset",
    "pg_stat_reset_shared",
    "pg_stat_reset_single_table_counters",
    "pg_stat_reset_single_function_counters",
    "pg_stat_reset_slru",
    "pg_stat_reset_replication_slot",
    "pg_stat_reset_subscription_stats",
    "pg_stat_statements_reset",
    "pg_stat_kcache_reset",
    "pg_wait_sampling_reset_profile",
    "pg_create_physical_replication_slot",
    "pg_create_logical_replication_slot",
    "pg_drop_replication_slot",
    "pg_copy_physical_replication_slot",
    "pg_copy_logical_replication_slot",
    "pg_replication_slot_advance",
    "pg_switch_wal",
    "pg_create_restore_point",
    "pg_promote",
    "pg_wal_replay_pause",
    "pg_wal_replay_resume",
    "pg_log_backend_memory_contexts",
    "pg_logical_emit_message",
    "pg_file_write",
    "pg_file_rename",
    "pg_file_unlink",
    "lo_export",
    "dblink",
    "dblink_exec",
    "dblink_send_query",
    "http",
    "http_get",
    "http_post",
    "http_put",
    "http_patch",
    "http_delete",
    "http_head",
];

const fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Вызов `name(` в тексте выражения плана: перед именем — не символ идентификатора (точка схемы допустима).
fn calls(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(i, _)| {
        let before_ok = text[..i].chars().next_back().is_none_or(|c| !ident_char(c));
        let after = text[i + name.len()..].trim_start();
        before_ok && after.starts_with('(')
    })
}

/// Административные функции, вызванные в VERBOSE-плане пробы (`Output`, условия, `Function Call`…): все строки JSON.
pub fn admin_calls(raw: &serde_json::Value) -> Vec<&'static str> {
    fn walk(v: &serde_json::Value, found: &mut Vec<&'static str>) {
        match v {
            serde_json::Value::String(s) => {
                for name in ADMIN_FUNCTIONS {
                    if !found.contains(name) && calls(s, name) {
                        found.push(name);
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, found)),
            serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk(raw, &mut found);
    found
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
