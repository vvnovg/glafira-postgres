# LLM assistants (MCP)

Glafira can act as a gateway for an LLM assistant: Qwen Code, Claude Code, Claude Desktop, Cursor or another client that supports MCP (Model Context Protocol). The assistant reads the diagnostics of the connections you have opened to it — topology, tool reports with findings, activity, top queries, query cards, tables, server parameters, plans without execution — and checks index hypotheses with hypopg. You can hand it a slow query from the EXPLAIN console or a query card: its rewrites and indexes come back to a panel where you compare the plans (see [Optimization requests and proposals](llm-mcp.md#optimization)). On test stands you can also let it read data with `SELECT` (see [Diagnostics and data level](llm-mcp.md#data-level)). It cannot change a database through Glafira: you cancel and terminate sessions, drop slots, run switchovers and restarts, create indexes and change queries yourself, and the assistant tells you what to do and where.

## How it works {#how-it-works}

The client runs `glafira-pg mcp`, the same application file in a mode without a window. The command finds the running Glafira by the `mcp.json` file in the data folder, connects to it through a local socket (a named pipe on Windows) and passes the assistant's requests to it. Glafira answers through its own connections: passwords, SSH keys and connection strings stay in Glafira, and the assistant never receives them.

The socket is available only to your user. Connecting needs a token that Glafira writes to `mcp.json` each time the server starts; the file is readable only by its owner and exists while the server runs.

## Turning it on {#enable}

1. Open [[ui:sidebar.settings]], section [[ui:settings.mcp.section]].
2. Check [[ui:settings.mcp.enable]]. The status changes to [[ui:settings.mcp.state.running]].
3. Open the connection form ([[ui:sidebar.editConnection]] in the connection menu), choose [[ui:connections.form.llmLevels.diagnostics]] in the [[ui:connections.form.llmAccess]] group and save. The connection gets an `LLM` badge in the tree. The third level, [[ui:connections.form.llmLevels.data]], is for test stands: see [Diagnostics and data level](llm-mcp.md#data-level).

MCP is off by default, and every connection starts at [[ui:connections.form.llmLevels.off]]: the assistant does not see such connections. The level is not carried by export and import of connections: an imported connection stays closed to the assistant until you open it yourself. Changing the level does not reopen the connection. The [[ui:common.readOnly]] flag does not affect the assistant's access: the assistant changes nothing. The `prod` environment does not forbid anything either; at the [[ui:connections.form.llmLevels.data]] level the form only warns: [[ui:connections.form.llmProdWarning]].

If MCP is already served by another Glafira window, the status is [[ui:settings.mcp.state.otherInstance]]; clients connect to that window.

## Connecting a client {#connect-client}

The [[ui:settings.mcp.connect.title]] block has ready snippets with the path to the installed application and copy buttons:

- Qwen Code — the command `qwen mcp add glafira-pg "<path>" mcp` or an `mcpServers` block for `~/.qwen/settings.json`;
- Claude Code — the command `claude mcp add glafira-pg -- "<path>" mcp`;
- Claude Desktop — a block for `claude_desktop_config.json`; restart Claude Desktop after editing it;
- Cursor — a block for `~/.cursor/mcp.json`;
- any other client that starts an MCP server with a command (stdio) — the command and the `mcp` argument.

The client can also work with a model on your own computer, without the cloud: see the [Local models](local-llm.md) chapter.

Open Glafira first, then start the client. If Glafira is closed, `glafira-pg mcp` starts it and waits up to 25 seconds, but clients do not wait long for the server (Qwen Code waits 30 seconds), and on a slow start the connection may time out. If you reinstalled Glafira to another location, copy the snippet again.

Where the client stores the server depends on its scope. Claude Code adds a server to the `local` scope by default, which is the current project only; to make glafira-pg available in all projects add `--scope user` (`-s user`): `claude mcp add --scope user glafira-pg -- "<path>" mcp`. Qwen Code uses the `user` scope by default (all projects); `-s project` limits the server to the current project. Other flags are in `claude mcp add --help` and `qwen mcp add --help`.

## Approving a client {#approve}

When a client connects for the first time, Glafira shows a dialog over any screen: the client's name and version and how many connections are open to LLM assistants. Press [[ui:mcp.approval.allow]] or [[ui:mcp.approval.reject]]; Escape rejects too. Until you answer, the assistant gets the `client_not_approved` error and asks you to answer in the Glafira window. When several clients wait, the dialogs come one after another.

An allowed client is remembered by name and appears in [[ui:settings.mcp.clients.title]]. A rejection is not remembered: Glafira asks again on the next connection. A client without a name is shown as [[ui:mcp.unknownClient]], and the approval lasts only until its session ends.

Any program on your computer can fake a client name: the approval shows who connected, and the protection is the token in `mcp.json`, which only your user can read.

## What the assistant sees {#access}

With [[ui:connections.form.llmLevels.diagnostics]] the assistant gets:

- the open connections (name, environment, kind, folder, level, default database) and their topology: nodes with roles, lag and timeline, the Patroni state, topology events;
- a health summary: the pulse and the findings of the quick tools (sessions, locks, xmin horizon, replicas, slots, and for Patroni failover readiness and health);
- the report of any tool in the [[ui:sidebar.diagnostics]] group with findings and “what to do” texts;
- activity: sessions with filters and observing them for up to 50 seconds — waits, top queries, sessions that appeared and left;
- the `pg_stat_statements` top — since reset, for a window (if the [[ui:diag.tools.queries]] tab has collected history) or by observing for up to 50 seconds — and the query card;
- a table: columns, indexes, constraints, statistics, DDL and `pg_stats` column statistics without values (the answer tells the assistant that the most common values and histograms are available only at the [[ui:connections.form.llmLevels.data]] level);
- server parameters from `pg_settings`: passwords in connection strings (URI forms included) and commands (`primary_conninfo`, `archive_command` and others) are replaced with `***`, `ssl_passphrase_command` and settings with secret-like names (extensions included) are hidden whole; rows with source `client` and `session` are the settings of Glafira's own connection, not of the server;
- a query plan without execution (EXPLAIN without ANALYZE) with recommendations, and an index hypothesis check with hypopg — up to five indexes at a time; without the hypopg extension the assistant gets a hint on how to enable it.

At [[ui:connections.form.llmLevels.data]] the assistant gets all of this without masking, plus the values from `pg_stats` and reading data with `run_select`: see [Diagnostics and data level](llm-mcp.md#data-level).

The assistant's SQL is checked before it reaches the server: exactly one `SELECT`, `INSERT`, `UPDATE`, `DELETE` or `MERGE` statement is accepted (without ANALYZE it does not run), and utility commands and dangerous functions (`pg_sleep`, `dblink`, `lo_import`, `set_config`, signals, statistics resets `pg_stat_reset*`, `ts_stat`, `ts_rewrite`, file functions and others) are refused. The plan is built in a read-only transaction that is always rolled back, on a separate connection with `application_name = glafira-pg-mcp`; the MCP session also starts with `default_transaction_read_only = on`. Error positions in SQL refer to the assistant's original text. The `run_select` tool has stricter rules, described in [Diagnostics and data level](llm-mcp.md#data-level).

Responses are compacted to fit the context of a local model: empty fields are left out, fractional numbers are rounded, top query rows have no zero counters and their text is cut to 500 characters (the full text is in the query card), the card history has at most 12 points, and the wait profile has no intervals without samples, while adjacent idle intervals are merged into one. Responses are bounded: a response is at most 100 KB, the query card has at most 20 running instances (with the total), the table card at most 200 partitions (with the total), and a session's query text is at most 2 KB. SSH tunnel errors are shown to the assistant as a generic text without the bastion address, and node errors inside reports have details and paths removed.

The assistant never gets secret-store passwords and secrets, SSH settings, profile connection strings, the actions journal, the MCP journal or the application settings. Glafira has no OS metrics or server logs — the assistant will ask you for them.

Query texts, comments and object names come from the database and can be set by anyone. Glafira warns the assistant that they are data, not instructions.

## Masking {#masking}

At [[ui:connections.form.llmLevels.diagnostics]] Glafira replaces literals in query texts with `$1`, `$2`… before giving them to the assistant: strings, numbers, `E'…'`, dollar-quoted strings and others. A comment body becomes `/*…*/` (comments may carry data, such as sqlcommenter tags); the `/* glafira */` mark stays. Masking covers `pg_stat_activity` texts in every tool, `pg_stat_statements` texts, the running instances in the query card and the conditions in plan nodes (`Filter`, `Index Cond` and others) of plans not built from the assistant's SQL. The journal shows how many literals were masked.

Not masked: the SQL the assistant sent itself and its plan, table DDL (defaults and `CHECK` are schema), names of objects, roles and applications, and the snapshot of an optimization request, which you send yourself (at any level, see [Optimization requests and proposals](llm-mcp.md#optimization)). Masking does not recognize data written into names. A text the server cut in the middle of a literal is masked to its end.

Masking does not depend on the node's `standard_conforming_strings` setting: the text is masked under both of its values, and if the literal boundaries come out different, the assistant gets only the common beginning and one `$N…` marker instead of the rest. Text search (the `search` parameter of `get_top_queries` and of the query list) at [[ui:connections.form.llmLevels.diagnostics]] matches the masked text among the first 200 rows of the top, so literal values cannot be searched for.

At [[ui:connections.form.llmLevels.data]] Glafira masks nothing, in any tool.

## Sessions, revoking and the journal {#sessions-log}

While a client is connected, an `LLM` indicator with the client's name (or the number of clients) is shown at the bottom of the sidebar; it is highlighted during a call. Clicking it opens the MCP section of the settings.

- [[ui:settings.mcp.sessions.title]] — the client, start time and number of calls; [[ui:settings.mcp.sessions.disconnect]] closes one session, [[ui:settings.mcp.sessions.disconnectAll]] closes all.
- [[ui:settings.mcp.clients.title]] — [[ui:settings.mcp.clients.revoke]] forgets the client and closes its sessions; on the next connection it asks for approval again.
- A new level in the connection form, a revoked client and turning MCP off take effect from the assistant's next call; the client does not need to reconnect.
- [[ui:settings.mcp.audit.title]] — the last 200 records: time, client, tool or event, connection, result, duration, number of masked literals (for `run_select` — number of rows). Arguments longer than 1 KB are cut, SQL is written in full up to 16 KB, answers and the values read by `run_select` are not written. The events [[ui:settings.mcp.audit.events.requestCreated]] and [[ui:settings.mcp.audit.events.proposalAnswered]] come from the [optimization requests](llm-mcp.md#optimization). [[ui:settings.mcp.audit.reveal]] opens the `mcp-audit.jsonl` file.
- For a successful `explain_query` or `run_select` call the [[ui:settings.mcp.audit.openInConsole]] button opens the connection's [EXPLAIN console](queries.md#console) with the same SQL, database and node (the connection's default database if the call had none). Nothing runs: build the plan yourself and compare it with what the assistant saw. The button is unavailable if the arguments are cut in the journal, the call gave a `queryId`, the connection no longer exists or LLM access is off for it; the button's tooltip says why.

To turn MCP off completely, uncheck [[ui:settings.mcp.enable]]: clients are disconnected and `mcp.json` is removed.

## Organization policy {#policy}

An administrator can forbid MCP with the `mcpEnabled` policy key: [[ui:settings.mcp.enable]] is unavailable and marked [[ui:settings.mcp.policy]], the server does not start, and `glafira-pg mcp` exits with an error. The `mcpMaxAccess` key caps the access level; with the `diagnostics` cap the MCP section says [[ui:settings.mcp.dataPolicy]], connections at [[ui:connections.form.llmLevels.data]] work as [[ui:connections.form.llmLevels.diagnostics]] (texts are masked again, `pg_stats` comes without values), their form says [[ui:connections.form.llmDataAsDiagnostics]], and `run_select` answers `data_disabled`. If the policy cannot be read or these keys have a wrong value, MCP is forbidden as well, and the error is shown in [[ui:sidebar.about]]. Details are in the administrator guide.

## Optimization requests and proposals {#optimization}

The EXPLAIN console (the [[ui:diag.explain.sourceSql]] mode, next to the plan buttons) and the query card (in the row under the query text) have the [[ui:llmAssist.ask.button]] button. It creates an optimization request, a snapshot of the query for the LLM assistant: the node, the database, the text, the parameter values of the last plan (console), the `pg_stat_statements` statistics (in the card), the plan as text and the DDL of the tables. If there is no plan for the current text, node and database, Glafira tries to build one without execution. A note under the buttons says what to do next: request r3a9f1c is sent, tell your assistant “optimize the query from Glafira”, and it fetches the request itself. Write just that in Qwen Code, Claude Code or another connected client: the id is not needed, without it the assistant takes the latest waiting request. A line with the id in the interface language, for example “Optimize query r3a9f1c from Glafira”, is already on the clipboard — paste it when several requests are waiting. The model fetches the snapshot with the `get_optimization_request` tool.

Next to it is the [[ui:diag.query.copyLlm]] button (see [Query card](queries.md#card)): it copies the same content as text for any chat and needs no MCP, but the model's answer does not come back to Glafira. The request of [[ui:llmAssist.ask.button]] is fetched by the MCP assistant itself; the assistant can check plans and indexes through Glafira and returns proposals to the [[ui:llmAssist.panel.title]] panel. The “?” of each button explains this difference too.

The snapshot goes to the model as it is, without masking literals, even when the connection is open only for diagnostics: you send it yourself, and the plan may contain the values typed into the parameter form. The button is unavailable while the MCP server is off or runs in another Glafira window, the policy forbids MCP or the connection is closed to LLM assistants; the “?” next to it says why and where to turn it on. In the card the button appears when the query text is known and not cut; a query longer than 64 KB cannot be sent.

While the model is working, a line “Waiting for the LLM: request r3a9f1c” with [[ui:llmAssist.ask.cancel]] is shown under the buttons of the console and the card: a cancelled request is no longer given to the model, it gets `request_closed`. A new request from the same console, or from the card of the same query, can be sent once the previous one is answered or cancelled. Glafira keeps the last 50 requests. A large snapshot is trimmed to fit the 100 KB answer: the DDL of the last tables and the end of the plan text go first, and the model is told what was left out, so it can read it with `get_table_details` and `explain_query`.

The model answers with the `propose_query_rewrite` tool: the rewritten query, up to five `CREATE INDEX` statements, a rationale (up to 4 KB) and the expected effect (up to 1 KB). Before saving, Glafira checks the proposal: the statement is of the same kind as the original; there are no forbidden functions; the indexes pass the same checks as in `hypothetical_index` (no `CONCURRENTLY` and `UNIQUE`, the index name is dropped); the server can plan the query (`prepare` and a plan without execution in a `READ ONLY` transaction that is always rolled back, on the node of the request). An error goes back to the model with the server's text, and the model fixes the query itself. Glafira does not run the rewritten query and does not create indexes. Repeating the same proposal does not create a duplicate; a request takes at most five proposals. The model may also propose a rewrite on its own, without a request from Glafira, for example for a query it found in the top: such a proposal appears in the console of its connection when the same database is selected there.

The [[ui:llmAssist.panel.title]] panel under the console editor shows the proposals for the requests of this console and the proposals without a request for the selected database; it stays collapsed until the first proposal arrives. A new proposal appears at once; if the console is on a background tab, a dot appears on the tab. Each proposal in the list shows its status, the client, the time and the first line of the proposed text. The status is written as a word and marked with a colour: [[ui:llmAssist.panel.status.new]] in blue, [[ui:llmAssist.panel.status.accepted]] in green, [[ui:llmAssist.panel.status.rejected]] in red. The selected proposal opens as a card. Its header has the status, the client and the time, the [[ui:llmAssist.panel.accept]] and [[ui:llmAssist.panel.reject]] buttons on the right, and below them [[ui:llmAssist.panel.compare]], [[ui:llmAssist.panel.openInConsole]] and [[ui:llmAssist.panel.copy]]; a rejection reason is a separate line under the status. Then come the original and the proposed text side by side with the changed lines highlighted and a permanent note: Glafira does not check that the rewritten query returns the same rows. After that, the rationale and the expected effect as plain text (the panel does not render the model's markup) and the indexes. The [[ui:llmAssist.panel.original]] column may show `$1::timestamptz` where the query card shows `timestamptz $1`: this is how `pg_stat_statements` writes typed literals, the server cannot plan such text, so Glafira stores and compares the original and the proposed text with `$N::type` casts.

- [[ui:llmAssist.panel.compare]] builds both plans without execution on the node of the proposal with the same parameter values; the proposed query is planned together with its indexes through `hypopg` (virtual indexes that live only in the session of the comparison, see [Plan recommendations](queries.md#recommendations)). The result is the cost before and after, the hot nodes of both plans, the recommendations for both and whether each index is used. Without `hypopg` in the database the comparison goes without the indexes, and this is marked. A query with `$1`, `$2`… and no saved values is compared by its generic plan, which needs PostgreSQL 16 or newer; on older versions compare it in the console with your own values. The comparison runs on Glafira's own EXPLAIN connection, like the console. The result is saved in the proposal, and the model sees it in `list_proposals`.
- [[ui:llmAssist.panel.openInConsole]] puts the proposed text into the console, with the database and the node of the proposal. The text it replaces goes to the [[ui:diag.explain.replaced.title]] list under the panel (the last 10 texts), where a button puts it back; the same happens when a query is opened from the MCP journal. From there the usual console rules apply, including the [plan with execution](queries.md#analyze).
- [[ui:llmAssist.panel.accept]] and [[ui:llmAssist.panel.reject]] record your decision; a rejection can carry a reason of up to 500 characters, and the model sees it in `list_proposals`. You can change the decision later. [[ui:llmAssist.panel.copy]] copies the proposed text. Applying the change — editing the application code, creating the index — is up to you.

In the query card, in the row under the query text, the link “LLM proposals: N — open in the console” appears after the [[ui:llmAssist.ask.button]] button. It counts the proposals for this query (by its `queryid` and by the requests sent from the card) and opens the console with the original text and the panel, the newest proposal selected (unanswered ones first).

Proposals are kept in `llm-proposals.json` in the app data folder (up to 100, up to five per request), requests in `llm-requests.json`; both files are readable only by your user. The MCP journal records the creation of a request and your answers to proposals, without the query text and the reason.

## Diagnostics and data level, run_select {#data-level}

The third LLM access level in the connection form, [[ui:connections.form.llmLevels.data]], is meant for load-testing stands and test databases. At this level the model sees everything it sees at the diagnostics level, but without masking in any tool (query texts, session texts and plans keep their values), gets the most common values and histograms of columns from `pg_stats` in `get_table_details` (values longer than 2 KB are cut), and reads data with the `run_select` tool. In the tree such a connection has a filled `LLM` badge. If the connection environment is `prod`, the form warns that production data will go to the LLM provider; this is a warning, not a ban. If the organization's policy limits access to diagnostics (`mcpMaxAccess`), the connection works as [[ui:connections.form.llmLevels.diagnostics]], and the form says so.

`run_select` accepts exactly one `SELECT` statement (including `VALUES`, `TABLE`, `WITH … SELECT`) without `INTO`, `FOR UPDATE` / `FOR SHARE` and data-modifying CTEs at any depth, and without `$1`-style parameters: the values go into the query text. The database is optional (by default, the connection's database), and so is the node: by default the query goes to the reachable replica with the smallest lag, otherwise to the leader. Glafira checks the query in several layers:

- the `pg_query` parse tree and a list of forbidden functions: administrative functions, file access, large objects, `dblink`, `http`, `set_config`, advisory locks, `pg_sleep`, `pg_notify`, `query_to_xml` and its relatives, extension functions that run SQL from a string (`pg_background`, `crosstab`, `connectby`, `xpath_table`, pgRouting `pgr_*`), and every function of the schemas `net` (pg_net), `aws_s3` and `aws_lambda`, which call outside the database — the error is `forbidden_function`;
- a separate connection with `application_name = glafira-pg-mcp` and a `BEGIN READ ONLY` transaction that is always rolled back, with `statement_timeout` 10 s and `lock_timeout` 1 s;
- a check of the called functions against `pg_proc`, first by the names in the query and then by a probe `EXPLAIN (VERBOSE)`, which also shows the functions called through a view or the `t.f` notation: a function outside `pg_catalog` that does not belong to an extension is user-defined, its body cannot be checked, and `run_select` refuses it (`user_function`); the probe also finds forbidden functions the parser cannot see. Extension functions, such as those of `pg_stat_statements` or PostGIS, are allowed unless they are on the list.

The query runs through a cursor: 100 rows by default, at most 500 (`maxRows`) and at most 100 KB of answer; values are cut on the server, and a value longer than 2 KB ends with the mark `…[truncated]`; `truncated: true` in the answer means there were more rows. The query has 10 seconds: when they run out, the model gets the rows collected so far, or `TIMEOUT` if there are none; opening the connection comes on top, so a call can take a little longer. When the client cancels the call, Glafira stops the query on the server. The MCP journal gets the query text and the number of rows, not the values.

The checks do not make the connection role safe: a function of an extension, an operator or a type cast can call something that is not on the list, and `READ ONLY` does not stop non-transactional actions such as signals to other sessions. Therefore, for connections at the [[ui:connections.form.llmLevels.data]] level, the [readiness checklist](connections.md#readiness) has the [[ui:readiness.llmRole.title]] item: if the connection role is a superuser or a member of `pg_signal_backend`, `pg_write_all_data`, `pg_read_server_files` or `pg_execute_server_program`, the item warns what the model could do through `run_select` if it got around the list and gives the SQL for a stand role; the same warning appears under the level in the form. The model gets it as `roleWarning` in `list_connections` once the connection has been checked with [[ui:connections.form.test]] or has served a `run_select` since Glafira started. The warning does not turn the level off. For a stand, a separate read-only role is enough:

```sql
CREATE ROLE glafira_llm LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_llm;         -- statistics for the diagnostics tools
GRANT pg_read_all_data TO glafira_llm;   -- PostgreSQL 14 and newer
-- PostgreSQL 13 has no pg_read_all_data: in every database, for every schema the model may read
-- GRANT USAGE ON SCHEMA public TO glafira_llm;
-- GRANT SELECT ON ALL TABLES IN SCHEMA public TO glafira_llm;
```

Create a separate connection with this role and the [[ui:connections.form.llmLevels.data]] level, and keep the main connection at [[ui:connections.form.llmLevels.diagnostics]].

## Scenarios (prompts) {#prompts}

Glafira gives MCP clients four ready-made scenarios (MCP prompts). Clients show and invoke them in their own way:

- Qwen Code (tested with 0.26.5) as slash commands marked `[MCP]`: `/optimize_query`, `/triage_incident` and so on; arguments are given by name: `/analyze_top_queries --connectionId="…" --windowS="900"`.
- Claude Code (tested with 2.1.319) shows them in its menu as `/glafira-pg:optimize_query (MCP)` but invokes them as `/mcp__glafira-pg__optimize_query` (typing `/glafira-pg:optimize_query` gives “Unknown command”); arguments follow separated by spaces in the declared order, each a single word (Claude Code splits the line on whitespace), for example `/mcp__glafira-pg__analyze_top_queries <connectionId>`. The Claude Code documentation does not say how to skip an optional argument in the middle, so to set `windowS`, give `member` (the node name) too: `/mcp__glafira-pg__analyze_top_queries <connectionId> <member> 900`.
- Other clients show them in their list of prompts, if they support MCP prompts at all.

The scenario texts are in English. They ask the model to answer in your language and briefly: first a summary of up to 10 lines (what is happening, the likely cause, the next action), details on request. In `triage_incident`, `analyze_top_queries` and `review_config` the model says what to do in Glafira by naming its screens and buttons, not tools. `connectionId` is the connection id from `list_connections`; if you give the connection name instead, the first tool call answers `not_allowed` with the advice to call `list_connections`, where the assistant can find the id. Ids and node names are single lines of up to 128 characters, `symptom` is up to 1000 characters, `windowS` is 300, 900 or 3600.

- `optimize_query` (argument `requestId`, optional: without it, the latest waiting request): fetches the optimization request, studies the plan and the statistics, checks hypotheses with `explain_query` and `hypothetical_index` when needed, sends one or more proposals with `propose_query_rewrite` and explains them briefly in the chat; then compare the plans in Glafira.
- `triage_incident` (`connectionId`, optional `symptom`): `get_health_summary`, then at most three `run_diagnostic` calls for the findings that need details (locks, the xmin horizon, replication, slots, and for Patroni failover readiness and health), `observe_activity` for 30 s if there is load; the result is what is happening, the likely causes, what to do in Glafira (which screen, which action) and what to check outside it (server logs, the OS).
- `analyze_top_queries` (`connectionId`, optional `member` and `windowS`, 900 by default): the top 10 queries for the window (without collected history — `observe_top_queries` for 50 s or the statistics since reset), the cards and plans of at most three main queries, one at a time, recommendations; the model sends rewrite candidates with `propose_query_rewrite`, and they appear in the console panel of the connection.
- `review_config` (`connectionId`, optional `member`): the changed and key settings (memory, checkpoints and WAL, autovacuum, timeouts, `track_io_timing`, `jit`, parallelism) in view of the number of connections and the load; recommendations with risks and whether a restart is needed. For Patroni the model suggests editing with `patronictl edit-config`; Glafira does not change the configuration.

The scenarios are not required: a plain question in the chat works too, see [Example questions](llm-mcp.md#examples).

## Example questions {#examples}

- “What is going on in the billing cluster?” — the assistant starts with the health summary and opens the reports of the findings.
- “Who holds the xmin horizon on billing, and what should I do about it?”
- “Why is the lag of pg-3 growing?”
- “What has been loading the database for the last 15 minutes?” — the 15-minute window exists if the [[ui:diag.tools.queries]] tab was open; otherwise the assistant observes the top for up to 50 seconds or takes the statistics since reset.
- “Show the plan of the heaviest query in the top and check whether an index on status would help” — a plan without execution and a hypopg hypothesis.
- “Optimize the query from Glafira” after [[ui:llmAssist.ask.button]] (or the line with the id from the clipboard) — the assistant fetches the request, and its proposals appear in the [[ui:llmAssist.panel.title]] panel.
- “How many orders in each status are there on the test stand?” — at the [[ui:connections.form.llmLevels.data]] level the assistant reads the answer with `run_select`.

## Troubleshooting {#troubleshooting}

The client shows a message from Glafira:

- `MCP is disabled in Glafira` — check [[ui:settings.mcp.enable]] and restart the MCP server in the client;
- `MCP is disabled by your organization` — MCP is forbidden by your organization's policy;
- `Glafira did not start the MCP server within 25 s` — open Glafira yourself, check the status in the [[ui:settings.mcp.section]] section and restart the server in the client;
- `bad_token` — restart the MCP server in the client: Glafira issues a new token each time the server starts.

Known limitations: on Linux without `XDG_RUNTIME_DIR`, and with a long data directory path, the socket is created in the shared `/tmp` under the fixed name `glafira-pg-mcp.sock` (mode 0600); a server error in a `queryId` answer from `pg_stat_statements` may contain data values; a pre-existing user function wrongly labeled `IMMUTABLE` may be evaluated while the plan is built, and its result can appear in the verbose plan (data cannot be changed: the transaction is read-only); constants that the planner computes in advance (for example `repeat('x', 100000000)`) go into the plan whole, so Glafira does not parse a plan larger than 8 MB and answers with an error that advises making such values smaller (`invalid_sql` for the assistant); on a `SQL_ASCII` database cutting a long value may break a character, and the answer then is `invalid_sql` with the advice to leave the column out or select it through `encode(…, 'hex')`; the total time of a `run_select` call can exceed 10 seconds (connection, probe plan and reading add to the statement time).

The assistant sees call errors itself and usually explains them: `client_not_approved` — answer the dialog in Glafira; `not_allowed` — the connection is closed to the assistant (level [[ui:connections.form.llmLevels.off]]) or does not exist; `rate_limited` and `busy` — a limit was hit (60 calls a minute, one observation at a time per session), and the assistant retries later; `data_not_allowed` — `run_select` needs the [[ui:connections.form.llmLevels.data]] level, and the connection is at [[ui:connections.form.llmLevels.diagnostics]]; `data_disabled` — reading data is limited by the organization's policy; `forbidden_function`, `user_function` and `invalid_sql` — Glafira refused the assistant's SQL, the message says why, and the assistant rewrites the query; `request_closed` — you cancelled the optimization request; `TIMEOUT` from `run_select` — the query returned no rows within 10 seconds.
