//! Перенос из Glafira for Kafka (crates/glafira-mcp/src/limits.rs): константы — по спец. P4 §10.1 и §10.3;
//! `HARD_LIMIT_BYTES` — ровно 100 КБ (запас под `truncated` и `hint` оставляет усечение в `glafira-pg-mcp`),
//! добавлены пределы аргументов журнала.
//! Лимиты нагрузки на базу и размера ответа.

use tokio::time::Instant;

/// Вызовов в минуту на сессию (token bucket).
pub const CALLS_PER_MINUTE: u32 = 60;
/// Одновременных операций с базой на сессию и на все сессии.
pub const SESSION_CONCURRENCY: usize = 2;
pub const GLOBAL_CONCURRENCY: usize = 4;
/// Предел сериализованного ответа инструмента; больше — `response_too_large`.
pub const HARD_LIMIT_BYTES: usize = 102_400;
/// Поля `sql` / `rewrittenSql` пишутся в журнал целиком до 16 КБ, остальные аргументы — до 1 КБ.
pub const MAX_SQL_AUDIT_BYTES: usize = 16_384;
pub const MAX_ARGS_AUDIT_BYTES: usize = 1_024;

/// Token bucket: `per_minute` вызовов подряд, затем один в `60 / per_minute` секунд.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    tokens: f64,
    refill_per_sec: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(per_minute: u32) -> Self {
        let capacity = f64::from(per_minute);
        Self {
            capacity,
            tokens: capacity,
            refill_per_sec: capacity / 60.0,
            last: Instant::now(),
        }
    }

    pub fn try_take(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn constants_follow_the_spec() {
        assert_eq!(
            (CALLS_PER_MINUTE, SESSION_CONCURRENCY, GLOBAL_CONCURRENCY),
            (60, 2, 4)
        );
        assert_eq!(HARD_LIMIT_BYTES, 100 * 1024);
        assert_eq!(
            (MAX_SQL_AUDIT_BYTES, MAX_ARGS_AUDIT_BYTES),
            (16 * 1024, 1024)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn bucket_refills_over_time() {
        let mut r = RateLimiter::new(60);
        for _ in 0..60 {
            assert!(r.try_take());
        }
        assert!(!r.try_take());
        tokio::time::advance(Duration::from_millis(1100)).await;
        assert!(r.try_take());
        assert!(!r.try_take());
        tokio::time::advance(Duration::from_secs(120)).await;
        for _ in 0..60 {
            assert!(r.try_take());
        }
        assert!(!r.try_take());
    }
}
