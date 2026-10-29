//! Стенд P2: роль приложения `glafira_app` на пишущих узлах и лаборатория `pg-lab` (`GPG_IT_LAB`):
//! логический слот, подписка, prepared transactions. Стенд P3: `pg-perf` (`GPG_IT_PERF`) — расширения и объёмная
//! схема. Без переменных тест ничего не проверяет.

mod common;

#[tokio::test]
async fn app_role_exists_on_every_writable_node() {
    for addr in common::writable_addrs().await {
        common::ensure_fixtures(&addr).await;
        let c = common::app(&addr).await;
        let n: i64 = c
            .query_one("SELECT count(*) FROM shop.orders", &[])
            .await
            .unwrap()
            .get(0);
        assert!(n > 0, "{addr}");
        let sup: bool = c
            .query_one(
                "SELECT rolsuper FROM pg_roles WHERE rolname = current_user",
                &[],
            )
            .await
            .unwrap()
            .get(0);
        assert!(!sup, "{addr}: glafira_app не суперпользователь");
    }
}

#[tokio::test]
async fn lab_has_logical_slot_subscription_and_prepared_transactions() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    let c = common::superuser(&lab, "orders").await;
    let r = c
        .query_one(
            "SELECT current_setting('wal_level'), current_setting('max_prepared_transactions')::int, \
             (SELECT slot_type || ':' || plugin FROM pg_replication_slots WHERE slot_name = 'lab_sub_slot'), \
             (SELECT count(*) FROM pg_stat_subscription WHERE subname = 'lab_sub')::bigint",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(r.get::<_, String>(0), "logical");
    assert_eq!(r.get::<_, i32>(1), 10);
    assert_eq!(
        r.get::<_, Option<String>>(2).as_deref(),
        Some("logical:pgoutput")
    );
    assert!(r.get::<_, i64>(3) >= 1, "воркер подписки запущен");
}

/// Стенд P3 (`GPG_IT_PERF`): PG 17 с pg_stat_statements (track_planning), hypopg, pg_stat_kcache, pg_wait_sampling,
/// объёмной схемой и ролью диагностики с правом чтения таблиц.
#[tokio::test]
async fn perf_has_extensions_settings_and_data() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let c = common::superuser(&perf, "perf").await;
    let r = c
        .query_one(
            "SELECT current_setting('server_version_num')::int, \
                    current_setting('pg_stat_statements.track_planning'), current_setting('compute_query_id'), \
                    (SELECT array_agg(extname::text ORDER BY extname) FROM pg_extension \
                      WHERE extname IN ('pg_stat_statements', 'pg_stat_kcache', 'pg_wait_sampling', 'hypopg')), \
                    (SELECT count(*) FROM shop.orders)::bigint, \
                    (SELECT count(*) FROM pg_inherits WHERE inhparent = 'shop.events'::regclass)::bigint, \
                    pg_has_role('glafira_diag', 'pg_read_all_data', 'USAGE')",
            &[],
        )
        .await
        .unwrap();
    assert!(r.get::<_, i32>(0) >= 170_000);
    assert_eq!(r.get::<_, String>(1), "on");
    assert_eq!(r.get::<_, String>(2), "on");
    assert_eq!(
        r.get::<_, Vec<String>>(3),
        [
            "hypopg",
            "pg_stat_kcache",
            "pg_stat_statements",
            "pg_wait_sampling"
        ]
    );
    assert!(r.get::<_, i64>(4) >= 500_000, "заказы загружены");
    assert_eq!(r.get::<_, i64>(5), 24, "24 партиции событий");
    assert!(
        r.get::<_, bool>(6),
        "glafira_diag читает таблицы приложения (EXPLAIN)"
    );
    // Роль диагностики видит статистику запросов других ролей. Соединение продукта идёт с `search_path = pg_catalog`
    // (`SESSION_OPTIONS`), поэтому представление расширения называется со схемой, как в запросах приложения.
    let d = common::connect_as(&perf, "perf", common::PG_USER, common::PG_PASSWORD_VALUE).await;
    let n: i64 = d
        .query_one(
            "SELECT count(*) FROM public.pg_stat_statements WHERE queryid IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(n >= 1, "pg_stat_statements заполнен хотя бы init-скриптом");
}
