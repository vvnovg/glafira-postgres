#!/usr/bin/env bash
# Ждёт готовности стенда: все PG принимают соединения, у Patroni есть лидер и две потоковые реплики,
# реплика без Patroni получила демо-базу.
set -euo pipefail
deadline=$((SECONDS + 240))

wait_for() {
  until "$@" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "timeout: $*" >&2
      exit 1
    fi
    sleep 2
  done
}

for c in gp-pg-single gp-pg-primary gp-pg-replica gp-patroni1 gp-patroni2 gp-patroni3; do
  wait_for docker exec "$c" pg_isready -h localhost -p 5432 -q
done

cluster_ok() {
  local out
  out=$(curl -fs http://localhost:8011/cluster) || return 1
  echo "$out" | grep -q '"role": *"leader"' || return 1
  [ "$(echo "$out" | grep -o '"state": *"streaming"' | wc -l)" -ge 2 ]
}
wait_for cluster_ok
wait_for docker exec gp-pg-replica psql -U postgres -d orders -tAc "select 1"
echo "stand is ready"
