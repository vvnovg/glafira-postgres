//! Опрос узлов списка и одиночного сервера (спец. P1 §6.3): роль, версия, репликация.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::Duration;

use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::conn::profile::{ConnectionKind, HostPort};
use crate::error::{AppError, AppResult, ErrorCode};
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

/// Предел на все запросы `collect` к одному узлу. Серверный `statement_timeout` (5 с) защищает
/// только от долгого запроса; молчащий узел (оборванный NAT, пропавший туннель) после успешного
/// подключения не отвечает вовсе, и без клиентского таймаута опрос (а с ним `join_all` в
/// `observe_hosts`) завис бы навсегда.
pub const COLLECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Предел на одно разрешение имени хоста: при недоступном DNS адреса просто не определяются.
const DNS_TIMEOUT: Duration = Duration::from_secs(3);

/// Версия сервера без суффикса дистрибутива: «16.15 (Debian 16.15-1.pgdg13+2)» → «16.15».
fn short_version(v: &str) -> String {
    v.split_whitespace().next().unwrap_or_default().to_string()
}

/// Опрашивает узел через уже открытое соединение, не дольше [`COLLECT_TIMEOUT`].
///
/// По таймауту запросы отбрасываются на полуслове, поэтому соединение нужно считать негодным
/// и закрыть.
pub async fn collect(c: &Client) -> AppResult<NodeFacts> {
    collect_with_timeout(c, COLLECT_TIMEOUT).await
}

/// [`collect`] с заданным пределом (для тестов молчащего узла).
pub async fn collect_with_timeout(c: &Client, limit: Duration) -> AppResult<NodeFacts> {
    match tokio::time::timeout(limit, collect_queries(c)).await {
        Ok(r) => r,
        Err(_) => Err(no_answer(limit)),
    }
}

/// Узел принял соединение, но не ответил за `limit`.
pub fn no_answer(limit: Duration) -> AppError {
    app_err!(
        ErrorCode::ConnectionFailed,
        "errors.pg.noAnswer",
        "The node did not answer within {{secs}} s",
        // Округление вверх: предел меньше секунды не должен показываться как «0 с».
        secs = limit.as_secs() + u64::from(limit.subsec_nanos() > 0)
    )
}

async fn collect_queries(c: &Client) -> AppResult<NodeFacts> {
    let row = c
        .query_one(
            "SELECT pg_is_in_recovery(), current_setting('server_version')",
            &[],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    let mut f = NodeFacts {
        in_recovery: row.get(0),
        server_version: short_version(&row.get::<_, String>(1)),
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
                // Задержка replay: у догнавшей реплики 0 (иначе на простаивающей она росла бы без
                // предела), иначе возраст последней применённой транзакции, не меньше 0 (расхождение
                // часов). GREATEST игнорирует NULL, поэтому «нет метки времени» отделено явно.
                "SELECT pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::bigint, \
                 (CASE WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() THEN 0 \
                       WHEN pg_last_xact_replay_timestamp() IS NULL THEN NULL \
                       ELSE GREATEST(0, extract(epoch FROM now() - pg_last_xact_replay_timestamp()) * 1000) \
                  END)::bigint",
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
    // Правило «единственной пары» — только для действующей потоковой репликации: строка в
    // `startup`/`catchup`/`backup` (например, pg_basebackup) не доказывает, что это та реплика.
    let free: Vec<usize> = (0..rows.len()).filter(|&j| !used[j]).collect();
    if let ([i], [j]) = (unmatched.as_slice(), free.as_slice())
        && rows[*j].state.as_deref() == Some("streaming")
    {
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

/// Адреса хостов для сопоставления реплик по `client_addr`. Хосты разрешаются параллельно, у
/// каждого разрешения свой предел [`DNS_TIMEOUT`]; не разрешился в срок — адресов нет.
async fn resolve(hosts: &[HostPort]) -> HashMap<String, Vec<IpAddr>> {
    let mut seen = HashSet::new();
    let lookups = hosts
        .iter()
        .filter(|h| seen.insert(h.host.as_str()))
        .map(|h| async move {
            let addrs = tokio::time::timeout(
                DNS_TIMEOUT,
                tokio::net::lookup_host((h.host.as_str(), h.port)),
            )
            .await
            .ok()?
            .ok()?;
            Some((h.host.clone(), addrs.map(|a| a.ip()).collect::<Vec<_>>()))
        });
    futures_util::future::join_all(lookups)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// Опрос всех узлов параллельно и сборка топологии. Имена хостов разрешаются одновременно с
/// опросом, а у одиночного сервера — не разрешаются вовсе: сопоставлять реплики некому.
pub async fn observe_hosts(
    connection_id: &str,
    kind: ConnectionKind,
    hosts: &[HostPort],
    nodes: &dyn NodeConnector,
    now_ms: i64,
) -> Topology {
    let names: Vec<String> = hosts.iter().map(|h| member_name(&h.host, h.port)).collect();
    let probes = futures_util::future::join_all(
        hosts
            .iter()
            .zip(&names)
            .map(|(h, name)| nodes.facts(name, &h.host, h.port)),
    );
    let resolution = async {
        if kind == ConnectionKind::Server {
            HashMap::new()
        } else {
            resolve(hosts).await
        }
    };
    let (results, resolved) = tokio::join!(probes, resolution);
    assemble(connection_id, kind, hosts, results, &resolved, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::BoxFuture;

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

    /// Реплика осталась нераспознанной: роль и лаг — собственные, строка primary не применена.
    fn assert_not_paired(m: &Member, own_lag: i64) {
        assert_eq!(m.role, Role::Replica);
        assert_eq!(m.lag_bytes, Some(own_lag), "лаг свой, не из строки primary");
        assert_eq!(m.state, None);
    }

    #[test]
    fn single_pair_rule_needs_exactly_one_free_row() {
        let hosts = [hp("localhost", 5451), hp("localhost", 5452)];
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![
                primary(vec![
                    row("walreceiver", Some("172.18.0.5"), "sync", 3),
                    row("walreceiver", Some("172.18.0.6"), "async", 4),
                ]),
                replica(None, Some(1)),
            ],
            &HashMap::new(),
            1,
        );
        assert_not_paired(&t.members[1], 1);
    }

    #[test]
    fn single_pair_rule_needs_exactly_one_unmatched_replica() {
        let hosts = [hp("a", 1), hp("b", 1), hp("c", 1)];
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![
                primary(vec![row("walreceiver", Some("172.18.0.5"), "sync", 3)]),
                replica(None, Some(1)),
                replica(None, Some(2)),
            ],
            &HashMap::new(),
            1,
        );
        assert_not_paired(&t.members[1], 1);
        assert_not_paired(&t.members[2], 2);
    }

    #[test]
    fn single_pair_rule_ignores_non_streaming_row() {
        let hosts = [hp("localhost", 5451), hp("localhost", 5452)];
        let catchup = ReplicationRow {
            state: Some("catchup".into()),
            ..row("walreceiver", Some("172.18.0.5"), "sync", 3)
        };
        let t = assemble(
            "c",
            ConnectionKind::HostList,
            &hosts,
            vec![primary(vec![catchup]), replica(None, Some(1))],
            &HashMap::new(),
            1,
        );
        assert_not_paired(&t.members[1], 1);
    }

    #[test]
    fn server_version_loses_distribution_suffix() {
        assert_eq!(short_version("16.15 (Debian 16.15-1.pgdg13+2)"), "16.15");
        assert_eq!(short_version("17.2"), "17.2");
        assert_eq!(short_version("17beta1 (Ubuntu 17~beta1-1)"), "17beta1");
        assert_eq!(short_version(""), "");
    }

    /// Фиктивные узлы: порт 1 — primary с задержкой, 2 — реплика, остальные недоступны.
    struct FakeNodes;

    impl NodeConnector for FakeNodes {
        fn facts<'a>(
            &'a self,
            _member: &'a str,
            _host: &'a str,
            port: u16,
        ) -> BoxFuture<'a, AppResult<NodeFacts>> {
            Box::pin(async move {
                match port {
                    1 => {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                        primary(vec![row("127.0.0.1", None, "sync", 8)])
                    }
                    2 => replica(Some(("127.0.0.1", 1)), Some(2)),
                    _ => down(),
                }
            })
        }
    }

    #[tokio::test]
    async fn observe_hosts_keeps_host_order_and_marks_failed_node_unreachable() {
        // Самый медленный узел — первый в списке: порядок членов задаёт список, не скорость ответа.
        let hosts = [hp("127.0.0.1", 1), hp("127.0.0.1", 2), hp("127.0.0.1", 3)];
        let t = observe_hosts("c", ConnectionKind::HostList, &hosts, &FakeNodes, 5).await;
        let names: Vec<&str> = t.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["127.0.0.1:1", "127.0.0.1:2", "127.0.0.1:3"]);
        assert_eq!(t.members[0].role, Role::Primary);
        assert_eq!(
            t.members[1].role,
            Role::SyncStandby,
            "сопоставлена по хосту"
        );
        assert_eq!(t.members[1].upstream.as_deref(), Some("127.0.0.1:1"));
        let failed = &t.members[2];
        assert!(!failed.reachable);
        assert_eq!(failed.error.as_ref().unwrap().key, "errors.pg.tcpFailed");
        assert!(t.warnings.contains(&TopologyWarning::MemberUnreachable {
            name: "127.0.0.1:3".into()
        }));
        assert_eq!(t.observed_at_ms, 5);
    }

    #[tokio::test]
    async fn resolve_maps_host_to_addresses() {
        let hosts = [hp("127.0.0.1", 1), hp("127.0.0.1", 2), hp("::1", 3)];
        let r = resolve(&hosts).await;
        assert_eq!(r.len(), 2, "{r:?}");
        assert_eq!(r["127.0.0.1"], vec!["127.0.0.1".parse::<IpAddr>().unwrap()]);
    }
}
