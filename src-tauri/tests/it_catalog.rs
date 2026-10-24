//! Каталог демо-схемы на узлах стенда: PG 15 (`GPG_IT_SINGLE`), 16 (`GPG_IT_PRIMARY`), 17 (Patroni).
//! Вкладки объектов (таблица, представление, функция, последовательность) — на тех же узлах.

mod common;

use glafira_pg_lib::catalog::objects::ObjectKind;
use glafira_pg_lib::catalog::{
    databases, extensions, function, objects, roles, schemas, sequence, table, view,
};
use glafira_pg_lib::conn::pg::ConnectSpec;
use glafira_pg_lib::conn::profile::SslMode;
use glafira_pg_lib::secrets::{MemoryStore, PG_PASSWORD, SecretStore, secret_key};

fn addrs() -> Vec<String> {
    ["GPG_IT_SINGLE", "GPG_IT_PRIMARY"]
        .into_iter()
        .filter_map(common::env)
        .chain(common::env("GPG_IT_PATRONI").map(|_| "localhost:5441".to_string()))
        .collect()
}

async fn client(addr: &str, db: &str) -> tokio_postgres::Client {
    let mut p = common::server_profile("cat", addr);
    p.pg.ssl_mode = SslMode::Disable;
    let s = MemoryStore::default();
    s.set(&secret_key("cat", PG_PASSWORD), common::PG_PASSWORD_VALUE)
        .unwrap();
    let (h, port) = common::host_port(addr);
    ConnectSpec::build(&p, &h, port, db, &s)
        .unwrap()
        .connect()
        .await
        .unwrap()
}

/// Блокировки отношений, которые держит сеанс `c` (без каталога `pg_catalog`).
async fn held_locks(c: &tokio_postgres::Client) -> Vec<String> {
    c.query(
        "SELECT relation::regclass::text FROM pg_locks l JOIN pg_class r ON r.oid = l.relation \
         WHERE l.pid = pg_backend_pid() AND l.locktype = 'relation' \
           AND r.relnamespace <> 'pg_catalog'::regnamespace ORDER BY 1",
        &[],
    )
    .await
    .unwrap()
    .iter()
    .map(|r| r.get(0))
    .collect()
}

/// Список таблиц не держит блокировок на партициях: `pg_partition_tree` (или любое открытие партиций) в
/// запросе страницы копил бы `AccessShareLock` на каждую партицию до конца транзакции. Размеры обычных
/// таблиц (`pg_total_relation_size`) снимают свою блокировку сразу.
#[tokio::test]
async fn object_list_takes_no_locks_on_partitions() {
    for addr in addrs() {
        let c = client(&addr, "orders").await;
        c.batch_execute("BEGIN READ ONLY").await.unwrap();
        let tables = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 500)
            .await
            .unwrap();
        assert!(tables.items.iter().any(|o| o.name == "events"), "{addr}");
        let locks = held_locks(&c).await;
        c.batch_execute("ROLLBACK").await.unwrap();
        assert!(
            !locks.iter().any(|l| l.contains("events_")),
            "{addr}: блокировки партиций: {locks:?}"
        );
        assert!(
            locks.is_empty(),
            "{addr}: запрос списка оставил блокировки отношений: {locks:?}"
        );
    }
}

#[tokio::test]
async fn demo_schema_on_every_node() {
    for addr in addrs() {
        let pg = client(&addr, "postgres").await;
        let dbs = databases::list(&pg, None).await.unwrap();
        assert!(
            dbs.iter()
                .any(|d| d.name == "orders" && d.size_bytes.is_some()),
            "{addr}: {dbs:?}"
        );
        let only = databases::list(&pg, Some(&["orders".to_string(), "missing".to_string()]))
            .await
            .unwrap();
        assert_eq!(
            only.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["orders"]
        );

        let c = client(&addr, "orders").await;
        let list = schemas::list(&c, false).await.unwrap();
        let shop = list
            .iter()
            .find(|s| s.name == "shop")
            .unwrap_or_else(|| panic!("{addr}: {list:?}"));
        assert_eq!(
            (
                shop.counts.tables,
                shop.counts.views,
                shop.counts.matviews,
                shop.counts.sequences,
                shop.counts.functions
            ),
            (3, 1, 1, 3, 3),
            "{addr}"
        );
        assert!(!list.iter().any(|s| s.name == "pg_catalog"));
        assert!(
            schemas::list(&c, true)
                .await
                .unwrap()
                .iter()
                .any(|s| s.name == "pg_catalog" && s.system)
        );

        let tables = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 500)
            .await
            .unwrap();
        assert_eq!(
            tables
                .items
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["customers", "events", "orders"]
        );
        let events = &tables.items[1];
        assert_eq!(
            (events.relkind.as_deref(), events.partitions),
            (Some("p"), Some(2))
        );
        assert!(tables.items[0].est_rows.is_some_and(|n| n >= 100));
        // У партиционированной таблицы размер в списке не считается (для этого пришлось бы открыть каждую
        // партицию), у обычных — точный.
        assert_eq!(events.total_bytes, None, "{addr}: {events:?}");
        assert!(
            tables.items[0].total_bytes.is_some_and(|n| n > 0),
            "{addr}: {:?}",
            tables.items[0]
        );
        assert!(events.est_rows.is_none_or(|n| n >= 0), "{addr}: {events:?}");
        let page = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 2)
            .await
            .unwrap();
        assert_eq!((page.total, page.next_offset), (3, Some(2)));
        // Страница за концом списка: общее число настоящее, а не 0, продолжения нет.
        let past = objects::list(&c, "shop", ObjectKind::Tables, None, 10, 2)
            .await
            .unwrap();
        assert_eq!(
            (past.total, past.items.len(), past.next_offset),
            (3, 0, None),
            "{addr}"
        );
        let filtered = objects::list(&c, "shop", ObjectKind::Tables, Some("ORD"), 0, 500)
            .await
            .unwrap();
        assert_eq!(
            filtered
                .items
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["orders"]
        );
        let underscore = objects::list(&c, "shop", ObjectKind::Tables, Some("_"), 0, 500)
            .await
            .unwrap();
        assert_eq!(underscore.total, 0, "_ — буквальный символ, а не любой");

        let parts = objects::partitions(&c, "shop", "events", 0, 500)
            .await
            .unwrap();
        assert!(
            parts.items.iter().all(|p| p.total_bytes.is_some()),
            "{addr}: у партиций-листьев размер есть: {parts:?}"
        );
        assert_eq!(
            parts
                .items
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["events_2026_09", "events_2026_10"]
        );

        let parts_past = objects::partitions(&c, "shop", "events", 5, 10)
            .await
            .unwrap();
        assert_eq!(
            (
                parts_past.total,
                parts_past.items.len(),
                parts_past.next_offset
            ),
            (2, 0, None),
            "{addr}"
        );

        let funcs_past = objects::list(&c, "shop", ObjectKind::Functions, None, 100, 10)
            .await
            .unwrap();
        assert_eq!(
            (
                funcs_past.total,
                funcs_past.items.len(),
                funcs_past.next_offset
            ),
            (3, 0, None),
            "{addr}"
        );

        let funcs = objects::list(&c, "shop", ObjectKind::Functions, None, 0, 500)
            .await
            .unwrap();
        let sigs: Vec<(String, Option<String>)> = funcs
            .items
            .iter()
            .map(|f| (f.name.clone(), f.signature.clone()))
            .collect();
        assert!(
            sigs.contains(&("order_count".into(), Some("c bigint".into()))),
            "{sigs:?}"
        );
        assert!(
            funcs
                .items
                .iter()
                .any(|f| f.name == "close_order" && f.relkind.as_deref() == Some("p"))
        );

        let ext = extensions::list(&c).await.unwrap();
        assert!(ext.iter().any(|e| e.name == "plpgsql"));

        let rl = roles::list(&c, false).await.unwrap();
        let diag = rl.iter().find(|r| r.name == "glafira_diag").unwrap();
        assert!(diag.member_of.contains(&"pg_monitor".to_string()));
        // Роль в `member_of` одна, даже если её выдали дважды (PostgreSQL 16+: разные выдавшие).
        for r in roles::list(&c, true).await.unwrap() {
            let mut unique = r.member_of.clone();
            unique.dedup();
            assert_eq!(unique, r.member_of, "{addr}: {}", r.name);
        }
        assert!(!rl.iter().any(|r| r.name == "pg_monitor"));
        assert!(
            roles::list(&c, true)
                .await
                .unwrap()
                .iter()
                .any(|r| r.name == "pg_monitor")
        );

        let tbl = table::get(&c, "shop", "orders").await.unwrap();
        assert_eq!(
            tbl.columns
                .iter()
                .map(|x| x.name.as_str())
                .collect::<Vec<_>>(),
            ["id", "customer_id", "total", "total_with_tax", "status"]
        );
        let total = &tbl.columns[2];
        assert_eq!(
            (total.data_type.as_str(), total.not_null),
            ("numeric(12,2)", true)
        );
        assert_eq!(tbl.columns[3].generated.as_deref(), Some("s"));
        assert_eq!(
            tbl.columns[0].default_expr.as_deref(),
            Some("nextval('shop.orders_id_seq'::regclass)")
        );
        // Цель статистики по умолчанию — NULL на любой версии (до 17 в каталоге -1).
        assert!(
            tbl.columns.iter().all(|x| x.stat_target.is_none()),
            "{addr}"
        );
        assert!(
            tbl.indexes
                .iter()
                .any(|i| i.name == "orders_pkey" && i.primary && i.valid)
        );
        assert!(
            tbl.indexes
                .iter()
                .any(|i| i.name == "orders_customer_idx" && !i.unique)
        );
        assert!(
            tbl.constraints
                .iter()
                .any(|k| k.kind == "f" && k.definition.contains("REFERENCES shop.customers(id)"))
        );
        assert!(
            tbl.constraints
                .iter()
                .any(|k| k.kind == "c" && k.definition.contains("total >= 0"))
        );
        assert!(tbl.stats.total_bytes > 0 && tbl.stats.n_live_tup.is_some());
        assert_eq!(tbl.partitioning.key, None);
        assert_eq!(tbl.owner, "postgres");

        let customers = table::get(&c, "shop", "customers").await.unwrap();
        assert_eq!(customers.comment.as_deref(), Some("Покупатели"));
        assert_eq!(customers.columns[0].identity.as_deref(), Some("a"));
        assert_eq!(
            customers
                .triggers
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["customers_touch"]
        );
        assert!(customers.incoming_fks.iter().any(|f| f.table == "orders"));

        let events = table::get(&c, "shop", "events").await.unwrap();
        assert_eq!(events.relkind, "p");
        assert_eq!(
            events.partitioning.key.as_deref(),
            Some("RANGE (happened_at)")
        );
        assert_eq!(
            events.partitioning.partitions,
            ["shop.events_2026_09", "shop.events_2026_10"]
        );
        let part = table::get(&c, "shop", "events_2026_09").await.unwrap();
        assert_eq!(part.partitioning.parent.as_deref(), Some("shop.events"));
        assert!(
            part.partitioning
                .bound
                .as_deref()
                .unwrap()
                .starts_with("FOR VALUES FROM")
        );
        // Размер партиционированной таблицы — сумма по дереву партиций (у самого родителя хранения нет).
        let part10 = table::get(&c, "shop", "events_2026_10").await.unwrap();
        assert_eq!(
            (events.stats.total_bytes, events.stats.toast_bytes),
            (
                part.stats.total_bytes + part10.stats.total_bytes,
                part.stats.toast_bytes + part10.stats.toast_bytes
            ),
            "{addr}"
        );
        assert!(
            events.stats.seq_scan.is_some(),
            "{addr}: {:?}",
            events.stats
        );

        let v = view::get(&c, "shop", "active_orders").await.unwrap();
        assert!(!v.matview && v.definition.contains("status <> 'done'"));
        assert_eq!((v.populated, v.size_bytes), (None, None));
        let mv = view::get(&c, "shop", "order_totals").await.unwrap();
        assert_eq!((mv.matview, mv.populated), (true, Some(true)));
        assert!(mv.size_bytes.unwrap() > 0);
        assert_eq!(
            view::get(&c, "shop", "orders").await.unwrap_err().key,
            "errors.catalog.objectNotFound",
            "таблица — не представление"
        );

        let f_oid = funcs
            .items
            .iter()
            .find(|f| f.name == "order_count")
            .unwrap()
            .oid;
        let f = function::get(&c, f_oid).await.unwrap();
        assert_eq!(
            (f.kind.as_str(), f.language.as_str(), f.volatility.as_str()),
            ("f", "sql", "s")
        );
        assert!(
            f.definition
                .as_deref()
                .unwrap()
                .contains("CREATE OR REPLACE FUNCTION shop.order_count")
        );
        assert_eq!(f.returns.as_deref(), Some("bigint"));
        let p_oid = funcs
            .items
            .iter()
            .find(|f| f.name == "close_order")
            .unwrap()
            .oid;
        let p = function::get(&c, p_oid).await.unwrap();
        assert_eq!((p.kind.as_str(), p.returns), ("p", None));
        assert_eq!(
            function::get(&c, 0).await.unwrap_err().key,
            "errors.catalog.objectNotFound"
        );

        let s = sequence::get(&c, "shop", "invoice_seq").await.unwrap();
        assert_eq!(
            (s.data_type.as_str(), s.increment, s.cycle),
            ("integer", 1, false)
        );
        assert_eq!(s.last_value, None, "последовательность не использовалась");
        let owned = sequence::get(&c, "shop", "orders_id_seq").await.unwrap();
        assert_eq!(owned.owned_by.as_deref(), Some("shop.orders.id"));
        // `last_value` виден только с правом SELECT или USAGE на последовательность (так устроен
        // `pg_sequences`); без права — NULL, а не ошибка. Свежий стенд выдаёт право в `init.sql`.
        let readable: bool = c
            .query_one(
                "SELECT has_sequence_privilege(c.oid, 'SELECT,USAGE') FROM pg_class c \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = 'shop' AND c.relname = 'orders_id_seq'",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        if readable {
            assert!(
                owned.last_value.is_some_and(|v| v >= 1000),
                "{addr}: {owned:?}"
            );
            assert!(owned.percent_used.is_some_and(|p| p > 0.0), "{addr}");
        } else {
            assert_eq!(
                (owned.last_value, owned.percent_used),
                (None, None),
                "{addr}"
            );
        }
        assert_eq!(
            sequence::get(&c, "shop", "orders").await.unwrap_err().key,
            "errors.catalog.objectNotFound",
            "таблица — не последовательность"
        );

        let e = table::get(&c, "shop", "missing").await.unwrap_err();
        assert_eq!(e.key, "errors.catalog.objectNotFound");
        assert_eq!(
            table::get(&c, "shop", "active_orders")
                .await
                .unwrap_err()
                .key,
            "errors.catalog.objectNotFound",
            "представление — не таблица"
        );
    }
}

/// Вкладка обычной таблицы не оставляет блокировок отношений: размеры (`pg_relation_size` и т. п.) снимают
/// свою блокировку сразу, разбор выражений и определений идёт по каталогу. Партиционированная таблица (сумма
/// по `pg_partition_tree`, отступление 7 плана) держит блокировки партиций до конца своего запроса, а
/// `pg_get_viewdef` у представления — блокировки самого представления и его таблиц (`AcquireRewriteLocks`):
/// каждый запрос вкладки идёт отдельной транзакцией, и они снимаются, как только запрос ответил.
#[tokio::test]
async fn table_details_keep_no_locks_on_plain_tables() {
    for addr in addrs() {
        let c = client(&addr, "orders").await;
        c.batch_execute("BEGIN READ ONLY").await.unwrap();
        table::get(&c, "shop", "orders").await.unwrap();
        table::get(&c, "shop", "customers").await.unwrap();
        table::get(&c, "shop", "events_2026_09").await.unwrap();
        let locks = held_locks(&c).await;
        c.batch_execute("ROLLBACK").await.unwrap();
        assert!(locks.is_empty(), "{addr}: блокировки отношений: {locks:?}");
    }
}
