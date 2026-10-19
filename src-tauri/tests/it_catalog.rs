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
        let page = objects::list(&c, "shop", ObjectKind::Tables, None, 0, 2)
            .await
            .unwrap();
        assert_eq!((page.total, page.next_offset), (3, Some(2)));
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
        assert_eq!(
            parts
                .items
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["events_2026_09", "events_2026_10"]
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
