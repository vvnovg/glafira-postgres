//! Тексты для модели (спец. P4 §5.1, уточнения 4–5 плана P4a): английские `title` и `fix` находок и рекомендаций
//! берутся из словаря интерфейса `src/locales/en.json` (один источник с интерфейсом), и каталог инструментов
//! `list_diagnostics`. Параметры форматируются как во фронтенде (`src/components/diag/findingText.ts`,
//! `src/components/queries/plan/recommendText.ts`), но по-английски и без локали: «1 h 2 min», «1.5 GB», «49.5 %».

use std::collections::BTreeMap;
use std::sync::LazyLock;

use glafira_pg_mcp::model::{DiagToolInfo, FindingOut};
use pg_advisor::findings::{Finding, FindingTarget, Severity};
use pg_advisor::plan_rules::Recommendation;
use serde_json::{Value, json};

use crate::diag::Tool;
use crate::i18n::render;

/// Словарь en интерфейса, встроенный в бинарь.
static EN: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../src/locales/en.json"))
        .expect("src/locales/en.json is valid JSON")
});

/// Строка словаря по пути через точку (`findings.longQuery.title`); нет — `None`.
fn en(path: &str) -> Option<&'static str> {
    let mut v: &'static Value = &EN;
    for part in path.split('.') {
        v = v.get(part)?;
    }
    v.as_str()
}

/// Число без лишних нулей: не больше `digits` знаков после точки.
fn trim(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Байты: «512 B», «1.5 MB»; единица — по значению после округления до десятых, как у `formatBytes`.
fn bytes(n: f64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n.abs() < 1024.0 {
        return format!("{} B", n.round() as i64);
    }
    let (mut v, mut i) = (n / 1024.0, 0);
    while (v * 10.0).round() / 10.0 >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// Длительность: старшая единица и смежная за ней, если она не нулевая («1 h 2 min», «1 d», «42 s»), как у
/// `formatDuration`; отрицательное (сервер отдаёт −0,001 с) — «0 s».
fn duration(seconds: f64) -> String {
    const UNITS: [(u64, &str); 4] = [(86_400, "d"), (3_600, "h"), (60, "min"), (1, "s")];
    let s = seconds.max(0.0).floor() as u64;
    let i = UNITS
        .iter()
        .position(|(size, _)| s >= *size)
        .unwrap_or(UNITS.len() - 1);
    let (size, unit) = UNITS[i];
    let mut out = format!("{} {unit}", s / size);
    if let Some((next, next_unit)) = UNITS.get(i + 1) {
        let n = (s % size) / next;
        if n > 0 {
            out.push_str(&format!(" {n} {next_unit}"));
        }
    }
    out
}

/// Миллисекунды: до 10 — два знака, до 100 — один, до 1000 — целые; дальше секунды, от минуты — длительность.
fn millis(v: f64) -> String {
    if v <= 0.0 {
        "0 ms".to_string()
    } else if v < 1000.0 {
        let digits = if v < 10.0 {
            2
        } else if v < 100.0 {
            1
        } else {
            0
        };
        format!("{} ms", trim(v, digits))
    } else if v < 60_000.0 {
        format!("{} s", trim(v / 1000.0, 2))
    } else {
        duration((v / 1000.0).round())
    }
}

/// Значение параметра для подстановки — по суффиксу имени, как во фронтенде. `durations` — `…S` как длительность (у
/// находок; у рекомендаций такого правила нет, как в `recommendText.ts`).
fn format_value(key: &str, v: &Value, durations: bool) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => {
            let x = n.as_f64()?;
            Some(if key.ends_with("Bytes") {
                bytes(x)
            } else if key.ends_with("Kb") {
                bytes(x * 1024.0)
            } else if key.ends_with("Pct") {
                format!("{} %", trim(x, 1))
            } else if key.ends_with("Ms") {
                millis(x)
            } else if durations && key.ends_with('S') {
                duration(x)
            } else if key.ends_with("Oid") {
                n.to_string()
            } else if key == "ratio" {
                trim(x, if x < 10.0 { 1 } else { 0 })
            } else {
                trim(x, 2)
            })
        }
        other => Some(other.to_string()),
    }
}

fn severity_name(s: Severity) -> String {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
    .to_string()
}

/// Параметры текста находки (`findingParams` фронтенда): значения по суффиксам, узел, объект (`pid` сессии,
/// `queryId` и `database` запроса pgss, иначе `name`), подпись держателя горизонта (`kindLabel`), общий «лаг» из байт и
/// времени и, для отношения другой базы, `relation` вида «oid N in database X».
fn finding_params(f: &Finding) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = f
        .params
        .iter()
        .filter_map(|(k, v)| format_value(k, v, true).map(|s| (k.clone(), s)))
        .collect();
    if let Some(m) = &f.member {
        out.insert("member".into(), m.clone());
    }
    match &f.target {
        Some(FindingTarget::Session { pid, .. }) => {
            out.insert("pid".into(), pid.to_string());
        }
        Some(FindingTarget::Query { query_id, database }) => {
            out.insert("queryId".into(), query_id.clone());
            out.insert("database".into(), database.clone());
        }
        Some(
            FindingTarget::Slot { name }
            | FindingTarget::Member { name }
            | FindingTarget::Relation { name }
            | FindingTarget::Database { name },
        ) => {
            out.entry("name".into()).or_insert_with(|| name.clone());
        }
        None => {}
    }
    if let Some(kind) = f.params.get("kind").and_then(Value::as_str)
        && let Some(label) = en(&format!("diag.horizon.kinds.{kind}"))
    {
        out.insert("kindLabel".into(), label.into());
    }
    if f.params.contains_key("lagBytes") || f.params.contains_key("delayS") {
        let lag: Vec<String> = ["lagBytes", "delayS"]
            .iter()
            .filter_map(|k| out.get(*k).cloned())
            .collect();
        out.insert("lag".into(), lag.join(", "));
    }
    if !out.contains_key("relation")
        && let (Some(oid), Some(db)) = (out.get("relationOid"), out.get("relationDatabase"))
    {
        let params = BTreeMap::from([
            ("oid".to_string(), oid.clone()),
            ("db".to_string(), db.clone()),
        ]);
        let template = en("diag.locks.otherDb").unwrap_or("oid {{oid}} in database {{db}}");
        out.insert("relation".into(), render(template, &params));
    }
    out
}

/// Находка для модели (спец. §5.1): `{ id, severity, member?, target?, params, title, fix }`, ссылок интерфейса нет.
/// Нет текста в словаре (правило новее словаря) — `title` = id, `fix` пустой.
pub fn finding_out(f: &Finding) -> FindingOut {
    let p = finding_params(f);
    let text = |field: &str| en(&format!("findings.{}.{field}", f.id)).map(|t| render(t, &p));
    FindingOut {
        id: f.id.clone(),
        severity: severity_name(f.severity),
        member: f.member.clone(),
        target: f.target.as_ref().and_then(|t| serde_json::to_value(t).ok()),
        params: Value::Object(f.params.clone()),
        title: text("title").unwrap_or_else(|| f.id.clone()),
        fix: text("fix").unwrap_or_default(),
    }
}

/// Вариант текста рекомендации по её параметрам (`variant` в `recommendText.ts`): группа партиций, bitmap у
/// `plan_spill`, оценка без ANALYZE и приведение у `plan_no_pruning`, параллельный JIT.
fn variant(r: &Recommendation, field: &'static str) -> &'static str {
    let num = |k: &str| r.params.get(k).and_then(Value::as_f64);
    let text = |k: &str| r.params.get(k).and_then(Value::as_str);
    match (r.id.as_str(), field) {
        ("plan_seq_scan_filter", "title") if num("partitions").is_some() => "titlePartitions",
        ("plan_seq_scan_filter", "evidence") if num("partitions").is_some() => {
            if num("tablesMore").is_some_and(|m| m > 0.0) {
                "evidencePartitionsMore"
            } else {
                "evidencePartitions"
            }
        }
        ("plan_spill", "title") if text("kind") == Some("bitmap") => "titleBitmap",
        ("plan_spill", "evidence") if text("kind") == Some("bitmap") => "evidenceBitmap",
        ("plan_no_pruning", "evidence") if r.severity == Severity::Info => "evidencePlan",
        ("plan_no_pruning", "title") if text("rewrite").is_some() => "titleCast",
        ("plan_jit_heavy", "evidence") if num("processes").is_some_and(|n| n > 1.0) => {
            "evidenceParallel"
        }
        _ => field,
    }
}

/// Параметры текста рекомендации (`recommendParams`): числа по суффиксам, подписи направления ошибки оценки
/// (`directionLabel`) и вида сброса на диск (`kindLabel`).
fn recommend_params(r: &Recommendation) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = r
        .params
        .iter()
        .filter_map(|(k, v)| format_value(k, v, false).map(|s| (k.clone(), s)))
        .collect();
    if let Some(d) = r.params.get("direction").and_then(Value::as_str)
        && let Some(label) = en(&format!("recommend.direction.{d}"))
    {
        out.insert("directionLabel".into(), label.into());
    }
    if let Some(k) = r.params.get("kind").and_then(Value::as_str)
        && let Some(label) = en(&format!("recommend.spillKinds.{k}"))
    {
        out.insert("kindLabel".into(), label.into());
    }
    out
}

/// Рекомендация по плану для модели (уточнение 5): `title` — заголовок, `fix` — доказательство (`evidence`); готовая
/// команда и индекс-гипотеза — в `params.command` и `params.hypoIndex`; узлы плана — в `target`.
pub fn recommendation_out(r: &Recommendation) -> FindingOut {
    let p = recommend_params(r);
    let text = |field: &'static str| {
        en(&format!("recommend.{}.{}", r.id, variant(r, field))).map(|t| render(t, &p))
    };
    let mut params = r.params.clone();
    if let Some(c) = &r.command {
        params.insert("command".into(), Value::String(c.clone()));
    }
    if let Some(h) = &r.hypo {
        params.insert("hypoIndex".into(), Value::String(h.index_ddl.clone()));
    }
    FindingOut {
        id: r.id.clone(),
        severity: severity_name(r.severity),
        member: None,
        target: Some(json!({ "kind": "planNodes", "nodeIds": r.node_ids })),
        params: Value::Object(params),
        title: text("title").unwrap_or_else(|| r.id.clone()),
        fix: text("evidence").unwrap_or_default(),
    }
}

/// Все инструменты диагностики в порядке каталога (P5 добавит свои сюда и в `describe`).
pub const ALL_TOOLS: [Tool; 10] = [
    Tool::Pulse,
    Tool::Sessions,
    Tool::Waits,
    Tool::Locks,
    Tool::Horizon,
    Tool::Replication,
    Tool::Slots,
    Tool::Failover,
    Tool::Health,
    Tool::Queries,
];

/// id инструмента (`pulse`, `sessions`…) — как его принимает `run_diagnostic`.
pub fn tool_id(t: Tool) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Английское назначение инструмента и его параметры в `run_diagnostic` (кроме `connectionId` и `tool`).
fn describe(t: Tool) -> (&'static str, &'static [&'static str]) {
    match t {
        Tool::Pulse => (
            "Cluster pulse: connections, longest transactions and queries, lock waits, XID age, pg_wal size, replica lag and rates (TPS, cache hit, temp files, WAL) of every member",
            &[],
        ),
        Tool::Sessions => (
            "Sessions from pg_stat_activity with durations, waits, held locks and blockers, including idle in transaction",
            &["member"],
        ),
        Tool::Waits => (
            "Wait profile from the activity sampler for a window of 300, 900 or 3600 s: top wait events, queries, applications, users, databases and clients",
            &["member", "windowS"],
        ),
        Tool::Locks => (
            "Lock tree: root blockers and their victims, wait cycles, DDL queued behind long transactions",
            &["member"],
        ),
        Tool::Horizon => (
            "xmin horizon holders that keep VACUUM from removing dead rows: sessions, prepared transactions, replication slots, replica feedback",
            &["member"],
        ),
        Tool::Replication => (
            "Replication of the member: WAL senders and receiver, lag in bytes and time, sync state, recovery conflicts, replay pause",
            &["member"],
        ),
        Tool::Slots => (
            "Replication slots: retained WAL, lost and inactive slots, Patroni member slots, logical subscriptions and their errors",
            &["member"],
        ),
        Tool::Failover => (
            "Patroni failover readiness: candidates and their lag against maximum_lag_on_failover, nofailover tags, pause, synchronous mode, timings",
            &[],
        ),
        Tool::Health => (
            "Patroni REST health matrix: /primary, /replica, /health and other endpoints of every member",
            &[],
        ),
        Tool::Queries => (
            "Top queries from pg_stat_statements since the statistics reset or for a window of 300, 900 or 3600 s with heuristics (N+1, unstable time, low hit ratio, temp files, heavy planning); queries = {mode: {kind: sinceReset} | {kind: window, s}, sort, limit: 50 | 100 | 200, database?, search?}. At the diagnostics access level search matches the masked query text ($N in place of literals) within the top 200 queries by sort, so an empty result may mean the query is outside that window",
            &["member", "queries"],
        ),
    }
}

/// Каталог `list_diagnostics` (уточнение 4): строится из `diag::Tool`, крейт шлюза о составе инструментов не знает.
pub fn diag_catalog() -> Vec<DiagToolInfo> {
    ALL_TOOLS
        .iter()
        .map(|&t| {
            let (title, params) = describe(t);
            DiagToolInfo {
                id: tool_id(t),
                title: title.to_string(),
                scope: if t.is_cluster() { "cluster" } else { "node" }.to_string(),
                patroni_only: t.patroni_only(),
                params: params.iter().map(|s| s.to_string()).collect(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use pg_advisor::findings::{Finding, FindingTarget, Severity};
    use pg_advisor::plan_rules::Recommendation;
    use serde_json::json;

    use super::*;

    #[test]
    fn finding_title_and_fix_come_from_en_json_with_formatted_params() {
        let f = Finding::new("idleInTransaction", Severity::Warning)
            .member(Some("pg-1"))
            .param("maxS", 3725.4)
            .param("count", 2);
        let o = finding_out(&f);
        assert_eq!(o.title, "Idle in transaction for 1 h 2 min");
        assert!(
            o.fix.starts_with("Such transactions hold locks"),
            "{}",
            o.fix
        );
        assert_eq!(
            (o.id.as_str(), o.severity.as_str(), o.member.as_deref()),
            ("idleInTransaction", "warning", Some("pg-1"))
        );
        assert_eq!(o.params, json!({"maxS": 3725.4, "count": 2}));
        // Ссылки интерфейса (`links`) модели не передаются.
        assert!(serde_json::to_value(&o).unwrap().get("links").is_none());
    }

    #[test]
    fn target_kind_lag_and_other_database_relation_fill_the_template() {
        let horizon = Finding::new("xminHorizon", Severity::Critical)
            .target(FindingTarget::Slot {
                name: "dead_slot".into(),
            })
            .param("kind", "slot")
            .param("xminAge", 1_500_000);
        assert_eq!(
            finding_out(&horizon).title,
            "The xmin horizon is held by slot dead_slot (1500000 transactions)"
        );
        let lag = Finding::new("replicaLag", Severity::Warning)
            .param("lagBytes", 3 * 1024 * 1024)
            .param("delayS", 75.0);
        assert_eq!(finding_out(&lag).title, "Replica lag: 3.0 MB, 1 min 15 s");
        let ddl = Finding::new("ddlQueue", Severity::Critical)
            .param("count", 3)
            .param("relationOid", 16_384)
            .param("relationDatabase", "orders");
        assert_eq!(
            finding_out(&ddl).title,
            "DDL on oid 16384 in database orders is queued and blocks sessions: 3"
        );
        let root = Finding::new("rootBlocker", Severity::Critical)
            .target(FindingTarget::Session {
                pid: 4242,
                backend_start_us: 1,
            })
            .param("count", 3)
            .param("maxS", 65.0);
        let o = finding_out(&root);
        assert_eq!(
            o.title,
            "Session 4242 blocks sessions: 3, the longest wait 1 min 5 s"
        );
        assert_eq!(
            o.target,
            Some(json!({"kind": "session", "pid": 4242, "backendStartUs": 1}))
        );
    }

    #[test]
    fn unknown_finding_id_keeps_the_id_as_title() {
        let o = finding_out(&Finding::new("fromTheFuture", Severity::Info));
        assert_eq!((o.title.as_str(), o.fix.as_str()), ("fromTheFuture", ""));
    }

    #[test]
    fn recommendation_text_picks_the_variant_by_params() {
        let grouped = Recommendation::new("plan_seq_scan_filter", Severity::Warning, 3)
            .param("table", "shop.orders")
            .param("partitions", 4)
            .param("tables", "a, b, c, d")
            .param("tablesHead", "a, b, c")
            .param("tablesMore", 1)
            .param("rowsRead", 1000)
            .param("rowsReturned", 5)
            .command("CREATE INDEX CONCURRENTLY ON shop.orders (status);")
            .hypo("CREATE INDEX ON shop.orders (status)");
        let o = recommendation_out(&grouped);
        assert_eq!(
            o.title,
            "Index on filter columns in 4 partitions: shop.orders and others"
        );
        assert_eq!(
            o.fix,
            "Seq Scans of a, b, c and 1 more read 1000 rows and returned 5"
        );
        assert_eq!(
            o.params["command"],
            "CREATE INDEX CONCURRENTLY ON shop.orders (status);"
        );
        assert_eq!(
            o.params["hypoIndex"],
            "CREATE INDEX ON shop.orders (status)"
        );
        assert_eq!(o.target, Some(json!({"kind": "planNodes", "nodeIds": [3]})));
        let plain = Recommendation::new("plan_seq_scan_filter", Severity::Warning, 1)
            .param("table", "t")
            .param("rowsRead", 10)
            .param("rowsReturned", 1)
            .param("loops", 1);
        let o = recommendation_out(&plain);
        assert_eq!(o.title, "Index on filter columns: t");
        assert_eq!(o.fix, "Seq Scan read 10 rows and returned 1 (1 loops)");
        let spill = Recommendation::new("plan_spill", Severity::Warning, 2)
            .param("kind", "sort")
            .param("spaceKb", 2048)
            .param("workMemKb", 1024);
        let o = recommendation_out(&spill);
        assert_eq!(o.title, "Sort spills to disk");
        assert_eq!(
            o.fix,
            "2.0 MB used on disk; work_mem for the session or role: 1.0 MB"
        );
    }

    /// id находок и рекомендаций в рабочем коде `pg-advisor`: строка первым аргументом `Finding::new`,
    /// `Recommendation::new` и помощников правил (`finding`, `session_finding`, `push`). Модули тестов в конце файлов
    /// (`#[cfg(test)] mod tests`) отбрасываются: в них — выдуманные id.
    fn advisor_ids() -> BTreeSet<String> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/pg-advisor/src"),
            &mut files,
        );
        let mut ids = BTreeSet::new();
        for f in files {
            let src = std::fs::read_to_string(&f).unwrap().replace("\r\n", "\n");
            let code = src
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or_default();
            for callee in ["Finding::new(", "Recommendation::new(", "finding(", "push("] {
                for (i, _) in code.match_indices(callee) {
                    let Some(rest) = code[i + callee.len()..].trim_start().strip_prefix('"') else {
                        continue;
                    };
                    let id: String = rest
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                        .collect();
                    if !id.is_empty() && rest[id.len()..].starts_with('"') {
                        ids.insert(id);
                    }
                }
            }
        }
        ids
    }

    #[test]
    fn every_finding_and_recommendation_of_pg_advisor_has_english_texts() {
        let ids = advisor_ids();
        assert!(ids.len() >= 40, "поиск id в pg-advisor сломался: {ids:?}");
        for id in &ids {
            let (title, fix) = if id.starts_with("plan_") {
                (
                    format!("recommend.{id}.title"),
                    format!("recommend.{id}.evidence"),
                )
            } else {
                (format!("findings.{id}.title"), format!("findings.{id}.fix"))
            };
            assert!(
                en(&title).is_some_and(|t| !t.is_empty()),
                "нет {title} в en.json"
            );
            assert!(
                en(&fix).is_some_and(|t| !t.is_empty()),
                "нет {fix} в en.json"
            );
        }
        for (id, v) in EN["findings"].as_object().unwrap() {
            assert!(
                v["title"].is_string() && v["fix"].is_string(),
                "findings.{id}"
            );
        }
    }

    /// Номер инструмента в `ALL_TOOLS`: исчерпывающий `match` не даст собрать тесты, пока новый инструмент (P5) не
    /// получит место в каталоге.
    fn position(t: Tool) -> usize {
        match t {
            Tool::Pulse => 0,
            Tool::Sessions => 1,
            Tool::Waits => 2,
            Tool::Locks => 3,
            Tool::Horizon => 4,
            Tool::Replication => 5,
            Tool::Slots => 6,
            Tool::Failover => 7,
            Tool::Health => 8,
            Tool::Queries => 9,
        }
    }

    #[test]
    fn catalog_lists_every_tool_once_with_scope_and_params() {
        for (i, t) in ALL_TOOLS.iter().enumerate() {
            assert_eq!(position(*t), i);
        }
        let c = diag_catalog();
        let ids: Vec<&str> = c.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "pulse",
                "sessions",
                "waits",
                "locks",
                "horizon",
                "replication",
                "slots",
                "failover",
                "health",
                "queries"
            ]
        );
        let by = |id: &str| c.iter().find(|t| t.id == id).unwrap();
        assert_eq!(
            (by("pulse").scope.as_str(), by("pulse").patroni_only),
            ("cluster", false)
        );
        assert_eq!(
            (by("failover").scope.as_str(), by("failover").patroni_only),
            ("cluster", true)
        );
        assert_eq!(
            (by("locks").scope.as_str(), by("locks").patroni_only),
            ("node", false)
        );
        assert_eq!(by("waits").params, ["member", "windowS"]);
        assert_eq!(by("queries").params, ["member", "queries"]);
        assert!(by("health").params.is_empty());
        assert!(c.iter().all(|t| !t.title.is_empty() && t.title.is_ascii()));
    }

    #[test]
    fn formats_follow_the_interface() {
        assert_eq!(bytes(512.0), "512 B");
        assert_eq!(bytes(1_048_575.0), "1.0 MB");
        assert_eq!(bytes(1.5 * 1024.0 * 1024.0 * 1024.0), "1.5 GB");
        assert_eq!(duration(86_405.0), "1 d");
        assert_eq!(duration(42.9), "42 s");
        assert_eq!(duration(-0.001), "0 s");
        assert_eq!(millis(0.4234), "0.42 ms");
        assert_eq!(millis(12.34), "12.3 ms");
        assert_eq!(millis(1250.0), "1.25 s");
        assert_eq!(millis(90_000.0), "1 min 30 s");
    }
}

/// Английский текст предупреждения о роли (спец. §4.5) для `list_connections.roleWarning`; `dangerous` — из
/// `pg_advisor::readiness::dangerous_rights` (`"superuser"` первым).
pub fn role_warning_text(role: &str, dangerous: &[String]) -> String {
    let superuser = dangerous.first().is_some_and(|r| r == "superuser");
    let abilities: Vec<&str> = if superuser {
        vec![
            "terminate sessions",
            "write data",
            "read server files",
            "run programs on the server",
        ]
    } else {
        dangerous
            .iter()
            .filter_map(|r| match r.as_str() {
                "pg_signal_backend" => Some("terminate sessions"),
                "pg_write_all_data" => Some("write data"),
                "pg_read_server_files" => Some("read server files"),
                "pg_execute_server_program" => Some("run programs on the server"),
                _ => None,
            })
            .collect()
    };
    let who = if superuser {
        format!("Role \"{role}\" is a superuser")
    } else {
        format!("Role \"{role}\" is a member of {}", dangerous.join(", "))
    };
    format!(
        "{who}: through run_select a model that gets past the denied function list could {}. Glafira only warns; for a \
         test stand connect with a role that only reads (pg_read_all_data).",
        join_and(&abilities)
    )
}

/// «a», «a and b», «a, b and c».
fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod p4b_tests {
    use super::role_warning_text;

    #[test]
    fn role_warning_names_the_rights_and_what_they_allow() {
        let su = role_warning_text("postgres", &["superuser".into()]);
        assert_eq!(
            su,
            "Role \"postgres\" is a superuser: through run_select a model that gets past the denied function list could \
             terminate sessions, write data, read server files and run programs on the server. Glafira only warns; for a \
             test stand connect with a role that only reads (pg_read_all_data)."
        );
        let m = role_warning_text("app", &["pg_read_server_files".into()]);
        assert!(
            m.starts_with("Role \"app\" is a member of pg_read_server_files: "),
            "{m}"
        );
        assert!(m.contains("could read server files."), "{m}");
    }
}
