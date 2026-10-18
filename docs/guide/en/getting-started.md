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

## Passwords {#passwords}

Passwords are kept in the system secret store (Keychain, Credential Manager, Secret Service). If no password
is set, `~/.pgpass` is used, as in `psql`.

## A role for diagnostics {#diagnostics-role}

Full diagnostics need the `pg_monitor` and `pg_signal_backend` roles; the role creation script is in the
connection test checklist.
