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

/// Оценка клетки матрицы — по роли участника, а не по одному коду: у здорового кластера 503 на адресе чужой роли
/// (`/primary` у реплики, `/replica` у лидера) — норма, а не сбой. Цвет клетки во фронтенде берётся отсюда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthGrade {
    /// 200 там, где адрес относится к роли участника.
    Ok,
    /// Ответ, ожидаемый на адресе чужой роли (503).
    NotApplicable,
    /// Участник в строю, но балансировщик не пошлёт на него трафик этого адреса (реплика с 503 на `/replica`,
    /// `/read-only`, `/async`), или `/sync` / `/async` разошлись с ролью из топологии.
    Degraded,
    /// Код противоречит роли (второй лидер, лидер без 200), `/health` не 200, иной код или API не ответил.
    Bad,
}

/// Что адрес значит для роли участника.
#[derive(Debug, Clone, Copy)]
enum Expect {
    /// Должен быть 200, иначе — сбой.
    Yes,
    /// Адрес лидера: у обычного кластера 200 даёт `/primary`, у standby-кластера — `/standby-leader`.
    LeaderPair,
    /// Адрес пула балансировщика: 200 — узел в пуле, 503 — выведен из пула.
    Pool,
    /// Адрес чужой роли: 503 — норма, 200 — расхождение.
    No,
    /// Оба ответа нормальны: роль неизвестна или Patroni отвечает по-разному в зависимости от режима.
    Either,
}

fn expect(role: CandidateRole, endpoint: &str) -> Expect {
    use CandidateRole::*;
    match (role, endpoint) {
        (_, "health") => Expect::Yes,
        (Unknown, _) => Expect::Either,
        (Leader, "primary" | "standby-leader") => Expect::LeaderPair,
        (Leader, "read-only") => Expect::Yes,
        // У лидера standby-кластера PostgreSQL в recovery: ответ `/replica` зависит от версии Patroni.
        (Leader, "replica") => Expect::Either,
        (Leader, _) => Expect::No,
        (_, "primary" | "standby-leader") => Expect::No,
        (_, "replica" | "read-only") => Expect::Pool,
        // Кворумный standby в Patroni 4 может отвечать и на `/sync`, и на `/async` — по режиму кворума.
        (Quorum, "sync" | "async") => Expect::Either,
        (Sync, "sync") | (Replica, "async") => Expect::Pool,
        _ => Expect::No,
    }
}

/// Оценки клеток участника по адресам из `HealthFacts.codes`.
pub fn grade_member(m: &HealthFacts) -> BTreeMap<String, HealthGrade> {
    let code = |e: &str| m.codes.get(e).copied().flatten();
    // Лидер подтверждён хотя бы одним адресом лидера — второй из пары тогда отвечает 503 законно.
    let leads = code("primary") == Some(200) || code("standby-leader") == Some(200);
    m.codes
        .iter()
        .map(|(e, status)| {
            let ex = expect(m.role, e);
            let grade = match (*status, ex) {
                (None, _) => HealthGrade::Bad,
                (Some(c), _) if c != 200 && c != 503 => HealthGrade::Bad,
                (Some(200), _) if !matches!(ex, Expect::No) => HealthGrade::Ok,
                (Some(_), Expect::Yes) => HealthGrade::Bad,
                (Some(_), Expect::Pool) => HealthGrade::Degraded,
                (Some(_), Expect::Either) => HealthGrade::NotApplicable,
                (Some(_), Expect::LeaderPair) if leads => HealthGrade::NotApplicable,
                (Some(_), Expect::LeaderPair) => HealthGrade::Bad,
                (Some(503), Expect::No) => HealthGrade::NotApplicable,
                // 200 на адресе чужой роли: `/sync` и `/async` — топология отстала от API, остальное — второй лидер
                // или реплика, которую балансировщик примет не за то.
                (Some(_), Expect::No) if matches!(e.as_str(), "sync" | "async") => {
                    HealthGrade::Degraded
                }
                (Some(_), Expect::No) => HealthGrade::Bad,
            };
            (e.clone(), grade)
        })
        .collect()
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

    fn grades(m: &HealthFacts) -> Vec<(&str, HealthGrade)> {
        let g = grade_member(m);
        HEALTH_ENDPOINTS
            .iter()
            .filter_map(|e| g.get(*e).map(|v| (*e, *v)))
            .collect()
    }

    const ALL_503: [(&str, Option<u16>); 7] = [
        ("primary", Some(503)),
        ("standby-leader", Some(503)),
        ("replica", Some(503)),
        ("read-only", Some(503)),
        ("sync", Some(503)),
        ("async", Some(503)),
        ("health", Some(503)),
    ];

    fn with(over: &[(&'static str, Option<u16>)]) -> Vec<(&'static str, Option<u16>)> {
        ALL_503
            .iter()
            .map(|(e, c)| {
                over.iter()
                    .find(|(o, _)| o == e)
                    .map_or((*e, *c), |(o, v)| (*o, *v))
            })
            .collect()
    }

    #[test]
    fn healthy_cluster_has_no_red_cells() {
        use HealthGrade::*;
        // Как на стенде: лидер обычного кластера и асинхронная реплика.
        let leader = h(
            "patroni2",
            CandidateRole::Leader,
            &with(&[
                ("primary", Some(200)),
                ("read-only", Some(200)),
                ("health", Some(200)),
            ]),
        );
        assert_eq!(
            grades(&leader),
            [
                ("primary", Ok),
                ("standby-leader", NotApplicable),
                ("replica", NotApplicable),
                ("read-only", Ok),
                ("sync", NotApplicable),
                ("async", NotApplicable),
                ("health", Ok),
            ]
        );
        let replica = h(
            "patroni1",
            CandidateRole::Replica,
            &with(&[
                ("replica", Some(200)),
                ("read-only", Some(200)),
                ("async", Some(200)),
                ("health", Some(200)),
            ]),
        );
        assert_eq!(
            grades(&replica),
            [
                ("primary", NotApplicable),
                ("standby-leader", NotApplicable),
                ("replica", Ok),
                ("read-only", Ok),
                ("sync", NotApplicable),
                ("async", Ok),
                ("health", Ok),
            ]
        );
        let sync = h(
            "s",
            CandidateRole::Sync,
            &with(&[
                ("replica", Some(200)),
                ("read-only", Some(200)),
                ("sync", Some(200)),
                ("health", Some(200)),
            ]),
        );
        assert!(
            grades(&sync)
                .iter()
                .all(|(_, g)| matches!(g, Ok | NotApplicable))
        );
        // Лидер standby-кластера: 200 на `/standby-leader`, а 503 на `/primary` законен.
        let standby = h(
            "a",
            CandidateRole::Leader,
            &with(&[
                ("standby-leader", Some(200)),
                ("replica", Some(200)),
                ("read-only", Some(200)),
                ("health", Some(200)),
            ]),
        );
        let g = grade_member(&standby);
        assert_eq!(g["primary"], NotApplicable);
        assert_eq!(g["standby-leader"], Ok);
        assert_eq!(g["replica"], Ok);
    }

    #[test]
    fn grades_out_of_pool_and_contradictions() {
        use HealthGrade::*;
        // Реплика вне пула (лаг или `noloadbalance`): жёлтая, не красная.
        let out = h("r", CandidateRole::Replica, &with(&[("health", Some(200))]));
        let g = grade_member(&out);
        assert_eq!(
            (g["replica"], g["read-only"], g["async"], g["primary"]),
            (Degraded, Degraded, Degraded, NotApplicable)
        );
        // Лидер без 200 ни на одном адресе лидера, `/health` не 200, реплика, которая отвечает как лидер.
        let lost = h("l", CandidateRole::Leader, &with(&[]));
        let g = grade_member(&lost);
        assert_eq!(
            (g["primary"], g["standby-leader"], g["health"]),
            (Bad, Bad, Bad)
        );
        let second = h(
            "r",
            CandidateRole::Replica,
            &with(&[("primary", Some(200)), ("health", Some(200))]),
        );
        assert_eq!(grade_member(&second)["primary"], Bad);
        // Топология отстала: асинхронная реплика уже отвечает 200 на `/sync`.
        let lag = h(
            "r",
            CandidateRole::Replica,
            &with(&[("sync", Some(200)), ("health", Some(200))]),
        );
        assert_eq!(grade_member(&lag)["sync"], Degraded);
        // Нет ответа и неожиданный код — всегда красные; неизвестная роль — 200 и 503 оба нормальны.
        let odd = h(
            "u",
            CandidateRole::Unknown,
            &[
                ("primary", None),
                ("replica", Some(500)),
                ("read-only", Some(503)),
                ("async", Some(200)),
            ],
        );
        let g = grade_member(&odd);
        assert_eq!(
            (g["primary"], g["replica"], g["read-only"], g["async"]),
            (Bad, Bad, NotApplicable, Ok)
        );
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
