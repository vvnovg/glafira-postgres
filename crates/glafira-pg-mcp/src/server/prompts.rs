//! MCP prompts (спец. P4 §9): сценарии для слэш-команд клиентов («MCP: glafira-pg» в Qwen Code). Тексты — по-английски
//! с просьбой отвечать на языке пользователя; аргументы prompts в MCP — строки. Данных prompts не читают: сценарий
//! выполняет модель вызовами инструментов, у которых свои проверки доступа.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{GetPromptResult, PromptMessage, Role};
use rmcp::{ErrorData, prompt, prompt_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::PgServer;
use crate::params::{DEFAULT_WINDOW_S, WINDOWS_S};

const MAX_ID_CHARS: usize = 128;
const MAX_SYMPTOM_CHARS: usize = 1_000;
const ANSWER_LANGUAGE: &str = "Answer in the language the user writes in.";
/// Объём ответа (смоук P4b, M3): ответ сценария был очень длинным, а у локальной модели с контекстом 64k не хватило
/// места.
pub const ANSWER_BUDGET: &str = "Keep the answer short: start with a summary of up to about 10 lines (what is happening or the main problem, the likely cause, the next action) and give details only when the user asks for them.";
/// Инструменты → экраны и кнопки Glafira (смоук P4b, M4): модель перечисляла пользователю имена инструментов как
/// «действия в Glafira». Карта (`screens_map!`, та же — в `INSTRUCTIONS`): подписи в “…” — точные английские строки
/// `src/locales/en.json` (дерево диагностики, карточка запроса, консоль EXPLAIN, панель предложений); тест
/// `quoted_labels_are_the_english_ui_labels` сверяет их со словарём.
pub const GLAFIRA_SCREENS: &str = concat!(
    "When you tell the user what to do in Glafira, name its screens and buttons, never tool names. ",
    screens_map!()
);

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OptimizeQueryArgs {
    /// Request id from Glafira (r + 6 hex), as copied by the “Send to LLM assistant” button; omit to take the latest waiting request.
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TriageIncidentArgs {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// What the user sees, in their own words.
    #[serde(default)]
    pub symptom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeTopQueriesArgs {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Node name; default — the leader.
    #[serde(default)]
    pub member: Option<String>,
    /// Window in seconds: 300, 900 (default) or 3600.
    #[serde(default)]
    pub window_s: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewConfigArgs {
    /// Connection id from list_connections.
    pub connection_id: String,
    /// Node name; default — the leader.
    #[serde(default)]
    pub member: Option<String>,
}

fn invalid(message: String) -> ErrorData {
    ErrorData::invalid_params(message, None)
}

/// Идентификатор из аргумента (id запроса или подключения, имя узла): одна строка не длиннее `MAX_ID_CHARS`, без
/// управляющих символов — в текст prompt не попадёт ничего, кроме имени. Пустой — как отсутствующий.
fn ident(name: &str, v: Option<&str>) -> Result<Option<String>, ErrorData> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > MAX_ID_CHARS || v.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{name} must be a single line of at most {MAX_ID_CHARS} characters"
        )));
    }
    Ok(Some(v.to_string()))
}

fn required(name: &str, v: &str) -> Result<String, ErrorData> {
    ident(name, Some(v))?.ok_or_else(|| invalid(format!("{name} is required")))
}

fn window(v: Option<&str>) -> Result<u32, ErrorData> {
    match v.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(DEFAULT_WINDOW_S),
        Some(s) => s
            .parse::<u32>()
            .ok()
            .filter(|w| WINDOWS_S.contains(w))
            .ok_or_else(|| invalid("windowS must be 300, 900 or 3600".to_string())),
    }
}

/// Симптом — слова пользователя: без управляющих символов (кроме перевода строки), не длиннее `MAX_SYMPTOM_CHARS`.
fn symptom(v: Option<&str>) -> Option<String> {
    let s: String = v?
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(MAX_SYMPTOM_CHARS)
        .collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn node(member: Option<&str>) -> String {
    match member {
        Some(m) => format!("node \"{m}\""),
        None => "the leader node".to_string(),
    }
}

fn member_arg(member: Option<&str>) -> String {
    member
        .map(|m| format!(", member \"{m}\""))
        .unwrap_or_default()
}

/// Шаги 1–4 сценария оптимизации — общие для `optimize_query` и промпта фонового запуска (P4c §4.3); `get` — начало
/// шага 1. Строка заканчивается переводом строки.
fn optimize_steps(get: &str) -> String {
    format!(
        "1. Call {get}. It returns the query text and parameter values, the plan, statistics, the server version and \
         the DDL of the tables, as the user saw them.\n\
         2. Analyze the plan and the statistics: the most expensive nodes, wrong row estimates, scans and joins, sorts \
         and hashes that spill to disk, missing or unused indexes.\n\
         3. When needed, check hypotheses on the same connection and database: explain_query for plans without \
         execution, hypothetical_index for up to 5 CREATE INDEX statements via hypopg.\n\
         4. Call propose_query_rewrite with the requestId for each variant worth comparing (one or several): \
         rewrittenSql, indexes if any, rationale and expectedEffect. The rewritten query must return the same rows as \
         the original: Glafira does not check that.\n"
    )
}

pub fn optimize_query_text(request_id: Option<&str>) -> String {
    let get = match request_id {
        Some(id) => format!("get_optimization_request with requestId \"{id}\""),
        None => "get_optimization_request without requestId: it returns the latest waiting request"
            .to_string(),
    };
    let steps = optimize_steps(&get);
    format!(
        "Optimize a PostgreSQL query that the user sent from Glafira with the “Send to LLM assistant” button.\n\n\
         {steps}\
         5. In the chat, briefly explain each variant and ask the user to compare the plans in Glafira (the “LLM \
         proposals” panel of the EXPLAIN console, “Compare plans”) and to accept or reject them. list_proposals shows \
         the decision and the comparison. The user's decisions and comparisons change at any time: when asked about \
         them, call list_proposals again right before answering and never answer from an earlier result.\n\n\
         Glafira never runs the proposed SQL or creates the indexes, and nothing in the database can be changed through \
         this gateway. {ANSWER_BUDGET} {ANSWER_LANGUAGE}"
    )
}

/// Промпт фонового запуска (спец. P4c §4.3): Glafira подаёт его агенту через stdin.
/// - Шаги 1–4 — те же, что у `optimize_query`, с явным `requestId`.
/// - После шагов — абзац о `requestId` (смоук P4b M5/M6, итог задачи 1 P4c): модели путали id запроса с `queryId` и
///   слали в `propose_query_rewrite` лишние поля.
/// - Чата нет: агент не задаёт вопросов; если улучшить нечего, ничего не предлагает.
/// - Последнее сообщение — только итог (смоук 2026-11-07, R16): первая строка ровно `SUMMARY` (по ней Glafira отделяет
///   итог от рассуждений, `agent::process::summary_part` приложения), дальше до 5 коротких строк на языке интерфейса
///   Glafira (`language`: `Russian` / `English`) простым текстом: id предложений или почему их нет и следующий шаг
///   пользователя (подписи экранов в “…” — для глоссария R8).
pub fn optimize_query_run_text(request_id: &str, language: &str) -> String {
    let steps = optimize_steps(&format!(
        "get_optimization_request with requestId \"{request_id}\""
    ));
    format!(
        "Glafira started you in the background: the user pressed “Send to LLM assistant” in Glafira for optimization \
         request \"{request_id}\". Nobody reads this chat: do not ask questions and do not wait for an answer.\n\n\
         Optimize the PostgreSQL query of this request.\n\n\
         {steps}\n\
         explain_query and hypothetical_index take the query text in sql and its parameter values in params, never the \
         requestId. propose_query_rewrite with the requestId needs only rewrittenSql, indexes, rationale and \
         expectedEffect.\n\n\
         If the query cannot be improved, propose nothing: a proposal that is not better wastes the user's time.\n\n\
         Your last message is only the summary for the user (Glafira shows it as the assistant's answer), and its \
         first line is exactly SUMMARY. Then at most 5 short lines in {language}, plain text without Markdown (no **, \
         no backticks, no headings, no tables, no code blocks): the ids of the proposals you made and what each one \
         changes, or why there is no proposal; and the next step for the user, for example: compare the plans in the \
         “LLM proposals” panel of the “EXPLAIN console” with “Compare plans” and accept or reject the proposal. Do not \
         put your reasoning in the last message.\n\n\
         Glafira never runs the proposed SQL or creates the indexes, and nothing in the database can be changed \
         through this gateway."
    )
}

/// Промпт «Проверить» (спец. P4c §8.3): короткий запуск без подключения. Проверяет программу, модель, MCP-конфиг и
/// билет.
pub fn check_run_text() -> String {
    "Glafira started you in the background to check the setup: nobody reads this chat. Call list_connections \
     once (the list may be empty), then answer with the single word OK. Do not call other tools."
        .to_string()
}

pub fn triage_incident_text(connection_id: &str, symptom: Option<&str>) -> String {
    let reported = match symptom {
        Some(s) => format!("The user reports: {s}"),
        None => "The user did not describe a symptom: find out what is wrong.".to_string(),
    };
    format!(
        "Triage an incident on the PostgreSQL connection with connectionId \"{connection_id}\" in Glafira. {reported}\n\n\
         1. Call get_health_summary with connectionId \"{connection_id}\": the pulse and the findings of the quick \
         checks of sessions, locks, horizon, replication and slots. Prefer its findings to new calls.\n\
         2. Only for the findings that need details, call run_diagnostic with the matching tool: locks (lock waits and \
         blockers), horizon (who holds the xmin horizon), replication (lag), slots (replication slots) and, for a \
         Patroni cluster, failover and health; at most 3 run_diagnostic calls before the summary. list_diagnostics \
         lists the tools and their parameters.\n\
         3. If there is load right now, call observe_activity for 30 seconds to see the waits and the top queries \
         during the window.\n\
         4. Summarize: what is happening, the likely causes ranked by evidence, what to do in Glafira (which screen and \
         which button) and what to check outside Glafira (server logs, OS metrics, the application). The tools cannot \
         change anything: the user acts in Glafira.\n\n\
         {GLAFIRA_SCREENS}\n\n\
         {ANSWER_BUDGET} {ANSWER_LANGUAGE}"
    )
}

pub fn analyze_top_queries_text(
    connection_id: &str,
    member: Option<&str>,
    window_s: u32,
) -> String {
    let node = node(member);
    let member_arg = member_arg(member);
    format!(
        "Analyze the top queries of the PostgreSQL connection with connectionId \"{connection_id}\" in Glafira ({node}) \
         over the last {window_s} seconds.\n\n\
         1. Call get_top_queries with connectionId \"{connection_id}\"{member_arg}, mode \"window\", windowS \
         {window_s} and limit 10. If the window has no coverage (the sampler buffer is empty), call \
         observe_top_queries for 50 seconds with limit 10, or get_top_queries with mode \"sinceReset\" and limit 10.\n\
         2. Pick at most 3 queries that matter most (total time, calls, rows, I/O). Look at them one query at a time, \
         not in parallel: call get_query_details and explain_query with its queryId before the next query.\n\
         3. Give recommendations per query: indexes (check them with hypothetical_index), rewrites, settings. For the \
         candidates for a rewrite, call propose_query_rewrite with connectionId and queryId (database defaults to the \
         connection's database) so that the user can compare the plans in Glafira.\n\n\
         {GLAFIRA_SCREENS}\n\n\
         {ANSWER_BUDGET} {ANSWER_LANGUAGE}"
    )
}

pub fn review_config_text(connection_id: &str, member: Option<&str>) -> String {
    let node = node(member);
    let member_arg = member_arg(member);
    format!(
        "Review the PostgreSQL configuration of the connection with connectionId \"{connection_id}\" in Glafira \
         ({node}).\n\n\
         1. Call get_settings with connectionId \"{connection_id}\"{member_arg} for the changed settings, then \
         get_settings with names for the key settings that may still be at their defaults: shared_buffers, \
         effective_cache_size, work_mem, maintenance_work_mem, max_connections, checkpoint_timeout, max_wal_size, \
         min_wal_size, checkpoint_completion_target, wal_compression, autovacuum and its thresholds and cost limits, \
         statement_timeout, idle_in_transaction_session_timeout, lock_timeout, track_io_timing, jit, \
         max_worker_processes, max_parallel_workers, max_parallel_workers_per_gather.\n\
         2. Take the number of connections and the load into account: get_health_summary and get_activity.\n\
         3. Give recommendations with their risks and say whether a restart is needed (context postmaster). For a \
         Patroni cluster, settings are changed with patronictl edit-config, not in postgresql.conf. Glafira does not \
         change the configuration.\n\n\
         {GLAFIRA_SCREENS}\n\n\
         {ANSWER_BUDGET} {ANSWER_LANGUAGE}"
    )
}

fn user_prompt(description: &str, text: String) -> GetPromptResult {
    GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
        .with_description(description)
}

#[prompt_router(vis = "pub(crate)")]
impl PgServer {
    #[prompt(
        description = "Optimize a query the user sent from Glafira with “Send to LLM assistant”: read the request, check ideas with plans and hypothetical indexes, propose rewrites for the user to compare in Glafira."
    )]
    fn optimize_query(
        &self,
        Parameters(a): Parameters<OptimizeQueryArgs>,
    ) -> Result<GetPromptResult, ErrorData> {
        let id = ident("requestId", a.request_id.as_deref())?;
        Ok(user_prompt(
            "Optimize a query sent from Glafira",
            optimize_query_text(id.as_deref()),
        ))
    }

    #[prompt(
        description = "Triage an incident on a connection: health summary, diagnostics for the findings, a short observation of the activity, then causes and what to do in Glafira."
    )]
    fn triage_incident(
        &self,
        Parameters(a): Parameters<TriageIncidentArgs>,
    ) -> Result<GetPromptResult, ErrorData> {
        let id = required("connectionId", &a.connection_id)?;
        let symptom = symptom(a.symptom.as_deref());
        Ok(user_prompt(
            "Triage an incident",
            triage_incident_text(&id, symptom.as_deref()),
        ))
    }

    #[prompt(
        description = "Analyze the top queries of a connection over a window: details and plans of the heaviest ones, recommendations, rewrite proposals."
    )]
    fn analyze_top_queries(
        &self,
        Parameters(a): Parameters<AnalyzeTopQueriesArgs>,
    ) -> Result<GetPromptResult, ErrorData> {
        let id = required("connectionId", &a.connection_id)?;
        let member = ident("member", a.member.as_deref())?;
        let window_s = window(a.window_s.as_deref())?;
        Ok(user_prompt(
            "Analyze the top queries",
            analyze_top_queries_text(&id, member.as_deref(), window_s),
        ))
    }

    #[prompt(
        description = "Review the PostgreSQL settings of a connection against its load: recommendations with risks; Glafira does not change the configuration."
    )]
    fn review_config(
        &self,
        Parameters(a): Parameters<ReviewConfigArgs>,
    ) -> Result<GetPromptResult, ErrorData> {
        let id = required("connectionId", &a.connection_id)?;
        let member = ident("member", a.member.as_deref())?;
        Ok(user_prompt(
            "Review the configuration",
            review_config_text(&id, member.as_deref()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{RUN_TOOL_NAMES, TOOL_NAMES};

    #[test]
    fn arguments_are_checked() {
        assert_eq!(ident("x", None).unwrap(), None);
        assert_eq!(ident("x", Some("  ")).unwrap(), None);
        assert_eq!(ident("x", Some(" pg-1 ")).unwrap().as_deref(), Some("pg-1"));
        assert!(ident("x", Some("a\nb")).is_err());
        assert!(ident("x", Some(&"a".repeat(MAX_ID_CHARS + 1))).is_err());
        assert!(required("connectionId", " ").is_err());
        assert_eq!(window(None).unwrap(), 900);
        assert_eq!(window(Some("300")).unwrap(), 300);
        assert!(window(Some("60")).is_err());
        assert!(window(Some("15m")).is_err());
        assert_eq!(
            symptom(Some(" lag\u{1b}[31m grows ")).as_deref(),
            Some("lag[31m grows")
        );
        assert_eq!(
            symptom(Some("x".repeat(5000).as_str())).unwrap().len(),
            MAX_SYMPTOM_CHARS
        );
    }

    fn all_texts() -> [String; 4] {
        [
            optimize_query_text(None),
            triage_incident_text("c1", None),
            analyze_top_queries_text("c1", None, 900),
            review_config_text("c1", Some("pg-2")),
        ]
    }

    /// Подписи в “…”.
    fn quoted(text: &str) -> Vec<&str> {
        text.split('“')
            .skip(1)
            .filter_map(|part| part.split_once('”').map(|(label, _)| label))
            .collect()
    }

    fn strings(v: &serde_json::Value, out: &mut std::collections::HashSet<String>) {
        match v {
            serde_json::Value::String(s) => {
                out.insert(s.clone());
            }
            serde_json::Value::Object(m) => m.values().for_each(|x| strings(x, out)),
            serde_json::Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
            _ => {}
        }
    }

    /// Смоук P4b, M4: каждая подпись интерфейса в “…” в текстах сценариев и в карте экранов — точная строка словаря
    /// `src/locales/en.json` (словарь встроен в тест, как `mcp::texts` приложения встраивает его в бинарь). Подпись,
    /// переименованная в интерфейсе, роняет этот тест.
    #[test]
    fn quoted_labels_are_the_english_ui_labels() {
        let en: serde_json::Value =
            serde_json::from_str(include_str!("../../../../src/locales/en.json")).unwrap();
        let mut labels = std::collections::HashSet::new();
        strings(&en, &mut labels);
        let map = quoted(GLAFIRA_SCREENS);
        for label in [
            "Pulse",
            "Sessions",
            "Idle in transaction",
            "Wait profile",
            "Locks",
            "xmin horizon",
            "Replicas",
            "Replication slots",
            "Queries",
            "EXPLAIN console",
            "Actions journal",
            "Send to LLM assistant",
            "LLM proposals",
            "Compare plans",
        ] {
            assert!(map.contains(&label), "{label} нет в карте экранов");
        }
        // Тексты фонового запуска (P4c) — не сценарии `prompts/list` и в `all_texts()` не входят, но их подписи —
        // тоже строки словаря.
        let run_texts = [
            optimize_query_run_text("r3a9f1c", "English"),
            check_run_text(),
        ];
        for text in all_texts()
            .iter()
            .chain(&run_texts)
            .map(String::as_str)
            .chain([
                GLAFIRA_SCREENS,
                crate::server::INSTRUCTIONS,
                crate::server::INSTRUCTIONS_RUN,
            ])
        {
            for label in quoted(text) {
                assert!(labels.contains(label), "“{label}” нет в en.json");
            }
        }
    }

    /// Смоук P4b, M1, M3, M4: короткий итог во всех четырёх сценариях, карта экранов — в трёх, где модель советует
    /// действия; пределы вызовов у разбора топа и инцидента; свежий list_proposals перед ответом о решениях.
    #[test]
    fn scenarios_keep_the_answer_and_the_context_small() {
        for t in all_texts() {
            assert!(t.contains(ANSWER_BUDGET), "{t}");
        }
        let [optimize, triage, analyze, review] = all_texts();
        assert!(!optimize.contains(GLAFIRA_SCREENS));
        for t in [&triage, &analyze, &review] {
            assert!(t.contains(GLAFIRA_SCREENS), "{t}");
        }
        for phrase in [
            "change at any time",
            "call list_proposals again right before answering",
            "never answer from an earlier result",
        ] {
            assert!(optimize.contains(phrase), "{phrase}: {optimize}");
        }
        for phrase in [
            "Prefer its findings",
            "at most 3 run_diagnostic calls before the summary",
        ] {
            assert!(triage.contains(phrase), "{phrase}: {triage}");
        }
        for phrase in [
            "windowS 900 and limit 10",
            "at most 3 queries",
            "one query at a time, not in parallel",
        ] {
            assert!(analyze.contains(phrase), "{phrase}: {analyze}");
        }
        assert!(
            !analyze.contains("3–5"),
            "деталей — не больше трёх запросов: {analyze}"
        );
    }

    /// Текст `optimize_query` на `feat/p4b` 4068590 — до выноса шагов 1–4 в `optimize_steps` (план P4c, «Сверка с
    /// `feat/p4b` 4068590», задача 3). `{get}` — начало шага 1.
    const OPTIMIZE_QUERY_4068590: &str = concat!(
        "Optimize a PostgreSQL query that the user sent from Glafira with the “Send to LLM assistant” button.\n",
        "\n",
        "1. Call {get}. It returns the query text and parameter values, the plan, statistics, the server version and the DDL of the tables, as the user saw them.\n",
        "2. Analyze the plan and the statistics: the most expensive nodes, wrong row estimates, scans and joins, sorts and hashes that spill to disk, missing or unused indexes.\n",
        "3. When needed, check hypotheses on the same connection and database: explain_query for plans without execution, hypothetical_index for up to 5 CREATE INDEX statements via hypopg.\n",
        "4. Call propose_query_rewrite with the requestId for each variant worth comparing (one or several): rewrittenSql, indexes if any, rationale and expectedEffect. The rewritten query must return the same rows as the original: Glafira does not check that.\n",
        "5. In the chat, briefly explain each variant and ask the user to compare the plans in Glafira (the “LLM proposals” panel of the EXPLAIN console, “Compare plans”) and to accept or reject them. list_proposals shows the decision and the comparison. The user's decisions and comparisons change at any time: when asked about them, call list_proposals again right before answering and never answer from an earlier result.\n",
        "\n",
        "Glafira never runs the proposed SQL or creates the indexes, and nothing in the database can be changed through this gateway. ",
        "Keep the answer short: start with a summary of up to about 10 lines (what is happening or the main problem, the likely cause, the next action) and give details only when the user asks for them. ",
        "Answer in the language the user writes in.",
    );

    /// Шаги 1–4 вынесены в `optimize_steps`, а текст `optimize_query` остался байт в байт тем же.
    #[test]
    fn optimize_query_text_is_the_same_as_before_the_background_run() {
        for (id, get) in [
            (
                Some("r3a9f1c"),
                "get_optimization_request with requestId \"r3a9f1c\"",
            ),
            (
                None,
                "get_optimization_request without requestId: it returns the latest waiting request",
            ),
        ] {
            assert_eq!(
                optimize_query_text(id),
                OPTIMIZE_QUERY_4068590.replace("{get}", get)
            );
        }
    }

    /// Промпт фонового запуска (спец. P4c §4.3): шаги 1–4 — те же, что у `optimize_query`, с явным `requestId`.
    /// Чата нет. Итог — на языке интерфейса Glafira. Абзац о `requestId` (смоук P4b M5/M6, итог задачи 1): модели
    /// путали id запроса с `queryId` и слали лишние поля. «Проверить» (§8.3) — `list_connections` и ответ `OK`.
    /// Последнее сообщение — только итог (смоук 2026-11-07, R16): первая строка `SUMMARY`, до 5 коротких строк простым
    /// текстом — id предложений или почему их нет и следующий шаг пользователя; рассуждения — не в нём.
    #[test]
    fn background_run_prompts() {
        let steps = optimize_steps("get_optimization_request with requestId \"r3a9f1c\"");
        let chat = optimize_query_text(Some("r3a9f1c"));
        assert!(chat.starts_with(
            "Optimize a PostgreSQL query that the user sent from Glafira with the “Send to LLM assistant” button.\n\n1. Call "
        ));
        assert!(chat.contains(&steps), "{chat}");
        assert!(chat.contains(".\n5. In the chat"), "{chat}");
        let t = optimize_query_run_text("r3a9f1c", "Russian");
        assert!(t.contains(&steps), "шаги 1–4 — те же: {t}");
        for phrase in [
            "in the background",
            "do not ask questions",
            "propose nothing",
            "never the requestId",
            "needs only rewrittenSql",
            "Your last message is only the summary",
            "its first line is exactly SUMMARY",
            "at most 5 short lines in Russian",
            "plain text without Markdown",
            "the ids of the proposals",
            "the next step for the user",
            "Do not put your reasoning in the last message",
        ] {
            assert!(t.contains(phrase), "{phrase}: {t}");
        }
        // Следующий шаг называет экраны, где пользователь сравнивает планы: подписи — в глоссарии (R8).
        for label in ["LLM proposals", "EXPLAIN console", "Compare plans"] {
            assert!(quoted(&t).contains(&label), "{label}: {t}");
        }
        for phrase in ["never the requestId", "needs only rewrittenSql"] {
            assert!(
                !chat.contains(phrase),
                "абзац только у фонового запуска: {phrase}"
            );
        }
        assert!(!t.contains("5. In the chat"), "чат никто не читает");
        assert!(!t.contains(ANSWER_LANGUAGE), "язык итога задан явно");
        assert!(!t.contains(ANSWER_BUDGET), "объём итога задан явно");
        assert!(quoted(&t).contains(&"Send to LLM assistant"), "{t}");
        assert!(
            optimize_query_run_text("r3a9f1c", "English")
                .contains("at most 5 short lines in English")
        );
        let c = check_run_text();
        assert!(c.contains("list_connections") && c.contains("OK"), "{c}");
        // Фоновый агент видит только инструменты запуска: другие имена в его промптах — вызовы мимо.
        for text in [&t, &c] {
            for name in TOOL_NAMES.iter().filter(|n| !RUN_TOOL_NAMES.contains(n)) {
                assert!(!text.contains(*name), "{name}: {text}");
            }
        }
        let cyrillic = |s: &str| s.chars().any(|ch| ('\u{0400}'..='\u{04FF}').contains(&ch));
        assert!(!cyrillic(&t) && !cyrillic(&c));
    }

    #[test]
    fn every_text_asks_for_the_users_language() {
        for t in all_texts() {
            assert!(t.ends_with(ANSWER_LANGUAGE), "{t}");
        }
        assert!(review_config_text("c1", Some("pg-2")).contains("member \"pg-2\""));
    }
}
