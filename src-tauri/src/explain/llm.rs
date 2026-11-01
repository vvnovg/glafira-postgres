//! Данные для «Скопировать для LLM» (спец. P3 §7.4): среда, текст, статистика, текст плана и DDL таблиц.
//! Markdown с заголовками на языке интерфейса собирает фронтенд (`src/lib/llmMarkdown.ts`).

use pg_advisor::plan::text_out::to_text;
use pg_advisor::plan::{PlanTree, RelationRef, relations};
use pg_advisor::queries::QueryStat;
use serde::{Deserialize, Serialize};

use crate::actions::Confirmation;
use crate::catalog::table;
use crate::commands::catalog::{catalog_limit, on_catalog_node_within};
use crate::diag::caps::V16;
use crate::diag::query_detail;
use crate::error::AppResult;
use crate::explain::params::count_params;
use crate::explain::{ExplainRequest, ExplainResult, Mode, Params, run};
use crate::state::AppState;

/// Таблиц плана в данных не больше: остальные считаются в `tables_omitted`.
pub const MAX_TABLES: usize = 10;
/// Бюджет DDL (таблицы вместе с их индексами, они внутри DDL), байты.
pub const MAX_DDL_BYTES: usize = 65_536;

/// Откуда запрос: строка pgss по `queryId` или SQL консоли.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LlmSource {
    #[serde(rename_all = "camelCase")]
    Query {
        #[serde(with = "pg_advisor::serde_util::i64_string")]
        query_id: i64,
    },
    Sql {
        sql: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTable {
    pub schema: String,
    pub name: String,
    /// DDL каталога (`catalog::ddl::table_ddl`): уже с `CREATE INDEX` индексов, не принадлежащих ограничениям; индексы
    /// ограничений (PRIMARY KEY, UNIQUE, EXCLUDE) описаны самими ограничениями — отдельного списка индексов нет.
    pub ddl: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmStats {
    pub since_reset: Option<QueryStat>,
    pub window_3600: Option<QueryStat>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmBundle {
    pub server_version_num: i32,
    pub pgss_version: Option<String>,
    pub in_recovery: bool,
    pub member: String,
    pub database: String,
    pub text: Option<String>,
    pub text_truncated: bool,
    pub stats: Option<LlmStats>,
    pub plan_text: Option<String>,
    pub tables: Vec<LlmTable>,
    pub tables_omitted: usize,
    /// Ключи интерфейса: `paramsNeeded` (PG < 16, текст с `$N` без значений — плана нет), `queryGone`, `planFailed`
    /// (пробный план не получен — по любой причине, в том числе из-за прав), `ddlFailed` (DDL части таблиц прочитать не удалось).
    pub warnings: Vec<String>,
}

/// Первые `MAX_TABLES` отношений плана и сколько осталось за бортом.
pub fn pick_tables(rels: &[RelationRef]) -> (Vec<RelationRef>, usize) {
    let picked: Vec<RelationRef> = rels.iter().take(MAX_TABLES).cloned().collect();
    (picked, rels.len().saturating_sub(MAX_TABLES))
}

/// Укладывает DDL в бюджет: таблицы в порядке плана, первая не влезшая и все за ней отбрасываются.
pub fn fit_ddl(tables: Vec<LlmTable>) -> (Vec<LlmTable>, usize) {
    let mut used = 0usize;
    let mut kept = Vec::new();
    let total = tables.len();
    for t in tables {
        let size = t.ddl.len();
        if used + size > MAX_DDL_BYTES {
            break;
        }
        used += size;
        kept.push(t);
    }
    let omitted = total - kept.len();
    (kept, omitted)
}

/// Параметры пробного плана по числу `$N` в тексте: без `$N` — `Params::None`, на 16+ — `GENERIC_PLAN`, раньше значения
/// взять неоткуда (`None` — предупреждение `paramsNeeded`, плана нет).
fn probe_params(n_params: usize, server_version_num: i32) -> Option<Params> {
    if n_params == 0 {
        Some(Params::None)
    } else if server_version_num >= V16 {
        Some(Params::Generic)
    } else {
        None
    }
}

/// Текст для пробного плана: усечённый текст не пробуется (план по обрубку — ошибка или чужой план), о причине
/// отсутствия плана пользователю говорит `text_truncated` в данных.
fn probe_text(text: Option<&str>, truncated: bool) -> Option<&str> {
    text.filter(|_| !truncated)
}

/// Итог пробного плана: «Скопировать для LLM» — подспорье, текст и статистика нужны и без плана, поэтому любая
/// ошибка (права на таблицы, ярус записи, отвергнутый текст, срок, узел) — предупреждение `planFailed`, а не отказ;
/// причина остаётся в журнале.
fn plan_or_warning(
    probe: AppResult<ExplainResult>,
    id: &str,
    member: &str,
    warnings: &mut Vec<String>,
) -> Option<PlanTree> {
    match probe {
        Ok(r) => Some(r.plan),
        Err(e) => {
            tracing::warn!(connection = %id, member = %member, "LLM bundle: probe plan failed: {}", e.message);
            warnings.push("planFailed".to_string());
            None
        }
    }
}

/// Итог чтения DDL: `read` — прочитанные таблицы и число не прочитанных; отказ всего чтения (соединение, срок) —
/// все `picked` не прочитаны (причина — в журнале). Не прочитанные таблицы не пропадают молча: одно предупреждение
/// `ddlFailed`, а их число вызывающий прибавляет к `tables_omitted`.
fn ddl_or_warning(
    read: AppResult<(Vec<LlmTable>, usize)>,
    picked: usize,
    id: &str,
    member: &str,
    warnings: &mut Vec<String>,
) -> (Vec<LlmTable>, usize) {
    let (fetched, failed) = read.unwrap_or_else(|e| {
        tracing::warn!(connection = %id, member = %member, "LLM bundle: DDL not read: {}", e.message);
        (Vec::new(), picked)
    });
    if failed > 0 {
        warnings.push("ddlFailed".to_string());
    }
    (fetched, failed)
}

pub async fn bundle(
    st: &AppState,
    id: &str,
    member: &str,
    database: &str,
    source: LlmSource,
    plan: Option<PlanTree>,
) -> AppResult<LlmBundle> {
    let mut warnings = Vec::new();
    // Узел и профиль берутся до плана: пропажа подключения между шагами не должна отнимать у пользователя текст,
    // статистику и план, уже собранные.
    let target = st.runtimes.diag_target(id, Some(member), false)?;
    let (text, text_truncated, stats, version, in_recovery, pgss_version) = match &source {
        LlmSource::Query { query_id } => {
            let d = query_detail::detail(st, id, member, database, *query_id).await?;
            if d.text.is_none() {
                warnings.push("queryGone".to_string());
            }
            let window_3600 = d
                .windows
                .iter()
                .find(|w| w.window_s == 3600)
                .and_then(|w| w.stat.clone());
            (
                d.text.clone(),
                d.text_truncated,
                Some(LlmStats {
                    since_reset: d.since_reset.clone(),
                    window_3600,
                }),
                d.server_version_num,
                d.in_recovery,
                d.pgss_version.clone(),
            )
        }
        LlmSource::Sql { sql } => {
            let h =
                crate::diag::on_member(st, id, &target, |c| Box::pin(crate::diag::head(c))).await?;
            (
                Some(sql.clone()),
                false,
                None,
                h.version_num,
                h.in_recovery,
                None,
            )
        }
    };
    // План: переданный, иначе пробный EXPLAIN без выполнения (GENERIC_PLAN на 16+ при `$N`); усечённый текст не пробуется.
    let plan = match plan {
        Some(p) => Some(p),
        None => match probe_text(text.as_deref(), text_truncated) {
            Some(sql) => match probe_params(count_params(sql), version) {
                Some(params) => {
                    let req = ExplainRequest {
                        member: member.to_string(),
                        database: database.to_string(),
                        sql: sql.to_string(),
                        mode: Mode::Plan,
                        params,
                        timeout_s: crate::explain::DEFAULT_TIMEOUT_S,
                        search_path: None,
                        confirmation: Confirmation::default(),
                        force_write: false,
                        write_confirmed: false,
                    };
                    plan_or_warning(run::run(st, id, &req).await, id, member, &mut warnings)
                }
                None => {
                    warnings.push("paramsNeeded".to_string());
                    None
                }
            },
            None => None,
        },
    };
    let (tables, tables_omitted) = match &plan {
        Some(p) => {
            let (picked, omitted_by_count) = pick_tables(&relations(p));
            if picked.is_empty() {
                (Vec::new(), omitted_by_count)
            } else {
                let picked_len = picked.len();
                // Владеемые копии: будущее запроса не может держать заимствования на время жизни соединения.
                let (conn, node) = (id.to_string(), member.to_string());
                let read = on_catalog_node_within(
                    catalog_limit(&target.profile),
                    st.client(id, Some(member), Some(database)),
                    |c| {
                        Box::pin(async move {
                            let mut out = Vec::new();
                            let mut failed = 0usize;
                            for r in &picked {
                                let schema =
                                    r.schema.clone().unwrap_or_else(|| "public".to_string());
                                // Отношение не прочиталось (не таблица, исчезло, обрыв, срок) — таблица пропускается.
                                match table::get(c, &schema, &r.name).await {
                                    Ok(t) => out.push(LlmTable {
                                        schema,
                                        name: r.name.clone(),
                                        ddl: t.ddl,
                                    }),
                                    Err(e) => {
                                        tracing::warn!(
                                            connection = %conn, member = %node,
                                            "LLM bundle: DDL of {}.{} not read: {}", schema, r.name, e.message
                                        );
                                        failed += 1;
                                    }
                                }
                            }
                            Ok((out, failed))
                        })
                    },
                )
                .await;
                let (fetched, failed) = ddl_or_warning(read, picked_len, id, member, &mut warnings);
                let (kept, omitted_by_size) = fit_ddl(fetched);
                (kept, omitted_by_count + failed + omitted_by_size)
            }
        }
        None => (Vec::new(), 0),
    };
    Ok(LlmBundle {
        server_version_num: version,
        pgss_version,
        in_recovery,
        member: member.to_string(),
        database: database.to_string(),
        text,
        text_truncated,
        stats,
        plan_text: plan.as_ref().map(to_text),
        tables,
        tables_omitted,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AppError, ErrorCode};

    fn rel(n: &str) -> RelationRef {
        RelationRef {
            schema: Some("shop".into()),
            name: n.into(),
        }
    }

    #[test]
    fn at_most_ten_tables_and_the_rest_is_counted() {
        let rels: Vec<RelationRef> = (0..12).map(|i| rel(&format!("t{i}"))).collect();
        let (picked, omitted) = pick_tables(&rels);
        assert_eq!((picked.len(), omitted), (MAX_TABLES, 2));
        assert_eq!(picked[0].name, "t0");
    }

    #[test]
    fn ddl_budget_drops_whole_tables_from_the_end() {
        let big = "x".repeat(MAX_DDL_BYTES / 2 + 1);
        let tables = vec![
            LlmTable {
                schema: "shop".into(),
                name: "a".into(),
                ddl: big.clone(),
            },
            LlmTable {
                schema: "shop".into(),
                name: "b".into(),
                ddl: big.clone(),
            },
            LlmTable {
                schema: "shop".into(),
                name: "c".into(),
                ddl: "small".into(),
            },
        ];
        let (kept, omitted) = fit_ddl(tables);
        assert_eq!(
            (kept.len(), omitted),
            (1, 2),
            "вторая таблица не влезла, третья тоже не берётся (порядок плана важен)"
        );
        assert_eq!(kept[0].name, "a");
    }

    /// Таблица с DDL из `ddl_len` байт.
    fn sized(name: &str, ddl_len: usize) -> LlmTable {
        LlmTable {
            schema: "shop".into(),
            name: name.into(),
            ddl: "x".repeat(ddl_len),
        }
    }

    fn names(tables: &[LlmTable]) -> Vec<&str> {
        tables.iter().map(|t| t.name.as_str()).collect()
    }

    #[test]
    fn no_relations_pick_nothing() {
        assert_eq!(pick_tables(&[]), (vec![], 0));
    }

    #[test]
    fn the_table_limit_is_inclusive() {
        let rels: Vec<RelationRef> = (0..MAX_TABLES + 1).map(|i| rel(&format!("t{i}"))).collect();
        let (picked, omitted) = pick_tables(&rels[..MAX_TABLES]);
        assert_eq!((picked.len(), omitted), (MAX_TABLES, 0), "ровно десять");
        let (picked, omitted) = pick_tables(&rels);
        assert_eq!(
            (picked.len(), omitted),
            (MAX_TABLES, 1),
            "одиннадцатая за бортом"
        );
        assert_eq!(picked.last().map(|r| r.name.as_str()), Some("t9"));
    }

    #[test]
    fn the_budget_counts_ddl_bytes_only() {
        // Вторая таблица не влезает: она и всё за ней отбрасываются.
        let tables = vec![
            sized("a", 117),
            sized("b", MAX_DDL_BYTES - 100),
            sized("c", 5),
        ];
        let (kept, omitted) = fit_ddl(tables);
        assert_eq!(
            (names(&kept), omitted),
            (vec!["a"], 2),
            "отбрасывание с первой не влезшей: маленькая «c» после «b» не берётся"
        );
        // Индексы — внутри DDL, лишних байт на них нет: DDL ровно на весь бюджет помещается.
        let (kept, omitted) = fit_ddl(vec![sized("a", MAX_DDL_BYTES)]);
        assert_eq!((kept.len(), omitted), (1, 0));
    }

    #[test]
    fn the_first_table_over_the_budget_leaves_nothing() {
        let (kept, omitted) = fit_ddl(vec![sized("a", MAX_DDL_BYTES + 1)]);
        assert_eq!((kept, omitted), (vec![], 1));
        // Пустой список — пустой результат.
        assert_eq!(fit_ddl(vec![]), (vec![], 0));
    }

    #[test]
    fn a_total_of_exactly_the_budget_is_kept() {
        let half = MAX_DDL_BYTES / 2;
        // 32 768 и 32 768: вместе ровно бюджет.
        let (kept, omitted) = fit_ddl(vec![sized("a", half), sized("b", half)]);
        assert_eq!((names(&kept), omitted), (vec!["a", "b"], 0));
        // Лишний байт — вторая таблица уже не влезает.
        let (kept, omitted) = fit_ddl(vec![sized("a", half), sized("b", half + 1)]);
        assert_eq!((names(&kept), omitted), (vec!["a"], 1));
    }

    #[test]
    fn probe_params_follow_the_placeholder_count_and_the_server_version() {
        // Без `$N` параметры не нужны на любой версии.
        assert_eq!(probe_params(0, 150_000), Some(Params::None));
        assert_eq!(probe_params(0, 170_000), Some(Params::None));
        // С `$N`: GENERIC_PLAN только с 16, раньше значений взять неоткуда.
        assert_eq!(probe_params(2, 159_999), None);
        assert_eq!(probe_params(2, 160_000), Some(Params::Generic));
        assert_eq!(probe_params(1, 170_000), Some(Params::Generic));
    }

    #[test]
    fn a_truncated_text_is_not_probed() {
        assert_eq!(probe_text(Some("SELECT 1"), false), Some("SELECT 1"));
        assert_eq!(probe_text(Some("SELECT 1"), true), None);
        assert_eq!(probe_text(None, false), None);
    }

    fn app_error(code: ErrorCode) -> AppError {
        AppError {
            code,
            key: "errors.test".into(),
            params: Default::default(),
            message: "test".into(),
            details: None,
            retryable: false,
        }
    }

    #[test]
    fn any_probe_error_is_a_warning_and_not_a_failure() {
        // Права на таблицы, ярус записи, отвергнутый текст, срок, потеря узла: данные без плана остаются доступны.
        for code in [
            ErrorCode::NotAuthorized,
            ErrorCode::ExplainWrites,
            ErrorCode::InvalidInput,
            ErrorCode::Timeout,
            ErrorCode::ConnectionFailed,
        ] {
            let mut warnings = Vec::new();
            assert_eq!(
                plan_or_warning(Err(app_error(code)), "c", "pg-1", &mut warnings),
                None,
                "{code:?}"
            );
            assert_eq!(warnings, ["planFailed"], "{code:?}");
        }
    }

    #[test]
    fn unread_tables_give_one_ddl_warning_and_are_counted() {
        let table = |n: &str| LlmTable {
            schema: "shop".into(),
            name: n.into(),
            ddl: "CREATE TABLE t ();".into(),
        };
        // Всё прочиталось: предупреждения нет.
        let mut warnings = Vec::new();
        let (got, failed) =
            ddl_or_warning(Ok((vec![table("a")], 0)), 1, "c", "pg-1", &mut warnings);
        assert_eq!((got.len(), failed, warnings.len()), (1, 0, 0));
        // Часть таблиц не прочиталась: остальные сохраняются, предупреждение одно на все.
        let (got, failed) =
            ddl_or_warning(Ok((vec![table("a")], 2)), 3, "c", "pg-1", &mut warnings);
        assert_eq!(
            (got.len(), failed, warnings.as_slice()),
            (1, 2, ["ddlFailed".to_string()].as_slice())
        );
        // Чтение не состоялось вовсе (соединение, срок): не прочитаны все выбранные.
        let mut warnings = Vec::new();
        let (got, failed) = ddl_or_warning(
            Err(app_error(ErrorCode::Timeout)),
            4,
            "c",
            "pg-1",
            &mut warnings,
        );
        assert_eq!(
            (got.len(), failed, warnings.as_slice()),
            (0, 4, ["ddlFailed".to_string()].as_slice())
        );
    }

    #[test]
    fn bundle_json_is_camel_case() {
        let b = LlmBundle {
            server_version_num: 170_000,
            pgss_version: Some("1.11".into()),
            in_recovery: false,
            member: "pg-1".into(),
            database: "perf".into(),
            text: Some("SELECT 1".into()),
            text_truncated: false,
            stats: Some(LlmStats {
                since_reset: None,
                window_3600: None,
            }),
            plan_text: None,
            tables: vec![LlmTable {
                schema: "shop".into(),
                name: "orders".into(),
                ddl: "CREATE TABLE shop.orders ();".into(),
            }],
            tables_omitted: 0,
            warnings: vec!["planFailed".into()],
        };
        let v = serde_json::to_value(&b).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "database",
                "inRecovery",
                "member",
                "pgssVersion",
                "planText",
                "serverVersionNum",
                "stats",
                "tables",
                "tablesOmitted",
                "text",
                "textTruncated",
                "warnings"
            ]
        );
        assert_eq!(
            v["stats"],
            serde_json::json!({"sinceReset": null, "window3600": null})
        );
        assert_eq!(
            v["tables"][0],
            serde_json::json!({"schema": "shop", "name": "orders", "ddl": "CREATE TABLE shop.orders ();"})
        );
        assert!(v["planText"].is_null());
    }

    #[test]
    fn source_json() {
        let s: LlmSource =
            serde_json::from_value(serde_json::json!({"kind": "query", "queryId": "-5"})).unwrap();
        assert_eq!(s, LlmSource::Query { query_id: -5 });
        let s: LlmSource =
            serde_json::from_value(serde_json::json!({"kind": "sql", "sql": "SELECT 1"})).unwrap();
        assert_eq!(
            s,
            LlmSource::Sql {
                sql: "SELECT 1".into()
            }
        );
    }
}
