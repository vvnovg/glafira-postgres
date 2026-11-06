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
/// «действия в Glafira». Подписи в “…” — точные английские строки `src/locales/en.json` (дерево диагностики, карточка
/// запроса, консоль EXPLAIN, панель предложений); тест `quoted_labels_are_the_english_ui_labels` сверяет их со словарём.
pub const GLAFIRA_SCREENS: &str = "When you tell the user what to do in Glafira, name its screens and buttons, never tool names. Screens behind the tools: get_health_summary → “Pulse”; sessions and get_activity → “Sessions” (with the buttons “Cancel query…” and “Terminate session…”) and “Idle in transaction”; waits and observe_activity → “Wait profile”; locks → “Locks”; horizon → “xmin horizon”; replication → “Replicas”; slots → “Replication slots”; failover → “Failover readiness”; health → “Health matrix”; get_top_queries, observe_top_queries and get_query_details → “Queries” and the query card that opens from its row; explain_query and hypothetical_index → “EXPLAIN console”; the user's own actions are listed in “Actions journal”. To have a query optimized, the user clicks “Send to LLM assistant” in the EXPLAIN console or the query card; proposals appear in the “LLM proposals” panel of the EXPLAIN console with “Compare plans”.";

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

pub fn optimize_query_text(request_id: Option<&str>) -> String {
    let get = match request_id {
        Some(id) => format!("get_optimization_request with requestId \"{id}\""),
        None => "get_optimization_request without requestId: it returns the latest waiting request"
            .to_string(),
    };
    format!(
        "Optimize a PostgreSQL query that the user sent from Glafira with the “Send to LLM assistant” button.\n\n\
         1. Call {get}. It returns the query text and parameter values, the plan, statistics, the server version and \
         the DDL of the tables, as the user saw them.\n\
         2. Analyze the plan and the statistics: the most expensive nodes, wrong row estimates, scans and joins, sorts \
         and hashes that spill to disk, missing or unused indexes.\n\
         3. When needed, check hypotheses on the same connection and database: explain_query for plans without \
         execution, hypothetical_index for up to 5 CREATE INDEX statements via hypopg.\n\
         4. Call propose_query_rewrite with the requestId for each variant worth comparing (one or several): \
         rewrittenSql, indexes if any, rationale and expectedEffect. The rewritten query must return the same rows as \
         the original: Glafira does not check that.\n\
         5. In the chat, briefly explain each variant and ask the user to compare the plans in Glafira (the “LLM \
         proposals” panel of the EXPLAIN console, “Compare plans”) and to accept or reject them. list_proposals shows \
         the decision and the comparison. The user's decisions and comparisons change at any time: when asked about \
         them, call list_proposals again right before answering and never answer from an earlier result.\n\n\
         Glafira never runs the proposed SQL or creates the indexes, and nothing in the database can be changed through \
         this gateway. {ANSWER_BUDGET} {ANSWER_LANGUAGE}"
    )
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
        for text in all_texts()
            .iter()
            .map(String::as_str)
            .chain([GLAFIRA_SCREENS])
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

    #[test]
    fn every_text_asks_for_the_users_language() {
        for t in all_texts() {
            assert!(t.ends_with(ANSWER_LANGUAGE), "{t}");
        }
        assert!(review_config_text("c1", Some("pg-2")).contains("member \"pg-2\""));
    }
}
