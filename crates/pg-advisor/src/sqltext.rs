//! Лексер PostgreSQL без дерева разбора (спец. P4 §6.1): параметры `$N`, первое слово инструкции, хвостовые `;` и
//! маскирование литералов и комментариев. Перенос из `src-tauri/src/explain/params.rs` (P3): приложение реэкспортирует
//! `count_params`, `first_word`, `normalize`, `MAX_PARAMS`; `functions` берёт отсюда пропуск строк и символы
//! идентификатора. Дерево `pg_query` здесь не годится: тексты `pg_stat_activity` обрезаны, выражения плана — фрагменты.

/// Предел номера параметра: число параметров в сообщении Bind протокола PostgreSQL — 16-битное.
pub const MAX_PARAMS: usize = 65_535;

/// Символ продолжает идентификатор (`ident_cont` лексера PostgreSQL): латиница, цифры, `_`, `$` и байты не-ASCII.
pub(crate) fn ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Символ тега долларовых кавычек (`dolq_cont`): как у идентификатора, но без `$`; первый — не цифра (`dolq_start`).
fn dolq_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// Конец строки в кавычках `q`, начатой перед `from`: удвоенная кавычка — символ строки, `escapes` (E-строка) —
/// обратная косая экранирует следующий символ. Незакрытая строка тянется до конца текста.
pub(crate) fn skip_quoted(b: &[u8], from: usize, q: u8, escapes: bool) -> usize {
    let mut j = from;
    while j < b.len() {
        if escapes && b[j] == b'\\' {
            j += 2;
        } else if b[j] == q {
            if b.get(j + 1) == Some(&q) {
                j += 2;
            } else {
                return j + 1;
            }
        } else {
            j += 1;
        }
    }
    b.len()
}

/// Открывающий тег долларовых кавычек в `at` (`$$` или `$tag$`): конец тега; `None` — это не тег.
fn dollar_tag(b: &[u8], at: usize) -> Option<usize> {
    let mut j = at + 1;
    if b.get(j)
        .is_some_and(|c| dolq_cont(*c) && !c.is_ascii_digit())
    {
        while b.get(j).is_some_and(|c| dolq_cont(*c)) {
            j += 1;
        }
    }
    (b.get(j) == Some(&b'$')).then_some(j + 1)
}

/// Конец тела долларовых кавычек с тегом `b[at..tag_end]`: позиция после закрывающего тега; незакрытое — конец текста.
fn dollar_quote_end(b: &[u8], at: usize, tag_end: usize) -> usize {
    let tag = &b[at..tag_end];
    b[tag_end..]
        .windows(tag.len())
        .position(|w| w == tag)
        .map_or(b.len(), |p| tag_end + p + tag.len())
}

/// Конец строчного комментария `--`, начатого в `at`: позиция перевода строки (он в комментарий не входит).
fn line_comment_end(b: &[u8], at: usize) -> usize {
    b[at..]
        .iter()
        .position(|c| *c == b'\n')
        .map_or(b.len(), |p| at + p)
}

/// Конец блочного комментария, начатого в `at` (`/*`), с вложенностью, как в PostgreSQL; `true` — комментарий закрыт.
fn block_comment_end(b: &[u8], at: usize) -> (usize, bool) {
    let (mut depth, mut j) = (1usize, at + 2);
    while j < b.len() && depth > 0 {
        match (b[j], b.get(j + 1).copied()) {
            (b'/', Some(b'*')) => {
                depth += 1;
                j += 2;
            }
            (b'*', Some(b'/')) => {
                depth -= 1;
                j += 2;
            }
            _ => j += 1,
        }
    }
    (j.min(b.len()), depth == 0)
}

/// Наибольший номер `$N` — число параметров. Как лексер PostgreSQL, пропускает строки `'…'` (в том числе `E'…'` с
/// экранированием `\`), идентификаторы `"…"`, комментарии `--` и `/* */` (вложенные) и тела долларовых кавычек
/// `$$…$$` / `$tag$…$tag$`; `$` сразу после символа идентификатора (`a$1`) — часть идентификатора. Иначе лишний `$N`
/// стал бы параметром без типа, и сервер отказал бы (42P18). Номер больше `MAX_PARAMS` обрезается до предела.
/// Зеркало — `countParams` в `src/lib/useExplain.ts`.
pub fn count_params(sql: &str) -> usize {
    let b = sql.as_bytes();
    let (mut max, mut i) = (0usize, 0usize);
    while i < b.len() {
        let next = b.get(i + 1).copied();
        match b[i] {
            b'\'' => {
                // `E'…'`: буква E не должна быть хвостом идентификатора (`some_e'…'` — обычная строка).
                let escapes =
                    i > 0 && matches!(b[i - 1], b'e' | b'E') && (i < 2 || !ident_cont(b[i - 2]));
                i = skip_quoted(b, i + 1, b'\'', escapes);
            }
            b'"' => i = skip_quoted(b, i + 1, b'"', false),
            b'-' if next == Some(b'-') => i = line_comment_end(b, i),
            b'/' if next == Some(b'*') => i = block_comment_end(b, i).0,
            b'$' if i > 0 && ident_cont(b[i - 1]) => i += 1,
            b'$' if next.is_some_and(|c| c.is_ascii_digit()) => {
                let mut j = i + 1;
                let mut n = 0usize;
                while j < b.len() && b[j].is_ascii_digit() {
                    // Насыщение: номер за пределом всё равно обрезается, переполнения нет.
                    n = (n * 10 + usize::from(b[j] - b'0')).min(MAX_PARAMS + 1);
                    j += 1;
                }
                max = max.max(n.min(MAX_PARAMS));
                i = j;
            }
            b'$' => match dollar_tag(b, i) {
                Some(end) => i = dollar_quote_end(b, i, end),
                None => i += 1,
            },
            _ => i += 1,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_largest_parameter_number() {
        assert_eq!(count_params("SELECT 1"), 0);
        assert_eq!(count_params("SELECT * FROM t WHERE a = $1 AND b = $2"), 2);
        assert_eq!(count_params("SELECT $3, $1"), 3);
        assert_eq!(count_params("SELECT $1::int, ($2)"), 2);
        assert_eq!(count_params("SELECT $"), 0);
    }

    /// Параметр — только вне строк, идентификаторов, комментариев и долларовых кавычек: иначе лишний `$N` объявлялся бы
    /// параметром без типа, и сервер отказал бы (42P18), а не показал лишнее поле.
    #[test]
    fn skips_literals_identifiers_comments_and_dollar_quotes() {
        assert_eq!(count_params("SELECT '$5', $1"), 1, "строковый литерал");
        assert_eq!(
            count_params("SELECT 'it''s $5', $1"),
            1,
            "удвоенная кавычка внутри строки"
        );
        assert_eq!(
            count_params(r"SELECT E'\'$5', $1"),
            1,
            "E-строка: обратная косая экранирует кавычку"
        );
        assert_eq!(
            count_params(r"SELECT some_e'\', $2"),
            2,
            "кавычка после идентификатора на e — обычная строка"
        );
        assert_eq!(
            count_params(r#"SELECT "col$5", $1"#),
            1,
            "идентификатор в кавычках"
        );
        assert_eq!(
            count_params(r#"SELECT "a""$5", $1"#),
            1,
            "удвоенная кавычка в идентификаторе"
        );
        assert_eq!(
            count_params("SELECT a$1, b$2x, $1"),
            1,
            "$ после буквы — часть идентификатора"
        );
        assert_eq!(
            count_params("SELECT x_$3, y9$4"),
            0,
            "$ после _ и цифры — тоже"
        );
        assert_eq!(count_params("SELECT $1 -- $5"), 1, "строчный комментарий");
        assert_eq!(
            count_params("SELECT 1 -- $5\n + $2"),
            2,
            "строчный комментарий — до конца строки"
        );
        assert_eq!(
            count_params("SELECT /* $5 /* $6 */ $7 */ $1"),
            1,
            "блочные комментарии вложены"
        );
        assert_eq!(count_params("SELECT $$text $5$$, $1"), 1, "$$…$$");
        assert_eq!(
            count_params("SELECT $f$ $5 $$ $6 $f$, $2"),
            2,
            "$tag$…$tag$: внутри и $$ не закрывает"
        );
        assert_eq!(
            count_params("SELECT $_x1$ $5 $_x1$, $1"),
            1,
            "тег с _ и цифрой"
        );
        assert_eq!(
            count_params("SELECT $1$, $2"),
            2,
            "$1$ — не тег: цифра не начинает тег"
        );
        assert_eq!(
            count_params("SELECT 'open $5"),
            0,
            "незакрытая строка — до конца"
        );
        assert_eq!(
            count_params("SELECT $q$ open $5"),
            0,
            "незакрытые долларовые кавычки — до конца"
        );
        assert_eq!(
            count_params("SELECT /* open $5"),
            0,
            "незакрытый комментарий — до конца"
        );
        assert_eq!(
            count_params("SELECT 'é$5', таблица$6, $1"),
            1,
            "не-ASCII — символ идентификатора"
        );
    }

    #[test]
    fn parameter_number_is_capped_by_the_protocol_limit() {
        assert_eq!(MAX_PARAMS, 65_535);
        assert_eq!(count_params("SELECT $65535"), 65_535);
        assert_eq!(
            count_params("SELECT $70000"),
            MAX_PARAMS,
            "больший номер — предел"
        );
        assert_eq!(
            count_params("SELECT $99999999999999999999999, $1"),
            MAX_PARAMS,
            "без переполнения"
        );
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
    fn normalizes_trailing_semicolons() {
        assert_eq!(normalize("  SELECT 1 ;; \n"), "SELECT 1");
        assert_eq!(
            normalize("SELECT 1; SELECT 2"),
            "SELECT 1; SELECT 2",
            "внутренняя точка с запятой остаётся — её отвергнет сервер"
        );
    }
}
