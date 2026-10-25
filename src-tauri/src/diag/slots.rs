//! Слоты репликации и подписки (спец. P2 §5.7). Рост ошибок подписок — по двум снимкам счётчика того же узла
//! (спец. §3.6); отметки слотов Patroni — только у подключения `patroni` по `/config` и топологии.

use std::collections::BTreeMap;

use pg_advisor::failover::DynConfig;
use pg_advisor::rates::{self, CounterSnapshot};
use pg_advisor::rules::slots::{
    self as rules, PatroniSlot, Slot, SlotsData, Subscription, patroni_slot_name,
};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::conn::profile::ConnectionKind;
use crate::diag::caps::{Caps, V14, V15, V16, V17};
use crate::diag::{DiagData, DiagReport, Tool, Unavailable, head, make_report, on_member, patroni};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::runtime::DiagTarget;

/// Слоты. Колонки: 0 имя, 1 тип, 2 плагин, 3 БД, 4 временный, 5 активный, 6 pid, 7–8 возраст `xmin` и `catalog_xmin`,
/// 9–10 `restart_lsn` и `confirmed_flush_lsn`, 11 `wal_status`, 12 `safe_wal_size`, 13 удерживаемый WAL в байтах (от
/// текущего LSN узла, на реплике — от применённого), 14 `two_phase` (14+), 15 `conflicting` (16+), 16 момент
/// неактивности в мс эпохи (17+), 17 причина инвалидации (17+), 18 `failover` (17+), 19 `synced` (17+).
pub fn slots_sql(caps: Caps) -> String {
    let two_phase = caps.col(V14, "two_phase", "bool");
    let conflicting = caps.col(V16, "conflicting", "bool");
    let inactive_since = caps.col(
        V17,
        "(extract(epoch FROM inactive_since) * 1000)::bigint",
        "bigint",
    );
    let reason = caps.col(V17, "invalidation_reason", "text");
    let failover = caps.col(V17, "failover", "bool");
    let synced = caps.col(V17, "synced", "bool");
    format!(
        "SELECT slot_name::text, slot_type, plugin::text, database::text, temporary, active, active_pid, \
                age(xmin)::bigint, age(catalog_xmin)::bigint, restart_lsn::text, confirmed_flush_lsn::text, wal_status, \
                safe_wal_size, pg_wal_lsn_diff(CASE WHEN pg_is_in_recovery() THEN pg_last_wal_replay_lsn() \
                                                    ELSE pg_current_wal_lsn() END, restart_lsn)::bigint, \
                {two_phase}, {conflicting}, {inactive_since}, {reason}, {failover}, {synced} \
         FROM pg_replication_slots ORDER BY slot_name"
    )
}

/// Подписки (строка основного процесса применения: `relid IS NULL`; подписка без процесса — строка с пустым `pid`).
/// На 16+ параллельные воркеры применения (`streaming = parallel`) тоже идут с `relid IS NULL`, но у них задан
/// `leader_pid`: без `leader_pid IS NULL` подписка попала бы в список несколько раз, а её счётчики ошибок из
/// `pg_stat_subscription_stats` повторились бы на каждой строке и задвоили сумму — отсюда ложный «рост» при
/// появлении и исчезновении воркеров. Колонки: 0 имя, 1 pid, 2 `received_lsn`, 3–4 возраст последнего сообщения и последней позиции отправителя в секундах,
/// 5–6 число ошибок применения и синхронизации (15+).
pub fn subscriptions_sql(caps: Caps) -> String {
    let (stats, apply, sync) = if caps.at_least(V15) {
        (
            "LEFT JOIN pg_stat_subscription_stats st ON st.subid = s.subid",
            "st.apply_error_count",
            "st.sync_error_count",
        )
    } else {
        ("", "NULL::bigint", "NULL::bigint")
    };
    // Колонка `leader_pid` появилась в 16: на более старых версиях параллельных воркеров применения нет.
    let leaders = if caps.at_least(V16) {
        " AND s.leader_pid IS NULL"
    } else {
        ""
    };
    format!(
        "SELECT s.subname::text, s.pid, s.received_lsn::text, extract(epoch FROM now() - s.last_msg_receipt_time)::float8, \
                extract(epoch FROM now() - s.latest_end_time)::float8, {apply}, {sync} \
         FROM pg_stat_subscription s {stats} WHERE s.relid IS NULL{leaders} ORDER BY s.subname"
    )
}

/// Колонки: 0 `max_slot_wal_keep_size` в мегабайтах (`-1` — без ограничения), 1 `now()` сервера, 2 запуск postmaster,
/// 3 наибольший `stats_reset` (мс эпохи) — основа снимка счётчика, как у конфликтов recovery.
const KEEP_SIZE_SQL: &str = "SELECT setting::bigint, (extract(epoch FROM now()) * 1000)::bigint, \
    (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
    (SELECT max((extract(epoch FROM stats_reset) * 1000)::bigint) FROM pg_stat_database) \
  FROM pg_settings WHERE name = 'max_slot_wal_keep_size'";

/// Ключ счётчика ошибок подписок (сумма применения и синхронизации) в снимке.
const ERRORS_KEY: &str = "subscriptionErrors";

/// Отметки слотов Patroni: слоты участников (имя по правилу Patroni) и постоянные из `/config`.
pub fn mark_patroni(d: &mut SlotsData, member_names: &[String], permanent: &[String]) {
    let members: Vec<String> = member_names.iter().map(|m| patroni_slot_name(m)).collect();
    for s in &mut d.slots {
        s.patroni = if members.contains(&s.name) {
            Some(PatroniSlot::Member)
        } else if permanent.contains(&s.name) {
            Some(PatroniSlot::Permanent)
        } else {
            None
        };
    }
}

/// Отметки по результату чтения `/config`: слоты участников узнаются по топологии и без него, постоянные — только по
/// нему. Не прочитался — отметки лишь у слотов участников и причина для `unavailable`.
pub(crate) fn mark_by_config(
    d: &mut SlotsData,
    id: &str,
    members: &[String],
    config: AppResult<DynConfig>,
) -> Option<Unavailable> {
    match config {
        Ok(config) => {
            mark_patroni(d, members, &config.permanent_slots);
            None
        }
        Err(e) => {
            // Причина (таймаут, TLS, отказ узла) в `unavailable` не попадает — оставляем её в журнале.
            tracing::warn!(connection = %id, "Patroni /config not read for slot marks: {}", e.message);
            mark_patroni(d, members, &[]);
            Some(Unavailable {
                part: "patroniSlots".to_string(),
                member: None,
                reason: ltext!(
                    "diag.unavailable.patroniConfig",
                    "{{part}}: the Patroni configuration could not be read",
                    part = "patroniSlots"
                ),
            })
        }
    }
}

/// Данные узла и снимок счётчика ошибок подписок (рост считает `report`: прошлый снимок хранит `AppState`).
pub async fn collect(c: &Client, caps: Caps) -> AppResult<(SlotsData, CounterSnapshot)> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let mut d = SlotsData::default();
    for r in c.query(&slots_sql(caps), &[]).await.map_err(pg)? {
        d.slots.push(Slot {
            name: r.get(0),
            slot_type: r.get(1),
            plugin: r.get(2),
            database: r.get(3),
            temporary: r.get(4),
            active: r.get(5),
            active_pid: r.get(6),
            xmin_age: r.get(7),
            catalog_xmin_age: r.get(8),
            restart_lsn: r.get(9),
            confirmed_flush_lsn: r.get(10),
            wal_status: r.get(11),
            safe_wal_size: r.get(12),
            retained_bytes: r.get(13),
            two_phase: r.get(14),
            conflicting: r.get(15),
            inactive_since_ms: r.get(16),
            invalidation_reason: r.get(17),
            failover: r.get(18),
            synced: r.get(19),
            patroni: None,
        });
    }
    // Сумма ошибок — только там, где статистика подписок есть (15+): иначе нулевой «рост» выглядел бы как измерение.
    let mut errors = 0_i64;
    for r in c.query(&subscriptions_sql(caps), &[]).await.map_err(pg)? {
        let sub = Subscription {
            name: r.get(0),
            pid: r.get(1),
            received_lsn: r.get(2),
            last_msg_age_s: r.get(3),
            latest_end_age_s: r.get(4),
            apply_error_count: r.get(5),
            sync_error_count: r.get(6),
        };
        errors += sub.apply_error_count.unwrap_or(0) + sub.sync_error_count.unwrap_or(0);
        d.subscriptions.push(sub);
    }
    let r = c.query_one(KEEP_SIZE_SQL, &[]).await.map_err(pg)?;
    d.max_slot_wal_keep_size_mb = r.get(0);
    let values = if caps.at_least(V15) {
        BTreeMap::from([(ERRORS_KEY.to_string(), errors)])
    } else {
        BTreeMap::new()
    };
    let snapshot = CounterSnapshot {
        at_ms: r.get(1),
        started_ms: r.get(2),
        reset_ms: r.get(3),
        values,
    };
    Ok((d, snapshot))
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let (h, mut data, snapshot) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let (data, snapshot) = collect(c, Caps::new(h.version_num)).await?;
            Ok((h, data, snapshot))
        })
    })
    .await?;
    let member = target.member.as_ref().map(|m| m.name.as_str());
    if let Some(member) = member {
        let prev = st
            .counters
            .swap(id, target.generation, member, Tool::Slots, snapshot.clone());
        data.subscription_errors_growth = rates::delta(prev.as_ref(), &snapshot, ERRORS_KEY);
    }
    let caps = Caps::new(h.version_num);
    let mut unavailable: Vec<Unavailable> = [
        caps.missing(V14, "slotTwoPhase"),
        caps.missing(V16, "slotConflicting"),
        caps.missing(V17, "slotInactiveSince"),
        caps.missing(V15, "subscriptionErrors"),
    ]
    .into_iter()
    .flatten()
    .collect();
    // Слоты Patroni — только у подключения `patroni`.
    if target.profile.kind == ConnectionKind::Patroni {
        let members: Vec<String> = target
            .topology
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let config = patroni::dyn_config(st, target).await;
        unavailable.extend(mark_by_config(&mut data, id, &members, config));
    }
    let findings = rules::evaluate(member, &data);
    Ok(make_report(
        Tool::Slots,
        target,
        Some(&h),
        DiagData::Slots(data),
        findings,
        unavailable,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_columns_and_patroni_marks() {
        assert!(slots_sql(Caps::new(130000)).contains("NULL::bool"));
        assert!(slots_sql(Caps::new(170000)).contains("inactive_since"));
        assert!(!subscriptions_sql(Caps::new(140000)).contains("pg_stat_subscription_stats"));
        assert!(subscriptions_sql(Caps::new(150000)).contains("pg_stat_subscription_stats"));
        let mut d = SlotsData {
            slots: ["patroni2", "debezium", "other"]
                .iter()
                .map(|n| Slot {
                    name: n.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        mark_patroni(
            &mut d,
            &["patroni1".into(), "patroni2".into()],
            &["debezium".into()],
        );
        let marks: Vec<Option<PatroniSlot>> = d.slots.iter().map(|s| s.patroni).collect();
        assert_eq!(
            marks,
            [
                Some(PatroniSlot::Member),
                Some(PatroniSlot::Permanent),
                None
            ]
        );
    }

    #[test]
    fn unreadable_patroni_config_leaves_only_member_marks_and_says_why() {
        let slots = |names: &[&str]| SlotsData {
            slots: names
                .iter()
                .map(|n| Slot {
                    name: n.to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let marks = |d: &SlotsData| -> Vec<Option<PatroniSlot>> {
            d.slots.iter().map(|s| s.patroni).collect()
        };
        let members = ["patroni1".to_string(), "PG-2.dc".to_string()];

        let mut d = slots(&["patroni1", "pg_2_dc", "debezium", "other"]);
        let config = DynConfig {
            permanent_slots: vec!["debezium".into()],
            ..Default::default()
        };
        assert!(mark_by_config(&mut d, "conn-1", &members, Ok(config)).is_none());
        assert_eq!(
            marks(&d),
            [
                Some(PatroniSlot::Member),
                Some(PatroniSlot::Member),
                Some(PatroniSlot::Permanent),
                None
            ]
        );

        let mut d = slots(&["patroni1", "debezium"]);
        let err = crate::error::AppError::internal_with("no /config");
        let u =
            mark_by_config(&mut d, "conn-1", &members, Err(err)).expect("причина в unavailable");
        assert_eq!((u.part.as_str(), u.member), ("patroniSlots", None));
        assert_eq!(u.reason.key, "diag.unavailable.patroniConfig");
        assert_eq!(
            u.reason.text,
            "patroniSlots: the Patroni configuration could not be read"
        );
        assert_eq!(marks(&d), [Some(PatroniSlot::Member), None]);
    }

    #[test]
    fn new_columns_are_typed_nulls_before_their_version() {
        let old = slots_sql(Caps::new(130000));
        for col in [
            "two_phase",
            "conflicting",
            "inactive_since",
            "invalidation_reason",
        ] {
            assert!(!old.contains(col), "{col}");
        }
        let v16 = slots_sql(Caps::new(160000));
        assert!(v16.contains("two_phase") && v16.contains("conflicting"));
        assert!(!v16.contains("inactive_since") && !v16.contains("synced"));
        assert!(slots_sql(Caps::new(170000)).contains("invalidation_reason"));
        assert!(subscriptions_sql(Caps::new(140000)).contains("NULL::bigint"));
        assert!(subscriptions_sql(Caps::new(150000)).contains("st.apply_error_count"));
    }

    #[test]
    fn parallel_apply_workers_are_left_out_of_subscriptions_on_16_and_newer() {
        // Параллельные воркеры применения (16+) — строки с `relid IS NULL` и заданным `leader_pid`: не считаем их.
        for v in [160000, 170000, 180000] {
            let sql = subscriptions_sql(Caps::new(v));
            assert!(sql.contains("s.leader_pid IS NULL"), "{v}");
            assert!(sql.contains("s.relid IS NULL"), "{v}");
        }
        // До 16 колонки `leader_pid` нет — условие сломало бы запрос.
        for v in [130000, 140000, 150000] {
            assert!(
                !subscriptions_sql(Caps::new(v)).contains("leader_pid"),
                "{v}"
            );
        }
    }
}
