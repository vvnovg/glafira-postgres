//! Действия P2b на стенде: соединение, сессии, слоты (спец. P2 §8, §12.2). Без `GPG_IT_*` тест пропускается.

mod common;

use glafira_pg_lib::actions::conn;

#[tokio::test]
async fn action_connection_is_read_write_and_tagged() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let c = conn::open(&st, "it", &member, None).await.unwrap();
    let row = c
        .client
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), \
                    current_setting('statement_timeout'), current_setting('search_path')",
            &[],
        )
        .await
        .unwrap();
    let got: (String, String, String, String) = (row.get(0), row.get(1), row.get(2), row.get(3));
    assert_eq!(
        got,
        (
            "off".into(),
            "glafira-pg".into(),
            "5s".into(),
            "pg_catalog".into()
        )
    );
}

#[tokio::test]
async fn unknown_member_is_rejected_before_connecting() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let Err(e) = conn::open(&st, "it", "no-such-node", None).await else {
        panic!("узла нет — соединения быть не должно")
    };
    assert_eq!(e.key, "errors.topology.unknownMember");
}
