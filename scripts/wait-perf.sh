#!/usr/bin/env bash
# Ждёт готовности стенда тяжёлых запросов pg-perf (профиль `perf`): init-скрипт (база perf, расширения, объёмная схема)
# отработал — настоящий сервер слушает TCP только после него.
set -euo pipefail
# shellcheck source=scripts/lib-wait.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-wait.sh"

wait_for docker exec gp-pg-perf pg_isready -h localhost -p 5432 -U postgres -q
wait_for docker exec gp-pg-perf psql -U postgres -d perf -tAc "select count(*) from pg_extension where extname = 'hypopg'"
echo "pg-perf is ready"
