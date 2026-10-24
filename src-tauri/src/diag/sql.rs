//! Общие фрагменты SQL диагностики.

/// `backend_start` сессии (`pg_stat_activity` под псевдонимом `a`) в микросекундах эпохи. Одно выражение во всех
/// запросах: на PG 13 `extract(epoch …)` — `double precision`, и только одинаковое вычисление на одном сервере даёт
/// точное равенство при сверке PID (P2b); на 14+ `extract` возвращает `numeric` и точен сам по себе.
pub const BACKEND_START_US: &str = "(extract(epoch FROM a.backend_start) * 1000000)::bigint";

/// Версия, роль узла и время сервера — первая строка каждого узлового отчёта.
pub const HEAD_SQL: &str = "SELECT current_setting('server_version_num')::int, pg_is_in_recovery(), (extract(epoch FROM now()) * 1000)::bigint";
