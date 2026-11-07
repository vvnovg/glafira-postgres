//! Журнал MCP (спец. P4 §10.3): события, счётчики приложения, события P4b, ротация, усечение аргументов; поля
//! фонового запуска (P4c §9.2).

use std::fs::OpenOptions;
use std::io::Write;

use glafira_mcp_core::audit::{
    AuditConnection, AuditEvent, AuditLog, CallRecord, FILE_NAME, MAX_BYTES, Outcome, ROTATED_NAME,
    truncate_args,
};
use glafira_mcp_core::limits::{MAX_ARGS_AUDIT_BYTES, MAX_SQL_AUDIT_BYTES};
use serde_json::{Map, Value, json};

fn call(extra: Map<String, Value>) -> AuditEvent {
    AuditEvent::Call(CallRecord {
        session: "s1".into(),
        run: None,
        client: Some("qwen-code 0.26.7".into()),
        tool: "get_activity".into(),
        connection: Some(AuditConnection {
            id: "c1".into(),
            name: "lab".into(),
        }),
        member: Some("pg-1".into()),
        args: json!({"connectionId": "c1", "limit": 50}),
        outcome: Outcome::Ok,
        code: None,
        bytes: 10,
        ms: 3,
        extra,
    })
}

#[test]
fn records_lines_newest_first_with_app_counters() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    assert_eq!(log.path(), dir.path().join(FILE_NAME).as_path());
    log.record(AuditEvent::SessionStarted {
        session: "s1".into(),
        run: None,
    });
    let mut extra = Map::new();
    extra.insert("masked".into(), json!(4));
    log.record(call(extra));
    log.record(call(Map::new()));
    let r = log.tail(10);
    assert_eq!(r.len(), 3);
    assert_eq!(r[0]["event"], "call");
    assert!(
        r[0].get("masked").is_none(),
        "без счётчиков приложения — без полей"
    );
    assert_eq!(r[1]["masked"], 4);
    assert_eq!(r[1]["connection"]["name"], "lab");
    assert_eq!(r[1]["member"], "pg-1");
    assert_eq!(r[1]["outcome"], "ok");
    assert_eq!(r[1]["code"], Value::Null);
    assert_eq!(r[1]["args"]["limit"], 50);
    assert!(r[1]["ts"].as_u64().unwrap() > 0);
    assert_eq!(r[2]["event"], "sessionStarted");
    assert_eq!(log.tail(1).len(), 1);
}

#[test]
fn every_gateway_event_has_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    log.record(AuditEvent::ServerStarted {
        endpoint: "/tmp/x/mcp.sock".into(),
    });
    log.record(AuditEvent::Approved {
        session: "s1".into(),
        client: Some("qwen-code".into()),
        run: None,
        by: None,
    });
    log.record(AuditEvent::Rejected {
        session: "s2".into(),
        client: None,
    });
    log.record(AuditEvent::Revoked {
        client: "cursor".into(),
    });
    log.record(AuditEvent::SessionEnded {
        session: "s1".into(),
        client: Some("qwen-code".into()),
        calls: 7,
    });
    log.record(AuditEvent::ServerStopped);
    let names: Vec<String> = log
        .tail(10)
        .iter()
        .rev()
        .map(|r| r["event"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        [
            "serverStarted",
            "approved",
            "rejected",
            "revoked",
            "sessionEnded",
            "serverStopped"
        ]
    );
    let r = log.tail(10);
    assert_eq!(r[1]["calls"], 7);
    assert_eq!(r[5]["endpoint"], "/tmp/x/mcp.sock");
}

#[test]
fn app_events_are_written_with_their_kind() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    let mut fields = Map::new();
    fields.insert("requestId".into(), json!("r3a9f1c"));
    fields.insert("event".into(), json!("spoofed"));
    fields.insert("ts".into(), json!(1));
    log.record(AuditEvent::App {
        kind: "requestCreated".into(),
        fields,
    });
    let r = log.tail(1);
    assert_eq!(
        r[0]["event"], "requestCreated",
        "event из полей не подменяет имя события"
    );
    assert_eq!(r[0]["requestId"], "r3a9f1c");
    assert!(r[0]["ts"].as_u64().unwrap() > 1);
}

#[test]
fn custom_file_names() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::with_names(dir.path(), "a.jsonl", "a.1.jsonl");
    log.record(AuditEvent::ServerStopped);
    assert!(dir.path().join("a.jsonl").exists());
    assert!(!dir.path().join(FILE_NAME).exists());
    assert_eq!(log.tail(1)[0]["event"], "serverStopped");
}

#[cfg(unix)]
#[test]
fn file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    log.record(AuditEvent::ServerStopped);
    let mode = std::fs::metadata(log.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn rotates_at_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), vec![b' '; MAX_BYTES as usize]).unwrap();
    let log = AuditLog::new(dir.path());
    log.record(AuditEvent::ServerStopped);
    assert!(dir.path().join(ROTATED_NAME).exists());
    let current = std::fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
    assert_eq!(current.lines().count(), 1);
    assert_eq!(
        log.tail(5).len(),
        1,
        "старый файл из пробелов не разбирается"
    );
}

#[test]
fn broken_line_is_skipped_not_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    log.record(AuditEvent::ServerStopped);
    // Строка, оборванная посреди многобайтного символа (сбой записи), и строка не JSON.
    let mut f = OpenOptions::new().append(true).open(log.path()).unwrap();
    f.write_all(b"{\"event\":\"call\",\"tool\":\"\xd0\n{not json\n")
        .unwrap();
    log.record(AuditEvent::Revoked {
        client: "cursor".into(),
    });
    let r = log.tail(10);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0]["event"], "revoked");
    assert_eq!(r[1]["event"], "serverStopped");
}

#[test]
fn sql_is_kept_whole_and_other_args_are_cut() {
    let full = ["sql", "rewrittenSql"];
    let small = json!({"connectionId": "c1", "sql": "SELECT 1", "member": null});
    assert_eq!(truncate_args(small.clone(), &full), small);

    let sql = format!("SELECT {}", "x".repeat(10_000));
    let v = truncate_args(
        json!({"connectionId": "c1", "database": "perf", "sql": sql}),
        &full,
    );
    assert_eq!(v["sql"], sql, "SQL до 16 КБ — целиком");
    assert_eq!(v["database"], "perf");
    assert!(v.get("_truncated").is_none());

    let long_sql = "y".repeat(MAX_SQL_AUDIT_BYTES + 100);
    let v = truncate_args(
        json!({"connectionId": "c1", "params": vec!["p".repeat(100); 20], "sql": long_sql}),
        &full,
    );
    let cut = v["sql"].as_str().unwrap();
    assert!(cut.ends_with('…') && cut.len() == MAX_SQL_AUDIT_BYTES + '…'.len_utf8());
    assert_eq!(v["_truncated"], true);
    let rest = v["_rest"].as_str().unwrap();
    assert!(rest.ends_with('…') && rest.len() <= MAX_ARGS_AUDIT_BYTES + '…'.len_utf8());
    assert!(
        v.get("connectionId").is_none(),
        "остальные аргументы — только в _rest"
    );
}

#[test]
fn non_object_args_are_cut_to_a_string() {
    let v = truncate_args(json!(vec!["t".repeat(100); 50]), &["sql"]);
    let s = v.as_str().unwrap();
    assert!(s.ends_with('…') && s.len() <= MAX_ARGS_AUDIT_BYTES + '…'.len_utf8());
    assert_eq!(truncate_args(json!([1, 2]), &["sql"]), json!([1, 2]));
    assert_eq!(truncate_args(Value::Null, &["sql"]), Value::Null);
}

/// P4c §9.2: `run` у записей сессии фонового запуска, `by: "run"` у подтверждения по билету; у обычных сессий полей нет.
#[test]
fn background_run_fields_are_written_only_when_set() {
    let dir = tempfile::tempdir().unwrap();
    let log = AuditLog::new(dir.path());
    log.record(AuditEvent::SessionStarted {
        session: "s1".into(),
        run: Some("a1b2c3d".into()),
    });
    log.record(AuditEvent::Approved {
        session: "s1".into(),
        client: Some("qwen-code".into()),
        run: Some("a1b2c3d".into()),
        by: Some("run".into()),
    });
    log.record(AuditEvent::Approved {
        session: "s2".into(),
        client: Some("cursor".into()),
        run: None,
        by: None,
    });
    let AuditEvent::Call(mut rec) = call(Map::new()) else {
        unreachable!()
    };
    rec.run = Some("a1b2c3d".into());
    log.record(AuditEvent::Call(rec));
    let r = log.tail(4);
    assert_eq!(
        (r[0]["event"].as_str(), r[0]["run"].as_str()),
        (Some("call"), Some("a1b2c3d"))
    );
    assert!(
        r[1].get("run").is_none() && r[1].get("by").is_none(),
        "подтверждение в диалоге — без полей запуска: {}",
        r[1]
    );
    assert_eq!(
        (r[2]["run"].as_str(), r[2]["by"].as_str()),
        (Some("a1b2c3d"), Some("run"))
    );
    assert_eq!(r[3]["run"], "a1b2c3d");
    assert!(
        serde_json::to_value(call(Map::new()))
            .unwrap()
            .get("run")
            .is_none(),
        "у вызова обычной сессии поля нет"
    );
}
