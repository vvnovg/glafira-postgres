//! Действия P2b на стенде: соединение, сессии, слоты (спец. P2 §8, §12.2). Без `GPG_IT_*` тест пропускается.

mod common;

use glafira_pg_lib::actions::Confirmation;
use glafira_pg_lib::actions::conn;
use glafira_pg_lib::actions::journal::Outcome;
use glafira_pg_lib::actions::sessions::{
    self as sa, DenyReason, SessionActionExecute, SessionActionRequest, SessionOutcome, SessionRef,
    SessionTarget, SignalKind,
};
use glafira_pg_lib::actions::slots::{self as slots_action, SlotDropRequest};
use glafira_pg_lib::conn::profile::Environment;
use glafira_pg_lib::diag::sql::BACKEND_START_US;

/// Сессия, простаивающая в транзакции: `BEGIN; SELECT 1` и ожидание. Возвращает pid; клиент держит сессию.
async fn idle_in_transaction(c: &tokio_postgres::Client) -> i32 {
    c.batch_execute("BEGIN; SELECT 1").await.unwrap();
    c.query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0)
}

/// `backend_start` сессии в микросекундах — тем же выражением, что в приложении.
async fn backend_start_us(su: &tokio_postgres::Client, pid: i32) -> i64 {
    su.query_one(
        &format!("SELECT {BACKEND_START_US} FROM pg_stat_activity a WHERE a.pid = $1"),
        &[&pid],
    )
    .await
    .unwrap()
    .get(0)
}

fn request(member: &str, target: SessionTarget) -> SessionActionRequest {
    SessionActionRequest {
        member: member.into(),
        kind: SignalKind::Terminate,
        target,
    }
}

#[tokio::test]
async fn action_connection_is_read_write_and_tagged() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let c = conn::open(&st, "it", &member, None).await.unwrap();
    let row = c
        .client
        .query_one(
            "SELECT current_setting('default_transaction_read_only'), current_setting('application_name'), \
                    current_setting('statement_timeout'), current_setting('search_path')",
            &[],
        )
        .await
        .unwrap();
    let got: (String, String, String, String) = (row.get(0), row.get(1), row.get(2), row.get(3));
    assert_eq!(
        got,
        (
            "off".into(),
            "glafira-pg".into(),
            "5s".into(),
            "pg_catalog".into()
        )
    );
}

#[tokio::test]
async fn unknown_member_is_rejected_before_connecting() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let Err(e) = conn::open(&st, "it", "no-such-node", None).await else {
        panic!("узла нет — соединения быть не должно")
    };
    assert_eq!(e.key, "errors.topology.unknownMember");
}

#[tokio::test]
async fn preview_lists_idle_sessions_with_rights() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let app_pid = idle_in_transaction(&app).await;
    let su_pid = idle_in_transaction(&su).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let p = sa::preview(
        &st,
        "it",
        &SessionActionRequest {
            member: member.clone(),
            kind: SignalKind::Terminate,
            target: SessionTarget::IdleInTransaction { older_than_s: 1.0 },
        },
    )
    .await
    .unwrap();
    assert_eq!(p.member, member);
    let row = |pid| {
        p.rows
            .iter()
            .find(|r| r.session.pid == pid)
            .unwrap_or_else(|| panic!("нет {pid}: {p:?}"))
    };
    // glafira_diag — член pg_signal_backend: сессию glafira_app можно, суперпользователя — нет.
    assert!(row(app_pid).allowed, "{:?}", row(app_pid));
    assert_eq!(row(su_pid).reason, Some(DenyReason::SuperuserSession));
    assert!(!row(su_pid).allowed);
    assert_eq!(row(app_pid).session.user.as_deref(), Some("glafira_app"));
    assert_eq!(
        row(app_pid).session.backend_start_us,
        backend_start_us(&su, app_pid).await
    );
    // Выше порога — сессий нет; порог ниже возраста простоя — те же строки.
    let none = sa::preview(
        &st,
        "it",
        &request(
            &member,
            SessionTarget::IdleInTransaction {
                older_than_s: 3600.0,
            },
        ),
    )
    .await
    .unwrap();
    assert!(none.rows.iter().all(|r| r.session.pid != app_pid));
}

#[tokio::test]
async fn preview_addresses_a_session_by_pid_and_backend_start() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let pid = idle_in_transaction(&app).await;
    let start = backend_start_us(&su, pid).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let pick = async |target| sa::preview(&st, "it", &request(&member, target)).await;
    // Верная пара — одна строка с описанием сессии.
    let ok = pick(SessionTarget::Selected {
        sessions: vec![SessionRef {
            pid,
            backend_start_us: start,
        }],
    })
    .await
    .unwrap();
    assert_eq!(ok.rows.len(), 1, "{ok:?}");
    let s = &ok.rows[0].session;
    assert_eq!((s.pid, s.backend_start_us), (pid, start));
    assert_eq!(s.state.as_deref(), Some("idle in transaction"));
    assert_eq!(s.backend_type.as_deref(), Some("client backend"));
    assert_eq!(s.database.as_deref(), Some("orders"));
    // Последний запрос сессии — вызов pid в помощнике.
    assert_eq!(s.query.as_deref(), Some("SELECT pg_backend_pid()"));
    assert!(ok.rows[0].allowed);
    // PID тот же, а время запуска другое: это уже другая сессия, в предпросмотр она не попадает.
    for target in [
        SessionTarget::Selected {
            sessions: vec![SessionRef {
                pid,
                backend_start_us: start + 1,
            }],
        },
        SessionTarget::RootBlocker {
            pid,
            backend_start_us: start + 1,
        },
    ] {
        let p = pick(target).await.unwrap();
        assert!(p.rows.is_empty(), "{p:?}");
    }
    let root = pick(SessionTarget::RootBlocker {
        pid,
        backend_start_us: start,
    })
    .await
    .unwrap();
    assert_eq!(root.rows.len(), 1);
    // Пустой выбор и несуществующий pid дают пустой предпросмотр, а не ошибку.
    assert!(
        pick(SessionTarget::Selected { sessions: vec![] })
            .await
            .unwrap()
            .rows
            .is_empty()
    );
    // Неизвестный узел отвергается до обращения к серверу.
    let e = sa::preview(
        &st,
        "it",
        &request("no-such-node", SessionTarget::Selected { sessions: vec![] }),
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.topology.unknownMember");
}

/// Роль без `pg_read_all_stats` не видит `backend_start` чужих сессий: такая сессия не адресуется парой
/// `(pid, backend_start)` и в предпросмотр не попадает (без паники на пустом значении); свою роль видно.
#[tokio::test]
async fn preview_skips_sessions_the_role_cannot_see() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let su = common::superuser(&addr, "postgres").await;
    let app_pid = idle_in_transaction(&app).await;
    let su_pid = idle_in_transaction(&su).await;
    let app_start = backend_start_us(&su, app_pid).await;
    let su_start = backend_start_us(&su, su_pid).await;
    let (_dir, st) =
        common::open_server_as(&addr, "orders", common::APP_USER, common::APP_PASSWORD).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let p = sa::preview(
        &st,
        "it",
        &request(
            &member,
            SessionTarget::Selected {
                sessions: vec![
                    SessionRef {
                        pid: app_pid,
                        backend_start_us: app_start,
                    },
                    SessionRef {
                        pid: su_pid,
                        backend_start_us: su_start,
                    },
                ],
            },
        ),
    )
    .await
    .unwrap();
    let pids: Vec<i32> = p.rows.iter().map(|r| r.session.pid).collect();
    assert_eq!(pids, [app_pid], "{p:?}");
    // Своя роль — можно и без pg_signal_backend.
    assert!(p.rows[0].allowed, "{:?}", p.rows[0]);
}

/// Есть ли в `pg_stat_activity` сессия с таким pid (смотрит посторонний сеанс `probe`).
async fn session_exists(probe: &tokio_postgres::Client, pid: i32) -> bool {
    probe
        .query_opt("SELECT 1 FROM pg_stat_activity WHERE pid = $1", &[&pid])
        .await
        .unwrap()
        .is_some()
}

fn exec(
    member: &str,
    kind: SignalKind,
    target: SessionTarget,
    sessions: Vec<SessionRef>,
) -> SessionActionExecute {
    SessionActionExecute {
        member: member.into(),
        kind,
        target,
        sessions,
    }
}

/// Строки журнала действий как JSON (по одной на действие).
fn journal_lines(dir: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("actions.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn terminating_the_root_blocker_frees_the_queue_and_writes_the_journal() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let holder = common::app(&addr).await;
    holder
        .batch_execute(
            "BEGIN; UPDATE shop.orders SET id = id WHERE id = (SELECT min(id) FROM shop.orders)",
        )
        .await
        .unwrap();
    let holder_pid: i32 = holder
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let victim = common::app(&addr).await;
    let victim_pid: i32 = victim
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let waiting = tokio::spawn(async move {
        victim
            .execute(
                "UPDATE shop.orders SET id = id WHERE id = (SELECT min(id) FROM shop.orders)",
                &[],
            )
            .await
    });
    let probe = common::superuser(&addr, "orders").await;
    common::wait_lock_wait(&probe, victim_pid, "жертва ждёт держателя").await;
    let start = backend_start_us(&probe, holder_pid).await;
    let (dir, st) = common::open_server_in(&addr, "orders").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let r = sa::execute(
        &st,
        "it",
        &exec(
            &member,
            SignalKind::Terminate,
            SessionTarget::RootBlocker {
                pid: holder_pid,
                backend_start_us: start,
            },
            vec![SessionRef {
                pid: holder_pid,
                backend_start_us: start,
            }],
        ),
        &Confirmation::default(),
    )
    .await
    .unwrap();
    assert_eq!(r.results[0].outcome, SessionOutcome::Terminated, "{r:?}");
    assert_eq!(r.outcome, Outcome::Ok);
    assert!(r.journal_written);
    // Очередь освободилась: жертва дождалась блокировки.
    tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let journal = std::fs::read_to_string(dir.path().join("actions.jsonl")).unwrap();
    assert!(
        journal.contains("\"action\":\"terminate\"")
            && journal.contains("\"result\":\"terminated\""),
        "{journal}"
    );
    // Строка сессии в журнале — снимок до сигнала: роль, база, состояние, возраст транзакции, последний запрос.
    let line = &journal_lines(dir.path())[0];
    let s = &line["sessions"][0];
    assert_eq!(s["pid"], holder_pid);
    assert_eq!(s["backendStartUs"], start);
    assert_eq!(s["user"], "glafira_app");
    assert_eq!(s["database"], "orders");
    assert_eq!(s["state"], "idle in transaction");
    assert!(s["xactAgeS"].as_f64().is_some(), "{s}");
    // Последний запрос держателя — вызов pid после UPDATE.
    assert_eq!(s["query"], "SELECT pg_backend_pid()");
    assert_eq!(line["connection"]["id"], "it");
    assert_eq!(line["member"], member.as_str());
    assert_eq!(line["outcome"], "ok");
}

#[tokio::test]
async fn stale_backend_start_is_gone_and_woken_session_no_longer_matches() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let pid = idle_in_transaction(&app).await;
    let probe = common::superuser(&addr, "postgres").await;
    let start = backend_start_us(&probe, pid).await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let execute = async |sessions, target| {
        sa::execute(
            &st,
            "it",
            &exec(&member, SignalKind::Terminate, target, sessions),
            &Confirmation::default(),
        )
        .await
        .unwrap()
    };
    // Чужой backend_start — сессии «нет», живая сессия не тронута.
    let r = execute(
        vec![SessionRef {
            pid,
            backend_start_us: start - 1,
        }],
        SessionTarget::Selected { sessions: vec![] },
    )
    .await;
    assert_eq!(r.results[0].outcome, SessionOutcome::Gone);
    assert_eq!(r.outcome, Outcome::Ok);
    assert!(session_exists(&probe, pid).await, "сессия жива");
    // Сессия проснулась (транзакция закончилась) — критерий idle in transaction не выполняется.
    app.batch_execute("COMMIT").await.unwrap();
    let r = execute(
        vec![SessionRef {
            pid,
            backend_start_us: start,
        }],
        SessionTarget::IdleInTransaction { older_than_s: 1.0 },
    )
    .await;
    assert_eq!(r.results[0].outcome, SessionOutcome::NoLongerMatches);
    assert!(session_exists(&probe, pid).await, "сессия жива");
    // В журнале есть обе сессии с итогом; снимка сессии нет — сигнал не отправлялся.
    let lines = journal_lines(dir.path());
    assert_eq!(lines.len(), 2, "{lines:?}");
    for (line, result) in lines.iter().zip(["gone", "noLongerMatches"]) {
        let rec = &line["sessions"][0];
        assert_eq!(rec["result"], result, "{line}");
        assert_eq!(rec["pid"], pid);
        assert!(rec["user"].is_null() && rec["query"].is_null(), "{rec}");
        assert_eq!(line["outcome"], "ok");
        assert!(line["code"].is_null(), "{line}");
    }
}

/// Одинокая сессия в транзакции никого не блокирует: критерий `rootBlocker` в самом запросе не выполняется, сигнал не
/// уходит и сессия остаётся жива (проверяется посторонним сеансом: запрос в самой сессии сдвинул бы `state_change`).
/// Контроль: тот же сигнал без критерия (`selected`) её завершает — значит, отказ дал именно критерий.
#[tokio::test]
async fn lone_idle_session_is_not_a_root_blocker() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let holder = common::app(&addr).await;
    let pid = idle_in_transaction(&holder).await;
    let probe = common::superuser(&addr, "postgres").await;
    let start = backend_start_us(&probe, pid).await;
    let (dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let pair = SessionRef {
        pid,
        backend_start_us: start,
    };
    let run = async |target| {
        sa::execute(
            &st,
            "it",
            &exec(&member, SignalKind::Terminate, target, vec![pair]),
            &Confirmation::default(),
        )
        .await
        .unwrap()
    };
    let r = run(SessionTarget::RootBlocker {
        pid,
        backend_start_us: start,
    })
    .await;
    assert_eq!(
        r.results[0].outcome,
        SessionOutcome::NoLongerMatches,
        "{r:?}"
    );
    assert_eq!(r.outcome, Outcome::Ok);
    assert!(session_exists(&probe, pid).await, "сессия жива");
    let r = run(SessionTarget::Selected { sessions: vec![] }).await;
    assert_eq!(r.results[0].outcome, SessionOutcome::Terminated, "{r:?}");
    assert!(!session_exists(&probe, pid).await, "сессия завершена");
    let lines = journal_lines(dir.path());
    assert_eq!(lines[0]["sessions"][0]["result"], "noLongerMatches");
    assert_eq!(lines[1]["sessions"][0]["result"], "terminated");
}

/// Смешанный пакет: завершена, отказано в правах, давно нет. Итог `partial`, в журнале все три сессии с итогами,
/// кода ошибки у записи нет (он пишется только при итоге `error`).
#[tokio::test]
async fn mixed_batch_is_partial_and_every_session_is_journaled() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let (a, b) = (common::app(&addr).await, common::app(&addr).await);
    let su = common::superuser(&addr, "postgres").await;
    let (pid_a, pid_b) = (idle_in_transaction(&a).await, idle_in_transaction(&b).await);
    let pid_su = idle_in_transaction(&su).await;
    let (start_a, start_b, start_su) = (
        backend_start_us(&su, pid_a).await,
        backend_start_us(&su, pid_b).await,
        backend_start_us(&su, pid_su).await,
    );
    let (dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let r = sa::execute(
        &st,
        "it",
        &exec(
            &member,
            SignalKind::Terminate,
            SessionTarget::Selected { sessions: vec![] },
            vec![
                SessionRef {
                    pid: pid_a,
                    backend_start_us: start_a,
                },
                SessionRef {
                    pid: pid_su,
                    backend_start_us: start_su,
                },
                // Другой запуск того же pid: сессии с таким временем запуска нет, живая сессия не тронута.
                SessionRef {
                    pid: pid_b,
                    backend_start_us: start_b - 1,
                },
            ],
        ),
        &Confirmation::default(),
    )
    .await
    .unwrap();
    let outcomes: Vec<_> = r.results.iter().map(|x| &x.outcome).collect();
    assert_eq!(
        outcomes,
        [
            &SessionOutcome::Terminated,
            &SessionOutcome::NotAllowed,
            &SessionOutcome::Gone
        ],
        "{r:?}"
    );
    assert_eq!(r.outcome, Outcome::Partial);
    assert!(session_exists(&su, pid_su).await && session_exists(&su, pid_b).await);
    let lines = journal_lines(dir.path());
    assert_eq!(lines[0]["outcome"], "partial");
    assert!(lines[0]["code"].is_null(), "{}", lines[0]);
    let results: Vec<_> = lines[0]["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["result"].as_str().unwrap())
        .collect();
    assert_eq!(results, ["terminated", "notAllowed", "gone"]);
    // Снимок есть только там, где строка была прочитана до сигнала.
    assert_eq!(lines[0]["sessions"][0]["user"], "glafira_app");
    assert!(lines[0]["sessions"][1]["user"].is_null());
}

#[tokio::test]
async fn cancel_interrupts_a_running_query() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let app = common::app(&addr).await;
    let pid: i32 = app
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    let sleeping = tokio::spawn(async move { app.execute("SELECT pg_sleep(60)", &[]).await });
    let probe = common::superuser(&addr, "postgres").await;
    common::wait_until(
        "pg_sleep идёт",
        std::time::Duration::from_secs(10),
        async || {
            probe
                .query_opt(
                    "SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND wait_event = 'PgSleep'",
                    &[&pid],
                )
                .await
                .unwrap()
                .is_some()
        },
    )
    .await;
    let start = backend_start_us(&probe, pid).await;
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let r = sa::execute(
        &st,
        "it",
        &exec(
            &member,
            SignalKind::Cancel,
            SessionTarget::Selected { sessions: vec![] },
            vec![SessionRef {
                pid,
                backend_start_us: start,
            }],
        ),
        &Confirmation::default(),
    )
    .await
    .unwrap();
    assert_eq!(r.results[0].outcome, SessionOutcome::SignalSent);
    let err = sleeping.await.unwrap().unwrap_err();
    assert_eq!(
        err.code(),
        Some(&tokio_postgres::error::SqlState::QUERY_CANCELED),
        "57014 у клиента"
    );
}

/// Пакет по критерию: обе сессии, простаивающие в транзакции, завершены; на prod — с именем подключения и числом
/// сессий; в журнале — обе строки и пометка `prodConfirmed`.
#[tokio::test]
async fn idle_in_transaction_batch_terminates_every_session_and_is_journaled() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    common::ensure_fixtures(&addr).await;
    let (a, b) = (common::app(&addr).await, common::app(&addr).await);
    let (pid_a, pid_b) = (idle_in_transaction(&a).await, idle_in_transaction(&b).await);
    let probe = common::superuser(&addr, "postgres").await;
    let sessions = vec![
        SessionRef {
            pid: pid_a,
            backend_start_us: backend_start_us(&probe, pid_a).await,
        },
        SessionRef {
            pid: pid_b,
            backend_start_us: backend_start_us(&probe, pid_b).await,
        },
    ];
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let (dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let mut p = st.profile("it").unwrap();
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let req = exec(
        &member,
        SignalKind::Terminate,
        SessionTarget::IdleInTransaction { older_than_s: 1.0 },
        sessions,
    );
    // Без числа сессий пакет на prod не стартует.
    let named = Confirmation {
        confirm: Some(p.name.clone()),
        typed_name: None,
    };
    let e = sa::execute(&st, "it", &req, &named).await.unwrap_err();
    assert_eq!(e.key, "errors.guard.typeToConfirm");
    // Живость проверяет посторонний сеанс: запрос самой сессии сдвинул бы её `state_change` и сбил критерий.
    assert!(session_exists(&probe, pid_a).await, "сессия жива");
    let confirmed = Confirmation {
        typed_name: Some("2".into()),
        ..named
    };
    let r = sa::execute(&st, "it", &req, &confirmed).await.unwrap();
    assert_eq!(
        r.results.iter().map(|x| &x.outcome).collect::<Vec<_>>(),
        [&SessionOutcome::Terminated, &SessionOutcome::Terminated],
        "{r:?}"
    );
    assert_eq!(r.outcome, Outcome::Ok);
    assert!(
        a.query_one("SELECT 1", &[]).await.is_err(),
        "сессия A завершена"
    );
    assert!(
        b.query_one("SELECT 1", &[]).await.is_err(),
        "сессия B завершена"
    );
    // Отказ guard ничего не записал; запись о пакете — одна.
    let lines = journal_lines(dir.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["prodConfirmed"], true);
    assert_eq!(lines[0]["connection"]["environment"], "prod");
    assert_eq!(lines[0]["sessions"].as_array().unwrap().len(), 2);
}

/// `glafira_diag` состоит в `pg_signal_backend`, но не суперпользователь: сигнал сессии суперпользователя сервер
/// отвергает кодом 42501 — итог сессии `notAllowed`, сессия не тронута, действие записано как ошибка.
#[tokio::test]
async fn signal_to_a_superuser_session_is_not_allowed() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let su = common::superuser(&addr, "postgres").await;
    let pid = idle_in_transaction(&su).await;
    let start = backend_start_us(&su, pid).await;
    let (dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let r = sa::execute(
        &st,
        "it",
        &exec(
            &member,
            SignalKind::Terminate,
            SessionTarget::Selected { sessions: vec![] },
            vec![SessionRef {
                pid,
                backend_start_us: start,
            }],
        ),
        &Confirmation::default(),
    )
    .await
    .unwrap();
    assert_eq!(r.results[0].outcome, SessionOutcome::NotAllowed, "{r:?}");
    assert_eq!(r.outcome, Outcome::Error);
    assert!(r.journal_written);
    assert!(su.query_one("SELECT 1", &[]).await.is_ok(), "сессия жива");
    // Запись — ошибка с кодом отказа; сессия в ней есть, снимка нет (сигнал не отправлялся).
    let lines = journal_lines(dir.path());
    assert_eq!(lines[0]["outcome"], "error");
    assert_eq!(lines[0]["code"], "NOT_AUTHORIZED");
    let rec = &lines[0]["sessions"][0];
    assert_eq!(rec["result"], "notAllowed");
    assert_eq!(
        (rec["pid"].as_i64(), rec["backendStartUs"].as_i64()),
        (Some(pid as i64), Some(start))
    );
    assert!(rec["user"].is_null() && rec["query"].is_null(), "{rec}");
}

#[tokio::test]
async fn read_only_and_prod_are_guarded_in_rust() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let req = exec(
        &member,
        SignalKind::Cancel,
        SessionTarget::Selected { sessions: vec![] },
        vec![SessionRef {
            pid: 1,
            backend_start_us: 1,
        }],
    );
    let mut p = st.profile("it").unwrap();
    p.read_only = true;
    st.connections.upsert(p.clone()).unwrap();
    let e = sa::execute(&st, "it", &req, &Confirmation::default())
        .await
        .unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ReadOnly);
    p.read_only = false;
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let e = sa::execute(&st, "it", &req, &Confirmation::default())
        .await
        .unwrap_err();
    assert_eq!(
        e.code,
        glafira_pg_lib::error::ErrorCode::ConfirmationRequired
    );
    // Отказ guard ничего не пишет в журнал.
    assert!(st.actions.list(Some("it"), 10).is_empty());
}

/// Первый узел топологии открытого подключения `it`.
fn first_member(st: &glafira_pg_lib::state::AppState) -> String {
    st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone()
}

fn confirm_slot(name: &str) -> Confirmation {
    Confirmation {
        confirm: None,
        typed_name: Some(name.into()),
    }
}

/// Мёртвый физический слот: без `REPLICATION` удалить его нельзя (отказ правом, слот на месте), суперпользователь
/// удаляет после ввода имени и оставляет строку журнала; активный слот реплики не удаляется и журнал не пишет.
#[tokio::test]
async fn dead_slot_is_dropped_active_slot_is_not() {
    let Some(addr) = common::env("GPG_IT_PRIMARY") else {
        return;
    };
    let su = common::superuser(&addr, "postgres").await;
    su.batch_execute(
        "SELECT pg_drop_replication_slot('it_dead_slot') FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'; \
         SELECT pg_create_physical_replication_slot('it_dead_slot', true)",
    )
    .await
    .unwrap();
    let a = addr.clone();
    let _cleanup = common::restore_on_drop(move || {
        common::run_sql_blocking(
            &a,
            "postgres",
            "SELECT pg_drop_replication_slot('it_dead_slot') FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'",
        );
    });
    // glafira_diag без REPLICATION: удаление отказано правом, слот на месте, отказ записан в журнал.
    let (d1, diag) = common::open_server(&addr).await;
    let member = first_member(&diag);
    let req = |slot: &str| SlotDropRequest {
        member: member.clone(),
        slot: slot.into(),
    };
    let p = slots_action::preview(&diag, "it", &req("it_dead_slot"))
        .await
        .unwrap();
    assert_eq!(p.member, member);
    assert!(!p.slot.active && p.slot.slot_type == "physical");
    let e = slots_action::drop(
        &diag,
        "it",
        &req("it_dead_slot"),
        &confirm_slot("it_dead_slot"),
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.actions.replicationRequired");
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::NotAuthorized);
    assert!(
        su.query_opt(
            "SELECT 1 FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'",
            &[]
        )
        .await
        .unwrap()
        .is_some(),
        "слот не тронут"
    );
    let lines = journal_lines(d1.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        (
            lines[0]["action"].as_str(),
            lines[0]["outcome"].as_str(),
            lines[0]["code"].as_str(),
            lines[0]["slot"]["name"].as_str()
        ),
        (
            Some("dropSlot"),
            Some("error"),
            Some("NOT_AUTHORIZED"),
            Some("it_dead_slot")
        )
    );
    // Суперпользователь: неверное имя — подтверждение; верное — слот удалён, строка журнала.
    let (d2, st) = common::open_server_as(
        &addr,
        "postgres",
        common::SUPERUSER,
        common::SUPERUSER_PASSWORD,
    )
    .await;
    let e = slots_action::drop(&st, "it", &req("it_dead_slot"), &confirm_slot("it_dead"))
        .await
        .unwrap_err();
    assert_eq!(
        e.code,
        glafira_pg_lib::error::ErrorCode::ConfirmationRequired
    );
    assert!(
        su.query_opt(
            "SELECT 1 FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'",
            &[]
        )
        .await
        .unwrap()
        .is_some(),
        "неверное имя слот не удаляет"
    );
    let r = slots_action::drop(
        &st,
        "it",
        &req("it_dead_slot"),
        &confirm_slot("it_dead_slot"),
    )
    .await
    .unwrap();
    assert!(r.journal_written);
    assert!(
        su.query_opt(
            "SELECT 1 FROM pg_replication_slots WHERE slot_name = 'it_dead_slot'",
            &[]
        )
        .await
        .unwrap()
        .is_none()
    );
    // Повторное удаление: слота уже нет на узле — ошибка предпросмотра, журнал не пишется.
    let e = slots_action::drop(
        &st,
        "it",
        &req("it_dead_slot"),
        &confirm_slot("it_dead_slot"),
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.actions.slotNotFound");
    // Активный слот реплики не удаляется.
    let e = slots_action::drop(&st, "it", &req("replica1"), &confirm_slot("replica1"))
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.actions.slotActiveOrGone");
    assert!(
        su.query_one(
            "SELECT active FROM pg_replication_slots WHERE slot_name = 'replica1'",
            &[]
        )
        .await
        .unwrap()
        .get::<_, bool>(0),
        "слот реплики жив"
    );
    assert_eq!(
        st.actions.list(Some("it"), 10).len(),
        1,
        "отказ «активен» и «нет слота» журнал не пишут"
    );
    let lines = journal_lines(d2.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        (
            lines[0]["action"].as_str(),
            lines[0]["outcome"].as_str(),
            lines[0]["slot"]["name"].as_str(),
            lines[0]["slot"]["slotType"].as_str(),
            lines[0]["prodConfirmed"].as_bool()
        ),
        (
            Some("dropSlot"),
            Some("ok"),
            Some("it_dead_slot"),
            Some("physical"),
            Some(false)
        )
    );
    assert!(lines[0]["code"].is_null(), "{}", lines[0]);
}

/// Логический слот удаляется из соединения с его БД: подключение смотрит в `postgres`, слот — в `orders`.
#[tokio::test]
async fn logical_slot_is_dropped_through_its_own_database() {
    let Some(addr) = common::lab_addr() else {
        return;
    };
    let orders = common::superuser(&addr, "orders").await;
    orders
        .batch_execute(
            "SELECT pg_drop_replication_slot('it_logical_slot') FROM pg_replication_slots WHERE slot_name = 'it_logical_slot'; \
             SELECT pg_create_logical_replication_slot('it_logical_slot', 'pgoutput')",
        )
        .await
        .unwrap();
    let a = addr.clone();
    let _cleanup = common::restore_on_drop(move || {
        common::run_sql_blocking(
            &a,
            "orders",
            "SELECT pg_drop_replication_slot('it_logical_slot') FROM pg_replication_slots WHERE slot_name = 'it_logical_slot'",
        );
    });
    let (dir, st) = common::open_server_as(
        &addr,
        "postgres",
        common::SUPERUSER,
        common::SUPERUSER_PASSWORD,
    )
    .await;
    let req = SlotDropRequest {
        member: first_member(&st),
        slot: "it_logical_slot".into(),
    };
    let p = slots_action::preview(&st, "it", &req).await.unwrap();
    assert_eq!(
        (p.slot.slot_type.as_str(), p.slot.database.as_deref()),
        ("logical", Some("orders"))
    );
    let r = slots_action::drop(&st, "it", &req, &confirm_slot("it_logical_slot"))
        .await
        .unwrap();
    assert!(r.journal_written);
    assert!(
        orders
            .query_opt(
                "SELECT 1 FROM pg_replication_slots WHERE slot_name = 'it_logical_slot'",
                &[]
            )
            .await
            .unwrap()
            .is_none()
    );
    let lines = journal_lines(dir.path());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["slot"]["database"], "orders");
    assert_eq!(lines[0]["slot"]["plugin"], "pgoutput");
}

/// read-only и prod проверяются в Rust до обращения к серверу; отказ ничего не пишет в журнал.
#[tokio::test]
async fn slot_drop_is_guarded_in_rust() {
    let Some(addr) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&addr).await;
    let req = SlotDropRequest {
        member: first_member(&st),
        slot: "no_such_slot".into(),
    };
    let mut p = st.profile("it").unwrap();
    p.read_only = true;
    st.connections.upsert(p.clone()).unwrap();
    let e = slots_action::drop(&st, "it", &req, &confirm_slot("no_such_slot"))
        .await
        .unwrap_err();
    assert_eq!(e.code, glafira_pg_lib::error::ErrorCode::ReadOnly);
    p.read_only = false;
    p.environment = Environment::Prod;
    st.connections.upsert(p.clone()).unwrap();
    let e = slots_action::drop(&st, "it", &req, &confirm_slot("no_such_slot"))
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.guard.prodConfirm");
    // Имя подключения введено, имя слота — нет: до сервера дело не доходит (слота «no_such_slot» там и нет).
    let named = Confirmation {
        confirm: Some(p.name.clone()),
        typed_name: None,
    };
    let e = slots_action::drop(&st, "it", &req, &named)
        .await
        .unwrap_err();
    assert_eq!(e.key, "errors.guard.typeToConfirm");
    assert!(st.actions.list(Some("it"), 10).is_empty());
}
