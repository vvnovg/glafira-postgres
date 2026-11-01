//! Снимки pgss на pg-perf (`GPG_IT_PERF`): снятие, список, сравнение, режим «между снимками». `--test-threads=1`.

mod common;

use std::time::Duration;

use glafira_pg_lib::diag::queries::{QueriesMode, QueriesParams};
use glafira_pg_lib::diag::{self, DiagData, DiagRequest, Tool};
use glafira_pg_lib::snapshots;
use pg_advisor::queries::QuerySort;

#[tokio::test]
async fn take_compare_and_between_mode() {
    let Some(perf) = common::perf_addr() else {
        return;
    };
    let (dir, st) = common::open_server_in(&perf, "perf").await;
    let member = st.runtimes.view("it").topology.unwrap().members[0]
        .name
        .clone();
    let a = snapshots::take(&st, "it", &member).await.unwrap();
    assert!(
        a.rows.is_some_and(|r| r > 0)
            && a.unreadable.is_none()
            && a.bytes > 0
            && a.file.starts_with(&format!("{member}/"))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("snapshots/it").join(&a.file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let su = common::superuser(&perf, "perf").await;
    for _ in 0..30 {
        su.batch_execute("SELECT count(*) FROM shop.orders WHERE status = 'disputed'")
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(1100)).await; // имя файла — до секунды
    let b = snapshots::take(&st, "it", &member).await.unwrap();
    assert_ne!(a.file, b.file);
    let list = st.snapshots.list("it").unwrap();
    assert_eq!(
        list.iter().map(|i| i.file.as_str()).collect::<Vec<_>>(),
        [b.file.as_str(), a.file.as_str()],
        "новые первыми"
    );
    let cmp = snapshots::compare(&st, "it", &a.file, &b.file)
        .await
        .unwrap();
    assert!(cmp.totals_to.calls > cmp.totals_from.calls);
    assert!(!cmp.reset);
    let params = |search: Option<&str>| QueriesParams {
        mode: QueriesMode::Between {
            from: a.file.clone(),
            to: b.file.clone(),
        },
        sort: QuerySort::Calls,
        limit: 50,
        database: None,
        toplevel_only: true,
        search: search.map(str::to_string),
    };
    let run = |p: QueriesParams| {
        let st = st.clone();
        let member = member.clone();
        async move {
            diag::run(
                &st,
                "it",
                DiagRequest {
                    tool: Tool::Queries,
                    member: Some(member),
                    window_s: None,
                    queries: Some(p),
                },
            )
            .await
            .unwrap()
        }
    };
    let r = run(params(None)).await;
    assert_eq!(r.member.as_deref(), Some(member.as_str()));
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(
        d.rows.iter().any(|x| x.calls >= 30),
        "{:?}",
        d.rows.iter().map(|x| x.calls).collect::<Vec<_>>()
    );
    assert!(d.coverage_s.is_some_and(|c| c >= 1));
    assert_eq!(d.ticks, None);
    assert!(d.rows.iter().all(|x| x.stddev_exec_ms.is_none()));
    assert!(
        d.rows.iter().any(|x| x.text.is_some()),
        "тексты верхних запросов — из снимка"
    );
    assert!(
        d.rows.iter().any(|x| x.own),
        "снятие снимка — свой запрос Глафиры: {:?}",
        d.rows.iter().map(|x| (&x.text, x.own)).collect::<Vec<_>>()
    );
    assert!(r.unavailable.is_empty(), "{:?}", r.unavailable);
    let pgss = d.pgss.as_ref().unwrap();
    assert!(pgss.max > 0, "предел pgss хранится в снимке");
    // pg_stat_kcache на pg-perf: снимки хранят CPU и диск, между снимками — дельты и сортировка по CPU.
    assert!(pgss.kcache, "kcache в обоих снимках");
    assert!(st.snapshots.read("it", &b.file).unwrap().kcache);
    let mut p = params(None);
    p.sort = QuerySort::CpuTime;
    let r = run(p).await;
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(
        d.rows.iter().any(|x| x.cpu_user_ms.is_some()),
        "CPU между снимками"
    );
    assert!(
        d.rows
            .windows(2)
            .all(|w| QuerySort::CpuTime.value(&w[0]) >= QuerySort::CpuTime.value(&w[1]))
    );
    // Поиск — по текстам снимков.
    let r = run(params(Some("STATUS = $1"))).await;
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(!d.rows.is_empty());
    assert!(d.rows.iter().all(|x| {
        x.text
            .as_deref()
            .is_some_and(|t| t.to_lowercase().contains("status = $1"))
    }));
    // Подключение закрыто — режим «между снимками» читает только файлы; пара — в любом порядке.
    st.runtimes.disconnect("it");
    let mut p = params(None);
    p.mode = QueriesMode::Between {
        from: b.file.clone(),
        to: a.file.clone(),
    };
    let r = run(p).await;
    let DiagData::Queries(d) = &r.data else {
        unreachable!()
    };
    assert!(d.rows.iter().any(|x| x.calls >= 30));
    assert_eq!(r.member.as_deref(), Some(member.as_str()));
    // min / max / stddev между снимками нет.
    let mut p = params(None);
    p.sort = QuerySort::StddevTime;
    let e = diag::run(
        &st,
        "it",
        DiagRequest {
            tool: Tool::Queries,
            member: Some(member.clone()),
            window_s: None,
            queries: Some(p),
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e.key, "errors.queries.sortUnavailable");
    assert!(
        st.snapshots.delete("it", &a.file).unwrap() && st.snapshots.delete("it", &b.file).unwrap()
    );
    assert!(st.snapshots.list("it").unwrap().is_empty());
    assert_eq!(
        snapshots::compare(&st, "it", &a.file, &b.file)
            .await
            .unwrap_err()
            .key,
        "errors.snapshots.notFound"
    );
}
