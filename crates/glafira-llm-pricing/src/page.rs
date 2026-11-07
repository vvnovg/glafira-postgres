//! Разбор страницы цен Ollama (<https://ollama.com/pricing>) и её загрузка замыканием приложения.
//!
//! Разбор не держится за вёрстку (классы, `table`/`div`): HTML сводится к последовательности текстов элементов (без
//! `script`, `style` и комментариев), и строка цен — это имя модели и следом три цены «вход, кэш, выход»: `$` и число
//! или прочерк (цены нет). Строка «вне пика» — та же модель с пометкой «Off-Peak» в имени или отдельным элементом. Строка
//! без цены входа или выхода пропускается: по ней стоимость не оценить. Итог проверяется на здравый смысл
//! (`PriceTable::validate`).

use std::fmt;
use std::future::Future;

use time::{OffsetDateTime, UtcOffset};

use crate::table::{ModelPrices, PRICING_URL, ParseError, PriceTable, Prices};

/// Таблица цен со страницы `html`, загруженной в `fetched_at`: источник — `PRICING_URL`, день цен — день загрузки (UTC).
pub fn parse_pricing_page(
    html: &str,
    fetched_at: OffsetDateTime,
) -> Result<PriceTable, ParseError> {
    let rows = rows(&text_tokens(html));
    let mut models: Vec<ModelPrices> = Vec::new();
    for r in rows.iter().filter(|r| !r.off_peak) {
        if !models.iter().any(|m| m.model == r.model) {
            models.push(ModelPrices {
                model: r.model.clone(),
                standard: r.prices,
                off_peak: None,
            });
        }
    }
    for r in rows.iter().filter(|r| r.off_peak) {
        if let Some(m) = models
            .iter_mut()
            .find(|m| m.model == r.model && m.off_peak.is_none())
        {
            m.off_peak = Some(r.prices);
        }
    }
    let table = PriceTable {
        source: PRICING_URL.to_string(),
        date: fetched_at.to_offset(UtcOffset::UTC).date(),
        fetched_at: Some(fetched_at),
        models,
    };
    table.validate()?;
    Ok(table)
}

/// Страница цен не загрузилась (ошибка замыкания приложения) или не разобралась.
#[derive(Debug)]
pub enum RefreshError<E> {
    /// Сеть, HTTP, таймаут, прокси — как их описало приложение.
    Network(E),
    Parse(ParseError),
}

impl<E: fmt::Display> fmt::Display for RefreshError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RefreshError::Network(e) => write!(f, "price page not loaded: {e}"),
            RefreshError::Parse(e) => write!(f, "price page not parsed: {e}"),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for RefreshError<E> {}

/// Загрузка страницы цен замыканием приложения `fetch` (получает `PRICING_URL`, отдаёт текст страницы; клиент, прокси,
/// таймаут и проверку политики выбирает приложение) и её разбор. Таблицу не записывает: файл пишет приложение
/// (`store::save`) там, где можно блокировать поток.
pub async fn fetch_table<F, Fut, E>(
    fetch: F,
    now: OffsetDateTime,
) -> Result<PriceTable, RefreshError<E>>
where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Result<String, E>>,
{
    let html = fetch(PRICING_URL).await.map_err(RefreshError::Network)?;
    parse_pricing_page(&html, now).map_err(RefreshError::Parse)
}

/// Строка цен страницы.
#[derive(Debug)]
struct Row {
    model: String,
    off_peak: bool,
    prices: Prices,
}

/// Строки «имя, [пометка Off-Peak], три цены» в текстах элементов.
fn rows(tokens: &[String]) -> Vec<Row> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if let Some((model, mut off_peak)) = model_name(&tokens[i]) {
            let mut j = i + 1;
            if !off_peak && tokens.get(j).is_some_and(|t| off_peak_marker(t)) {
                off_peak = true;
                j += 1;
            }
            let cell = |k: usize| tokens.get(k).and_then(|t| price_cell(t));
            if let (Some(input), Some(cached_input), Some(output)) =
                (cell(j), cell(j + 1), cell(j + 2))
            {
                if let (Some(input), Some(output)) = (input, output) {
                    out.push(Row {
                        model,
                        off_peak,
                        prices: Prices {
                            input,
                            cached_input,
                            output,
                        },
                    });
                }
                i = j + 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

const OFF_PEAK: [&str; 3] = ["off-peak", "off peak", "offpeak"];

/// Пометка «вне пика» отдельным элементом: `Off-Peak`, `(Off-Peak)`.
fn off_peak_marker(t: &str) -> bool {
    let t = t
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    OFF_PEAK.iter().any(|m| t.eq_ignore_ascii_case(m))
}

/// Имя модели (в нижнем регистре) и пометка «(Off-Peak)» в нём: латиница, цифры и `._:/-`, начало — буква или цифра, хотя
/// бы одна буква, до 128 символов. Слова через пробел (заголовки, тексты) — не имя.
fn model_name(t: &str) -> Option<(String, bool)> {
    let lower = t.trim().to_ascii_lowercase();
    let suffix = OFF_PEAK
        .iter()
        .find_map(|m| lower.strip_suffix(&format!("({m})")).map(str::trim_end));
    let (name, off_peak) = match suffix {
        Some(name) => (name, true),
        None => (lower.as_str(), false),
    };
    let ok = (1..=128).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name.chars().any(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/-".contains(c));
    ok.then(|| (name.to_string(), off_peak))
}

/// Ячейка цены: `Some(Some(цена))` — `$` и число (`$0.30`, `$ 1,000`), `Some(None)` — прочерк (цены нет), `None` — не цена.
fn price_cell(t: &str) -> Option<Option<f64>> {
    let t = t.trim();
    if ["-", "–", "—", "n/a"]
        .iter()
        .any(|d| t.eq_ignore_ascii_case(d))
    {
        return Some(None);
    }
    let digits: String = t.strip_prefix('$')?.trim_start().replace(',', "");
    let valid = !digits.is_empty()
        && digits.starts_with(|c: char| c.is_ascii_digit())
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.');
    if !valid {
        return None;
    }
    digits.parse::<f64>().ok().map(Some)
}

/// Тексты элементов HTML по порядку: без тегов, `script`, `style` и комментариев; сущности раскрыты, пробелы схлопнуты,
/// пустые пропущены.
fn text_tokens(html: &str) -> Vec<String> {
    // Строчные ASCII не меняют длину в байтах: индексы `lower` годятся для `html`.
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    while i < html.len() {
        let rest = &lower[i..];
        if rest.starts_with("<!--") {
            flush(&mut text, &mut out);
            i = rest.find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        if let Some(name) = ["script", "style"].into_iter().find(|n| {
            rest.starts_with(&format!("<{n}")) && !is_name_char(rest.as_bytes().get(n.len() + 1))
        }) {
            flush(&mut text, &mut out);
            let close = format!("</{name}");
            i = match rest.find(&close) {
                Some(c) => tag_end(html, i + c),
                None => html.len(),
            };
            continue;
        }
        if rest.starts_with('<')
            && rest[1..]
                .starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!' || c == '?')
        {
            flush(&mut text, &mut out);
            i = tag_end(html, i);
            continue;
        }
        let c = html[i..].chars().next().expect("i < len");
        text.push(c);
        i += c.len_utf8();
    }
    flush(&mut text, &mut out);
    out
}

fn is_name_char(b: Option<&u8>) -> bool {
    b.is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// Индекс после `>` тега, начатого в `start`; `>` внутри кавычек атрибута тег не закрывает.
fn tag_end(html: &str, start: usize) -> usize {
    let mut quote = None;
    for (k, c) in html[start..].char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, '>') => return start + k + 1,
            _ => {}
        }
    }
    html.len()
}

fn flush(text: &mut String, out: &mut Vec<String>) {
    let t = decode_entities(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !t.is_empty() {
        out.push(t);
    }
    text.clear();
}

/// Сущности HTML, которые бывают в ценах и именах: `&amp;`, `&nbsp;`, `&#36;`, `&#x24;`, тире; незнакомые — как есть.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail.find(';').filter(|&e| e <= 10).and_then(|e| {
            let name = &tail[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                "dollar" => Some('$'),
                _ => name.strip_prefix('#').and_then(|n| {
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => n.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            c.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_of_elements_without_scripts_comments_and_tags() {
        let html = r#"<p title="a > b">One&nbsp;&amp; two</p><script type="x">"<b>no</b>"</script><scripts>yes</scripts><!-- <b>hidden</b> --><style>p{}</style> <b>$&#36;1</b> 3 < 4"#;
        assert_eq!(text_tokens(html), ["One & two", "yes", "$$1", "3 < 4"]);
        assert_eq!(text_tokens("<b>open"), ["open"]);
        assert_eq!(text_tokens("<!-- never closed"), Vec::<String>::new());
        assert_eq!(text_tokens("a <script>no end"), ["a"]);
    }

    #[test]
    fn prices_dashes_and_names() {
        assert_eq!(price_cell("$0.30"), Some(Some(0.30)));
        assert_eq!(price_cell(" $ 1,000 "), Some(Some(1000.0)));
        assert_eq!(price_cell("-"), Some(None));
        assert_eq!(price_cell("—"), Some(None));
        for t in ["$", "$x", "0.30", "$0.30/mo", "$-1", "$.5", "Input"] {
            assert_eq!(price_cell(t), None, "{t}");
        }
        assert_eq!(
            model_name("gpt-oss:120b"),
            Some(("gpt-oss:120b".into(), false))
        );
        assert_eq!(
            model_name("deepseek-v4-pro (Off-Peak)"),
            Some(("deepseek-v4-pro".into(), true))
        );
        for t in ["Pay as you go", "2026", "-x", "", "a b"] {
            assert_eq!(model_name(t), None, "{t}");
        }
        assert!(off_peak_marker("(Off-Peak)") && off_peak_marker("off peak"));
        assert!(!off_peak_marker("peak"));
    }
}
