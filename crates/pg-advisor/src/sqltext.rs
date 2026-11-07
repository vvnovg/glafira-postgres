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

/// Метка Глафиры в тексте запроса (`marked_sql!` приложения): этот комментарий маскирование оставляет.
const MARKER_COMMENT: &str = "/* glafira */";

/// Параметры маскирования: значение `standard_conforming_strings` узла, с которого взят текст (неизвестно — `on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaskOptions {
    /// `off`: обратная косая в `'…'` и `N'…'` экранирует, как на сервере, — иначе граница литерала разошлась бы с
    /// серверной и часть значения осталась бы открытой.
    pub standard_conforming_strings: bool,
}

impl Default for MaskOptions {
    fn default() -> Self {
        Self {
            standard_conforming_strings: true,
        }
    }
}

/// Конец `UESCAPE 'x'` после `U&'…'` / `U&"…"`, начиная с `from`; нет его — `from`.
fn uescape_end(b: &[u8], from: usize) -> usize {
    let skip_space = |mut j: usize| {
        while b.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        j
    };
    let word = b"uescape";
    let j = skip_space(from);
    let is_word = b
        .get(j..j + word.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(word))
        && b.get(j + word.len()).is_none_or(|c| !ident_cont(*c));
    if !is_word {
        return from;
    }
    let k = skip_space(j + word.len());
    if b.get(k) == Some(&b'\'') {
        skip_quoted(b, k + 1, b'\'', false)
    } else {
        from
    }
}

/// Конец числа в `at` (цифра или `.` перед цифрой): целые, десятичные, порядок `e±N`, `0x` / `0o` / `0b` и
/// разделитель `_` (PostgreSQL 16).
fn number_end(b: &[u8], at: usize) -> usize {
    let digits = |mut j: usize, ok: fn(u8) -> bool| {
        while b.get(j).is_some_and(|c| ok(*c) || *c == b'_') {
            j += 1;
        }
        j
    };
    if b[at] == b'0' {
        let radix: Option<fn(u8) -> bool> = match b.get(at + 1) {
            Some(b'x' | b'X') => Some(|c: u8| c.is_ascii_hexdigit()),
            Some(b'o' | b'O') => Some(|c: u8| (b'0'..=b'7').contains(&c)),
            Some(b'b' | b'B') => Some(|c: u8| c == b'0' || c == b'1'),
            _ => None,
        };
        if let Some(ok) = radix {
            return digits(at + 2, ok);
        }
    }
    let decimal: fn(u8) -> bool = |c| c.is_ascii_digit();
    let mut j = digits(at, decimal);
    if b.get(j) == Some(&b'.') && b.get(j + 1) != Some(&b'.') {
        j = digits(j + 1, decimal);
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        let k = if matches!(b.get(j + 1), Some(b'+' | b'-')) {
            j + 2
        } else {
            j + 1
        };
        if b.get(k).is_some_and(u8::is_ascii_digit) {
            j = digits(k, decimal);
        }
    }
    j
}

/// Литералы и тела комментариев в тексте запроса (спец. P4 §6.1). Строки `'…'`, `E'…'`, `U&'…' [UESCAPE '…']`, `B'…'`,
/// `X'…'`, `N'…'`, долларовые кавычки и числа заменяются на `$N` с номером после наибольшего `$N` в тексте (как
/// `pg_query normalize`); приведение остаётся (`'abc'::text` → `$3::text`). Тело блочного комментария (вложенного, как
/// в PostgreSQL) — на `/*…*/`, строчного — на `--…`: sqlcommenter и подобные несут данные; метка `/* glafira */`
/// остаётся. Идентификаторы (`"…"`, `U&"…"`) и `$N` не меняются. Незакрытый литерал или комментарий (текст обрезан по
/// `track_activity_query_size`) маскируется до конца. Второе значение — число замаскированных литералов (комментарии не
/// считаются) — уходит в журнал MCP (`masked`).
pub fn mask_literals(text: &str, opts: MaskOptions) -> (String, usize) {
    let b = text.as_bytes();
    let backslash = !opts.standard_conforming_strings;
    let mut out = String::with_capacity(text.len());
    let mut next = count_params(text) + 1;
    let mut masked = 0usize;
    // `kept` — начало ещё не перенесённого в `out` куска исходного текста.
    let (mut i, mut kept) = (0usize, 0usize);
    while i < b.len() {
        let c = b[i];
        let (n1, n2) = (b.get(i + 1).copied(), b.get(i + 2).copied());
        let literal_end = match c {
            b'\'' => Some(skip_quoted(b, i + 1, b'\'', backslash)),
            b'e' | b'E' if n1 == Some(b'\'') => Some(skip_quoted(b, i + 2, b'\'', true)),
            b'n' | b'N' if n1 == Some(b'\'') => Some(skip_quoted(b, i + 2, b'\'', backslash)),
            b'b' | b'B' | b'x' | b'X' if n1 == Some(b'\'') => {
                Some(skip_quoted(b, i + 2, b'\'', false))
            }
            b'u' | b'U' if n1 == Some(b'&') && n2 == Some(b'\'') => {
                Some(uescape_end(b, skip_quoted(b, i + 3, b'\'', false)))
            }
            b'$' if n1.is_some_and(|d| d.is_ascii_digit()) => None,
            b'$' => dollar_tag(b, i).map(|end| dollar_quote_end(b, i, end)),
            b'0'..=b'9' => Some(number_end(b, i)),
            b'.' if n1.is_some_and(|d| d.is_ascii_digit()) => Some(number_end(b, i)),
            _ => None,
        };
        if let Some(end) = literal_end {
            out.push_str(&text[kept..i]);
            out.push('$');
            out.push_str(&next.to_string());
            next += 1;
            masked += 1;
            (i, kept) = (end, end);
            continue;
        }
        match c {
            b'-' if n1 == Some(b'-') => {
                out.push_str(&text[kept..i]);
                out.push_str("--…");
                i = line_comment_end(b, i);
                kept = i;
            }
            b'/' if n1 == Some(b'*') => {
                let (end, closed) = block_comment_end(b, i);
                if text.get(i..end) != Some(MARKER_COMMENT) {
                    out.push_str(&text[kept..i]);
                    out.push_str(if closed { "/*…*/" } else { "/*…" });
                    kept = end;
                }
                i = end;
            }
            b'"' => i = skip_quoted(b, i + 1, b'"', false),
            b'u' | b'U' if n1 == Some(b'&') && n2 == Some(b'"') => {
                i = uescape_end(b, skip_quoted(b, i + 3, b'"', false));
            }
            // `$N` (номер параметра) и одиночный `$` — не литералы.
            b'$' => {
                i += 1;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
            }
            // Идентификатор целиком: цифры, `$` и буквы префиксов внутри него — не начало литерала.
            _ if ident_cont(c) => {
                while b.get(i).is_some_and(|x| ident_cont(*x)) {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[kept..]);
    (out, masked)
}

/// Типы типизированных литералов (`date '2026-06-01'`), которые pg_stat_statements нормализует в `date $1`: такой
/// текст не разбирается ни `pg_query`, ни сервером. Слова — в нижнем регистре; составные имена — раньше своих начал.
const TYPED_LITERAL_TYPES: [&[&str]; 34] = [
    &["timestamp", "with", "time", "zone"],
    &["timestamp", "without", "time", "zone"],
    &["time", "with", "time", "zone"],
    &["time", "without", "time", "zone"],
    &["double", "precision"],
    &["date"],
    &["time"],
    &["timetz"],
    &["timestamp"],
    &["timestamptz"],
    &["interval"],
    &["uuid"],
    &["json"],
    &["jsonb"],
    &["numeric"],
    &["decimal"],
    &["int2"],
    &["int4"],
    &["int8"],
    &["smallint"],
    &["integer"],
    &["int"],
    &["bigint"],
    &["real"],
    &["float4"],
    &["float8"],
    &["text"],
    &["varchar"],
    &["char"],
    &["boolean"],
    &["bool"],
    &["inet"],
    &["cidr"],
    &["bytea"],
];

/// Нормализованный текст pg_stat_statements с типизированными литералами (смоук P4b, G5): `<тип> $N` → `$N::<тип>`
/// для типов `TYPED_LITERAL_TYPES` (без учёта регистра, целыми словами; имя типа — как в тексте). Строки, идентификаторы
/// в кавычках, комментарии и тела долларовых кавычек не трогаются; тип, перед которым точка (`s.date`), и тип,
/// отделённый от `$N` комментарием, — тоже. Уже приведённый `$1::date` остаётся как есть.
pub fn cast_typed_literal_params(sql: &str) -> String {
    apply_typed_literal_casts(sql, &typed_literal_casts(sql))
}

/// Текст с приведениями `casts` (`typed_literal_casts` этого же текста).
pub fn apply_typed_literal_casts(sql: &str, casts: &[TypedLiteralCast]) -> String {
    let mut out = String::with_capacity(sql.len() + casts.len() * 2);
    let mut copied = 0;
    for c in casts {
        out.push_str(&sql[copied..c.type_start]);
        out.push_str(&sql[c.param_start..c.param_end]);
        out.push_str("::");
        out.push_str(&sql[c.type_start..c.type_end]);
        copied = c.param_end;
    }
    out.push_str(&sql[copied..]);
    out
}

/// Приведение `<тип> $N` → `$N::<тип>` в тексте: байтовые границы типа (как в тексте, слова и пробелы между ними) и
/// параметра `$N`; между ними — только пробельные символы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedLiteralCast {
    pub type_start: usize,
    pub type_end: usize,
    pub param_start: usize,
    pub param_end: usize,
}

/// Приведения, которые сделает `cast_typed_literal_params`, по порядку текста.
pub fn typed_literal_casts(sql: &str) -> Vec<TypedLiteralCast> {
    let tokens = sql_tokens(sql.as_bytes());
    tokens
        .iter()
        .enumerate()
        .filter_map(|(k, tok)| {
            let Token::Param(param_start, param_end) = *tok else {
                return None;
            };
            let (type_start, type_end) = typed_literal_type(sql, &tokens[..k])?;
            Some(TypedLiteralCast {
                type_start,
                type_end,
                param_start,
                param_end,
            })
        })
        .collect()
}

/// Лексема для `cast_typed_literal_params`: слово без кавычек, параметр `$N` или что-то другое (строка, идентификатор в
/// кавычках, комментарий, число, знак) с позицией начала. Позиции — байтовые, на границах символов.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Word(usize, usize),
    Param(usize, usize),
    Other(usize),
}

/// Лексемы текста без пробельных символов — как в `count_params`, но со словами.
fn sql_tokens(b: &[u8]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            b'\'' => {
                let escapes =
                    i > 0 && matches!(b[i - 1], b'e' | b'E') && (i < 2 || !ident_cont(b[i - 2]));
                i = skip_quoted(b, i + 1, b'\'', escapes);
            }
            b'"' => i = skip_quoted(b, i + 1, b'"', false),
            b'-' if next == Some(b'-') => i = line_comment_end(b, i),
            b'/' if next == Some(b'*') => i = block_comment_end(b, i).0,
            b'$' if next.is_some_and(|d| d.is_ascii_digit()) => {
                i += 1;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                out.push(Token::Param(start, i));
                continue;
            }
            b'$' => i = dollar_tag(b, i).map_or(i + 1, |end| dollar_quote_end(b, i, end)),
            c if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                while b.get(i).is_some_and(|c| ident_cont(*c)) {
                    i += 1;
                }
                out.push(Token::Word(start, i));
                continue;
            }
            c if c.is_ascii_digit() => {
                // Число вместе с хвостом (`1e5`, `0x1F`): хвост — не слово.
                while b.get(i).is_some_and(|c| ident_cont(*c) || *c == b'.') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
        out.push(Token::Other(start));
    }
    out
}

/// Тип из `TYPED_LITERAL_TYPES`, которым кончаются лексемы `before` (перед параметром): начало и конец его текста. Перед
/// типом не должно быть точки — `s.date` это имя, а не тип — и `::`: `x::date $1` — приведение `x`, а не литерал
/// (текст и так не SQL, а двойное приведение `x::$1::date` увело бы ошибку сервера от текста модели).
fn typed_literal_type(sql: &str, before: &[Token]) -> Option<(usize, usize)> {
    let b = sql.as_bytes();
    TYPED_LITERAL_TYPES.iter().find_map(|words| {
        let from = before.len().checked_sub(words.len())?;
        let mut span = None;
        for (tok, word) in before[from..].iter().zip(words.iter()) {
            let Token::Word(s, e) = *tok else {
                return None;
            };
            if !sql[s..e].eq_ignore_ascii_case(word) {
                return None;
            }
            span = Some(span.map_or((s, e), |(first, _)| (first, e)));
        }
        let qualified = from.checked_sub(1).is_some_and(|p| {
            matches!(before[p], Token::Other(at)
                if b[at] == b'.' || (b[at] == b':' && at > 0 && b[at - 1] == b':'))
        });
        span.filter(|_| !qualified)
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

    fn mask(text: &str) -> (String, usize) {
        mask_literals(text, MaskOptions::default())
    }

    fn pair(text: &str, count: usize) -> (String, usize) {
        (text.to_string(), count)
    }

    #[test]
    fn mask_options_default_to_standard_conforming_strings() {
        assert!(MaskOptions::default().standard_conforming_strings);
    }

    /// Строки и числа → `$N` после наибольшего `$N` текста; приведения и `$N` остаются.
    #[test]
    fn masks_strings_and_numbers_numbering_after_existing_params() {
        assert_eq!(
            mask("SELECT * FROM t WHERE a = 'x' AND b = 42"),
            pair("SELECT * FROM t WHERE a = $1 AND b = $2", 2)
        );
        assert_eq!(
            mask("SELECT 'it''s'"),
            pair("SELECT $1", 1),
            "удвоенная кавычка"
        );
        assert_eq!(
            mask("SELECT $2, 'a', $1"),
            pair("SELECT $2, $3, $1", 1),
            "номер — после наибольшего $N"
        );
        assert_eq!(
            mask("SELECT 'abc'::text, 5::int"),
            pair("SELECT $1::text, $2::int", 2),
            "приведение остаётся"
        );
        assert_eq!(
            mask("SELECT $1 + 2"),
            pair("SELECT $1 + $2", 1),
            "$N — не литерал"
        );
        assert_eq!(mask("SELECT -5"), pair("SELECT -$1", 1), "минус — оператор");
        assert_eq!(
            mask("SELECT date '2024-01-01', interval '1 day'"),
            pair("SELECT date $1, interval $2", 2),
            "типизированные литералы"
        );
        assert_eq!(
            mask("ALTER ROLE app PASSWORD 'hunter2'"),
            pair("ALTER ROLE app PASSWORD $1", 1),
            "служебные команды pgss бывают с литералами"
        );
        assert_eq!(mask(""), pair("", 0));
    }

    #[test]
    fn masks_every_kind_of_string_literal() {
        assert_eq!(
            mask(r"SELECT E'it\'s', e'\\'"),
            pair("SELECT $1, $2", 2),
            "E-строки: обратная косая экранирует"
        );
        assert_eq!(
            mask("SELECT U&'d!0061t' UESCAPE '!', u&'x'"),
            pair("SELECT $1, $2", 2),
            "U&'…' вместе с UESCAPE — один литерал"
        );
        assert_eq!(
            mask("SELECT B'1010', X'1F', N'abc', b'0'"),
            pair("SELECT $1, $2, $3, $4", 4),
            "битовые и национальные строки"
        );
        assert_eq!(
            mask("SELECT $$a'b$$, $tag$ x $$ y $tag$, $1"),
            pair("SELECT $2, $3, $1", 2),
            "долларовые кавычки с метками; $$ внутри $tag$ не закрывает"
        );
        assert_eq!(
            mask(r"SELECT some_e'\', 1"),
            pair("SELECT some_e$1, $2", 2),
            "кавычка после идентификатора на e — обычная строка"
        );
        assert_eq!(
            mask("SELECT 'привет', имя FROM таблица WHERE x = 'é'"),
            pair("SELECT $1, имя FROM таблица WHERE x = $2", 2),
            "не-ASCII"
        );
    }

    /// Числа: целые, десятичные, с порядком, с основанием и разделителем `_` (PostgreSQL 16); цифры в именах не трогаются.
    #[test]
    fn masks_numbers_but_not_digits_inside_identifiers() {
        assert_eq!(
            mask("SELECT 1, 1.5, .5, 1e10, 2.5E-3, 0x1F, 0o17, 0b101, 1_000_000"),
            pair("SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9", 9)
        );
        assert_eq!(
            mask("SELECT t1.c2, x$1, col_3 FROM t1"),
            pair("SELECT t1.c2, x$1, col_3 FROM t1", 0)
        );
        assert_eq!(mask("SELECT a1+2"), pair("SELECT a1+$1", 1));
        assert_eq!(
            mask("SELECT e, x, b, n, u FROM t"),
            pair("SELECT e, x, b, n, u FROM t", 0),
            "буквы префиксов без кавычки — имена"
        );
    }

    /// `standard_conforming_strings = off`: обратная косая в `'…'` экранирует, как на сервере. При `on` та же строка
    /// кончается раньше — граница литерала совпадает с серверной в обоих случаях.
    #[test]
    fn follows_standard_conforming_strings_of_the_server() {
        let off = MaskOptions {
            standard_conforming_strings: false,
        };
        assert_eq!(
            mask_literals(r"SELECT 'a\'b', 1", off),
            pair("SELECT $1, $2", 2)
        );
        assert_eq!(mask_literals(r"SELECT N'a\'b'", off), pair("SELECT $1", 1));
        // `'a\'` закрыта; дальше `b', 1` — битовая строка `b'…'` без закрытия (как в лексере сервера): $2 до конца.
        assert_eq!(mask(r"SELECT 'a\'b', 1"), pair("SELECT $1$2", 2));
    }

    #[test]
    fn masks_comment_bodies_but_keeps_the_marker() {
        assert_eq!(
            mask("SELECT 1 /* app=web user=42 */ -- secret 'x'\nFROM t"),
            pair("SELECT $1 /*…*/ --…\nFROM t", 1),
            "комментарии не считаются"
        );
        assert_eq!(
            mask("SELECT /* a /* b */ c */ 2"),
            pair("SELECT /*…*/ $1", 1),
            "вложенные"
        );
        assert_eq!(
            mask("SELECT 1 /*controller='users',db_driver='psycopg'*/"),
            pair("SELECT $1 /*…*/", 1),
            "sqlcommenter"
        );
        assert_eq!(
            mask("/* glafira */ SELECT 'x'"),
            pair("/* glafira */ SELECT $1", 1),
            "метка Глафиры остаётся"
        );
    }

    #[test]
    fn keeps_identifiers_and_parameters() {
        assert_eq!(
            mask(r#"SELECT "it's", "a""b" FROM "t 1""#),
            pair(r#"SELECT "it's", "a""b" FROM "t 1""#, 0)
        );
        assert_eq!(
            mask(r#"SELECT U&"d!0061t" UESCAPE '!' FROM t"#),
            pair(r#"SELECT U&"d!0061t" UESCAPE '!' FROM t"#, 0),
            "U&\"…\" UESCAPE — идентификатор"
        );
    }

    /// Текст обрезан по `track_activity_query_size`: незакрытый литерал или комментарий маскируется до конца.
    #[test]
    fn masks_unterminated_tail_to_the_end() {
        assert_eq!(
            mask("SELECT * FROM t WHERE a = 'secr"),
            pair("SELECT * FROM t WHERE a = $1", 1)
        );
        assert_eq!(mask("SELECT 1 /* trunc"), pair("SELECT $1 /*…", 1));
        assert_eq!(mask("SELECT 1 -- trunc"), pair("SELECT $1 --…", 1));
        assert_eq!(mask("SELECT $q$ open"), pair("SELECT $1", 1));
        assert_eq!(mask(r"SELECT E'abc\"), pair("SELECT $1", 1));
    }

    /// Смоук P4b, G5: `date $1` из pgss становится `$1::date`, каждая форма типа из списка.
    #[test]
    fn typed_literal_params_become_casts() {
        let cast = cast_typed_literal_params;
        for t in [
            "date",
            "time",
            "timetz",
            "timestamp",
            "timestamptz",
            "timestamp with time zone",
            "timestamp without time zone",
            "time with time zone",
            "time without time zone",
            "interval",
            "uuid",
            "json",
            "jsonb",
            "numeric",
            "decimal",
            "int2",
            "int4",
            "int8",
            "smallint",
            "integer",
            "int",
            "bigint",
            "real",
            "double precision",
            "float4",
            "float8",
            "text",
            "varchar",
            "char",
            "boolean",
            "bool",
            "inet",
            "cidr",
            "bytea",
        ] {
            assert_eq!(
                cast(&format!("SELECT * FROM t WHERE a = {t} $1")),
                format!("SELECT * FROM t WHERE a = $1::{t}"),
                "{t}"
            );
        }
        assert_eq!(
            cast("SELECT count(*) FROM o WHERE o.created_at >= TIMESTAMPTZ $1 - Interval $2"),
            "SELECT count(*) FROM o WHERE o.created_at >= $1::TIMESTAMPTZ - $2::Interval"
        );
        assert_eq!(
            cast("SELECT a FROM t WHERE d BETWEEN date $1 AND date $12 LIMIT $13"),
            "SELECT a FROM t WHERE d BETWEEN $1::date AND $12::date LIMIT $13"
        );
        assert_eq!(
            cast("SELECT timestamp\n  with time\tzone $1, double  precision $2"),
            "SELECT $1::timestamp\n  with time\tzone, $2::double  precision"
        );
        assert_eq!(cast("SELECT (date $1)"), "SELECT ($1::date)");
    }

    /// Строки, идентификаторы в кавычках, комментарии, долларовые кавычки и уже приведённые параметры не меняются, как и
    /// слова, которые типом здесь не являются.
    #[test]
    fn typed_literal_casts_leave_everything_else_alone() {
        for text in [
            "SELECT 'date $1' FROM t",
            "SELECT E'it\\'s date $1' FROM t",
            "SELECT \"date $1\" FROM t",
            "SELECT 1 -- date $1\nFROM t",
            "SELECT 1 /* date $1 /* nested */ */ FROM t",
            "SELECT $$date $1$$, $f$ date $2 $f$",
            "SELECT * FROM t WHERE d = $1::date",
            "SELECT * FROM t WHERE d = date($1)",
            "SELECT * FROM t WHERE date = $1",
            "SELECT * FROM t WHERE d = mydate $1",
            "SELECT * FROM t WHERE d = date_x $1",
            "SELECT * FROM t WHERE d = s.date $1",
            "SELECT * FROM t WHERE d = date /* c */ $1",
            "SELECT ts AT TIME ZONE $1 FROM t",
            "SELECT * FROM t WHERE a = b$1",
            "SELECT * FROM t WHERE d = character varying $1",
            "SELECT * FROM t LIMIT $1",
            // Финальное ревью правок смоука, nit #5: тип после `::` — приведение, а не типизированный литерал.
            "SELECT * FROM t WHERE d = x::date $1",
            "SELECT * FROM t WHERE d = x:: double precision $1",
        ] {
            assert_eq!(cast_typed_literal_params(text), text);
        }
        assert_eq!(
            cast_typed_literal_params("SELECT 'x' || text $1, '-- date $2' FROM t -- int $3"),
            "SELECT 'x' || $1::text, '-- date $2' FROM t -- int $3"
        );
    }

    /// Ревью правки B, #2: границы каждого приведения — тип (как в тексте) и параметр; по ним шлюз пересчитывает позиции
    /// ошибок сервера в текст, который прислала модель.
    #[test]
    fn typed_literal_casts_give_the_type_and_param_spans() {
        let sql = "SELECT 'ж', date $1, timestamp\n with time zone  $2 FROM t WHERE x = $3 AND y = int $4";
        let spans: Vec<(&str, &str)> = typed_literal_casts(sql)
            .iter()
            .map(|c| {
                (
                    &sql[c.type_start..c.type_end],
                    &sql[c.param_start..c.param_end],
                )
            })
            .collect();
        assert_eq!(
            spans,
            [
                ("date", "$1"),
                ("timestamp\n with time zone", "$2"),
                ("int", "$4")
            ]
        );
        assert!(typed_literal_casts("SELECT $1::date, date '2026-01-01'").is_empty());
    }

    /// Повторное маскирование ничего не меняет: `$N`, `/*…*/` и `--…` — уже маска.
    #[test]
    fn masking_is_idempotent() {
        for text in [
            "SELECT * FROM t WHERE a = 'x' AND b = 42 /* c */ -- d\n",
            "SELECT $$a$$, 0x1F, E'\\n' FROM t WHERE c = 'open",
            "/* glafira */ SELECT 1 /* trunc",
        ] {
            let (once, _) = mask(text);
            assert_eq!(mask(&once), (once.clone(), 0), "{text:?}");
        }
    }
}
