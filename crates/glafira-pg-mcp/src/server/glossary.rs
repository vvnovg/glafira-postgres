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
    /// Абзац к тексту `text`: язык интерфейса и пары для подписей, которые называет текст. Язык ответа абзац не задаёт:
    /// тексты сами говорят «отвечай на языке пользователя» (решение владельца, смоук 2026-11-07). Подписи без пары
    /// пропускаются (полноту глоссария проверяют тесты приложения); нет ни одной пары — абзаца нет (пустая строка).
    pub fn paragraph(&self, text: &str) -> String {
        let pairs: Vec<String> = quoted_labels(text)
            .into_iter()
            .filter_map(|en| self.labels.get(en).map(|ui| format!("“{en}” — «{ui}»")))
            .collect();
        if pairs.is_empty() {
            return String::new();
        }
        format!(
            "The user's Glafira interface is in {}. When you name screens and buttons, use their interface labels: {}.",
            self.language,
            pairs.join(", ")
        )
    }
}

/// Текст для модели с абзацем глоссария в конце; без глоссария (английский интерфейс) или без пар — текст как есть.
pub fn with_glossary(text: String, glossary: Option<&UiGlossary>) -> String {
    match glossary.map(|g| g.paragraph(&text)) {
        Some(p) if !p.is_empty() => format!("{text}\n\n{p}"),
        _ => text,
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
                 interface labels: “Wait profile” — «Профиль ожиданий», “Pulse” — «Пульс»."
            )
        );
        assert!(
            !out.contains("Блокировки"),
            "пара только для подписей текста"
        );
        assert!(
            !out.contains("Answer the user in"),
            "язык ответа задают сами тексты"
        );
        // Нет пар — нет и абзаца: текст как есть.
        assert_eq!(glossary().paragraph("nothing quoted"), "");
        assert_eq!(
            with_glossary("nothing quoted".into(), Some(&glossary())),
            "nothing quoted"
        );
    }

    #[test]
    fn without_a_glossary_the_text_stays_as_it_is() {
        assert_eq!(with_glossary("“Pulse”".into(), None), "“Pulse”");
    }
}
