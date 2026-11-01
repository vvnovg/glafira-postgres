//! Топ запросов (спец. P3 §4): ключ строки `pg_stat_statements`, аддитивные счётчики, строка отчёта, итоги и
//! сортировки. Эвристики без плана — `rules`.

pub mod compare;
pub mod rules;

use serde::{Deserialize, Serialize};

use crate::pgss::PgssCaps;

/// Ключ строки pgss. `query_id` в JSON — десятичная строка: 64-битный хеш не помещается в число JS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryKey {
    #[serde(with = "crate::serde_util::i64_string")]
    pub query_id: i64,
    pub dbid: u32,
    pub userid: u32,
    pub toplevel: bool,
}

/// Аддитивные счётчики строки pgss: накопительные с момента сброса или дельта за тик / окно. `Option` — колонки,
/// которых нет у версии расширения (`PgssCaps`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counters {
    pub calls: i64,
    pub total_exec_ms: f64,
    pub plans: i64,
    pub total_plan_ms: f64,
    pub rows: i64,
    pub shared_hit: i64,
    pub shared_read: i64,
    pub shared_dirtied: i64,
    pub shared_written: i64,
    pub local_hit: i64,
    pub local_read: i64,
    pub local_dirtied: i64,
    pub local_written: i64,
    pub temp_read: i64,
    pub temp_written: i64,
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    pub temp_io_ms: Option<f64>,
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: i64,
    pub jit_functions: Option<i64>,
    pub jit_ms: Option<f64>,
    pub parallel_planned: Option<i64>,
    pub parallel_launched: Option<i64>,
}

fn sub_opt<T: std::ops::Sub<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a - b),
        (a, None) => a,
        (None, Some(_)) => None,
    }
}

fn add_opt<T: std::ops::Add<Output = T>>(a: Option<T>, b: Option<T>) -> Option<T> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, None) => a,
        (None, b) => b,
    }
}

impl Counters {
    /// Строка изменилась с прошлого тика: были вызовы или время.
    pub fn changed_since(&self, prev: &Counters) -> bool {
        self.calls != prev.calls || self.total_exec_ms != prev.total_exec_ms
    }

    /// Счётчики уменьшились: строка вытеснена и вернулась или статистика сброшена — нужна новая база.
    pub fn decreased_from(&self, prev: &Counters) -> bool {
        self.calls < prev.calls || self.total_exec_ms < prev.total_exec_ms
    }

    /// `self − prev` по всем полям.
    pub fn delta(&self, p: &Counters) -> Counters {
        Counters {
            calls: self.calls - p.calls,
            total_exec_ms: self.total_exec_ms - p.total_exec_ms,
            plans: self.plans - p.plans,
            total_plan_ms: self.total_plan_ms - p.total_plan_ms,
            rows: self.rows - p.rows,
            shared_hit: self.shared_hit - p.shared_hit,
            shared_read: self.shared_read - p.shared_read,
            shared_dirtied: self.shared_dirtied - p.shared_dirtied,
            shared_written: self.shared_written - p.shared_written,
            local_hit: self.local_hit - p.local_hit,
            local_read: self.local_read - p.local_read,
            local_dirtied: self.local_dirtied - p.local_dirtied,
            local_written: self.local_written - p.local_written,
            temp_read: self.temp_read - p.temp_read,
            temp_written: self.temp_written - p.temp_written,
            io_read_ms: self.io_read_ms - p.io_read_ms,
            io_write_ms: self.io_write_ms - p.io_write_ms,
            temp_io_ms: sub_opt(self.temp_io_ms, p.temp_io_ms),
            wal_records: self.wal_records - p.wal_records,
            wal_fpi: self.wal_fpi - p.wal_fpi,
            wal_bytes: self.wal_bytes - p.wal_bytes,
            jit_functions: sub_opt(self.jit_functions, p.jit_functions),
            jit_ms: sub_opt(self.jit_ms, p.jit_ms),
            parallel_planned: sub_opt(self.parallel_planned, p.parallel_planned),
            parallel_launched: sub_opt(self.parallel_launched, p.parallel_launched),
        }
    }

    pub fn add(&mut self, o: &Counters) {
        self.calls += o.calls;
        self.total_exec_ms += o.total_exec_ms;
        self.plans += o.plans;
        self.total_plan_ms += o.total_plan_ms;
        self.rows += o.rows;
        self.shared_hit += o.shared_hit;
        self.shared_read += o.shared_read;
        self.shared_dirtied += o.shared_dirtied;
        self.shared_written += o.shared_written;
        self.local_hit += o.local_hit;
        self.local_read += o.local_read;
        self.local_dirtied += o.local_dirtied;
        self.local_written += o.local_written;
        self.temp_read += o.temp_read;
        self.temp_written += o.temp_written;
        self.io_read_ms += o.io_read_ms;
        self.io_write_ms += o.io_write_ms;
        self.temp_io_ms = add_opt(self.temp_io_ms, o.temp_io_ms);
        self.wal_records += o.wal_records;
        self.wal_fpi += o.wal_fpi;
        self.wal_bytes += o.wal_bytes;
        self.jit_functions = add_opt(self.jit_functions, o.jit_functions);
        self.jit_ms = add_opt(self.jit_ms, o.jit_ms);
        self.parallel_planned = add_opt(self.parallel_planned, o.parallel_planned);
        self.parallel_launched = add_opt(self.parallel_launched, o.parallel_launched);
    }
}

/// Строка pgss с накопительными значениями — как прочитана с сервера.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cumulative {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub counters: Counters,
    pub min_exec_ms: f64,
    pub max_exec_ms: f64,
    pub stddev_exec_ms: f64,
}

/// Итоги набора — знаменатели долей.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub calls: i64,
    pub total_exec_ms: f64,
    pub rows: i64,
}

pub fn totals<'a>(rows: impl IntoIterator<Item = &'a Counters>) -> Totals {
    let mut t = Totals::default();
    for c in rows {
        t.calls += c.calls;
        t.total_exec_ms += c.total_exec_ms;
        t.rows += c.rows;
    }
    t
}

/// Строка отчёта (спец. §4.4). `min` / `max` / `stddev` есть только у накопительных данных: дельты их не дают.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStat {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    pub calls: i64,
    pub total_exec_ms: f64,
    pub mean_exec_ms: f64,
    pub min_exec_ms: Option<f64>,
    pub max_exec_ms: Option<f64>,
    pub stddev_exec_ms: Option<f64>,
    pub rows: i64,
    pub rows_per_call: f64,
    pub plans: i64,
    pub total_plan_ms: f64,
    pub shared_hit: i64,
    pub shared_read: i64,
    pub shared_dirtied: i64,
    pub shared_written: i64,
    pub local_hit: i64,
    pub local_read: i64,
    pub local_dirtied: i64,
    pub local_written: i64,
    pub temp_read: i64,
    pub temp_written: i64,
    pub io_read_ms: f64,
    pub io_write_ms: f64,
    pub temp_io_ms: Option<f64>,
    pub wal_records: i64,
    pub wal_fpi: i64,
    pub wal_bytes: i64,
    pub jit_functions: Option<i64>,
    pub jit_ms: Option<f64>,
    pub parallel_planned: Option<i64>,
    pub parallel_launched: Option<i64>,
    /// Доля в `totals.total_exec_ms`, проценты.
    pub share_pct: f64,
    /// `hit / (hit + read)`; `None` — блоков не читали.
    pub hit_ratio: Option<f64>,
    /// Текст из кеша; `None` — ещё не запрашивался.
    pub text: Option<String>,
    pub text_truncated: bool,
    /// Запрос самой Глафиры (текст с её меткой): не участвует в правилах находок и в итогах, по которым считаются доли.
    #[serde(default)]
    pub own: bool,
}

/// Строка отчёта из счётчиков; `minmax` — `(min, max, stddev)` только у накопительных данных.
pub fn stat(
    key: QueryKey,
    datname: Option<String>,
    rolname: Option<String>,
    c: &Counters,
    minmax: Option<(f64, f64, f64)>,
    totals: &Totals,
) -> QueryStat {
    let per_call = |v: f64| if c.calls > 0 { v / c.calls as f64 } else { 0.0 };
    let blocks = c.shared_hit + c.shared_read;
    QueryStat {
        key,
        datname,
        rolname,
        calls: c.calls,
        total_exec_ms: c.total_exec_ms,
        mean_exec_ms: per_call(c.total_exec_ms),
        min_exec_ms: minmax.map(|m| m.0),
        max_exec_ms: minmax.map(|m| m.1),
        stddev_exec_ms: minmax.map(|m| m.2),
        rows: c.rows,
        rows_per_call: per_call(c.rows as f64),
        plans: c.plans,
        total_plan_ms: c.total_plan_ms,
        shared_hit: c.shared_hit,
        shared_read: c.shared_read,
        shared_dirtied: c.shared_dirtied,
        shared_written: c.shared_written,
        local_hit: c.local_hit,
        local_read: c.local_read,
        local_dirtied: c.local_dirtied,
        local_written: c.local_written,
        temp_read: c.temp_read,
        temp_written: c.temp_written,
        io_read_ms: c.io_read_ms,
        io_write_ms: c.io_write_ms,
        temp_io_ms: c.temp_io_ms,
        wal_records: c.wal_records,
        wal_fpi: c.wal_fpi,
        wal_bytes: c.wal_bytes,
        jit_functions: c.jit_functions,
        jit_ms: c.jit_ms,
        parallel_planned: c.parallel_planned,
        parallel_launched: c.parallel_launched,
        share_pct: if totals.total_exec_ms > 0.0 {
            c.total_exec_ms / totals.total_exec_ms * 100.0
        } else {
            0.0
        },
        hit_ratio: (blocks > 0).then(|| c.shared_hit as f64 / blocks as f64),
        text: None,
        text_truncated: false,
        own: false,
    }
}

/// Сортировки топа (спец. §4.3). `hitRatio` — по возрастанию (худшие сверху), остальные — по убыванию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QuerySort {
    TotalTime,
    MeanTime,
    MaxTime,
    StddevTime,
    Calls,
    RowsPerCall,
    SharedRead,
    HitRatio,
    TempWritten,
    WalBytes,
    IoTime,
    PlanTime,
    JitTime,
}

impl QuerySort {
    pub fn ascending(self) -> bool {
        matches!(self, Self::HitRatio)
    }

    /// Ключ сортировки строки; `None` — значения нет (колонки нет в версии или режиме): такие строки в конце.
    pub fn value(self, s: &QueryStat) -> Option<f64> {
        match self {
            Self::TotalTime => Some(s.total_exec_ms),
            Self::MeanTime => Some(s.mean_exec_ms),
            Self::MaxTime => s.max_exec_ms,
            Self::StddevTime => s.stddev_exec_ms,
            Self::Calls => Some(s.calls as f64),
            Self::RowsPerCall => Some(s.rows_per_call),
            Self::SharedRead => Some(s.shared_read as f64),
            Self::HitRatio => s.hit_ratio,
            Self::TempWritten => Some(s.temp_written as f64),
            Self::WalBytes => Some(s.wal_bytes as f64),
            Self::IoTime => Some(s.io_read_ms + s.io_write_ms),
            Self::PlanTime => Some(s.total_plan_ms),
            Self::JitTime => s.jit_ms,
        }
    }

    /// Доступна ли сортировка: `min` / `max` / `stddev` нет в режиме окна, JIT — только с pgss 1.10.
    pub fn available(self, caps: &PgssCaps, window: bool) -> bool {
        match self {
            Self::MaxTime | Self::StddevTime => !window,
            Self::JitTime => caps.jit,
            _ => true,
        }
    }
}

/// Ключ строки для сравнения: порядок равных строк не зависит от порядка чтения с сервера.
fn key_order(k: &QueryKey) -> (i64, u32, u32, bool) {
    (k.query_id, k.dbid, k.userid, k.toplevel)
}

/// Сортировка и обрезка до `limit`; строки без значения — в конце. Равные по значению строки — по общему времени (по
/// убыванию), затем по ключу: состав и порядок топа не меняются между обновлениями. Сравнение — `total_cmp` (NaN не
/// ломает сортировку).
pub fn top(rows: &mut Vec<QueryStat>, sort: QuerySort, limit: usize) {
    use std::cmp::Ordering;
    rows.sort_by(|a, b| {
        let primary = match (sort.value(a), sort.value(b)) {
            (Some(x), Some(y)) => {
                let o = x.total_cmp(&y);
                if sort.ascending() { o } else { o.reverse() }
            }
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        primary
            .then_with(|| b.total_exec_ms.total_cmp(&a.total_exec_ms))
            .then_with(|| key_order(&a.key).cmp(&key_order(&b.key)))
    });
    rows.truncate(limit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pgss::PgssCaps;

    fn key(id: i64) -> QueryKey {
        QueryKey {
            query_id: id,
            dbid: 5,
            userid: 10,
            toplevel: true,
        }
    }

    fn counters(
        calls: i64,
        total_ms: f64,
        shared_hit: i64,
        shared_read: i64,
        temp_written: i64,
    ) -> Counters {
        Counters {
            calls,
            total_exec_ms: total_ms,
            rows: calls * 2,
            shared_hit,
            shared_read,
            temp_written,
            ..Counters::default()
        }
    }

    #[test]
    fn query_key_serializes_the_id_as_a_string() {
        let v = serde_json::to_value(key(-9_223_372_036_854_775_000)).unwrap();
        assert_eq!(v["queryId"], "-9223372036854775000");
        assert_eq!(
            (v["dbid"].as_u64(), v["toplevel"].as_bool()),
            (Some(5), Some(true))
        );
        let back: QueryKey = serde_json::from_value(v).unwrap();
        assert_eq!(back, key(-9_223_372_036_854_775_000));
    }

    #[test]
    fn counters_delta_add_and_decrease() {
        let a = counters(10, 100.0, 50, 5, 0);
        let b = counters(15, 160.0, 70, 9, 3);
        let d = b.delta(&a);
        assert_eq!(
            (
                d.calls,
                d.total_exec_ms,
                d.shared_hit,
                d.shared_read,
                d.temp_written,
                d.rows
            ),
            (5, 60.0, 20, 4, 3, 10)
        );
        assert!(b.changed_since(&a) && !a.changed_since(&a));
        assert!(a.decreased_from(&b) && !b.decreased_from(&a));
        let mut s = a;
        s.add(&d);
        assert_eq!(s, b);
        // Option-колонки: None − Some остаётся None, Some + None — Some.
        let with_jit = Counters {
            jit_ms: Some(4.0),
            ..Counters::default()
        };
        assert_eq!(with_jit.delta(&Counters::default()).jit_ms, Some(4.0));
        assert_eq!(Counters::default().delta(&with_jit).jit_ms, None);
        let mut acc = Counters::default();
        acc.add(&with_jit);
        assert_eq!(acc.jit_ms, Some(4.0));
    }

    #[test]
    fn stat_computes_shares_ratios_and_means() {
        let t = Totals {
            calls: 100,
            total_exec_ms: 1000.0,
            rows: 0,
        };
        let s = stat(
            key(1),
            Some("perf".into()),
            Some("app".into()),
            &counters(10, 250.0, 90, 10, 0),
            Some((1.0, 50.0, 7.5)),
            &t,
        );
        assert_eq!(
            (s.mean_exec_ms, s.share_pct, s.rows_per_call),
            (25.0, 25.0, 2.0)
        );
        assert_eq!(s.hit_ratio, Some(0.9));
        assert_eq!(
            (s.min_exec_ms, s.max_exec_ms, s.stddev_exec_ms),
            (Some(1.0), Some(50.0), Some(7.5))
        );
        assert_eq!((&s.text, s.text_truncated, s.own), (&None, false, false));
        assert_eq!(serde_json::to_value(&s).unwrap()["own"], false);
        // Без чтений hit ratio не определён; без вызовов среднее — 0; без итогов доля — 0.
        let z = stat(
            key(2),
            None,
            None,
            &Counters::default(),
            None,
            &Totals::default(),
        );
        assert_eq!(
            (
                z.hit_ratio,
                z.mean_exec_ms,
                z.share_pct,
                z.rows_per_call,
                z.min_exec_ms
            ),
            (None, 0.0, 0.0, 0.0, None)
        );
    }

    #[test]
    fn totals_sum_the_counters() {
        let t = totals([&counters(1, 10.0, 0, 0, 0), &counters(2, 5.0, 0, 0, 0)]);
        assert_eq!((t.calls, t.total_exec_ms, t.rows), (3, 15.0, 6));
    }

    #[test]
    fn top_sorts_descending_puts_missing_values_last_and_truncates() {
        let t = Totals {
            calls: 10,
            total_exec_ms: 100.0,
            rows: 0,
        };
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 10.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 50.0, 0, 0, 0), None, &t),
            stat(key(3), None, None, &counters(1, 40.0, 0, 0, 0), None, &t),
        ];
        top(&mut rows, QuerySort::TotalTime, 2);
        assert_eq!(
            rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [2, 3]
        );
        // Hit ratio — по возрастанию, строки без чтений (None) — в конце.
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 1.0, 50, 50, 0), None, &t),
            stat(key(3), None, None, &counters(1, 1.0, 99, 1, 0), None, &t),
        ];
        top(&mut rows, QuerySort::HitRatio, 10);
        assert_eq!(
            rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [2, 3, 1]
        );
        // В режиме окна stddev недоступен → строки без значения в конце, равные — по ключу.
        let mut rows = vec![
            stat(key(1), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
            stat(key(2), None, None, &counters(1, 1.0, 0, 0, 0), None, &t),
        ];
        top(&mut rows, QuerySort::StddevTime, 10);
        assert_eq!(
            rows.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [1, 2]
        );
    }

    /// Равные значения сортировки не перемешиваются между обновлениями: дальше — общее время по убыванию, затем ключ.
    #[test]
    fn top_breaks_ties_by_total_time_then_key() {
        let t = Totals {
            calls: 10,
            total_exec_ms: 100.0,
            rows: 0,
        };
        let k = |id: i64, userid: u32| QueryKey {
            query_id: id,
            dbid: 5,
            userid,
            toplevel: true,
        };
        let row = |key: QueryKey, calls: i64, ms: f64| {
            stat(key, None, None, &counters(calls, ms, 0, 0, 0), None, &t)
        };
        let ids = |rows: &[QueryStat]| {
            rows.iter()
                .map(|r| (r.key.query_id, r.key.userid))
                .collect::<Vec<_>>()
        };
        let input = vec![
            row(k(3, 1), 5, 10.0),
            row(k(2, 2), 5, 10.0),
            row(k(2, 1), 5, 10.0),
            row(k(9, 1), 5, 30.0),
            row(k(1, 1), 7, 1.0),
        ];
        let mut a = input.clone();
        top(&mut a, QuerySort::Calls, 10);
        assert_eq!(ids(&a), [(1, 1), (9, 1), (2, 1), (2, 2), (3, 1)]);
        // Другой исходный порядок — тот же результат.
        let mut b: Vec<QueryStat> = input.into_iter().rev().collect();
        top(&mut b, QuerySort::Calls, 10);
        assert_eq!(ids(&a), ids(&b));
        // NaN не роняет сортировку (`total_cmp`).
        let mut c = vec![row(k(1, 1), 1, f64::NAN), row(k(2, 1), 1, 5.0)];
        top(&mut c, QuerySort::MeanTime, 10);
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn sort_availability_depends_on_caps_and_mode() {
        let c8 = PgssCaps::from_version("1.8").unwrap();
        let c10 = PgssCaps::from_version("1.10").unwrap();
        assert!(QuerySort::TotalTime.available(&c8, true));
        assert!(
            !QuerySort::StddevTime.available(&c10, true)
                && QuerySort::StddevTime.available(&c10, false)
        );
        assert!(!QuerySort::MaxTime.available(&c10, true));
        assert!(
            !QuerySort::JitTime.available(&c8, false) && QuerySort::JitTime.available(&c10, false)
        );
        assert!(QuerySort::HitRatio.ascending() && !QuerySort::Calls.ascending());
        assert_eq!(
            serde_json::to_value(QuerySort::RowsPerCall).unwrap(),
            "rowsPerCall"
        );
    }
}
