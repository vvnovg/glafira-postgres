//! Удаление слота репликации (спец. P2 §8.4).

use std::time::Instant;

use pg_advisor::rules::slots::Slot;
use serde::{Deserialize, Serialize};
use tokio_postgres::error::SqlState;

use crate::actions::Confirmation;
use crate::actions::journal::{self, ActionRecord, RecordConnection, SlotRecord};
use crate::conn::profile::{ConnectionKind, Environment};
use crate::diag::caps::Caps;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::guard::{ensure_mutation_allowed, ensure_name_confirmed};
use crate::state::AppState;

/// Только неактивный слот: активный за время между предпросмотром и удалением сюда не попадёт.
pub const DROP_SQL: &str = "SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots WHERE slot_name = $1 AND NOT active";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDropRequest {
    pub member: String,
    pub slot: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotDropPreview {
    pub member: String,
    /// Слот с отметкой Patroni (`patroni: member | permanent`): интерфейс предупреждает, что Patroni его пересоздаст.
    pub slot: Slot,
}

/// Итог действия без подробностей: записан ли журнал.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub journal_written: bool,
}

pub fn slot_record(s: &Slot) -> SlotRecord {
    SlotRecord {
        name: s.name.clone(),
        slot_type: s.slot_type.clone(),
        plugin: s.plugin.clone(),
        database: s.database.clone(),
        retained_bytes: s.retained_bytes,
        wal_status: s.wal_status.clone(),
    }
}

/// Свежие данные слота на узле `member` с отметкой Patroni (как во вкладке «Слоты»).
pub async fn preview(st: &AppState, id: &str, req: &SlotDropRequest) -> AppResult<SlotDropPreview> {
    let target = st.runtimes.diag_target(id, Some(&req.member), false)?;
    let mut data = crate::diag::on_member(st, id, &target, |c| {
        Box::pin(async move {
            let h = crate::diag::head(c).await?;
            Ok(crate::diag::slots::collect(c, Caps::new(h.version_num))
                .await?
                .0)
        })
    })
    .await?;
    if target.profile.kind == ConnectionKind::Patroni {
        let members: Vec<String> = target
            .topology
            .members
            .iter()
            .map(|m| m.name.clone())
            .collect();
        let config = crate::diag::patroni::dyn_config(st, &target).await;
        crate::diag::slots::mark_by_config(&mut data, id, &members, config);
    }
    let slot = data
        .slots
        .into_iter()
        .find(|s| s.name == req.slot)
        .ok_or_else(|| {
            app_err!(
                ErrorCode::NotFound,
                "errors.actions.slotNotFound",
                "Slot {{slot}} is not on node {{member}}",
                slot = req.slot,
                member = req.member
            )
        })?;
    Ok(SlotDropPreview {
        member: req.member.clone(),
        slot,
    })
}

/// Как действие понимает отказ сервера на `DROP_SQL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DropFailure {
    /// 55006 (`object_in_use`): слот стал активным между фильтром `NOT active` и самим удалением. Ничего не
    /// произошло, как и при пустом ответе: журнал не пишется.
    ActiveOrGone,
    /// 42501: у роли нет атрибута `REPLICATION` и она не суперпользователь.
    NotAuthorized,
    Other,
}

fn classify_drop_error(code: Option<&SqlState>) -> DropFailure {
    match code {
        Some(c) if *c == SqlState::OBJECT_IN_USE => DropFailure::ActiveOrGone,
        Some(c) if *c == SqlState::INSUFFICIENT_PRIVILEGE => DropFailure::NotAuthorized,
        _ => DropFailure::Other,
    }
}

/// Слот не удалён, потому что стал активным или исчез: пустой ответ и 55006 дают одну и ту же ошибку.
fn active_or_gone() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.actions.slotActiveOrGone",
        "The slot became active or is gone"
    )
}

/// Удаляет неактивный слот. Подтверждение — имя слота всегда, имя подключения — на prod.
pub async fn drop(
    st: &AppState,
    id: &str,
    req: &SlotDropRequest,
    confirmation: &Confirmation,
) -> AppResult<ActionResult> {
    let profile = st.profile(id)?;
    ensure_mutation_allowed(&profile, confirmation.confirm.as_deref())?;
    ensure_name_confirmed(&req.slot, confirmation.typed_name.as_deref())?;
    let started = Instant::now();
    let slot = preview(st, id, req).await?.slot;
    // Логический слот удаляется из соединения с его БД.
    let database = if slot.slot_type == "logical" {
        slot.database.as_deref()
    } else {
        None
    };
    let c = crate::actions::conn::open(st, id, &req.member, database).await?;
    let result = c.client.query(DROP_SQL, &[&req.slot]).await;
    let (outcome, code, ret) = match result {
        // Ничего не произошло: журнал не пишется.
        Ok(rows) if rows.is_empty() => return Err(active_or_gone()),
        Ok(_) => (journal::Outcome::Ok, None, Ok(())),
        Err(e) => match classify_drop_error(e.code()) {
            DropFailure::ActiveOrGone => return Err(active_or_gone()),
            DropFailure::NotAuthorized => {
                let err = app_err!(
                    ErrorCode::NotAuthorized,
                    "errors.actions.replicationRequired",
                    "Dropping a slot needs the REPLICATION attribute or superuser"
                )
                .with_details(crate::conn::errors::chain(&e));
                (
                    journal::Outcome::Error,
                    Some(journal::code_name(err.code)),
                    Err(err),
                )
            }
            DropFailure::Other => {
                let err = crate::conn::errors::from_pg(&e);
                (
                    journal::Outcome::Error,
                    Some(journal::code_name(err.code)),
                    Err(err),
                )
            }
        },
    };
    let record = ActionRecord {
        ts: journal::now_ms(),
        action: "dropSlot".into(),
        connection: RecordConnection::of(&profile),
        member: Some(req.member.clone()),
        prod_confirmed: profile.environment == Environment::Prod,
        outcome,
        code,
        ms: started.elapsed().as_millis() as u64,
        sessions: vec![],
        slot: Some(slot_record(&slot)),
        patroni: None,
    };
    let journal = st.actions.clone();
    let journal_written = crate::conn::run_blocking(move || Ok(journal.append(&record)))
        .await
        .unwrap_or(false);
    ret.map(|()| ActionResult { journal_written })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_sql_drops_only_an_inactive_slot() {
        assert!(DROP_SQL.contains("pg_drop_replication_slot(slot_name)"));
        assert!(DROP_SQL.contains("WHERE slot_name = $1 AND NOT active"));
    }

    #[test]
    fn slot_record_carries_what_was_dropped() {
        let s = Slot {
            name: "it_dead_slot".into(),
            slot_type: "physical".into(),
            retained_bytes: Some(4096),
            wal_status: Some("reserved".into()),
            ..Default::default()
        };
        let r = slot_record(&s);
        assert_eq!(
            (r.name.as_str(), r.slot_type.as_str(), r.retained_bytes),
            ("it_dead_slot", "physical", Some(4096))
        );
    }

    #[test]
    fn busy_slot_error_is_the_same_as_an_empty_answer() {
        // 55006: слот стал активным между фильтром `NOT active` и самим удалением — ничего не произошло.
        assert_eq!(
            classify_drop_error(Some(&SqlState::OBJECT_IN_USE)),
            DropFailure::ActiveOrGone
        );
        assert_eq!(SqlState::OBJECT_IN_USE.code(), "55006");
        assert_eq!(
            classify_drop_error(Some(&SqlState::INSUFFICIENT_PRIVILEGE)),
            DropFailure::NotAuthorized
        );
        for other in [
            Some(&SqlState::SYNTAX_ERROR),
            Some(&SqlState::QUERY_CANCELED),
            None,
        ] {
            assert_eq!(classify_drop_error(other), DropFailure::Other, "{other:?}");
        }
        let e = active_or_gone();
        assert_eq!(e.code, ErrorCode::InvalidInput);
        assert_eq!(e.key, "errors.actions.slotActiveOrGone");
        assert_eq!(e.message, "The slot became active or is gone");
    }
}
