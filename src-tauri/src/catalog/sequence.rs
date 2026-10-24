//! Вкладка последовательности (спец. P1 §9): тип, параметры, последнее значение, доля использованного
//! диапазона, владеющая колонка.

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::catalog::{object_not_found, relation_oid};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SequenceDetails {
    pub oid: i64,
    pub schema: String,
    pub name: String,
    /// `smallint`, `integer` или `bigint`.
    pub data_type: String,
    pub start: i64,
    pub increment: i64,
    pub min: i64,
    pub max: i64,
    pub cycle: bool,
    pub cache: i64,
    /// Последнее выданное значение; `None` — последовательность ещё не использовалась или у роли нет права
    /// SELECT/USAGE на неё (так устроен `pg_sequences`).
    pub last_value: Option<i64>,
    pub percent_used: Option<f64>,
    /// Колонка-владелец (`OWNED BY`, serial, identity): `schema.table.column`.
    pub owned_by: Option<String>,
}

/// Строка последовательности (`$1` — OID); не последовательность — строки нет. `last_value` берётся из
/// `pg_sequences`: без права на последовательность там NULL, а не ошибка, как у `pg_sequence_last_value`.
const SQL: &str = "SELECT n.nspname::text, c.relname::text, COALESCE(format_type(s.seqtypid, NULL), ''), \
    s.seqstart, s.seqincrement, s.seqmin, s.seqmax, s.seqcycle, s.seqcache, \
    (SELECT ps.last_value FROM pg_sequences ps WHERE ps.schemaname = n.nspname AND ps.sequencename = c.relname), \
    (SELECT quote_ident(tn.nspname) || '.' || quote_ident(t.relname) || '.' || quote_ident(a.attname) \
       FROM pg_depend d JOIN pg_class t ON t.oid = d.refobjid JOIN pg_namespace tn ON tn.oid = t.relnamespace \
       JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
       WHERE d.objid = c.oid AND d.classid = 'pg_class'::regclass AND d.refclassid = 'pg_class'::regclass \
         AND d.deptype IN ('a', 'i') LIMIT 1) \
  FROM pg_sequence s JOIN pg_class c ON c.oid = s.seqrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
  WHERE s.seqrelid = $1";

/// Доля использованного диапазона: для растущей — от min, для убывающей — от max; пустой диапазон — None.
pub fn percent_used(last: Option<i64>, min: i64, max: i64, increment: i64) -> Option<f64> {
    let last = last?;
    let span = (max as f64) - (min as f64);
    if span <= 0.0 {
        return None;
    }
    let used = if increment >= 0 {
        last as f64 - min as f64
    } else {
        max as f64 - last as f64
    };
    Some((used / span * 100.0).clamp(0.0, 100.0))
}

pub async fn get(c: &Client, schema: &str, name: &str) -> AppResult<SequenceDetails> {
    // OID из каталога всегда помещается в `u32`.
    let oid = relation_oid(c, schema, name).await? as u32;
    let r = c
        .query_opt(SQL, &[&oid])
        .await
        .map_err(|e| from_pg(&e))?
        .ok_or_else(|| object_not_found(schema, name))?;
    let (increment, min, max, last_value) = (r.get(4), r.get(5), r.get(6), r.get(9));
    Ok(SequenceDetails {
        oid: i64::from(oid),
        schema: r.get(0),
        name: r.get(1),
        data_type: r.get(2),
        start: r.get(3),
        increment,
        min,
        max,
        cycle: r.get(7),
        cache: r.get(8),
        last_value,
        percent_used: percent_used(last_value, min, max, increment),
        owned_by: r.get(10),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_used_counts_from_the_start_in_the_direction_of_the_increment() {
        // 49 / 99 · 100 в f64 — 49.494949494949495 (ближайшее к 49,(49) представимое число).
        assert_eq!(percent_used(Some(50), 1, 100, 1), Some(49.494949494949495));
        assert_eq!(percent_used(Some(100), 1, 100, 1), Some(100.0));
        assert_eq!(percent_used(Some(1), 1, 100, 1), Some(0.0));
        // Убывающая: использовано расстояние от max.
        assert_eq!(
            percent_used(Some(-50), -100, -1, -1),
            Some(49.494949494949495)
        );
        assert_eq!(percent_used(None, 1, 100, 1), None);
        assert_eq!(percent_used(Some(5), 5, 5, 1), None, "пустой диапазон");
    }
}
