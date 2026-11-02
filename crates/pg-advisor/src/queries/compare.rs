//! Сравнение двух снимков pgss (спец. P3 §9.3): новые, исчезнувшие, деградировавшие и ускорившиеся запросы по
//! дельтам между снимками; режим топа «между снимками» использует те же дельты.

use std::cmp::Ordering;
use std::collections::HashMap;

use serde::Serialize;

use crate::findings::thresholds as th;
use crate::queries::{Counters, Cumulative, QueryKey, QueryStat, Totals, stat, totals};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub key: QueryKey,
    pub datname: Option<String>,
    pub rolname: Option<String>,
    /// Средняя дельты `from` относительно предыдущего состояния неизвестна: `before` — накопительное `from`.
    pub before: QueryStat,
    /// Статистика дельты `to − from` (без min / max / stddev).
    pub after: QueryStat,
    /// `after.mean / before.mean`.
    pub ratio: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    /// Ключи только в `to` — накопительные значения `to`, доли от `totals_to`.
    pub new: Vec<QueryStat>,
    /// Ключи только в `from` — накопительные значения `from`, доли от `totals_from`.
    pub gone: Vec<QueryStat>,
    /// Новых сверх `COMPARE_LIST_LIMIT` (в `new` — самые долгие).
    pub new_more: usize,
    /// Исчезнувших сверх `COMPARE_LIST_LIMIT`.
    pub gone_more: usize,
    /// По убыванию `ratio`.
    pub degraded: Vec<Change>,
    /// По возрастанию `ratio`.
    pub improved: Vec<Change>,
    pub totals_from: Totals,
    pub totals_to: Totals,
    /// Статистика сбрасывалась между снимками: дельты не сравниваются.
    pub reset: bool,
}

/// Дельты `to − from` по ключу, в порядке `to`: ключ только в `to` — значение `to` целиком; с уменьшившимися
/// счётчиками (и все — после сброса) — счётчики pgss `to` целиком, а колонки `pg_stat_kcache` — дельтой, пока они не
/// уменьшились (`Counters::rebased_with_kcache_delta`: у kcache свой сброс).
pub fn deltas(from: &[Cumulative], to: &[Cumulative], reset: bool) -> Vec<(QueryKey, Counters)> {
    let prev: HashMap<QueryKey, &Counters> = from.iter().map(|c| (c.key, &c.counters)).collect();
    to.iter()
        .map(|c| match prev.get(&c.key) {
            Some(p) if !reset && !c.counters.decreased_from(p) => (c.key, c.counters.delta(p)),
            Some(p) => (c.key, c.counters.rebased_with_kcache_delta(p)),
            None => (c.key, c.counters),
        })
        .collect()
}

fn cumulative_stat(c: &Cumulative, t: &Totals) -> QueryStat {
    stat(
        c.key,
        c.datname.clone(),
        c.rolname.clone(),
        &c.counters,
        Some((c.min_exec_ms, c.max_exec_ms, c.stddev_exec_ms)),
        t,
    )
}

/// Оставляет первые `COMPARE_LIST_LIMIT` строк; сколько отброшено.
fn cap(rows: &mut Vec<QueryStat>) -> usize {
    let more = rows.len().saturating_sub(th::COMPARE_LIST_LIMIT);
    rows.truncate(th::COMPARE_LIST_LIMIT);
    more
}

/// Сравнение снимков: `*_reset_ms` — `stats_reset` каждого снимка; различаются — `reset`, и деградации с ускорениями
/// не ищутся (дельта после сброса — не дельта).
pub fn compare(
    from: &[Cumulative],
    from_reset_ms: Option<i64>,
    to: &[Cumulative],
    to_reset_ms: Option<i64>,
) -> Comparison {
    let reset = from_reset_ms != to_reset_ms;
    let totals_from = totals(from.iter().map(|c| &c.counters));
    let totals_to = totals(to.iter().map(|c| &c.counters));
    let from_by: HashMap<QueryKey, &Cumulative> = from.iter().map(|c| (c.key, c)).collect();
    let to_by: HashMap<QueryKey, &Cumulative> = to.iter().map(|c| (c.key, c)).collect();
    let mut out = Comparison {
        totals_from,
        totals_to,
        reset,
        ..Comparison::default()
    };
    out.new = to
        .iter()
        .filter(|c| !from_by.contains_key(&c.key))
        .map(|c| cumulative_stat(c, &totals_to))
        .collect();
    out.gone = from
        .iter()
        .filter(|c| !to_by.contains_key(&c.key))
        .map(|c| cumulative_stat(c, &totals_from))
        .collect();
    if !reset {
        let d = deltas(from, to, false);
        let delta_totals = totals(d.iter().map(|(_, c)| c));
        for (key, d) in d {
            let (Some(f), Some(t)) = (from_by.get(&key), to_by.get(&key)) else {
                continue;
            };
            if f.counters.calls < th::COMPARE_MIN_CALLS || d.calls < th::COMPARE_MIN_CALLS {
                continue;
            }
            let before_mean = f.counters.total_exec_ms / f.counters.calls as f64;
            let after_mean = d.total_exec_ms / d.calls as f64;
            if before_mean <= 0.0 {
                continue;
            }
            let ratio = after_mean / before_mean;
            let change = || Change {
                key,
                datname: t.datname.clone(),
                rolname: t.rolname.clone(),
                before: cumulative_stat(f, &totals_from),
                after: stat(
                    key,
                    t.datname.clone(),
                    t.rolname.clone(),
                    &d,
                    None,
                    &delta_totals,
                ),
                ratio,
            };
            if ratio >= th::COMPARE_DEGRADED_RATIO {
                out.degraded.push(change());
            } else if ratio <= th::COMPARE_IMPROVED_RATIO {
                out.improved.push(change());
            }
        }
    }
    let by_time = |a: &QueryStat, b: &QueryStat| b.total_exec_ms.total_cmp(&a.total_exec_ms);
    out.new.sort_by(by_time);
    out.gone.sort_by(by_time);
    out.new_more = cap(&mut out.new);
    out.gone_more = cap(&mut out.gone);
    out.degraded
        .sort_by(|a, b| b.ratio.partial_cmp(&a.ratio).unwrap_or(Ordering::Equal));
    out.improved
        .sort_by(|a, b| a.ratio.partial_cmp(&b.ratio).unwrap_or(Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{Counters, Cumulative, QueryKey};

    fn row(id: i64, calls: i64, ms: f64) -> Cumulative {
        Cumulative {
            key: QueryKey {
                query_id: id,
                dbid: 1,
                userid: 1,
                toplevel: true,
            },
            datname: Some("perf".into()),
            rolname: Some("app".into()),
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

    /// Новые и исчезнувшие — не больше `COMPARE_LIST_LIMIT` самых долгих, остаток — числом (`newMore`, `goneMore`).
    #[test]
    fn new_and_gone_are_capped_with_a_remainder_count() {
        let limit = th::COMPARE_LIST_LIMIT;
        let n = limit as i64 + 5;
        let from: Vec<Cumulative> = (0..n).map(|i| row(i, 10, i as f64)).collect();
        let to: Vec<Cumulative> = (n..n * 2).map(|i| row(i, 10, i as f64)).collect();
        let c = compare(&from, Some(1), &to, Some(1));
        assert_eq!((c.new.len(), c.new_more), (limit, 5));
        assert_eq!((c.gone.len(), c.gone_more), (limit, 5));
        assert_eq!(c.new[0].key.query_id, n * 2 - 1, "самые долгие остаются");
        assert_eq!(c.gone.last().unwrap().key.query_id, 5);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!((&v["newMore"], &v["goneMore"]), (&5.into(), &5.into()));
        let small = compare(&from[..3], Some(1), &to[..2], Some(1));
        assert_eq!((small.new_more, small.gone_more), (0, 0));
    }

    #[test]
    fn classifies_new_gone_degraded_and_improved() {
        let from = [
            row(1, 100, 1000.0),
            row(2, 100, 1000.0),
            row(3, 100, 1000.0),
            row(4, 10, 100.0),
        ];
        let to = [
            row(1, 200, 4000.0),
            row(2, 200, 2500.0),
            row(3, 200, 1200.0),
            row(4, 20, 110.0),
            row(5, 50, 50.0),
        ];
        // Дельты: 1 — 100 вызовов за 3000 мс (30 мс против 10 раньше — ×3, деградация); 2 — 100 за 1500 (15 против 10 — ×1,5, в норме);
        // 3 — 100 за 200 (2 против 10 — ×0,2, ускорился); 4 — мало вызовов; 5 — новый.
        let c = compare(&from, Some(1), &to, Some(1));
        assert_eq!(
            c.new.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [5]
        );
        assert!(c.gone.is_empty());
        assert_eq!(
            c.degraded
                .iter()
                .map(|x| (x.key.query_id, (x.ratio * 10.0).round() / 10.0))
                .collect::<Vec<_>>(),
            [(1, 3.0)]
        );
        assert_eq!(
            c.improved
                .iter()
                .map(|x| x.key.query_id)
                .collect::<Vec<_>>(),
            [3]
        );
        assert_eq!((c.totals_from.calls, c.totals_to.calls), (310, 670));
        assert!(!c.reset);
        assert_eq!(
            c.degraded[0].after.mean_exec_ms, 30.0,
            "`after` — средняя дельты, не накопительная"
        );
        assert_eq!(c.degraded[0].after.stddev_exec_ms, None);
        assert_eq!(c.degraded[0].before.mean_exec_ms, 10.0);
    }

    #[test]
    fn gone_and_reset_are_reported() {
        let from = [row(1, 100, 1000.0), row(9, 100, 1000.0)];
        let to = [row(1, 20, 200.0)];
        let c = compare(&from, Some(1), &to, Some(2));
        assert_eq!(
            c.gone.iter().map(|r| r.key.query_id).collect::<Vec<_>>(),
            [9]
        );
        assert!(c.reset, "stats_reset различается");
        assert!(
            c.degraded.is_empty() && c.improved.is_empty(),
            "после сброса дельты не сравниваются"
        );
    }

    /// Замечание 15 смоука P3b: `pg_stat_statements_reset()` не сбрасывает `pg_stat_kcache`. После сброса pgss счётчики
    /// pgss — значения `to` целиком, а колонки kcache — дельтой против `from`, пока они не уменьшились (kcache не
    /// сбрасывался); уменьшились — значение `to` целиком.
    #[test]
    fn kcache_deltas_survive_a_pgss_reset() {
        let with_cpu = |mut r: Cumulative, cpu: f64| {
            r.counters.cpu_user_ms = Some(cpu);
            r.counters.disk_read_bytes = Some(cpu as i64 * 10);
            r
        };
        let from = [
            with_cpu(row(1, 1000, 10_000.0), 4_000_000.0),
            with_cpu(row(2, 10, 100.0), 500.0),
        ];
        let to = [
            with_cpu(row(1, 50, 80_000.0), 4_060_000.0),
            with_cpu(row(2, 5, 50.0), 20.0),
            with_cpu(row(3, 1, 1.0), 7.0),
        ];
        let d = deltas(&from, &to, true);
        let got: Vec<_> = d
            .iter()
            .map(|(k, c)| (k.query_id, c.calls, c.cpu_user_ms, c.disk_read_bytes))
            .collect();
        assert_eq!(
            got,
            [
                (1, 50, Some(60_000.0), Some(600_000)),
                (2, 5, Some(20.0), Some(200)),
                (3, 1, Some(7.0), Some(70)),
            ],
            "pgss — целиком, kcache — дельта, сброшенный kcache и новый ключ — целиком"
        );
        // Вытесненная и вернувшаяся строка pgss без сброса — то же правило.
        let d = deltas(&from[..1], &to[..1], false);
        assert_eq!((d[0].1.calls, d[0].1.cpu_user_ms), (50, Some(60_000.0)));
    }

    #[test]
    fn deltas_between_snapshots_handle_decreased_counters() {
        let from = [row(1, 100, 1000.0), row(2, 100, 1000.0)];
        let to = [row(1, 150, 1500.0), row(2, 5, 50.0)];
        let d = deltas(&from, &to, false);
        assert_eq!(
            d.iter()
                .map(|(k, c)| (k.query_id, c.calls))
                .collect::<Vec<_>>(),
            [(1, 50), (2, 5)],
            "уменьшившийся счётчик — значение целиком"
        );
        let d = deltas(&from, &to, true);
        assert_eq!(
            d.iter()
                .map(|(k, c)| (k.query_id, c.calls))
                .collect::<Vec<_>>(),
            [(1, 150), (2, 5)],
            "после сброса — значения `to`"
        );
    }
}
