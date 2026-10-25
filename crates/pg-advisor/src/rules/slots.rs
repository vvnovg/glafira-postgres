//! Слоты репликации и подписки (спец. P2 §5.7): слоты с удерживаемым WAL и колонками, зависящими от версии, логические
//! подписки с ростом счётчиков ошибок, отметки слотов Patroni. Сбор (SQL) — в приложении, здесь типы данных и правила.

use serde::{Deserialize, Serialize};

use crate::findings::{Finding, FindingTarget, Severity, grade, thresholds as th, worse};

/// Чей это слот в Patroni: слот участника (Patroni создаёт и убирает его сам) или постоянный слот из `/config`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PatroniSlot {
    Member,
    Permanent,
}

/// Строка `pg_replication_slots`. Колонки новее версии сервера пусты (`NULL`), причина — в `unavailable` отчёта.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub name: String,
    /// `physical` / `logical`.
    pub slot_type: String,
    pub plugin: Option<String>,
    /// База логического слота.
    pub database: Option<String>,
    pub temporary: bool,
    pub active: bool,
    pub active_pid: Option<i32>,
    /// Возраст `xmin` слота (физического — с обратной связью реплики) в транзакциях.
    pub xmin_age: Option<i64>,
    pub catalog_xmin_age: Option<i64>,
    pub restart_lsn: Option<String>,
    pub confirmed_flush_lsn: Option<String>,
    /// `reserved` / `extended` / `unreserved` / `lost`.
    pub wal_status: Option<String>,
    /// Сколько WAL ещё можно записать до потери слота (при `max_slot_wal_keep_size`); пусто — без предела.
    pub safe_wal_size: Option<i64>,
    /// Текущий LSN узла (на реплике — применённый) минус `restart_lsn`: сколько WAL удерживает слот.
    pub retained_bytes: Option<i64>,
    /// PostgreSQL 14+.
    pub two_phase: Option<bool>,
    /// PostgreSQL 16+.
    pub conflicting: Option<bool>,
    /// PostgreSQL 17+: момент (мс эпохи), с которого слот неактивен; пусто — слот активен.
    pub inactive_since_ms: Option<i64>,
    /// PostgreSQL 17+.
    pub invalidation_reason: Option<String>,
    /// PostgreSQL 17+.
    pub failover: Option<bool>,
    /// PostgreSQL 17+.
    pub synced: Option<bool>,
    /// Только у подключения `patroni`; без `/config` отмечены лишь слоты участников.
    pub patroni: Option<PatroniSlot>,
}

/// Логическая подписка (`pg_stat_subscription`, строка основного процесса: `relid IS NULL`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub name: String,
    /// Нет процесса применения — подписка не работает.
    pub pid: Option<i32>,
    pub received_lsn: Option<String>,
    pub last_msg_age_s: Option<f64>,
    pub latest_end_age_s: Option<f64>,
    /// `pg_stat_subscription_stats`, PostgreSQL 15+.
    pub apply_error_count: Option<i64>,
    pub sync_error_count: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotsData {
    pub slots: Vec<Slot>,
    pub subscriptions: Vec<Subscription>,
    /// Значение `max_slot_wal_keep_size` в мегабайтах; `-1` — без ограничения.
    pub max_slot_wal_keep_size_mb: Option<i64>,
    /// Прирост суммы ошибок применения и синхронизации с прошлого запроса того же узла (спец. §3.6); без
    /// сравнимого снимка — пусто.
    pub subscription_errors_growth: Option<i64>,
    /// Роль подключения может удалять слоты: суперпользователь или `REPLICATION` (спец. §8.7). Старые сохранённые
    /// отчёты поля не знают — для них «нельзя».
    #[serde(default)]
    pub can_drop_slots: bool,
}

/// Имя слота участника, как его строит Patroni (`slot_name_from_member_name`): нижний регистр, `-` и `.` → `_`,
/// прочие символы вне `[a-z0-9_]` → `u` и четыре цифры кода символа, не длиннее 63 символов.
pub fn patroni_slot_name(member: &str) -> String {
    let mut out = String::new();
    for c in member.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if c == '-' || c == '.' {
            out.push('_');
        } else {
            out.push_str(&format!("u{:04}", c as u32));
        }
    }
    out.chars().take(63).collect()
}

pub fn evaluate(member: Option<&str>, d: &SlotsData) -> Vec<Finding> {
    let mut out = Vec::new();
    // Потерянные слоты — одна находка на узел: цель — первый, `count` — сколько их.
    let lost: Vec<&Slot> = d
        .slots
        .iter()
        .filter(|s| s.wal_status.as_deref() == Some("lost"))
        .collect();
    if let Some(s) = lost.first() {
        out.push(
            Finding::new("slotLost", Severity::Critical)
                .member(member)
                .target(FindingTarget::Slot {
                    name: s.name.clone(),
                })
                .param("count", lost.len()),
        );
    }
    // Неактивный слот держит WAL: оценка по объёму, а `unreserved` (WAL вот-вот удалят) — всегда critical.
    // Активный слот копит WAL, пока реплика его читает, — это лаг, а не забытый слот. Синхронизированный слот
    // (`synced`, копия слота failover на реплике PostgreSQL 17+) неактивен по устройству и на реплике не удаляется:
    // забытым он быть не может, поэтому в оценку не идёт (в списке данных остаётся).
    let inactive = d
        .slots
        .iter()
        .filter(|s| !s.active && s.synced != Some(true) && s.wal_status.as_deref() != Some("lost"));
    let graded: Vec<(&Slot, Severity)> = inactive
        .filter_map(|s| {
            let by_bytes = s.retained_bytes.and_then(|b| {
                grade(
                    b as f64,
                    th::SLOT_RETAINED_BYTES.0,
                    th::SLOT_RETAINED_BYTES.1,
                )
            });
            let unreserved =
                (s.wal_status.as_deref() == Some("unreserved")).then_some(Severity::Critical);
            worse(by_bytes, unreserved).map(|v| (s, v))
        })
        .collect();
    if let Some((s, sev)) = graded
        .iter()
        .max_by_key(|(s, v)| (*v, s.retained_bytes.unwrap_or(0)))
    {
        let mut f = Finding::new("slotRetainsWal", *sev)
            .member(member)
            .target(FindingTarget::Slot {
                name: s.name.clone(),
            })
            .param("count", graded.len())
            .link_tool("slots", member);
        if let Some(b) = s.retained_bytes {
            f = f.param("retainedBytes", b);
        }
        if let Some(w) = &s.wal_status {
            f = f.param("walStatus", w.clone());
        }
        out.push(f);
    }
    if let Some(g) = d.subscription_errors_growth.filter(|g| *g > 0) {
        out.push(
            Finding::new("subscriptionErrors", Severity::Warning)
                .member(member)
                .param("count", g),
        );
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::{FindingTarget, Severity};

    const GIB: i64 = 1024 * 1024 * 1024;

    fn slot(name: &str, active: bool, retained: i64, wal_status: &str) -> Slot {
        Slot {
            name: name.into(),
            slot_type: "physical".into(),
            active,
            retained_bytes: Some(retained),
            wal_status: Some(wal_status.into()),
            ..Default::default()
        }
    }

    #[test]
    fn patroni_member_slot_names() {
        assert_eq!(patroni_slot_name("patroni1"), "patroni1");
        assert_eq!(patroni_slot_name("PG-1.dc"), "pg_1_dc");
        assert_eq!(patroni_slot_name("a b"), "au0032b");
        assert_eq!(patroni_slot_name(&"x".repeat(70)).len(), 63);
    }

    #[test]
    fn retained_wal_lost_and_subscription_errors() {
        let d = SlotsData {
            slots: vec![
                slot("busy", true, 50 * GIB, "extended"),
                slot("dead", false, 2 * GIB, "extended"),
                slot("old", false, 0, "lost"),
            ],
            subscription_errors_growth: Some(3),
            ..Default::default()
        };
        let f = evaluate(Some("pg-1"), &d);
        let got: Vec<(&str, Severity)> = f.iter().map(|x| (x.id.as_str(), x.severity)).collect();
        assert_eq!(
            got,
            [
                ("slotLost", Severity::Critical),
                ("slotRetainsWal", Severity::Warning),
                ("subscriptionErrors", Severity::Warning)
            ]
        );
        let retains = f.iter().find(|x| x.id == "slotRetainsWal").unwrap();
        assert_eq!(
            retains.target,
            Some(FindingTarget::Slot {
                name: "dead".into()
            })
        );
        assert_eq!(retains.params["retainedBytes"], 2 * GIB);
        let unreserved = SlotsData {
            slots: vec![slot("u", false, 10, "unreserved")],
            ..Default::default()
        };
        assert_eq!(evaluate(None, &unreserved)[0].severity, Severity::Critical);
    }

    #[test]
    fn retained_wal_thresholds_are_inclusive_and_only_inactive_slots_count() {
        let sev = |bytes: i64| {
            let d = SlotsData {
                slots: vec![slot("s", false, bytes, "reserved")],
                ..Default::default()
            };
            evaluate(None, &d).first().map(|f| f.severity)
        };
        assert_eq!(sev(GIB - 1), None);
        assert_eq!(sev(GIB), Some(Severity::Warning));
        assert_eq!(sev(10 * GIB - 1), Some(Severity::Warning));
        assert_eq!(sev(10 * GIB), Some(Severity::Critical));
        // Активный слот копит WAL, пока его читает реплика, — находки нет, даже если объём огромен.
        let busy = SlotsData {
            slots: vec![slot("busy", true, 100 * GIB, "extended")],
            ..Default::default()
        };
        assert!(evaluate(None, &busy).is_empty());
    }

    #[test]
    fn one_finding_per_rule_points_at_the_worst_slot_and_counts_the_rest() {
        let d = SlotsData {
            slots: vec![
                slot("a", false, 2 * GIB, "reserved"),
                slot("b", false, 12 * GIB, "reserved"),
                slot("c", false, 3 * GIB, "reserved"),
                slot("gone1", false, 0, "lost"),
                slot("gone2", false, 0, "lost"),
            ],
            ..Default::default()
        };
        let f = evaluate(Some("pg-1"), &d);
        let lost = f.iter().find(|x| x.id == "slotLost").unwrap();
        assert_eq!(lost.params["count"], 2);
        assert_eq!(
            lost.target,
            Some(FindingTarget::Slot {
                name: "gone1".into()
            })
        );
        let r = f.iter().find(|x| x.id == "slotRetainsWal").unwrap();
        assert_eq!(r.severity, Severity::Critical);
        assert_eq!(r.target, Some(FindingTarget::Slot { name: "b".into() }));
        assert_eq!(
            (&r.params["count"], &r.params["walStatus"]),
            (&3.into(), &"reserved".into())
        );
        assert_eq!(r.links.len(), 1, "кнопка ведёт на вкладку слотов узла");
    }

    #[test]
    fn synced_standby_copies_of_failover_slots_are_not_forgotten_slots() {
        let synced = |synced: Option<bool>| {
            let mut s = slot("failover_copy", false, 20 * GIB, "reserved");
            s.synced = synced;
            s
        };
        let ids = |s: Slot| -> Vec<String> {
            let d = SlotsData {
                slots: vec![s],
                ..Default::default()
            };
            evaluate(Some("pg-2"), &d)
                .into_iter()
                .map(|f| f.id)
                .collect()
        };
        // Неактивный синхронизированный слот с 20 ГиБ — находки нет; такой же не синхронизированный — есть.
        assert!(ids(synced(Some(true))).is_empty());
        assert_eq!(ids(synced(Some(false))), ["slotRetainsWal"]);
        assert_eq!(ids(synced(None)), ["slotRetainsWal"]);
        // Синхронизированный слот не прячет соседний забытый: цель находки — тот, что не синхронизирован.
        let d = SlotsData {
            slots: vec![
                synced(Some(true)),
                slot("forgotten", false, 2 * GIB, "reserved"),
            ],
            ..Default::default()
        };
        let f = evaluate(None, &d);
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Slot {
                name: "forgotten".into()
            })
        );
        assert_eq!(f[0].params["count"], 1);
    }

    #[test]
    fn subscription_errors_need_a_positive_growth() {
        let with = |g| SlotsData {
            subscription_errors_growth: g,
            ..Default::default()
        };
        assert!(evaluate(None, &with(None)).is_empty());
        assert!(evaluate(None, &with(Some(0))).is_empty());
        assert_eq!(evaluate(None, &with(Some(1)))[0].id, "subscriptionErrors");
    }

    #[test]
    fn slot_json_is_camel_case_with_the_patroni_mark() {
        let s = Slot {
            name: "debezium".into(),
            slot_type: "logical".into(),
            patroni: Some(PatroniSlot::Permanent),
            ..Default::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(
            (&v["slotType"], &v["patroni"], &v["retainedBytes"]),
            (
                &"logical".into(),
                &"permanent".into(),
                &serde_json::Value::Null
            )
        );
        let back: Slot = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn patroni_slot_names_follow_the_patroni_rule_for_unusual_characters() {
        assert_eq!(patroni_slot_name("db-1.dc2_x"), "db_1_dc2_x");
        // Код символа — десятичный (`f"u{ord(c):04d}"` в Patroni), регистр понижается до замены.
        assert_eq!(patroni_slot_name("Узел"), "u1091u1079u1077u1083");
        // Слот не длиннее 63 символов; `-` на 63-й позиции — ещё `_`.
        assert_eq!(
            patroni_slot_name(&format!("{}-", "a".repeat(62))),
            format!("{}_", "a".repeat(62))
        );
    }
}
