//! Горизонт xmin (спец. P2 §5.5): сессии, prepared transactions, слоты и обратная связь реплик, отсортированные по
//! возрасту xmin. Четыре небольших запроса и настройка: каждый видит свою часть, а общую форму держателя собирает код.

use pg_advisor::rules::horizon::{self as rules, Holder, HolderKind, HorizonData};
use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::diag::caps::{Caps, V17};
use crate::diag::{
    DiagData, DiagReport, Tool, head, make_report, on_member, sender_member, sql::BACKEND_START_US,
};
use crate::error::AppResult;
use crate::state::AppState;
use crate::topology::model::Topology;
use crate::topology::runtime::DiagTarget;

/// Сессии с `backend_xmin` или назначенным XID, кроме самой диагностики и отправителей WAL (их xmin — обратная связь
/// реплики, она читается из `pg_stat_replication`). Колонки: 0 pid, 1 `backend_start`, 2 пользователь, 3 приложение,
/// 4 БД, 5 состояние, 6 возраст xmin или XID (больший), 7 возраст транзакции в секундах.
pub fn sessions_sql() -> String {
    format!(
        "SELECT a.pid, {BACKEND_START_US}, a.usename::text, a.application_name, a.datname::text, a.state, \
                GREATEST(age(a.backend_xmin), age(a.backend_xid))::bigint, extract(epoch FROM now() - a.xact_start)::float8 \
         FROM pg_stat_activity a \
         WHERE (a.backend_xmin IS NOT NULL OR a.backend_xid IS NOT NULL) AND a.pid <> pg_backend_pid() \
           AND a.backend_type IS DISTINCT FROM 'walsender'"
    )
}

/// Подготовленные транзакции: 0 gid, 1 владелец, 2 БД, 3 возраст XID, 4 сколько подготовлена, секунд.
pub const PREPARED_SQL: &str = "SELECT gid, owner::text, database::text, age(transaction)::bigint, \
    extract(epoch FROM now() - prepared)::float8 FROM pg_prepared_xacts";

/// Слоты с xmin или `catalog_xmin`: 0 имя, 1 вид, 2 активен, 3 возраст xmin, 4 возраст `catalog_xmin`, 5 сколько
/// неактивен, секунд (`inactive_since` — PostgreSQL 17+).
pub fn slots_sql(caps: Caps) -> String {
    let inactive = caps.col(
        V17,
        "extract(epoch FROM now() - inactive_since)::float8",
        "float8",
    );
    format!(
        "SELECT slot_name::text, slot_type, active, age(xmin)::bigint, age(catalog_xmin)::bigint, {inactive} \
         FROM pg_replication_slots WHERE xmin IS NOT NULL OR catalog_xmin IS NOT NULL"
    )
}

/// Обратная связь реплик без слота (с физическим слотом xmin лежит в самом слоте): 0 `application_name`,
/// 1 адрес клиента, 2 возраст xmin.
pub const FEEDBACK_SQL: &str = "SELECT application_name, host(client_addr), age(backend_xmin)::bigint \
    FROM pg_stat_replication WHERE backend_xmin IS NOT NULL";

pub const FEEDBACK_SETTING_SQL: &str =
    "SELECT setting = 'on' FROM pg_settings WHERE name = 'hot_standby_feedback'";

/// Слот: возраст — больший из `xmin` и `catalog_xmin`; горизонт каталога — когда `catalog_xmin` старше `xmin`
/// (или `xmin` пуст).
fn slot_holder(
    name: String,
    slot_type: Option<String>,
    active: Option<bool>,
    xmin_age: Option<i64>,
    catalog_xmin_age: Option<i64>,
    inactive_s: Option<f64>,
) -> Holder {
    let catalog = match (xmin_age, catalog_xmin_age) {
        (Some(x), Some(c)) => c > x,
        (None, Some(_)) => true,
        _ => false,
    };
    Holder {
        kind: HolderKind::Slot,
        name,
        xmin_age: xmin_age.max(catalog_xmin_age).unwrap_or(0),
        duration_s: inactive_s,
        slot_type,
        active,
        catalog,
        ..Default::default()
    }
}

/// Обратная связь реплики без слота; реплика сопоставляется с участником топологии по `application_name`.
fn feedback_holder(
    application: String,
    client_addr: Option<String>,
    xmin_age: i64,
    topology: &Topology,
) -> Holder {
    Holder {
        kind: HolderKind::StandbyFeedback,
        member: sender_member(topology, Some(&application)),
        name: application,
        xmin_age,
        client_addr,
        ..Default::default()
    }
}

pub async fn collect(c: &Client, caps: Caps, topology: &Topology) -> AppResult<HorizonData> {
    let pg = |e: tokio_postgres::Error| from_pg(&e);
    let mut holders = Vec::new();
    for r in c.query(&sessions_sql(), &[]).await.map_err(pg)? {
        let pid: i32 = r.get(0);
        holders.push(Holder {
            kind: HolderKind::Session,
            name: pid.to_string(),
            xmin_age: r.get::<_, Option<i64>>(6).unwrap_or(0),
            duration_s: r.get(7),
            pid: Some(pid),
            backend_start_us: r.get(1),
            user: r.get(2),
            application: r.get(3),
            database: r.get(4),
            state: r.get(5),
            ..Default::default()
        });
    }
    for r in c.query(PREPARED_SQL, &[]).await.map_err(pg)? {
        let gid: String = r.get(0);
        holders.push(Holder {
            kind: HolderKind::PreparedXact,
            name: gid.clone(),
            gid: Some(gid),
            user: r.get(1),
            database: r.get(2),
            xmin_age: r.get::<_, Option<i64>>(3).unwrap_or(0),
            duration_s: r.get(4),
            ..Default::default()
        });
    }
    for r in c.query(&slots_sql(caps), &[]).await.map_err(pg)? {
        holders.push(slot_holder(
            r.get(0),
            r.get(1),
            r.get(2),
            r.get(3),
            r.get(4),
            r.get(5),
        ));
    }
    for r in c.query(FEEDBACK_SQL, &[]).await.map_err(pg)? {
        holders.push(feedback_holder(
            r.get::<_, Option<String>>(0).unwrap_or_default(),
            r.get(1),
            r.get::<_, Option<i64>>(2).unwrap_or(0),
            topology,
        ));
    }
    // Стабильная сортировка: при равном возрасте остаётся порядок источников (сессии, prepared, слоты, обратная связь).
    holders.sort_by_key(|h| std::cmp::Reverse(h.xmin_age));
    let hot_standby_feedback = c
        .query_opt(FEEDBACK_SETTING_SQL, &[])
        .await
        .map_err(pg)?
        .and_then(|r| r.get::<_, Option<bool>>(0));
    Ok(HorizonData {
        holders,
        hot_standby_feedback,
    })
}

pub async fn report(st: &AppState, id: &str, target: &DiagTarget) -> AppResult<DiagReport> {
    let topology = target.topology.clone();
    let (h, data) = on_member(st, id, target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let data = collect(c, Caps::new(h.version_num), &topology).await?;
            Ok((h, data))
        })
    })
    .await?;
    let member = target.member.as_ref().map(|m| m.name.as_str());
    let findings = rules::evaluate(member, &data);
    let unavailable = Caps::new(h.version_num)
        .missing(V17, "slotInactiveSince")
        .into_iter()
        .collect();
    Ok(make_report(
        Tool::Horizon,
        target,
        Some(&h),
        DiagData::Horizon(data),
        findings,
        unavailable,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::model::{Member, Role, topo};

    #[test]
    fn inactive_since_exists_only_since_17() {
        assert!(slots_sql(Caps::new(160004)).contains("NULL::float8"));
        assert!(!slots_sql(Caps::new(160004)).contains("inactive_since"));
        assert!(slots_sql(Caps::new(170000)).contains("now() - inactive_since"));
    }

    #[test]
    fn sessions_sql_skips_the_collector_and_walsenders() {
        let sql = sessions_sql();
        assert!(sql.contains("a.pid <> pg_backend_pid()"));
        assert!(sql.contains("IS DISTINCT FROM 'walsender'"));
        assert!(sql.contains(BACKEND_START_US));
    }

    #[test]
    fn slot_horizon_is_the_older_of_data_and_catalog_and_flags_the_catalog() {
        let h = |x, c| slot_holder("s".into(), None, None, x, c, None);
        assert_eq!(
            (h(Some(5), None).xmin_age, h(Some(5), None).catalog),
            (5, false)
        );
        assert_eq!(
            (h(None, Some(9)).xmin_age, h(None, Some(9)).catalog),
            (9, true)
        );
        assert_eq!(
            (h(Some(5), Some(9)).xmin_age, h(Some(5), Some(9)).catalog),
            (9, true)
        );
        assert_eq!(
            (h(Some(9), Some(5)).xmin_age, h(Some(9), Some(5)).catalog),
            (9, false)
        );
        assert!(!h(Some(7), Some(7)).catalog);
        let s = slot_holder(
            "s".into(),
            Some("physical".into()),
            Some(false),
            Some(1),
            None,
            Some(30.0),
        );
        assert_eq!(
            (s.kind, s.slot_type.as_deref(), s.active, s.duration_s),
            (HolderKind::Slot, Some("physical"), Some(false), Some(30.0))
        );
    }

    #[test]
    fn feedback_is_matched_to_a_member_by_name_or_host() {
        let mut m2 = Member::new("pg-2", "db2.example", 5432);
        m2.role = Role::Replica;
        let t = topo(vec![Member::new("pg-1", "db1.example", 5432), m2]);
        let by_name = feedback_holder("pg-2".into(), Some("10.0.0.2".into()), 3, &t);
        assert_eq!(
            (
                by_name.kind,
                by_name.member.as_deref(),
                by_name.name.as_str(),
                by_name.client_addr.as_deref()
            ),
            (
                HolderKind::StandbyFeedback,
                Some("pg-2"),
                "pg-2",
                Some("10.0.0.2")
            )
        );
        assert_eq!(
            feedback_holder("db2.example".into(), None, 3, &t)
                .member
                .as_deref(),
            Some("pg-2")
        );
        assert_eq!(
            feedback_holder("walreceiver".into(), None, 3, &t).member,
            None
        );
        assert_eq!(
            feedback_holder("pg-2".into(), None, 3, &Topology::empty_for_tests()).member,
            None
        );
    }
}
