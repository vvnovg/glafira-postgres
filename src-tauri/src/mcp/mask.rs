//! Маскирование литералов на уровне `diagnostics` (спец. P4 §6.1): тексты `pg_stat_activity` и pgss в данных
//! инструментов и выражения узлов планов, построенных не по SQL модели. Лексер — `pg_advisor::sqltext::mask_literals`;
//! здесь — где в данных тексты. `standard_conforming_strings` узла (уточнение 3 плана) читается тем же узлом перед
//! маскированием; неизвестно — `on`.

use pg_advisor::locktree::LockNode;
use pg_advisor::sqltext::{MaskOptions, mask_literals};
use serde_json::Value;
use tokio_postgres::Client;

use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::diag::{DiagData, DiagReport};
use crate::state::AppState;

/// Строковые выражения узлов JSON-плана (`EXPLAIN (FORMAT JSON, VERBOSE)`): в них сервер печатает литералы запроса.
pub const PLAN_EXPRESSION_KEYS: [&str; 24] = [
    "Output",
    "Filter",
    "Join Filter",
    "One-Time Filter",
    "Index Cond",
    "Order By",
    "Recheck Cond",
    "TID Cond",
    "Merge Cond",
    "Hash Cond",
    "Run Condition",
    "Sort Key",
    "Presorted Key",
    "Group Key",
    "Group Keys",
    "Hash Key",
    "Hash Keys",
    "Cache Key",
    "Function Call",
    "Table Function Call",
    "Sampling Parameters",
    "Repeatable Seed",
    "Conflict Filter",
    "Window",
];

/// Маскирует текст на месте; итог — число замаскированных литералов (комментарии не считаются).
pub fn mask_text(text: &mut String, opts: MaskOptions) -> usize {
    let (masked, n) = mask_literals(text, opts);
    *text = masked;
    n
}

pub fn mask_opt(text: &mut Option<String>, opts: MaskOptions) -> usize {
    text.as_mut().map_or(0, |t| mask_text(t, opts))
}

/// В данных инструмента бывают тексты запросов.
pub fn has_texts(d: &DiagData) -> bool {
    matches!(
        d,
        DiagData::Sessions(_) | DiagData::Locks(_) | DiagData::Waits(_) | DiagData::Queries(_)
    )
}

fn mask_lock_node(n: &mut LockNode, opts: MaskOptions) -> usize {
    mask_opt(&mut n.session.query, opts)
        + n.children
            .iter_mut()
            .map(|c| mask_lock_node(c, opts))
            .sum::<usize>()
}

/// Тексты запросов в данных отчёта: сессии, дерево блокировок и очередь DDL, топ профиля ожиданий, строки pgss.
/// Исчерпывающий `match`: новый инструмент (P5) не пройдёт сборку, пока здесь не решено, есть ли у него тексты.
pub fn mask_report(report: &mut DiagReport, opts: MaskOptions) -> usize {
    match &mut report.data {
        DiagData::Sessions(d) => d
            .rows
            .iter_mut()
            .map(|r| mask_opt(&mut r.query, opts))
            .sum(),
        DiagData::Locks(d) => {
            let roots: usize = d.roots.iter_mut().map(|n| mask_lock_node(n, opts)).sum();
            let queue: usize = d
                .ddl_queue
                .iter_mut()
                .map(|q| mask_opt(&mut q.session.query, opts))
                .sum();
            roots + queue
        }
        DiagData::Waits(d) => d
            .top_queries
            .iter_mut()
            .map(|q| mask_text(&mut q.text, opts))
            .sum(),
        DiagData::Queries(d) => d.rows.iter_mut().map(|r| mask_opt(&mut r.text, opts)).sum(),
        // Держатели горизонта — сессии без текста, остальное — счётчики, имена слотов и состояния узлов.
        DiagData::Pulse(_)
        | DiagData::Horizon(_)
        | DiagData::Replication(_)
        | DiagData::Slots(_)
        | DiagData::Failover(_)
        | DiagData::Health(_) => 0,
    }
}

fn mask_strings(v: &mut Value, opts: MaskOptions) -> usize {
    match v {
        Value::String(s) => mask_text(s, opts),
        Value::Array(a) => a.iter_mut().map(|x| mask_strings(x, opts)).sum(),
        Value::Object(_) => mask_plan_value(v, opts),
        _ => 0,
    }
}

/// Выражения узлов JSON-плана (`PLAN_EXPRESSION_KEYS`) на любой глубине; числа плана, имена отношений и индексов и
/// настройки (`Settings`) не меняются.
pub fn mask_plan_value(raw: &mut Value, opts: MaskOptions) -> usize {
    match raw {
        Value::Object(m) => m
            .iter_mut()
            .map(|(k, v)| {
                if PLAN_EXPRESSION_KEYS.contains(&k.as_str()) {
                    mask_strings(v, opts)
                } else {
                    mask_plan_value(v, opts)
                }
            })
            .sum(),
        Value::Array(a) => a.iter_mut().map(|v| mask_plan_value(v, opts)).sum(),
        _ => 0,
    }
}

const SCS_SQL: &str = marked_sql!("SHOW standard_conforming_strings");

/// `standard_conforming_strings` на соединении: `off` — обратная косая в '…' экранирует; ошибка или неизвестное
/// значение — `on` (значение по умолчанию сервера).
pub async fn scs_of(c: &Client) -> MaskOptions {
    let on = match c.query_one(SCS_SQL, &[]).await {
        Ok(r) => r.get::<_, Option<String>>(0).as_deref() != Some("off"),
        Err(_) => true,
    };
    MaskOptions {
        standard_conforming_strings: on,
    }
}

/// То же на узле `member` открытого подключения (пул каталога, БД `database` или БД профиля). Подключение или узел
/// недоступны — `on`.
pub async fn scs_on(
    st: &AppState,
    connection_id: &str,
    member: &str,
    database: Option<&str>,
) -> MaskOptions {
    let Ok((_, profile)) = st.runtimes.target(connection_id, Some(member)) else {
        return MaskOptions::default();
    };
    on_catalog_node_within(
        catalog_limit(&profile),
        st.client(connection_id, Some(member), database),
        |c| Box::pin(async move { Ok(scs_of(c).await) }),
    )
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use pg_advisor::ash::{TopQuery, WaitsData};
    use pg_advisor::locktree::{DdlQueueItem, LockNode, LockSession, LocksData};
    use pg_advisor::queries::{Counters, QueryKey, Totals, stat};
    use pg_advisor::rules::horizon::{Holder, HorizonData};
    use pg_advisor::rules::sessions::{SessionRow, SessionsData};
    use serde_json::json;

    use super::*;
    use crate::diag::queries::{QueriesData, QueriesMode};
    use crate::diag::{DiagReport, Tool};

    const ON: MaskOptions = MaskOptions {
        standard_conforming_strings: true,
    };
    const SECRET: &str = "SELECT * FROM shop.customers WHERE email = 'ann@example.com' AND id = 42 /* tenant=acme */";

    fn report(tool: Tool, data: DiagData) -> DiagReport {
        DiagReport {
            tool,
            member: Some("pg-1".into()),
            follows_leader: true,
            observed_at_ms: 1,
            server_version_num: Some(170_000),
            in_recovery: Some(false),
            data,
            findings: vec![],
            unavailable: vec![],
        }
    }

    fn clean(s: &str) {
        assert!(
            !s.contains("ann@example.com") && !s.contains("42") && !s.contains("tenant=acme"),
            "{s}"
        );
        assert!(s.contains("$1") && s.contains("shop.customers"), "{s}");
    }

    #[test]
    fn session_texts_are_masked() {
        let mut r = report(
            Tool::Sessions,
            DiagData::Sessions(SessionsData {
                rows: vec![
                    SessionRow {
                        pid: 1,
                        query: Some(SECRET.into()),
                        ..Default::default()
                    },
                    SessionRow {
                        pid: 2,
                        query: None,
                        ..Default::default()
                    },
                ],
                settings: Default::default(),
            }),
        );
        assert_eq!(mask_report(&mut r, ON), 2);
        let DiagData::Sessions(d) = &r.data else {
            unreachable!()
        };
        clean(d.rows[0].query.as_deref().unwrap());
        assert_eq!(d.rows[1].query, None);
    }

    #[test]
    fn lock_tree_texts_are_masked_at_every_depth() {
        let s = |pid| LockSession {
            pid,
            query: Some(SECRET.into()),
            ..Default::default()
        };
        let tree = LockNode {
            session: s(1),
            children: vec![LockNode {
                session: s(2),
                children: vec![LockNode {
                    session: s(3),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut r = report(
            Tool::Locks,
            DiagData::Locks(LocksData {
                roots: vec![tree],
                cycles: vec![],
                ddl_queue: vec![DdlQueueItem {
                    session: s(4),
                    mode: "AccessExclusiveLock".into(),
                    ..Default::default()
                }],
            }),
        );
        assert_eq!(mask_report(&mut r, ON), 8, "4 сессии × 2 литерала");
        let v = serde_json::to_string(&r.data).unwrap();
        assert!(
            !v.contains("ann@example.com") && !v.contains("tenant=acme"),
            "{v}"
        );
    }

    #[test]
    fn wait_profile_queries_are_masked() {
        let mut r = report(
            Tool::Waits,
            DiagData::Waits(WaitsData {
                top_queries: vec![TopQuery {
                    key: "id:7".into(),
                    text: SECRET.into(),
                    samples: 3,
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        assert_eq!(mask_report(&mut r, ON), 2);
        let DiagData::Waits(d) = &r.data else {
            unreachable!()
        };
        clean(&d.top_queries[0].text);
    }

    #[test]
    fn statement_texts_are_masked() {
        let mut row = stat(
            QueryKey {
                query_id: 7,
                dbid: 1,
                userid: 10,
                toplevel: true,
            },
            Some("orders".into()),
            None,
            &Counters::default(),
            None,
            &Totals::default(),
        );
        row.text = Some("ALTER ROLE app PASSWORD 'hunter2'".into());
        let mut r = report(
            Tool::Queries,
            DiagData::Queries(QueriesData {
                rows: vec![row],
                ..QueriesData::empty(QueriesMode::SinceReset)
            }),
        );
        assert_eq!(mask_report(&mut r, ON), 1);
        let DiagData::Queries(d) = &r.data else {
            unreachable!()
        };
        assert_eq!(
            d.rows[0].text.as_deref(),
            Some("ALTER ROLE app PASSWORD $1")
        );
    }

    #[test]
    fn data_without_query_texts_is_left_as_is() {
        let data = DiagData::Horizon(HorizonData {
            holders: vec![Holder {
                name: "4242".into(),
                xmin_age: 10,
                ..Default::default()
            }],
            hot_standby_feedback: Some(false),
        });
        assert!(!has_texts(&data));
        let mut r = report(Tool::Horizon, data.clone());
        assert_eq!(mask_report(&mut r, ON), 0);
        assert_eq!(r.data, data);
    }

    #[test]
    fn backslash_follows_standard_conforming_strings() {
        // При `off` обратная косая в '…' экранирует кавычку, как на сервере: литерал тянется до второй кавычки, и
        // «OR 1=1» остаётся внутри него. При `on` литерал кончается на первой кавычке.
        let text = r"SELECT 'a\' OR 1=1 --', 'b'";
        let mut off = text.to_string();
        mask_text(
            &mut off,
            MaskOptions {
                standard_conforming_strings: false,
            },
        );
        assert!(!off.contains("OR"), "{off}");
        let mut on = text.to_string();
        mask_text(&mut on, ON);
        assert!(on.contains("OR"), "{on}");
    }

    #[test]
    fn plan_expressions_are_masked_but_plan_numbers_and_names_stay() {
        let mut raw = json!([{
            "Plan": {
                "Node Type": "Seq Scan", "Relation Name": "orders", "Total Cost": 12.5, "Plan Rows": 3,
                "Filter": "(status = 'disputed'::text)", "Output": ["id", "'x'::text"],
                "Plans": [{"Node Type": "Index Scan", "Index Name": "orders_pkey", "Index Cond": "(id = 42)"}]
            },
            "Settings": {"work_mem": "1MB"}
        }]);
        assert_eq!(mask_plan_value(&mut raw, ON), 3);
        let p = &raw[0]["Plan"];
        assert_eq!(p["Filter"], "(status = $1::text)");
        assert_eq!(p["Output"][1], "$1::text");
        assert_eq!(p["Plans"][0]["Index Cond"], "(id = $1)");
        assert_eq!(p["Total Cost"], 12.5);
        assert_eq!(p["Relation Name"], "orders");
        assert_eq!(raw[0]["Settings"]["work_mem"], "1MB");
    }
}
