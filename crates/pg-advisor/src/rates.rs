//! Скорости и дельты по двум снимкам счётчиков (спец. P2 §3.6). Время снимка — часы сервера.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CounterSnapshot {
    /// `now()` сервера в момент снимка, мс эпохи.
    pub at_ms: i64,
    /// `pg_postmaster_start_time()`, мс: другой — сервер перезапущен, счётчики начались заново.
    pub started_ms: i64,
    /// Наибольший `stats_reset` из `pg_stat_database`, мс: изменился — статистику сбросили.
    pub reset_ms: Option<i64>,
    pub values: BTreeMap<String, i64>,
}

fn comparable(prev: &CounterSnapshot, next: &CounterSnapshot) -> bool {
    prev.started_ms == next.started_ms && prev.reset_ms == next.reset_ms && next.at_ms > prev.at_ms
}

/// Прирост счётчика `key`; `None` — нет прошлого снимка, снимки несравнимы, ключа нет или счётчик уменьшился.
pub fn delta(prev: Option<&CounterSnapshot>, next: &CounterSnapshot, key: &str) -> Option<i64> {
    let prev = prev.filter(|p| comparable(p, next))?;
    let d = next.values.get(key)? - prev.values.get(key)?;
    (d >= 0).then_some(d)
}

/// Прирост в секунду серверного времени.
pub fn rate(prev: Option<&CounterSnapshot>, next: &CounterSnapshot, key: &str) -> Option<f64> {
    let d = delta(prev, next, key)?;
    let secs = (next.at_ms - prev?.at_ms) as f64 / 1000.0;
    (secs > 0.0).then(|| d as f64 / secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(at_ms: i64, started_ms: i64, reset_ms: Option<i64>, commits: i64) -> CounterSnapshot {
        CounterSnapshot {
            at_ms,
            started_ms,
            reset_ms,
            values: [("commits".to_string(), commits)].into(),
        }
    }

    #[test]
    fn rate_is_delta_per_second_of_server_time() {
        let a = snap(1_000, 5, None, 100);
        let b = snap(3_000, 5, None, 300);
        assert_eq!(delta(Some(&a), &b, "commits"), Some(200));
        assert_eq!(rate(Some(&a), &b, "commits"), Some(100.0));
    }

    #[test]
    fn no_rate_without_a_comparable_previous_snapshot() {
        let a = snap(1_000, 5, None, 100);
        assert_eq!(rate(None, &a, "commits"), None, "первый снимок");
        assert_eq!(
            rate(Some(&a), &snap(3_000, 6, None, 300), "commits"),
            None,
            "сервер перезапущен"
        );
        assert_eq!(
            rate(Some(&a), &snap(3_000, 5, Some(2_000), 300), "commits"),
            None,
            "статистику сбросили"
        );
        assert_eq!(
            rate(Some(&a), &snap(3_000, 5, None, 50), "commits"),
            None,
            "счётчик уменьшился"
        );
        assert_eq!(
            rate(Some(&a), &snap(1_000, 5, None, 300), "commits"),
            None,
            "нулевой интервал"
        );
        assert_eq!(
            rate(Some(&a), &snap(3_000, 5, None, 300), "missing"),
            None,
            "нет ключа"
        );
    }
}
