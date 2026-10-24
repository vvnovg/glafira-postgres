# Connections

This chapter covers the connection kinds, pasting a connection string, the SSH tunnel, the Patroni settings and the readiness checklist. A connection is created and edited in a form: click “+” above the connection tree, or choose [[ui:sidebar.editConnection]] in the connection menu (⋯).

## Connection kinds {#kinds}

The [[ui:connections.form.kind]] field has three values:

- [[ui:connections.form.kinds.server]] — one PostgreSQL server: a host and a port. The tree has no [[ui:sidebar.members]] group, and the role of the server, primary or standby, is shown next to the connection name.
- [[ui:connections.form.kinds.hostList]] — several servers (up to 16), for example a primary and its replicas without Patroni. The app asks each server whether it is in recovery, so the primary is found automatically, like with a multi-host JDBC URL. If no node is primary, or several are, the overview shows a warning (several primaries may mean split-brain). [[ui:connections.form.target]] sets which node the catalog is read from by default: [[ui:connections.form.targets.primary]], [[ui:connections.form.targets.standby]], [[ui:connections.form.targets.preferStandby]] or [[ui:connections.form.targets.any]].
- [[ui:connections.form.kinds.patroni]] — a Patroni cluster. Instead of nodes you give from 1 to 8 [[ui:connections.form.patroniUrls]]. The app reads the members, their roles, lag and timeline from the Patroni REST API and connects to the PostgreSQL server of each member at the address Patroni reports.

To turn a host list into a Patroni cluster, tick [[ui:connections.form.underPatroni]]: the API addresses are filled in from the hosts, with port 8008.

The nodes are polled every 10 seconds. For a host list or a Patroni cluster, the connection menu lets you choose the node the catalog is read from: [[ui:sidebar.catalogDefault]] or one item per node (“Catalog — from node …”). If the chosen node becomes unavailable, the catalog is read from the default one, and the tree says so.

## Connection string {#connection-string}

Paste a connection string into [[ui:connections.form.connstr]] and click [[ui:connections.form.connstrApply]]. The form fields are filled in, and the string is cleared from the field. Three formats are understood:

- JDBC: `jdbc:postgresql://h1:5432,h2:5433/orders?user=app&targetServerType=primary&ssl=true`
- URI: `postgresql://app:secret@h1,h2:5433/orders?target_session_attrs=read-write&sslmode=verify-full` (`postgres://` works too)
- conninfo: `host=h1,h2 port=5432,5433 dbname=orders user=app sslmode=require`

One host gives a [[ui:connections.form.kinds.server]], several hosts give a [[ui:connections.form.kinds.hostList]]; a string never gives a Patroni cluster. Each host may have its own port; otherwise the common `port` is used, otherwise 5432. The user, the database, `sslmode`, the CA, certificate and key files, the connect timeout and the target node (`target_session_attrs`, `targetServerType`) are filled in as well. A JDBC string with `ssl=true` and no `sslmode` means `verify-full`, as in the PostgreSQL JDBC driver.

A password in the string goes to the password field and is saved to the system secret store when you save the connection, never to `connections.json`. Without a saved password the app uses `~/.pgpass`, as `psql` does. Unix sockets are not supported.

Parameters the app does not use (`ApplicationName`, `currentSchema`, `options` and so on) are skipped. This is not an error: they are listed under the field after “Not recognized and skipped”.

## SSH tunnel {#ssh}

If the nodes can be reached only through a bastion (a jump host), turn on [[ui:connections.form.ssh.enabled]] in the [[ui:connections.form.ssh.section]] section. All connections of the profile then go through the tunnel: PostgreSQL nodes, the Patroni REST API, the connection test.

**How it works.** The app starts the system OpenSSH client, one `ssh` process per connection, with dynamic forwarding (`-D`): a SOCKS5 port that listens on 127.0.0.1 only. Node names are resolved on the bastion, not on your computer, so the bastion must see the nodes by the names written in the profile: hosts, Patroni members, API addresses. New Patroni members need no extra setup. TLS certificates are checked against the original host name, not against 127.0.0.1. On the bastion, sshd must allow forwarding (`AllowTcpForwarding yes`). The SOCKS port has no password of its own: on a computer shared by several users (a terminal server, a shared jump host), anyone logged in there can use it while the connection is open and reach whatever your bastion account reaches, so on such a machine use the tunnel only if your administrator has limited forwarding on the bastion.

**Fields.**

- [[ui:connections.form.ssh.target]] — a host name or an alias from `~/.ssh/config`. Your `~/.ssh/config` is read, so ProxyJump, the SSH agent, hardware keys and per-host options work. The app, however, sets a few options on the `ssh` command line, and they win over the file: `ControlMaster=no` and `ControlPath=none` (the tunnel never joins a shared connection), `StrictHostKeyChecking=ask`, `ServerAliveInterval=15` with `ServerAliveCountMax=3`, `NumberOfPasswordPrompts=1`, `BatchMode=no`, `ExitOnForwardFailure=yes` and, for [[ui:connections.form.ssh.auths.key]] and [[ui:connections.form.ssh.auths.password]], `PreferredAuthentications` (with `IdentitiesOnly=yes` when a key file is chosen).
- [[ui:connections.form.ssh.port]] and [[ui:connections.form.ssh.user]] — leave them empty to take the values from `~/.ssh/config`. Enter the user name in the [[ui:connections.form.ssh.user]] field, not as `user@host` in the target; a fully qualified name such as `name@domain` is accepted there. The target may contain only Latin letters, digits, “.”, “_” and “-”; the user name may also contain “@”.
- [[ui:connections.form.ssh.auth]] — how to log in to the bastion:
  - [[ui:connections.form.ssh.auths.agent]] — keys from the SSH agent, including password-manager agents and hardware keys. Nothing is stored in the app.
  - [[ui:connections.form.ssh.auths.key]] — a private key file; choose it in the field that appears below. Only this key is offered to the server. For an encrypted key, also enter [[ui:connections.form.ssh.passphrase]].
  - [[ui:connections.form.ssh.auths.password]] — the login password, entered in [[ui:connections.form.ssh.password]].

The passphrase and the password are kept in the system secret store, never in `connections.json`, in exported files, in the diagnostics archive or in logs.

**OpenSSH version.** A password or a key passphrase needs OpenSSH 8.4 or newer: the app hands them over through ssh's own prompt, which older clients do not support. The SSH agent and a key without a passphrase work with any version. Windows 10 comes with OpenSSH 8.1: for a password or a passphrase install Win32-OpenSSH 8.4 or newer. The app answers only the standard password and key passphrase prompts; one-time codes and other questions of the server cannot be answered, so for such a bastion use an agent or a key. If `ssh` is not found, install the OpenSSH client (Windows: the “OpenSSH Client” optional feature).

**First login: the host key.** If the key of the bastion is not in `~/.ssh/known_hosts` yet, the app shows the [[ui:hostKey.title]] dialog with the [[ui:hostKey.host]] and the [[ui:hostKey.fingerprint]]. Compare the fingerprint with the one your administrator gave you. [[ui:hostKey.trust]] repeats what you were doing (connecting, reconnecting or testing), and OpenSSH itself writes the key to `~/.ssh/known_hosts`; the app never edits that file. If you cancel, nothing is trusted.

**The host key has changed.** The app reports that the host key of the bastion has changed and offers nothing to click: this may be an attack. Check with the administrator, then fix `~/.ssh/known_hosts` yourself, for example with `ssh-keygen -R <host>`.

**Checking.** [[ui:connections.form.test]] starts with the [[ui:test.step.ssh]] step: it shows the OpenSSH version and that the tunnel is up. For a connection without a tunnel the step says [[ui:test.sshNotUsed]].

**Lost tunnel.** If the `ssh` process dies, for example because the network is gone (a dead connection is noticed in about a minute), the connection becomes [[ui:sidebar.state.degraded]], the nodes behind the tunnel are marked unreachable (a Patroni cluster keeps the last known members, since the member list itself comes through the tunnel), and the app restarts the tunnel with pauses of 1, 2, 5 and 10 seconds, then every 30 seconds.

A refused login is different. If the bastion rejects the login (a wrong password, a key that is not accepted, an agent that is locked at the moment), or the host key is not trusted or has changed, or `ssh` is not found, the app stops retrying so as not to hammer the bastion with failed logins. The connection stays degraded until you fix the cause and choose [[ui:sidebar.reopen]] in the connection menu.

**Errors.** “The bastion … could not reach host:port” means that the tunnel is up but that node is not reachable from the bastion: check that the node name resolves on the bastion and that its port is open from there. “The SSH tunnel to … is not running (local port … refused)” means that the `ssh` process has just stopped; the app restarts it, and the details under the message show the reason. “Could not bind a local port for the SSH tunnel” means that the local port was taken twice in a row; try again.

**Logs.** The output of each `ssh` process goes to `ssh-<connection id>.log` in the log folder (its path is shown next to [[ui:about.logDir]] on the [[ui:sidebar.about]] screen); `ssh-<connection id>.1.log` is the previous run. Both files are included in the diagnostics archive.

**Limitation.** Run only one copy of the app: a second copy, when it starts, stops the `ssh` processes it finds from a previous run, including those of the first copy.

## Patroni {#patroni}

For a [[ui:connections.form.kinds.patroni]] connection, enter the Patroni REST API in [[ui:connections.form.patroniUrls]]: from 1 to 8 addresses starting with `http://` or `https://`, without `user:password@`. The app asks them in order until one answers; the member list comes from `GET /cluster`. It only reads: it never changes the cluster.

- [[ui:connections.form.patroniUser]] and [[ui:connections.form.patroniPassword]] — basic authentication, if the API requires it. The password goes to the system secret store. If all your API addresses are `https://`, the app does not send the password to a member that reports an `http://` address.
- [[ui:connections.form.apiTls]] — TLS settings for `https://` addresses: a CA file instead of the system roots, host name verification and a client certificate (PEM, or PKCS#12). A client certificate is needed if Patroni requires one (`restapi.verify_client: required`).
- [[ui:connections.form.overrides]] — for members whose reported addresses your computer cannot reach (a pod IP, NAT). For a member name, set the PostgreSQL host and port and the address of its API. Behind an SSH tunnel the bastion resolves the names that Patroni reports, so overrides are usually not needed there.

In the tree and in the overview the members are shown with their roles (Leader, Sync, Replica), lag and timeline. If the Patroni API stops answering, the app keeps showing the last known data and a warning.

## Readiness checklist {#readiness}

Click [[ui:connections.form.test]] in the form: the connection is checked without being saved. The steps run one after another: [[ui:test.step.ssh]], [[ui:test.step.patroni]] (only for a Patroni cluster), [[ui:test.step.discovery]], then [[ui:test.step.node]] for each node (TCP, TLS, login, version, role), and last [[ui:test.step.readiness]]. A failed step shows the error and its details; for SSH errors the details are the last lines of the `ssh` output.

The [[ui:test.readinessTitle]] list shows what the app needs for full diagnostics. Each item is [[ui:test.readyStatus.ok]], [[ui:test.readyStatus.warn]] or [[ui:test.readyStatus.missing]], says what stops working without it and offers the exact SQL or configuration line to copy:

- [[ui:readiness.monitorRole.title]] — the role is a superuser or a member of `pg_monitor`. Without it, queries of other users, `pg_stat_statements` and the size of `pg_wal` are not visible.
- [[ui:readiness.signalRole.title]] — membership in `pg_signal_backend`.
- [[ui:readiness.maintainRole.title]] — `pg_maintain` (PostgreSQL 17 and newer); before 17, `VACUUM` and `ANALYZE` work only for the user's own tables.
- `pg_stat_statements` — loaded through `shared_preload_libraries` on every node (this needs a restart; in Patroni, change it with `patronictl edit-config`), and created in the database with `CREATE EXTENSION`. Create it in every database where you need query analysis.
- `compute_query_id`, `track_io_timing` and `track_activity_query_size` (at least 4096).
- Optional extensions that give extra checks if they are installed: `hypopg`, `pgstattuple`, `pg_buffercache`, `pg_wait_sampling`, `pg_qualstats`, `pg_stat_kcache`.

If the monitoring role or the signal role is missing, the [[ui:test.roleScript]] section opens a script that creates the `glafira_diag` role; the same script is in the administrator guide. See also [A role for diagnostics](getting-started.md#diagnostics-role). The test uses its own short-lived connections to every node, so keep some headroom in `max_connections` and in the role limits.

A connection works without these items, but the missing ones limit what the app can show.
