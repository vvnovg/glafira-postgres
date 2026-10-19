//! Один опрос топологии подключения и выбор узла каталога по умолчанию (спец. P1 §6.3, §7).

use crate::conn::profile::{ConnectionKind, ConnectionProfile, TargetServerType};
use crate::error::{AppResult, ErrorCode};
use crate::topology::connector::NodeConnector;
use crate::topology::model::{Member, Role, Topology};
use crate::topology::patroni::{PatroniHttp, discover};
use crate::topology::probe::observe_hosts;

pub async fn observe(
    profile: &ConnectionProfile,
    http: Option<&dyn PatroniHttp>,
    nodes: &dyn NodeConnector,
    previous: Option<&Topology>,
    now_ms: i64,
) -> AppResult<Topology> {
    match profile.kind {
        ConnectionKind::Patroni => {
            let (Some(settings), Some(http)) = (profile.patroni.as_ref(), http) else {
                return Err(app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.noUrls",
                    "No Patroni API addresses"
                ));
            };
            discover(http, settings, &profile.id, previous, now_ms).await
        }
        ConnectionKind::HostList | ConnectionKind::Server => {
            Ok(observe_hosts(&profile.id, profile.kind, &profile.hosts, nodes, now_ms).await)
        }
    }
}

/// Узел, с которого читается каталог, пока пользователь не выбрал другой.
pub fn default_member(profile: &ConnectionProfile, t: &Topology) -> Option<String> {
    let reachable = |m: &&Member| m.reachable;
    let pick = match profile.kind {
        // Недоступный сервер узлом каталога не показывается: дерево не должно предлагать «Базы данных» мёртвого узла.
        ConnectionKind::Server => t.members.first().filter(reachable),
        ConnectionKind::Patroni => t
            .leader()
            .filter(reachable)
            .or_else(|| t.members.iter().find(reachable)),
        ConnectionKind::HostList => {
            let primary = t
                .members
                .iter()
                .find(|m| m.reachable && m.role == Role::Primary);
            let standby = t
                .members
                .iter()
                .find(|m| m.reachable && m.role.is_standby());
            match profile.target_server_type {
                TargetServerType::Primary => primary,
                TargetServerType::Standby => standby,
                TargetServerType::PreferStandby => standby.or(primary),
                TargetServerType::Any => t.members.iter().find(reachable),
            }
        }
    };
    pick.map(|m| m.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{ConnectionKind, TargetServerType, sample_profile};
    use crate::topology::model::{Role, member, topo};

    fn host_list(target: TargetServerType) -> ConnectionProfile {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::HostList;
        p.target_server_type = target;
        p
    }

    #[test]
    fn host_list_targets() {
        let t = topo(vec![
            member("p", Role::Primary),
            member("r1", Role::Replica),
            member("r2", Role::SyncStandby),
        ]);
        assert_eq!(
            default_member(&host_list(TargetServerType::Primary), &t).as_deref(),
            Some("p")
        );
        assert_eq!(
            default_member(&host_list(TargetServerType::Standby), &t).as_deref(),
            Some("r1")
        );
        assert_eq!(
            default_member(&host_list(TargetServerType::PreferStandby), &t).as_deref(),
            Some("r1")
        );
        assert_eq!(
            default_member(&host_list(TargetServerType::Any), &t).as_deref(),
            Some("p")
        );

        let mut only_primary = topo(vec![member("p", Role::Primary), member("r", Role::Replica)]);
        only_primary.members[1].reachable = false;
        assert_eq!(
            default_member(&host_list(TargetServerType::Standby), &only_primary),
            None
        );
        assert_eq!(
            default_member(&host_list(TargetServerType::PreferStandby), &only_primary).as_deref(),
            Some("p")
        );
    }

    #[test]
    fn patroni_and_server() {
        let mut p = sample_profile("c");
        p.kind = ConnectionKind::Patroni;
        let mut t = topo(vec![member("a", Role::Replica), member("b", Role::Primary)]);
        assert_eq!(default_member(&p, &t).as_deref(), Some("b"));
        t.members[1].reachable = false;
        assert_eq!(
            default_member(&p, &t).as_deref(),
            Some("a"),
            "лидер недоступен — любой доступный"
        );
        let s = sample_profile("s");
        let mut one = topo(vec![member("localhost:5432", Role::Replica)]);
        assert_eq!(default_member(&s, &one).as_deref(), Some("localhost:5432"));
        one.members[0].reachable = false;
        assert_eq!(
            default_member(&s, &one),
            None,
            "недоступный сервер — не узел каталога"
        );
    }
}
