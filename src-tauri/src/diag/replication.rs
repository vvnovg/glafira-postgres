//! Реплики (спец. P2 §5.6): отправители WAL с лагами (primary и каскадная реплика), приёмник, пауза replay и конфликты
//! recovery (реплика). Рост конфликтов — по двум снимкам счётчика того же узла (спец. §3.6).

use std::collections::BTreeMap;

use pg_advisor::rates::{self, CounterSnapshot};
use pg_advisor::rules::replication::{
    self as rules, Conflicts, Receiver, Replay, ReplicationData, Sender,
};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V14, V16};
use crate::diag::{
    DiagData, DiagReport, Tool, head, make_report, needs_monitor, on_member, reads_all_stats,
    sender_member,
};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::model::Topology;
use crate::topology::runtime::DiagTarget;

/// Отправители WAL. Колонки: 0 pid, 1 пользователь, 2 приложение, 3 адрес клиента, 4 состояние, 5 `sync_state`,
/// 6 приоритет, 7–10 лаги в байтах (отправлено, записано, сброшено, применено) от текущего LSN узла (на реплике —
/// полученного), 11–13 лаги записи, сброса и применения в секундах, 14 возраст ответа, 15 слот, 16 возраст xmin.
pub const SENDERS_SQL: &str = "SELECT r.pid, r.usename::text, r.application_name, host(r.client_addr), r.state, r.sync_state, \
    r.sync_priority, pg_wal_lsn_diff(c.lsn, r.sent_lsn)::bigint, pg_wal_lsn_diff(c.lsn, r.write_lsn)::bigint, \
    pg_wal_lsn_diff(c.lsn, r.flush_lsn)::bigint, pg_wal_lsn_diff(c.lsn, r.replay_lsn)::bigint, \
    extract(epoch FROM r.write_lag)::float8, extract(epoch FROM r.flush_lag)::float8, extract(epoch FROM r.replay_lag)::float8, \
    extract(epoch FROM now() - r.reply_time)::float8, \
    (SELECT s.slot_name::text FROM pg_replication_slots s WHERE s.active_pid = r.pid LIMIT 1), age(r.backend_xmin)::bigint \
  FROM pg_stat_replication r, \
    LATERAL (SELECT CASE WHEN pg_is_in_recovery() THEN pg_last_wal_receive_lsn() ELSE pg_current_wal_lsn() END AS lsn) c \
  ORDER BY r.application_name";

/// Приёмник WAL (одна строка или ни одной): 0 состояние, 1 хост, 2 порт, 3 слот, 4 таймлайн, 5–6 возраст последнего
/// сообщения и последней позиции отправителя в секундах.
pub const RECEIVER_SQL: &str = "SELECT status, sender_host, sender_port, slot_name, received_tli::bigint, \
    extract(epoch FROM now() - last_msg_receipt_time)::float8, extract(epoch FROM now() - latest_end_time)::float8 \
  FROM pg_stat_wal_receiver";

/// Применение WAL: 0 полученное, но не применённое (байты), 1 задержка применения (0, если применено всё полученное),
/// 2 состояние паузы. `pg_get_wal_replay_pause_state()` — с PostgreSQL 14; раньше только «на паузе / нет».
pub fn replay_sql(caps: Caps) -> String {
    let pause = if caps.at_least(V14) {
        "pg_get_wal_replay_pause_state()"
    } else {
        "CASE WHEN pg_is_wal_replay_paused() THEN 'paused' ELSE 'not paused' END"
    };
    format!(
        "SELECT pg_wal_lsn_diff(pg_last_wal_receive_lsn(), pg_last_wal_replay_lsn())::bigint, \
         (CASE WHEN pg_last_wal_receive_lsn() = pg_last_wal_replay_lsn() THEN 0 \
               ELSE GREATEST(extract(epoch FROM now() - pg_last_xact_replay_timestamp()), 0) END)::float8, {pause}"
    )
}

/// Конфликты recovery суммой по базам: 0–4 `tablespace`, `lock`, `snapshot`, `bufferpin`, `deadlock`, 5 `active_logicalslot`
/// (16+), 6 `now()` сервера, 7 запуск postmaster, 8 наибольший `stats_reset` (мс эпохи) — основа снимка счётчика.
pub fn conflicts_sql(caps: Caps) -> String {
    let logical = caps.col(
        V16,
        "COALESCE(sum(confl_active_logicalslot), 0)::bigint",
        "bigint",
    );
    format!(
        "SELECT COALESCE(sum(confl_tablespace), 0)::bigint, COALESCE(sum(confl_lock), 0)::bigint, \
         COALESCE(sum(confl_snapshot), 0)::bigint, COALESCE(sum(confl_bufferpin), 0)::bigint, \
         COALESCE(sum(confl_deadlock), 0)::bigint, {logical}, \
         (extract(epoch FROM now()) * 1000)::bigint, (extract(epoch FROM pg_postmaster_start_time()) * 1000)::bigint, \
         (SELECT max((extract(epoch FROM stats_reset) * 1000)::bigint) FROM pg_stat_database) \
         FROM pg_stat_database_conflicts"
    )
}

pub const SETTINGS_SQL: &str = "SELECT name::text, setting FROM pg_settings WHERE name IN \
    ('synchronous_standby_names', 'synchronous_commit', 'hot_standby_feedback', 'max_standby_streaming_delay')";

/// Ключ счётчика конфликтов в снимке.
const CONFLICTS_KEY: &str = "conflicts";

/// Значение настройки `pg_settings` — в поля данных; чужие имена игнорируются. `max_standby_streaming_delay` в
/// `pg_settings` — миллисекунды, `-1` — без предела.
fn apply_setting(d: &mut ReplicationData, name: &str, value: Option<String>) {
    match name {
        "synchronous_standby_names" => d.synchronous_standby_names = value,
        "synchronous_commit" => d.synchronous_commit = value,
        "hot_standby_feedback" => d.hot_standby_feedback = value.map(|v| v == "on"),
        "max_standby_streaming_delay" => {
            d.max_standby_streaming_delay_ms = value.and_then(|v| v.parse().ok())
        }
        _ => {}
    }
}

/// Данные узла и, на реплике, снимок счётчика конфликтов (рост считает `report`: прошлый снимок хранит `AppState`).
pub async fn collect(
    c: &Client,
    caps: Caps,
    in_recovery: bool,
    topology: &Topology,
) -> AppResult<(ReplicationData, Option<CounterSnapshot>)> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    // Право читается раз за сбор: без `pg_read_all_stats` сервер отдаёт у приёмника и отправителей пустые состояние и
    // LSN, и правила их не оценивают.
    let mut d = ReplicationData {
        in_recovery,
        details_hidden: !reads_all_stats(c).await?,
        ..Default::default()
    };
    // Отправители есть и у каскадной реплики.
    for r in c.query(SENDERS_SQL, &[]).await.map_err(pg)? {
        let application: Option<String> = r.get(2);
        d.senders.push(Sender {
            pid: r.get(0),
            user: r.get(1),
            member: sender_member(topology, application.as_deref()),
            application,
            client_addr: r.get(3),
            state: r.get(4),
            sync_state: r.get(5),
            sync_priority: r.get(6),
            sent_lag_bytes: r.get(7),
            write_lag_bytes: r.get(8),
            flush_lag_bytes: r.get(9),
            replay_lag_bytes: r.get(10),
            write_lag_s: r.get(11),
            flush_lag_s: r.get(12),
            replay_lag_s: r.get(13),
            reply_age_s: r.get(14),
            slot_name: r.get(15),
            xmin_age: r.get(16),
        });
    }
    for r in c.query(SETTINGS_SQL, &[]).await.map_err(pg)? {
        apply_setting(&mut d, &r.get::<_, String>(0), r.get(1));
    }
    if !in_recovery {
        return Ok((d, None));
    }
    // Нет строки приёмника — он не работает: `status` пуст.
    d.receiver = Some(match c.query_opt(RECEIVER_SQL, &[]).await.map_err(pg)? {
        Some(r) => Receiver {
            status: r.get(0),
            sender_host: r.get(1),
            sender_port: r.get(2),
            slot_name: r.get(3),
            received_tli: r.get(4),
            last_msg_age_s: r.get(5),
            latest_end_age_s: r.get(6),
        },
        None => Receiver::default(),
    });
    let r = c.query_one(&replay_sql(caps), &[]).await.map_err(pg)?;
    d.replay = Some(Replay {
        receive_replay_gap_bytes: r.get(0),
        replay_delay_s: r.get(1),
        pause: r.get(2),
    });
    let r = c.query_one(&conflicts_sql(caps), &[]).await.map_err(pg)?;
    let conflicts = Conflicts {
        tablespace: r.get(0),
        lock: r.get(1),
        snapshot: r.get(2),
        bufferpin: r.get(3),
        deadlock: r.get(4),
        active_logicalslot: r.get(5),
        growth: None,
    };
    let snapshot = CounterSnapshot {
        at_ms: r.get(6),
        started_ms: r.get(7),
        reset_ms: r.get(8),
        values: BTreeMap::from([(CONFLICTS_KEY.to_string(), conflicts.total())]),
    };
    d.conflicts = Some(conflicts);
    Ok((d, Some(snapshot)))
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let topology = target.topology.clone();
    let (h, mut data, snapshot) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let (data, snapshot) =
                collect(c, Caps::new(h.version_num), h.in_recovery, &topology).await?;
            Ok((h, data, snapshot))
        })
    })
    .await?;
    let member = target.member.as_ref().map(|m| m.name.as_str());
    if let (Some(member), Some(snapshot)) = (member, snapshot) {
        let prev = st.counters.swap(
            id,
            target.generation,
            member,
            Tool::Replication,
            snapshot.clone(),
        );
        if let Some(c) = data.conflicts.as_mut() {
            c.growth = rates::delta(prev.as_ref(), &snapshot, CONFLICTS_KEY);
        }
    }
    let findings = rules::evaluate(member, &data);
    let caps = Caps::new(h.version_num);
    let mut unavailable: Vec<_> = if h.in_recovery {
        [
            caps.missing(V14, "replayPauseState"),
            caps.missing(V16, "conflictActiveLogicalSlot"),
        ]
        .into_iter()
        .flatten()
        .collect()
    } else {
        Vec::new()
    };
    if data.details_hidden {
        unavailable.push(needs_monitor("replicationDetails"));
    }
    Ok(make_report(
        Tool::Replication,
        target,
        Some(&h),
        DiagData::Replication(data),
        findings,
        unavailable,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_state_and_logical_conflicts_depend_on_the_version() {
        assert!(replay_sql(Caps::new(130000)).contains("pg_is_wal_replay_paused()"));
        assert!(!replay_sql(Caps::new(130000)).contains("pg_get_wal_replay_pause_state"));
        assert!(replay_sql(Caps::new(140000)).contains("pg_get_wal_replay_pause_state()"));
        assert!(conflicts_sql(Caps::new(150000)).contains("NULL::bigint"));
        assert!(!conflicts_sql(Caps::new(150000)).contains("confl_active_logicalslot"));
        assert!(conflicts_sql(Caps::new(160000)).contains("sum(confl_active_logicalslot)"));
    }

    #[test]
    fn settings_are_parsed_by_name() {
        let mut d = ReplicationData::default();
        apply_setting(
            &mut d,
            "synchronous_standby_names",
            Some("ANY 1 (a)".into()),
        );
        apply_setting(&mut d, "synchronous_commit", Some("remote_apply".into()));
        apply_setting(&mut d, "hot_standby_feedback", Some("on".into()));
        apply_setting(&mut d, "max_standby_streaming_delay", Some("-1".into()));
        apply_setting(&mut d, "other", Some("x".into()));
        assert_eq!(d.synchronous_standby_names.as_deref(), Some("ANY 1 (a)"));
        assert_eq!(d.synchronous_commit.as_deref(), Some("remote_apply"));
        assert_eq!(
            (d.hot_standby_feedback, d.max_standby_streaming_delay_ms),
            (Some(true), Some(-1))
        );
        apply_setting(&mut d, "hot_standby_feedback", Some("off".into()));
        apply_setting(&mut d, "max_standby_streaming_delay", None);
        assert_eq!(
            (d.hot_standby_feedback, d.max_standby_streaming_delay_ms),
            (Some(false), None)
        );
    }

    #[test]
    fn senders_sql_measures_from_the_receive_position_on_a_standby() {
        assert!(SENDERS_SQL.contains("pg_last_wal_receive_lsn()"));
        assert!(SENDERS_SQL.contains("pg_current_wal_lsn()"));
        assert!(SENDERS_SQL.contains("pg_is_in_recovery()"));
    }
}
