# pg-perf — стенд тяжёлых запросов (P3)

PostgreSQL 17 (`gp-pg-perf`, `127.0.0.1:5471`) с `pg_stat_statements` (`track_planning = on`, `pg_stat_statements.max = 5000`),
`pg_stat_kcache`, `pg_wait_sampling` и `hypopg`; база `perf`, схема `shop` (~3,6 млн строк: `customers` 200 тыс., `orders` 500 тыс.,
`order_items` 1,5 млн, `events` 1 млн в 24 партициях, `audit` 200 тыс., `balances` 200 тыс.). Параметры сервера нарочно тесные:
`work_mem = 1MB`, `max_parallel_workers = 2`, низкие пороги JIT.

Роли (пароли — как у остального стенда: `glafira-secret` и `app-secret`):
- `glafira_diag` — `pg_monitor`, `pg_signal_backend`, `pg_read_all_data`; записи в `shop` нет, поэтому EXPLAIN запроса с записью под ней
  отвергается («не хватает прав»);
- `glafira_app` — `SELECT`, `INSERT`, `UPDATE`, `DELETE` на таблицы `shop` (и `USAGE`, `SELECT` на её последовательности), без `pg_monitor`:
  под ней идёт ярус записи EXPLAIN ANALYZE.

Поднять на уже работающем стенде (остальные контейнеры не трогаются):

    docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf

Нагрузка — сценарий `pgbench` из 13 инструкций:

    docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 300

Что в сценарии нарочно:
- **N+1.** Три точечных выбора покупателя (`:cid`, `:cid1`, `:cid2`) нормализуются в одну строку `pg_stat_statements` — около 23 % вызовов при пороге 20 %.
  Соседние id считаются в `\set`: выражение `:cid + 1` прямо в запросе дало бы другой текст и другую строку, и доля упала бы ниже порога.
  Он же — материал для «Планирование дольше выполнения» (у точечного выбора планирование дороже выполнения).
- **Нестабильное время.** `SELECT count(*) FROM shop.audit WHERE order_id = :a AND id BETWEEN 1 AND 300000`: у заказа 1 — 99 % строк `audit`
  (десятки миллисекунд), у остальных — доли миллисекунды. Среднее около 1 мс, то есть на пороге правила (stddev ≥ 2 × среднего при среднем
  от 1 мс), поэтому находка может не появиться.
- **Temp-файлы и чтения с диска** — сортировка и хеш-соединение больших таблиц при `work_mem = 1MB`.
- Остальные запросы — материал для правил рекомендаций P3b: Seq Scan с большим отсевом, Nested Loop по неверной оценке (`audit` без статистики),
  Sort под Limit, Index Only Scan с heap fetches (`balances` без VACUUM), SubPlan на строку, Append без pruning, параллельная агрегация с JIT.
- «Топ неполный» (`queries_incomplete`) этим стендом недостижим: при `pg_stat_statements.max = 5000` вытеснений нет (правило проверяют тесты).

Образ запекает `heavy.sql` при сборке, поэтому живой контейнер, созданный до правки сценария, несёт старый файл (12 инструкций, N+1 недостижим).
Признак: `docker exec gp-pg-perf grep -c cid1 /perf/heavy.sql` печатает `0`. Пока контейнер не пересоздан, подложите файл из рабочей копии:

    docker cp dev/perf/heavy.sql gp-pg-perf:/tmp/heavy.sql
    docker exec gp-pg-perf pgbench -n -U postgres -d perf -f /tmp/heavy.sql -c 4 -T 300

Сбросить статистику: `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT pg_stat_statements_reset()"` (перед прогоном смоука это обязательно:
вызовы интеграционных тестов искажают долю N+1).
Остановить нагрузку: `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = 'pgbench'"`.
Интеграционные тесты: `GPG_IT_PERF=localhost:5471`; нагрузка и `heavy.sql` им не нужны (нужные строки `pg_stat_statements` тесты создают сами), на CI образ
собирается из `dev/perf/Dockerfile`. Смоук — `docs/smoke/p3a.md`.
