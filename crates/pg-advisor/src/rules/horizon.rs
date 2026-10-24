//! Горизонт xmin (спец. P2 §5.5): что не даёт VACUUM убирать мёртвые строки — сессии, prepared transactions, слоты,
//! обратная связь реплик. Сбор (SQL) — в приложении, здесь типы данных и правило.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, FindingTarget, grade, thresholds as th, worse};

/// Чем держится горизонт. Слот с `hot_standby_feedback` реплики — тоже `Slot` (xmin лежит в слоте), а
/// `StandbyFeedback` — отправитель WAL без слота (`pg_stat_replication.backend_xmin`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HolderKind {
    #[default]
    Session,
    PreparedXact,
    Slot,
    StandbyFeedback,
}

/// Держатель горизонта. `xmin_age` — возраст его xmin в транзакциях, `duration_s` — как долго он держит (от `now()`
/// сервера), если это известно.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holder {
    pub kind: HolderKind,
    /// Сессия — `pid`, prepared transaction — `gid`, слот — имя слота, обратная связь — `application_name`.
    pub name: String,
    /// Участник топологии (сопоставление по `application_name`): у `StandbyFeedback` и у слота с активным отправителем
    /// WAL (физический слот реплики).
    pub member: Option<String>,
    pub xmin_age: i64,
    pub duration_s: Option<f64>,
    pub pid: Option<i32>,
    /// `backend_start` сессии в микросекундах эпохи: вместе с `pid` — ключ сессии.
    pub backend_start_us: Option<i64>,
    pub user: Option<String>,
    pub application: Option<String>,
    pub database: Option<String>,
    pub state: Option<String>,
    pub gid: Option<String>,
    pub slot_type: Option<String>,
    pub active: Option<bool>,
    /// Слот держит горизонт каталога (`catalog_xmin` старше `xmin`), а не данных.
    pub catalog: bool,
    pub client_addr: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HorizonData {
    /// По убыванию `xmin_age` — порядок задаёт сбор.
    pub holders: Vec<Holder>,
    /// Значение `hot_standby_feedback` на узле.
    pub hot_standby_feedback: Option<bool>,
}

/// Строковая константа SQL: кавычки удваиваются (`standard_conforming_strings = on`).
pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn kind_name(k: HolderKind) -> &'static str {
    match k {
        HolderKind::Session => "session",
        HolderKind::PreparedXact => "preparedXact",
        HolderKind::Slot => "slot",
        HolderKind::StandbyFeedback => "standbyFeedback",
    }
}

/// Горизонт — самый старый держатель; одна находка (остальные видны в таблице).
pub fn evaluate(member: Option<&str>, d: &HorizonData) -> Vec<Finding> {
    let Some(h) = d.holders.first() else {
        return vec![];
    };
    let by_age = grade(h.xmin_age as f64, th::XMIN_AGE.0, th::XMIN_AGE.1);
    let by_time = h
        .duration_s
        .and_then(|s| grade(s, th::XMIN_DURATION_S.0, th::XMIN_DURATION_S.1));
    let Some(sev) = worse(by_age, by_time) else {
        return vec![];
    };
    let mut f = Finding::new("xminHorizon", sev)
        .member(member)
        .param("kind", kind_name(h.kind))
        .param("name", h.name.clone())
        .param("xminAge", h.xmin_age);
    if let Some(s) = h.duration_s {
        f = f.param("maxS", s);
    }
    f = match h.kind {
        HolderKind::Session => {
            if let (Some(pid), Some(us)) = (h.pid, h.backend_start_us) {
                f = f.target(FindingTarget::Session {
                    pid,
                    backend_start_us: us,
                });
            }
            f.link_tool("sessions", member)
        }
        HolderKind::Slot => {
            // Тип слота и его состояние — в параметры: по ним интерфейс отличает «забытый» слот от живой реплики.
            if let Some(t) = &h.slot_type {
                f = f.param("slotType", t.clone());
            }
            if let Some(a) = h.active {
                f = f.param("active", a);
            }
            match (&h.member, h.slot_type.as_deref(), h.active) {
                // Обратная связь реплики через физический слот: xmin лежит в слоте, но держит горизонт реплика —
                // чинить нужно на ней (её долгие запросы), поэтому цель и ссылка ведут к участнику.
                (Some(m), Some("physical"), Some(true)) => f
                    .target(FindingTarget::Member { name: m.clone() })
                    .link_tool("horizon", Some(m)),
                _ => f
                    .target(FindingTarget::Slot {
                        name: h.name.clone(),
                    })
                    .link_tool("slots", member),
            }
        }
        HolderKind::PreparedXact => f.param(
            "command",
            format!(
                "ROLLBACK PREPARED {};",
                quote_literal(h.gid.as_deref().unwrap_or(&h.name))
            ),
        ),
        HolderKind::StandbyFeedback => match &h.member {
            Some(m) => f
                .target(FindingTarget::Member { name: m.clone() })
                .link_tool("horizon", Some(m)),
            None => f,
        },
    };
    vec![f]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{FindingLink, FindingTarget, Severity};

    fn holder(kind: HolderKind, name: &str, age: i64, dur: Option<f64>) -> Holder {
        Holder {
            kind,
            name: name.into(),
            xmin_age: age,
            duration_s: dur,
            ..Default::default()
        }
    }

    #[test]
    fn the_oldest_holder_is_the_horizon() {
        let mut s = holder(HolderKind::Session, "4242", 12_000_000, Some(100.0));
        s.pid = Some(4242);
        s.backend_start_us = Some(7);
        let d = HorizonData {
            holders: vec![s, holder(HolderKind::Slot, "dead", 10, None)],
            hot_standby_feedback: Some(false),
        };
        let f = evaluate(Some("pg-1"), &d);
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].id.as_str(), f[0].severity),
            ("xminHorizon", Severity::Warning)
        );
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Session {
                pid: 4242,
                backend_start_us: 7
            })
        );
        assert_eq!(f[0].params["kind"], "session");
        assert_eq!(
            f[0].links,
            [FindingLink::OpenTool {
                tool: "sessions".into(),
                member: Some("pg-1".into())
            }]
        );
    }

    #[test]
    fn duration_alone_can_raise_and_prepared_gets_a_command() {
        let mut p = holder(HolderKind::PreparedXact, "it's", 5, Some(30_000.0));
        p.gid = Some("it's".into());
        let f = evaluate(
            None,
            &HorizonData {
                holders: vec![p],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(f[0].severity, Severity::Critical);
        assert_eq!(f[0].params["command"], "ROLLBACK PREPARED 'it''s';");
        let mut fb = holder(
            HolderKind::StandbyFeedback,
            "walreceiver",
            200_000_000,
            None,
        );
        fb.member = Some("pg-2".into());
        let f = evaluate(
            Some("pg-1"),
            &HorizonData {
                holders: vec![fb],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Member {
                name: "pg-2".into()
            })
        );
        assert_eq!(
            f[0].links,
            [FindingLink::OpenTool {
                tool: "horizon".into(),
                member: Some("pg-2".into())
            }]
        );
        assert!(evaluate(None, &HorizonData::default()).is_empty());
    }

    #[test]
    fn replica_feedback_through_a_physical_slot_points_at_the_replica() {
        let mut slot = holder(HolderKind::Slot, "replica1", 20_000_000, None);
        slot.slot_type = Some("physical".into());
        slot.active = Some(true);
        slot.application = Some("pg-2".into());
        slot.member = Some("pg-2".into());
        let f = evaluate(
            Some("pg-1"),
            &HorizonData {
                holders: vec![slot.clone()],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Member {
                name: "pg-2".into()
            })
        );
        assert_eq!(
            f[0].links,
            [FindingLink::OpenTool {
                tool: "horizon".into(),
                member: Some("pg-2".into())
            }]
        );
        assert_eq!(
            (
                &f[0].params["kind"],
                &f[0].params["slotType"],
                &f[0].params["active"]
            ),
            (&"slot".into(), &"physical".into(), &true.into())
        );
        // Отправитель не сопоставился с участником — цель слот, как у любого слота.
        slot.member = None;
        let f = evaluate(
            Some("pg-1"),
            &HorizonData {
                holders: vec![slot.clone()],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Slot {
                name: "replica1".into()
            })
        );
        assert_eq!(
            f[0].links,
            [FindingLink::OpenTool {
                tool: "slots".into(),
                member: Some("pg-1".into())
            }]
        );
        assert_eq!(
            (&f[0].params["slotType"], &f[0].params["active"]),
            (&"physical".into(), &true.into())
        );
        // Участник известен, но слот логический или неактивен: держит слот, а не реплика.
        for (ty, active) in [("logical", true), ("physical", false)] {
            let mut s = slot.clone();
            s.member = Some("pg-2".into());
            s.slot_type = Some(ty.into());
            s.active = Some(active);
            let f = evaluate(
                Some("pg-1"),
                &HorizonData {
                    holders: vec![s],
                    hot_standby_feedback: None,
                },
            );
            assert_eq!(
                f[0].target,
                Some(FindingTarget::Slot {
                    name: "replica1".into()
                }),
                "{ty} {active}"
            );
            assert_eq!(f[0].params["active"], active);
        }
        // Тип и состояние неизвестны — параметров нет.
        let bare = holder(HolderKind::Slot, "s", 20_000_000, None);
        let f = evaluate(
            None,
            &HorizonData {
                holders: vec![bare],
                hot_standby_feedback: None,
            },
        );
        assert!(!f[0].params.contains_key("slotType") && !f[0].params.contains_key("active"));
    }

    #[test]
    fn slot_and_session_targets_and_json_shape() {
        let slot = holder(HolderKind::Slot, "s1", 20_000_000, None);
        let f = evaluate(
            Some("pg-1"),
            &HorizonData {
                holders: vec![slot.clone()],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(f[0].target, Some(FindingTarget::Slot { name: "s1".into() }));
        assert_eq!(f[0].params["kind"], "slot");
        assert_eq!(
            f[0].links,
            [FindingLink::OpenTool {
                tool: "slots".into(),
                member: Some("pg-1".into())
            }]
        );
        // Сессия без `backend_start` цели не получает: пару (pid, start) нельзя составить.
        let s = holder(HolderKind::Session, "5", 20_000_000, None);
        let f = evaluate(
            None,
            &HorizonData {
                holders: vec![s],
                hot_standby_feedback: None,
            },
        );
        assert_eq!(f[0].target, None);
        // Обратная связь, не сопоставленная с участником, — находка без цели и ссылки.
        let fb = holder(HolderKind::StandbyFeedback, "walreceiver", 20_000_000, None);
        let f = evaluate(
            None,
            &HorizonData {
                holders: vec![fb],
                hot_standby_feedback: None,
            },
        );
        assert_eq!((f[0].target.clone(), f[0].links.len()), (None, 0));
        // Ниже порогов по возрасту и по времени — находки нет.
        let calm = holder(HolderKind::Session, "6", 9_999_999, Some(3599.0));
        assert!(
            evaluate(
                None,
                &HorizonData {
                    holders: vec![calm],
                    hot_standby_feedback: None
                }
            )
            .is_empty()
        );
        let v = serde_json::to_value(Holder {
            kind: HolderKind::PreparedXact,
            xmin_age: 3,
            ..slot
        })
        .unwrap();
        assert_eq!(v["kind"], "preparedXact");
        assert_eq!(v["xminAge"], 3);
        assert!(v.get("backendStartUs").is_some());
    }
}
