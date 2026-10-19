//! Открытые подключения: открытие и закрытие, топология, журнал событий, узел каталога (спец. P1 §6–7).

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::conn::errors::{connection_suspect, from_pg};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::topology::journal::TopologyEvent;
use crate::topology::probe::no_answer;
use crate::topology::runtime::{ConnState, RuntimeSink, TopologyView};

/// События наблюдения → окно приложения.
pub struct TauriSink(pub AppHandle);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatePayload<'a> {
    connection_id: &'a str,
    state: ConnState,
    error: Option<&'a AppError>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct TopologyPayload<'a> {
    connection_id: &'a str,
    view: &'a TopologyView,
    new_events: &'a [TopologyEvent],
}

impl RuntimeSink for TauriSink {
    fn state_changed(&self, connection_id: &str, state: ConnState, error: Option<&AppError>) {
        let payload = StatePayload {
            connection_id,
            state,
            error,
        };
        if let Err(e) = self.0.emit("connection://state", payload) {
            tracing::warn!("emit connection://state: {e}");
        }
    }

    fn topology_changed(
        &self,
        connection_id: &str,
        view: &TopologyView,
        new_events: &[TopologyEvent],
    ) {
        let payload = TopologyPayload {
            connection_id,
            view,
            new_events,
        };
        if let Err(e) = self.0.emit("topology://changed", payload) {
            tracing::warn!("emit topology://changed: {e}");
        }
    }
}

#[tauri::command]
pub async fn connect(state: State<'_, AppState>, id: String) -> AppResult<TopologyView> {
    let profile = state.profile(&id)?;
    state.runtimes.connect(profile).await
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>, id: String) -> AppResult<()> {
    state.runtimes.disconnect(&id);
    Ok(())
}

#[tauri::command]
pub fn get_topology(state: State<'_, AppState>, id: String) -> AppResult<TopologyView> {
    Ok(state.runtimes.view(&id))
}

#[tauri::command]
pub fn list_topology_events(
    state: State<'_, AppState>,
    id: String,
) -> AppResult<Vec<TopologyEvent>> {
    Ok(state.runtimes.events(&id))
}

#[tauri::command]
pub fn set_catalog_member(
    state: State<'_, AppState>,
    id: String,
    member: Option<String>,
) -> AppResult<TopologyView> {
    state.runtimes.set_catalog_member(&id, member)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberFacts {
    pub started_at_ms: i64,
    pub connections: i64,
    pub max_connections: i64,
    pub databases: i64,
}

/// Предел на запрос сведений об узле: серверный `statement_timeout` не защищает от молчащего пути.
pub const FACTS_TIMEOUT: Duration = Duration::from_secs(10);

#[tauri::command]
pub async fn get_member_facts(
    state: State<'_, AppState>,
    id: String,
    member: String,
) -> AppResult<MemberFacts> {
    member_facts(&state, &id, &member).await
}

pub async fn member_facts(st: &AppState, id: &str, member: &str) -> AppResult<MemberFacts> {
    member_facts_within(st, id, member, FACTS_TIMEOUT).await
}

/// [`member_facts`] с заданным пределом. Таймаут и обрыв соединения закрывают его, а не возвращают в пул.
pub async fn member_facts_within(
    st: &AppState,
    id: &str,
    member: &str,
    limit: Duration,
) -> AppResult<MemberFacts> {
    let c = st.client(id, Some(member), None).await?;
    let query = c.query_one(
        "SELECT (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
         (SELECT count(*) FROM pg_stat_activity)::bigint, \
         current_setting('max_connections')::bigint, \
         (SELECT count(*) FROM pg_database WHERE NOT datistemplate)::bigint",
        &[],
    );
    match tokio::time::timeout(limit, query).await {
        Ok(Ok(r)) => Ok(MemberFacts {
            started_at_ms: r.get(0),
            connections: r.get(1),
            max_connections: r.get(2),
            databases: r.get(3),
        }),
        Ok(Err(e)) => {
            if connection_suspect(&e) {
                c.discard();
            }
            Err(from_pg(&e))
        }
        Err(_) => {
            c.discard();
            Err(no_answer(limit))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::journal::EventKind;

    #[test]
    fn event_payloads_are_camel_case() {
        let view = TopologyView {
            state: ConnState::Degraded,
            topology: None,
            catalog_member: Some("a".into()),
            catalog_fallback: true,
            error: None,
        };
        let events = [TopologyEvent {
            at_ms: 7,
            event: EventKind::PauseChanged { paused: true },
        }];
        let json = serde_json::to_value(TopologyPayload {
            connection_id: "c",
            view: &view,
            new_events: &events,
        })
        .unwrap();
        assert_eq!(json["connectionId"], "c");
        assert_eq!(json["view"]["state"], "degraded");
        assert_eq!(json["view"]["catalogMember"], "a");
        assert_eq!(json["view"]["catalogFallback"], true);
        assert_eq!(
            json["newEvents"][0],
            serde_json::json!({"atMs": 7, "kind": "pauseChanged", "paused": true})
        );

        let err = crate::error::AppError::internal_with("x");
        let json = serde_json::to_value(StatePayload {
            connection_id: "c",
            state: ConnState::Failed,
            error: Some(&err),
        })
        .unwrap();
        assert_eq!(json["connectionId"], "c");
        assert_eq!(json["state"], "failed");
        assert_eq!(json["error"]["code"], "INTERNAL");
    }

    #[test]
    fn member_facts_json_is_camel_case() {
        let json = serde_json::to_value(MemberFacts {
            started_at_ms: 1,
            connections: 2,
            max_connections: 3,
            databases: 4,
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"startedAtMs": 1, "connections": 2, "maxConnections": 3, "databases": 4})
        );
    }
}
