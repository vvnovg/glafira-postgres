//! Топология подключения (спец. P1 §6.1): участники, роли, лаг, предупреждения.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::conn::profile::ConnectionKind;
use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Primary,
    StandbyLeader,
    SyncStandby,
    QuorumStandby,
    Replica,
    Unknown,
}

impl Role {
    pub fn is_standby(self) -> bool {
        matches!(
            self,
            Role::SyncStandby | Role::QuorumStandby | Role::Replica
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub name: String,
    pub host: String,
    pub port: u16,
    /// Базовый адрес REST API Patroni (`http://h:8008`).
    pub api_url: Option<String>,
    pub role: Role,
    /// Роль и состояние как их прислал Patroni.
    pub raw_role: Option<String>,
    pub state: Option<String>,
    pub timeline: Option<i64>,
    pub lag_bytes: Option<i64>,
    pub replay_delay_ms: Option<i64>,
    /// Список узлов: от кого реплика получает WAL.
    pub upstream: Option<String>,
    pub tags: serde_json::Map<String, serde_json::Value>,
    pub pending_restart: bool,
    pub server_version: Option<String>,
    pub patroni_version: Option<String>,
    pub dcs_last_seen_ms: Option<i64>,
    /// Нужен только для предупреждения `systemIdMismatch`.
    #[serde(skip)]
    pub system_id: Option<String>,
    pub reachable: bool,
    pub error: Option<AppError>,
}

impl Member {
    pub fn new(name: impl Into<String>, host: impl Into<String>, port: u16) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            port,
            api_url: None,
            role: Role::Unknown,
            raw_role: None,
            state: None,
            timeline: None,
            lag_bytes: None,
            replay_delay_ms: None,
            upstream: None,
            tags: Default::default(),
            pending_restart: false,
            server_version: None,
            patroni_version: None,
            dcs_last_seen_ms: None,
            system_id: None,
            reachable: true,
            error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TopologyWarning {
    NoPrimary,
    MultiplePrimaries { members: Vec<String> },
    SystemIdMismatch { members: Vec<String> },
    PatroniUnreachable,
    MemberUnreachable { name: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Topology {
    pub connection_id: String,
    pub kind: ConnectionKind,
    pub observed_at_ms: i64,
    pub scope: Option<String>,
    pub paused: Option<bool>,
    pub members: Vec<Member>,
    pub warnings: Vec<TopologyWarning>,
}

impl Topology {
    /// Узел, принимающий запись: primary, иначе лидер standby-кластера.
    pub fn leader(&self) -> Option<&Member> {
        self.members
            .iter()
            .find(|m| m.role == Role::Primary)
            .or_else(|| self.members.iter().find(|m| m.role == Role::StandbyLeader))
    }

    pub fn member(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.name == name)
    }

    pub fn is_degraded(&self) -> bool {
        self.warnings.iter().any(|w| {
            matches!(
                w,
                TopologyWarning::PatroniUnreachable | TopologyWarning::MemberUnreachable { .. }
            )
        })
    }

    pub fn same_except_time(&self, other: &Topology) -> bool {
        self.members == other.members
            && self.warnings == other.warnings
            && self.paused == other.paused
            && self.scope == other.scope
    }
}

/// Имя участника сервера и списка узлов — `host:port` (IPv6 — в скобках).
pub fn member_name(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Участники принадлежат разным кластерам: у них разные `system_identifier`.
pub fn system_id_warning(members: &[Member]) -> Option<TopologyWarning> {
    let ids: BTreeSet<&str> = members
        .iter()
        .filter_map(|m| m.system_id.as_deref())
        .collect();
    (ids.len() > 1).then(|| TopologyWarning::SystemIdMismatch {
        members: members
            .iter()
            .filter(|m| m.system_id.is_some())
            .map(|m| m.name.clone())
            .collect(),
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) fn topo(members: Vec<Member>) -> Topology {
    Topology {
        connection_id: "c".into(),
        kind: ConnectionKind::Patroni,
        observed_at_ms: 1_000,
        scope: Some("demo".into()),
        paused: Some(false),
        members,
        warnings: vec![],
    }
}

#[cfg(test)]
pub(crate) fn member(name: &str, role: Role) -> Member {
    let mut m = Member::new(name, name, 5432);
    m.role = role;
    m.state = Some(
        if role == Role::Primary {
            "running"
        } else {
            "streaming"
        }
        .into(),
    );
    m.timeline = Some(1);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leader_prefers_primary_then_standby_leader() {
        let t = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        assert_eq!(t.leader().unwrap().name, "b");
        let s = topo(vec![
            member("a", Role::StandbyLeader),
            member("b", Role::Replica),
        ]);
        assert_eq!(s.leader().unwrap().name, "a");
        assert!(topo(vec![member("a", Role::Replica)]).leader().is_none());
    }

    #[test]
    fn member_names_and_system_ids() {
        assert_eq!(member_name("db1", 5432), "db1:5432");
        assert_eq!(member_name("::1", 5433), "[::1]:5433");
        let mut a = member("a", Role::Primary);
        let mut b = member("b", Role::Replica);
        assert!(
            system_id_warning(&[a.clone(), b.clone()]).is_none(),
            "нет данных — нет предупреждения"
        );
        a.system_id = Some("1".into());
        b.system_id = Some("1".into());
        assert!(system_id_warning(&[a.clone(), b.clone()]).is_none());
        b.system_id = Some("2".into());
        assert_eq!(
            system_id_warning(&[a, b]),
            Some(TopologyWarning::SystemIdMismatch {
                members: vec!["a".into(), "b".into()]
            })
        );
    }

    #[test]
    fn json_shape() {
        let mut t = topo(vec![member("a", Role::SyncStandby)]);
        t.members[0].system_id = Some("x".into());
        t.warnings = vec![TopologyWarning::MemberUnreachable { name: "a".into() }];
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["connectionId"], "c");
        assert_eq!(v["members"][0]["role"], "syncStandby");
        assert!(v["members"][0].get("systemId").is_none());
        assert_eq!(
            v["warnings"][0],
            serde_json::json!({"kind": "memberUnreachable", "name": "a"})
        );
        assert!(t.is_degraded());
    }

    #[test]
    fn same_except_time_ignores_observation_moment() {
        let a = topo(vec![member("a", Role::Primary)]);
        let mut b = a.clone();
        b.observed_at_ms = 99;
        assert!(a.same_except_time(&b));
        b.members[0].lag_bytes = Some(1);
        assert!(!a.same_except_time(&b));
    }
}
