//! Матрица health Patroni (спец. P2 §5.9): HTTP-коды адресов проверки здоровья каждого участника — то, что видит
//! балансировщик. Коды собирает приложение, здесь только правила.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::failover::CandidateRole;
use crate::findings::{Finding, Severity};

/// Адреса проверки здоровья Patroni — то, что видит балансировщик.
pub const HEALTH_ENDPOINTS: [&str; 7] = [
    "primary",
    "standby-leader",
    "replica",
    "read-only",
    "sync",
    "async",
    "health",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthFacts {
    pub member: String,
    pub role: CandidateRole,
    /// Адрес → HTTP-код; `None` — API не ответил.
    pub codes: BTreeMap<String, Option<u16>>,
}

pub fn evaluate_health(members: &[HealthFacts]) -> Vec<Finding> {
    let code = |m: &HealthFacts, e: &str| m.codes.get(e).copied().flatten();
    let mut out = Vec::new();
    // Участники, чей API ответил на `/primary`: по ним считается, сколько узлов балансировщик примет за лидера.
    // Лидер standby-кластера отвечает 200 на `/standby-leader`, а не на `/primary`: он тоже лидер.
    let answered: Vec<&HealthFacts> = members
        .iter()
        .filter(|m| code(m, "primary").is_some())
        .collect();
    let primaries = answered
        .iter()
        .filter(|m| code(m, "primary") == Some(200) || code(m, "standby-leader") == Some(200))
        .count();
    if !answered.is_empty() && primaries != 1 {
        out.push(Finding::new("healthPrimary", Severity::Critical).param("count", primaries));
    }
    for m in members {
        if code(m, "health").is_some_and(|c| c != 200) {
            out.push(Finding::new("healthUnhealthy", Severity::Critical).member(Some(&m.member)));
        }
        if matches!(
            m.role,
            CandidateRole::Replica | CandidateRole::Sync | CandidateRole::Quorum
        ) && code(m, "replica").is_some_and(|c| c != 200)
        {
            out.push(Finding::new("replicaOutOfPool", Severity::Warning).member(Some(&m.member)));
        }
    }
    crate::findings::sort(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failover::CandidateRole;
    use crate::findings::Severity;

    fn h(member: &str, role: CandidateRole, codes: &[(&str, Option<u16>)]) -> HealthFacts {
        HealthFacts {
            member: member.into(),
            role,
            codes: codes.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    #[test]
    fn health_matrix_findings() {
        let ok = [
            h(
                "a",
                CandidateRole::Leader,
                &[
                    ("primary", Some(200)),
                    ("replica", Some(503)),
                    ("health", Some(200)),
                ],
            ),
            h(
                "b",
                CandidateRole::Replica,
                &[
                    ("primary", Some(503)),
                    ("replica", Some(200)),
                    ("health", Some(200)),
                ],
            ),
        ];
        assert!(evaluate_health(&ok).is_empty());
        let bad = [
            h(
                "a",
                CandidateRole::Leader,
                &[("primary", Some(200)), ("health", Some(200))],
            ),
            h(
                "b",
                CandidateRole::Replica,
                &[
                    ("primary", Some(200)),
                    ("replica", Some(503)),
                    ("health", Some(503)),
                ],
            ),
            h(
                "c",
                CandidateRole::Replica,
                &[("primary", None), ("replica", None), ("health", None)],
            ),
        ];
        // Standby-кластер: лидер — `/standby-leader`, `/primary` у всех 503; это не «лидера нет».
        let standby = [
            h(
                "a",
                CandidateRole::Leader,
                &[
                    ("primary", Some(503)),
                    ("standby-leader", Some(200)),
                    ("health", Some(200)),
                ],
            ),
            h(
                "b",
                CandidateRole::Replica,
                &[
                    ("primary", Some(503)),
                    ("standby-leader", Some(503)),
                    ("replica", Some(200)),
                    ("health", Some(200)),
                ],
            ),
        ];
        assert!(evaluate_health(&standby).is_empty());
        // Лидера нет вовсе (все ответили, никто не 200) — тоже находка.
        let none = [h(
            "a",
            CandidateRole::Replica,
            &[
                ("primary", Some(503)),
                ("replica", Some(200)),
                ("health", Some(200)),
            ],
        )];
        let f = evaluate_health(&none);
        assert_eq!(
            f.iter()
                .map(|x| (x.id.as_str(), x.params["count"].clone()))
                .collect::<Vec<_>>(),
            [("healthPrimary", serde_json::json!(0))]
        );
        let f = evaluate_health(&bad);
        let got: Vec<(&str, Severity, Option<&str>)> = f
            .iter()
            .map(|x| (x.id.as_str(), x.severity, x.member.as_deref()))
            .collect();
        assert_eq!(
            got,
            [
                ("healthPrimary", Severity::Critical, None),
                ("healthUnhealthy", Severity::Critical, Some("b")),
                ("replicaOutOfPool", Severity::Warning, Some("b")),
            ]
        );
    }
}
