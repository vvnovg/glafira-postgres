# pg-perf — стенд тяжёлых запросов (P3)

PostgreSQL 17 (`gp-pg-perf`, `127.0.0.1:5471`) с `pg_stat_statements` (`track_planning = on`), `pg_stat_kcache`,
`pg_wait_sampling` и `hypopg`; база `perf`, схема `shop` (~3 млн строк), роли `glafira_diag` (`pg_monitor`,
`pg_read_all_data`) и `glafira_app` (пароли — как у остального стенда). Параметры сервера нарочно тесные:
`work_mem = 1MB`, `max_parallel_workers = 2`, низкие пороги JIT.

Поднять на уже работающем стенде (остальные контейнеры не трогаются):

    docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf

Нагрузка — сценарий `pgbench` с запросами под каждую эвристику топа и правило рекомендаций:

    docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300

Сбросить статистику: `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT pg_stat_statements_reset()"`.
Интеграционные тесты: `GPG_IT_PERF=localhost:5471`.
