//! Подписи интерфейса на языке пользователя (смоук 2026-11-07, R8). Тексты для модели (`instructions`, prompts, промпт
//! фонового запуска) называют экраны и кнопки Glafira английскими подписями в “…”: по ним модель связывает инструменты и
//! экраны. При русском интерфейсе модель так и писала пользователю — «Wait profile», «Send to LLM assistant». Поэтому к
//! тексту добавляется абзац с парами «английская подпись — подпись интерфейса» и просьбой отвечать на языке интерфейса.
//!
//! Словарей интерфейса крейт не знает. Подписи собирает приложение из `src/locales/<язык>.json` и отдаёт через
//! `PgBackend::ui_glossary`. У английского интерфейса глоссария нет, и тексты остаются как были.

use std::collections::BTreeMap;

/// Подписи интерфейса на языке пользователя.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiGlossary {
    /// Язык интерфейса по-английски, например `Russian`.
    pub language: String,
    /// Английская подпись (как в “…” текстов) → подпись в интерфейсе пользователя.
    pub labels: BTreeMap<String, String>,
}

/// Подписи в “…” по порядку первого появления, без повторов.
pub fn quoted_labels(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for part in text.split('“').skip(1) {
        if let Some((label, _)) = part.split_once('”')
            && !out.contains(&label)
        {
            out.push(label);
        }
    }
    out
}

impl UiGlossary {
    /// Абзац к тексту `text`: язык интерфейса, пары для подписей, которые называет текст, и просьба отвечать на языке
    /// интерфейса. Подписи без пары пропускаются: полноту глоссария проверяют тесты приложения.
    pub fn paragraph(&self, text: &str) -> String {
        let pairs: Vec<String> = quoted_labels(text)
            .into_iter()
            .filter_map(|en| self.labels.get(en).map(|ui| format!("“{en}” — «{ui}»")))
            .collect();
        let language = &self.language;
        let mut p = format!("The user's Glafira interface is in {language}.");
        if !pairs.is_empty() {
            p.push_str(" When you name screens and buttons, use their interface labels: ");
            p.push_str(&pairs.join(", "));
            p.push('.');
        }
        p.push_str(&format!(" Answer the user in {language}."));
        p
    }
}

/// Текст для модели с абзацем глоссария в конце; без глоссария (английский интерфейс) — текст как есть.
pub fn with_glossary(text: String, glossary: Option<&UiGlossary>) -> String {
    match glossary {
        Some(g) => {
            let p = g.paragraph(&text);
            format!("{text}\n\n{p}")
        }
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary() -> UiGlossary {
        UiGlossary {
            language: "Russian".into(),
            labels: BTreeMap::from([
                ("Pulse".to_string(), "Пульс".to_string()),
                ("Wait profile".to_string(), "Профиль ожиданий".to_string()),
                ("Locks".to_string(), "Блокировки".to_string()),
            ]),
        }
    }

    #[test]
    fn labels_are_taken_from_the_quotes_once_in_order() {
        assert_eq!(
            quoted_labels("“Wait profile” then “Pulse”, again “Wait profile”; broken “tail"),
            ["Wait profile", "Pulse"]
        );
        assert!(quoted_labels("no labels").is_empty());
    }

    #[test]
    fn the_paragraph_pairs_only_the_labels_of_the_text() {
        let text = "Open “Wait profile”, then “Pulse” and “Unknown”.".to_string();
        let out = with_glossary(text.clone(), Some(&glossary()));
        assert_eq!(
            out,
            format!(
                "{text}\n\nThe user's Glafira interface is in Russian. When you name screens and buttons, use their \
                 interface labels: “Wait profile” — «Профиль ожиданий», “Pulse” — «Пульс». Answer the user in Russian."
            )
        );
        assert!(
            !out.contains("Блокировки"),
            "пара только для подписей текста"
        );
        assert_eq!(
            glossary().paragraph("nothing quoted"),
            "The user's Glafira interface is in Russian. Answer the user in Russian."
        );
    }

    #[test]
    fn without_a_glossary_the_text_stays_as_it_is() {
        assert_eq!(with_glossary("“Pulse”".into(), None), "“Pulse”");
    }
}
