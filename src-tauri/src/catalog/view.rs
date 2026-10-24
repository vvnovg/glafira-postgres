//! Вкладка представления и материализованного представления (спец. P1 §9): определение, колонки, у
//! материализованного — заполненность, размер и индексы.

use serde::{Deserialize, Serialize};
use tokio_postgres::Client;

use crate::catalog::table::{COLUMNS_SQL, Column, Index, column_from, index_from, index_sql, rows};
use crate::catalog::{object_not_found, relation_oid, server_version_num};
use crate::conn::errors::from_pg;
use crate::error::AppResult;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewDetails {
    pub oid: i64,
    pub schema: String,
    pub name: String,
    pub matview: bool,
    pub owner: String,
    pub comment: Option<String>,
    /// Запрос представления (`pg_get_viewdef`, с отступами).
    pub definition: String,
    pub columns: Vec<Column>,
    /// Заполнено ли материализованное представление (`REFRESH` был); у обычного — `None`.
    pub populated: Option<bool>,
    /// Полный размер материализованного представления; у обычного — `None`.
    pub size_bytes: Option<i64>,
    /// Индексы материализованного представления; у обычного — пусто.
    pub indexes: Vec<Index>,
}

/// Заголовок (`$1` — OID). Не представление (`v`) и не материализованное (`m`) — строки нет.
const HEAD_SQL: &str = "SELECT n.nspname::text, c.relname::text, c.relkind::text, \
    pg_get_userbyid(c.relowner)::text, obj_description(c.oid, 'pg_class'), \
    COALESCE(pg_get_viewdef(c.oid, true), ''), \
    CASE WHEN c.relkind = 'm' THEN c.relispopulated END, \
    CASE WHEN c.relkind = 'm' THEN pg_total_relation_size(c.oid) END \
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
  WHERE c.oid = $1 AND c.relkind IN ('v', 'm')";

pub async fn get(c: &Client, schema: &str, name: &str) -> AppResult<ViewDetails> {
    // OID из каталога всегда помещается в `u32`.
    let oid = relation_oid(c, schema, name).await? as u32;
    let h = c
        .query_opt(HEAD_SQL, &[&oid])
        .await
        .map_err(|e| from_pg(&e))?
        .ok_or_else(|| object_not_found(schema, name))?;
    let matview = h.get::<_, String>(2) == "m";
    let columns = rows(c, COLUMNS_SQL, oid)
        .await?
        .iter()
        .map(column_from)
        .collect();
    let indexes = if matview {
        let version = server_version_num(c).await?;
        rows(c, &index_sql(version), oid)
            .await?
            .iter()
            .map(index_from)
            .collect()
    } else {
        Vec::new()
    };
    Ok(ViewDetails {
        oid: i64::from(oid),
        schema: h.get(0),
        name: h.get(1),
        matview,
        owner: h.get(3),
        comment: h.get(4),
        definition: h.get(5),
        columns,
        populated: h.get(6),
        size_bytes: h.get(7),
        indexes,
    })
}
