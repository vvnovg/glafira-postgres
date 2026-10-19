use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::catalog::{check_page, like_pattern};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ObjectKind {
    Tables,
    Views,
    Matviews,
    Functions,
    Sequences,
}

impl ObjectKind {
    pub fn relkinds(self) -> Option<&'static [&'static str]> {
        match self {
            ObjectKind::Tables => Some(&["r", "p", "f"]),
            ObjectKind::Views => Some(&["v"]),
            ObjectKind::Matviews => Some(&["m"]),
            ObjectKind::Sequences => Some(&["S"]),
            ObjectKind::Functions => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectItem {
    pub oid: i64,
    pub name: String,
    pub schema: String,
    pub relkind: Option<String>,
    /// Для партиционированной таблицы — число партиций.
    pub partitions: Option<i64>,
    pub est_rows: Option<i64>,
    pub total_bytes: Option<i64>,
    /// Аргументы функции (`pg_get_function_identity_arguments`).
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPage {
    pub total: i64,
    pub items: Vec<ObjectItem>,
    pub next_offset: Option<i64>,
}

pub(crate) fn next_offset(offset: i64, got: usize, total: i64) -> Option<i64> {
    let end = offset + got as i64;
    (got > 0 && end < total).then_some(end)
}

const REL_COLUMNS: &str = "c.oid::bigint, c.relname::text, n.nspname::text, c.relkind::text, \
    CASE WHEN c.relkind = 'p' THEN (SELECT count(*) FROM pg_inherits i WHERE i.inhparent = c.oid) END, \
    CASE WHEN c.relkind IN ('r', 'p', 'm') AND c.reltuples >= 0 THEN c.reltuples::bigint END, \
    CASE WHEN c.relkind IN ('r', 'p', 'm') THEN pg_total_relation_size(c.oid) END, \
    count(*) OVER ()";

fn rel_item(r: &tokio_postgres::Row) -> ObjectItem {
    ObjectItem {
        oid: r.get(0),
        name: r.get(1),
        schema: r.get(2),
        relkind: r.get(3),
        partitions: r.get(4),
        est_rows: r.get(5),
        total_bytes: r.get(6),
        signature: None,
    }
}

pub async fn list(
    c: &Client,
    schema: &str,
    kind: ObjectKind,
    filter: Option<&str>,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    let pattern = like_pattern(filter);
    let (items, total): (Vec<ObjectItem>, i64) = match kind.relkinds() {
        Some(relkinds) => {
            let sql = format!(
                "SELECT {REL_COLUMNS} FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE n.nspname = $1 AND c.relkind::text = ANY($2) AND NOT c.relispartition \
                   AND ($3::text IS NULL OR c.relname ILIKE $3) \
                 ORDER BY c.relname LIMIT $4 OFFSET $5"
            );
            let rows = c
                .query(&sql, &[&schema, &relkinds, &pattern, &limit, &offset])
                .await
                .map_err(|e| from_pg(&e))?;
            let total = rows.first().map(|r| r.get::<_, i64>(7)).unwrap_or(0);
            (rows.iter().map(rel_item).collect(), total)
        }
        None => {
            let rows = c
                .query(
                    "SELECT p.oid::bigint, p.proname::text, n.nspname::text, p.prokind::text, \
                       pg_get_function_identity_arguments(p.oid), count(*) OVER () \
                     FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 AND p.prokind IN ('f', 'p') AND ($2::text IS NULL OR p.proname ILIKE $2) \
                     ORDER BY p.proname, 5 LIMIT $3 OFFSET $4",
                    &[&schema, &pattern, &limit, &offset],
                )
                .await
                .map_err(|e| from_pg(&e))?;
            let total = rows.first().map(|r| r.get::<_, i64>(5)).unwrap_or(0);
            let items = rows
                .iter()
                .map(|r| ObjectItem {
                    oid: r.get(0),
                    name: r.get(1),
                    schema: r.get(2),
                    relkind: r.get(3),
                    partitions: None,
                    est_rows: None,
                    total_bytes: None,
                    signature: r.get(4),
                })
                .collect();
            (items, total)
        }
    };
    let next_offset = next_offset(offset, items.len(), total);
    Ok(ObjectPage {
        total,
        items,
        next_offset,
    })
}

/// Партиции таблицы (могут лежать в другой схеме — поле `schema`).
pub async fn partitions(
    c: &Client,
    schema: &str,
    table: &str,
    offset: i64,
    limit: i64,
) -> AppResult<ObjectPage> {
    check_page(offset, limit)?;
    let sql = format!(
        "SELECT {REL_COLUMNS} FROM pg_inherits inh \
         JOIN pg_class c ON c.oid = inh.inhrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE inh.inhparent = (SELECT pc.oid FROM pg_class pc JOIN pg_namespace pn ON pn.oid = pc.relnamespace \
                                WHERE pn.nspname = $1 AND pc.relname = $2) \
         ORDER BY c.relname LIMIT $3 OFFSET $4"
    );
    let rows = c
        .query(&sql, &[&schema, &table, &limit, &offset])
        .await
        .map_err(|e| from_pg(&e))?;
    let total = rows.first().map(|r| r.get::<_, i64>(7)).unwrap_or(0);
    let items: Vec<ObjectItem> = rows.iter().map(rel_item).collect();
    let next_offset = next_offset(offset, items.len(), total);
    Ok(ObjectPage {
        total,
        items,
        next_offset,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_offset_only_when_more_rows() {
        assert_eq!(next_offset(0, 2, 3), Some(2));
        assert_eq!(next_offset(2, 1, 3), None);
        assert_eq!(next_offset(0, 0, 0), None);
    }

    #[test]
    fn kinds_have_relkinds() {
        assert_eq!(ObjectKind::Tables.relkinds(), Some(&["r", "p", "f"][..]));
        assert_eq!(ObjectKind::Sequences.relkinds(), Some(&["S"][..]));
        assert_eq!(ObjectKind::Functions.relkinds(), None);
        let k: ObjectKind = serde_json::from_str("\"matviews\"").unwrap();
        assert_eq!(k, ObjectKind::Matviews);
    }
}
