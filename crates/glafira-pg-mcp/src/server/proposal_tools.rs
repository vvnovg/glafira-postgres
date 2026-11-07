//! Инструменты предложений (спец. P4 §8.2). Базу они не меняют: запрос на оптимизацию — снимок, который отправил
//! пользователь; предложение пишется в Glafira, и человек сравнивает планы и решает сам. Проверки SQL, доступ и
//! хранилища — в приложении (`PgBackend::llm`); здесь — параметры, слот базы у предложения и вид ответа. У сессии
//! фонового запуска (P4c §5.2) в операцию идёт её `run`; область запроса проверяет приложение.

use glafira_mcp_core::{CallMeta, CallOk, ToolError};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, tool, tool_router};
use serde_json::json;

use super::{PgServer, args, to_value, unexpected};
use crate::model::{LlmOp, PgAnswer, PgOutput};
use crate::params::{GetRequestParams, ListProposalsParams, ProposeParams};

/// Подключение для журнала: пустая строка — «не задано» (E2), как у проверки параметров.
fn connection_of(id: Option<&str>) -> Option<&str> {
    id.filter(|s| !s.trim().is_empty())
}

#[tool_router(router = proposal_router, vis = "pub(crate)")]
impl PgServer {
    #[tool(
        description = "Get an optimization request the user sent from Glafira with the “Send to LLM assistant” button (EXPLAIN console or query card): the query text and parameter values, the plan, statistics, server version and table DDL as the user saw them. The snapshot is not masked: the user sent it. Without requestId returns the latest waiting request among connections open to you. Parts of a large snapshot (table DDL from the end, the end of the plan text) may be omitted to fit the response: hint says what. Then analyze it, check ideas with explain_query and hypothetical_index on the same connection, database and member, and answer with propose_query_rewrite (requestId). A request the user cancelled answers request_closed.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_optimization_request(
        &self,
        Parameters(p): Parameters<GetRequestParams>,
        rctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_optimization_request",
            args: args(&p),
            connection_id: None,
            member: None,
        };
        let body = async {
            let params = p.clone().validate()?;
            let op = LlmOp::GetRequest {
                params,
                run: self.run_id(),
            };
            llm_ok(self.backend.llm(op).await?)
        };
        Ok(self.ctx.call(&rctx, meta, body).await)
    }

    #[tool(
        description = "Propose a rewritten query and optional indexes for the user to compare in Glafira. Nothing is executed: Glafira checks that the SQL parses and plans on the server (errors come back as invalid_sql with the server's text: fix and retry) and shows the proposal next to the original; the user compares the plans (EXPLAIN without execution, indexes as hypothetical hypopg indexes), then accepts or rejects it. Pass requestId from get_optimization_request (then only rewrittenSql, indexes, rationale and expectedEffect are needed), or connectionId and sql (or queryId from get_top_queries) for a query you found yourself; database defaults to the connection's database. rewrittenSql must be one statement of the same kind as the original (SELECT stays SELECT) and must return the same rows: Glafira does not check that. indexes: up to 5 CREATE INDEX statements without CONCURRENTLY or UNIQUE. rationale (up to 4 KB) and expectedEffect (up to 1 KB) are plain text for the user. Repeating the same proposal returns the existing one; at most 5 proposals per request. See the user's decision and the plan comparison with list_proposals.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn propose_query_rewrite(
        &self,
        Parameters(p): Parameters<ProposeParams>,
        rctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let client = self.ctx.identify(rctx.client_info()).client.map(|c| c.name);
        let meta = CallMeta {
            tool: "propose_query_rewrite",
            args: args(&p),
            connection_id: connection_of(p.connection_id.as_deref()),
            member: None,
        };
        let body = async {
            let params = p.clone().validate()?;
            // Проверка предложения ходит в базу (`prepare` и проба плана на сервере) — слот, как у чтения.
            let _slots = self.ctx.db_slots().await?;
            let op = LlmOp::Propose {
                params,
                client,
                run: self.run_id(),
            };
            llm_ok(self.backend.llm(op).await?)
        };
        Ok(self.ctx.call(&rctx, meta, body).await)
    }

    #[tool(
        description = "List your query rewrite proposals with the user's decision: status new, accepted or rejected (reason when the user gave one), and comparison: the result of “Compare plans” if the user ran it (costBefore, costAfter, indexesUsed, member, withoutHypopg). Filter by connectionId or requestId; newest first. Check it after proposing instead of asking the user in the chat. The user's decisions and comparisons change at any time: call list_proposals again right before answering about them, never answer from an earlier result.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_proposals(
        &self,
        Parameters(p): Parameters<ListProposalsParams>,
        rctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "list_proposals",
            args: args(&p),
            connection_id: connection_of(p.connection_id.as_deref()),
            member: None,
        };
        let body = async {
            let params = p.clone().validate()?;
            let op = LlmOp::ListProposals {
                params,
                run: self.run_id(),
            };
            llm_ok(self.backend.llm(op).await?)
        };
        Ok(self.ctx.call(&rctx, meta, body).await)
    }
}

/// Ответ бэкенда → результат инструмента; список — объектом (ответ инструмента всегда JSON-объект).
fn llm_ok(answer: PgAnswer) -> Result<CallOk, ToolError> {
    let value = match answer.body {
        PgOutput::Request(v) => v,
        PgOutput::Proposed(o) => to_value(&o)?,
        PgOutput::Proposals(items) => json!({ "proposals": items }),
        _ => return Err(unexpected()),
    };
    Ok(CallOk::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProposedOut;

    #[test]
    fn backend_answers_become_objects() {
        let ok = llm_ok(PgAnswer::new(PgOutput::Proposals(vec![
            json!({"proposalId": "p1"}),
        ])))
        .unwrap();
        assert_eq!(ok.value, json!({ "proposals": [{ "proposalId": "p1" }] }));
        let ok = llm_ok(PgAnswer::new(PgOutput::Proposed(ProposedOut {
            proposal_id: "p1".into(),
            status: "accepted".into(),
        })))
        .unwrap();
        assert_eq!(
            ok.value,
            json!({ "proposalId": "p1", "status": "accepted" })
        );
        let e = llm_ok(PgAnswer::new(PgOutput::Diagnostics(vec![]))).unwrap_err();
        assert_eq!(e.code, "internal");
        assert_eq!(connection_of(Some(" ")), None);
        assert_eq!(connection_of(Some("c1")), Some("c1"));
    }
}
