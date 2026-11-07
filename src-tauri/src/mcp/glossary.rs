//! Глоссарий подписей интерфейса для текстов модели (смоук 2026-11-07, R8). `INSTRUCTIONS`, `INSTRUCTIONS_RUN`, prompts и
//! промпт фонового запуска называют экраны и кнопки Glafira английскими подписями в “…”. При русском интерфейсе модель
//! так и писала пользователю, поэтому к этим текстам `glafira-pg-mcp` добавляет пары «английская подпись — подпись
//! интерфейса» (`UiGlossary`).
//!
//! Пары берутся из словарей интерфейса `src/locales/{en,ru}.json`, встроенных в бинарь, по тем же ключам, что у экранов
//! и кнопок в интерфейсе. Своих строк на языке интерфейса в Rust нет. Язык — действующий язык интерфейса
//! (`settings::ui_language`) на момент начала сессии MCP, `prompts/get` или старта фонового запуска. Английский интерфейс
//! глоссария не получает.

use std::sync::LazyLock;

use glafira_pg_mcp::server::glossary::UiGlossary;
use serde_json::Value;

use super::texts::{en, lookup};

/// Словарь ru интерфейса, встроенный в бинарь.
static RU: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/locales/ru.json"))
        .expect("src/locales/ru.json is valid JSON")
});

/// Ключи словаря у экранов и кнопок, которые тексты для модели называют в “…”: дерево «Диагностики» и заголовки
/// инструментов (`diag.tools.*`), кнопки «Сессий», консоль EXPLAIN, «Журнал действий», кнопка «Передать
/// LLM-ассистенту», панель «Предложения LLM» и «Сравнить планы». Полноту проверяет тест
/// `every_quoted_label_of_the_texts_has_a_pair`.
pub const LABEL_KEYS: [&str; 18] = [
    "diag.tools.pulse",
    "diag.tools.sessions",
    "actions.buttons.cancel",
    "actions.buttons.terminate",
    "diag.tools.idleTx",
    "diag.tools.waits",
    "diag.tools.locks",
    "diag.tools.horizon",
    "diag.tools.replication",
    "diag.tools.slots",
    "diag.tools.failover",
    "diag.tools.health",
    "diag.tools.queries",
    "diag.explain.title",
    "actions.journal.title",
    "llmAssist.ask.button",
    "llmAssist.panel.title",
    "llmAssist.panel.compare",
];

/// Язык интерфейса по-английски для текстов модели; `None` — английский или язык без словаря.
pub fn language_name(code: &str) -> Option<&'static str> {
    match code {
        "ru" => Some("Russian"),
        _ => None,
    }
}

fn dictionary(code: &str) -> Option<&'static Value> {
    match code {
        "ru" => Some(&RU),
        _ => None,
    }
}

/// Глоссарий для языка интерфейса `ui_language`; у английского (и языка без словаря) его нет.
pub fn glossary(ui_language: &str) -> Option<UiGlossary> {
    let language = language_name(ui_language)?;
    let dict = dictionary(ui_language)?;
    let labels = LABEL_KEYS
        .iter()
        .filter_map(|key| Some((en(key)?.to_string(), lookup(dict, key)?.to_string())))
        .collect();
    Some(UiGlossary {
        language: language.to_string(),
        labels,
    })
}

#[cfg(test)]
mod tests {
    use glafira_pg_mcp::server::glossary::{quoted_labels, with_glossary};
    use glafira_pg_mcp::server::prompts::{
        analyze_top_queries_text, check_run_text, optimize_query_run_text, optimize_query_text,
        review_config_text, triage_incident_text,
    };
    use glafira_pg_mcp::server::{INSTRUCTIONS, INSTRUCTIONS_RUN};

    use super::*;

    /// Словарь интерфейса, прочитанный тестом с диска (не встроенный в бинарь).
    fn locale(code: &str) -> Value {
        let path = format!("{}/../src/locales/{code}.json", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn at<'a>(v: &'a Value, key: &str) -> &'a str {
        key.split('.')
            .fold(v, |v, part| &v[part])
            .as_str()
            .unwrap_or_else(|| panic!("{key} нет в словаре"))
    }

    /// Все тексты для модели, которые называют экраны: instructions обеих сессий, четыре сценария, промпт фонового
    /// запуска (и «Проверить» — без подписей).
    fn texts() -> Vec<String> {
        vec![
            INSTRUCTIONS.to_string(),
            INSTRUCTIONS_RUN.to_string(),
            optimize_query_text(None),
            triage_incident_text("c1", None),
            analyze_top_queries_text("c1", Some("pg-2"), 900),
            review_config_text("c1", None),
            optimize_query_run_text("r3a9f1c", "Russian"),
            check_run_text(),
        ]
    }

    #[test]
    fn russian_labels_come_from_the_dictionaries_of_the_interface() {
        let (en_json, ru_json) = (locale("en"), locale("ru"));
        let g = glossary("ru").unwrap();
        assert_eq!(g.language, "Russian");
        assert_eq!(
            g.labels.len(),
            LABEL_KEYS.len(),
            "английские подписи не повторяются"
        );
        for key in LABEL_KEYS {
            assert_eq!(
                g.labels.get(at(&en_json, key)).map(String::as_str),
                Some(at(&ru_json, key)),
                "{key}"
            );
        }
        assert_eq!(g.labels["Wait profile"], "Профиль ожиданий");
        assert_eq!(g.labels["Send to LLM assistant"], "Передать LLM-ассистенту");
    }

    #[test]
    fn the_english_interface_has_no_glossary() {
        assert_eq!(glossary("en"), None);
        assert_eq!(glossary("de"), None);
        assert_eq!(
            (language_name("ru"), language_name("en")),
            (Some("Russian"), None)
        );
    }

    /// Каждая подпись в “…” каждого текста для модели — с парой в глоссарии, и абзац текста называет её подпись из
    /// `ru.json`; английские подписи в тексте остаются.
    #[test]
    fn every_quoted_label_of_the_texts_has_a_pair() {
        let ru_json = locale("ru");
        let g = glossary("ru").unwrap();
        let by_en: std::collections::BTreeMap<&str, &str> = LABEL_KEYS
            .iter()
            .map(|k| (en(k).unwrap(), at(&ru_json, k)))
            .collect();
        let mut named = std::collections::BTreeSet::new();
        for text in texts() {
            let out = with_glossary(text.clone(), Some(&g));
            assert!(out.starts_with(&text));
            for label in quoted_labels(&text) {
                let ru = by_en
                    .get(label)
                    .unwrap_or_else(|| panic!("“{label}” нет в глоссарии"));
                assert!(
                    out.contains(&format!("“{label}” — «{ru}»")),
                    "{label}: {out}"
                );
                named.insert(label.to_string());
            }
            assert!(!out.contains("Answer the user in"), "{out}");
        }
        assert_eq!(
            named.len(),
            LABEL_KEYS.len(),
            "в глоссарии нет лишних подписей: {named:?}"
        );
    }
}
