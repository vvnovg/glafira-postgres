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

/// Текст для пробного плана с приведениями типизированных литералов частых типов: `date $1` → `$1::date`
/// (`pg_advisor::sqltext::cast_typed_literal_params`; смоук P4b, ревью правки B, #2). Так пишет нормализованные тексты
/// pg_stat_statements, а сервер `<тип> $N` не разбирает — меняются только тексты, которые иначе не дали бы плана.
/// Текст длиннее `MAX_SQL_BYTES` на лексемы не разбирается: его отвергнет проверка длины.
pub fn plannable(sql: &str) -> String {
    if normalize(sql).len() > crate::explain::MAX_SQL_BYTES {
        return sql.to_string();
    }
    pg_advisor::sqltext::cast_typed_literal_params(sql)
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

    /// Ревью правки B, #2: снимок карточки и «Сравнить планы» планируют `date $1` как `$1::date`; длинный текст — как
    /// есть (без разбора на лексемы).
    #[test]
    fn typed_literal_params_are_planned_as_casts() {
        assert_eq!(
            plannable("SELECT * FROM t WHERE d > date $1 AND s = 'date $2'"),
            "SELECT * FROM t WHERE d > $1::date AND s = 'date $2'"
        );
        assert_eq!(plannable("SELECT $1::date"), "SELECT $1::date");
        let long = format!(
            "SELECT date $1, {}1",
            "1,".repeat(crate::explain::MAX_SQL_BYTES / 2)
        );
        assert_eq!(plannable(&long), long);
    }
}
