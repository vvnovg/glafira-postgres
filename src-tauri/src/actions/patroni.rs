//! Действия Patroni (спец. P2 §8.5): модель действия, предусловия на топологии, запрос и адреса, отправка.
//! Предпросмотр и выполнение с журналом и наблюдением — задачи 7 и 8.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::topology::model::{Member, Topology};
use crate::topology::patroni::{HttpMethod, HttpReply, PatroniHttp};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum PatroniAction {
    /// `scheduled_at` — RFC 3339 со смещением.
    #[serde(rename_all = "camelCase")]
    Switchover {
        candidate: Option<String>,
        scheduled_at: Option<String>,
    },
    CancelSwitchover,
    Failover {
        candidate: String,
    },
    #[serde(rename_all = "camelCase")]
    Restart {
        member: String,
        only_pending: bool,
        scheduled_at: Option<String>,
    },
    CancelRestart {
        member: String,
    },
    Pause {
        paused: bool,
    },
}

/// Кому адресован запрос: API лидера (с обходом остальных при отказе соединения) или API участника.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Leader,
    Member(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatroniRequest {
    pub method: HttpMethod,
    pub path: &'static str,
    pub body: Option<Value>,
    pub route: Route,
}

/// Имя действия в журнале.
pub fn action_name(a: &PatroniAction) -> &'static str {
    match a {
        PatroniAction::Switchover { .. } => "switchover",
        PatroniAction::CancelSwitchover => "cancelSwitchover",
        PatroniAction::Failover { .. } => "failover",
        PatroniAction::Restart { .. } => "restart",
        PatroniAction::CancelRestart { .. } => "cancelRestart",
        PatroniAction::Pause { paused: true } => "pause",
        PatroniAction::Pause { paused: false } => "resume",
    }
}

/// Время расписания (RFC 3339 со смещением) в мс эпохи.
pub fn schedule_ms(s: &str) -> AppResult<i64> {
    let t = OffsetDateTime::parse(s, &Rfc3339).map_err(|_| {
        app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.badSchedule",
            "The schedule time {{value}} is not valid",
            value = s
        )
    })?;
    Ok((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

fn check_schedule(at: &Option<String>, now_ms: i64) -> AppResult<()> {
    match at {
        Some(s) if schedule_ms(s)? <= now_ms => Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.actions.pastSchedule",
            "The schedule time is in the past"
        )),
        _ => Ok(()),
    }
}

/// Лидер, принимающий запись: доступен и в состоянии `running`.
fn running_leader(t: &Topology) -> Option<&Member> {
    t.leader()
        .filter(|m| m.reachable && m.state.as_deref() == Some("running"))
}

fn bad_candidate(name: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.actions.badCandidate",
        "{{member}} cannot become the leader: it is not a replica of this cluster",
        member = name
    )
}

fn nothing_scheduled() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.actions.nothingScheduled",
        "Nothing is scheduled"
    )
}

/// Предусловия действия на текущей топологии (спец. §8.5); `now_ms` — часы приложения.
pub fn check(a: &PatroniAction, t: &Topology, now_ms: i64) -> AppResult<()> {
    let is_member = |n: &str| t.member(n).is_some();
    match a {
        PatroniAction::Switchover {
            candidate,
            scheduled_at,
        } => {
            let leader = running_leader(t).ok_or_else(|| {
                app_err!(
                    ErrorCode::InvalidInput,
                    "errors.actions.noLeader",
                    "There is no running leader to switch over from"
                )
            })?;
            if let Some(c) = candidate
                && (!is_member(c) || c == &leader.name)
            {
                return Err(bad_candidate(c));
            }
            check_schedule(scheduled_at, now_ms)
        }
        PatroniAction::CancelSwitchover => match t.scheduled_switchover {
            Some(_) => Ok(()),
            None => Err(nothing_scheduled()),
        },
        PatroniAction::Failover { candidate } => {
            if running_leader(t).is_some() {
                return Err(app_err!(
                    ErrorCode::InvalidInput,
                    "errors.actions.leaderHealthy",
                    "The leader is healthy: use switchover"
                ));
            }
            if !is_member(candidate) || t.leader().is_some_and(|l| &l.name == candidate) {
                return Err(bad_candidate(candidate));
            }
            Ok(())
        }
        PatroniAction::Restart {
            member,
            scheduled_at,
            ..
        } => {
            if !t.member(member).is_some_and(|m| m.reachable) {
                return Err(app_err!(
                    ErrorCode::NodeUnavailable,
                    "errors.actions.nodeUnavailable",
                    "Node {{member}} is unavailable",
                    member = member
                ));
            }
            check_schedule(scheduled_at, now_ms)
        }
        PatroniAction::CancelRestart { member } => {
            match t.member(member).and_then(|m| m.scheduled_restart.as_ref()) {
                Some(_) => Ok(()),
                None => Err(nothing_scheduled()),
            }
        }
        PatroniAction::Pause { paused } => {
            if t.paused.unwrap_or(false) == *paused {
                Err(app_err!(
                    ErrorCode::InvalidInput,
                    "errors.actions.pauseUnchanged",
                    "The cluster is already in this state"
                ))
            } else {
                Ok(())
            }
        }
    }
}

/// Запрос Patroni для действия (тела — как у `patronictl`).
pub fn request(a: &PatroniAction, t: &Topology) -> PatroniRequest {
    let leader = t.leader().map(|m| m.name.clone());
    match a {
        PatroniAction::Switchover {
            candidate,
            scheduled_at,
        } => {
            let mut body = serde_json::Map::new();
            if let Some(l) = leader {
                body.insert("leader".into(), json!(l));
            }
            if let Some(c) = candidate {
                body.insert("candidate".into(), json!(c));
            }
            if let Some(s) = scheduled_at {
                body.insert("scheduled_at".into(), json!(s));
            }
            PatroniRequest {
                method: HttpMethod::Post,
                path: "/switchover",
                body: Some(Value::Object(body)),
                route: Route::Leader,
            }
        }
        PatroniAction::CancelSwitchover => PatroniRequest {
            method: HttpMethod::Delete,
            path: "/switchover",
            body: None,
            route: Route::Leader,
        },
        PatroniAction::Failover { candidate } => PatroniRequest {
            method: HttpMethod::Post,
            path: "/failover",
            body: Some(json!({ "candidate": candidate })),
            route: Route::Leader,
        },
        PatroniAction::Restart {
            member,
            only_pending,
            scheduled_at,
        } => {
            let mut body = serde_json::Map::new();
            if *only_pending {
                body.insert("restart_pending".into(), json!(true));
            }
            if let Some(s) = scheduled_at {
                body.insert("schedule".into(), json!(s));
            }
            PatroniRequest {
                method: HttpMethod::Post,
                path: "/restart",
                body: Some(Value::Object(body)),
                route: Route::Member(member.clone()),
            }
        }
        PatroniAction::CancelRestart { member } => PatroniRequest {
            method: HttpMethod::Delete,
            path: "/restart",
            body: None,
            route: Route::Member(member.clone()),
        },
        PatroniAction::Pause { paused } => PatroniRequest {
            method: HttpMethod::Patch,
            path: "/config",
            // `null` удаляет ключ `pause` из динамической конфигурации — так снимается пауза.
            body: Some(if *paused {
                json!({ "pause": true })
            } else {
                json!({ "pause": null })
            }),
            route: Route::Leader,
        },
    }
}

/// Базовые адреса API по порядку: лидер, остальные участники, адреса профиля (без повторов); у участника — только он.
pub fn bases(t: &Topology, route: &Route, profile_urls: &[String]) -> Vec<String> {
    let trim = |u: &str| u.trim_end_matches('/').to_string();
    match route {
        Route::Member(name) => t
            .member(name)
            .and_then(|m| m.api_url.as_deref())
            .map(trim)
            .into_iter()
            .collect(),
        Route::Leader => {
            let mut out: Vec<String> = Vec::new();
            let leader = t.leader();
            let ordered = leader.into_iter().chain(
                t.members
                    .iter()
                    .filter(|m| leader.is_none_or(|l| l.name != m.name)),
            );
            for u in ordered
                .filter_map(|m| m.api_url.as_deref())
                .map(trim)
                .chain(profile_urls.iter().map(|u| trim(u)))
            {
                if !out.contains(&u) {
                    out.push(u);
                }
            }
            out
        }
    }
}

/// Отправляет запрос по адресам `bases`: следующий адрес — только при отказе соединения (Patroni не ответил);
/// ответ Patroni (любой код) — окончательный. 2xx — `Ok((адрес, ответ))`; 401/403 — `AUTH_FAILED`;
/// иначе — `errors.patroni.actionRejected` с кодом и первыми 500 символами ответа в `details`.
pub async fn send(
    http: &dyn PatroniHttp,
    req: &PatroniRequest,
    bases: &[String],
) -> AppResult<(String, HttpReply)> {
    let mut last: Option<AppError> = None;
    for base in bases {
        let url = format!("{base}{}", req.path);
        match http.send_json(req.method, &url, req.body.as_ref()).await {
            Ok(r) if (200..300).contains(&r.status) => return Ok((url, r)),
            Ok(r) if matches!(r.status, 401 | 403) => {
                return Err(app_err!(
                    ErrorCode::AuthFailed,
                    "errors.patroni.unauthorized",
                    "The Patroni API at {{url}} rejected the credentials",
                    url = url
                ));
            }
            Ok(r) => {
                return Err(app_err!(
                    ErrorCode::PatroniError,
                    "errors.patroni.actionRejected",
                    "Patroni rejected the action ({{status}})",
                    status = r.status
                )
                .with_details(crate::actions::journal::cut(
                    &r.body,
                    crate::actions::journal::RESPONSE_CHARS,
                )));
            }
            Err(e) if e.code == ErrorCode::AuthFailed => return Err(e),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        app_err!(
            ErrorCode::PatroniError,
            "errors.patroni.noUrls",
            "No Patroni API addresses"
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::MutexExt;
    use crate::topology::BoxFuture;
    use crate::topology::model::{Member, Role, ScheduledSwitchover, Topology};
    use std::sync::Mutex;

    fn member(name: &str, role: Role, state: &str) -> Member {
        let mut m = Member::new(name, format!("{name}-host"), 5432);
        m.role = role;
        m.state = Some(state.into());
        m.reachable = true;
        m.api_url = Some(format!("http://{name}:8008"));
        m
    }

    fn cluster() -> Topology {
        let mut t = Topology::empty_for_tests();
        t.members = vec![
            member("pg-1", Role::Primary, "running"),
            member("pg-2", Role::Replica, "streaming"),
            member("pg-3", Role::Replica, "streaming"),
        ];
        t
    }

    const NOW: i64 = 1_761_300_000_000; // 2025-10-24T10:00:00Z

    #[test]
    fn switchover_needs_a_running_leader_and_a_real_candidate() {
        let t = cluster();
        let ok = PatroniAction::Switchover {
            candidate: Some("pg-2".into()),
            scheduled_at: None,
        };
        assert!(check(&ok, &t, NOW).is_ok());
        let to_leader = PatroniAction::Switchover {
            candidate: Some("pg-1".into()),
            scheduled_at: None,
        };
        assert_eq!(
            check(&to_leader, &t, NOW).unwrap_err().key,
            "errors.actions.badCandidate"
        );
        let mut down = cluster();
        down.members[0].state = Some("stopped".into());
        assert_eq!(
            check(&ok, &down, NOW).unwrap_err().key,
            "errors.actions.noLeader"
        );
        let past = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: Some("2025-10-24T09:00:00Z".into()),
        };
        assert_eq!(
            check(&past, &t, NOW).unwrap_err().key,
            "errors.actions.pastSchedule"
        );
        let bad = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: Some("tomorrow".into()),
        };
        assert_eq!(
            check(&bad, &t, NOW).unwrap_err().key,
            "errors.actions.badSchedule"
        );
        let later = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: Some("2025-10-24T13:00:00+03:00".into()),
        };
        assert_eq!(
            check(&later, &t, NOW).unwrap_err().key,
            "errors.actions.pastSchedule",
            "смещение учитывается"
        );
        let future = PatroniAction::Switchover {
            candidate: None,
            scheduled_at: Some("2025-10-24T14:00:00+03:00".into()),
        };
        assert!(check(&future, &t, NOW).is_ok());
    }

    #[test]
    fn failover_only_without_a_healthy_leader() {
        let a = PatroniAction::Failover {
            candidate: "pg-2".into(),
        };
        assert_eq!(
            check(&a, &cluster(), NOW).unwrap_err().key,
            "errors.actions.leaderHealthy"
        );
        let mut t = cluster();
        t.members[0].reachable = false;
        assert!(check(&a, &t, NOW).is_ok());
        let unknown = PatroniAction::Failover {
            candidate: "pg-9".into(),
        };
        assert_eq!(
            check(&unknown, &t, NOW).unwrap_err().key,
            "errors.actions.badCandidate"
        );
    }

    #[test]
    fn cancels_and_pause_need_something_to_change() {
        let t = cluster();
        assert_eq!(
            check(&PatroniAction::CancelSwitchover, &t, NOW)
                .unwrap_err()
                .key,
            "errors.actions.nothingScheduled"
        );
        let mut s = cluster();
        s.scheduled_switchover = Some(ScheduledSwitchover {
            at: "2025-10-24T11:00:00Z".into(),
            from: None,
            to: None,
        });
        assert!(check(&PatroniAction::CancelSwitchover, &s, NOW).is_ok());
        let cancel = PatroniAction::CancelRestart {
            member: "pg-2".into(),
        };
        assert_eq!(
            check(&cancel, &t, NOW).unwrap_err().key,
            "errors.actions.nothingScheduled"
        );
        assert_eq!(
            check(&PatroniAction::Pause { paused: false }, &t, NOW)
                .unwrap_err()
                .key,
            "errors.actions.pauseUnchanged"
        );
        assert!(check(&PatroniAction::Pause { paused: true }, &t, NOW).is_ok());
        let mut down = cluster();
        down.members[1].reachable = false;
        let restart = PatroniAction::Restart {
            member: "pg-2".into(),
            only_pending: false,
            scheduled_at: None,
        };
        assert_eq!(
            check(&restart, &down, NOW).unwrap_err().key,
            "errors.actions.nodeUnavailable"
        );
    }

    #[test]
    fn requests_follow_the_patroni_api() {
        let t = cluster();
        let r = request(
            &PatroniAction::Switchover {
                candidate: Some("pg-2".into()),
                scheduled_at: None,
            },
            &t,
        );
        assert_eq!(
            (r.method, r.path, r.route.clone()),
            (HttpMethod::Post, "/switchover", Route::Leader)
        );
        assert_eq!(
            r.body,
            Some(serde_json::json!({"leader": "pg-1", "candidate": "pg-2"}))
        );
        let s = request(
            &PatroniAction::Switchover {
                candidate: None,
                scheduled_at: Some("2025-10-24T11:00:00Z".into()),
            },
            &t,
        );
        assert_eq!(
            s.body,
            Some(serde_json::json!({"leader": "pg-1", "scheduled_at": "2025-10-24T11:00:00Z"}))
        );
        assert_eq!(
            request(&PatroniAction::CancelSwitchover, &t).method,
            HttpMethod::Delete
        );
        let f = request(
            &PatroniAction::Failover {
                candidate: "pg-2".into(),
            },
            &t,
        );
        assert_eq!(
            (f.path, f.body),
            ("/failover", Some(serde_json::json!({"candidate": "pg-2"})))
        );
        let rs = request(
            &PatroniAction::Restart {
                member: "pg-2".into(),
                only_pending: true,
                scheduled_at: None,
            },
            &t,
        );
        assert_eq!(
            (rs.path, rs.route.clone()),
            ("/restart", Route::Member("pg-2".into()))
        );
        assert_eq!(rs.body, Some(serde_json::json!({"restart_pending": true})));
        let p = request(&PatroniAction::Pause { paused: true }, &t);
        assert_eq!(
            (p.method, p.path, p.body),
            (
                HttpMethod::Patch,
                "/config",
                Some(serde_json::json!({"pause": true}))
            )
        );
        let resume = request(&PatroniAction::Pause { paused: false }, &t);
        assert_eq!(
            resume.body,
            Some(serde_json::json!({"pause": null})),
            "null удаляет ключ из конфигурации"
        );
        assert_eq!(
            action_name(&PatroniAction::Pause { paused: false }),
            "resume"
        );
    }

    #[test]
    fn leader_first_then_other_members_then_profile() {
        let t = cluster();
        let profile = vec!["http://lb:8008".to_string(), "http://pg-1:8008".to_string()];
        assert_eq!(
            bases(&t, &Route::Leader, &profile),
            [
                "http://pg-1:8008",
                "http://pg-2:8008",
                "http://pg-3:8008",
                "http://lb:8008"
            ]
        );
        assert_eq!(
            bases(&t, &Route::Member("pg-3".into()), &profile),
            ["http://pg-3:8008"]
        );
    }

    /// Подделка: первый адрес не отвечает, второй отвечает заданным кодом; запросы записываются.
    struct Fake {
        dead: Vec<String>,
        status: u16,
        calls: Mutex<Vec<(HttpMethod, String, Option<serde_json::Value>)>>,
    }

    impl PatroniHttp for Fake {
        fn get_json<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, AppResult<serde_json::Value>> {
            Box::pin(async { Ok(serde_json::Value::Null) })
        }
        fn get_status<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, AppResult<u16>> {
            Box::pin(async { Ok(200) })
        }
        fn send_json<'a>(
            &'a self,
            method: HttpMethod,
            url: &'a str,
            body: Option<&'a serde_json::Value>,
        ) -> BoxFuture<'a, AppResult<HttpReply>> {
            self.calls
                .lock_safe()
                .push((method, url.to_string(), body.cloned()));
            let dead = self.dead.iter().any(|d| url.starts_with(d.as_str()));
            let status = self.status;
            Box::pin(async move {
                if dead {
                    Err(app_err!(
                        ErrorCode::PatroniError,
                        "errors.patroni.unreachable",
                        "Patroni API at {{url}} is unreachable",
                        url = url
                    ))
                } else {
                    Ok(HttpReply {
                        status,
                        body: "Successfully switched over".into(),
                    })
                }
            })
        }
    }

    #[tokio::test]
    async fn send_skips_unreachable_addresses_but_not_answers() {
        let req = request(
            &PatroniAction::Switchover {
                candidate: None,
                scheduled_at: None,
            },
            &cluster(),
        );
        let bases = vec![
            "http://pg-1:8008".to_string(),
            "http://pg-2:8008".to_string(),
        ];
        let ok = Fake {
            dead: vec!["http://pg-1".into()],
            status: 200,
            calls: Mutex::new(vec![]),
        };
        let (url, reply) = send(&ok, &req, &bases).await.unwrap();
        assert_eq!(
            (url.as_str(), reply.status),
            ("http://pg-2:8008/switchover", 200)
        );
        // Ответ Patroni — не повод идти к следующему адресу: отказ возвращается сразу.
        let rejected = Fake {
            dead: vec![],
            status: 412,
            calls: Mutex::new(vec![]),
        };
        let e = send(&rejected, &req, &bases).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.actionRejected")
        );
        assert_eq!(e.params["status"], "412");
        assert_eq!(rejected.calls.lock_safe().len(), 1);
        let denied = Fake {
            dead: vec![],
            status: 401,
            calls: Mutex::new(vec![]),
        };
        assert_eq!(
            send(&denied, &req, &bases).await.unwrap_err().code,
            ErrorCode::AuthFailed
        );
    }
    #[test]
    fn action_reads_camel_case_json_from_the_frontend() {
        let a: PatroniAction =
            serde_json::from_str(r#"{"kind":"switchover","candidate":"pg-2"}"#).unwrap();
        assert_eq!(
            a,
            PatroniAction::Switchover {
                candidate: Some("pg-2".into()),
                scheduled_at: None
            }
        );
        let r: PatroniAction = serde_json::from_str(
            r#"{"kind":"restart","member":"pg-2","onlyPending":true,"scheduledAt":"2025-10-24T11:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            r,
            PatroniAction::Restart {
                member: "pg-2".into(),
                only_pending: true,
                scheduled_at: Some("2025-10-24T11:00:00Z".into())
            }
        );
        let c: PatroniAction = serde_json::from_str(r#"{"kind":"cancelSwitchover"}"#).unwrap();
        assert_eq!(c, PatroniAction::CancelSwitchover);
        let p: PatroniAction = serde_json::from_str(r#"{"kind":"pause","paused":true}"#).unwrap();
        assert_eq!(p, PatroniAction::Pause { paused: true });
        assert_eq!(
            serde_json::to_value(PatroniAction::CancelRestart {
                member: "pg-2".into()
            })
            .unwrap(),
            serde_json::json!({"kind": "cancelRestart", "member": "pg-2"})
        );
    }

    #[test]
    fn restart_request_carries_the_schedule_and_cancel_goes_to_the_member() {
        let t = cluster();
        let r = request(
            &PatroniAction::Restart {
                member: "pg-3".into(),
                only_pending: false,
                scheduled_at: Some("2025-10-24T11:00:00Z".into()),
            },
            &t,
        );
        assert_eq!(
            r.body,
            Some(serde_json::json!({"schedule": "2025-10-24T11:00:00Z"}))
        );
        let c = request(
            &PatroniAction::CancelRestart {
                member: "pg-3".into(),
            },
            &t,
        );
        assert_eq!(
            (c.method, c.path, c.body, c.route),
            (
                HttpMethod::Delete,
                "/restart",
                None,
                Route::Member("pg-3".into())
            )
        );
    }

    #[test]
    fn a_scheduled_restart_can_be_cancelled_only_on_its_member() {
        let mut t = cluster();
        t.members[1].scheduled_restart = Some(crate::topology::model::ScheduledRestart {
            at: "2025-10-24T11:00:00Z".into(),
            postgres_version: None,
        });
        let on = |m: &str| PatroniAction::CancelRestart { member: m.into() };
        assert!(check(&on("pg-2"), &t, NOW).is_ok());
        assert_eq!(
            check(&on("pg-3"), &t, NOW).unwrap_err().key,
            "errors.actions.nothingScheduled"
        );
        assert_eq!(
            check(&on("pg-9"), &t, NOW).unwrap_err().key,
            "errors.actions.nothingScheduled"
        );
    }

    #[test]
    fn bases_without_a_leader_trim_slashes_and_skip_members_without_an_address() {
        let mut t = cluster();
        t.members[0].role = Role::Replica;
        t.members[1].api_url = Some("http://pg-2:8008/".into());
        t.members[2].api_url = None;
        let profile = vec!["http://lb:8008//".to_string()];
        assert_eq!(
            bases(&t, &Route::Leader, &profile),
            ["http://pg-1:8008", "http://pg-2:8008", "http://lb:8008"]
        );
        assert!(bases(&t, &Route::Member("pg-3".into()), &profile).is_empty());
        assert!(bases(&t, &Route::Member("pg-9".into()), &profile).is_empty());
    }

    #[tokio::test]
    async fn send_reports_the_last_connection_failure_or_missing_addresses() {
        let req = request(&PatroniAction::CancelSwitchover, &cluster());
        let all_dead = Fake {
            dead: vec!["http://".into()],
            status: 200,
            calls: Mutex::new(vec![]),
        };
        let bases = vec![
            "http://pg-1:8008".to_string(),
            "http://pg-2:8008".to_string(),
        ];
        let e = send(&all_dead, &req, &bases).await.unwrap_err();
        assert_eq!(
            (e.code, e.key.as_str()),
            (ErrorCode::PatroniError, "errors.patroni.unreachable")
        );
        assert_eq!(e.params["url"], "http://pg-2:8008/switchover");
        assert_eq!(all_dead.calls.lock_safe().len(), 2, "пробуются все адреса");
        let none = send(&all_dead, &req, &[]).await.unwrap_err();
        assert_eq!(none.key, "errors.patroni.noUrls");
    }

    #[tokio::test]
    async fn rejection_details_are_cut_to_the_response_limit() {
        struct Verbose;
        impl PatroniHttp for Verbose {
            fn get_json<'a>(
                &'a self,
                _url: &'a str,
            ) -> BoxFuture<'a, AppResult<serde_json::Value>> {
                Box::pin(async { Ok(serde_json::Value::Null) })
            }
            fn get_status<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, AppResult<u16>> {
                Box::pin(async { Ok(200) })
            }
            fn send_json<'a>(
                &'a self,
                _method: HttpMethod,
                _url: &'a str,
                _body: Option<&'a serde_json::Value>,
            ) -> BoxFuture<'a, AppResult<HttpReply>> {
                Box::pin(async {
                    Ok(HttpReply {
                        status: 412,
                        body: "ж".repeat(900),
                    })
                })
            }
        }
        let req = request(&PatroniAction::CancelSwitchover, &cluster());
        let e = send(&Verbose, &req, &["http://pg-1:8008".to_string()])
            .await
            .unwrap_err();
        assert_eq!(e.details.as_deref().map(|d| d.chars().count()), Some(500));
    }
}
