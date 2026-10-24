# Лаборатория P2 (pg-lab): образ postgres:16 с init-скриптами внутри, без bind-монтирования, чтобы контейнер не
# зависел от пути рабочей копии. Контекст сборки — каталог dev.
FROM postgres:16
COPY sql/init.sql /docker-entrypoint-initdb.d/20-init.sql
COPY --chmod=0755 lab/30-logical.sh /docker-entrypoint-initdb.d/30-logical.sh
