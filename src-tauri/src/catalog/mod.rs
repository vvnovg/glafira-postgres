//! Запросы каталога для дерева (спец. P1 §8) и вкладок объектов (§9). Все — на узле каталога, текст
//! рассчитан на PostgreSQL 13+; столбцы новых версий выбираются по [`server_version_num`].

pub mod databases;
pub mod ddl;
pub mod extensions;
pub mod function;
pub mod objects;
pub mod roles;
pub mod schemas;
pub mod sequence;
pub mod table;
pub mod view;

use tokio_postgres::Client;

use crate::conn::errors::from_pg;
use crate::error::{AppError, AppResult, ErrorCode};

pub const MAX_PAGE: i64 = 1000;

/// Экранирует `\`, `%`, `_` для `ILIKE` (экранирующий символ — `\` по умолчанию).
pub fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Подстрока фильтра → шаблон `%…%`; пустой фильтр — без ограничения.
pub fn like_pattern(filter: Option<&str>) -> Option<String> {
    filter
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| format!("%{}%", escape_like(f)))
}

/// Версия сервера для выбора текста запроса (`last_idx_scan` — 16+ и т. п.).
pub async fn server_version_num(c: &Client) -> AppResult<i32> {
    let r = c
        .query_one("SELECT current_setting('server_version_num')::int", &[])
        .await
        .map_err(|e| from_pg(&e))?;
    Ok(r.get(0))
}

/// Объекта нет (удалён или переименован после того, как его показало дерево).
pub(crate) fn object_not_found(schema: &str, name: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.catalog.objectNotFound",
        "Object {{schema}}.{{name}} was not found (dropped or renamed?)",
        schema = schema,
        name = name
    )
}

/// OID отношения по схеме и имени; нет — `objectNotFound`. Только каталог: приведение `'s.n'::regclass`
/// потребовало бы права USAGE на схему, а у роли диагностики его обычно нет.
pub async fn relation_oid(c: &Client, schema: &str, name: &str) -> AppResult<i64> {
    let row = c
        .query_opt(
            "SELECT c.oid::bigint FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relname = $2",
            &[&schema, &name],
        )
        .await
        .map_err(|e| from_pg(&e))?;
    row.map(|r| r.get(0))
        .ok_or_else(|| object_not_found(schema, name))
}

pub(crate) fn check_page(offset: i64, limit: i64) -> AppResult<()> {
    if offset < 0 || !(1..=MAX_PAGE).contains(&limit) {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.catalog.badPage",
            "Invalid page: offset {{offset}}, limit {{limit}}",
            offset = offset,
            limit = limit
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escaping() {
        assert_eq!(escape_like(r"a_b%c\d"), r"a\_b\%c\\d");
        assert_eq!(like_pattern(Some(" ord ")).as_deref(), Some("%ord%"));
        assert_eq!(like_pattern(Some("  ")), None);
        assert_eq!(like_pattern(None), None);
    }

    #[test]
    fn index_query_mentions_last_idx_scan_only_on_16_plus() {
        assert!(table::index_sql(160000).contains("last_idx_scan"));
        assert!(!table::index_sql(150004).contains("last_idx_scan"));
    }

    #[test]
    fn pages() {
        assert!(check_page(0, 500).is_ok());
        assert!(check_page(-1, 10).is_err());
        assert!(check_page(0, 0).is_err());
        assert!(check_page(0, MAX_PAGE + 1).is_err());
    }
}
