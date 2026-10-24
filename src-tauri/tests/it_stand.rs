//! Стенд P2: роль приложения `glafira_app` на пишущих узлах и лаборатория `pg-lab` (`GPG_IT_LAB`):
//! логический слот, подписка, prepared transactions. Без переменных тест ничего не проверяет.

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
