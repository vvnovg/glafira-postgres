//! Сэмплер активности на стенде (`GPG_IT_SINGLE`): ожидания `pg_sleep` и блокировки, остановка с последним подписчиком;
//! запрос сэмплера на PG 13–18 (`GPG_IT_MATRIX`). Запускать с `--test-threads=1`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::diag::caps::Caps;
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::sampler::{
    self,
    registry::{SamplerEvent, SamplerSink},
};
use pg_advisor::ash::WaitClass;

#[derive(Default)]
struct Events(Mutex<Vec<SamplerEvent>>);
impl SamplerSink for Events {
    fn send(&self, e: SamplerEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

#[tokio::test]
async fn sampler_sees_sleep_and_lock_waits_and_stops_with_the_last_subscriber() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let (_dir, st) = common::open_server(&addr).await;
    let events = Arc::new(Events::default());
    let sid = sampler::start(&st, "it", None, events.clone()).unwrap();

    let holder = common::app(&addr).await;
    holder
        .batch_execute("BEGIN; UPDATE shop.orders SET status = status WHERE id = 2;")
        .await
        .unwrap();
    let waiter = common::app(&addr).await;
    let waiting = tokio::spawn(async move {
        waiter
            .batch_execute("UPDATE shop.orders SET status = status WHERE id = 2")
            .await
    });
    let sleeper = common::app(&addr).await;
    let sleeping = tokio::spawn(async move { sleeper.batch_execute("SELECT pg_sleep(4)").await });
    tokio::time::sleep(Duration::from_secs(3)).await;

    let r = diag::run(
        &st,
        "it",
        DiagRequest {
            tool: Tool::Waits,
            member: None,
            window_s: Some(300),
        },
    )
    .await
    .unwrap();
    let DiagData::Waits(d) = &r.data else {
        panic!()
    };
    assert!(d.sampler_running && d.total_samples >= 2, "{d:?}");
    assert!(
        d.top_events
            .iter()
            .any(|e| e.class == WaitClass::Timeout && e.event.as_deref() == Some("PgSleep")),
        "{:?}",
        d.top_events
    );
    assert!(
        d.top_events
            .iter()
            .any(|e| e.class == WaitClass::Lock && e.event.as_deref() == Some("transactionid")),
        "{:?}",
        d.top_events
    );
    assert!(
        events
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, SamplerEvent::Tick { .. }))
    );

    holder.batch_execute("ROLLBACK").await.unwrap();
    waiting.await.unwrap().unwrap();
    sleeping.await.unwrap().unwrap();
    assert!(st.sessions.stop(&sid));
    let member = r.member.clone().unwrap();
    common::wait_until(
        "сэмплер остановлен",
        Duration::from_secs(3),
        async || !st.samplers.is_running("it", &member),
    )
    .await;
}

/// Запрос сэмплера идёт на каждой версии (на PG 13 `query_id` — `NULL::bigint`) и видит чужую сессию в `pg_sleep`.
#[tokio::test]
async fn sample_sql_runs_on_every_version() {
    let addrs: Vec<String> = [
        "GPG_IT_SINGLE",
        "GPG_IT_PRIMARY",
        "GPG_IT_REPLICA",
        "GPG_IT_LAB",
    ]
    .into_iter()
    .filter_map(common::env)
    .chain(common::matrix_addrs())
    .collect();
    for addr in addrs {
        let sleeper = common::superuser(&addr, "postgres").await;
        let sleeping =
            tokio::spawn(async move { sleeper.batch_execute("SELECT pg_sleep(1.5)").await });
        let c = common::connect_as(
            &addr,
            "postgres",
            common::PG_USER,
            common::PG_PASSWORD_VALUE,
        )
        .await;
        let sql = sampler::query::sample_sql(Caps::new(diag::head(&c).await.unwrap().version_num));
        common::wait_until(
            &format!("{addr}: pg_sleep в сэмпле"),
            Duration::from_secs(3),
            async || {
                c.query(&sql, &[])
                    .await
                    .unwrap_or_else(|e| panic!("{addr}: {e:?}"))
                    .iter()
                    .any(|r| {
                        r.get::<_, Option<String>>(8).as_deref() == Some("PgSleep")
                            && r.get::<_, Option<String>>(5).as_deref() == Some("glafira-it")
                    })
            },
        )
        .await;
        sleeping.await.unwrap().unwrap();
    }
}
