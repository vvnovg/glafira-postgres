//! Реплики (спец. P2 §5.6): отправители WAL с лагами на primary и каскадной реплике, приёмник, пауза replay и
//! конфликты recovery на реплике. Сбор (SQL) — в приложении, здесь типы данных и правила.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, FindingTarget, Severity, grade, thresholds as th, worse};

/// Строка `pg_stat_replication`. Лаги в байтах — от текущего LSN узла, в секундах — как их считает сервер.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
    pub pid: i32,
    pub user: Option<String>,
    pub application: Option<String>,
    /// Участник топологии, которому соответствует `application_name` (без DNS); не сопоставился — пусто.
    pub member: Option<String>,
    pub client_addr: Option<String>,
    pub state: Option<String>,
    pub sync_state: Option<String>,
    pub sync_priority: Option<i32>,
    pub sent_lag_bytes: Option<i64>,
    pub write_lag_bytes: Option<i64>,
    pub flush_lag_bytes: Option<i64>,
    pub replay_lag_bytes: Option<i64>,
    pub write_lag_s: Option<f64>,
    pub flush_lag_s: Option<f64>,
    pub replay_lag_s: Option<f64>,
    /// Сколько прошло с последнего ответа реплики.
    pub reply_age_s: Option<f64>,
    pub slot_name: Option<String>,
    pub xmin_age: Option<i64>,
}

/// `pg_stat_wal_receiver` реплики. Нет строки — `status` пуст.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receiver {
    pub status: Option<String>,
    pub sender_host: Option<String>,
    pub sender_port: Option<i32>,
    pub slot_name: Option<String>,
    pub received_tli: Option<i64>,
    pub last_msg_age_s: Option<f64>,
    pub latest_end_age_s: Option<f64>,
}

/// Применение WAL на реплике.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    /// Полученный, но ещё не применённый WAL.
    pub receive_replay_gap_bytes: Option<i64>,
    /// 0, если применён весь полученный WAL, иначе возраст последней применённой транзакции.
    pub replay_delay_s: Option<f64>,
    /// `not paused` / `pause requested` / `paused`; на 13 — `paused` / `not paused`.
    pub pause: Option<String>,
}

/// Конфликты recovery — сумма `pg_stat_database_conflicts` по базам.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conflicts {
    pub tablespace: i64,
    pub lock: i64,
    pub snapshot: i64,
    pub bufferpin: i64,
    pub deadlock: i64,
    /// PostgreSQL 16+.
    pub active_logicalslot: Option<i64>,
    /// Прирост с прошлого запроса того же узла (спец. §3.6); без сравнимого снимка — пусто.
    pub growth: Option<i64>,
}

impl Conflicts {
    pub fn total(&self) -> i64 {
        self.tablespace
            + self.lock
            + self.snapshot
            + self.bufferpin
            + self.deadlock
            + self.active_logicalslot.unwrap_or(0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplicationData {
    pub in_recovery: bool,
    /// Отправители WAL — на primary и на каскадной реплике.
    pub senders: Vec<Sender>,
    pub synchronous_standby_names: Option<String>,
    pub synchronous_commit: Option<String>,
    /// Только на узле в recovery.
    pub receiver: Option<Receiver>,
    pub replay: Option<Replay>,
    pub conflicts: Option<Conflicts>,
    pub hot_standby_feedback: Option<bool>,
    /// `-1` — ждать без предела.
    pub max_standby_streaming_delay_ms: Option<i64>,
}

pub fn evaluate(member: Option<&str>, d: &ReplicationData) -> Vec<Finding> {
    let mut out = Vec::new();
    let lag_sev = |bytes: Option<i64>, secs: Option<f64>| {
        worse(
            bytes.and_then(|b| grade(b as f64, th::REPLICA_LAG_BYTES.0, th::REPLICA_LAG_BYTES.1)),
            secs.and_then(|s| grade(s, th::REPLICA_LAG_S.0, th::REPLICA_LAG_S.1)),
        )
    };
    // Отправители: одна находка на правило, цель — худшая реплика (уточнение 2 плана P2a).
    let lagging: Vec<(&Sender, Severity)> = d
        .senders
        .iter()
        .filter_map(|s| lag_sev(s.replay_lag_bytes, s.replay_lag_s).map(|v| (s, v)))
        .collect();
    if let Some((s, sev)) = lagging
        .iter()
        .max_by_key(|(s, v)| (*v, s.replay_lag_bytes.unwrap_or(0)))
    {
        let mut f = Finding::new("replicaLag", *sev)
            .member(member)
            .param("count", lagging.len());
        if let Some(b) = s.replay_lag_bytes {
            f = f.param("lagBytes", b);
        }
        if let Some(t) = s.replay_lag_s {
            f = f.param("delayS", t);
        }
        if let Some(m) = &s.member {
            f = f
                .target(FindingTarget::Member { name: m.clone() })
                .link_tool("replication", Some(m));
        }
        out.push(f);
    }
    let not_streaming: Vec<&Sender> = d
        .senders
        .iter()
        .filter(|s| s.state.as_deref() != Some("streaming"))
        .collect();
    if let Some(s) = not_streaming.first() {
        let mut f = Finding::new("replicaNotStreaming", Severity::Warning)
            .member(member)
            .param("count", not_streaming.len())
            .param("state", s.state.clone().unwrap_or_default());
        if let Some(m) = &s.member {
            f = f.target(FindingTarget::Member { name: m.clone() });
        }
        out.push(f);
    }
    let stale: Vec<&Sender> = d
        .senders
        .iter()
        .filter(|s| s.reply_age_s.is_some_and(|a| a >= th::REPLY_STALE_S))
        .collect();
    if let Some(s) = stale.first() {
        out.push(
            Finding::new("replicaReplyStale", Severity::Warning)
                .member(member)
                .param("count", stale.len())
                .param("maxS", s.reply_age_s.unwrap_or(0.0)),
        );
    }
    if !d.in_recovery
        && d.synchronous_standby_names
            .as_deref()
            .is_some_and(|n| !n.trim().is_empty())
        && !d
            .senders
            .iter()
            .any(|s| matches!(s.sync_state.as_deref(), Some("sync" | "quorum")))
    {
        out.push(Finding::new("syncStandbyMissing", Severity::Critical).member(member));
    }
    if d.in_recovery {
        if d.receiver.as_ref().and_then(|r| r.status.as_deref()) != Some("streaming") {
            out.push(Finding::new("walReceiverDown", Severity::Critical).member(member));
        }
        if let Some(r) = &d.replay {
            if matches!(r.pause.as_deref(), Some("paused" | "pause requested")) {
                out.push(Finding::new("replayPaused", Severity::Warning).member(member));
            }
            if let Some(sev) = lag_sev(r.receive_replay_gap_bytes, r.replay_delay_s) {
                let mut f = Finding::new("replicaLag", sev).member(member);
                if let Some(b) = r.receive_replay_gap_bytes {
                    f = f.param("lagBytes", b);
                }
                if let Some(t) = r.replay_delay_s {
                    f = f.param("delayS", t);
                }
                out.push(f);
            }
        }
        if let Some(c) = &d.conflicts {
            if c.growth.is_some_and(|g| g > 0) {
                out.push(
                    Finding::new("recoveryConflicts", Severity::Warning)
                        .member(member)
                        .param("count", c.growth.unwrap_or(0)),
                );
            } else if c.total() > 0 {
                out.push(
                    Finding::new("recoveryConflictsTotal", Severity::Info)
                        .member(member)
                        .param("count", c.total()),
                );
            }
        }
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::Severity;

    fn sender(app: &str, state: &str, sync: &str, replay_bytes: i64) -> Sender {
        Sender {
            pid: 1,
            application: Some(app.into()),
            member: Some(app.into()),
            state: Some(state.into()),
            sync_state: Some(sync.into()),
            replay_lag_bytes: Some(replay_bytes),
            replay_lag_s: Some(0.0),
            reply_age_s: Some(1.0),
            ..Default::default()
        }
    }

    fn ids(f: &[Finding]) -> Vec<(&str, Severity)> {
        f.iter().map(|x| (x.id.as_str(), x.severity)).collect()
    }

    #[test]
    fn primary_side_findings() {
        let d = ReplicationData {
            senders: vec![
                sender("pg-2", "streaming", "async", 20 * 1024 * 1024),
                sender("pg-3", "catchup", "async", 0),
            ],
            synchronous_standby_names: Some("ANY 1 (pg-2, pg-3)".into()),
            ..Default::default()
        };
        assert_eq!(
            ids(&evaluate(Some("pg-1"), &d)),
            [
                ("syncStandbyMissing", Severity::Critical),
                ("replicaLag", Severity::Warning),
                ("replicaNotStreaming", Severity::Warning)
            ]
        );
        let ok = ReplicationData {
            senders: vec![sender("pg-2", "streaming", "quorum", 0)],
            synchronous_standby_names: Some("ANY 1 (pg-2)".into()),
            ..Default::default()
        };
        assert!(evaluate(None, &ok).is_empty());
    }

    #[test]
    fn replica_side_findings() {
        let d = ReplicationData {
            in_recovery: true,
            receiver: Some(Receiver {
                status: Some("stopping".into()),
                ..Default::default()
            }),
            replay: Some(Replay {
                receive_replay_gap_bytes: Some(0),
                replay_delay_s: Some(40.0),
                pause: Some("paused".into()),
            }),
            conflicts: Some(Conflicts {
                snapshot: 3,
                growth: Some(2),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            ids(&evaluate(Some("pg-2"), &d)),
            [
                ("walReceiverDown", Severity::Critical),
                ("recoveryConflicts", Severity::Warning),
                ("replayPaused", Severity::Warning),
                ("replicaLag", Severity::Warning)
            ]
        );
        let calm = ReplicationData {
            in_recovery: true,
            receiver: Some(Receiver {
                status: Some("streaming".into()),
                ..Default::default()
            }),
            conflicts: Some(Conflicts {
                lock: 1,
                growth: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(
            ids(&evaluate(None, &calm)),
            [("recoveryConflictsTotal", Severity::Info)]
        );
    }

    #[test]
    fn stale_replies_count_totals_and_missing_receiver() {
        let mut stale = sender("pg-2", "streaming", "async", 0);
        stale.reply_age_s = Some(90.0);
        let d = ReplicationData {
            senders: vec![stale],
            ..Default::default()
        };
        let f = evaluate(Some("pg-1"), &d);
        assert_eq!(ids(&f), [("replicaReplyStale", Severity::Warning)]);
        assert_eq!(f[0].params["maxS"], 90.0);
        // Лаг по времени без лага в байтах тоже даёт находку; цель — участник, ссылка ведёт к его вкладке.
        let mut slow = sender("pg-2", "streaming", "async", 0);
        slow.replay_lag_s = Some(400.0);
        let f = evaluate(
            Some("pg-1"),
            &ReplicationData {
                senders: vec![slow],
                ..Default::default()
            },
        );
        assert_eq!(ids(&f), [("replicaLag", Severity::Critical)]);
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Member {
                name: "pg-2".into()
            })
        );
        // Нет строки приёмника — он не работает.
        let down = ReplicationData {
            in_recovery: true,
            ..Default::default()
        };
        assert_eq!(
            ids(&evaluate(None, &down)),
            [("walReceiverDown", Severity::Critical)]
        );
        let c = Conflicts {
            tablespace: 1,
            lock: 2,
            snapshot: 3,
            bufferpin: 4,
            deadlock: 5,
            active_logicalslot: Some(6),
            growth: None,
        };
        assert_eq!(c.total(), 21);
        // Без сравнимого прошлого снимка рост неизвестен: конфликты видны только как накопленные.
        let d = ReplicationData {
            in_recovery: true,
            receiver: Some(Receiver {
                status: Some("streaming".into()),
                ..Default::default()
            }),
            conflicts: Some(c),
            ..Default::default()
        };
        assert_eq!(
            ids(&evaluate(None, &d)),
            [("recoveryConflictsTotal", Severity::Info)]
        );
    }

    #[test]
    fn data_json_is_camel_case() {
        let d = ReplicationData {
            in_recovery: true,
            senders: vec![sender("pg-2", "streaming", "sync", 1)],
            replay: Some(Replay {
                receive_replay_gap_bytes: Some(5),
                replay_delay_s: None,
                pause: None,
            }),
            max_standby_streaming_delay_ms: Some(-1),
            ..Default::default()
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["inRecovery"], true);
        assert_eq!(v["senders"][0]["syncState"], "sync");
        assert_eq!(v["senders"][0]["replayLagBytes"], 1);
        assert_eq!(v["replay"]["receiveReplayGapBytes"], 5);
        assert_eq!(v["maxStandbyStreamingDelayMs"], -1);
    }
}
