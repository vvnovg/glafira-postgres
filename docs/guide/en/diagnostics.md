# Diagnostics

The [[ui:sidebar.diagnostics]] group sits in the tree of an expanded connection, after the groups with nodes, databases and roles. Each tool opens in its own tab: [[ui:diag.tools.pulse]], [[ui:diag.tools.sessions]], [[ui:diag.tools.idleTx]], [[ui:diag.tools.waits]], [[ui:diag.tools.locks]], [[ui:diag.tools.horizon]], [[ui:diag.tools.replication]] and [[ui:diag.tools.slots]]; a Patroni cluster also has [[ui:diag.tools.failover]] and [[ui:diag.tools.health]]. All the tools only read: the app runs `SELECT` on PostgreSQL nodes and `GET` on the Patroni REST API, and changes nothing on the server or in the cluster. Diagnostic sessions are read-only, with `statement_timeout` of 5 seconds and `lock_timeout` of 1 second, and are named `glafira-pg` (`application_name`).

The header is the same in every tab:

- [[ui:diag.header.node]] — which node to read. The default is [[ui:diag.header.followLeaderPending]]: the node that accepts writes — the Patroni leader (the leader of a standby cluster in that case), the available primary of a host list, or the only server. When the leader changes, the tab moves to the new one and says “Node changed: old → new”. A node chosen from the list stays pinned to the tab. Tools that poll all members themselves ([[ui:diag.tools.pulse]], [[ui:diag.tools.failover]], [[ui:diag.tools.health]]) and a single server have no node selector.
- [[ui:diag.header.interval]] — how often to refresh the data: 2, 5, 10 or 30 seconds, or [[ui:diag.header.pause]]. The default is 5 seconds; the Patroni tools are polled every 10 seconds, like the topology. The choice is not kept between launches.
- [[ui:diag.header.refresh]] reads the data right away; next to it is the time when the app received the last report. If a refresh fails, the previous data stays on screen, marked [[ui:diag.header.stale]], with an error message.

A tab polls the server only while it is visible: a hidden tab sends nothing, and when you return to it, data that went stale is read at once. After a login is rejected, polling stops: repeated attempts with a wrong password can lock the account. If the connection is closed, the tab says [[ui:diag.closed]] and offers [[ui:diag.connect]].

For the full picture the role needs `pg_monitor` (it includes `pg_read_all_stats`): without it the server hides the query texts of other sessions, replica details and the size of `pg_wal`. The tool says so: the hidden part is listed as grey lines under the findings block, for example “pg_wal size: needs the pg_monitor role”. What the server version lacks is explained the same way, for example “Query ID (query_id): PostgreSQL 14+”. Privileges and the load on the server are described in the administrator guide, in the “Diagnostics” section.

## Pulse {#pulse}

[[ui:diag.tools.pulse]] is one table for the whole cluster: a row per node, a column per indicator. All available nodes are polled in parallel: for each one, a query for the indicators and a separate query for the size of `pg_wal` (`pg_ls_waldir()`). An unavailable node stays a row with the reason ([[ui:diag.pulse.unreachable]]).

Indicators:

- [[ui:diag.pulse.columns.connections]] — how many of the connections available to ordinary roles are in use (`max_connections` minus the superuser reserve and, in PostgreSQL 16 and later, minus `reserved_connections`), and the percentage.
- [[ui:diag.pulse.columns.tps]] — transactions per second; hover to see commits and rollbacks separately.
- [[ui:diag.pulse.columns.cache]] — the share of block reads served from the cache between two refreshes.
- [[ui:diag.pulse.columns.wal]] — the WAL write rate (receive rate on a replica).
- [[ui:diag.pulse.columns.xact]], [[ui:diag.pulse.columns.query]] and [[ui:diag.pulse.columns.idle]] — the longest transaction, active query and idle-in-transaction time among client sessions. The server computes durations from its own `now()`, the clock of your computer is not used, and the collecting session itself is not counted.
- [[ui:diag.pulse.columns.locks]] — how many client sessions wait for a lock and how long the oldest of them has waited. Background processes such as autovacuum are not counted here: their waits are shown in [[ui:diag.tools.locks]].
- [[ui:diag.pulse.columns.xid]] — the largest `datfrozenxid` age across databases and its share of the wraparound limit (2³¹).
- [[ui:diag.pulse.columns.walDir]] — the size of the `pg_wal` directory; hover to see `max_wal_size`.
- [[ui:diag.pulse.columns.lag]] — replica lag in bytes and in time. It comes from the topology; there is no separate query to the replica.

A cell is colored by the most severe finding of the node for that indicator. Color is not the only cue: the cell has a mark (`i`, `!` or `!!`) and a hidden label for screen readers. Clicking a cell opens the tool where the culprits are listed, straight on that node: connections, queries, transactions and cache — [[ui:diag.tools.sessions]]; idle in transaction — [[ui:diag.tools.idleTx]]; TPS and lock waits — [[ui:diag.tools.locks]]; XID age — [[ui:diag.tools.horizon]]; WAL rate and lag — [[ui:diag.tools.replication]]; `pg_wal` size — [[ui:diag.tools.slots]].

The rates (TPS, cache hit, WAL per second) are the difference between two consecutive refreshes, so the first refresh shows “…” instead. The same happens after a server restart or a statistics reset: there is nothing to compare with. The size of `pg_wal` is read by `pg_ls_waldir()`, which needs the `pg_monitor` role; without it the column shows a dash and the reason is given under the findings.

## Sessions and idle in transaction {#sessions}

[[ui:diag.tools.sessions]] shows `pg_stat_activity` of the chosen node: a row per session, virtualized for large lists. [[ui:diag.tools.idleTx]] is the same list with the “Idle in transaction” preset selected in advance.

- Presets: [[ui:diag.sessions.presets.all]], [[ui:diag.sessions.presets.active]], [[ui:diag.sessions.presets.idleTx]] and [[ui:diag.sessions.presets.waiting]] (waiting for a lock). Active and waiting sessions are sorted by query age, idle in transaction by idle time.
- The filter is a case-insensitive substring of pid, role, application, client, database and query text.
- [[ui:diag.sessions.groupBy]] — application, role, client or database: instead of the list you get a table with session counts per state. The preset does not affect the groups, but the filter and [[ui:diag.sessions.hideGlafira]] do.
- [[ui:diag.sessions.hideGlafira]] is on by default: the app's own sessions (`application_name = glafira-pg`) are shown neither in the list nor in the groups.
- The [[ui:diag.sessions.columns.blockedBy]] column holds the pids of the blocking sessions; it is filled only for sessions waiting for a lock.
- Clicking a row opens the session panel: backend type, start time, age of the transaction, query and state, transaction ID, xmin age, query ID (PostgreSQL 14 and later), the number of locks held, the full query text and the [[ui:diag.sessions.copyQuery]] button. The text is limited to 16 KB, and a cut is marked. When the session ends, the panel stays with the last data and a note.

The query texts of other sessions are visible to a role with `pg_read_all_stats`. Without it the text is `<insufficient privilege>`, and the “Texts of sessions are hidden” finding suggests granting `pg_monitor` to the role.

A session in the `idle in transaction` state opened a transaction and does nothing. It holds the locks it has acquired (the [[ui:diag.sessions.columns.locksHeld]] column) and the xmin horizon (the [[ui:diag.sessions.columns.xminAge]] column): autovacuum cannot remove dead rows newer than its snapshot, and tables bloat. Usually the application is at fault — it did not run `COMMIT` or `ROLLBACK`; grouping by application and role helps to find it. Server-side protection is `idle_in_transaction_session_timeout`: set it for the application role rather than for the whole server. The “Idle in transaction” preset shows the current value of the setting. If it is 0 and a session has been idle for 5 minutes or more, a finding suggests a ready command, `ALTER ROLE … SET idle_in_transaction_session_timeout = '10min';`, with the [[ui:diag.findings.copy]] button; the app does not run this command.

## Wait profile {#waits}

[[ui:diag.tools.waits]] answers the question “what do active sessions spend their time on”. It is built from samples collected by the sampler:

- Once a second, one light query to `pg_stat_activity` runs on the chosen node: active sessions of clients, autovacuum and parallel workers. Background processes that wait forever (walsender, checkpointer and the like) are not sampled so they do not distort the picture; the sampler's own session is not sampled either. Query text is taken up to 16 KB.
- The sampler has its own connection (one per node), so a poll every second never waits for catalog queries.
- The sampler runs while the profile tab is open, including while the tab is hidden: history is not interrupted when you switch tabs. Closing the tab stops the sampler. A tab that follows the leader switches to the sampler of the new leader when the leader changes.
- History is kept only in the memory of the app: the last 60 minutes, but not more than 500,000 sample rows (about 40 MB); older data is dropped. It lives until the connection is closed or the node leaves the cluster. If you close the tab and open it again, the history continues, and the time when the sampler did not run is marked as a gap (hatched on the chart).
- A query that has not finished within a second does not overlap with the next one: the tick is skipped. After an error the sampler pauses for 1, 2, 5, 10 and 30 seconds; after a rejected login it stops, and it starts again when you choose [[ui:sidebar.reopen]] in the connection menu.

[[ui:diag.waits.window]] is chosen in the header: 5, 15 or 60 minutes (15 by default). The chart is made of columns with the average number of active sessions per wait class: one column covers 5 seconds for a 5-minute window, 10 seconds for 15 minutes and 40 seconds for 60 minutes. The legend and the table columns list only the classes found in the window; a table with the same numbers is given for screen readers. If the history is shorter than the window, the tab says since when the sampler has been running. The line above the chart shows that the sampler is alive, with the number of active sessions and of sessions waiting for a lock at the last tick.

Wait classes are `wait_event_type` values. An active session with no wait event is counted as the `CPU*` class: it is on the CPU or waits where PostgreSQL does not instrument the wait, hence the asterisk. The other classes:

- `Lock` — waiting for a heavyweight lock; [[ui:diag.tools.locks]] shows who blocks.
- `LWLock` — internal lightweight locks of the server, contention for shared structures.
- `IO` — reading and writing data files and WAL.
- `IPC` — waiting for another process, for example a parallel worker or replication.
- `Client` — waiting for the client: the session waits for data from the application or sends a result.
- `Timeout` — waiting for a timer, for example `pg_sleep`.
- `BufferPin`, `Extension`, `Activity` and others.

Below the chart are top waits (class and wait event, share and number of samples), top queries and breakdowns by application, role, database and client; each table has up to 10 rows. A query in the top is identified by `query_id` (PostgreSQL 14 and later, when it exists and is not 0), otherwise by its text; in PostgreSQL 13 always by text. Parallel workers are counted under the query of their leader. For a query its main wait is shown.

The finding about lock waits fires when `Lock` accounts for 20% of the window's samples or more (50% is critical); it is computed only if the window holds at least 30 samples. The profile report reads the buffer in memory and does not query the server itself: the load comes only from the sampler.

## Locks {#locks}

[[ui:diag.tools.locks]] shows who holds whom. The data comes from `pg_stat_activity`, `pg_locks` and `pg_blocking_pids`; the list of blockers is requested only for sessions waiting for a lock, and relation sizes are not computed, so the tool puts no lock load on the server.

- The [[ui:diag.locks.tree]] starts from the root blocker — a session that waits for nothing itself but holds others. The victims are nested under it, and under them those who queue behind the victims. Each node shows the user, application, database, state, transaction age, query, the number of victims in the subtree, and for a waiting session what it requested (lock mode on a relation) and how long it has waited. A victim's parent is the first blocker from `pg_blocking_pids`; the others are listed in the “also waits for” line. Branches collapse.
- If the blocker is not visible — a prepared transaction (pid 0) or a session that ended between queries — the waiting session becomes the root of its own subtree, and the caption explains what it waits for. Look for prepared transactions in [[ui:diag.tools.horizon]].
- PostgreSQL 13 does not record when a lock wait began (`pg_locks.waitstart` exists since version 14), so there the wait is estimated from the last state change of the session and marked “≈”.
- A lock on a relation from another database is labeled “oid N in database X”: the name can be read only in the database of the connection.

Above the tree are the [[ui:diag.locks.ddlQueue]] cards. This is DDL that waits for a relation lock and holds others back itself: `ALTER`, `DROP`, `TRUNCATE`, `LOCK`, `CREATE TRIGGER`, foreign keys and `CREATE INDEX` without `CONCURRENTLY` ask for `AccessExclusiveLock`, `ShareRowExclusiveLock` or `ShareLock`. This is the most common reason for “everything stopped”: a migration waits for a long transaction, and ordinary `SELECT`s line up behind the migration. A card shows the DDL, whom it waits for, for how long, and how many sessions stand behind it. What to do: cancel the DDL — the queue clears at once — or the query the DDL waits for; run migrations with `lock_timeout` so that the DDL gives up by itself. The app only shows the situation; cancel queries and end sessions with the tools of the server (`pg_cancel_backend`, `pg_terminate_backend`, the `pg_signal_backend` role).

The [[ui:diag.locks.cycles]] section lists sessions that wait for each other. The server resolves such a cycle itself after `deadlock_timeout`; the details of deadlocks the server has already resolved exist only in its log, and a note about this always stands under the tab.

Findings: a lock wait of 30 seconds or more (critical from 5 minutes); a root blocker whose victim waits 10 seconds or more (critical from 60 seconds or with 10 or more victims; one finding per root); queued DDL (critical, one finding per item of the queue); a wait cycle (info).

## xmin horizon {#horizon}

The xmin horizon is the oldest transaction whose snapshot is still needed by someone on the server. `VACUUM` cannot remove dead rows newer than the horizon, so a long-lived holder bloats tables and grows the XID age. [[ui:diag.tools.horizon]] lists the holders by descending xmin age (in transactions) and shows how long each has held it:

- a session with an open transaction — pid, role, application, database, state; it helps to find the application and close the transaction;
- a prepared transaction — one forgotten after a coordinator failure; below it is a ready command `ROLLBACK PREPARED '…';` with the [[ui:diag.findings.copy]] button (the app does not run it);
- a replication slot — an inactive slot holds `xmin` or `catalog_xmin` (the catalog horizon is marked separately); fix it in [[ui:diag.tools.slots]];
- replica feedback: with `hot_standby_feedback = on` a long query on a replica holds the horizon on the primary. The replica is matched to a cluster member, and below its row is a link to the horizon of that member: fix it there, not on the primary. A replica connected through a physical slot is shown the same way.

Autovacuum and lazy `VACUUM` (including a manual one) are not counted as holders: their xmin does not hold back the horizon of other processes; `VACUUM FULL` is an ordinary transaction and is counted. The value of `hot_standby_feedback` is shown above the table. Without `pg_read_all_stats` the server hides the xmin of other sessions — this is stated under the findings.

The “The xmin horizon is held by …” finding describes the oldest holder: 10 million transactions or 1 hour is a warning, 100 million or 6 hours is critical. The XID age of the database itself is in [[ui:diag.tools.pulse]].

## Replicas {#replication}

[[ui:diag.tools.replication]] shows replication from the point of view of the chosen node.

On a primary and on a cascading replica this is the [[ui:diag.replication.senders]] table — a row per connected replica. A replica is matched to a cluster member by `application_name`: it equals the member name or its host (names are not resolved through DNS). If it cannot be matched, the row is labeled with `application_name` and there are no links to a member. The sent, write, flush and replay lags are shown in bytes from the current WAL position of the node, the replay lag also in time; the write and flush times are on hover. Besides the lags, the table has the client address, state, sync state, time since the replica's last reply, slot and xmin age. Above the table are `synchronous_standby_names` and `synchronous_commit`.

On a replica, the [[ui:diag.replication.receiver]] card stands at the top: status and source, slot, timeline, time of the last message and of the last position report, the gap between received and applied WAL, replay delay, WAL replay pause state, query conflicts, `hot_standby_feedback` and `max_standby_streaming_delay`. Recovery conflicts — queries cancelled on the replica to apply WAL — are summed over databases; next to the total is the growth since the previous refresh.

A role without `pg_read_all_stats` cannot see the state of senders and of the receiver. Then, instead of the table, only the number of connected WAL senders is shown, the view says so, and no findings are built from them: empty fields do not mean that replication is broken.

Findings: replica lag of 16 MB or 30 seconds (critical from 1 GB or 5 minutes); a replica that is not streaming; a replica silent for more than 60 seconds; `synchronous_standby_names` is set but there is no synchronous replica (critical); the WAL receiver is not running (critical); WAL replay is paused — if the pause is not intentional, resume it with `pg_wal_replay_resume()`; new recovery conflicts (consider `hot_standby_feedback` or `max_standby_streaming_delay`).

## Replication slots {#slots}

[[ui:diag.tools.slots]] shows the replication slots of the chosen node: name, type (physical or logical, plugin), database, state, `wal_status`, retained WAL, WAL headroom before the slot is lost (with `max_slot_wal_keep_size`), xmin and `catalog_xmin` age, the moment since which the slot has been inactive (PostgreSQL 17 and later), and flags: temporary, failover, synced from the primary, two-phase, conflicting, invalidated. Columns the server version lacks are empty, and the reason is given under the findings. Below the table is the value of `max_slot_wal_keep_size`.

A slot keeps WAL until its consumer has read it. An inactive slot whose consumer is gone piles up WAL, and `pg_wal` can fill the disk. The slot keeps the WAL it needs while `wal_status` is `reserved` or `extended`; `unreserved` means that the WAL is about to be removed, and `lost` that it is gone and the consumer cannot continue: drop the slot and resynchronize the consumer. If the consumer is gone for good, drop the slot; otherwise bring the consumer back and set `max_slot_wal_keep_size`. The app does not drop slots. An active slot piles up WAL while it is being read — that is replica lag, not a forgotten slot; synced slots (PostgreSQL 17 and later, on a standby) are inactive by design and raise no findings.

For a Patroni cluster there is an extra column with the slot mark: a member slot — Patroni creates and removes it itself — or a permanent slot from the `slots` section of the dynamic configuration (`GET /config`). The name of a member slot is built by the Patroni rule: lower case, `-` and `.` become `_`, other characters outside `[a-z0-9_]` become `u` and a four-digit character code, at most 63 characters long. If `/config` could not be read, only member slots are marked, and the reason is given under the findings.

Below the table are the logical replication subscriptions: the apply worker (or “not running”), the received LSN, the time of the last message, the numbers of apply and sync errors (PostgreSQL 15 and later) and their growth since the previous refresh.

Findings: an inactive slot retains 1 GB of WAL or more (critical from 10 GB or with `unreserved`); a slot lost its WAL (critical); new subscription errors. The size of `pg_wal` as a whole is in [[ui:diag.tools.pulse]].

## Failover readiness {#failover}

This tool exists only for a Patroni cluster. [[ui:diag.tools.failover]] shows what Patroni is ready for; the decision to switch over is Patroni's own. Every member except the leader gets a verdict, [[ui:diag.failover.ready]] or [[ui:diag.failover.notReady]], with reasons:

- the member is unreachable;
- its state is neither streaming nor running;
- the lag exceeds `maximum_lag_on_failover` or is unknown;
- the `nofailover` tag;
- `failover_priority = 0` (Patroni 4 and later);
- it is not a synchronous replica while `synchronous_mode` is on (in `quorum` mode quorum replicas qualify as well).

A “restart pending” mark does not forbid being a candidate. Above the table is a summary: how many members are ready, the dynamic configuration of Patroni from `GET /config` (`maximum_lag_on_failover`, `synchronous_mode`, `synchronous_mode_strict`, `synchronous_node_count`, `ttl`, `loop_wait`, `retry_timeout`, permanent slots) and a warning if the cluster is paused and there is no automatic failover. If the configuration has no `maximum_lag_on_failover` key, the Patroni default of 1 MB is used. The configuration is read by one request per refresh, without a cache. When `/config` does not answer, the tool shows an error instead of data.

Findings: no ready candidate at all (critical), one ready candidate among three members or more (warning); `synchronous_mode` is on but there is no synchronous replica (critical); the cluster is paused; `ttl` is less than `loop_wait` plus twice `retry_timeout` — the leader key can expire during a DCS hiccup. Scheduled Patroni switchovers and restarts are shown not here but in the cluster overview: as a banner and as a mark in the member's row.

## Health matrix {#health}

This tool exists only for a Patroni cluster. [[ui:diag.tools.health]] is a grid of members by REST API addresses: for every member that has an API address, `GET /primary`, `/standby-leader`, `/replica`, `/read-only`, `/sync`, `/async` and `/health` run in parallel (without authorization, 5 seconds per request). A cell holds the HTTP status code, or [[ui:diag.health.noAnswer]] if the API did not answer (the reason is in the cell's tooltip). A member without an API address has no cells ([[ui:diag.health.noApi]]).

The cell color depends not on the code but on whether the response matches the member's role:

- green: the expected 200, such as `/primary` on the leader or `/replica` on a replica;
- gray: the endpoint does not apply to the role, so 503 is normal, such as `/primary` on a replica, `/replica` on the leader or `/sync` on an asynchronous replica;
- yellow: the node works, but the load balancer sends it no traffic for this endpoint, such as a replica answering 503 on `/replica` because of lag or the `noloadbalance` tag;
- red: the response contradicts the role (a replica answers 200 on `/primary`, or the leader answers as a leader nowhere), `/health` is not 200, or the API did not answer.

So a healthy cluster looks green and gray, and red shows at once where to look. The meaning of the color is in the cell's tooltip. This is how a load balancer sees the cluster, for example HAProxy with `option httpchk GET /primary`. Parameters like `/replica?lag=…` are not checked.

Findings: exactly one member must answer 200 on `/primary` (on `/standby-leader` in a standby cluster), otherwise a load balancer sends writes to the wrong place or nowhere (critical); `/health` is not 200 on a member whose API answered — PostgreSQL on that member is not running (critical); a replica whose `/replica` is not 200 is excluded from reads (warning: lag, the `noloadbalance` tag or a stopped replica).

## Findings {#findings}

Above the data of each tab is the [[ui:diag.findings.title]] block: rules check the data of the tool and name what needs attention. The block collapses; if there are no problems, it says “No problems found”.

A finding has a severity — [[ui:diag.severity.info]], [[ui:diag.severity.warning]] or [[ui:diag.severity.critical]] (color and a text label), a title with numbers and an expandable [[ui:diag.findings.fix]] hint. The buttons under a finding open the right tool on the right node; some findings have a ready command with the [[ui:diag.findings.copy]] button. The app does not run commands: you copy and run them yourself. Severity does not depend on the environment of the connection.

A rule that can fire on many objects (long queries, idle transactions, inactive slots, lagging replicas) gives one finding per node: the title shows the worst object, and the number of objects that reached the threshold is in the parameters. The exceptions are the root blocker and queued DDL: those get a finding per root and per item of the queue. In the tools of a cluster ([[ui:diag.tools.pulse]], [[ui:diag.tools.failover]], [[ui:diag.tools.health]]) findings belong to different nodes, and the node is shown next to each.

Thresholds: the first value is a warning, the second is critical.

- Connections in use: 80% and 95% of those available.
- Idle in transaction: 5 minutes and 30 minutes; an active query: 15 minutes and 1 hour; a transaction: 1 hour and 6 hours.
- XID age: 50% and 75% of the wraparound limit.
- A lock wait: 30 seconds and 5 minutes; a root blocker — a victim waits 10 seconds and 60 seconds, or 10 victims.
- The size of `pg_wal`: 2 and 4 times `max_wal_size`.
- xmin horizon: 10 million and 100 million transactions, or 1 hour and 6 hours.
- Replica lag: 16 MB and 1 GB, or 30 seconds and 5 minutes; a replica silent for 60 seconds.
- An inactive slot retains 1 GB and 10 GB of WAL.
- The share of `Lock` in the wait profile: 20% and 50% of samples.

Findings with the “Info” severity need no immediate action: a low cache hit in the interval (below 90% with at least 1000 blocks read), new temporary files, `idle_in_transaction_session_timeout` equal to 0, hidden query texts, a cycle of lock waits, recovery conflicts that have accumulated but do not grow. Deadlocks in the interval raise a warning in [[ui:diag.tools.pulse]], and the details exist only in the server log: the application should take locks in one order.

Rates and growth (recovery conflicts, subscription errors, deadlocks) are computed between two consecutive refreshes, so on the first refresh, after a server restart and after a statistics reset there are no findings about growth.
