//! Функции, запрещённые в SQL от модели, и поиск их вызовов (спец. P4 §7.2).
//!
//! `ADMIN_FUNCTIONS` — перенос из `src-tauri/src/explain/tier.rs`: ярус EXPLAIN ANALYZE P3 берёт список отсюда.
//! `DENIED_FUNCTIONS` — он же и функции, которые читают файлы сервера и большие объекты, ходят на другие серверы, меняют
//! настройки, держат блокировки, спят, шлют уведомления, читают логическую репликацию или выполняют SQL из строки.
//! Шаблон с `*` на конце — префикс имени; сравнение без учёта регистра и схемы, кроме схемы `net` (pg_net) — она
//! запрещена целиком. Поиск в тексте выражений — для VERBOSE-плана пробы (спец. §7.3, шаг 3): план печатает и вызовы,
//! которых разбор не видит (функциональная запись `(row).f`, вызовы из представлений).

use serde_json::Value;

use crate::sqltext::{ident_cont, skip_quoted};

/// Административные функции и (необязательно) добавки к ним: один источник для `ADMIN_FUNCTIONS` и
/// `DENIED_FUNCTIONS`.
macro_rules! admin_functions {
    ($($extra:literal),* $(,)?) => {
        &[
            "pg_cancel_backend",
            "pg_terminate_backend",
            "pg_reload_conf",
            "pg_rotate_logfile",
            "pg_stat_reset",
            "pg_stat_reset_shared",
            "pg_stat_reset_single_table_counters",
            "pg_stat_reset_single_function_counters",
            "pg_stat_reset_slru",
            "pg_stat_reset_replication_slot",
            "pg_stat_reset_subscription_stats",
            "pg_stat_statements_reset",
            "pg_stat_kcache_reset",
            "pg_wait_sampling_reset_profile",
            "pg_create_physical_replication_slot",
            "pg_create_logical_replication_slot",
            "pg_drop_replication_slot",
            "pg_copy_physical_replication_slot",
            "pg_copy_logical_replication_slot",
            "pg_replication_slot_advance",
            "pg_switch_wal",
            "pg_create_restore_point",
            "pg_promote",
            "pg_wal_replay_pause",
            "pg_wal_replay_resume",
            "pg_log_backend_memory_contexts",
            "pg_logical_emit_message",
            "pg_file_write",
            "pg_file_rename",
            "pg_file_unlink",
            "lo_export",
            "dblink",
            "dblink_exec",
            "dblink_send_query",
            "http",
            "http_get",
            "http_post",
            "http_put",
            "http_patch",
            "http_delete",
            "http_head",
            $($extra,)*
        ]
    };
}

/// Функции, которые выполняются и внутри `BEGIN READ ONLY`: завершают сессии, сбрасывают статистику, перечитывают
/// конфигурацию, удаляют и создают слоты, переключают WAL, повышают реплику, ходят наружу. EXPLAIN ANALYZE такого
/// `SELECT` выполнил бы функцию ярусом чтения — без подтверждения и журнала. Список закрывает случай частично: вызовы
/// внутри тела другой функции или триггера в плане не видны.
pub const ADMIN_FUNCTIONS: &[&str] = admin_functions!();

/// Запрещённые в SQL модели (`explain_query`, `hypothetical_index`, `run_select`, предложение P4b) на любой глубине.
pub const DENIED_FUNCTIONS: &[&str] = admin_functions!(
    // Файлы сервера и большие объекты (`lo_import`, `lo_export`…); размер `pg_wal` модель видит в пульсе.
    "lo_*",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_stat_file",
    "pg_ls_*",
    // Другие серверы и внешние вызовы (dblink, pgsql-http; pg_net — схема `net`).
    "dblink*",
    "http*",
    // Настройки, блокировки, ожидание, уведомления.
    "set_config",
    "pg_advisory*",
    "pg_try_advisory*",
    "pg_sleep*",
    "pg_notify",
    // Логическая репликация, резервное копирование (имена PG 13–14 и 15+), статистика, каталог.
    "pg_logical_slot_*",
    "pg_replication_origin_*",
    "pg_backup_start",
    "pg_backup_stop",
    "pg_start_backup",
    "pg_stop_backup",
    "pg_stat_reset*",
    "pg_sync_replication_slots",
    "pg_log_standby_snapshot",
    "pg_import_system_collations",
    // SQL из строки и выгрузка целых таблиц, схем и баз.
    "query_to_xml*",
    "cursor_to_xml*",
    "ts_stat",
    "ts_rewrite",
    "table_to_xml*",
    "schema_to_xml*",
    "database_to_xml*",
);

/// Функции, которые выполняют SQL из строки-аргумента (шаблоны с `*` — префиксы). После переноса поиска вызовов в
/// `calls_in_text` литерал с SQL не виден, поэтому ярус EXPLAIN ANALYZE (`explain::tier`) считает вызов такой функции
/// записью: внутри строки может быть `pg_terminate_backend(…)`. Подмножество `DENIED_FUNCTIONS`.
pub const SQL_FROM_STRING: &[&str] = &["query_to_xml*", "cursor_to_xml*", "ts_stat", "ts_rewrite"];

/// Схемы, все функции которых запрещены: pg_net (`net.http_get`, `net._http_collect_response`…).
pub const DENIED_SCHEMAS: &[&str] = &["net"];

/// Имя подходит под шаблон: точно или, если шаблон кончается на `*`, по префиксу; без учёта регистра.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name
            .as_bytes()
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes())),
        None => name.eq_ignore_ascii_case(pattern),
    }
}

/// Функция в `DENIED_FUNCTIONS`; схему отбрасывает вызывающий.
pub fn is_denied(name: &str) -> bool {
    DENIED_FUNCTIONS.iter().any(|p| matches_pattern(p, name))
}

/// Как `is_denied`, плюс схемы, запрещённые целиком (`DENIED_SCHEMAS`).
pub fn is_denied_qualified(schema: Option<&str>, name: &str) -> bool {
    schema.is_some_and(|s| DENIED_SCHEMAS.iter().any(|d| s.eq_ignore_ascii_case(d)))
        || is_denied(name)
}

/// Вызов в тексте выражения: имя и схема, если она указана (`a.b.f(` — схема `b`).
struct Call {
    schema: Option<String>,
    name: String,
}

impl Call {
    /// Имя для сообщения: со схемой, если запрещена именно схема.
    fn shown(&self) -> String {
        match &self.schema {
            Some(s) if !is_denied(&self.name) => format!("{s}.{}", self.name),
            _ => self.name.clone(),
        }
    }
}

/// Вызовы `имя(` и `схема.имя(` в тексте выражения (VERBOSE-план: `Output`, условия, `Function Call`). Идентификаторы —
/// как в лексере PostgreSQL, в кавычках `"…"` — с удвоенной кавычкой; строки `'…'` и `E'…'` пропускаются: имя в
/// литерале — не вызов. Пробелы между именем и скобкой допустимы (`pg_reload_conf ()`).
fn calls(expr: &str) -> Vec<Call> {
    let b = expr.as_bytes();
    let mut found = Vec::new();
    // Части текущего составного имени `a.b.c`; `after_dot` — после последней части стоит точка.
    let mut parts: Vec<String> = Vec::new();
    let mut after_dot = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let part: Option<String> = match c {
            _ if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b'"' => {
                let end = skip_quoted(b, i + 1, b'"', false);
                let inner = &expr[i + 1..end];
                let name = inner
                    .strip_suffix('"')
                    .unwrap_or(inner)
                    .replace("\"\"", "\"");
                i = end;
                Some(name)
            }
            b'e' | b'E' if b.get(i + 1) == Some(&b'\'') => {
                i = skip_quoted(b, i + 2, b'\'', true);
                None
            }
            b'\'' => {
                i = skip_quoted(b, i + 1, b'\'', false);
                None
            }
            b'.' if !parts.is_empty() && !after_dot => {
                after_dot = true;
                i += 1;
                continue;
            }
            b'(' if !parts.is_empty() && !after_dot => {
                let name = parts.pop().unwrap_or_default();
                found.push(Call {
                    schema: parts.pop(),
                    name,
                });
                i += 1;
                None
            }
            _ if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                let start = i;
                while b.get(i).is_some_and(|x| ident_cont(*x)) {
                    i += 1;
                }
                Some(expr[start..i].to_string())
            }
            // Число (и его хвост, как `1e5`) — не имя.
            _ if c.is_ascii_digit() => {
                while b.get(i).is_some_and(|x| ident_cont(*x)) {
                    i += 1;
                }
                None
            }
            _ => {
                i += 1;
                None
            }
        };
        match part {
            Some(part) => {
                if !after_dot {
                    parts.clear();
                }
                parts.push(part);
            }
            None => parts.clear(),
        }
        after_dot = false;
    }
    found
}

/// Имена из `names` (шаблоны с `*` — префиксы), вызванные в тексте выражения, в порядке появления, без повторов.
pub fn calls_in_text(expr: &str, names: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for call in calls(expr) {
        if names.iter().any(|p| matches_pattern(p, &call.name)) && !found.contains(&call.name) {
            found.push(call.name);
        }
    }
    found
}

/// Все строки JSON по порядку обхода (массивы — по порядку, объекты — по порядку ключей).
fn for_each_string(v: &Value, f: &mut dyn FnMut(&str)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(items) => {
            for x in items {
                for_each_string(x, f);
            }
        }
        Value::Object(o) => {
            for x in o.values() {
                for_each_string(x, f);
            }
        }
        _ => {}
    }
}

/// `calls_in_text` по всем строкам VERBOSE-плана (`Output`, условия, `Function Call`…), без повторов.
pub fn calls_in_plan(raw: &Value, names: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for_each_string(raw, &mut |s| {
        for name in calls_in_text(s, names) {
            if !found.contains(&name) {
                found.push(name);
            }
        }
    });
    found
}

/// Первое запрещённое имя (`DENIED_FUNCTIONS`, `DENIED_SCHEMAS`) в строках VERBOSE-плана; `None` — запрещённых нет.
/// Проба плана ловит расхождения разбора модели и сервера (спец. §7.3, шаг 3).
pub fn denied_in_plan(raw: &Value) -> Option<String> {
    let mut first = None;
    for_each_string(raw, &mut |s| {
        if first.is_none() {
            first = calls(s)
                .into_iter()
                .find(|c| is_denied_qualified(c.schema.as_deref(), &c.name))
                .map(|c| c.shown());
        }
    });
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn admin_functions_moved_from_tier_unchanged() {
        assert_eq!(ADMIN_FUNCTIONS.len(), 41);
        assert_eq!(ADMIN_FUNCTIONS[0], "pg_cancel_backend");
        assert_eq!(ADMIN_FUNCTIONS[40], "http_head");
        assert!(
            ADMIN_FUNCTIONS.iter().all(|n| !n.ends_with('*')),
            "административные — точные имена"
        );
    }

    /// Запрещённый список — административные функции и шаблоны спеки §7.2, без повторов.
    #[test]
    fn denied_list_is_admin_functions_plus_spec_patterns() {
        assert_eq!(&DENIED_FUNCTIONS[..ADMIN_FUNCTIONS.len()], ADMIN_FUNCTIONS);
        assert_eq!(
            &DENIED_FUNCTIONS[ADMIN_FUNCTIONS.len()..],
            [
                "lo_*",
                "pg_read_file",
                "pg_read_binary_file",
                "pg_stat_file",
                "pg_ls_*",
                "dblink*",
                "http*",
                "set_config",
                "pg_advisory*",
                "pg_try_advisory*",
                "pg_sleep*",
                "pg_notify",
                "pg_logical_slot_*",
                "pg_replication_origin_*",
                "pg_backup_start",
                "pg_backup_stop",
                "pg_start_backup",
                "pg_stop_backup",
                "pg_stat_reset*",
                "pg_sync_replication_slots",
                "pg_log_standby_snapshot",
                "pg_import_system_collations",
                "query_to_xml*",
                "cursor_to_xml*",
                "ts_stat",
                "ts_rewrite",
                "table_to_xml*",
                "schema_to_xml*",
                "database_to_xml*",
            ]
        );
        let mut unique = DENIED_FUNCTIONS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), DENIED_FUNCTIONS.len(), "без повторов");
        assert_eq!(DENIED_SCHEMAS, ["net"]);
    }

    #[test]
    fn sql_from_string_is_a_subset_of_denied() {
        for p in SQL_FROM_STRING {
            assert!(DENIED_FUNCTIONS.contains(p), "{p}");
        }
    }

    #[test]
    fn patterns_match_by_prefix_and_ignore_case() {
        for name in [
            "pg_sleep",
            "PG_SLEEP_FOR",
            "pg_sleep_until",
            "lo_import",
            "lo_from_bytea",
            "dblink",
            "dblink_connect_u",
            "pg_ls_dir",
            "pg_ls_waldir",
            "query_to_xmlschema",
            "table_to_xml_and_xmlschema",
            "http",
            "http_get",
            "pg_advisory_unlock_all",
            "pg_try_advisory_xact_lock",
            "pg_logical_slot_peek_binary_changes",
            "pg_replication_origin_session_setup",
            "Set_Config",
            "pg_terminate_backend",
            "ts_stat",
            "TS_REWRITE",
            "pg_start_backup",
            "pg_stop_backup",
            "pg_stat_reset_backend_stats",
            "pg_stat_reset_anything_future",
            "pg_sync_replication_slots",
            "pg_log_standby_snapshot",
        ] {
            assert!(is_denied(name), "{name}");
        }
        for name in [
            "lower",
            "lo",
            "loread",
            "pg_ls",
            "pg_lsn_in",
            "current_setting",
            "pg_size_pretty",
            "pg_stat_get_activity",
            "pg_stat_files",
            "set_configs",
            "pg_current_wal_lsn",
            "pg_listening_channels",
            "xhttp_get",
            "now",
            "ts_rank",
            "ts_headline",
            "ts_stats",
        ] {
            assert!(!is_denied(name), "{name}");
        }
        assert!(is_denied_qualified(Some("net"), "_http_collect_response"));
        assert!(is_denied_qualified(Some("NET"), "anything"));
        assert!(is_denied_qualified(Some("pg_catalog"), "set_config"));
        assert!(is_denied_qualified(None, "pg_sleep"));
        assert!(!is_denied_qualified(Some("public"), "lower"));
        assert!(
            !is_denied_qualified(Some("network"), "lower"),
            "схема net — целиком, network — нет"
        );
    }

    /// Случаи P3 (`tier.rs`): имя внутри другого идентификатора, без скобки или в литерале — не вызов; схема перед
    /// именем и пробел перед скобкой — вызов.
    #[test]
    fn calls_in_text_keeps_the_p3_behaviour() {
        assert_eq!(
            calls_in_text(
                "pg_terminate_backend(pg_stat_activity.pid)",
                ADMIN_FUNCTIONS
            ),
            ["pg_terminate_backend"]
        );
        assert_eq!(
            calls_in_text("public.dblink_exec('x'::text, 'y'::text)", ADMIN_FUNCTIONS),
            ["dblink_exec"]
        );
        for quiet in [
            "my_pg_terminate_backend(1)",
            "pg_terminate_backend_count",
            "pg_reload_conf_x(1)",
            "'pg_switch_wal'::text",
            "(pg_stat_activity.usename = 'app'::name)",
        ] {
            assert!(calls_in_text(quiet, ADMIN_FUNCTIONS).is_empty(), "{quiet}");
        }
        assert_eq!(
            calls_in_text("pg_catalog.pg_reload_conf ()", ADMIN_FUNCTIONS),
            ["pg_reload_conf"]
        );
    }

    #[test]
    fn calls_in_text_skips_literals_and_reads_quoted_names() {
        assert!(
            calls_in_text("'pg_sleep(1)'::text", DENIED_FUNCTIONS).is_empty(),
            "имя в литерале"
        );
        assert!(
            calls_in_text(r"E'\'pg_sleep(1)'::text", DENIED_FUNCTIONS).is_empty(),
            "E-строка"
        );
        assert_eq!(
            calls_in_text(r#""pg_sleep"(1)"#, DENIED_FUNCTIONS),
            ["pg_sleep"]
        );
        assert_eq!(
            calls_in_text(
                "public.dblink_connect('c'::text, 'host=x'::text)",
                DENIED_FUNCTIONS
            ),
            ["dblink_connect"]
        );
        assert_eq!(
            calls_in_text(
                "(lower(a) = 'x'::text) AND (pg_sleep_for('00:01:00'::interval) IS NOT NULL)",
                DENIED_FUNCTIONS
            ),
            ["pg_sleep_for"]
        );
        assert_eq!(
            calls_in_text(
                "pg_sleep(1), pg_sleep(2), set_config('a'::text, 'b'::text, false)",
                DENIED_FUNCTIONS
            ),
            ["pg_sleep", "set_config"],
            "в порядке появления, без повторов"
        );
        for quiet in [
            "(id = ANY ('{1,2}'::integer[]))",
            "t.http_status, (x)::numeric(12,2)",
            "count(*)",
        ] {
            assert!(calls_in_text(quiet, DENIED_FUNCTIONS).is_empty(), "{quiet}");
        }
    }

    /// Фикстура P3 (`tier.rs`): все строки плана, в том числе вложенные `Plans` и `Function Call`.
    #[test]
    fn calls_in_plan_walks_every_string_of_the_verbose_plan() {
        let raw = json!([{"Plan": {
            "Node Type": "Seq Scan",
            "Output": ["pg_terminate_backend(pg_stat_activity.pid)", "pid"],
            "Filter": "(pg_stat_activity.usename = 'app'::name)",
            "Plans": [{"Node Type": "Function Scan", "Function Call": "public.dblink_exec('x'::text, 'y'::text)"}]
        }, "Settings": {"search_path": "pg_catalog"}}]);
        assert_eq!(
            calls_in_plan(&raw, ADMIN_FUNCTIONS),
            ["pg_terminate_backend", "dblink_exec"]
        );
        assert_eq!(
            calls_in_plan(&raw, DENIED_FUNCTIONS),
            ["pg_terminate_backend", "dblink_exec"]
        );
        assert!(calls_in_plan(&json!({"Relation Name": "dblink"}), DENIED_FUNCTIONS).is_empty());
    }

    #[test]
    fn denied_in_plan_finds_a_denied_call() {
        let ls = json!([{"Plan": {"Node Type": "Function Scan", "Function Call": "pg_ls_dir('.'::text)", "Output": ["pg_ls_dir"]}}]);
        assert_eq!(denied_in_plan(&ls).as_deref(), Some("pg_ls_dir"));
        let net = json!({"Plan": {"Node Type": "Result", "Output": ["net._http_collect_response('1'::bigint, true)"]}});
        assert_eq!(
            denied_in_plan(&net).as_deref(),
            Some("net._http_collect_response"),
            "запрещена схема — имя со схемой"
        );
        // Функциональная запись `(row).http` в плане печатается обычным вызовом.
        let row = json!({"Plan": {"Node Type": "Result", "Output": [
            "http(ROW('GET'::http_method, 'http://x'::character varying, NULL::http_header[], NULL::character varying, NULL::character varying)::http_request)"
        ]}});
        assert_eq!(denied_in_plan(&row).as_deref(), Some("http"));
        let clean = json!({"Plan": {
            "Node Type": "Seq Scan",
            "Relation Name": "dblink",
            "Output": ["lower(c.email)", "now()", "c.http"],
            "Filter": "((c.note = 'pg_sleep(1)'::text) AND (c.id = ANY ('{1,2}'::integer[])))"
        }});
        assert_eq!(denied_in_plan(&clean), None);
    }
}
