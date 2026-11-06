//! Инструменты P4a (спец. P4 §5.2). Все только читают. Параметры проверяются в теле вызова (`validate`), то есть после
//! права и лимита частоты, как в Glafira for Kafka: неверные аргументы попадают в журнал с кодом `invalid_argument` и
//! до бэкенда не доходят. Описания — для LLM, по-английски.

use glafira_mcp_core::CallMeta;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, tool, tool_router};
use serde_json::json;

use super::{PgServer, answer, args, report_value, to_value, unexpected};
use crate::model::{PgObserve, PgOutput, PgRead};
use crate::params::{
    ActivityParams, ConnectionParams, ExplainParams, HypoIndexParams, OBSERVE_DEFAULT_S,
    ObserveActivityParams, ObserveQueriesParams, QueryDetailsParams, RunDiagnosticParams,
    SettingsParams, TOPOLOGY_EVENTS_MAX, TableDetailsParams, TopQueriesParams,
};

macro_rules! expect {
    ($out:expr, $variant:ident) => {
        match $out {
            PgOutput::$variant(v) => Ok(v),
            _ => Err(unexpected()),
        }
    };
}

#[tool_router(vis = "pub(crate)")]
impl PgServer {
    #[tool(
        description = "List PostgreSQL connections that the user opened for LLM assistants in Glafira: id, name, environment (local, dev, test, stage or prod), kind (server, hosts or patroni), folder path, read-only flag, database (the connection's default database, used when a tool's database is omitted), the effective llmAccess level and whether Glafira is connected now. Call it first and use the id in every other tool.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_connections(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "list_connections",
            args: json!({}),
            connection_id: None,
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let a = self.read_local(PgRead::ListConnections).await?;
                let list = expect!(a.body, Connections)?;
                Ok(answer(json!({ "connections": list }), a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Topology of a connection: members with role, state, host, port, timeline, replication lag in bytes, Patroni tags, pending restart and nofailover; for Patroni also maintenance mode and the scheduled switchover or restart; up to 50 recent topology events (leader changes, members going down or coming back) seen by Glafira while it runs.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_topology(
        &self,
        Parameters(p): Parameters<ConnectionParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_topology",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let a = self.read_db(PgRead::Topology(p.clone())).await?;
                let mut t = expect!(a.body, Topology)?;
                t.events.truncate(TOPOLOGY_EVENTS_MAX);
                Ok(answer(to_value(&t)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Quick health summary of a connection: the pulse (connections, transactions, WAL, replication, waits) and the findings of the fast diagnostics (sessions, locks, horizon, replication, slots; for Patroni also failover and health) ordered by severity. Diagnostics that failed or did not finish within 20 seconds are listed in unavailable. Start an incident investigation here.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_health_summary(
        &self,
        Parameters(p): Parameters<ConnectionParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_health_summary",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let a = self.read_db(PgRead::HealthSummary(p.clone())).await?;
                let h = expect!(a.body, HealthSummary)?;
                Ok(answer(to_value(&h)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "List Glafira's diagnostic tools for run_diagnostic: id, title, scope (node or cluster), whether the tool needs a Patroni cluster and which parameters it takes.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_diagnostics(
        &self,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "list_diagnostics",
            args: json!({}),
            connection_id: None,
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let a = self.read_local(PgRead::ListDiagnostics).await?;
                let list = expect!(a.body, Diagnostics)?;
                Ok(answer(json!({ "tools": list }), a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Run one Glafira diagnostic tool (ids from list_diagnostics) on a member or on the whole cluster and return its report: the tool data, findings with an English title and fix, and the parts that are unavailable with the reason. Without member the tool follows the leader (primary); cluster tools ignore member. windowS (300, 900 or 3600) applies to waits. A report over 100 KB is truncated: truncated is true and hint says how to narrow it.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn run_diagnostic(
        &self,
        Parameters(p): Parameters<RunDiagnosticParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "run_diagnostic",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::RunDiagnostic(p)).await?;
                let r = expect!(a.body, Report)?;
                Ok(answer(report_value(&r)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Current sessions from pg_stat_activity on a member (without member: the leader or primary), longest first, after filters: state, database, application, minimum duration in seconds and wait event type. limit 1..=500, default 100; total is the number of sessions after filters. At the diagnostics level literals in query texts are masked.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_activity(
        &self,
        Parameters(p): Parameters<ActivityParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_activity",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::Activity(p)).await?;
                let out = expect!(a.body, Activity)?;
                Ok(answer(to_value(&out)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Watch the activity of a member for 10..=50 seconds (default 30) through Glafira's activity sampler: the wait profile, top queries by samples, sessions that appeared and disappeared, peak sessions and lock waits. Sends progress notifications; cancelling the call ends the observation early with what was collected. One observation per session at a time.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn observe_activity(
        &self,
        Parameters(p): Parameters<ObserveActivityParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "observe_activity",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let seconds = p.duration_seconds.unwrap_or(OBSERVE_DEFAULT_S);
                let a = self.observe(PgObserve::Activity(p), seconds, &ctx).await?;
                let o = expect!(a.body, ObservedActivity)?;
                Ok(answer(to_value(&o)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Top queries from pg_stat_statements on a member. mode sinceReset (the default without windowS) gives totals since the last statistics reset; mode window (the default when windowS is given) gives the last windowS seconds (300, 900 or 3600, default 900) from Glafira's buffer, which fills while the Queries tab or a query card is open in Glafira; without the buffer the coverage is 0, then use observe_top_queries or sinceReset. sort: totalTime (default), meanTime, maxTime, stddevTime, calls, rowsPerCall, sharedRead, hitRatio, tempWritten, walBytes, ioTime, planTime, jitTime or cpuTime. limit 1..=100, default 10; filters database (a database that does not exist on the member gives not_found with the list of databases) and search (a case-insensitive substring of the query text). At the diagnostics access level search matches the masked text ($N in place of literals) within the top 200 queries by the chosen sort, so an empty result may mean the query is outside that window.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_top_queries(
        &self,
        Parameters(p): Parameters<TopQueriesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_top_queries",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::TopQueries(p)).await?;
                let r = expect!(a.body, Report)?;
                Ok(answer(report_value(&r)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Measure the top queries over the next 10..=50 seconds (default 30): reads pg_stat_statements at the start and at the end of the window and returns the difference in the form of get_top_queries, with sort and limit as there. Sends progress notifications; cancelling the call ends the observation early. One observation per session at a time.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn observe_top_queries(
        &self,
        Parameters(p): Parameters<ObserveQueriesParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "observe_top_queries",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let seconds = p.duration_seconds.unwrap_or(OBSERVE_DEFAULT_S);
                let a = self
                    .observe(PgObserve::TopQueries(p), seconds, &ctx)
                    .await?;
                let r = expect!(a.body, ObservedQueries)?;
                Ok(answer(report_value(&r)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Card of one query by queryId from get_top_queries: text, metrics since the statistics reset and for the window, history, running instances (the 20 longest-running; runningTotal is the full count when there are more), waits, tables and findings. database is the database of the query (default: the connection's database).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_query_details(
        &self,
        Parameters(p): Parameters<QueryDetailsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_query_details",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::QueryDetails(p)).await?;
                let v = expect!(a.body, QueryDetails)?;
                Ok(answer(v, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Table details: columns, indexes, constraints, statistics, partitions (up to 200, also in the DDL comment; partitioning.partitionsTotal is the full count when there are more), triggers and DDL, plus pg_stats per column in columnStats (null fraction, average width, distinct values, correlation; at the data access level also most common values and histogram bounds).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_table_details(
        &self,
        Parameters(p): Parameters<TableDetailsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_table_details",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: None,
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::TableDetails(p)).await?;
                let t = expect!(a.body, Table)?;
                Ok(answer(to_value(&t)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Server settings from pg_settings on a member: name, value, unit, source, context and pending restart. By default only settings changed from their defaults (changedOnly true); names (up to 200) returns exactly these settings. Rows with source client or session (returned when asked by names) show the settings of Glafira's own connection, for example statement_timeout, lock_timeout, search_path, DateStyle or default_transaction_read_only, not the values configured on the server. Passwords and secrets in connection strings, *_command settings and secret-named settings are removed or replaced with ***, and such settings have hidden: true.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_settings(
        &self,
        Parameters(p): Parameters<SettingsParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "get_settings",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::Settings(p)).await?;
                let list = expect!(a.body, Settings)?;
                Ok(answer(json!({ "settings": list }), a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Build the plan of a query without executing it (EXPLAIN with FORMAT JSON, VERBOSE and SETTINGS, never ANALYZE). Pass sql or queryId (its text from pg_stat_statements). $1, $2 and so on take values from params; for queryId without params PostgreSQL 16+ builds a generic plan, older versions need params. Allowed: one SELECT, INSERT, UPDATE, DELETE or MERGE statement; nothing is executed. Returns the plan as psql text, the cost summary, Glafira's recommendations, tables and warnings.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn explain_query(
        &self,
        Parameters(p): Parameters<ExplainParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "explain_query",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::Explain(p)).await?;
                let e = expect!(a.body, Explain)?;
                Ok(answer(to_value(&e)?, a.masked, a.rows))
            })
            .await)
    }

    #[tool(
        description = "Check whether hypothetical indexes would help a query, with the hypopg extension: pass sql or queryId (and params, as in explain_query) and 1..=5 CREATE INDEX statements without CONCURRENTLY or UNIQUE (an index name, if given, is ignored). Returns the plan cost before and after, whether each index is used and its estimated size, and the new plan. Nothing is created in the database. Needs hypopg on the server.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn hypothetical_index(
        &self,
        Parameters(p): Parameters<HypoIndexParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let meta = CallMeta {
            tool: "hypothetical_index",
            args: args(&p),
            connection_id: Some(&p.connection_id),
            member: p.member.as_deref(),
        };
        Ok(self
            .ctx
            .call(&ctx, meta, async {
                let p = p.clone().validate()?;
                let a = self.read_db(PgRead::HypoIndex(p)).await?;
                let h = expect!(a.body, Hypo)?;
                Ok(answer(to_value(&h)?, a.masked, a.rows))
            })
            .await)
    }
}
