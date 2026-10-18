//! Тексты для интерфейса: ключ словаря + параметры + английский текст (для логов и как запасной вариант).
//! Перевод делает фронтенд по `src/locales/<язык>.json`; шаблоны `{{name}}` совпадают со словарём en.
//!
//! Правила для `ltext!` / `app_err!` (их проверяет `scripts/i18n-check.mjs`):
//! - ключ и шаблон — по одному обычному строковому литералу `"…"`: не `concat!`, не `format!`,
//!   не константа и не `r"…"`; из escape-последовательностей допустимы `\n`, `\t`, `\"`, `\\`,
//!   `\u{…}` и перенос строки через `\`. Английский текст в `en.json` должен совпадать с шаблоном;
//! - параметры — `name = значение`, и их набор совпадает с `{{…}}` шаблона (ни лишних, ни недостающих);
//! - значение параметра превращается в строку через `Display` (`.to_string()`): для путей —
//!   `path.display()`, для `Option`/`Vec` и прочего без `Display` — заранее `format!("{x:?}")`
//!   или явное преобразование.

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalizedText {
    pub key: String,
    pub params: BTreeMap<String, String>,
    pub text: String,
}

/// Подставляет `{{name}}` (пробелы внутри скобок допустимы); неизвестные параметры остаются как есть.
pub fn render(template: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                match params.get(name) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

impl LocalizedText {
    pub fn new(key: &str, en: &str, params: BTreeMap<String, String>) -> Self {
        Self {
            key: key.to_string(),
            text: render(en, &params),
            params,
        }
    }
}

/// `ltext!("messages.x.y", "English {{p}}", p = value)` → `LocalizedText`.
#[macro_export]
macro_rules! ltext {
    ($key:literal, $en:literal $(, $name:ident = $val:expr)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut params = ::std::collections::BTreeMap::<::std::string::String, ::std::string::String>::new();
        $( params.insert(::std::stringify!($name).to_string(), ($val).to_string()); )*
        $crate::i18n::LocalizedText::new($key, $en, params)
    }};
}

/// `app_err!(ErrorCode::X, "errors.m.y", "English {{p}}", p = value)` → `AppError`.
#[macro_export]
macro_rules! app_err {
    ($code:expr, $key:literal, $en:literal $(, $name:ident = $val:expr)* $(,)?) => {
        $crate::error::AppError::localized($code, $crate::ltext!($key, $en $(, $name = $val)*))
    };
}

impl LocalizedText {
    /// Технический текст как есть (сообщение сервера или драйвера, ошибка парсера и т. п.):
    /// не переводится, показывается через `errors.generic` (`{{message}}`). Только для текста,
    /// который нельзя перевести; фразы приложения — всегда через `ltext!` / `app_err!` с ключом.
    pub fn raw(text: impl Into<String>) -> Self {
        ltext!("errors.generic", "{{message}}", message = text.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn render_substitutes_params() {
        let mut p = BTreeMap::new();
        p.insert("name".to_string(), "prod-1".to_string());
        assert_eq!(
            render("Connection “{{name}}” is read-only", &p),
            "Connection “prod-1” is read-only"
        );
        assert_eq!(
            render("{{ name }} and {{missing}}", &p),
            "prod-1 and {{missing}}"
        );
        assert_eq!(render("tail {{name", &p), "tail {{name");
    }

    #[test]
    fn macros_build_key_params_and_english_text() {
        let t = ltext!("messages.test.hello", "Hello, {{who}}", who = "PostgreSQL");
        assert_eq!(t.key, "messages.test.hello");
        assert_eq!(t.params["who"], "PostgreSQL");
        assert_eq!(t.text, "Hello, PostgreSQL");
        let e = app_err!(
            ErrorCode::NotAuthorized,
            "errors.guard.readOnly",
            "Connection “{{name}}” is read-only",
            name = "p"
        );
        assert_eq!(e.code, ErrorCode::NotAuthorized);
        assert_eq!(e.message, "Connection “p” is read-only");
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["key"], "errors.guard.readOnly");
        assert_eq!(json["params"]["name"], "p");
    }

    #[test]
    fn raw_text_and_error_text() {
        let t = LocalizedText::raw("Server: unknown database");
        assert_eq!(t.key, "errors.generic");
        assert_eq!(t.params["message"], "Server: unknown database");
        assert_eq!(t.text, "Server: unknown database");
        let e = app_err!(
            ErrorCode::NotFound,
            "errors.databases.notFound",
            "Database “{{database}}” not found",
            database = "t"
        );
        let t = e.text();
        assert_eq!(t.key, "errors.databases.notFound");
        assert_eq!(t.params["database"], "t");
        assert_eq!(t.text, e.message);
    }
}
