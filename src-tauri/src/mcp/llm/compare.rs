//! «Сравнить планы» (спец. P4 §8.4): EXPLAIN без выполнения исходного и предложенного запроса на одном узле с теми же
//! значениями `$N`; у предложенного — индексы через hypopg (`check_many`: виртуальные индексы в транзакции
//! `READ ONLY` с `hypopg_reset()`), без hypopg — без индексов с пометкой `without_hypopg`. Это действие пользователя из
//! панели: запросы идут соединением консоли EXPLAIN P3 (`ConnectSpec::for_explain`, `application_name` Glafira,
//! пробный план в `READ ONLY`), а не сессией SQL модели (`GuardTx`); ничего не выполняется.

use pg_advisor::plan::PlanTree;
use pg_advisor::plan_rules::evaluate;

use super::proposals::{Comparison, IndexUse, LlmProposal};
use crate::actions::Confirmation;
use crate::error::{AppResult, ErrorCode};
use crate::explain::hypopg::{self, HypoManyRequest};
use crate::explain::params::count_params;
use crate::explain::{DEFAULT_TIMEOUT_S, ExplainRequest, Mode, PROBE_TIMEOUT_S, Params};
use crate::state::AppState;
use crate::topology::model::now_ms;

/// Параметры EXPLAIN одного из запросов: без `$N` — без параметров; значения запроса консоли — если их столько же;
/// иначе обобщённый план (`GENERIC_PLAN`, PostgreSQL 16+; на старых версиях EXPLAIN ответит, что нужны значения, —
/// тогда пользователь сравнивает в консоли со своими значениями).
pub fn params_for(n: usize, values: &[Option<String>]) -> Params {
    if n == 0 {
        Params::None
    } else if values.len() == n {
        Params::Values {
            values: values.to_vec(),
        }
    } else {
        Params::Generic
    }
}

/// План без выполнения (`Mode::Plan`: пробный план консоли P3 в `READ ONLY`).
async fn plan_of(
    st: &AppState,
    connection_id: &str,
    member: &str,
    database: &str,
    sql: &str,
    params: Params,
) -> AppResult<PlanTree> {
    let req = ExplainRequest {
        member: member.into(),
        database: database.into(),
        sql: sql.into(),
        mode: Mode::Plan,
        params,
        timeout_s: DEFAULT_TIMEOUT_S,
        search_path: None,
        confirmation: Confirmation::default(),
        force_write: false,
        write_confirmed: false,
    };
    Ok(crate::explain::run::run(st, connection_id, &req)
        .await?
        .plan)
}

/// План предложенного запроса и использование его индексов: без индексов — простой план; с индексами — hypopg
/// (`check_many`), без hypopg в базе — план без индексов, все индексы «не использованы», `without_hypopg`.
async fn plan_after(
    st: &AppState,
    p: &LlmProposal,
    member: &str,
    params: Params,
) -> AppResult<(PlanTree, Vec<IndexUse>, bool)> {
    let plain = || {
        plan_of(
            st,
            &p.connection_id,
            member,
            &p.database,
            &p.rewritten_sql,
            params.clone(),
        )
    };
    if p.indexes.is_empty() {
        return Ok((plain().await?, Vec::new(), false));
    }
    let client =
        crate::actions::conn::open_explain(st, &p.connection_id, member, Some(&p.database)).await?;
    if hypopg::schema_of(&client.client).await?.is_none() {
        drop(client);
        let unused = p
            .indexes
            .iter()
            .map(|d| IndexUse {
                ddl: d.clone(),
                used: false,
            })
            .collect();
        return Ok((plain().await?, unused, true));
    }
    let r = hypopg::check_many(
        &client.client,
        &HypoManyRequest {
            member: member.to_string(),
            database: p.database.clone(),
            sql: p.rewritten_sql.clone(),
            params: params.clone(),
            search_path: None,
            index_ddls: p.indexes.clone(),
            timeout_s: PROBE_TIMEOUT_S,
        },
    )
    .await?;
    let used = r
        .indexes
        .into_iter()
        .map(|i| IndexUse {
            ddl: i.ddl,
            used: i.used,
        })
        .collect();
    Ok((r.plan, used, false))
}

/// Сравнение планов предложения: узел — тот, на котором модель проверяла предложение (`member` предложения), без него —
/// «за лидером»; значения `$N` — из запроса на оптимизацию, если он ещё хранится.
pub async fn compare(st: &AppState, p: &LlmProposal) -> AppResult<Comparison> {
    let member = match &p.member {
        Some(m) => m.clone(),
        None => st
            .runtimes
            .diag_target(&p.connection_id, None, false)?
            .member
            .map(|m| m.name)
            .ok_or_else(|| {
                app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.topology.noNode",
                    "No suitable node is available"
                )
            })?,
    };
    let values = p
        .request_id
        .as_deref()
        .and_then(|id| st.llm.requests.get(id))
        .map(|r| r.params)
        .unwrap_or_default();
    let before = plan_of(
        st,
        &p.connection_id,
        &member,
        &p.database,
        &p.original_sql,
        params_for(count_params(&p.original_sql), &values),
    )
    .await?;
    let (after, indexes_used, without_hypopg) = plan_after(
        st,
        p,
        &member,
        params_for(count_params(&p.rewritten_sql), &values),
    )
    .await?;
    Ok(Comparison {
        cost_before: before.root.total_cost,
        cost_after: after.root.total_cost,
        indexes_used,
        member,
        at_ms: now_ms(),
        recommendations_before: evaluate(&before).recommendations,
        recommendations_after: evaluate(&after).recommendations,
        plan_before: before,
        plan_after: after,
        without_hypopg,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::secrets::MemoryStore;

    #[test]
    fn both_queries_get_the_same_parameter_values() {
        let values = vec![Some("new".to_string()), None];
        assert_eq!(params_for(0, &values), Params::None);
        assert_eq!(
            params_for(2, &values),
            Params::Values {
                values: values.clone()
            }
        );
        // Значений нет или их не столько — обобщённый план (PostgreSQL 16+).
        assert_eq!(params_for(1, &values), Params::Generic);
        assert_eq!(params_for(1, &[]), Params::Generic);
    }

    #[tokio::test]
    async fn an_unknown_proposal_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let st = AppState::new(dir.path(), Arc::new(MemoryStore::default())).unwrap();
        let e = crate::mcp::llm::compare_proposal(&st, "p000000")
            .await
            .unwrap_err();
        assert_eq!(e.key, "errors.llm.proposalNotFound");
    }
}
