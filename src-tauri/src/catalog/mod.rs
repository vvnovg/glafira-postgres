//! Запросы каталога для дерева (спец. P1 §8). Все — на узле каталога, текст рассчитан на PostgreSQL 13+.

pub mod databases;
pub mod extensions;
pub mod objects;
pub mod roles;
pub mod schemas;

use crate::error::{AppResult, ErrorCode};

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
    fn pages() {
        assert!(check_page(0, 500).is_ok());
        assert!(check_page(-1, 10).is_err());
        assert!(check_page(0, 0).is_err());
        assert!(check_page(0, MAX_PAGE + 1).is_err());
    }
}
