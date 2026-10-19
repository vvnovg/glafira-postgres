//! Каталог демо-схемы на узлах стенда: PG 15 (`GPG_IT_SINGLE`), 16 (`GPG_IT_PRIMARY`), 17 (Patroni).

mod common;

use glafira_pg_lib::catalog::objects::ObjectKind;
use glafira_pg_lib::catalog::{databases, extensions, objects, roles, schemas};
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
    }
}
