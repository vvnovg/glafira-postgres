//! Patroni: готовность к failover и матрица health (спец. P2 §5.8–5.9). Только `GET`.

use futures_util::future::join_all;
use pg_advisor::failover::{
    self, CandidateFacts, CandidateRole, DEFAULT_MAXIMUM_LAG_ON_FAILOVER, DynConfig, SyncMode,
};
use pg_advisor::rules::patroni::{
    HEALTH_ENDPOINTS, HealthFacts, HealthGrade, evaluate_health, grade_member,
};
use serde::Serialize;
use serde_json::Value;

use crate::diag::{DiagData, DiagReport, Tool, make_report};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::state::AppState;
use crate::topology::model::{Role, Topology};
use crate::topology::patroni::{PatroniHttp, cluster_bases, fetch_config};
use crate::topology::runtime::DiagTarget;

/// Динамическая конфигурация из `GET /config`: читается мягко — чего нет или не того типа, берётся по умолчанию Patroni.
pub fn parse_dyn_config(v: &Value) -> DynConfig {
    let int = |k: &str| v.get(k).and_then(Value::as_i64);
    let synchronous_mode = match v.get("synchronous_mode") {
        Some(Value::Bool(true)) => SyncMode::On,
        Some(Value::String(s)) if s.eq_ignore_ascii_case("quorum") => SyncMode::Quorum,
        Some(Value::String(s)) if ["on", "true"].contains(&s.to_ascii_lowercase().as_str()) => {
            SyncMode::On
        }
        _ => SyncMode::Off,
    };
    DynConfig {
        maximum_lag_on_failover: int("maximum_lag_on_failover")
            .unwrap_or(DEFAULT_MAXIMUM_LAG_ON_FAILOVER),
        synchronous_mode,
        synchronous_mode_strict: v
            .get("synchronous_mode_strict")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        synchronous_node_count: int("synchronous_node_count"),
        ttl: int("ttl"),
        loop_wait: int("loop_wait"),
        retry_timeout: int("retry_timeout"),
        permanent_slots: v
            .get("slots")
            .and_then(Value::as_object)
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default(),
    }
}

pub fn candidate_role(r: Role) -> CandidateRole {
    match r {
        Role::Primary | Role::StandbyLeader => CandidateRole::Leader,
        Role::SyncStandby => CandidateRole::Sync,
        Role::QuorumStandby => CandidateRole::Quorum,
        Role::Replica => CandidateRole::Replica,
        Role::Unknown => CandidateRole::Unknown,
    }
}

pub fn candidate_facts(t: &Topology) -> Vec<CandidateFacts> {
    t.members
        .iter()
        .map(|m| CandidateFacts {
            name: m.name.clone(),
            role: candidate_role(m.role),
            state: m.state.clone(),
            reachable: m.reachable,
            lag_bytes: m.lag_bytes,
            nofailover: m
                .tags
                .get("nofailover")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            failover_priority: m.tags.get("failover_priority").and_then(Value::as_i64),
            pending_restart: m.pending_restart,
        })
        .collect()
}

/// Динамическая конфигурация кластера подключения: клиент HTTP подключения (`LiveSource::http_for`), адреса — как у
/// `/cluster`.
pub async fn dyn_config(st: &AppState, target: &DiagTarget) -> AppResult<DynConfig> {
    let settings = target.profile.patroni.as_ref().ok_or_else(|| {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.noUrls",
            "No Patroni API addresses"
        )
    })?;
    let http = st.live.http_for(&target.profile).await?;
    let v = fetch_config(
        http.as_ref(),
        &cluster_bases(settings, Some(&target.topology)),
    )
    .await?;
    Ok(parse_dyn_config(&v))
}

/// Клетка матрицы: код ответа адреса проверки здоровья или ошибка запроса (API не ответил).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCell {
    pub endpoint: String,
    pub status: Option<u16>,
    pub error: Option<AppError>,
    /// Оценка по роли участника (`grade_member`): от неё цвет клетки.
    pub grade: HealthGrade,
}

/// Строка матрицы: участник и его клетки по `HealthData.endpoints`. Без адреса API клеток нет.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthMember {
    pub member: String,
    pub api_url: Option<String>,
    pub cells: Vec<HealthCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthData {
    pub endpoints: Vec<String>,
    pub members: Vec<HealthMember>,
}

pub async fn failover_report(st: &AppState, target: &DiagTarget) -> AppResult<DiagReport> {
    let config = dyn_config(st, target).await?;
    let (data, findings) = failover::evaluate(
        &candidate_facts(&target.topology),
        &config,
        target.topology.paused.unwrap_or(false),
    );
    Ok(make_report(
        Tool::Failover,
        target,
        None,
        DiagData::Failover(data),
        findings,
        vec![],
    ))
}

pub async fn health_report(st: &AppState, target: &DiagTarget) -> AppResult<DiagReport> {
    let http = st.live.http_for(&target.profile).await?;
    // Все участники и все адреса параллельно; у каждого запроса свой срок клиента (5 с).
    let mut members = join_all(target.topology.members.iter().map(|m| {
        let http = http.clone();
        async move {
            let Some(base) = m.api_url.clone() else {
                return HealthMember {
                    member: m.name.clone(),
                    api_url: None,
                    cells: vec![],
                };
            };
            let cells = join_all(HEALTH_ENDPOINTS.iter().map(|e| {
                let (http, url) = (http.clone(), format!("{base}/{e}"));
                async move {
                    match http.get_status(&url).await {
                        // Оценку ставит `grade_member` ниже, когда известны роль и все коды участника.
                        Ok(code) => HealthCell {
                            endpoint: e.to_string(),
                            status: Some(code),
                            error: None,
                            grade: HealthGrade::Bad,
                        },
                        Err(err) => HealthCell {
                            endpoint: e.to_string(),
                            status: None,
                            error: Some(err),
                            grade: HealthGrade::Bad,
                        },
                    }
                }
            }))
            .await;
            HealthMember {
                member: m.name.clone(),
                api_url: Some(base),
                cells,
            }
        }
    }))
    .await;
    let facts: Vec<HealthFacts> = members
        .iter()
        .zip(&target.topology.members)
        .map(|(h, m)| HealthFacts {
            member: h.member.clone(),
            role: candidate_role(m.role),
            codes: h
                .cells
                .iter()
                .map(|c| (c.endpoint.clone(), c.status))
                .collect(),
        })
        .collect();
    let findings = evaluate_health(&facts);
    for (m, f) in members.iter_mut().zip(&facts) {
        let grades = grade_member(f);
        for c in &mut m.cells {
            c.grade = grades.get(&c.endpoint).copied().unwrap_or(HealthGrade::Bad);
        }
    }
    let data = HealthData {
        endpoints: HEALTH_ENDPOINTS.iter().map(|e| e.to_string()).collect(),
        members,
    };
    Ok(make_report(
        Tool::Health,
        target,
        None,
        DiagData::Health(data),
        findings,
        vec![],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_configuration_is_read_leniently() {
        let c = parse_dyn_config(&serde_json::json!({
            "ttl": 30, "loop_wait": 10, "retry_timeout": 10, "synchronous_mode": "quorum", "synchronous_node_count": 1,
            "slots": {"debezium": {"type": "logical", "database": "orders", "plugin": "pgoutput"}}
        }));
        assert_eq!(
            (c.synchronous_mode, c.maximum_lag_on_failover, c.ttl),
            (SyncMode::Quorum, DEFAULT_MAXIMUM_LAG_ON_FAILOVER, Some(30))
        );
        assert_eq!(c.permanent_slots, ["debezium"]);
        assert_eq!(
            parse_dyn_config(
                &serde_json::json!({"synchronous_mode": true, "maximum_lag_on_failover": 5})
            )
            .synchronous_mode,
            SyncMode::On
        );
        assert_eq!(
            parse_dyn_config(&serde_json::json!({"synchronous_mode": "off"})).synchronous_mode,
            SyncMode::Off
        );
        assert_eq!(
            parse_dyn_config(&serde_json::json!({})).maximum_lag_on_failover,
            DEFAULT_MAXIMUM_LAG_ON_FAILOVER
        );
    }

    #[test]
    fn candidate_facts_from_topology_tags() {
        use crate::topology::model::{Role, member, topo};
        let mut t = topo(vec![
            member("a", Role::Primary),
            member("b", Role::SyncStandby),
        ]);
        t.members[1]
            .tags
            .insert("nofailover".into(), serde_json::json!(true));
        t.members[1]
            .tags
            .insert("failover_priority".into(), serde_json::json!(2));
        let f = candidate_facts(&t);
        assert_eq!(
            (f[0].role, f[1].role),
            (CandidateRole::Leader, CandidateRole::Sync)
        );
        assert_eq!((f[1].nofailover, f[1].failover_priority), (true, Some(2)));
    }
}
