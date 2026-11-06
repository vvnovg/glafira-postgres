//! Размеры ответов модели (смоук P4b, G1–G4) на pg-perf (`GPG_IT_PERF`): `get_top_queries` (10 строк, окно 900 с и с
//! момента сброса) и `get_query_details` первой строки окна — сколько байт получает клиент MCP (текст и
//! `structuredContent`). Только печать, без проверок размеров: тест помечен `#[ignore]`, запуск —
//! `cargo test --test it_mcp_sizes -- --ignored --nocapture --test-threads=1`. Окно берётся из буфера сэмплера pgss:
//! тест запускает сэмплер, даёт нагрузку из `dev/perf/heavy.sql` (литералы вместо переменных pgbench) и ждёт второго
//! тика (до 75 с).

mod common;
mod mcp_harness;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::sampler::registry::SamplerSink;
use glafira_pg_lib::sampler::statements::{self, StatementsEvent};
use mcp_harness::{Gateway, set_access, text};
use rmcp::model::CallToolResult;
use serde_json::{Value, json};

/// Запросы `dev/perf/heavy.sql` с литералами (простой протокол); без двух, что отдают клиенту сотни тысяч строк.
const LOAD: [&str; 10] = [
    "SELECT * FROM shop.customers WHERE id = 4242",
    "SELECT count(*) FROM shop.audit WHERE order_id = 7 AND id BETWEEN 1 AND 300000",
    "SELECT count(*) FROM shop.orders WHERE status = 'disputed'",
    "SELECT * FROM shop.orders ORDER BY total DESC LIMIT 10",
    "SELECT count(*) FROM shop.balances WHERE customer_id BETWEEN 4242 AND 4242 + 1000",
    "SELECT oi.id FROM shop.order_items oi WHERE oi.product_id BETWEEN 77 AND 77 + 10 \
     AND oi.price > (SELECT avg(x.price) FROM shop.order_items x WHERE x.order_id = oi.order_id)",
    "SELECT count(*) FROM shop.events WHERE kind = 'pay' AND payload ->> 'k' = '7'",
    "SELECT customer_id % 1000, sum(total * 1.07 + 3), avg(total) FROM shop.orders GROUP BY 1",
    "SELECT count(*) FROM shop.order_items oi JOIN shop.orders o ON o.id = oi.order_id \
     WHERE o.created_at >= timestamptz '2025-01-01' + 100 * interval '1 day' - interval '30 days'",
    "SELECT o.id FROM shop.orders o WHERE o.customer_id = 4242",
];

#[derive(Default)]
struct Ticks(Mutex<Vec<StatementsEvent>>);

impl SamplerSink<StatementsEvent> for Ticks {
    fn send(&self, e: StatementsEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

impl Ticks {
    fn seen(&self, n: u32) -> bool {
        self.0
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(e, StatementsEvent::Tick { ticks, .. } if *ticks == n))
    }
}

/// Байты ответа: текстовые блоки и `structuredContent` (до смоук-правки G3 JSON шёл в обоих).
fn sizes(r: &CallToolResult) -> (usize, usize) {
    let structured = r
        .structured_content
        .as_ref()
        .map_or(0, |v| v.to_string().len());
    (text(r).len(), structured)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "печать размеров ответов: --ignored --nocapture"]
async fn perf_answer_sizes() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "perf").await;
    set_access(&st, "it", LlmAccess::Diagnostics);
    let gw = Gateway::start(&st, dir.path()).await;
    let ticks = Arc::new(Ticks::default());
    let sid = statements::start(&st, "it", None, ticks.clone()).unwrap();
    common::wait_until(
        "первый тик сэмплера pgss",
        Duration::from_secs(20),
        async || ticks.seen(0),
    )
    .await;
    let load = common::superuser(&perf, "perf").await;
    for _ in 0..2 {
        for sql in LOAD {
            load.batch_execute(sql).await.unwrap();
        }
    }
    common::wait_until(
        "второй тик сэмплера pgss",
        Duration::from_secs(75),
        async || ticks.seen(1),
    )
    .await;
    let window = gw
        .call(
            "get_top_queries",
            json!({"connectionId": "it", "windowS": 900, "limit": 10}),
        )
        .await;
    let since = gw
        .call(
            "get_top_queries",
            json!({"connectionId": "it", "mode": "sinceReset", "limit": 10}),
        )
        .await;
    let top: Value = serde_json::from_str(&text(&window)).unwrap();
    let query_id = top["data"]["rows"][0]["key"]["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("в окне нет строк: {top}"))
        .to_string();
    let card = gw
        .call(
            "get_query_details",
            json!({"connectionId": "it", "database": "perf", "queryId": query_id}),
        )
        .await;
    for (name, r) in [
        ("get_top_queries windowS 900 limit 10", &window),
        ("get_top_queries sinceReset limit 10", &since),
        ("get_query_details (first window row)", &card),
    ] {
        assert_eq!(r.is_error, Some(false), "{name}: {}", text(r));
        let (t, s) = sizes(r);
        println!(
            "SIZE {name}: text {t} B, structuredContent {s} B, to the client {} B",
            t + s
        );
    }
    let rows = top["data"]["rows"].as_array().map_or(0, Vec::len);
    let card: Value = serde_json::from_str(&text(&card)).unwrap();
    let history = card["history"].as_array().map_or(0, Vec::len);
    println!("SIZE window rows {rows}, card history points {history}");
    assert!(st.sessions.stop(&sid));
}
