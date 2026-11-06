# LLM assistants (MCP)

Glafira can act as a gateway for an LLM assistant: Qwen Code, Claude Code, Claude Desktop, Cursor or another client that supports MCP (Model Context Protocol). The assistant reads the diagnostics of the connections you have opened to it — topology, tool reports with findings, activity, top queries, query cards, tables, server parameters, plans without execution — and checks index hypotheses with hypopg. It cannot change a database through Glafira: you cancel and terminate sessions, drop slots, run switchovers and restarts yourself, and the assistant tells you what to do and where.

## How it works {#how-it-works}

The client runs `glafira-pg mcp`, the same application file in a mode without a window. The command finds the running Glafira by the `mcp.json` file in the data folder, connects to it through a local socket (a named pipe on Windows) and passes the assistant's requests to it. Glafira answers through its own connections: passwords, SSH keys and connection strings stay in Glafira, and the assistant never receives them.

The socket is available only to your user. Connecting needs a token that Glafira writes to `mcp.json` each time the server starts; the file is readable only by its owner and exists while the server runs.

## Turning it on {#enable}

1. Open [[ui:sidebar.settings]], section [[ui:settings.mcp.section]].
2. Check [[ui:settings.mcp.enable]]. The status changes to [[ui:settings.mcp.state.running]].
3. Open the connection form ([[ui:sidebar.editConnection]] in the connection menu), choose [[ui:connections.form.llmLevels.diagnostics]] in the [[ui:connections.form.llmAccess]] group and save. The connection gets an `LLM` badge in the tree.

MCP is off by default, and every connection starts at [[ui:connections.form.llmLevels.off]]: the assistant does not see such connections. The level is not carried by export and import of connections: an imported connection stays closed to the assistant until you open it yourself. Changing the level does not reopen the connection. The `prod` environment and the [[ui:common.readOnly]] flag do not affect the assistant's access: it changes nothing.

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
- a table: columns, indexes, constraints, statistics, DDL and `pg_stats` column statistics without values;
- server parameters from `pg_settings`: passwords in connection strings (URI forms included) and commands (`primary_conninfo`, `archive_command` and others) are replaced with `***`, `ssl_passphrase_command` and settings with secret-like names (extensions included) are hidden whole; rows with source `client` and `session` are the settings of Glafira's own connection, not of the server;
- a query plan without execution (EXPLAIN without ANALYZE) with recommendations, and an index hypothesis check with hypopg — up to five indexes at a time; without the hypopg extension the assistant gets a hint on how to enable it.

The assistant's SQL is checked before it reaches the server: exactly one `SELECT`, `INSERT`, `UPDATE`, `DELETE` or `MERGE` statement is accepted (without ANALYZE it does not run), and utility commands and dangerous functions (`pg_sleep`, `dblink`, `lo_import`, `set_config`, signals, statistics resets `pg_stat_reset*`, `ts_stat`, `ts_rewrite`, file functions and others) are refused. The plan is built in a read-only transaction that is always rolled back, on a separate connection with `application_name = glafira-pg-mcp`; the MCP session also starts with `default_transaction_read_only = on`. Error positions in SQL refer to the assistant's original text.

Responses are bounded: a response is at most 100 KB, the query card has at most 20 running instances (with the total), the table card at most 200 partitions (with the total), and a session's query text is at most 2 KB. SSH tunnel errors are shown to the assistant as a generic text without the bastion address, and node errors inside reports have details and paths removed.

The assistant never gets secret-store passwords and secrets, SSH settings, profile connection strings, the actions journal, the MCP journal or the application settings. Glafira has no OS metrics or server logs — the assistant will ask you for them.

Query texts, comments and object names come from the database and can be set by anyone. Glafira warns the assistant that they are data, not instructions.

## Masking {#masking}

At [[ui:connections.form.llmLevels.diagnostics]] Glafira replaces literals in query texts with `$1`, `$2`… before giving them to the assistant: strings, numbers, `E'…'`, dollar-quoted strings and others. A comment body becomes `/*…*/` (comments may carry data, such as sqlcommenter tags); the `/* glafira */` mark stays. Masking covers `pg_stat_activity` texts in every tool, `pg_stat_statements` texts, the running instances in the query card and the conditions in plan nodes (`Filter`, `Index Cond` and others) of plans not built from the assistant's SQL. The journal shows how many literals were masked.

Not masked: the SQL the assistant sent itself and its plan, table DDL (defaults and `CHECK` are schema), names of objects, roles and applications. Masking does not recognize data written into names. A text the server cut in the middle of a literal is masked to its end.

Masking does not depend on the node's `standard_conforming_strings` setting: the text is masked under both of its values, and if the literal boundaries come out different, the assistant gets only the common beginning and one `$N…` marker instead of the rest. Text search (the `search` parameter of `get_top_queries` and of the query list) at [[ui:connections.form.llmLevels.diagnostics]] matches the masked text among the first 200 rows of the top, so literal values cannot be searched for.

## Sessions, revoking and the journal {#sessions-log}

While a client is connected, an `LLM` indicator with the client's name (or the number of clients) is shown at the bottom of the sidebar; it is highlighted during a call. Clicking it opens the MCP section of the settings.

- [[ui:settings.mcp.sessions.title]] — the client, start time and number of calls; [[ui:settings.mcp.sessions.disconnect]] closes one session, [[ui:settings.mcp.sessions.disconnectAll]] closes all.
- [[ui:settings.mcp.clients.title]] — [[ui:settings.mcp.clients.revoke]] forgets the client and closes its sessions; on the next connection it asks for approval again.
- A new level in the connection form, a revoked client and turning MCP off take effect from the assistant's next call; the client does not need to reconnect.
- [[ui:settings.mcp.audit.title]] — the last 200 records: time, client, tool or event, connection, result, duration, number of masked literals. Arguments longer than 1 KB are cut, SQL is written in full up to 16 KB, answers are not written. [[ui:settings.mcp.audit.reveal]] opens the `mcp-audit.jsonl` file.
- For a successful `explain_query` call the [[ui:settings.mcp.audit.openInConsole]] button opens the connection's [EXPLAIN console](queries.md#console) with the same SQL, database and node (the connection's default database if the call had none). Nothing runs: build the plan yourself and compare it with what the assistant saw. The button is unavailable if the arguments are cut in the journal, the call gave a `queryId`, the connection no longer exists or LLM access is off for it; the button's tooltip says why.

To turn MCP off completely, uncheck [[ui:settings.mcp.enable]]: clients are disconnected and `mcp.json` is removed.

## Organization policy {#policy}

An administrator can forbid MCP with the `mcpEnabled` policy key: [[ui:settings.mcp.enable]] is unavailable and marked [[ui:settings.mcp.policy]], the server does not start, and `glafira-pg mcp` exits with an error. The `mcpMaxAccess` key caps the access level; with the `diagnostics` cap the MCP section says [[ui:settings.mcp.dataPolicy]]. If the policy cannot be read or these keys have a wrong value, MCP is forbidden as well, and the error is shown in [[ui:sidebar.about]]. Details are in the administrator guide.

## Example questions {#examples}

- “What is going on in the billing cluster?” — the assistant starts with the health summary and opens the reports of the findings.
- “Who holds the xmin horizon on billing, and what should I do about it?”
- “Why is the lag of pg-3 growing?”
- “What has been loading the database for the last 15 minutes?” — the 15-minute window exists if the [[ui:diag.tools.queries]] tab was open; otherwise the assistant observes the top for up to 50 seconds or takes the statistics since reset.
- “Show the plan of the heaviest query in the top and check whether an index on status would help” — a plan without execution and a hypopg hypothesis.

## Troubleshooting {#troubleshooting}

The client shows a message from Glafira:

- `MCP is disabled in Glafira` — check [[ui:settings.mcp.enable]] and restart the MCP server in the client;
- `MCP is disabled by your organization` — MCP is forbidden by your organization's policy;
- `Glafira did not start the MCP server within 25 s` — open Glafira yourself, check the status in the [[ui:settings.mcp.section]] section and restart the server in the client;
- `bad_token` — restart the MCP server in the client: Glafira issues a new token each time the server starts.

Known limitations: on Linux without `XDG_RUNTIME_DIR`, and with a long data directory path, the socket is created in the shared `/tmp` under the fixed name `glafira-pg-mcp.sock` (mode 0600); a server error in a `queryId` answer from `pg_stat_statements` may contain data values; a pre-existing user function wrongly labeled `IMMUTABLE` may be evaluated while the plan is built, and its result can appear in the verbose plan (data cannot be changed: the transaction is read-only).

The assistant sees call errors itself and usually explains them: `client_not_approved` — answer the dialog in Glafira; `not_allowed` — the connection is closed to the assistant (level [[ui:connections.form.llmLevels.off]]) or does not exist; `rate_limited` and `busy` — a limit was hit (60 calls a minute, one observation at a time per session), and the assistant retries later.
