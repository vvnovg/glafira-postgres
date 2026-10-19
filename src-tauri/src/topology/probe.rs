//! Опрос узлов списка и одиночного сервера (спец. P1 §6.3): роль, версия, репликация.

use std::collections::HashMap;
use std::net::IpAddr;

use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionKind, HostPort};
use crate::error::AppResult;
use crate::topology::connector::NodeConnector;
use crate::topology::model::{
    Member, Role, Topology, TopologyWarning, member_name, system_id_warning,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplicationRow {
    pub application_name: String,
    pub client_addr: Option<String>,
    pub state: Option<String>,
    pub sync_state: Option<String>,
    pub replay_lag_bytes: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeFacts {
    pub in_recovery: bool,
    pub server_version: String,
    pub system_id: Option<String>,
    /// Строки `pg_stat_replication` (только у primary).
    pub replicas: Vec<ReplicationRow>,
    /// `pg_stat_wal_receiver.sender_host:sender_port` (только у реплики).
    pub sender: Option<(String, u16)>,
    pub own_lag_bytes: Option<i64>,
    pub replay_delay_ms: Option<i64>,
}

pub async fn collect(c: &Client) -> AppResult<NodeFacts> {
    let row = c
        .query_one(
            "SELECT pg_is_in_recovery(), current_setting('server_version')",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    let mut f = NodeFacts {
        in_recovery: row.get(0),
        server_version: row.get(1),
        ..Default::default()
    };
    // pg_control_system() может быть недоступна роли — тогда без проверки system_identifier.
    f.system_id = c
        .query_one(
            "SELECT system_identifier::text FROM pg_control_system()",
            &[],
        )
        .await
        .ok()
        .map(|r| r.get(0));
    if f.in_recovery {
        if let Some(r) = c
            .query_opt(
                "SELECT sender_host, sender_port FROM pg_stat_wal_receiver",
                &[],
            )
            .await
            .map_err(|e| from_pg(&e))?
        {
            let host: Option<String> = r.get(0);
            let port: Option<i32> = r.get(1);
            f.sender = host.map(|h| (h, port.and_then(|p| u16::try_from(p).ok()).unwrap_or(5432)));
        }
        let r = c
            .query_one(
                "SELECT pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::bigint, \
                 (extract(epoch FROM now() - pg_last_xact_replay_timestamp()) * 1000)::bigint",
                &[],
            )
            .await
            .map_err(|e| from_pg(&e))?;
        f.own_lag_bytes = r.get(0);
        f.replay_delay_ms = r.get(1);
    } else {
        let rows = c
            .query(
                "SELECT application_name, host(client_addr), state, sync_state, \
                 pg_wal_lsn_diff(pg_current_wal_lsn(), replay_lsn)::bigint FROM pg_stat_replication",
                &[],
            )
            .await
            .map_err(|e| from_pg(&e))?;
        f.replicas = rows
            .iter()
            .map(|r| ReplicationRow {
                application_name: r.get::<_, Option<String>>(0).unwrap_or_default(),
                client_addr: r.get(1),
                state: r.get(2),
                sync_state: r.get(3),
                replay_lag_bytes: r.get(4),
            })
            .collect();
    }
    Ok(f)
}

fn apply_row(m: &mut Member, r: &ReplicationRow) {
    if r.replay_lag_bytes.is_some() {
        m.lag_bytes = r.replay_lag_bytes;
    }
    m.role = match r.sync_state.as_deref() {
        Some("sync") => Role::SyncStandby,
        Some("quorum") => Role::QuorumStandby,
        _ => Role::Replica,
    };
    m.state = r.state.clone();
}

/// Реплики ↔ строки `pg_stat_replication` primary: по `application_name`, по адресу клиента,
/// затем правило «единственной пары» (отступление 5 плана P1a).
fn match_replicas(
    members: &mut [Member],
    rows: &[ReplicationRow],
    resolved: &HashMap<String, Vec<IpAddr>>,
) {
    let mut used = vec![false; rows.len()];
    let mut unmatched = Vec::new();
    let replicas: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].role == Role::Replica)
        .collect();
    for i in replicas {
        let m = &members[i];
        let ips = resolved.get(&m.host);
        let found = rows.iter().enumerate().position(|(j, r)| {
            !used[j]
                && (r.application_name == m.host
                    || r.application_name == m.name
                    || r.client_addr.as_ref().is_some_and(|a| {
                        ips.is_some_and(|ips| ips.iter().any(|ip| ip.to_string() == *a))
                    }))
        });
        match found {
            Some(j) => {
                used[j] = true;
                apply_row(&mut members[i], &rows[j]);
            }
            None => unmatched.push(i),
        }
    }
    let free: Vec<usize> = (0..rows.len()).filter(|&j| !used[j]).collect();
    if let ([i], [j]) = (unmatched.as_slice(), free.as_slice()) {
        apply_row(&mut members[*i], &rows[*j]);
    }
}

pub fn assemble(
    connection_id: &str,
    kind: ConnectionKind,
    hosts: &[HostPort],
    results: Vec<AppResult<NodeFacts>>,
    resolved: &HashMap<String, Vec<IpAddr>>,
    now_ms: i64,
) -> Topology {
    let mut members = Vec::with_capacity(hosts.len());
    let mut facts: Vec<Option<NodeFacts>> = Vec::with_capacity(hosts.len());
    let mut warnings = Vec::new();
    for (h, r) in hosts.iter().zip(results) {
        let mut m = Member::new(member_name(&h.host, h.port), &h.host, h.port);
        match r {
            Ok(f) => {
                m.role = if f.in_recovery {
                    Role::Replica
                } else {
                    Role::Primary
                };
                m.server_version = Some(f.server_version.clone());
                m.system_id = f.system_id.clone();
                m.lag_bytes = f.own_lag_bytes;
                m.replay_delay_ms = f.replay_delay_ms;
                m.upstream = f.sender.as_ref().map(|(sh, sp)| {
                    hosts
                        .iter()
                        .find(|x| &x.host == sh && x.port == *sp)
                        .map(|x| member_name(&x.host, x.port))
                        .unwrap_or_else(|| member_name(sh, *sp))
                });
                facts.push(Some(f));
            }
            Err(e) => {
                m.reachable = false;
                m.error = Some(e);
                warnings.push(TopologyWarning::MemberUnreachable {
                    name: m.name.clone(),
                });
                facts.push(None);
            }
        }
        members.push(m);
    }
    let primaries: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].role == Role::Primary)
        .collect();
    if kind == ConnectionKind::HostList {
        match primaries.len() {
            0 => warnings.insert(0, TopologyWarning::NoPrimary),
            1 => {}
            _ => warnings.insert(
                0,
                TopologyWarning::MultiplePrimaries {
                    members: primaries.iter().map(|&i| members[i].name.clone()).collect(),
                },
            ),
        }
    }
    if let [p] = primaries.as_slice() {
        let rows = facts[*p]
            .as_ref()
            .map(|f| f.replicas.clone())
            .unwrap_or_default();
        match_replicas(&mut members, &rows, resolved);
    }
    if let Some(w) = system_id_warning(&members) {
        warnings.push(w);
    }
    Topology {
        connection_id: connection_id.to_string(),
        kind,
        observed_at_ms: now_ms,
        scope: None,
        paused: None,
        members,
        warnings,
    }
}

async fn resolve(hosts: &[HostPort]) -> HashMap<String, Vec<IpAddr>> {
    let mut out = HashMap::new();
    for h in hosts {
        if let Ok(addrs) = tokio::net::lookup_host((h.host.as_str(), h.port)).await {
            out.insert(h.host.clone(), addrs.map(|a| a.ip()).collect());
        }
    }
    out
}

/// Опрос всех узлов параллельно и сборка топологии.
pub async fn observe_hosts(
    connection_id: &str,
    kind: ConnectionKind,
    hosts: &[HostPort],
    nodes: &dyn NodeConnector,
    now_ms: i64,
) -> Topology {
    let names: Vec<String> = hosts.iter().map(|h| member_name(&h.host, h.port)).collect();
    let results = futures_util::future::join_all(
        hosts
            .iter()
            .zip(&names)
            .map(|(h, name)| nodes.facts(name, &h.host, h.port)),
    )
    .await;
    let resolved = resolve(hosts).await;
    assemble(connection_id, kind, hosts, results, &resolved, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn hp(h: &str, p: u16) -> HostPort {
        HostPort {
            host: h.into(),
            port: p,
        }
    }

    fn primary(rows: Vec<ReplicationRow>) -> AppResult<NodeFacts> {
        Ok(NodeFacts {
            in_recovery: false,
            server_version: "16.4".into(),
            system_id: Some("1".into()),
            replicas: rows,
            ..Default::default()
        })
    }

    fn replica(sender: Option<(&str, u16)>, lag: Option<i64>) -> AppResult<NodeFacts> {
        Ok(NodeFacts {
            in_recovery: true,
            server_version: "16.4".into(),
            system_id: Some("1".into()),
            sender: sender.map(|(h, p)| (h.to_string(), p)),
            own_lag_bytes: lag,
            replay_delay_ms: Some(120),
            ..Default::default()
        })
    }

    fn row(app: &str, addr: Option<&str>, sync: &str, lag: i64) -> ReplicationRow {
        ReplicationRow {
            application_name: app.into(),
            client_addr: addr.map(String::from),
            state: Some("streaming".into()),
            sync_state: Some(sync.into()),
            replay_lag_bytes: Some(lag),
        }
    }

    fn down() -> AppResult<NodeFacts> {
        Err(app_err!(
            ErrorCode::ConnectionFailed,
            "errors.pg.tcpFailed",
            "Could not reach {{host}}:{{port}}",
            host = "x",
            port = 1
        ))
    }

    #[test]
    fn primary_and_replica_matched_by_application_name() {
        let hosts = [hp("db1", 5432), hp("db2", 5432)];
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![
                primary(vec![row("db2", None, "sync", 42)]),
                replica(Some(("db1", 5432)), Some(7)),
            ],
            &HashMap::new(),
            9,
        );
        assert_eq!(t.members[0].role, Role::Primary);
        assert_eq!(t.members[1].role, Role::SyncStandby);
        assert_eq!(
            t.members[1].lag_bytes,
            Some(42),
            "лаг primary точнее собственного"
        );
        assert_eq!(t.members[1].upstream.as_deref(), Some("db1:5432"));
        assert_eq!(t.members[1].replay_delay_ms, Some(120));
        assert_eq!(t.members[1].state.as_deref(), Some("streaming"));
        assert!(t.warnings.is_empty());
        assert_eq!(t.observed_at_ms, 9);
    }

    #[test]
    fn replica_matched_by_client_address() {
        let hosts = [hp("db1", 5432), hp("db2", 5432)];
        let resolved = HashMap::from([("db2".to_string(), vec!["10.0.0.2".parse().unwrap()])]);
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![
                primary(vec![
                    row("walreceiver", Some("10.0.0.9"), "async", 1),
                    row("walreceiver", Some("10.0.0.2"), "quorum", 5),
                ]),
                replica(None, None),
            ],
            &resolved,
            1,
        );
        assert_eq!(t.members[1].role, Role::QuorumStandby);
        assert_eq!(t.members[1].lag_bytes, Some(5));
    }

    #[test]
    fn single_pair_rule() {
        let hosts = [hp("localhost", 5451), hp("localhost", 5452)];
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![
                primary(vec![row("walreceiver", Some("172.18.0.5"), "sync", 3)]),
                replica(Some(("pg-primary", 5432)), Some(1)),
            ],
            &HashMap::new(),
            1,
        );
        assert_eq!(t.members[1].role, Role::SyncStandby);
        assert_eq!(t.members[1].lag_bytes, Some(3));
        assert_eq!(
            t.members[1].upstream.as_deref(),
            Some("pg-primary:5432"),
            "не из списка — как есть"
        );
    }

    #[test]
    fn primary_warnings() {
        let hosts = [hp("a", 1), hp("b", 1)];
        let none = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![replica(None, None), replica(None, None)],
            &HashMap::new(),
            1,
        );
        assert_eq!(none.warnings, vec![TopologyWarning::NoPrimary]);
        let two = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![primary(vec![]), primary(vec![])],
            &HashMap::new(),
            1,
        );
        assert_eq!(
            two.warnings,
            vec![TopologyWarning::MultiplePrimaries {
                members: vec!["a:1".into(), "b:1".into()]
            }]
        );
        let server = assemble(
            "c",
            ConnectionKind::Server,
            &hosts[..1],
            vec![replica(None, None)],
            &HashMap::new(),
            1,
        );
        assert!(
            server.warnings.is_empty(),
            "у сервера нет требований к primary"
        );
    }

    #[test]
    fn unreachable_and_system_id() {
        let hosts = [hp("a", 1), hp("b", 1), hp("c", 1)];
        let mut other = replica(None, None).unwrap();
        other.system_id = Some("2".into());
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![primary(vec![]), down(), Ok(other)],
            &HashMap::new(),
            1,
        );
        let b = t.member("b:1").unwrap();
        assert!(!b.reachable);
        assert_eq!(b.role, Role::Unknown);
        assert_eq!(b.error.as_ref().unwrap().key, "errors.pg.tcpFailed");
        assert!(
            t.warnings
                .contains(&TopologyWarning::MemberUnreachable { name: "b:1".into() })
        );
        assert!(t.warnings.contains(&TopologyWarning::SystemIdMismatch {
            members: vec!["a:1".into(), "c:1".into()]
        }));
        assert!(t.is_degraded());
    }
}
