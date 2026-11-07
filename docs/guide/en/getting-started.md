# Getting started

Glafira for PostgreSQL helps you understand what is going on with PostgreSQL and a Patroni cluster: which
nodes are alive, who the leader is, how far replicas lag and how the databases are laid out.

## First connection {#first-connection}

1. Click “+” above the connection tree.
2. Choose a kind: **Server** (a single PostgreSQL), **Host list** (several servers, the primary is detected
   automatically) or **Patroni cluster** (Patroni REST API addresses).
3. You can paste a connection string right away — JDBC, `postgresql://…` or `host=… port=…`: the fields
   are filled in for you.
4. Click “Test”: the app shows which steps passed and a diagnostics readiness checklist.
5. Save the connection and expand it in the tree.

To connect through a bastion, see [SSH tunnel](connections.md#ssh); for the tree and the tabs of tables, views, functions and sequences, see [Tree](objects.md#tree) and [Object tabs](objects.md#tabs).

## Keys {#keys}

On macOS the shortcuts are in the application menu (File, View, Window, Help); on Windows and Linux the same shortcuts work with Ctrl instead of ⌘. Buttons that have a shortcut show it next to the label.

- ⌘N — new connection; ⌘W — close the tab; ⇧⌘W — close the window.
- ⌘R — Refresh on the open screen: a diagnostics tool, an object tab, a query card, the actions journal.
- ⌘⏎ — Plan in the EXPLAIN console and in a query card (a plan without execution).
- ⌘F — the filter or search of the open screen (sessions, Queries, the guide), or the connection filter if the screen has none; ⇧⌘F — the connection filter right away.
- ⌘1…⌘9 — a tab by its number; ⌃Tab and ⌃⇧Tab — the next and the previous tab.
- ⌘, — Settings (the open section stays); ⇧⌘/ and F1 — the guide chapter about the open screen.

The window size and position are remembered between launches.

## Passwords {#passwords}

Passwords are kept in the system secret store (Keychain, Credential Manager, Secret Service). If no password
is set, `~/.pgpass` is used, as in `psql`.

## A role for diagnostics {#diagnostics-role}

Full diagnostics need the `pg_monitor` and `pg_signal_backend` roles; the role creation script is in the
connection test checklist. The last value of a sequence is shown only to a role with the `SELECT` or `USAGE` privilege on it; see [Object tabs](objects.md#tabs).
