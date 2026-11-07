//! Промпт запуска (спец. P4c §4.3): текст — по-английски из `glafira-pg-mcp` (тексты для модели не локализуются); итог
//! агент пишет на языке интерфейса Glafira. Интерфейс не на английском — с подписями интерфейса (смоук 2026-11-07, R8).

use glafira_pg_mcp::server::glossary::with_glossary;
use glafira_pg_mcp::server::prompts::{check_run_text, optimize_query_run_text};

use crate::mcp::glossary::{glossary, language_name};

/// Язык итога агента по языку интерфейса Glafira.
pub fn summary_language(ui_language: &str) -> &'static str {
    language_name(ui_language).unwrap_or("English")
}

/// Запуск по запросу на оптимизацию: шаги `optimize_query` с явным `requestId`, итог — на языке интерфейса, подписи
/// экранов — как в интерфейсе (`mcp::glossary`).
pub fn run_prompt(request_id: &str, ui_language: &str) -> String {
    with_glossary(
        optimize_query_run_text(request_id, summary_language(ui_language)),
        glossary(ui_language).as_ref(),
    )
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
        // Смоук 2026-11-07, R8: при русском интерфейсе — подписи экранов, где пользователь сравнивает планы.
        assert_eq!(
            p,
            with_glossary(
                optimize_query_run_text("r3a9f1c", "Russian"),
                glossary("ru").as_ref()
            )
        );
        assert!(
            p.contains("“LLM proposals” — «Предложения LLM»")
                && p.contains("“Compare plans” — «Сравнить планы»"),
            "{p}"
        );
        assert_eq!(
            run_prompt("r3a9f1c", "en"),
            optimize_query_run_text("r3a9f1c", "English"),
            "английский интерфейс — без глоссария"
        );
        assert!(!check_prompt().is_empty());
        assert_ne!(check_prompt(), p);
    }
}
