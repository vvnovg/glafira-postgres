//! Сервер шлюза (перенос `start`, `serve_stream`, `McpServerHandle`, `GatewayOptions`, `SessionSnapshot` из
//! `server/mod.rs` Glafira for Kafka): приём соединений шима, рукопожатие, сессии. Сервер rmcp одной сессии строит
//! потребитель через [`SessionFactory`]: маршрутизатор инструментов rmcp привязан к типу сервера, поэтому
//! инструменты и `ServerHandler` объявляет крейт приложения (`glafira-pg-mcp`), а ядро даёт ему [`SessionCtx`].

use std::io;
use std::sync::Arc;
use std::time::Duration;

use rmcp::{ServerHandler, ServiceExt};
use serde::Serialize;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditEvent, AuditLog};
use crate::call::SessionCtx;
use crate::endpoint::{BoxStream, Endpoint, Listener};
use crate::gate::Gate;
use crate::session::{SessionEntry, Shared, next_session_id};

#[derive(Debug, Clone)]
pub struct GatewayOptions {
    /// Имя сервера MCP (`serverInfo.name`), например `glafira-pg`.
    pub name: String,
    pub version: String,
    /// `instructions` сервера для модели (по-английски).
    pub instructions: String,
}

impl GatewayOptions {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            instructions: instructions.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub id: String,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub started_ms: u64,
    pub calls: u64,
    /// Идёт вызов (подсветка индикатора).
    pub active: bool,
}

/// Строит сервер rmcp одной сессии. Сервер проводит каждый вызов через `SessionCtx::call`, в `initialize` зовёт
/// `identify` и `request_approval`, а `tools/list` отвечает `SessionCtx::list_tools_result`.
pub trait SessionFactory: Send + Sync + 'static {
    type Server: ServerHandler + Send + Sync + 'static;
    fn create(&self, ctx: SessionCtx) -> Self::Server;
}

pub struct McpServerHandle {
    shared: Arc<Shared>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}

impl McpServerHandle {
    pub fn sessions(&self) -> Vec<SessionSnapshot> {
        self.shared.snapshot()
    }

    pub fn disconnect(&self, id: &str) -> bool {
        self.shared.cancel_where(|sid, _| sid == id) > 0
    }

    pub fn disconnect_client(&self, name: &str) -> usize {
        self.shared
            .cancel_where(|_, e| e.client.as_ref().is_some_and(|c| c.name == name))
    }

    pub fn disconnect_all(&self) {
        self.shared.cancel_where(|_, _| true);
    }

    /// Закрывает все сессии и перестаёт слушать; файл сокета удаляется.
    pub async fn shutdown(self) {
        self.stop.cancel();
        let _ = self.task.await;
    }
}

/// Начинает слушать `endpoint`. Вызывать внутри runtime tokio.
pub async fn start<F: SessionFactory>(
    endpoint: &Endpoint,
    token: String,
    gate: Arc<dyn Gate>,
    audit: Arc<AuditLog>,
    options: GatewayOptions,
    factory: Arc<F>,
) -> io::Result<McpServerHandle> {
    let mut listener = Listener::bind(endpoint).await?;
    audit.record(AuditEvent::ServerStarted {
        endpoint: endpoint.to_string(),
    });
    let shared = Shared::new(gate, audit, options);
    let stop = CancellationToken::new();
    let task = {
        let (shared, stop) = (shared.clone(), stop.clone());
        tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    r = listener.accept() => match r {
                        Ok(stream) => {
                            conns.spawn(handle_conn(
                                shared.clone(),
                                factory.clone(),
                                stream,
                                token.clone(),
                                stop.child_token(),
                            ));
                            // Законченные соединения не копятся в JoinSet до остановки.
                            while conns.try_join_next().is_some() {}
                        }
                        Err(e) => {
                            tracing::warn!("mcp accept failed: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    },
                }
            }
            drop(listener);
            while conns.join_next().await.is_some() {}
            shared.audit.record(AuditEvent::ServerStopped);
        })
    };
    Ok(McpServerHandle { shared, stop, task })
}

async fn handle_conn<F: SessionFactory>(
    shared: Arc<Shared>,
    factory: Arc<F>,
    mut stream: BoxStream,
    token: String,
    ct: CancellationToken,
) {
    // Остановка сервера не ждёт молчащее рукопожатие (до 5 с).
    let handshake = tokio::select! {
        _ = ct.cancelled() => return,
        r = crate::handshake::server(&mut stream, &token) => r,
    };
    if let Err(e) = handshake {
        tracing::debug!("mcp handshake failed: {e}");
        return;
    }
    serve_session(shared, factory, stream, ct).await;
}

/// Одна сессия MCP поверх готового потока, без рукопожатия и без сокета (интеграционные тесты приложения).
/// Возвращается, когда клиент закрыл поток.
pub async fn serve_stream<F: SessionFactory>(
    stream: BoxStream,
    gate: Arc<dyn Gate>,
    audit: Arc<AuditLog>,
    options: GatewayOptions,
    factory: Arc<F>,
) {
    let shared = Shared::new(gate, audit, options);
    serve_session(shared, factory, stream, CancellationToken::new()).await;
}

async fn serve_session<F: SessionFactory>(
    shared: Arc<Shared>,
    factory: Arc<F>,
    stream: BoxStream,
    ct: CancellationToken,
) {
    let id = next_session_id();
    shared
        .sessions()
        .insert(id.clone(), SessionEntry::new(ct.clone()));
    shared.audit.record(AuditEvent::SessionStarted {
        session: id.clone(),
    });
    shared.gate.sessions_changed();
    let server = factory.create(SessionCtx::new(shared.clone(), id.clone()));
    match server.serve_with_ct(stream, ct).await {
        Ok(running) => {
            let _ = running.waiting().await;
        }
        Err(e) => tracing::debug!("mcp session {id} failed to start: {e}"),
    }
    let entry = shared.sessions().remove(&id);
    shared.audit.record(AuditEvent::SessionEnded {
        session: id.clone(),
        client: entry
            .as_ref()
            .and_then(|e| e.client.as_ref())
            .map(|c| c.name.clone()),
        calls: entry.map_or(0, |e| e.calls),
    });
    shared.gate.session_closed(&id);
    shared.gate.sessions_changed();
}
