//! Проверка SQL от модели по дереву `pg_query` (спец. P4 §7.1). Задача 1 плана P4a — проба сборки `pg_query` на всех
//! раннерах (`parse_count`); проверки по назначениям — задача 3.

/// Число выражений в тексте по разбору `pg_query` (грамматика PostgreSQL 18); ошибка — текст `pg_query`.
pub fn parse_count(sql: &str) -> Result<usize, String> {
    pg_query::parse(sql, pg_query::ParserOptions::DEFAULT)
        .map(|r| r.protobuf.stmts.len())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Проба сборки `pg_query`: грамматика PostgreSQL 18 считает выражения; `;` внутри строк, комментариев и долларовых
    /// кавычек выражений не делит; пустой текст и одни комментарии — ноль выражений.
    #[test]
    fn parse_count_counts_statements() {
        assert_eq!(parse_count("SELECT 1"), Ok(1));
        assert_eq!(parse_count("SELECT 1; SELECT 2;"), Ok(2));
        assert_eq!(parse_count("SELECT ';' -- ;\n/* ; */"), Ok(1));
        assert_eq!(parse_count("SELECT $$;$$, $q$ ; $q$"), Ok(1));
        assert_eq!(parse_count(""), Ok(0));
        assert_eq!(parse_count("-- только комментарий"), Ok(0));
    }

    /// Грамматика — PostgreSQL 18: числа PG 16 (основание, `_`), JSON_TABLE и MERGE … RETURNING из PG 17,
    /// RETURNING WITH (OLD AS …, NEW AS …) из PG 18.
    #[test]
    fn parse_count_knows_postgresql_18_syntax() {
        assert_eq!(parse_count("SELECT 0x1F, 0o17, 0b101, 1_000_000"), Ok(1));
        assert_eq!(
            parse_count(
                "SELECT * FROM JSON_TABLE('[{\"a\":1}]', '$[*]' COLUMNS (a int PATH '$.a')) jt"
            ),
            Ok(1)
        );
        assert_eq!(
            parse_count(
                "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE RETURNING t.*"
            ),
            Ok(1)
        );
        assert_eq!(
            parse_count("UPDATE t SET a = a + 1 RETURNING WITH (OLD AS o, NEW AS n) o.a, n.a"),
            Ok(1)
        );
    }

    #[test]
    fn parse_count_reports_errors() {
        let e = parse_count("SELEC 1").unwrap_err();
        assert!(e.contains("syntax error"), "{e}");
        assert!(parse_count("SELECT 1\0").is_err(), "NUL в тексте");
    }
}
