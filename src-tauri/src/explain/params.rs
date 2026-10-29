//! Параметры `$N`, первое слово инструкции, `search_path` — разбор без парсера SQL (решение владельца: `pg_query` — P4).

/// Наибольший номер `$N` — число параметров. Текст pgss литералов не содержит; в консоли литерал с `$1` внутри
/// строки даст лишнее поле формы, а не ошибку. `$$` долларовых кавычек — не параметр.
pub fn count_params(sql: &str) -> usize {
    let b = sql.as_bytes();
    let (mut max, mut i) = (0usize, 0usize);
    while i < b.len() {
        if b[i] == b'$' {
            let mut j = i + 1;
            let mut n = 0usize;
            while j < b.len() && b[j].is_ascii_digit() && n < 100_000 {
                n = n * 10 + usize::from(b[j] - b'0');
                j += 1;
            }
            if j > i + 1 && n > max {
                max = n;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    max
}

/// Первое слово инструкции без ведущих комментариев (`--`, `/* */`) и открывающих скобок; пусто — слов нет.
pub fn first_word(sql: &str) -> String {
    let mut s = sql;
    loop {
        s = s.trim_start_matches(|c: char| c.is_whitespace() || c == '(');
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |(_, r)| r);
        } else {
            break;
        }
    }
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// Хвостовые `;` и пробелы снимаются; внутренняя `;` остаётся — её отвергнет сервер (одна инструкция).
pub fn normalize(sql: &str) -> String {
    let mut s = sql.trim();
    while let Some(t) = s.strip_suffix(';') {
        s = t.trim_end();
    }
    s.to_string()
}

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
    fn counts_the_largest_parameter_number() {
        assert_eq!(count_params("SELECT 1"), 0);
        assert_eq!(count_params("SELECT * FROM t WHERE a = $1 AND b = $2"), 2);
        assert_eq!(count_params("SELECT $3, $1"), 3);
        assert_eq!(
            count_params("SELECT $$text$$, $1"),
            1,
            "долларовые кавычки — не параметр"
        );
        assert_eq!(
            count_params("SELECT '$5'"),
            5,
            "литерал в консоли даёт лишнее поле формы, не ошибку"
        );
        assert_eq!(count_params("SELECT $"), 0);
    }

    #[test]
    fn first_word_skips_comments_and_parentheses() {
        assert_eq!(first_word("  SELECT 1"), "SELECT");
        assert_eq!(first_word("-- comment\n/* block */ (select 1)"), "select");
        assert_eq!(first_word("/* a */ CREATE TABLE x AS SELECT 1"), "CREATE");
        assert_eq!(first_word("with d as (delete from t) select 1"), "with");
        assert_eq!(first_word("   "), "");
    }

    #[test]
    fn normalizes_trailing_semicolons_and_validates_search_path() {
        assert_eq!(normalize("  SELECT 1 ;; \n"), "SELECT 1");
        assert_eq!(
            normalize("SELECT 1; SELECT 2"),
            "SELECT 1; SELECT 2",
            "внутренняя точка с запятой остаётся — её отвергнет сервер"
        );
        assert!(valid_search_path("\"$user\", public, shop"));
        assert!(valid_search_path("pg_catalog"));
        assert!(!valid_search_path("public; DROP TABLE x"));
        assert!(!valid_search_path(""));
    }
}
