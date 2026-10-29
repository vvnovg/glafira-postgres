//! Эвристики топа без плана (спец. P3 §6): считаются по строкам отчёта (уже обрезанным до предела) в выбранном режиме. Критических находок нет — это подсказки «куда смотреть», а не аварии.

use crate::findings::{Finding, FindingTarget, Severity, thresholds as th};
use crate::queries::{QueryStat, Totals};

/// Режим отчёта: накопительно с момента сброса pgss или дельта за окно (накопительных min/max/stddev в окне нет).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    SinceReset,
    Window,
}

/// Состояние pgss для правила «топ неполный» и для планирования.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PgssState {
    /// `pg_stat_statements.track_planning = on`: иначе колонки планирования нулевые и правило молчит.
    pub track_planning: bool,
    /// `pg_stat_statements_info.dealloc` накопительно (1.9+).
    pub dealloc: Option<i64>,
    /// `dealloc` вырос за окно отчёта.
    pub dealloc_grew: bool,
    /// `pg_stat_statements.max`.
    pub max: i64,
}

/// Эвристики топа без плана. `rows` — строки уже обрезанного отчёта; `totals` — итоги того же набора.
pub fn evaluate(
    member: Option<&str>,
    rows: &[QueryStat],
    totals: &Totals,
    pgss: &PgssState,
    mode: Mode,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for r in rows {
        let qid = r.key.query_id.to_string();
        let db = r.datname.clone().unwrap_or_default();
        let finding = |id: &str, severity: Severity| {
            Finding::new(id, severity)
                .member(member)
                .target(FindingTarget::Query {
                    query_id: qid.clone(),
                    database: db.clone(),
                })
                .link_query(&qid, &db, member)
        };
        // N+1: большая доля вызовов при микроскопическом среднем и заметном объёме.
        if totals.calls > 0 {
            let share = r.calls as f64 / totals.calls as f64;
            if share >= th::QUERY_N1_CALLS_SHARE
                && r.mean_exec_ms <= th::QUERY_N1_MEAN_MS
                && r.calls >= th::QUERY_N1_MIN_CALLS
            {
                out.push(
                    finding("query_n_plus_one", Severity::Warning)
                        .param("callsSharePct", share * 100.0)
                        .param("meanMs", r.mean_exec_ms)
                        .param("calls", r.calls),
                );
            }
        }
        // Нестабильный план: stddev есть только у накопительных данных.
        if mode == Mode::SinceReset
            && let Some(stddev) = r.stddev_exec_ms
            && r.mean_exec_ms >= th::QUERY_UNSTABLE_MIN_MEAN_MS
            && stddev >= th::QUERY_UNSTABLE_STDDEV_RATIO * r.mean_exec_ms
            && r.calls >= th::QUERY_UNSTABLE_MIN_CALLS
        {
            out.push(
                finding("query_unstable", Severity::Warning)
                    .param("stddevMs", stddev)
                    .param("meanMs", r.mean_exec_ms)
                    .param("calls", r.calls),
            );
        }
        // Низкий hit ratio: блоки читались с диска или ОС, а не брались из кеша.
        if r.shared_read >= th::QUERY_HIT_MIN_READ_BLOCKS
            && let Some(hit) = r.hit_ratio
            && hit < th::QUERY_HIT_RATIO_LOW
        {
            out.push(
                finding("query_low_hit_ratio", Severity::Warning)
                    .param("hitRatioPct", hit * 100.0)
                    .param("readBytes", r.shared_read * th::BLOCK_BYTES),
            );
        }
        // Temp-файлы: запрос регулярно пишет во временные файлы (сортировка, хеш, материализация).
        if r.calls >= th::QUERY_TEMP_MIN_CALLS
            && r.temp_written / r.calls >= th::QUERY_TEMP_BLOCKS_PER_CALL
        {
            out.push(
                finding("query_temp", Severity::Warning)
                    .param("tempBytes", r.temp_written / r.calls * th::BLOCK_BYTES)
                    .param("calls", r.calls),
            );
        }
        // Планирование дороже выполнения: нужна track_planning, иначе колонки нулевые.
        if pgss.track_planning
            && r.plans >= th::QUERY_PLANNING_MIN_PLANS
            && r.total_plan_ms > r.total_exec_ms
        {
            out.push(
                finding("query_planning_heavy", Severity::Info)
                    .param("planMs", r.total_plan_ms)
                    .param("execMs", r.total_exec_ms),
            );
        }
    }
    // Топ неполный: pgss вытеснял строки (накопительно с сброса или за окно — рост dealloc).
    let incomplete = match mode {
        Mode::SinceReset => pgss.dealloc.is_some_and(|d| d > 0),
        Mode::Window => pgss.dealloc_grew,
    };
    if incomplete {
        let mut f = Finding::new("queries_incomplete", Severity::Warning)
            .member(member)
            .param("dealloc", pgss.dealloc.unwrap_or(0))
            .param("max", pgss.max);
        if let Some(m) = member {
            f = f.target(FindingTarget::Member {
                name: m.to_string(),
            });
        }
        out.push(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::{Counters, QueryKey, stat};

    fn key(id: i64) -> QueryKey {
        QueryKey {
            query_id: id,
            dbid: 1,
            userid: 1,
            toplevel: true,
        }
    }

    fn row(id: i64, c: Counters, minmax: Option<(f64, f64, f64)>, totals: &Totals) -> QueryStat {
        stat(
            key(id),
            Some("perf".into()),
            Some("app".into()),
            &c,
            minmax,
            totals,
        )
    }

    fn pgss() -> PgssState {
        PgssState {
            track_planning: true,
            dealloc: Some(0),
            dealloc_grew: false,
            max: 5000,
        }
    }

    fn ids(f: &[Finding]) -> Vec<&str> {
        f.iter().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn n_plus_one_needs_share_tiny_mean_and_volume() {
        let totals = Totals {
            calls: 10_000,
            total_exec_ms: 10_000.0,
            rows: 0,
        };
        let hot = Counters {
            calls: 3000,
            total_exec_ms: 1500.0,
            ..Counters::default()
        }; // 30 %, 0,5 мс
        let cold = Counters {
            calls: 3000,
            total_exec_ms: 9000.0,
            ..Counters::default()
        }; // 3 мс
        let few = Counters {
            calls: 900,
            total_exec_ms: 10.0,
            ..Counters::default()
        }; // 9 %
        let f = evaluate(
            Some("pg-1"),
            &[
                row(1, hot, None, &totals),
                row(2, cold, None, &totals),
                row(3, few, None, &totals),
            ],
            &totals,
            &pgss(),
            Mode::Window,
        );
        assert_eq!(ids(&f), ["query_n_plus_one"]);
        let n1 = &f[0];
        assert_eq!(n1.severity, Severity::Warning);
        assert_eq!(
            n1.target,
            Some(FindingTarget::Query {
                query_id: "1".into(),
                database: "perf".into()
            })
        );
        assert_eq!(n1.params["callsSharePct"], 30.0);
        assert_eq!(n1.params["calls"], 3000);
        assert!(matches!(
            &n1.links[0],
            crate::findings::FindingLink::OpenQuery { query_id, member: Some(m), .. }
                if query_id == "1" && m == "pg-1"
        ));
    }

    #[test]
    fn unstable_plan_only_with_cumulative_stddev() {
        let totals = Totals {
            calls: 100,
            total_exec_ms: 1000.0,
            rows: 0,
        };
        let c = Counters {
            calls: 100,
            total_exec_ms: 1000.0,
            ..Counters::default()
        }; // mean 10
        let f = evaluate(
            None,
            &[row(1, c, Some((1.0, 500.0, 25.0)), &totals)],
            &totals,
            &pgss(),
            Mode::SinceReset,
        );
        assert_eq!(ids(&f), ["query_unstable"]);
        assert_eq!(
            (
                f[0].params["stddevMs"].as_f64(),
                f[0].params["meanMs"].as_f64()
            ),
            (Some(25.0), Some(10.0))
        );
        assert!(
            evaluate(
                None,
                &[row(1, c, Some((1.0, 500.0, 19.0)), &totals)],
                &totals,
                &pgss(),
                Mode::SinceReset,
            )
            .is_empty()
        );
        // За окно stddev нет: правило молчит, даже если бы minmax передали.
        assert!(
            evaluate(
                None,
                &[row(1, c, Some((1.0, 500.0, 25.0)), &totals)],
                &totals,
                &pgss(),
                Mode::Window,
            )
            .is_empty()
        );
    }

    #[test]
    fn low_hit_ratio_and_temp_files() {
        let totals = Totals {
            calls: 100,
            total_exec_ms: 100.0,
            rows: 0,
        };
        let cold = Counters {
            calls: 100,
            total_exec_ms: 100.0,
            shared_hit: 10_000,
            shared_read: 40_000,
            ..Counters::default()
        }; // 20 %
        let small = Counters {
            calls: 100,
            total_exec_ms: 100.0,
            shared_hit: 10,
            shared_read: 9_000,
            ..Counters::default()
        }; // мало чтений
        let temp = Counters {
            calls: 10,
            total_exec_ms: 100.0,
            temp_written: 1_280,
            ..Counters::default()
        }; // 128 блоков на вызов
        let temp_few = Counters {
            calls: 9,
            total_exec_ms: 100.0,
            temp_written: 9_000,
            ..Counters::default()
        };
        let f = evaluate(
            None,
            &[
                row(1, cold, None, &totals),
                row(2, small, None, &totals),
                row(3, temp, None, &totals),
                row(4, temp_few, None, &totals),
            ],
            &totals,
            &pgss(),
            Mode::Window,
        );
        assert_eq!(ids(&f), ["query_low_hit_ratio", "query_temp"]);
        assert_eq!(f[0].params["hitRatioPct"], 20.0);
        assert_eq!(f[0].params["readBytes"], 40_000 * 8192);
        assert_eq!(f[1].params["tempBytes"], 128 * 8192);
    }

    #[test]
    fn planning_heavier_than_execution_needs_track_planning() {
        // 999 вызовов: при 1000 сработало бы и правило N+1 (доля 100 %, среднее 0,1 мс), а тест проверяет только планирование.
        let totals = Totals {
            calls: 999,
            total_exec_ms: 100.0,
            rows: 0,
        };
        let c = Counters {
            calls: 999,
            total_exec_ms: 100.0,
            plans: 999,
            total_plan_ms: 150.0,
            ..Counters::default()
        };
        let f = evaluate(
            None,
            &[row(1, c, None, &totals)],
            &totals,
            &pgss(),
            Mode::SinceReset,
        );
        assert_eq!(ids(&f), ["query_planning_heavy"]);
        assert_eq!(f[0].severity, Severity::Info);
        let off = PgssState {
            track_planning: false,
            ..pgss()
        };
        assert!(
            evaluate(
                None,
                &[row(1, c, None, &totals)],
                &totals,
                &off,
                Mode::SinceReset
            )
            .is_empty()
        );
    }

    #[test]
    fn incomplete_top_depends_on_mode() {
        let totals = Totals::default();
        let since = PgssState {
            dealloc: Some(3),
            dealloc_grew: false,
            ..pgss()
        };
        let f = evaluate(Some("pg-1"), &[], &totals, &since, Mode::SinceReset);
        assert_eq!(ids(&f), ["queries_incomplete"]);
        assert_eq!(
            f[0].target,
            Some(FindingTarget::Member {
                name: "pg-1".into()
            })
        );
        assert_eq!(
            (f[0].params["dealloc"].as_i64(), f[0].params["max"].as_i64()),
            (Some(3), Some(5000))
        );
        // За окно важен рост, а не накопленное значение.
        assert!(evaluate(None, &[], &totals, &since, Mode::Window).is_empty());
        let grew = PgssState {
            dealloc: Some(3),
            dealloc_grew: true,
            ..pgss()
        };
        assert_eq!(
            ids(&evaluate(None, &[], &totals, &grew, Mode::Window)),
            ["queries_incomplete"]
        );
        assert!(
            evaluate(
                None,
                &[],
                &totals,
                &PgssState {
                    dealloc: None,
                    ..pgss()
                },
                Mode::SinceReset,
            )
            .is_empty()
        );
    }
}
