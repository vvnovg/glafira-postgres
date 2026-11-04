//! Параметры `$N`, первое слово инструкции, `search_path`. Лексер PostgreSQL переехал в `pg_advisor::sqltext` вместе с
//! тестами (спец. P4 §2.1): реэкспорт сохраняет пути вызовов P3 (`explain::params::count_params` …); проверка
//! `search_path` — правило приложения и остаётся здесь.

pub use pg_advisor::sqltext::{MAX_PARAMS, count_params, first_word, normalize};

/// `search_path` для `SET LOCAL`: идентификаторы, кавычки, `$user`, запятые, пробелы, точки и дефисы.
pub fn valid_search_path(s: &str) -> bool {
    !s.trim().is_empty()
        && s.len() <= 1024
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '"' | ',' | ' ' | '.' | '-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_search_path() {
        assert!(valid_search_path("\"$user\", public, shop"));
        assert!(valid_search_path("pg_catalog"));
        assert!(!valid_search_path("public; DROP TABLE x"));
        assert!(!valid_search_path(""));
    }

    /// Реэкспорт: вызовы P3 (`explain::params::*`) видят перенесённый лексер (его тесты — в `pg_advisor::sqltext`).
    #[test]
    fn reexports_the_lexer() {
        assert_eq!(count_params("SELECT $2, '$9'"), 2);
        assert_eq!(first_word("/* x */ (select 1)"), "select");
        assert_eq!(normalize("SELECT 1 ;"), "SELECT 1");
        assert_eq!(MAX_PARAMS, 65_535);
    }
}
