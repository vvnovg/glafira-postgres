//! Инструмент «Запросы» на стенде: топ «с момента сброса» на pg-perf (`GPG_IT_PERF`), SQL pgss на матрице
//! 13–18 (`GPG_IT_MATRIX`), отсутствие расширения на pg-single, скрытые строки под ролью без pg_read_all_stats.
//! Запускать с `--test-threads=1`; перед тестами pg-perf должен был поработать под `heavy.sql` (dev/perf/README.md).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use glafira_pg_lib::diag::queries::{self, Detect, QueriesMode, QueriesParams};
use glafira_pg_lib::diag::query_detail;
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::sampler::registry::SamplerSink;
use glafira_pg_lib::sampler::statements::{self, StatementsEvent};
use pg_advisor::queries::QuerySort;

fn since_reset(sort: QuerySort) -> QueriesParams {
    QueriesParams {
        mode: QueriesMode::SinceReset,
        sort,
        limit: 50,
        database: None,
        toplevel_only: true,
    }
}

fn request(p: QueriesParams) -> DiagRequest {
    DiagRequest {
        tool: Tool::Queries,
        member: None,
        window_s: None,
        queries: Some(p),
    }
}

#[tokio::test]
async fn top_since_reset_on_perf() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &r.data else {
        panic!("не queries: {:?}", r.data)
    };
    let pgss = d.pgss.as_ref().expect("pgss установлен в perf");
    assert!(pgss.version.starts_with("1."), "{}", pgss.version);
    assert!(pgss.track_planning && pgss.max == 5000);
    assert!(
        pgss.stats_reset_ms.is_some() && pgss.dealloc.is_some(),
        "pgss 1.9+: info есть"
    );
    assert!(!d.rows.is_empty() && d.totals.calls > 0);
    assert!(
        d.rows
            .windows(2)
            .all(|w| w[0].total_exec_ms >= w[1].total_exec_ms),
        "по убыванию времени"
    );
    // Сэмплер pgss не запущен: кеша текстов у узла нет, строки без текстов (они подгружаются по ключам).
    assert!(
        d.rows
            .iter()
            .all(|x| x.key.toplevel && x.datname.is_some() && x.text.is_none())
    );
    assert!(
        r.unavailable
            .iter()
            .all(|u| u.part != "statements" && u.part != "others"),
        "{:?}",
        r.unavailable
    );
    // Тексты — отдельной командой по ключам.
    let keys: Vec<_> = d.rows.iter().take(3).map(|x| x.key).collect();
    let texts = queries::texts(&st, "it", None, keys.clone()).await.unwrap();
    assert_eq!(texts.len(), 3);
    assert!(
        texts
            .iter()
            .all(|t| keys.contains(&t.key) && t.text.as_deref().is_some_and(|s| !s.is_empty()))
    );
    // Сортировка по hit ratio — по возрастанию, без чтений — в конце.
    let r = diag::run(&st, "it", request(since_reset(QuerySort::HitRatio)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    let ratios: Vec<_> = d.rows.iter().map(|x| x.hit_ratio).collect();
    let known: Vec<f64> = ratios.iter().flatten().copied().collect();
    assert!(known.windows(2).all(|w| w[0] <= w[1]), "{ratios:?}");
    assert!(
        ratios
            .iter()
            .skip_while(|r| r.is_some())
            .all(|r| r.is_none()),
        "None в конце: {ratios:?}"
    );
}

#[tokio::test]
async fn missing_extension_is_unavailable_not_an_error() {
    let Some(single) = common::env("GPG_IT_SINGLE") else {
        return;
    };
    let (_dir, st) = common::open_server(&single).await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(d.rows.is_empty() && d.pgss.is_none());
    assert!(
        r.unavailable.iter().any(|u| u.part == "statements"),
        "{:?}",
        r.unavailable
    );
}

#[tokio::test]
async fn role_without_read_all_stats_sees_hidden_rows() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) =
        common::open_server_as(&perf, "perf", common::APP_USER, common::APP_PASSWORD).await;
    let r = diag::run(&st, "it", request(since_reset(QuerySort::TotalTime)))
        .await
        .unwrap();
    assert!(
        r.unavailable.iter().any(|u| u.part == "others"),
        "{:?}",
        r.unavailable
    );
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(
        d.rows
            .iter()
            .all(|x| x.rolname.as_deref() == Some(common::APP_USER)),
        "видны только свои строки"
    );
}

#[tokio::test]
async fn statements_sql_runs_on_every_version() {
    let mut addrs = common::matrix_addrs();
    addrs.extend(common::perf_addr());
    addrs.extend(common::lab_addr());
    for addr in addrs {
        let db = if common::perf_addr().as_deref() == Some(addr.as_str()) {
            "perf"
        } else {
            "postgres"
        };
        let c = common::connect_as(&addr, db, common::PG_USER, common::PG_PASSWORD_VALUE).await;
        let Detect::Installed(pgss) = queries::detect(&c).await.unwrap() else {
            panic!("{addr}: pgss не найден")
        };
        let version = diag::head(&c).await.unwrap().version_num;
        // Версия расширения растёт вместе с сервером: 13 → 1.8, 14 → 1.9, 15/16 → 1.10, 17 → 1.11, 18 → 1.12.
        let expected = match version / 10_000 {
            13 => "1.8",
            14 => "1.9",
            15 | 16 => "1.10",
            17 => "1.11",
            _ => "1.12",
        };
        assert_eq!(pgss.version, expected, "{addr}");
        let snap = queries::read_cumulative(&c, &pgss)
            .await
            .unwrap_or_else(|e| panic!("{addr}: {e:?}"));
        assert!(
            !snap.rows.is_empty(),
            "{addr}: хотя бы запрос самого detect"
        );
        assert!(snap.info.max > 0, "{addr}");
        assert_eq!(snap.info.dealloc.is_some(), pgss.caps.info, "{addr}");
        let keys: Vec<_> = snap.rows.iter().take(2).map(|r| r.key).collect();
        let texts = c
            .query(
                &queries::texts_sql(&pgss.schema, pgss.caps),
                &[&keys.iter().map(|k| k.query_id).collect::<Vec<i64>>()],
            )
            .await
            .unwrap();
        assert!(!texts.is_empty(), "{addr}");
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<StatementsEvent>>);
impl SamplerSink<StatementsEvent> for Events {
    fn send(&self, e: StatementsEvent) -> bool {
        self.0.lock().unwrap().push(e);
        true
    }
}

/// Сэмплер pgss на pg-perf: первый тик — база, второй (через минуту) — дельты; окно считается из буфера без
/// сервера; тексты, прочитанные по ключам, попадают в кеш и возвращаются в строках отчёта.
#[tokio::test]
async fn statements_sampler_fills_the_window_and_caches_texts() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let events = Arc::new(Events::default());
    let sid = statements::start(&st, "it", None, events.clone()).unwrap();
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    common::wait_until(
        "первый тик сэмплера pgss",
        Duration::from_secs(20),
        async || {
            events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, StatementsEvent::Tick { ticks: 0, .. }))
        },
    )
    .await;
    // Окно до второго тика: пусто с пометкой.
    let window = QueriesParams {
        mode: QueriesMode::Window { s: 300 },
        ..since_reset(QuerySort::TotalTime)
    };
    let r = diag::run(&st, "it", request(window.clone())).await.unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(d.rows.is_empty() && d.coverage_s.is_none());
    assert!(r.unavailable.iter().any(|u| u.part == "window"));
    // Тексты по ключам — в кеш узла.
    let since = diag::run(&st, "it", request(since_reset(QuerySort::Calls)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &since.data else {
        unreachable!()
    };
    let keys: Vec<_> = d.rows.iter().take(2).map(|x| x.key).collect();
    queries::texts(&st, "it", None, keys.clone()).await.unwrap();
    let again = diag::run(&st, "it", request(since_reset(QuerySort::Calls)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &again.data else {
        unreachable!()
    };
    assert!(
        d.rows
            .iter()
            .filter(|x| keys.contains(&x.key))
            .all(|x| x.text.is_some()),
        "тексты из кеша"
    );
    // Нагрузка между тиками, затем второй тик (до 75 с).
    let app = common::superuser(&perf, "perf").await;
    for _ in 0..50 {
        app.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'disputed'")
            .await
            .unwrap();
    }
    common::wait_until(
        "второй тик сэмплера pgss",
        Duration::from_secs(75),
        async || {
            events
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, StatementsEvent::Tick { ticks: 1, .. }))
        },
    )
    .await;
    let r = diag::run(&st, "it", request(window)).await.unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert_eq!(d.ticks, Some(1));
    assert!(
        d.coverage_s.is_some_and(|c| (60..=300).contains(&c)),
        "{:?}",
        d.coverage_s
    );
    assert!(
        d.rows.iter().any(|x| x.calls >= 50),
        "дельты за минуту видны: {:?}",
        d.rows.iter().map(|x| x.calls).collect::<Vec<_>>()
    );
    assert!(
        d.rows.iter().all(|x| x.stddev_exec_ms.is_none()),
        "за окно stddev нет"
    );
    assert!(d.pgss.is_some() && r.server_version_num.is_some());
    assert!(st.statements.is_running("it", &member));
    assert!(st.sessions.stop(&sid));
    assert!(
        !st.statements.is_running("it", &member),
        "последний подписчик — задача снята"
    );
    assert!(
        st.statements.buffer("it", &member).is_some(),
        "буфер остаётся"
    );
}

/// Текст карточки — из кеша текстов узла (буфер сэмплера pgss): первый вызов читает его с сервера и кладёт в кеш,
/// следующий берёт из кеша и `pg_stat_statements(true)` не читает (подложенный в кеш текст возвращается как есть).
#[tokio::test]
async fn query_detail_text_goes_through_the_sampler_cache() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let events = Arc::new(Events::default());
    let sid = statements::start(&st, "it", None, events.clone()).unwrap();
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let r = diag::run(&st, "it", request(since_reset(QuerySort::Calls)))
        .await
        .unwrap();
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    let query_id = d.rows[0].key.query_id;
    let buffer = st.statements.buffer("it", &member).expect("буфер узла");
    let first = query_detail::detail(&st, "it", &member, "perf", query_id)
        .await
        .unwrap();
    let text = first.text.clone().expect("текст прочитан с сервера");
    let keys: Vec<_> = first.by_user.iter().map(|u| u.stat.key).collect();
    assert!(!keys.is_empty());
    assert!(
        keys.iter().any(|k| buffer
            .lock()
            .unwrap()
            .text(k)
            .is_some_and(|(t, _)| *t == *text)),
        "прочитанный текст — в кеше узла"
    );
    buffer
        .lock()
        .unwrap()
        .remember_texts(keys.iter().map(|k| (*k, "-- из кеша".to_string(), false)));
    let second = query_detail::detail(&st, "it", &member, "perf", query_id)
        .await
        .unwrap();
    assert_eq!(
        second.text.as_deref(),
        Some("-- из кеша"),
        "второй вызов берёт текст из кеша, а не с сервера"
    );
    assert!(
        second.since_reset.is_some(),
        "статистика читается как прежде"
    );
    assert!(st.sessions.stop(&sid));
}

/// Карточка запроса на pg-perf (PG 17: экземпляры читаются) и на PG 13 (экземпляров нет — пометка).
#[tokio::test]
async fn query_detail_on_perf_and_on_pg13() {
    if let Some(perf) = common::perf_addr() {
        let (_dir, st) = common::open_server_in(&perf, "perf").await;
        let member = st.runtimes.view("it").topology.unwrap().members[0]
            .name
            .clone();
        let r = diag::run(&st, "it", request(since_reset(QuerySort::Calls)))
            .await
            .unwrap();
        let DiagData::Queries(d) = &r.data else {
            unreachable!()
        };
        let top = &d.rows[0];
        let detail = query_detail::detail(&st, "it", &member, "perf", top.key.query_id)
            .await
            .unwrap();
        assert!(detail.text.as_deref().is_some_and(|t| !t.is_empty()));
        let since = detail.since_reset.as_ref().expect("строка pgss есть");
        assert!(
            since.calls >= top.calls,
            "карточка суммирует роли и toplevel"
        );
        assert!(since.stddev_exec_ms.is_some());
        assert!(!detail.by_user.is_empty() && detail.totals.calls >= since.calls);
        assert_eq!(
            detail
                .windows
                .iter()
                .map(|w| w.window_s)
                .collect::<Vec<_>>(),
            [300, 900, 3600]
        );
        assert!(
            detail.unavailable.iter().all(|u| u.part != "running"),
            "PG 17: экземпляры читаются"
        );
        assert!(detail.server_version_num >= 170_000 && !detail.in_recovery);
        assert_eq!(
            detail.pgss_version.as_deref().map(|v| v.starts_with("1.")),
            Some(true)
        );
        // Запроса нет в pgss — карточка без текста и статистики, но без ошибки.
        let gone = query_detail::detail(&st, "it", &member, "perf", 1)
            .await
            .unwrap();
        assert!(gone.text.is_none() && gone.since_reset.is_none() && gone.by_user.is_empty());
    }
    if let Some(pg13) = common::matrix_addrs()
        .into_iter()
        .find(|a| a.ends_with(":5513"))
    {
        let (_dir, st) = common::open_server(&pg13).await;
        let member = st.runtimes.view("it").topology.unwrap().members[0]
            .name
            .clone();
        let detail = query_detail::detail(&st, "it", &member, "postgres", 1)
            .await
            .unwrap();
        assert!(
            detail.unavailable.iter().any(|u| u.part == "running"),
            "{:?}",
            detail.unavailable
        );
    }
}

/// Выполняющийся экземпляр виден в карточке по `query_id` сессии; текст, упёршийся в `track_activity_query_size`
/// (многобайтный — длина считается в байтах), помечается обрезанным. Простаивающая сессия, чья последняя инструкция
/// имела тот же `query_id` (он сохраняется в `pg_stat_activity`), экземпляром не считается.
#[tokio::test]
async fn query_detail_lists_running_instances() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (_dir, st) = common::open_server_in(&perf, "perf").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let probe = common::superuser(&perf, "perf").await;
    // Соединение остаётся открытым и простаивает: его последняя инструкция имеет тот же нормализованный текст.
    let idle = common::superuser(&perf, "perf").await;
    let idle_pid: i32 = idle
        .query_one("SELECT pg_backend_pid()", &[])
        .await
        .unwrap()
        .get(0);
    idle.batch_execute("SELECT pg_sleep(0), '1'").await.unwrap();
    for (literal, truncated) in [("42".to_string(), false), ("я".repeat(800), true)] {
        let sleeper = common::superuser(&perf, "perf").await;
        let pid: i32 = sleeper
            .query_one("SELECT pg_backend_pid()", &[])
            .await
            .unwrap()
            .get(0);
        let sql = format!("SELECT pg_sleep(3), '{literal}'");
        let job = tokio::spawn(async move { sleeper.batch_execute(&sql).await });
        common::wait_until("запрос сессии выполняется", Duration::from_secs(5), async || {
            probe
                .query_opt(
                    "SELECT 1 FROM pg_stat_activity WHERE pid = $1 AND state = 'active' AND query_id IS NOT NULL",
                    &[&pid],
                )
                .await
                .unwrap()
                .is_some()
        })
        .await;
        let query_id: i64 = probe
            .query_one(
                "SELECT query_id FROM pg_stat_activity WHERE pid = $1",
                &[&pid],
            )
            .await
            .unwrap()
            .get(0);
        // Предпосылка: простаивающая сессия действительно несёт тот же `query_id` — иначе проверка фильтра пуста.
        let idle_row = probe
            .query_one(
                "SELECT state, query_id FROM pg_stat_activity WHERE pid = $1",
                &[&idle_pid],
            )
            .await
            .unwrap();
        assert_eq!(
            (
                idle_row.get::<_, String>(0),
                idle_row.get::<_, Option<i64>>(1)
            ),
            ("idle".to_string(), Some(query_id)),
            "простаивающая сессия хранит query_id последней инструкции"
        );
        let detail = query_detail::detail(&st, "it", &member, "perf", query_id)
            .await
            .unwrap();
        assert_eq!(
            detail.running.len(),
            1,
            "простаивающая сессия {idle_pid} не экземпляр: {:?}",
            detail.running
        );
        let r = &detail.running[0];
        assert_eq!((r.pid, r.state.as_deref()), (pid, Some("active")));
        assert!(r.started_ms.is_some() && r.duration_s.is_some() && r.backend_start_us > 0);
        assert_eq!(
            r.truncated,
            truncated,
            "длина текста — {} символов",
            r.query.chars().count()
        );
        job.await.unwrap().unwrap();
    }
}
