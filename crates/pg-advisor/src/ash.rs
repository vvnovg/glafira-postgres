//! Профиль ожиданий (спец. P2 §5.3): агрегация сэмплов активности в средние активные сессии и топы.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// Класс ожидания по `wait_event_type`. Ключ карты `Bucket::aas` в JSON — имя класса (`cpu`, `bufferPin`…).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum WaitClass {
    #[default]
    Cpu,
    Lock,
    Lwlock,
    Io,
    Ipc,
    Client,
    Timeout,
    BufferPin,
    Extension,
    Activity,
    Other,
}

impl WaitClass {
    /// Класс по `wait_event_type`; активная сессия без ожидания — `Cpu` (на процессоре или в неинструментированном ожидании).
    pub fn of(wait_type: Option<&str>) -> Self {
        match wait_type {
            None => Self::Cpu,
            Some("Lock") => Self::Lock,
            Some("LWLock") => Self::Lwlock,
            Some("IO") => Self::Io,
            Some("IPC") => Self::Ipc,
            Some("Client") => Self::Client,
            Some("Timeout") => Self::Timeout,
            Some("BufferPin") => Self::BufferPin,
            Some("Extension") => Self::Extension,
            Some("Activity") => Self::Activity,
            Some(_) => Self::Other,
        }
    }
}

/// Строка сэмпла: активная сессия в момент тика `at_ms` (часы приложения). Строки — ссылки в словарь буфера;
/// у параллельного воркера текст и `query_id` — запроса лидера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample<'a> {
    pub at_ms: i64,
    pub pid: i32,
    pub leader_pid: Option<i32>,
    pub wait_type: Option<&'a str>,
    pub wait_event: Option<&'a str>,
    pub query_id: Option<i64>,
    pub query: &'a str,
    pub application: &'a str,
    pub user: &'a str,
    pub database: &'a str,
    pub client: &'a str,
}

/// Интервал графика: `sampled_s` — сколько секунд в нём работал сэмплер (тики), `aas` — среднее число активных
/// сессий по классам ожидания.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub start_ms: i64,
    pub sampled_s: f64,
    pub aas: BTreeMap<WaitClass, f64>,
}

/// Промежуток в истории: сэмплер не работал (вкладка была закрыта, запрос не успевал или падал).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Gap {
    pub from_ms: i64,
    pub to_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopEvent {
    pub class: WaitClass,
    /// `wait_event`; у `cpu` пусто.
    pub event: Option<String>,
    pub samples: u64,
    pub share: f64,
}

/// Запрос в топе: ключ — `id:<query_id>`, если он есть и не 0, иначе `text:<хэш текста>` (на PG 13 — всегда хэш).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopQuery {
    pub key: String,
    pub query_id: Option<i64>,
    pub text: String,
    pub samples: u64,
    pub share: f64,
    /// Самое частое ожидание запроса: `Тип:событие`, `Тип` или `CPU*`.
    pub top_event: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TopName {
    pub name: String,
    pub samples: u64,
    pub share: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitsData {
    pub window_s: u32,
    pub bucket_s: u32,
    /// Сэмплер узла сейчас работает (подписана вкладка); иначе в отчёте то, что накоплено.
    pub sampler_running: bool,
    /// Окно длиннее накопленной истории.
    pub truncated: bool,
    pub total_samples: u64,
    pub buckets: Vec<Bucket>,
    pub gaps: Vec<Gap>,
    pub top_events: Vec<TopEvent>,
    pub top_queries: Vec<TopQuery>,
    pub by_application: Vec<TopName>,
    pub by_user: Vec<TopName>,
    pub by_database: Vec<TopName>,
    pub by_client: Vec<TopName>,
}

/// Длина интервала графика для окна: 5 с для 300, 10 с для 900, 40 с для 3600.
pub fn bucket_s(window_s: u32) -> u32 {
    match window_s {
        300 => 5,
        900 => 10,
        _ => 40,
    }
}

fn event_label(wait_type: Option<&str>, wait_event: Option<&str>) -> String {
    match (wait_type, wait_event) {
        (Some(t), Some(e)) => format!("{t}:{e}"),
        (Some(t), None) => t.to_string(),
        _ => "CPU*".into(),
    }
}

/// Ключ запроса в топе: `query_id`, если он есть и не 0, иначе текст.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum QueryKey<'s> {
    Id(i64),
    Text(&'s str),
}

impl QueryKey<'_> {
    /// Ключ в ответе: у текста — хэш, чтобы не дублировать текст в JSON.
    fn render(self) -> String {
        match self {
            Self::Id(id) => format!("id:{id}"),
            Self::Text(t) => format!("text:{:016x}", hash64(&format!("text:{t}"))),
        }
    }
}

/// Счётчик запроса: `query_id`, текст первого сэмпла, число сэмплов, ожидания.
type QueryAcc<'s> = (
    Option<i64>,
    &'s str,
    u64,
    HashMap<(Option<&'s str>, Option<&'s str>), u64>,
);

/// Топ-10 имён (приложение, роль, БД, клиент) по числу сэмплов; строки ответа строятся только для десяти.
fn top_names(m: HashMap<&str, u64>, total: u64) -> Vec<TopName> {
    let mut v: Vec<(&str, u64)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.truncate(10);
    v.into_iter()
        .map(|(n, c)| TopName {
            name: n.to_string(),
            samples: c,
            share: c as f64 / total as f64,
        })
        .collect()
}

/// Профиль ожиданий за окно `window_s`, заканчивающееся в `now_ms`. Среднее число активных сессий интервала —
/// сэмплы интервала, делённые на секунды, когда сэмплер работал (тики). Параллельные воркеры несут текст и
/// `query_id` запроса лидера, поэтому учитываются за его запросом. Пустое окно даёт нули без деления на ноль.
/// Один проход по сэмплам без копий и строк на сэмпл: в буфере до 500 000 строк.
pub fn profile(
    samples: &[Sample],
    ticks: &[i64],
    gaps: &[(i64, i64)],
    window_s: u32,
    now_ms: i64,
    oldest_tick_ms: Option<i64>,
    sampler_running: bool,
) -> WaitsData {
    let bucket = bucket_s(window_s);
    let (bucket_ms, from) = (bucket as i64 * 1000, now_ms - window_s as i64 * 1000);
    // Окно короче интервала невозможно (окна — `WINDOWS_S`), но и тогда интервал есть хотя бы один.
    let n = ((window_s / bucket) as usize).max(1);
    let mut buckets: Vec<Bucket> = (0..n)
        .map(|i| Bucket {
            start_ms: from + i as i64 * bucket_ms,
            sampled_s: 0.0,
            aas: BTreeMap::new(),
        })
        .collect();
    let index = |at: i64| (((at - from) / bucket_ms) as usize).min(n - 1);
    for t in ticks.iter().filter(|t| **t >= from && **t < now_ms) {
        buckets[index(*t)].sampled_s += 1.0;
    }
    let mut total = 0u64;
    let mut events: HashMap<(WaitClass, Option<&str>), u64> = HashMap::new();
    let mut queries: HashMap<QueryKey, QueryAcc> = HashMap::new();
    let mut names: [HashMap<&str, u64>; 4] = Default::default();
    for s in samples
        .iter()
        .filter(|s| s.at_ms >= from && s.at_ms < now_ms)
    {
        total += 1;
        let class = WaitClass::of(s.wait_type);
        *buckets[index(s.at_ms)].aas.entry(class).or_default() += 1.0;
        *events.entry((class, s.wait_event)).or_default() += 1;
        let key = match s.query_id {
            Some(id) if id != 0 => QueryKey::Id(id),
            _ => QueryKey::Text(s.query),
        };
        let q = queries.entry(key).or_insert((
            s.query_id.filter(|i| *i != 0),
            s.query,
            0,
            HashMap::new(),
        ));
        q.2 += 1;
        *q.3.entry((s.wait_type, s.wait_event)).or_default() += 1;
        for (m, name) in names
            .iter_mut()
            .zip([s.application, s.user, s.database, s.client])
        {
            *m.entry(name).or_default() += 1;
        }
    }
    for b in &mut buckets {
        if b.sampled_s > 0.0 {
            for v in b.aas.values_mut() {
                *v /= b.sampled_s;
            }
        }
    }
    let t = total.max(1);
    // Топ ожиданий.
    let mut top_events: Vec<TopEvent> = events
        .into_iter()
        .map(|((class, e), c)| TopEvent {
            class,
            event: e.map(str::to_string),
            samples: c,
            share: c as f64 / t as f64,
        })
        .collect();
    top_events.sort_by(|a, b| {
        b.samples
            .cmp(&a.samples)
            .then(a.class.cmp(&b.class))
            .then(a.event.cmp(&b.event))
    });
    top_events.truncate(10);
    // Топ запросов: порядок — число сэмплов, затем ключ ответа; тексты копируются только для десяти.
    let mut ranked: Vec<(String, QueryAcc)> = queries
        .into_iter()
        .map(|(k, acc)| (k.render(), acc))
        .collect();
    ranked.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(&b.0)));
    ranked.truncate(10);
    let top_queries: Vec<TopQuery> = ranked
        .into_iter()
        .map(|(key, (qid, text, c, ev))| TopQuery {
            key,
            query_id: qid,
            text: text.to_string(),
            samples: c,
            share: c as f64 / t as f64,
            top_event: ev
                .into_iter()
                .map(|((wt, we), c)| (event_label(wt, we), c))
                .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
                .map(|(k, _)| k),
        })
        .collect();
    let [by_application, by_user, by_database, by_client] = names.map(|m| top_names(m, t));
    WaitsData {
        window_s,
        bucket_s: bucket,
        sampler_running,
        // История короче окна: первый тик позже начала окна больше чем на интервал.
        truncated: oldest_tick_ms.is_none_or(|o| o > from + bucket_ms),
        total_samples: total,
        gaps: gaps
            .iter()
            .filter(|(_, to)| *to > from)
            .map(|(a, b)| Gap {
                from_ms: *a,
                to_ms: *b,
            })
            .collect(),
        top_events,
        top_queries,
        by_application,
        by_user,
        by_database,
        by_client,
        buckets,
    }
}

/// Хэш текста запроса для ключа топа. `DefaultHasher::new()` не случаен (ключи SipHash постоянные): в пределах
/// сборки приложения ключ строки один и тот же от отчёта к отчёту.
fn hash64(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(
        at_ms: i64,
        wait_type: Option<&'static str>,
        wait_event: Option<&'static str>,
        query_id: Option<i64>,
        query: &'static str,
    ) -> Sample<'static> {
        Sample {
            at_ms,
            pid: 1,
            leader_pid: None,
            wait_type,
            wait_event,
            query_id,
            query,
            application: "api",
            user: "app",
            database: "orders",
            client: "10.0.0.1",
        }
    }

    #[test]
    fn buckets_count_active_sessions_per_sampled_second() {
        let now = 300_000;
        let ticks: Vec<i64> = (0..300).map(|i| i * 1000 + 500).collect();
        // Две сессии каждую секунду первого интервала (5 с): одна на CPU, одна ждёт блокировку.
        let mut samples = Vec::new();
        for t in &ticks[..5] {
            samples.push(s(*t, None, None, Some(7), "select 1"));
            samples.push(s(
                *t,
                Some("Lock"),
                Some("transactionid"),
                Some(8),
                "update t",
            ));
        }
        let d = profile(&samples, &ticks, &[], 300, now, Some(500), true);
        assert_eq!((d.bucket_s, d.buckets.len(), d.total_samples), (5, 60, 10));
        assert_eq!(d.buckets[0].sampled_s, 5.0);
        assert_eq!(d.buckets[0].aas[&WaitClass::Cpu], 1.0);
        assert_eq!(d.buckets[0].aas[&WaitClass::Lock], 1.0);
        assert!(d.buckets[1].aas.is_empty());
        assert_eq!(d.top_events[0].share, 0.5);
        assert_eq!(d.top_queries.len(), 2);
        assert_eq!(
            d.top_queries
                .iter()
                .find(|q| q.query_id == Some(8))
                .unwrap()
                .top_event
                .as_deref(),
            Some("Lock:transactionid")
        );
        assert_eq!(d.by_application[0].name, "api");
        assert!(!d.truncated);
    }

    #[test]
    fn window_longer_than_history_is_truncated_and_text_keys_without_query_id() {
        let d = profile(
            &[
                s(10_000, None, None, None, "select 2"),
                s(11_000, None, None, Some(0), "select 2"),
            ],
            &[10_000, 11_000],
            &[(5_000, 9_000)],
            900,
            12_000,
            Some(10_000),
            false,
        );
        assert!(d.truncated && !d.sampler_running);
        assert_eq!(
            d.gaps,
            [Gap {
                from_ms: 5_000,
                to_ms: 9_000
            }]
        );
        assert_eq!(
            d.top_queries.len(),
            1,
            "query_id 0 и пустой — ключ по тексту"
        );
        assert_eq!(WaitClass::of(Some("LWLock")), WaitClass::Lwlock);
        assert_eq!(WaitClass::of(Some("InjectionPoint")), WaitClass::Other);
    }

    #[test]
    fn tops_keep_ten_rows_by_samples_and_workers_count_for_the_leader_query() {
        let texts: Vec<String> = (0..12).map(|i| format!("select {i}")).collect();
        let mut v = Vec::new();
        for (i, t) in texts.iter().enumerate() {
            for _ in 0..=i {
                v.push(Sample {
                    query: t,
                    client: t,
                    ..s(1_000, Some("IO"), Some("DataFileRead"), None, "")
                });
            }
        }
        // Лидер и воркер одного параллельного запроса: тот же query_id, у воркера — `leader_pid`.
        v.push(s(1_000, None, None, Some(42), "select big"));
        v.push(Sample {
            pid: 2,
            leader_pid: Some(1),
            ..s(
                1_000,
                Some("IPC"),
                Some("BgWorkerShutdown"),
                Some(42),
                "select big",
            )
        });
        let d = profile(&v, &[1_000], &[], 300, 2_000, Some(1_000), true);
        assert_eq!(d.total_samples, 80);
        assert_eq!(d.top_queries.len(), 10);
        assert_eq!(
            (d.top_queries[0].text.as_str(), d.top_queries[0].samples),
            ("select 11", 12)
        );
        assert!(d.top_queries[0].key.starts_with("text:") && d.top_queries[0].key.len() == 21);
        assert_eq!(
            d.top_queries[0].top_event.as_deref(),
            Some("IO:DataFileRead")
        );
        assert_eq!(d.top_queries[9].samples, 3);
        assert!(
            d.top_queries.iter().all(|q| q.query_id != Some(42)),
            "2 сэмпла — вне топа"
        );
        assert_eq!(
            (d.by_client.len(), d.by_client[0].name.as_str()),
            (10, "select 11")
        );
        assert_eq!(
            d.by_user,
            [TopName {
                name: "app".into(),
                samples: 80,
                share: 1.0
            }]
        );
        assert_eq!(
            (d.buckets[59].sampled_s, d.buckets[59].aas[&WaitClass::Io]),
            (1.0, 78.0),
            "последний интервал окна — самый свежий"
        );

        let big = profile(&v[78..], &[1_000], &[], 300, 2_000, Some(1_000), true);
        assert_eq!(big.top_queries.len(), 1, "воркер — за запросом лидера");
        assert_eq!(
            (big.top_queries[0].key.as_str(), big.top_queries[0].samples),
            ("id:42", 2)
        );
        assert_eq!(
            big.top_queries[0].top_event.as_deref(),
            Some("CPU*"),
            "равные счёты — меньшая подпись"
        );
    }

    #[test]
    fn empty_window_has_zeros_and_json_keys_are_class_names() {
        let d = profile(&[], &[], &[], 3600, 1_000_000, None, false);
        assert_eq!((d.bucket_s, d.buckets.len(), d.total_samples), (40, 90, 0));
        assert!(d.truncated && d.top_events.is_empty() && d.by_client.is_empty());
        let b = Bucket {
            start_ms: 1,
            sampled_s: 2.0,
            aas: BTreeMap::from([(WaitClass::BufferPin, 0.5), (WaitClass::Lwlock, 1.0)]),
        };
        let v = serde_json::to_value(&b).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"startMs": 1, "sampledS": 2.0, "aas": {"bufferPin": 0.5, "lwlock": 1.0}})
        );
        assert_eq!(serde_json::from_value::<Bucket>(v).unwrap(), b);
        let w = serde_json::to_value(WaitsData::default()).unwrap();
        assert_eq!(
            (&w["windowS"], &w["samplerRunning"], &w["byApplication"]),
            (&0.into(), &false.into(), &serde_json::json!([]))
        );
    }
}
