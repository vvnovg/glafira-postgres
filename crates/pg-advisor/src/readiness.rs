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
    /// Атрибут `REPLICATION` (`pg_roles.rolreplication`): удаление слотов (P2b).
    #[serde(default)]
    pub has_replication: bool,
    /// Встроенные роли, в которые пользователь входит (транзитивно).
    pub member_of: Vec<String>,
    /// Участник → есть ли `pg_stat_statements` в `shared_preload_libraries` (только доступные).
    pub preload_by_member: Vec<(String, bool)>,
    /// Участник → библиотеки из `shared_preload_libraries` (только узлы, где значение видно роли): шаги
    /// включения расширений, которым нужна предзагрузка.
    #[serde(default)]
    pub preload_libraries_by_member: Vec<(String, Vec<String>)>,
    pub pgss_installed: bool,
    /// `pg_extension.extversion` расширения (P3): колонки топа запросов зависят от неё.
    #[serde(default)]
    pub pgss_version: Option<String>,
    /// `pg_stat_statements.track_planning`; `None` — настройка не видна (библиотека не загружена).
    #[serde(default)]
    pub pgss_track_planning: Option<String>,
    /// Член `pg_read_all_data` (14+): EXPLAIN запросов приложения читает его таблицы.
    #[serde(default)]
    pub read_all_data: bool,
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

/// Имя роли в SQL: всегда в двойных кавычках (`"` внутри удваивается). Правило «простое имя — как есть»
/// ломается на зарезервированных словах (`user`, `select`, `current_user`): `GRANT … TO user` — другая роль
/// или ошибка синтаксиса, а кавычки безопасны для любого имени.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Расширения, которым нужна библиотека в `shared_preload_libraries` (и перезапуск сервера).
const NEEDS_PRELOAD: [&str; 2] = ["pg_stat_kcache", "pg_wait_sampling"];

/// Шаги включения расширения по факту состояния сервера (`params.enable` — вариант текста): нет на сервере →
/// пакет PGDG (`package` для apt, `rpm` для yum/dnf) → предзагрузка с перезапуском, если нужна и библиотеки
/// ещё нет → `CREATE EXTENSION`. Предзагрузка считается сделанной, только если библиотека есть на всех узлах,
/// где список виден; не виден нигде — вариант «проверьте сами».
fn enable_steps(f: &ReadinessFacts, name: &str, available: bool) -> Vec<(&'static str, String)> {
    let libs = &f.preload_libraries_by_member;
    let loaded = !libs.is_empty() && libs.iter().all(|(_, l)| l.iter().any(|x| x == name));
    let preload = NEEDS_PRELOAD.contains(&name);
    let variant = match (available, preload) {
        (false, true) if !loaded => "packagePreload",
        (false, _) => "package",
        (true, false) => "create",
        (true, true) if loaded => "create",
        (true, true) if libs.is_empty() => "preloadUnknown",
        (true, true) => "preload",
    };
    let major = f.server_version_num / 10_000;
    vec![
        ("enable", variant.into()),
        (
            "package",
            format!("postgresql-{major}-{}", name.replace('_', "-")),
        ),
        ("rpm", format!("{name}_{major}")),
    ]
}

/// Узлы, где список `shared_preload_libraries` виден и библиотеки расширения с предзагрузкой в нём нет; список не
/// виден нигде или предзагрузка не нужна — пусто (утверждать нечего).
fn not_loaded_on(f: &ReadinessFacts, name: &str) -> Vec<String> {
    if !NEEDS_PRELOAD.contains(&name) {
        return Vec::new();
    }
    f.preload_libraries_by_member
        .iter()
        .filter(|(_, l)| !l.iter().any(|x| x == name))
        .map(|(m, _)| m.clone())
        .collect()
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
    // Атрибут даёт больше, чем удаление слотов (логические слоты читают изменения всех таблиц), поэтому пункт —
    // предупреждение, а не «нет обязательного»; docs/admin.md советует отдельную роль для действий.
    out.push(if f.is_superuser || f.has_replication {
        item("replicationAttr", Status::Ok, &[], None)
    } else {
        item(
            "replicationAttr",
            Status::Warn,
            &[],
            Some(format!("ALTER ROLE {user} REPLICATION;")),
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

    // Планирование считается только при `track_planning = on`; без расширения настройки нет — пункт не нужен.
    if f.pgss_installed {
        out.push(if f.pgss_track_planning.as_deref() == Some("on") {
            item("pgssTrackPlanning", Status::Ok, &[], None)
        } else {
            item(
                "pgssTrackPlanning",
                Status::Warn,
                &[],
                Some("pg_stat_statements.track_planning = on".into()),
            )
        });
    }
    // EXPLAIN запроса приложения читает его таблицы: нужна роль `pg_read_all_data` (14+) или права на таблицы.
    out.push(if f.is_superuser || f.read_all_data {
        item("readAllData", Status::Ok, &[], None)
    } else if f.server_version_num >= 140_000 {
        item("readAllData", Status::Warn, &[], grant("pg_read_all_data"))
    } else {
        item(
            "readAllData",
            Status::Warn,
            &[("reason", "version".into())],
            None,
        )
    });

    const HINTED: [&str; 3] = ["hypopg", "pg_stat_kcache", "pg_wait_sampling"];
    for name in OPTIONAL_EXTENSIONS {
        let fact = f.extensions.iter().find(|e| e.name == name);
        let mut params: Vec<(&str, String)> = vec![("name", name.into())];
        if HINTED.contains(&name) {
            params.push(("hint", name.into()));
            if !matches!(
                fact,
                Some(ExtensionFact {
                    installed: Some(_),
                    ..
                })
            ) {
                params.extend(enable_steps(f, name, fact.is_some_and(|e| e.available)));
            }
        }
        out.push(match fact {
            Some(ExtensionFact {
                installed: Some(v), ..
            }) => {
                params.push(("version", v.clone()));
                // Создано, но библиотека не загружена (убрана из preload, реплика без preload): расширение не работает,
                // «Запросы» и карточка его не используют — предупреждение с шагами preload (M6 ревью волны смоука P3b).
                let unloaded = not_loaded_on(f, name);
                if HINTED.contains(&name) && !unloaded.is_empty() {
                    params.push(("reason", "notLoaded".into()));
                    params.push(("enable", "loadCreated".into()));
                    params.push(("members", unloaded.join(", ")));
                    item("extension", Status::Warn, &params, None)
                } else {
                    item("extension", Status::Ok, &params, None)
                }
            }
            Some(ExtensionFact {
                available: true, ..
            }) => item(
                "extension",
                Status::Warn,
                &params,
                Some(format!("CREATE EXTENSION IF NOT EXISTS {name};")),
            ),
            _ => item("extension", Status::Missing, &params, None),
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
            has_replication: true,
            member_of: vec![
                "pg_monitor".into(),
                "pg_signal_backend".into(),
                "pg_maintain".into(),
                "pg_read_all_data".into(),
            ],
            preload_by_member: vec![("a".into(), true), ("b".into(), true)],
            preload_libraries_by_member: vec![],
            pgss_installed: true,
            pgss_version: Some("1.11".into()),
            pgss_track_planning: Some("on".into()),
            read_all_data: true,
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
        let ids: Vec<&str> = items.iter().map(|i| i.id).take(11).collect();
        assert_eq!(
            ids,
            [
                "monitorRole",
                "signalRole",
                "replicationAttr",
                "maintainRole",
                "pgssPreloaded",
                "pgssInstalled",
                "computeQueryId",
                "trackIoTiming",
                "trackActivityQuerySize",
                "pgssTrackPlanning",
                "readAllData"
            ]
        );
        assert!(
            items[..11].iter().all(|i| i.status == Status::Ok),
            "{items:?}"
        );
        assert_eq!(items.len(), 11 + OPTIONAL_EXTENSIONS.len());
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
    fn extension_items_carry_a_hint_for_p3_extensions() {
        let items = evaluate(&facts());
        assert_eq!(
            ext(&items, "hypopg").params.get("hint").map(String::as_str),
            Some("hypopg")
        );
        assert_eq!(ext(&items, "pgstattuple").params.get("hint"), None);
    }

    fn enable_of(items: &[ReadinessItem], name: &str) -> Option<String> {
        ext(items, name).params.get("enable").cloned()
    }

    #[test]
    fn enable_steps_follow_the_server_state() {
        let mut f = facts();
        f.extensions = vec![
            ExtensionFact {
                name: "hypopg".into(),
                installed: Some("1.4.1".into()),
                available: true,
            },
            ExtensionFact {
                name: "pg_stat_kcache".into(),
                installed: None,
                available: true,
            },
            ExtensionFact {
                name: "pg_wait_sampling".into(),
                installed: None,
                available: true,
            },
        ];
        // Библиотеки загружены на всех узлах (pg-perf, база без расширений): только CREATE EXTENSION.
        f.preload_libraries_by_member = vec![
            (
                "a".into(),
                vec![
                    "pg_stat_statements".into(),
                    "pg_stat_kcache".into(),
                    "pg_wait_sampling".into(),
                ],
            ),
            (
                "b".into(),
                vec![
                    "pg_stat_statements".into(),
                    "pg_stat_kcache".into(),
                    "pg_wait_sampling".into(),
                ],
            ),
        ];
        let items = evaluate(&f);
        assert_eq!(
            enable_of(&items, "pg_stat_kcache").as_deref(),
            Some("create")
        );
        assert_eq!(
            enable_of(&items, "pg_wait_sampling").as_deref(),
            Some("create")
        );
        assert_eq!(
            ext(&items, "pg_stat_kcache").fix.as_deref(),
            Some("CREATE EXTENSION IF NOT EXISTS pg_stat_kcache;")
        );
        // Установлено — шагов нет; hypopg не на сервере — пакет PGDG под мажорную версию, без preload.
        assert_eq!(enable_of(&items, "hypopg"), None);
        f.extensions.retain(|e| e.name != "hypopg");
        let items = evaluate(&f);
        let h = ext(&items, "hypopg");
        assert_eq!(h.params.get("enable").map(String::as_str), Some("package"));
        assert_eq!(h.params["package"], "postgresql-17-hypopg");
        assert_eq!(h.params["rpm"], "hypopg_17");
        // На одном из узлов библиотеки нет: preload и перезапуск, затем CREATE EXTENSION.
        f.preload_libraries_by_member[1].1 = vec!["pg_stat_statements".into()];
        let items = evaluate(&f);
        assert_eq!(
            enable_of(&items, "pg_stat_kcache").as_deref(),
            Some("preload")
        );
        // Список не виден роли — не утверждаем ни того, ни другого.
        f.preload_libraries_by_member.clear();
        let items = evaluate(&f);
        assert_eq!(
            enable_of(&items, "pg_wait_sampling").as_deref(),
            Some("preloadUnknown")
        );
        // Не установлено на сервере (pg-single): пакет, preload и перезапуск, CREATE EXTENSION.
        f.extensions.clear();
        f.preload_libraries_by_member = vec![("a".into(), vec!["pg_stat_statements".into()])];
        let items = evaluate(&f);
        let k = ext(&items, "pg_stat_kcache");
        assert_eq!(
            (k.status, k.params.get("enable").map(String::as_str)),
            (Status::Missing, Some("packagePreload"))
        );
        assert_eq!(k.params["package"], "postgresql-17-pg-stat-kcache");
        assert_eq!(k.params["rpm"], "pg_stat_kcache_17");
        assert_eq!(k.fix, None);
        // Расширения без подсказки (pgstattuple и др.) шагов включения не получают.
        assert_eq!(enable_of(&items, "pgstattuple"), None);
    }

    /// M6 ревью: расширение создано в базе, но библиотеки нет в preload (на узле, где список виден) — предупреждение с
    /// шагами preload и перезапуска, без CREATE EXTENSION; список не виден — не утверждаем.
    #[test]
    fn a_created_extension_without_its_library_is_a_warning() {
        let mut f = facts();
        f.extensions = vec![
            ExtensionFact {
                name: "pg_stat_kcache".into(),
                installed: Some("2.3.0".into()),
                available: true,
            },
            ExtensionFact {
                name: "pg_wait_sampling".into(),
                installed: Some("1.1".into()),
                available: true,
            },
        ];
        f.preload_libraries_by_member = vec![
            (
                "a".into(),
                vec!["pg_stat_statements".into(), "pg_stat_kcache".into()],
            ),
            ("b".into(), vec!["pg_stat_statements".into()]),
        ];
        let items = evaluate(&f);
        let k = ext(&items, "pg_stat_kcache");
        assert_eq!(k.status, Status::Warn);
        assert_eq!(k.params["reason"], "notLoaded");
        assert_eq!(k.params["enable"], "loadCreated");
        assert_eq!(k.params["members"], "b");
        assert_eq!(k.params["version"], "2.3.0");
        assert_eq!(k.fix, None);
        assert_eq!(ext(&items, "pg_wait_sampling").params["members"], "a, b");
        f.preload_libraries_by_member.clear();
        let items = evaluate(&f);
        assert_eq!(ext(&items, "pg_stat_kcache").status, Status::Ok);
        assert!(!ext(&items, "pg_stat_kcache").params.contains_key("reason"));
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
    fn role_name_is_always_quoted() {
        let mut f = facts();
        f.member_of.clear();
        for (user, quoted) in [
            ("glafira_diag", "\"glafira_diag\""),
            ("user", "\"user\""),
            ("select", "\"select\""),
            ("current_user", "\"current_user\""),
            ("Mixed", "\"Mixed\""),
            ("we\"ird", "\"we\"\"ird\""),
        ] {
            f.user = user.into();
            let items = evaluate(&f);
            assert_eq!(
                get(&items, "monitorRole").fix,
                Some(format!("GRANT pg_monitor TO {quoted};"))
            );
            assert_eq!(
                get(&items, "maintainRole").fix,
                Some(format!("GRANT pg_maintain TO {quoted};"))
            );
        }
    }

    #[test]
    fn superuser_has_everything() {
        let mut f = facts();
        f.member_of.clear();
        f.is_superuser = true;
        let items = evaluate(&f);
        for id in [
            "monitorRole",
            "signalRole",
            "replicationAttr",
            "maintainRole",
        ] {
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
    fn track_planning_item_follows_the_setting_and_needs_pgss() {
        let items = evaluate(&facts());
        assert_eq!(get(&items, "pgssTrackPlanning").status, Status::Ok);
        let mut f = facts();
        f.pgss_track_planning = Some("off".into());
        let i = get(&evaluate(&f), "pgssTrackPlanning").clone();
        assert_eq!(
            (i.status, i.fix.as_deref()),
            (Status::Warn, Some("pg_stat_statements.track_planning = on"))
        );
        f.pgss_installed = false;
        f.pgss_track_planning = None;
        assert!(
            evaluate(&f).iter().all(|i| i.id != "pgssTrackPlanning"),
            "без pgss пункт не показывается"
        );
    }

    #[test]
    fn read_all_data_item_depends_on_version_and_membership() {
        assert_eq!(get(&evaluate(&facts()), "readAllData").status, Status::Ok);
        let mut f = facts();
        f.read_all_data = false;
        let i = get(&evaluate(&f), "readAllData").clone();
        assert_eq!(
            (i.status, i.fix.as_deref()),
            (
                Status::Warn,
                Some("GRANT pg_read_all_data TO \"glafira_diag\";")
            )
        );
        f.server_version_num = 130_000;
        let i = get(&evaluate(&f), "readAllData").clone();
        assert_eq!(
            (i.status, i.params.get("reason").map(String::as_str), i.fix),
            (Status::Warn, Some("version"), None)
        );
        f.is_superuser = true;
        assert_eq!(get(&evaluate(&f), "readAllData").status, Status::Ok);
    }

    #[test]
    fn replication_attribute_is_a_warning_with_its_fix() {
        let mut f = ReadinessFacts {
            user: "glafira_diag".into(),
            server_version_num: 170000,
            ..Default::default()
        };
        let item = |f: &ReadinessFacts| {
            evaluate(f)
                .into_iter()
                .find(|i| i.id == "replicationAttr")
                .unwrap()
        };
        let i = item(&f);
        assert_eq!(i.status, Status::Warn);
        assert_eq!(
            i.fix.as_deref(),
            Some("ALTER ROLE \"glafira_diag\" REPLICATION;")
        );
        f.has_replication = true;
        assert_eq!(item(&f).status, Status::Ok);
        f.has_replication = false;
        f.is_superuser = true;
        assert_eq!(item(&f).status, Status::Ok);
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
