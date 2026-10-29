# Стенд P3 (pg-perf): PostgreSQL 17 с расширениями, которых нет в официальном образе (hypopg, pg_stat_kcache,
# pg_wait_sampling), объёмной схемой и сценарием pgbench «плохих» запросов. Init-скрипты запечены в образ (как у
# pg-lab), чтобы контейнер не зависел от пути рабочей копии. Контекст сборки — каталог dev.
FROM postgres:17
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      postgresql-17-hypopg postgresql-17-pg-stat-kcache postgresql-17-pg-wait-sampling \
 && rm -rf /var/lib/apt/lists/*
COPY perf/20-init.sql /docker-entrypoint-initdb.d/20-init.sql
COPY perf/heavy.sql /perf/heavy.sql
