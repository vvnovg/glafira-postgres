//! Промпт запуска (спец. P4c §4.3): текст — по-английски из `glafira-pg-mcp` (тексты для модели не локализуются); итог
//! агент пишет на языке интерфейса Glafira.

use glafira_pg_mcp::server::prompts::{check_run_text, optimize_query_run_text};

/// Язык итога агента по языку интерфейса Glafira.
pub fn summary_language(ui_language: &str) -> &'static str {
    if ui_language == "ru" {
        "Russian"
    } else {
        "English"
    }
}

/// Запуск по запросу на оптимизацию: шаги `optimize_query` с явным `requestId`, итог — на языке интерфейса.
pub fn run_prompt(request_id: &str, ui_language: &str) -> String {
    optimize_query_run_text(request_id, summary_language(ui_language))
}

/// «Проверить» (спец. §8.3): вызвать `list_connections` и ответить OK.
pub fn check_prompt() -> String {
    check_run_text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_names_the_request_and_the_language_of_the_summary() {
        assert_eq!(
            (
                summary_language("ru"),
                summary_language("en"),
                summary_language("de")
            ),
            ("Russian", "English", "English")
        );
        let p = run_prompt("r3a9f1c", "ru");
        assert!(p.contains("r3a9f1c") && p.contains("Russian"), "{p}");
        assert!(run_prompt("r3a9f1c", "en").contains("English"));
        assert!(!check_prompt().is_empty());
        assert_ne!(check_prompt(), p);
    }
}
