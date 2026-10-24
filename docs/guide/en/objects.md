# Tree and objects

This chapter explains the connection tree, the tabs that open for tables, views, functions and sequences, and the DDL that the app builds for a table.

## Tree {#tree}

Click the arrow next to a connection to open it: the app connects and shows what is inside. Each level loads only when you expand it:

- [[ui:sidebar.members]] — the nodes of a host list or a Patroni cluster, with roles, timeline and lag. A node you cannot reach, or one with a lag over 16 MB, is marked with a warning sign. Clicking a node opens the overview with that node highlighted. A single server has no such group.
- [[ui:sidebar.databases]] — the databases of the node that the catalog is read from, each with its size (a dash means you have no `CONNECT` privilege on the database). Template databases and databases that do not accept connections are not listed, and if the connection has [[ui:connections.form.databases]] set, only those databases are shown.
- Inside a database — its schemas, and the [[ui:sidebar.extensions]] group with the version of each extension and an arrow if a newer version is available.
- Inside a schema — the groups [[ui:sidebar.kinds.tables]], [[ui:sidebar.kinds.views]], [[ui:sidebar.kinds.matviews]], [[ui:sidebar.kinds.functions]] and [[ui:sidebar.kinds.sequences]], each with the number of objects in brackets. Empty groups are not shown.
- [[ui:sidebar.roles]] — the roles of the server, with a mark for superusers and for roles that cannot log in.

Long lists are loaded in pages of 500, and the page counter at the bottom says how many objects are shown out of how many there are. Use [[ui:sidebar.more]] to load the next page; a list of more than 200 objects scrolls and loads the next page as you reach the end. A group of more than 20 objects has a filter field: type part of a name, the search is a case-insensitive substring match done on the server, so it covers the whole list, not only the loaded part.

System objects are hidden: the `pg_catalog` and `information_schema` schemas and the `pg_*` roles. To show them, turn on [[ui:settings.showSystemObjects]] in [[ui:sidebar.settings]]. The `pg_toast` and `pg_temp` schemas are never shown.

A partitioned table has its own arrow next to the name: it expands the partitions, and the number of partitions is shown. Partitions are not repeated in the list of the schema. The size of a partitioned table is not computed in the tree (it shows a dash), because that would take a lock on every partition; the table tab shows it.

The catalog is read from one node of the connection. By default it is the leader or the primary; the connection menu can switch it to another node (see the chapter [Connections](connections.md#kinds)).

## Object tabs {#tabs}

Click the name of a table, view, function or sequence in the tree (not the arrow of a partitioned table) to open its tab. Opening the same object again switches to its tab. The tab title shows the schema and the name, and next to it the kind of the object: table, partitioned table, partition, foreign table, view, materialized view, function, procedure, aggregate function, window function or sequence.

Everything on the tabs is read-only. Data is read when the tab opens; [[ui:object.refresh]] reads it again. If the connection is lost and comes back, open tabs reload by themselves. If the object was dropped or renamed in the meantime, the tab says that it was not found.

**Table.** A comment on the table, if there is one, is shown above the sections. An unlogged table is marked [[ui:object.unlogged]]: it is not written to WAL, so it is not replicated to standbys and is emptied after a crash. The sections:

- [[ui:object.columns]] — name, type, `NOT NULL`, default, identity or generated expression, collation, statistics target, storage and comment.
- [[ui:object.indexes]] — definition, flags (`PRIMARY KEY`, `UNIQUE`, and `INVALID` for an index that is not usable), size, and usage from the statistics: index scans, entries read and the time of the last scan.
- [[ui:object.constraints]] — primary key, unique, foreign key, check and exclusion constraints with their definitions; a constraint that has not been validated is marked `NOT VALID`. Below, [[ui:object.incomingFks]] lists the tables that reference this one.
- [[ui:object.stats]] — the table data, indexes, TOAST and the total size; the estimated number of rows; sequential and index scans; live and dead rows; rows changed since the last analyze; the time and the number of runs of vacuum, autovacuum, analyze and autoanalyze; storage parameters, tablespace and owner. For a partitioned table, sizes and counters are summed over the whole partition tree, without locking the partitions.
- [[ui:object.partitioning]] — only for a partitioned table and for a partition: the partition key, the partition bound, the parent table and the list of partitions.
- [[ui:object.triggers]] — name, state (enabled, disabled, replica only or always) and definition.
- [[ui:object.ddl]] — see the next section.

**View and materialized view.** The owner is shown above the sections. A materialized view also shows whether it is populated and its size. The sections are [[ui:object.definition]] (the query text, with [[ui:object.copyDefinition]]), [[ui:object.columns]] and, for a materialized view, [[ui:object.indexes]].

**Function, procedure, aggregate.** The tab shows the signature, the return type, the language, the volatility, whether the function is `SECURITY DEFINER`, and the owner, followed by the definition with [[ui:object.copyDefinition]]. The tab is opened by the function identifier, so overloaded functions with the same name open separately. For an aggregate the definition cannot be built, and only the signature is shown.

**Sequence.** The tab shows the data type, the last value, the start, minimum and maximum values, the increment, whether the sequence cycles, the cache size, how much of the range is used, and the column that owns the sequence. The last value and the used range can be read only with the `SELECT` or `USAGE` privilege on the sequence; without it the tab shows [[ui:object.noPrivilege]] instead of a number. To fix this for the diagnostics role, grant `SELECT ON ALL SEQUENCES IN SCHEMA <schema>`, or the `pg_read_all_data` role (PostgreSQL 14 and newer).

## DDL {#ddl}

The [[ui:object.ddl]] section of a table shows the statements that would create the table. They are built by the app from the catalog, not by `pg_dump`, so treat the text as a reading aid and a starting point. The first line of the text says so:

```
-- DDL built by Glafira from the catalog; not a replacement for pg_dump (no privileges, RLS policies or owners)
```

What the text contains, in this order:

1. `CREATE TABLE` with columns, types, defaults, `NOT NULL`, identity and generated columns, then the primary key, unique, check and exclusion constraints, `PARTITION BY` or `PARTITION OF`, storage parameters (`WITH (…)`) and the tablespace. An unlogged table gets `UNLOGGED`.
2. For a partitioned table, a comment line with the names of the partitions. Open a partition's own tab to get its DDL.
3. Indexes that are not created by constraints. An index that is not valid is marked with a comment. For a partitioned table, the indexes are written without `ONLY`, so they apply to all partitions.
4. Foreign keys and constraints that are not validated, as separate `ALTER TABLE … ADD CONSTRAINT` statements. PostgreSQL ignores `NOT VALID` inside `CREATE TABLE`, so these constraints cannot stay there; `pg_dump` does the same.
5. Triggers, followed by `ALTER TABLE … DISABLE TRIGGER` (or `ENABLE REPLICA` and `ENABLE ALWAYS`) for triggers that are not in the default state.
6. `COMMENT ON TABLE` and `COMMENT ON COLUMN`.

What the text does not contain: privileges (`GRANT`), row-level security policies and owners. For a foreign table the app does not build the statement and says so in a comment, because the foreign server and its options are not read.

Identifiers are quoted the way PostgreSQL quotes them. String constants are written for `standard_conforming_strings = on` (the default); a string with a backslash is written as `E'…'`. Definitions of constraints, indexes, triggers and views always include schema names, whatever `search_path` the role has, and dates are in ISO format.

Click [[ui:object.copyDdl]] to copy the text to the clipboard. For the exact schema of a production database use `pg_dump --schema-only`.
