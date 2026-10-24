//! Готовность к failover (спец. P2 §5.8): кто из участников может стать лидером и почему нет. Динамическая
//! конфигурация Patroni (`GET /config`) и факты участников приходят снаружи; здесь только вердикты.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, Severity};

/// `maximum_lag_on_failover` по умолчанию в Patroni — 1 МиБ.
pub const DEFAULT_MAXIMUM_LAG_ON_FAILOVER: i64 = 1_048_576;

/// Режим синхронной репликации Patroni (`synchronous_mode`: `false` / `true` / `"quorum"`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncMode {
    #[default]
    Off,
    On,
    Quorum,
}

/// Динамическая конфигурация кластера (`GET /config`): то, что влияет на выбор нового лидера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynConfig {
    pub maximum_lag_on_failover: i64,
    pub synchronous_mode: SyncMode,
    pub synchronous_mode_strict: bool,
    pub synchronous_node_count: Option<i64>,
    pub ttl: Option<i64>,
    pub loop_wait: Option<i64>,
    pub retry_timeout: Option<i64>,
    /// Имена постоянных слотов (`slots` в динамической конфигурации).
    pub permanent_slots: Vec<String>,
}

impl Default for DynConfig {
    /// Значения Patroni без настроек: порог лага — 1 МиБ, синхронность выключена.
    fn default() -> Self {
        Self {
            maximum_lag_on_failover: DEFAULT_MAXIMUM_LAG_ON_FAILOVER,
            synchronous_mode: SyncMode::Off,
            synchronous_mode_strict: false,
            synchronous_node_count: None,
            ttl: None,
            loop_wait: None,
            retry_timeout: None,
            permanent_slots: Vec::new(),
        }
    }
}

/// Роль участника для вердикта (из роли топологии).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateRole {
    Leader,
    Sync,
    Quorum,
    Replica,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateFacts {
    pub name: String,
    pub role: CandidateRole,
    /// Состояние, как его прислал Patroni (`streaming`, `running`, `stopped`…).
    pub state: Option<String>,
    pub reachable: bool,
    pub lag_bytes: Option<i64>,
    /// Тег `nofailover`.
    pub nofailover: bool,
    /// Тег `failover_priority` (Patroni 4+).
    pub failover_priority: Option<i64>,
    pub pending_restart: bool,
}

/// Почему участник не годится в лидеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    Unreachable,
    NotStreaming,
    LagTooHigh,
    LagUnknown,
    NoFailoverTag,
    ZeroPriority,
    NotSynchronous,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub member: String,
    pub ready: bool,
    pub reasons: Vec<Reason>,
    pub lag_bytes: Option<i64>,
    pub pending_restart: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverData {
    pub paused: bool,
    pub config: DynConfig,
    pub candidates: Vec<Candidate>,
    pub ready_count: usize,
}

pub fn evaluate(
    members: &[CandidateFacts],
    config: &DynConfig,
    paused: bool,
) -> (FailoverData, Vec<Finding>) {
    let mut candidates = Vec::new();
    for m in members.iter().filter(|m| m.role != CandidateRole::Leader) {
        let mut reasons = Vec::new();
        if !m.reachable {
            reasons.push(Reason::Unreachable);
        } else {
            if !matches!(m.state.as_deref(), Some("streaming" | "running")) {
                reasons.push(Reason::NotStreaming);
            }
            match m.lag_bytes {
                Some(l) if l > config.maximum_lag_on_failover => reasons.push(Reason::LagTooHigh),
                Some(_) => {}
                None => reasons.push(Reason::LagUnknown),
            }
        }
        if m.nofailover {
            reasons.push(Reason::NoFailoverTag);
        }
        if m.failover_priority == Some(0) {
            reasons.push(Reason::ZeroPriority);
        }
        let sync_ok = match config.synchronous_mode {
            SyncMode::Off => true,
            SyncMode::On => m.role == CandidateRole::Sync,
            SyncMode::Quorum => matches!(m.role, CandidateRole::Quorum | CandidateRole::Sync),
        };
        if !sync_ok {
            reasons.push(Reason::NotSynchronous);
        }
        candidates.push(Candidate {
            member: m.name.clone(),
            ready: reasons.is_empty(),
            reasons,
            lag_bytes: m.lag_bytes,
            pending_restart: m.pending_restart,
        });
    }
    let ready_count = candidates.iter().filter(|c| c.ready).count();
    let mut f = Vec::new();
    if ready_count == 0 && !candidates.is_empty() {
        f.push(
            Finding::new("failoverCandidates", Severity::Critical)
                .param("count", 0)
                .link_tool("failover", None),
        );
    } else if ready_count == 1 && members.len() >= 3 {
        f.push(
            Finding::new("failoverCandidates", Severity::Warning)
                .param("count", 1)
                .link_tool("failover", None),
        );
    }
    if config.synchronous_mode != SyncMode::Off
        && !members
            .iter()
            .any(|m| matches!(m.role, CandidateRole::Sync | CandidateRole::Quorum))
    {
        f.push(Finding::new("syncModeWithoutSync", Severity::Critical));
    }
    if paused {
        f.push(Finding::new("clusterPaused", Severity::Warning));
    }
    // Лидер должен успеть обновить ключ в DCS до истечения `ttl`: `loop_wait` на цикл и `retry_timeout` на повторы.
    if let (Some(ttl), Some(lw), Some(rt)) = (config.ttl, config.loop_wait, config.retry_timeout)
        && ttl < lw + 2 * rt
    {
        f.push(
            Finding::new("patroniTiming", Severity::Warning)
                .param("ttlS", ttl)
                .param("loopWaitS", lw)
                .param("retryTimeoutS", rt),
        );
    }
    crate::findings::sort(&mut f);
    (
        FailoverData {
            paused,
            config: config.clone(),
            candidates,
            ready_count,
        },
        f,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Severity;

    fn m(name: &str, role: CandidateRole, lag: Option<i64>) -> CandidateFacts {
        CandidateFacts {
            name: name.into(),
            role,
            state: Some(
                if role == CandidateRole::Leader {
                    "running"
                } else {
                    "streaming"
                }
                .into(),
            ),
            reachable: true,
            lag_bytes: lag,
            ..Default::default()
        }
    }

    #[test]
    fn verdicts_and_reasons() {
        let mut far = m("c", CandidateRole::Replica, Some(5_000_000));
        far.nofailover = true;
        let mut down = m("d", CandidateRole::Replica, None);
        down.reachable = false;
        let mut zero = m("e", CandidateRole::Replica, Some(0));
        zero.failover_priority = Some(0);
        let cfg = DynConfig {
            maximum_lag_on_failover: DEFAULT_MAXIMUM_LAG_ON_FAILOVER,
            ..Default::default()
        };
        let (d, f) = evaluate(
            &[
                m("a", CandidateRole::Leader, None),
                m("b", CandidateRole::Replica, Some(0)),
                far,
                down,
                zero,
            ],
            &cfg,
            false,
        );
        let r: Vec<(&str, bool, Vec<Reason>)> = d
            .candidates
            .iter()
            .map(|c| (c.member.as_str(), c.ready, c.reasons.clone()))
            .collect();
        assert_eq!(
            r,
            [
                ("b", true, vec![]),
                ("c", false, vec![Reason::LagTooHigh, Reason::NoFailoverTag]),
                ("d", false, vec![Reason::Unreachable]),
                ("e", false, vec![Reason::ZeroPriority]),
            ]
        );
        assert_eq!(d.ready_count, 1);
        assert_eq!(
            f.iter()
                .map(|x| (x.id.as_str(), x.severity))
                .collect::<Vec<_>>(),
            [("failoverCandidates", Severity::Warning)]
        );
    }

    #[test]
    fn synchronous_mode_and_pause_and_timing() {
        let cfg = DynConfig {
            maximum_lag_on_failover: DEFAULT_MAXIMUM_LAG_ON_FAILOVER,
            synchronous_mode: SyncMode::On,
            ttl: Some(20),
            loop_wait: Some(10),
            retry_timeout: Some(10),
            ..Default::default()
        };
        let (d, f) = evaluate(
            &[
                m("a", CandidateRole::Leader, None),
                m("b", CandidateRole::Replica, Some(0)),
            ],
            &cfg,
            true,
        );
        assert_eq!(d.candidates[0].reasons, [Reason::NotSynchronous]);
        let ids: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        assert_eq!(
            ids,
            [
                ("failoverCandidates", Severity::Critical),
                ("syncModeWithoutSync", Severity::Critical),
                ("clusterPaused", Severity::Warning),
                ("patroniTiming", Severity::Warning)
            ]
        );
        let quorum = DynConfig {
            synchronous_mode: SyncMode::Quorum,
            ..cfg.clone()
        };
        let (d, _) = evaluate(
            &[
                m("a", CandidateRole::Leader, None),
                m("b", CandidateRole::Quorum, Some(0)),
            ],
            &quorum,
            false,
        );
        assert!(d.candidates[0].ready);
    }
}
