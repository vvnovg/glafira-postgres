//! Соединение для действия (спец. P2 §8.2): отдельное, вне пулов, на одно действие или пакет.

use tokio::task::AbortHandle;
use tokio_postgres::Client;

use crate::conn::pg::ConnectSpec;
use crate::conn::pool::{PoolKey, PoolPurpose};
use crate::conn::run_blocking;
use crate::error::{AppResult, ErrorCode};
use crate::state::AppState;

/// Соединение действия: закрывается при `Drop` (задача соединения прерывается — без ожидания Terminate).
pub struct ActionClient {
    pub client: Client,
    abort: AbortHandle,
}

impl Drop for ActionClient {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// Открывает соединение с участником `member` (БД `database` или БД профиля) через туннель подключения.
/// Пул `Catalog` узла запомнил отказ во входе — возвращается он, без новой попытки входа.
pub async fn open(
    st: &AppState,
    id: &str,
    member: &str,
    database: Option<&str>,
) -> AppResult<ActionClient> {
    let (m, profile) = st.runtimes.target(id, Some(member))?;
    if !m.reachable {
        return Err(app_err!(
            ErrorCode::NodeUnavailable,
            "errors.actions.nodeUnavailable",
            "Node {{member}} is unavailable",
            member = m.name
        ));
    }
    let db = database
        .map(str::to_string)
        .unwrap_or_else(|| profile.pg.database.clone());
    let catalog = PoolKey {
        connection_id: profile.id.clone(),
        member: m.name.clone(),
        database: profile.pg.database.clone(),
        purpose: PoolPurpose::Catalog,
    };
    if let Some(e) = st.pools.get(&catalog).and_then(|p| p.auth_failure()) {
        return Err(e);
    }
    let socks = st.tunnels.route_for(&profile)?;
    let secrets = st.secrets.clone();
    let (host, port) = (m.host.clone(), m.port);
    let spec = run_blocking(move || {
        ConnectSpec::build_via(&profile, &host, port, &db, secrets.as_ref(), socks)
    })
    .await?
    .for_action();
    let (client, abort) = spec.connect_with_handle().await?;
    Ok(ActionClient { client, abort })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::conn::pg::ConnectSpec;
    use crate::conn::pool::{PoolKey, PoolPurpose};
    use crate::conn::profile::sample_profile;
    use crate::conn::testutil;
    use crate::error::AppError;
    use crate::secrets::MemoryStore;
    use crate::topology::BoxFuture;
    use crate::topology::model::{Role, Topology, member, topo};
    use crate::topology::runtime::{Runtimes, TopologySource};

    /// Источник топологии, отдающий одну и ту же топологию.
    struct Fixed(Topology);

    impl TopologySource for Fixed {
        fn observe<'a>(
            &'a self,
            _profile: &'a crate::conn::profile::ConnectionProfile,
            _previous: Option<&'a Topology>,
        ) -> BoxFuture<'a, AppResult<Topology>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    /// Состояние с открытым подключением `a`: участник `up` доступен (адрес `127.0.0.1:up_port`), `down` — нет.
    async fn state_with(up_port: u16) -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let mut st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let mut up = member("up", Role::Primary);
        up.host = "127.0.0.1".into();
        up.port = up_port;
        let mut down = member("down", Role::Replica);
        down.reachable = false;
        st.runtimes = Runtimes::new(
            Arc::new(Fixed(topo(vec![up, down]))),
            st.pools.clone(),
            st.tunnels.clone(),
        );
        let mut p = sample_profile("a");
        p.pg.ssl_mode = crate::conn::profile::SslMode::Disable;
        st.runtimes.connect(p).await.unwrap();
        (dir, st)
    }

    async fn failure(st: &AppState, id: &str, member: &str) -> AppError {
        match open(st, id, member, None).await {
            Ok(_) => panic!("соединение не должно открываться"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn closed_connection_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let e = failure(&st, "a", "up").await;
        assert_eq!(e.code, ErrorCode::NodeUnavailable);
        assert_eq!(e.key, "errors.topology.notConnected");
    }

    #[tokio::test]
    async fn unknown_member_is_rejected() {
        let (_dir, st) = state_with(1).await;
        let e = failure(&st, "a", "no-such-node").await;
        assert_eq!(e.code, ErrorCode::InvalidInput);
        assert_eq!(e.key, "errors.topology.unknownMember");
    }

    #[tokio::test]
    async fn unreachable_member_is_node_unavailable_without_connecting() {
        let server = testutil::auth_failing_pg_server();
        let (_dir, st) = state_with(server.port).await;
        let e = failure(&st, "a", "down").await;
        assert_eq!(e.code, ErrorCode::NodeUnavailable);
        assert_eq!(e.key, "errors.actions.nodeUnavailable");
        assert_eq!(e.message, "Node down is unavailable");
        assert_eq!(server.accepted(), 0);
    }

    #[tokio::test]
    async fn remembered_login_refusal_is_returned_without_a_new_login() {
        let server = testutil::auth_failing_pg_server();
        let (_dir, st) = state_with(server.port).await;
        let profile = st.runtimes.target("a", Some("up")).unwrap().1;
        let spec = ConnectSpec::build(
            &profile,
            "127.0.0.1",
            server.port,
            &profile.pg.database,
            &MemoryStore::default(),
        )
        .unwrap();
        let key = PoolKey {
            connection_id: "a".into(),
            member: "up".into(),
            database: profile.pg.database.clone(),
            purpose: PoolPurpose::Catalog,
        };
        let remembered = app_err!(
            ErrorCode::AuthFailed,
            "errors.pg.loginNotRetried",
            "Login was refused earlier"
        );
        st.pools
            .insert_if_absent(key, spec)
            .remember_auth_failure(remembered.clone());
        let e = failure(&st, "a", "up").await;
        assert_eq!(e, remembered);
        assert_eq!(server.accepted(), 0, "нового входа быть не должно");
    }
}
