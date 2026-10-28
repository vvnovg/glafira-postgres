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
    // Числа `bigint` уходят в интерфейс строками: JSON-число в JavaScript точно только до 2^53, а граница
    // `bigint` (9223372036854775807) выше.
    #[serde(with = "pg_advisor::serde_util::i64_string")]
    pub start: i64,
    #[serde(with = "pg_advisor::serde_util::i64_string")]
    pub increment: i64,
    #[serde(with = "pg_advisor::serde_util::i64_string")]
    pub min: i64,
    #[serde(with = "pg_advisor::serde_util::i64_string")]
    pub max: i64,
    pub cycle: bool,
    #[serde(with = "pg_advisor::serde_util::i64_string")]
    pub cache: i64,
    /// Последнее выданное значение; `None` — последовательность ещё не использовалась или у роли нет права
    /// SELECT/USAGE на неё (так устроен `pg_sequences`).
    #[serde(with = "pg_advisor::serde_util::opt_i64_string", default)]
    pub last_value: Option<i64>,
    /// У роли есть право SELECT или USAGE на последовательность (предикат `pg_sequences` для `last_value`).
    /// Нет права — `last_value` и `percent_used` пусты при любом использовании; вкладка отличает это от
    /// «ещё не использовалась» (у роли `pg_monitor` без явных прав `readable` — `false` у всех).
    pub readable: bool,
    pub percent_used: Option<f64>,
    /// Колонка-владелец (`OWNED BY`, serial, identity): `schema.table.column`.
    pub owned_by: Option<String>,
}

/// Строка последовательности (`$1` — OID); не последовательность — строки нет. `last_value` берётся из
/// `pg_sequences`: без права на последовательность там NULL, а не ошибка, как у `pg_sequence_last_value`.
/// `readable` — тот же предикат, что у `pg_sequences` (`COALESCE`: для исчезнувшей последовательности
/// `has_sequence_privilege` вернул бы NULL).
const SQL: &str = "SELECT n.nspname::text, c.relname::text, COALESCE(format_type(s.seqtypid, NULL), ''), \
    s.seqstart, s.seqincrement, s.seqmin, s.seqmax, s.seqcycle, s.seqcache, \
    (SELECT ps.last_value FROM pg_sequences ps WHERE ps.schemaname = n.nspname AND ps.sequencename = c.relname), \
    (SELECT quote_ident(tn.nspname) || '.' || quote_ident(t.relname) || '.' || quote_ident(a.attname) \
       FROM pg_depend d JOIN pg_class t ON t.oid = d.refobjid JOIN pg_namespace tn ON tn.oid = t.relnamespace \
       JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid \
       WHERE d.objid = c.oid AND d.classid = 'pg_class'::regclass AND d.refclassid = 'pg_class'::regclass \
         AND d.deptype IN ('a', 'i') LIMIT 1), \
    COALESCE(has_sequence_privilege(c.oid, 'SELECT,USAGE'), false) \
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
        readable: r.get(11),
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

    fn details(max: i64, last_value: Option<i64>) -> SequenceDetails {
        SequenceDetails {
            oid: 16_400,
            schema: "shop".into(),
            name: "big_seq".into(),
            data_type: "bigint".into(),
            start: 1,
            increment: -2,
            min: i64::MIN,
            max,
            cycle: false,
            cache: 1,
            last_value,
            readable: true,
            percent_used: Some(0.5),
            owned_by: None,
        }
    }

    #[test]
    fn bigint_fields_go_to_the_ui_as_strings_without_losing_digits() {
        // JSON-число в JavaScript точно только до 2^53: границы bigint уходят строками, `oid` и доля — числами.
        let json = serde_json::to_value(details(i64::MAX, None)).unwrap();
        assert_eq!(json["max"], "9223372036854775807");
        assert_eq!(json["min"], "-9223372036854775808");
        assert_eq!(json["start"], "1");
        assert_eq!(json["increment"], "-2");
        assert_eq!(json["cache"], "1");
        assert_eq!(json["lastValue"], serde_json::Value::Null);
        assert_eq!(json["oid"], 16_400);
        assert_eq!(json["percentUsed"], 0.5);
        let text = serde_json::to_string(&details(i64::MAX, Some(i64::MAX - 1))).unwrap();
        assert!(text.contains(r#""max":"9223372036854775807""#), "{text}");
        assert!(
            text.contains(r#""lastValue":"9223372036854775806""#),
            "{text}"
        );
    }

    #[test]
    fn details_survive_a_json_round_trip() {
        let d = details(i64::MAX, Some(i64::MIN));
        let back: SequenceDetails =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
    }
}
