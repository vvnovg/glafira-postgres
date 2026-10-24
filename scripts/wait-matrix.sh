#!/usr/bin/env bash
# Ждёт готовности матрицы версий (профиль `matrix`): контейнеры gp-pg13…gp-pg18 принимают соединения по TCP
# и уже выполнили dev/sql/init.sql (демо-база orders доступна). Во время инициализации образ слушает только
# unix-сокет, поэтому `pg_isready -h localhost` отвечает лишь после перезапуска на настоящем порту.
# Без аргументов ждёт все версии 13…18; можно назвать часть: `scripts/wait-matrix.sh 15 16`.
set -euo pipefail
# shellcheck source=scripts/lib-wait.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-wait.sh"

versions=("$@")
[ "${#versions[@]}" -gt 0 ] || versions=(13 14 15 16 17 18)
for v in "${versions[@]}"; do
  wait_for docker exec "gp-pg$v" pg_isready -h localhost -U postgres -q
  wait_for docker exec "gp-pg$v" psql -U postgres -d orders -tAc "select 1"
done
echo "matrix is ready"
