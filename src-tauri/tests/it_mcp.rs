//! MCP-шлюз против стенда (спец. P4 §13 «Интеграция», часть P4a): сервер `glafira-pg-mcp` в том же процессе поверх
//! `tokio::io::duplex`, клиент rmcp, данные — функции приложения `mcp::read::{read_op, observe_op}` на открытом
//! подключении (те же, что у `AppBackend`). `Gate` теста подтверждает любого клиента (подтверждение, политика и
//! `mcp.json` — unit-тесты). Здесь проверяется путь модели целиком: клиент → протокол → ядро (право, слоты, журнал,
//! предел ответа, ход наблюдения) → данные приложения → стенд; слои `GuardTx` и брошенные вызовы бэкенда — в
//! `it_mcp_backend`. Части: pg-lab (`GPG_IT_LAB`) — сессии, блокировки, горизонт и маскирование живых текстов,
//! наблюдение активности; Patroni (`GPG_IT_PATRONI`) — топология, сводка, `failover` и `health`; реплика
//! (`GPG_IT_REPLICA`) — пароль `primary_conninfo`; матрица PG 13–18 (`GPG_IT_MATRIX`) — настройки и таблица с `pg_stats`;
//! pg-perf (`GPG_IT_PERF`, workflow perf) — топ, наблюдение pgss, EXPLAIN и hypopg. Запускать с `--test-threads=1`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_mcp_core::audit::AuditLog;
use glafira_mcp_core::error::{BoxFuture, ToolError};
use glafira_mcp_core::gate::{Gate, SessionAccess, SessionView};
use glafira_mcp_core::server::{GatewayOptions, serve_stream};
use glafira_pg_lib::conn::profile::LlmAccess;
use glafira_pg_lib::mcp::read::{observe_op, read_op};
use glafira_pg_lib::state::AppState;
use glafira_pg_mcp::model::{ObserveCtx, PgAnswer, PgBackend, PgObserve, PgRead};
use glafira_pg_mcp::server::{INSTRUCTIONS, PgFactory};
use rmcp::model::{CallToolRequestParams, CallToolResult, ProgressNotificationParam};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};

/// Бэкенд теста: данные — те же функции, что у `AppBackend`; клиент подтверждён заранее.
struct Backend {
    st: AppState,
}

impl Gate for Backend {
    fn access(&self, _session: &SessionView) -> SessionAccess {
        SessionAccess::Approved
    }

    fn connection_name(&self, id: &str) -> Option<String> {
        self.st.connections.get(id).ok().map(|p| p.name)
    }

    fn session_closed(&self, _session_id: &str) {}

    fn sessions_changed(&self) {}
}

impl PgBackend for Backend {
    fn read(&self, op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(read_op(&self.st, op))
    }

    fn observe(
        &self,
        op: PgObserve,
        ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(observe_op(&self.st, op, ctx))
    }
}

/// Подключение `it` открыто для модели. `llmAccess` — локальное поле профиля: соединение не переоткрывается.
fn allow_llm(st: &AppState) {
    let mut p = st.profile("it").unwrap();
    p.llm_access = LlmAccess::Diagnostics;
    st.connections.upsert(p).unwrap();
}

/// Сервер одной сессии поверх `duplex`, клиент rmcp и журнал MCP (во временной папке, живущей с `Gateway`).
struct Gateway<H: ClientHandler = ()> {
    _dir: tempfile::TempDir,
    audit: Arc<AuditLog>,
    client: RunningService<RoleClient, H>,
}

async fn gateway(st: AppState) -> Gateway {
    gateway_with(st, ()).await
}

async fn gateway_with<H: ClientHandler>(st: AppState, handler: H) -> Gateway<H> {
    allow_llm(&st);
    let dir = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(dir.path()));
    let backend = Arc::new(Backend { st });
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(serve_stream(
        Box::new(server_io),
        backend.clone(),
        audit.clone(),
        GatewayOptions::new("glafira-pg", "it", INSTRUCTIONS),
        Arc::new(PgFactory::new(backend)),
    ));
    let client = handler.serve(client_io).await.unwrap();
    Gateway {
        _dir: dir,
        audit,
        client,
    }
}

async fn call<H: ClientHandler>(
    c: &RunningService<RoleClient, H>,
    tool: &str,
    args: Value,
) -> CallToolResult {
    let mut p = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    c.call_tool(p).await.unwrap()
}

fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ok(r: CallToolResult) -> Value {
    assert_eq!(r.is_error, Some(false), "{}", text(&r));
    r.structured_content.unwrap()
}

fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "{}", text(r));
    text(r)
}

/// Последняя запись журнала MCP о вызове `tool` (журнал пишется до ответа клиенту).
fn logged(audit: &AuditLog, tool: &str) -> Value {
    audit
        .tail(50)
        .into_iter()
        .find(|r| r["tool"] == tool)
        .unwrap_or_else(|| panic!("{tool} нет в журнале MCP"))
}

/// Клиент, который запоминает уведомления о ходе (`notifications/progress`).
#[derive(Clone, Default)]
struct ProgressLog(Arc<Mutex<Vec<ProgressNotificationParam>>>);

impl ClientHandler for ProgressLog {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _: NotificationContext<RoleClient>,
    ) {
        self.0.lock().unwrap().push(params);
    }
}

impl ProgressLog {
    /// Ход наблюдения дошёл до клиента: не меньше двух уведомлений, все — из `total` секунд, ход не убывает.
    /// Обработчик клиента может отставать от ответа — уведомления ждутся.
    async fn assert_reported(&self, total: f64) {
        common::wait_until(
            "уведомления о ходе наблюдения",
            Duration::from_secs(3),
            async || self.0.lock().unwrap().len() >= 2,
        )
        .await;
        let notes = self.0.lock().unwrap().clone();
        assert!(notes.iter().all(|n| n.total == Some(total)), "{notes:?}");
        assert!(
            notes.windows(2).all(|w| w[0].progress <= w[1].progress),
            "ход не убывает: {notes:?}"
        );
    }
}

async fn pid(c: &tokio_postgres::Client) -> i32 {
    c.query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0)
}

/// Бэкенд без базы: `tools/list` и рукопожатие отвечают без данных, любой вызов данных — ошибка.
struct NoData;

impl Gate for NoData {
    fn access(&self, _session: &SessionView) -> SessionAccess {
        SessionAccess::Approved
    }

    fn connection_name(&self, _id: &str) -> Option<String> {
        None
    }

    fn session_closed(&self, _session_id: &str) {}

    fn sessions_changed(&self) {}
}

impl PgBackend for NoData {
    fn read(&self, _op: PgRead) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::not_allowed()) })
    }

    fn observe(
        &self,
        _op: PgObserve,
        _ctx: ObserveCtx,
    ) -> BoxFuture<'_, Result<PgAnswer, ToolError>> {
        Box::pin(async { Err(ToolError::not_allowed()) })
    }
}

/// Без стенда (§17 P4a, критерий 2): по протоколу сервер отдаёт ровно 14 инструментов, у каждого все четыре подсказки
/// (`crates/glafira-pg-mcp/tests/tools.rs` проверяет то же на уровне крейта; здесь — через приложение и `serve_stream`).
#[tokio::test(flavor = "multi_thread")]
async fn tools_list_has_the_fourteen_tools_with_read_only_annotations() {
    let dir = tempfile::tempdir().unwrap();
    let audit = Arc::new(AuditLog::new(dir.path()));
    let backend = Arc::new(NoData);
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(serve_stream(
        Box::new(server_io),
        backend.clone(),
        audit,
        GatewayOptions::new("glafira-pg", "it", INSTRUCTIONS),
        Arc::new(PgFactory::new(backend)),
    ));
    let client = ().serve(client_io).await.unwrap();
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(
        names,
        [
            "explain_query",
            "get_activity",
            "get_health_summary",
            "get_query_details",
            "get_settings",
            "get_table_details",
            "get_top_queries",
            "get_topology",
            "hypothetical_index",
            "list_connections",
            "list_diagnostics",
            "observe_activity",
            "observe_top_queries",
            "run_diagnostic",
        ]
    );
    for t in &tools {
        let a = t
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{}: нет подсказок", t.name));
        assert_eq!(
            (
                a.read_only_hint,
                a.destructive_hint,
                a.idempotent_hint,
                a.open_world_hint
            ),
            (Some(true), Some(false), Some(true), Some(false)),
            "{}",
            t.name
        );
    }
}

/// Счётчик записей в таблицах `shop` (вставки + изменения + удаления; откаченные тоже считаются). Статистика
/// сбрасывается в разделяемую память с задержкой до секунды — перед чтением ждём.
async fn shop_writes(su: &tokio_postgres::Client) -> i64 {
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    su.query_one(
        "SELECT coalesce(sum(n_tup_ins + n_tup_upd + n_tup_del), 0)::bigint \
         FROM pg_stat_user_tables WHERE schemaname = 'shop'",
        &[],
    )
    .await
    .unwrap()
    .get(0)
}

/// Держатель с XID и блокировкой таблицы, жертва ждёт; литералы и комментарий в текстах не доходят до модели ни в
/// одном инструменте, а журнал MCP получает их число.
#[tokio::test(flavor = "multi_thread")]
async fn lab_sessions_locks_and_horizon_are_read_with_masked_texts() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    common::ensure_fixtures(&lab).await;
    let (_dir, st) = common::open_server_in(&lab, "orders").await;
    let g = gateway(st).await;
    let c = &g.client;
    assert_eq!(c.list_all_tools().await.unwrap().len(), 14);
    let su = common::superuser(&lab, "orders").await;
    let writes_before = shop_writes(&su).await;
    let conns = ok(call(c, "list_connections", json!({})).await);
    assert_eq!(
        (
            &conns["connections"][0]["kind"],
            &conns["connections"][0]["llmAccess"],
            &conns["connections"][0]["connected"]
        ),
        (&json!("server"), &json!("diagnostics"), &json!(true)),
        "{conns}"
    );

    let holder = common::app(&lab).await;
    let holder_pid = pid(&holder).await;
    holder
        .batch_execute(
            "BEGIN; UPDATE shop.orders SET status = status WHERE id = 1; \
             LOCK TABLE shop.orders IN ACCESS EXCLUSIVE MODE; SELECT 'holder-secret-41' /* tenant=acme */",
        )
        .await
        .unwrap();
    let victim = common::app(&lab).await;
    let victim_pid = pid(&victim).await;
    let waiting = tokio::spawn(async move {
        victim
            .batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'victim-secret-42'")
            .await
    });
    let probe =
        common::connect_as(&lab, "orders", common::PG_USER, common::PG_PASSWORD_VALUE).await;
    common::wait_lock_wait(&probe, victim_pid, "SELECT жертвы ждёт блокировку").await;
    let secrets = ["holder-secret-41", "victim-secret-42", "tenant=acme"];
    let clean = |v: &Value| {
        let s = v.to_string();
        for secret in secrets {
            assert!(!s.contains(secret), "{secret} ушёл модели: {s}");
        }
    };

    let sessions = ok(call(
        c,
        "run_diagnostic",
        json!({"connectionId": "it", "tool": "sessions"}),
    )
    .await);
    clean(&sessions);
    let row = sessions["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["pid"] == holder_pid)
        .expect("держатель виден")
        .clone();
    let q = row["query"].as_str().unwrap();
    assert!(
        q.contains("shop.orders") && q.contains('$') && q.contains("/*…*/"),
        "{q}"
    );
    assert!(
        sessions["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["title"].is_string() && f["fix"].is_string() && f.get("links").is_none()),
        "{sessions}"
    );
    // Счётчик литералов дошёл от бэкенда до журнала: у держателя два, у жертвы один.
    let rec = logged(&g.audit, "run_diagnostic");
    assert!(rec["masked"].as_u64().is_some_and(|n| n >= 3), "{rec}");
    assert_eq!(
        rec["connection"],
        json!({"id": "it", "name": "it"}),
        "{rec}"
    );

    let activity = ok(call(
        c,
        "get_activity",
        json!({"connectionId": "it", "waitEventType": "Lock"}),
    )
    .await);
    clean(&activity);
    assert!(activity["total"].as_u64().unwrap() >= 1, "{activity}");
    let victim_row = activity["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["pid"] == victim_pid)
        .unwrap_or_else(|| panic!("жертва в ожидании Lock: {activity}"));
    assert!(
        victim_row["query"]
            .as_str()
            .is_some_and(|q| q.contains("status = $")),
        "{victim_row}"
    );

    let locks = ok(call(
        c,
        "run_diagnostic",
        json!({"connectionId": "it", "tool": "locks"}),
    )
    .await);
    clean(&locks);
    assert!(
        locks["data"]["roots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["session"]["pid"] == holder_pid),
        "{locks}"
    );

    let horizon = ok(call(
        c,
        "run_diagnostic",
        json!({"connectionId": "it", "tool": "horizon"}),
    )
    .await);
    clean(&horizon);
    assert!(
        horizon["data"]["holders"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["pid"] == holder_pid),
        "держатель с XID держит горизонт: {horizon}"
    );

    let health = ok(call(c, "get_health_summary", json!({"connectionId": "it"})).await);
    clean(&health);
    assert_eq!(health["pulse"]["kind"], "pulse");
    assert!(
        health["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| !u["reason"]
                .as_str()
                .unwrap_or("")
                .starts_with("did not finish")),
        "{health}"
    );

    let unknown = err(&call(
        c,
        "run_diagnostic",
        json!({"connectionId": "nope", "tool": "sessions"}),
    )
    .await);
    assert!(unknown.starts_with("not_allowed:"), "{unknown}");

    // Чтение не пишет: счётчики записей `shop.*` не сдвинулись (держатель сбросит свои при откате — после замера).
    assert_eq!(
        shop_writes(&su).await,
        writes_before,
        "инструменты чтения ничего не записали в shop.*"
    );
    holder.batch_execute("ROLLBACK").await.unwrap();
    waiting.await.unwrap().unwrap();
}

/// Наблюдение активности через протокол: подписка на сэмплер узла на 10 с ловит идущие запросы, литералы не уходят
/// модели (их число — в журнале), ход доходит до клиента уведомлениями.
#[tokio::test(flavor = "multi_thread")]
async fn lab_observe_activity_sees_running_queries_without_literals() {
    let Some(lab) = common::lab_addr() else {
        return;
    };
    common::ensure_fixtures(&lab).await;
    let (_dir, st) = common::open_server_in(&lab, "orders").await;
    let log = ProgressLog::default();
    let g = gateway_with(st, log.clone()).await;
    let app = common::app(&lab).await;
    let load = tokio::spawn(async move {
        for _ in 0..6 {
            app.batch_execute("SELECT pg_sleep(2), 'observe-secret-7'")
                .await
                .unwrap();
        }
    });
    let v = ok(call(
        &g.client,
        "observe_activity",
        json!({"connectionId": "it", "durationSeconds": 10}),
    )
    .await);
    load.await.unwrap();
    assert!(v["samples"].as_u64().unwrap() >= 5, "{v}");
    assert!(v["peakSessions"].as_u64().unwrap() >= 1, "{v}");
    assert!(
        v["topQueries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["query"].as_str().is_some_and(|t| t.contains("pg_sleep"))),
        "{v}"
    );
    assert!(!v.to_string().contains("observe-secret-7"), "{v}");
    let rec = logged(&g.audit, "observe_activity");
    assert!(rec["masked"].as_u64().is_some_and(|n| n >= 2), "{rec}");
    log.assert_reported(10.0).await;
}

/// Patroni: топология (роли, пауза), сводка по трём узлам без «не успели», кластерные `failover` и `health`, каталог.
#[tokio::test(flavor = "multi_thread")]
async fn patroni_topology_summary_and_cluster_tools() {
    let Some((_dir, st)) = common::open_patroni().await else {
        return;
    };
    let g = gateway(st).await;
    let c = &g.client;
    let conns = ok(call(c, "list_connections", json!({})).await);
    assert_eq!(
        (
            &conns["connections"][0]["kind"],
            &conns["connections"][0]["connected"]
        ),
        (&json!("patroni"), &json!(true)),
        "{conns}"
    );
    let t = ok(call(c, "get_topology", json!({"connectionId": "it"})).await);
    assert_eq!(t["kind"], "patroni");
    let members = t["members"].as_array().unwrap();
    assert_eq!(members.len(), 3, "{t}");
    assert_eq!(
        members.iter().filter(|m| m["role"] == "primary").count(),
        1,
        "{t}"
    );
    assert_eq!(t["patroni"]["paused"], false);
    let h = ok(call(c, "get_health_summary", json!({"connectionId": "it"})).await);
    assert_eq!(h["pulse"]["nodes"].as_array().unwrap().len(), 3, "{h}");
    assert!(
        h["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|u| u["tool"] != "failover" && u["tool"] != "health"),
        "{h}"
    );
    // Реплики могут ещё догонять после соседних тестов (failover, пауза): ждём два готовых узла, но не дольше 30 с.
    let mut f = Value::Null;
    common::wait_until(
        "два готовых узла в сводке failover",
        Duration::from_secs(30),
        async || {
            f = ok(call(
                c,
                "run_diagnostic",
                json!({"connectionId": "it", "tool": "failover"}),
            )
            .await);
            f["data"]["readyCount"] == json!(2)
        },
    )
    .await;
    assert_eq!(f["data"]["kind"], "failover", "{f}");
    assert!(f["member"].is_null(), "кластерный инструмент без узла: {f}");
    let health = ok(call(
        c,
        "run_diagnostic",
        json!({"connectionId": "it", "tool": "health"}),
    )
    .await);
    assert_eq!(
        health["data"]["members"].as_array().unwrap().len(),
        3,
        "{health}"
    );
    let catalog = ok(call(c, "list_diagnostics", json!({})).await);
    assert!(
        catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == "failover" && t["patroniOnly"] == true),
        "{catalog}"
    );
}

/// Реплика пары: пароль `primary_conninfo` (его пишет `pg_basebackup -R`) модели не уходит ни по имени, ни в списке
/// изменённых параметров; параметр помечен `hidden`.
#[tokio::test(flavor = "multi_thread")]
async fn replica_conninfo_password_is_hidden() {
    let Some(replica) = common::env("GPG_IT_REPLICA") else {
        return;
    };
    let su = common::connect_as(
        &replica,
        "postgres",
        common::SUPERUSER,
        common::SUPERUSER_PASSWORD,
    )
    .await;
    let raw: String = su
        .query_one("SHOW primary_conninfo", &[])
        .await
        .unwrap()
        .get(0);
    // Предусловие: на стенде пароль в `primary_conninfo` есть — иначе тест прошёл бы вхолостую.
    let secret = raw
        .split_whitespace()
        .find_map(|kv| kv.strip_prefix("password="))
        .map(|p| p.trim_matches('\'').to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| panic!("в primary_conninfo реплики стенда нет пароля: {raw}"));
    let (_dir, st) = common::open_server(&replica).await;
    let g = gateway(st).await;
    let c = &g.client;
    let v = ok(call(
        c,
        "get_settings",
        json!({"connectionId": "it", "names": ["primary_conninfo"]}),
    )
    .await);
    let s = &v["settings"][0];
    assert_eq!(s["name"], "primary_conninfo", "{v}");
    let setting = s["setting"].as_str().unwrap();
    assert!(!setting.contains("password"), "{setting}");
    assert!(!v.to_string().contains(&secret), "{v}");
    assert_eq!(s["hidden"], true, "{v}");
    assert!(
        setting.contains("user=replicator"),
        "остальные пары на месте: {setting}"
    );
    // Изменённые параметры (по умолчанию): `primary_conninfo` из `postgresql.auto.conf` — среди них, тоже без пароля.
    let changed = ok(call(c, "get_settings", json!({"connectionId": "it"})).await);
    assert!(!changed.to_string().contains(&secret), "{changed}");
    let conninfo = changed["settings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "primary_conninfo")
        .unwrap_or_else(|| panic!("primary_conninfo изменён на реплике: {changed}"));
    assert_eq!(conninfo["setting"], s["setting"], "{changed}");
}

/// Матрица PG 13–18: изменённые настройки и названные, вкладка таблицы с `pg_stats` (суперпользователь видит статистику
/// всех колонок; на уровне `diagnostics` — без значений).
#[tokio::test(flavor = "multi_thread")]
async fn matrix_settings_and_table_details_on_every_version() {
    for addr in common::matrix_addrs() {
        let (_dir, st) = common::open_server_as(
            &addr,
            "orders",
            common::SUPERUSER,
            common::SUPERUSER_PASSWORD,
        )
        .await;
        let g = gateway(st).await;
        let c = &g.client;
        let named = ok(call(
            c,
            "get_settings",
            json!({
                "connectionId": "it", "names": ["shared_preload_libraries", "MAX_CONNECTIONS"]
            }),
        )
        .await);
        let names: Vec<&str> = named["settings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["max_connections", "shared_preload_libraries"],
            "{addr}"
        );
        let changed = ok(call(c, "get_settings", json!({"connectionId": "it"})).await);
        let rows = changed["settings"].as_array().unwrap();
        assert!(!rows.is_empty(), "{addr}");
        assert!(
            rows.iter()
                .all(|s| !["default", "override", "client", "session"]
                    .contains(&s["source"].as_str().unwrap_or(""))),
            "{addr}: {changed}"
        );
        let t = ok(call(
            c,
            "get_table_details",
            json!({
                "connectionId": "it", "database": "orders", "schema": "shop", "table": "orders"
            }),
        )
        .await);
        assert_eq!(t["table"]["name"], "orders", "{addr}");
        assert!(
            t["table"]["ddl"].as_str().unwrap().contains("CREATE TABLE"),
            "{addr}"
        );
        let stats = t["columnStats"].as_array().unwrap();
        assert_eq!(stats[0]["column"], "id", "{addr}: порядок колонок таблицы");
        assert!(
            stats
                .iter()
                .any(|s| s["column"] == "status" && s["nDistinct"].is_number()),
            "{addr}: {t}"
        );
        assert!(
            stats.iter().all(|s| s.get("mostCommonVals").is_none()
                && s.get("mostCommonFreqs").is_none()
                && s.get("histogramBounds").is_none()),
            "{addr}: {t}"
        );
    }
}

/// Топ с момента сброса и окно без истории (подсказка), наблюдение pgss под нагрузкой: окно — от начала вызова и в
/// пределах срока ядра (`durationSeconds + 6`), ход — уведомлениями.
#[tokio::test(flavor = "multi_thread")]
async fn perf_top_queries_and_statements_observation() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let log = ProgressLog::default();
    let g = gateway_with(st, log.clone()).await;
    let c = &g.client;
    let warm = common::superuser(&perf, "perf").await;
    for i in 0..20_i64 {
        warm.query(
            "SELECT count(*) FROM shop.orders WHERE customer_id = $1",
            &[&i],
        )
        .await
        .unwrap();
    }
    // Запрос с характерным литералом (простым протоколом — литерал в тексте): до модели он не доходит ни в топе,
    // ни в диагностике `queries`.
    warm.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'top-secret-lit-93'")
        .await
        .unwrap();
    let top = ok(call(
        c,
        "get_top_queries",
        json!({"connectionId": "it", "mode": "sinceReset", "limit": 5}),
    )
    .await);
    assert!(!top.to_string().contains("top-secret-lit-93"), "{top}");
    let found = ok(call(
        c,
        "get_top_queries",
        json!({
            "connectionId": "it", "mode": "sinceReset", "search": "FROM shop.orders WHERE status", "limit": 50
        }),
    )
    .await);
    assert!(
        found["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["text"]
                .as_str()
                .is_some_and(|t| t.contains("shop.orders"))),
        "запрос с литералом в топе: {found}"
    );
    assert!(!found.to_string().contains("top-secret-lit-93"), "{found}");
    let diag = ok(call(
        c,
        "run_diagnostic",
        json!({"connectionId": "it", "tool": "queries"}),
    )
    .await);
    assert!(!diag.to_string().contains("top-secret-lit-93"), "{diag}");
    assert_eq!(top["tool"], "queries");
    let rows = top["data"]["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 5, "{top}");
    assert!(top["hint"].is_null(), "{top}");
    let window = ok(call(
        c,
        "get_top_queries",
        json!({"connectionId": "it", "mode": "window", "windowS": 300}),
    )
    .await);
    assert_eq!(window["data"]["coverageS"], 0, "{window}");
    assert!(
        window["hint"]
            .as_str()
            .is_some_and(|h| h.contains("observe_top_queries")),
        "{window}"
    );

    let load = tokio::spawn(async move {
        for i in 0..40_i64 {
            warm.query(
                "SELECT count(*) FROM shop.orders WHERE customer_id = $1",
                &[&i],
            )
            .await
            .unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let observed = ok(call(
        c,
        "observe_top_queries",
        json!({"connectionId": "it", "durationSeconds": 10, "limit": 10}),
    )
    .await);
    load.await.unwrap();
    assert_eq!(observed["data"]["mode"]["kind"], "window", "{observed}");
    let rows = observed["data"]["rows"].as_array().unwrap();
    assert!(rows.len() <= 10, "{observed}");
    assert!(
        rows.iter().any(|r| r["text"]
            .as_str()
            .is_some_and(|t| t.contains("customer_id"))),
        "нагрузка окна видна: {observed}"
    );
    let rec = logged(&g.audit, "observe_top_queries");
    assert_eq!(rec["outcome"], "ok", "{rec}");
    let ms = rec["ms"].as_u64().unwrap();
    assert!(
        (9_000..16_000).contains(&ms),
        "окно — 10 с от начала вызова, вызов — до срока ядра (16 с): {rec}"
    );
    log.assert_reported(10.0).await;
}

/// EXPLAIN модели: план без выполнения, отказ враждебного SQL на разборе и на пробе плана (встроенная SQL-функция),
/// `queryId` из топа с GENERIC_PLAN, гипотезы hypopg; запись только планируется (строки на месте), статистика базы не
/// сброшена.
#[tokio::test(flavor = "multi_thread")]
async fn perf_explain_and_hypotheses_never_change_the_database() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let su = common::superuser(&perf, "perf").await;
    // Встраиваемая SQL-функция с запрещённым вызовом: разбор её не видит, проба VERBOSE-плана — видит.
    su.batch_execute(
        "CREATE OR REPLACE FUNCTION shop.it_mcp_cfg() RETURNS text LANGUAGE sql VOLATILE \
         AS $$SELECT set_config('glafira.it_mcp', '1', false)$$",
    )
    .await
    .unwrap();
    let _drop_fn = common::restore_on_drop({
        let perf = perf.clone();
        move || common::run_sql_blocking(&perf, "perf", "DROP FUNCTION IF EXISTS shop.it_mcp_cfg()")
    });
    su.query(
        "SELECT count(*) FROM shop.orders WHERE status = $1",
        &[&"disputed"],
    )
    .await
    .unwrap();
    let count = || async {
        su.query_one("SELECT count(*) FROM shop.orders", &[])
            .await
            .unwrap()
            .get::<_, i64>(0)
    };
    let stats_reset = || async {
        su.query_one(
            "SELECT coalesce(stats_reset::text, '') FROM pg_stat_database WHERE datname = current_database()",
            &[],
        )
        .await
        .unwrap()
        .get::<_, String>(0)
    };
    let (rows_before, reset_before) = (count().await, stats_reset().await);
    let writes_before = shop_writes(&su).await;
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let g = gateway(st).await;
    let c = &g.client;
    let explain = |sql: &str| json!({"connectionId": "it", "database": "perf", "sql": sql});

    let v = ok(call(
        c,
        "explain_query",
        explain("SELECT count(*) FROM shop.orders WHERE status = 'disputed'"),
    )
    .await);
    let plan = v["planText"].as_str().unwrap();
    assert!(
        plan.contains("orders") && plan.contains("disputed"),
        "SQL модели не маскируется: {plan}"
    );
    assert!(v["summary"]["totalCost"].as_f64().unwrap() > 0.0, "{v}");
    assert!(
        v["tables"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t == "shop.orders"),
        "{v}"
    );
    assert!(logged(&g.audit, "explain_query").get("masked").is_none());

    for (sql, code) in [
        ("SELECT pg_sleep(1)", "forbidden_function:"),
        (
            "SELECT * FROM shop.orders WHERE pg_catalog.set_config('a.b', '1', false) IS NOT NULL",
            "forbidden_function:",
        ),
        (
            "SELECT pg_terminate_backend(pg_backend_pid())",
            "forbidden_function:",
        ),
        // Расширенный список (сброс статистики, SQL из строки, имена PG 13–14, снимок резервной копии).
        ("SELECT pg_stat_reset()", "forbidden_function:"),
        (
            "SELECT * FROM ts_stat('SELECT to_tsvector(status) FROM shop.orders')",
            "forbidden_function:",
        ),
        ("SELECT pg_start_backup('it')", "forbidden_function:"),
        ("SELECT pg_log_standby_snapshot()", "forbidden_function:"),
        ("SELECT 1; DELETE FROM shop.orders", "invalid_sql:"),
        ("SET work_mem = '1GB'", "invalid_sql:"),
        ("DO $$ BEGIN PERFORM 1; END $$", "invalid_sql:"),
        ("SELECT * FROM shop.no_such_table", "invalid_sql:"),
    ] {
        let e = err(&call(c, "explain_query", explain(sql)).await);
        assert!(e.starts_with(code), "{sql}: {e}");
    }
    // В тексте — только имя своей функции: запрещённый вызов находит проба VERBOSE-плана (функция встроена).
    let e = err(&call(c, "explain_query", explain("SELECT shop.it_mcp_cfg()")).await);
    assert!(
        e.starts_with("forbidden_function:")
            && e.contains("set_config")
            && e.contains("inlined SQL function"),
        "запрещённое найдено в плане, а не в тексте: {e}"
    );

    // Путь модели: `queryId` из топа (поиск — по замаскированному тексту pgss), затем план по нему. Текст pgss самого
    // запроса, а не обёртки EXPLAIN (её строки тоже находит поиск).
    let top = ok(call(
        c,
        "get_top_queries",
        json!({
            "connectionId": "it", "mode": "sinceReset", "search": "FROM shop.orders WHERE status", "limit": 50
        }),
    )
    .await);
    let query_id = top["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["text"].as_str().is_some_and(|t| {
                let t = t.to_lowercase();
                t.starts_with("select count(*) from shop.orders where status = $1")
            })
        })
        .and_then(|r| r["key"]["queryId"].as_str())
        .unwrap_or_else(|| panic!("запрос в pgss: {top}"))
        .to_string();
    let by_id = ok(call(
        c,
        "explain_query",
        json!({"connectionId": "it", "database": "perf", "queryId": query_id}),
    )
    .await);
    assert!(
        by_id["planText"].as_str().unwrap().contains("$1"),
        "GENERIC_PLAN на PG 17: {by_id}"
    );
    assert!(
        by_id["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("GENERIC_PLAN")),
        "{by_id}"
    );

    let hypo = ok(call(
        c,
        "hypothetical_index",
        json!({
            "connectionId": "it", "database": "perf",
            "sql": "SELECT count(*) FROM shop.orders WHERE status = $1", "params": ["disputed"],
            "indexes": ["CREATE INDEX ON shop.orders (status)", "CREATE INDEX ON shop.orders (total)"]
        }),
    )
    .await);
    assert_eq!(
        (&hypo["indexes"][0]["used"], &hypo["indexes"][1]["used"]),
        (&json!(true), &json!(false)),
        "{hypo}"
    );
    assert!(
        hypo["indexes"][0]["estimatedSizeBytes"]
            .as_i64()
            .is_some_and(|b| b > 0),
        "{hypo}"
    );
    assert!(
        hypo["costAfter"].as_f64().unwrap() < hypo["costBefore"].as_f64().unwrap(),
        "{hypo}"
    );
    let hypo_call = |sql: &str, idx: &str| {
        json!({
            "connectionId": "it", "database": "perf", "sql": sql, "indexes": [idx]
        })
    };
    let ddl = "CREATE INDEX ON shop.orders (status)";
    // Тот же враждебный набор, что у `explain_query`, — в `sql` гипотезы при корректном DDL индекса.
    for (sql, code) in [
        ("SELECT pg_sleep(1)", "forbidden_function:"),
        (
            "SELECT * FROM shop.orders WHERE pg_catalog.set_config('a.b', '1', false) IS NOT NULL",
            "forbidden_function:",
        ),
        (
            "SELECT pg_terminate_backend(pg_backend_pid())",
            "forbidden_function:",
        ),
        (
            "SELECT * FROM ts_stat('SELECT to_tsvector(status) FROM shop.orders')",
            "forbidden_function:",
        ),
        ("SELECT 1; DELETE FROM shop.orders", "invalid_sql:"),
        ("DO $$ BEGIN PERFORM 1; END $$", "invalid_sql:"),
    ] {
        let e = err(&call(c, "hypothetical_index", hypo_call(sql, ddl)).await);
        assert!(e.starts_with(code), "hypothetical_index sql {sql}: {e}");
    }
    let e = err(&call(
        c,
        "hypothetical_index",
        hypo_call("SELECT shop.it_mcp_cfg()", ddl),
    )
    .await);
    assert!(
        e.starts_with("forbidden_function:") && e.contains("set_config"),
        "встроенная функция: запрещённое найдено в плане: {e}"
    );
    // И в тексте DDL индекса: не-индекс, несколько команд, CONCURRENTLY, запрещённые вызовы в выражении индекса.
    for (idx, code) in [
        (
            "CREATE INDEX CONCURRENTLY ON shop.orders (status)",
            "invalid_sql:",
        ),
        ("DROP TABLE shop.orders", "invalid_sql:"),
        (
            "CREATE INDEX ON shop.orders (status); DROP TABLE shop.orders",
            "invalid_sql:",
        ),
        ("DO $$ BEGIN PERFORM 1; END $$", "invalid_sql:"),
        (
            "CREATE INDEX ON shop.orders ((pg_sleep(1)::text))",
            "forbidden_function:",
        ),
        (
            "CREATE INDEX ON shop.orders ((set_config('a.b', '1', false)))",
            "forbidden_function:",
        ),
    ] {
        let e = err(&call(
            c,
            "hypothetical_index",
            hypo_call("SELECT count(*) FROM shop.orders WHERE status = 'x'", idx),
        )
        .await);
        assert!(e.starts_with(code), "hypothetical_index DDL {idx}: {e}");
    }

    // Запись только планируется: под суперпользователем (права записи есть) строки на месте.
    let (_dir_su, st_su) =
        common::open_server_as(&perf, "perf", common::SUPERUSER, common::SUPERUSER_PASSWORD).await;
    let g_su = gateway(st_su).await;
    let v = ok(call(
        &g_su.client,
        "explain_query",
        explain("DELETE FROM shop.orders WHERE id < 100"),
    )
    .await);
    assert!(v["planText"].as_str().unwrap().contains("Delete on"), "{v}");
    let e = err(&call(
        &g_su.client,
        "explain_query",
        explain("SELECT shop.it_mcp_cfg()"),
    )
    .await);
    assert!(
        e.starts_with("forbidden_function:"),
        "суперпользователь не обходит пробу плана: {e}"
    );
    assert_eq!(count().await, rows_before, "ничего не удалено");
    assert_eq!(
        shop_writes(&su).await,
        writes_before,
        "EXPLAIN и гипотезы не записали ни строки в shop.*"
    );
    assert_eq!(
        stats_reset().await,
        reset_before,
        "статистика базы не сброшена"
    );
}
