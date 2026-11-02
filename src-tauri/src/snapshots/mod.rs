//! Снимки `pg_stat_statements` (спец. P3 §9.3): файлы в каталоге данных пользователя, снятие, сравнение.

pub mod store;

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use pg_advisor::queries::compare::Comparison;
use pg_advisor::queries::{Cumulative, QueryKey, QueryStat};
use serde::{Deserialize, Serialize};

use crate::actions::journal::RecordConnection;
use crate::conn::errors::from_pg;
use crate::diag::kcache::KcacheNode;
use crate::diag::queries::{Detect, detect, read_cumulative_on, texts_sql};
use crate::diag::{head, on_member};
use crate::error::{AppError, AppResult, ErrorCode};
use crate::sampler::statements::{pgss_missing, pgss_too_old};
use crate::state::AppState;
pub use store::{SnapshotInfo, SnapshotStore, Unreadable, file_name, member_dir, valid_file};

/// Текстов в снимке — у верхних запросов по общему времени и, отдельно, по числу вызовов (по ним узнаются частые
/// собственные запросы Глафиры и кандидаты N+1).
pub const TEXTS_IN_SNAPSHOT: usize = 500;

/// Версия формата файла снимка.
pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub v: u32,
    /// Время сервера (`now()`) в момент снятия.
    pub taken_at_ms: i64,
    pub connection: RecordConnection,
    pub member: String,
    pub server_version_num: i32,
    pub in_recovery: bool,
    pub pgss_version: String,
    pub stats_reset_ms: Option<i64>,
    pub dealloc: Option<i64>,
    /// `pg_stat_statements.max` (0 — неизвестен).
    pub max: i64,
    pub track_planning: bool,
    /// Были строки со скрытым `queryid` (чужие без `pg_read_all_stats`).
    pub hidden: bool,
    /// При снятии был `pg_stat_kcache`: в счётчиках строк есть CPU и диск. Поле добавлено без смены `v` (файлы без
    /// него читаются как «без kcache», старые версии приложения поле и колонки kcache пропускают).
    #[serde(default)]
    pub kcache: bool,
    /// Число строк — для списка без разбора `rows` (`store::Header`); выставляется при записи.
    #[serde(default)]
    pub row_count: usize,
    /// Все строки pgss накопительно, без текстов.
    pub rows: Vec<Cumulative>,
    /// `queryId` → текст (не длиннее `QUERY_LIMIT`) для верхних запросов (`TEXTS_IN_SNAPSHOT`).
    pub texts: BTreeMap<String, String>,
    /// `queryId` текстов, обрезанных до `QUERY_LIMIT`.
    pub truncated: BTreeSet<String>,
}

impl Snapshot {
    /// Текст и признак обрезки по `queryId`.
    pub fn text(&self, key: &QueryKey) -> Option<(&str, bool)> {
        let k = key.query_id.to_string();
        self.texts
            .get(&k)
            .map(|t| (t.as_str(), self.truncated.contains(&k)))
    }
}

/// `queryId` для текстов снимка: верхние `TEXTS_IN_SNAPSHOT` по общему времени и по числу вызовов, без повторов.
pub fn text_ids(rows: &[Cumulative]) -> Vec<i64> {
    let mut by_time: Vec<&Cumulative> = rows.iter().collect();
    by_time.sort_by(|a, b| {
        b.counters
            .total_exec_ms
            .total_cmp(&a.counters.total_exec_ms)
    });
    let mut by_calls: Vec<&Cumulative> = rows.iter().collect();
    by_calls.sort_by_key(|c| std::cmp::Reverse(c.counters.calls));
    let mut seen = HashSet::new();
    by_time
        .iter()
        .take(TEXTS_IN_SNAPSHOT)
        .chain(by_calls.iter().take(TEXTS_IN_SNAPSHOT))
        .map(|c| c.key.query_id)
        .filter(|id| seen.insert(*id))
        .collect()
}

/// Снимок узла: строки pgss, состояние и тексты верхних запросов; файл — в каталоге подключения.
pub async fn take(st: &AppState, id: &str, member: &str) -> AppResult<SnapshotInfo> {
    let profile = st.profile(id)?;
    let target = st.runtimes.diag_target(id, Some(member), false)?;
    let kcache = KcacheNode::new(&st.kcache_probes, id, target.generation, member);
    let member = member.to_string();
    let snapshot = on_member(st, id, &target, |c| {
        Box::pin(async move {
            let h = head(c).await?;
            let pgss = match detect(c, Some(&kcache)).await? {
                Detect::Installed(p) => p,
                Detect::Missing => return Err(pgss_missing()),
                Detect::TooOld(v) => return Err(pgss_too_old(&v)),
            };
            let snap = read_cumulative_on(c, &pgss, &kcache).await?;
            let ids = text_ids(&snap.rows);
            let mut texts = BTreeMap::new();
            let mut truncated = BTreeSet::new();
            if !ids.is_empty() {
                let rows = c
                    .query(&texts_sql(&pgss.schema, pgss.caps), &[&ids])
                    .await
                    .map_err(|e| from_pg(&e))?;
                for r in rows {
                    let (Some(qid), Some(text)) =
                        (r.get::<_, Option<i64>>(0), r.get::<_, Option<String>>(4))
                    else {
                        continue;
                    };
                    let k = qid.to_string();
                    if r.get::<_, Option<bool>>(5).unwrap_or(false) {
                        truncated.insert(k.clone());
                    }
                    texts.entry(k).or_insert(text);
                }
            }
            Ok(Snapshot {
                v: SNAPSHOT_VERSION,
                taken_at_ms: h.now_ms,
                connection: RecordConnection::of(&profile),
                member,
                server_version_num: h.version_num,
                in_recovery: h.in_recovery,
                pgss_version: pgss.version.clone(),
                stats_reset_ms: snap.info.stats_reset_ms,
                dealloc: snap.info.dealloc,
                max: snap.info.max,
                track_planning: snap.info.track_planning,
                hidden: snap.hidden,
                kcache: pgss.kcache.is_some(),
                row_count: snap.rows.len(),
                rows: snap.rows,
                texts,
                truncated,
            })
        })
    })
    .await?;
    let store = st.snapshots.clone();
    let cid = id.to_string();
    crate::conn::run_blocking(move || store.write(&cid, &snapshot)).await
}

fn different_members() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.snapshots.differentMembers",
        "Snapshots of different nodes cannot be compared"
    )
}

pub(crate) fn same_snapshot() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.snapshots.samePair",
        "Choose two different snapshots"
    )
}

/// Пара в порядке «старший → младший»: порядок аргументов не важен.
pub fn ordered(a: Snapshot, b: Snapshot) -> (Snapshot, Snapshot) {
    if a.taken_at_ms > b.taken_at_ms {
        (b, a)
    } else {
        (a, b)
    }
}

/// Пара снимков подключения, старший первым (чтение файлов — в блокирующем потоке). Один и тот же файл и снимки
/// разных узлов не сравниваются: счётчики разных узлов независимы.
pub async fn read_pair(
    st: &AppState,
    id: &str,
    from: &str,
    to: &str,
) -> AppResult<(Snapshot, Snapshot)> {
    if from == to {
        return Err(same_snapshot());
    }
    let store = st.snapshots.clone();
    let (cid, from, to) = (id.to_string(), from.to_string(), to.to_string());
    let (a, b) =
        crate::conn::run_blocking(move || Ok((store.read(&cid, &from)?, store.read(&cid, &to)?)))
            .await?;
    if a.member != b.member {
        return Err(different_members());
    }
    Ok(ordered(a, b))
}

/// Текст запроса из любого из двух снимков (сначала `to`) и признак обрезки.
pub fn text_of<'a>(a: &'a Snapshot, b: &'a Snapshot, key: &QueryKey) -> Option<(&'a str, bool)> {
    b.text(key).or_else(|| a.text(key))
}

pub use crate::diag::texts::is_own_text;

/// Текст и признак `own` строке сравнения или отчёта — по текстам снимков.
pub fn annotate(a: &Snapshot, b: &Snapshot, s: &mut QueryStat) {
    if let Some((text, truncated)) = text_of(a, b, &s.key) {
        s.own = is_own_text(text);
        s.text = Some(text.to_string());
        s.text_truncated = truncated;
    }
}

/// Сравнение двух снимков подключения; строки получают тексты из снимков и признак `own` (свои запросы Глафиры
/// остаются в списках — фронтенд приглушает их, как в топе).
pub async fn compare(st: &AppState, id: &str, from: &str, to: &str) -> AppResult<Comparison> {
    let (a, b) = read_pair(st, id, from, to).await?;
    Ok(compare_pair(&a, &b))
}

/// Сравнение пары, старший первым. Колонки `pg_stat_kcache` остаются, только если расширение было при снятии обоих
/// (`kcache_between`), как в режиме «между снимками».
pub fn compare_pair(a: &Snapshot, b: &Snapshot) -> Comparison {
    let strip = |rows: &[Cumulative]| -> Vec<Cumulative> {
        rows.iter()
            .map(|r| Cumulative {
                counters: r.counters.without_kcache(),
                ..r.clone()
            })
            .collect()
    };
    let mut c = if crate::diag::queries::kcache_between(a, b) {
        pg_advisor::queries::compare::compare(&a.rows, a.stats_reset_ms, &b.rows, b.stats_reset_ms)
    } else {
        pg_advisor::queries::compare::compare(
            &strip(&a.rows),
            a.stats_reset_ms,
            &strip(&b.rows),
            b.stats_reset_ms,
        )
    };
    for s in c.new.iter_mut().chain(c.gone.iter_mut()) {
        annotate(a, b, s);
    }
    for ch in c.degraded.iter_mut().chain(c.improved.iter_mut()) {
        annotate(a, b, &mut ch.before);
        annotate(a, b, &mut ch.after);
    }
    c
}

/// Тексты снимков как `Arc<str>` — для `diag::queries::{own_keys, cached_matches}`.
pub fn text_lookup<'a>(
    a: &'a Snapshot,
    b: &'a Snapshot,
) -> impl Fn(&QueryKey) -> Option<Arc<str>> + 'a {
    move |k| text_of(a, b, k).map(|(t, _)| Arc::from(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_advisor::queries::Counters;

    fn row(id: i64, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: QueryKey {
                query_id: id,
                dbid: 1,
                userid: 1,
                toplevel: true,
            },
            datname: None,
            rolname: None,
            counters: Counters {
                calls,
                total_exec_ms: ms,
                ..Counters::default()
            },
            min_exec_ms: 0.0,
            max_exec_ms: 0.0,
            stddev_exec_ms: 0.0,
        }
    }

    #[test]
    fn text_ids_take_the_top_by_time_and_by_calls() {
        let mut rows: Vec<Cumulative> = (0..TEXTS_IN_SNAPSHOT as i64)
            .map(|i| row(i, 1, 1000.0 + i as f64))
            .collect();
        rows.push(row(9_000, 1_000_000, 0.5)); // частый и быстрый — как опрос Глафиры
        rows.push(row(9_001, 0, 0.0));
        let ids = text_ids(&rows);
        assert!(ids.contains(&9_000), "верх по вызовам");
        assert!(ids.contains(&0) && ids.contains(&499), "верх по времени");
        assert!(!ids.contains(&9_001));
        assert_eq!(
            ids.len(),
            HashSet::<i64>::from_iter(ids.iter().copied()).len()
        );
    }

    #[test]
    fn snapshot_texts_mark_own_rows_and_truncation() {
        let mut a = Snapshot {
            v: 1,
            taken_at_ms: 0,
            connection: RecordConnection {
                id: "c".into(),
                name: "C".into(),
                environment: crate::conn::profile::Environment::Dev,
            },
            member: "pg-1".into(),
            server_version_num: 170_000,
            in_recovery: false,
            pgss_version: "1.11".into(),
            stats_reset_ms: None,
            dealloc: None,
            max: 0,
            track_planning: false,
            hidden: false,
            kcache: false,
            row_count: 0,
            rows: vec![],
            texts: BTreeMap::new(),
            truncated: BTreeSet::new(),
        };
        let mut b = a.clone();
        a.texts.insert("1".into(), "/* glafira */ SELECT 1".into());
        b.texts
            .insert("2".into(), "SELECT * FROM shop.orders".into());
        b.truncated.insert("2".into());
        let key = |id| row(id, 1, 1.0).key;
        let t = pg_advisor::queries::Totals::default();
        let mut s1 = pg_advisor::queries::stat(key(1), None, None, &Counters::default(), None, &t);
        annotate(&a, &b, &mut s1);
        assert!(s1.own && !s1.text_truncated);
        let mut s2 = pg_advisor::queries::stat(key(2), None, None, &Counters::default(), None, &t);
        annotate(&a, &b, &mut s2);
        assert!(!s2.own && s2.text_truncated);
        assert_eq!(s2.text.as_deref(), Some("SELECT * FROM shop.orders"));
        let mut s3 = pg_advisor::queries::stat(key(3), None, None, &Counters::default(), None, &t);
        annotate(&a, &b, &mut s3);
        assert_eq!((s3.text, s3.own), (None, false));
        // Файл без полей `pg_stat_kcache` (записан до задачи 8, `v` тот же): читается как «без kcache».
        let mut c = b.clone();
        c.rows = vec![row(1, 1, 1.0)];
        c.rows[0].counters.cpu_user_ms = Some(2.0);
        c.kcache = true;
        let mut v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["kcache"], true);
        assert_eq!(v["rows"][0]["counters"]["cpuUserMs"], 2.0);
        v.as_object_mut().unwrap().remove("kcache");
        let counters = v["rows"][0]["counters"].as_object_mut().unwrap();
        for f in ["cpuUserMs", "cpuSysMs", "diskReadBytes", "diskWriteBytes"] {
            counters.remove(f);
        }
        let back: Snapshot = serde_json::from_value(v).unwrap();
        assert!(!back.kcache && back.v == SNAPSHOT_VERSION);
        assert_eq!(back.rows[0].counters.cpu_user_ms, None);
    }

    /// Сравнение: колонки kcache — только если расширение было в обоих снимках.
    #[test]
    fn compare_keeps_kcache_only_when_both_snapshots_have_it() {
        let mut a: Snapshot = serde_json::from_value(serde_json::json!({
            "v": 1, "takenAtMs": 0, "connection": {"id": "c", "name": "C", "environment": "dev"},
            "member": "pg-1", "serverVersionNum": 170000, "inRecovery": false, "pgssVersion": "1.11",
            "statsResetMs": null, "dealloc": null, "max": 0, "trackPlanning": false, "hidden": false,
            "rows": [], "texts": {}, "truncated": []
        }))
        .unwrap();
        let mut b = a.clone();
        let with_cpu = |id: i64, cpu: f64| {
            let mut r = row(id, 10, 10.0);
            r.counters.cpu_user_ms = Some(cpu);
            r.counters.cpu_sys_ms = Some(0.0);
            r
        };
        a.rows = vec![with_cpu(1, 1.0)];
        b.rows = vec![with_cpu(2, 7.0)];
        b.kcache = true;
        let c = compare_pair(&a, &b);
        assert_eq!((c.new.len(), c.gone.len()), (1, 1));
        assert_eq!(c.new[0].cpu_user_ms, None, "kcache только в `to`");
        a.kcache = true;
        let c = compare_pair(&a, &b);
        assert_eq!(
            (c.new[0].cpu_user_ms, c.gone[0].cpu_user_ms),
            (Some(7.0), Some(1.0))
        );
    }
}
