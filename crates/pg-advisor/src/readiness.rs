//! Чек-лист готовности к диагностике (спец. P1 §10): факты о роли, настройках и расширениях → пункты.
//! Тексты пунктов — в словарях интерфейса по `id`; `fix` — SQL или строка конфигурации, не переводится.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const OPTIONAL_EXTENSIONS: [&str; 6] = [
    "hypopg",
    "pgstattuple",
    "pg_buffercache",
    "pg_wait_sampling",
    "pg_qualstats",
    "pg_stat_kcache",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionFact {
    pub name: String,
    pub installed: Option<String>,
    pub available: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessFacts {
    pub server_version_num: i32,
    pub user: String,
    pub database: String,
    pub is_superuser: bool,
    /// Встроенные роли, в которые пользователь входит (транзитивно).
    pub member_of: Vec<String>,
    /// Участник → есть ли `pg_stat_statements` в `shared_preload_libraries` (только доступные).
    pub preload_by_member: Vec<(String, bool)>,
    pub pgss_installed: bool,
    pub compute_query_id: Option<String>,
    pub track_io_timing: String,
    pub track_activity_query_size: i64,
    pub extensions: Vec<ExtensionFact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Ok,
    Warn,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadinessItem {
    pub id: &'static str,
    pub status: Status,
    pub params: BTreeMap<String, String>,
    pub fix: Option<String>,
}

fn item(
    id: &'static str,
    status: Status,
    params: &[(&str, String)],
    fix: Option<String>,
) -> ReadinessItem {
    ReadinessItem {
        id,
        status,
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        fix,
    }
}

/// Имя роли в SQL: простое — как есть, иначе в двойных кавычках.
fn quote_ident(name: &str) -> String {
    let simple = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if simple {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

const PRELOAD_FIX: &str = "shared_preload_libraries = 'pg_stat_statements'";

pub fn evaluate(f: &ReadinessFacts) -> Vec<ReadinessItem> {
    let has = |role: &str| f.is_superuser || f.member_of.iter().any(|r| r == role);
    let user = quote_ident(&f.user);
    let grant = |role: &str| Some(format!("GRANT {role} TO {user};"));
    let mut out = Vec::new();

    out.push(if has("pg_monitor") {
        item("monitorRole", Status::Ok, &[], None)
    } else {
        item("monitorRole", Status::Missing, &[], grant("pg_monitor"))
    });
    out.push(if has("pg_signal_backend") {
        item("signalRole", Status::Ok, &[], None)
    } else {
        item(
            "signalRole",
            Status::Missing,
            &[],
            grant("pg_signal_backend"),
        )
    });
    out.push(
        if f.is_superuser || (f.server_version_num >= 170_000 && has("pg_maintain")) {
            item("maintainRole", Status::Ok, &[], None)
        } else if f.server_version_num >= 170_000 {
            item(
                "maintainRole",
                Status::Warn,
                &[("reason", "grant".into())],
                grant("pg_maintain"),
            )
        } else {
            item(
                "maintainRole",
                Status::Warn,
                &[("reason", "ownerOnly".into())],
                None,
            )
        },
    );

    let missing: Vec<&str> = f
        .preload_by_member
        .iter()
        .filter(|(_, loaded)| !loaded)
        .map(|(m, _)| m.as_str())
        .collect();
    let preloaded_everywhere = !f.preload_by_member.is_empty() && missing.is_empty();
    out.push(if f.preload_by_member.is_empty() {
        item(
            "pgssPreloaded",
            Status::Warn,
            &[("reason", "unknown".into())],
            None,
        )
    } else if missing.is_empty() {
        item("pgssPreloaded", Status::Ok, &[], None)
    } else if missing.len() == f.preload_by_member.len() {
        item(
            "pgssPreloaded",
            Status::Missing,
            &[],
            Some(PRELOAD_FIX.into()),
        )
    } else {
        item(
            "pgssPreloaded",
            Status::Warn,
            &[("members", missing.join(", "))],
            Some(PRELOAD_FIX.into()),
        )
    });
    out.push(if f.pgss_installed {
        item("pgssInstalled", Status::Ok, &[], None)
    } else {
        item(
            "pgssInstalled",
            Status::Missing,
            &[("database", f.database.clone())],
            Some("CREATE EXTENSION IF NOT EXISTS pg_stat_statements;".into()),
        )
    });

    let fix_qid = Some("compute_query_id = on".to_string());
    out.push(if f.server_version_num < 140_000 {
        item(
            "computeQueryId",
            Status::Warn,
            &[("reason", "version".into())],
            None,
        )
    } else {
        match f.compute_query_id.as_deref() {
            Some("on") => item("computeQueryId", Status::Ok, &[], None),
            Some("auto") if preloaded_everywhere => item("computeQueryId", Status::Ok, &[], None),
            Some("auto") => item("computeQueryId", Status::Warn, &[], fix_qid),
            _ => item("computeQueryId", Status::Missing, &[], fix_qid),
        }
    });
    out.push(if f.track_io_timing == "on" {
        item("trackIoTiming", Status::Ok, &[], None)
    } else {
        item(
            "trackIoTiming",
            Status::Warn,
            &[],
            Some("track_io_timing = on".into()),
        )
    });
    out.push(if f.track_activity_query_size >= 4096 {
        item("trackActivityQuerySize", Status::Ok, &[], None)
    } else {
        item(
            "trackActivityQuerySize",
            Status::Warn,
            &[("value", f.track_activity_query_size.to_string())],
            Some("track_activity_query_size = 4096".into()),
        )
    });

    for name in OPTIONAL_EXTENSIONS {
        let fact = f.extensions.iter().find(|e| e.name == name);
        out.push(match fact {
            Some(ExtensionFact {
                installed: Some(v), ..
            }) => item(
                "extension",
                Status::Ok,
                &[("name", name.into()), ("version", v.clone())],
                None,
            ),
            Some(ExtensionFact {
                available: true, ..
            }) => item(
                "extension",
                Status::Warn,
                &[("name", name.into())],
                Some(format!("CREATE EXTENSION IF NOT EXISTS {name};")),
            ),
            _ => item("extension", Status::Missing, &[("name", name.into())], None),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> ReadinessFacts {
        ReadinessFacts {
            server_version_num: 170_002,
            user: "glafira_diag".into(),
            database: "postgres".into(),
            is_superuser: false,
            member_of: vec![
                "pg_monitor".into(),
                "pg_signal_backend".into(),
                "pg_maintain".into(),
            ],
            preload_by_member: vec![("a".into(), true), ("b".into(), true)],
            pgss_installed: true,
            compute_query_id: Some("auto".into()),
            track_io_timing: "on".into(),
            track_activity_query_size: 4096,
            extensions: vec![
                ExtensionFact {
                    name: "hypopg".into(),
                    installed: Some("1.4.1".into()),
                    available: true,
                },
                ExtensionFact {
                    name: "pgstattuple".into(),
                    installed: None,
                    available: true,
                },
            ],
        }
    }

    fn get<'a>(items: &'a [ReadinessItem], id: &str) -> &'a ReadinessItem {
        items.iter().find(|i| i.id == id).unwrap()
    }

    fn ext<'a>(items: &'a [ReadinessItem], name: &str) -> &'a ReadinessItem {
        items
            .iter()
            .find(|i| i.id == "extension" && i.params["name"] == name)
            .unwrap()
    }

    #[test]
    fn healthy_setup_is_ok() {
        let items = evaluate(&facts());
        let ids: Vec<&str> = items.iter().map(|i| i.id).take(8).collect();
        assert_eq!(
            ids,
            [
                "monitorRole",
                "signalRole",
                "maintainRole",
                "pgssPreloaded",
                "pgssInstalled",
                "computeQueryId",
                "trackIoTiming",
                "trackActivityQuerySize"
            ]
        );
        assert!(
            items[..8].iter().all(|i| i.status == Status::Ok),
            "{items:?}"
        );
        assert_eq!(items.len(), 8 + OPTIONAL_EXTENSIONS.len());
        assert_eq!(ext(&items, "hypopg").status, Status::Ok);
        assert_eq!(ext(&items, "hypopg").params["version"], "1.4.1");
        let pst = ext(&items, "pgstattuple");
        assert_eq!(pst.status, Status::Warn);
        assert_eq!(
            pst.fix.as_deref(),
            Some("CREATE EXTENSION IF NOT EXISTS pgstattuple;")
        );
        assert_eq!(ext(&items, "pg_qualstats").status, Status::Missing);
        assert_eq!(ext(&items, "pg_qualstats").fix, None);
    }

    #[test]
    fn missing_roles_get_grants_with_quoted_user() {
        let mut f = facts();
        f.member_of.clear();
        f.user = "App User".into();
        let items = evaluate(&f);
        assert_eq!(get(&items, "monitorRole").status, Status::Missing);
        assert_eq!(
            get(&items, "monitorRole").fix.as_deref(),
            Some("GRANT pg_monitor TO \"App User\";")
        );
        assert_eq!(
            get(&items, "signalRole").fix.as_deref(),
            Some("GRANT pg_signal_backend TO \"App User\";")
        );
        assert_eq!(get(&items, "maintainRole").status, Status::Warn);
        assert_eq!(get(&items, "maintainRole").params["reason"], "grant");
    }

    #[test]
    fn superuser_has_everything() {
        let mut f = facts();
        f.member_of.clear();
        f.is_superuser = true;
        let items = evaluate(&f);
        for id in ["monitorRole", "signalRole", "maintainRole"] {
            assert_eq!(get(&items, id).status, Status::Ok, "{id}");
        }
    }

    #[test]
    fn old_versions() {
        let mut f = facts();
        f.server_version_num = 130_015;
        f.compute_query_id = None;
        let items = evaluate(&f);
        assert_eq!(get(&items, "maintainRole").params["reason"], "ownerOnly");
        assert_eq!(get(&items, "maintainRole").fix, None);
        assert_eq!(get(&items, "computeQueryId").status, Status::Warn);
        assert_eq!(get(&items, "computeQueryId").params["reason"], "version");
    }

    #[test]
    fn pg_stat_statements_preload() {
        let mut f = facts();
        f.preload_by_member = vec![("a".into(), true), ("b".into(), false)];
        let items = evaluate(&f);
        let p = get(&items, "pgssPreloaded");
        assert_eq!(p.status, Status::Warn);
        assert_eq!(p.params["members"], "b");
        assert_eq!(
            get(&items, "computeQueryId").status,
            Status::Warn,
            "auto без загрузки везде"
        );

        f.preload_by_member = vec![("a".into(), false)];
        f.pgss_installed = false;
        let items = evaluate(&f);
        assert_eq!(get(&items, "pgssPreloaded").status, Status::Missing);
        assert_eq!(get(&items, "pgssInstalled").status, Status::Missing);
        assert_eq!(get(&items, "pgssInstalled").params["database"], "postgres");

        f.preload_by_member.clear();
        assert_eq!(
            get(&evaluate(&f), "pgssPreloaded").params["reason"],
            "unknown"
        );
    }

    #[test]
    fn settings() {
        let mut f = facts();
        f.compute_query_id = Some("off".into());
        f.track_io_timing = "off".into();
        f.track_activity_query_size = 1024;
        let items = evaluate(&f);
        assert_eq!(get(&items, "computeQueryId").status, Status::Missing);
        assert_eq!(
            get(&items, "computeQueryId").fix.as_deref(),
            Some("compute_query_id = on")
        );
        assert_eq!(get(&items, "trackIoTiming").status, Status::Warn);
        let q = get(&items, "trackActivityQuerySize");
        assert_eq!(q.status, Status::Warn);
        assert_eq!(q.params["value"], "1024");
    }

    #[test]
    fn json_shape() {
        let v = serde_json::to_value(&evaluate(&facts())[0]).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"id": "monitorRole", "status": "ok", "params": {}, "fix": null})
        );
    }
}
