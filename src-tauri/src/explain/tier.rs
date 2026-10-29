//! Ярус EXPLAIN ANALYZE (спец. P3 §8.3): запись — `ModifyTable` в плане (в том числе внутри CTE) или инструкция
//! `CREATE … AS`; иначе чтение. Ошибку яруса страхует транзакция `READ ONLY`.

use pg_advisor::plan::{PlanTree, has_modify_table};
use serde::Serialize;

use crate::explain::params::first_word;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Read,
    Write,
}

pub fn tier_of(tree: &PlanTree, sql: &str) -> Tier {
    if has_modify_table(tree) || first_word(sql).eq_ignore_ascii_case("create") {
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
        assert_eq!(tier_of(&tree("Seq Scan"), "SELECT 1"), Tier::Read);
        assert_eq!(
            tier_of(&tree("ModifyTable"), "UPDATE t SET a = 1"),
            Tier::Write
        );
        assert_eq!(
            tier_of(&tree("Seq Scan"), "/* x */ create table t2 as select 1"),
            Tier::Write
        );
        assert_eq!(
            tier_of(&tree("Seq Scan"), "SELECT nextval('s')"),
            Tier::Read,
            "побочный эффект ловит READ ONLY"
        );
        assert_eq!(serde_json::to_value(Tier::Write).unwrap(), "write");
    }
}
