#!/usr/bin/env bash
# Ждёт готовности стенда: все PG принимают соединения, у Patroni есть лидер и две потоковые реплики,
# реплика без Patroni получила демо-базу и подключилась к primary (потоковая репликация).
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
  out=$(curl -fs --max-time 5 http://localhost:8011/cluster) || return 1
  echo "$out" | grep -q '"role": *"leader"' || return 1
  [ "$(echo "$out" | grep -o '"state": *"streaming"' | wc -l)" -ge 2 ]
}
wait_for cluster_ok
wait_for docker exec gp-pg-replica psql -U postgres -d orders -tAc "select 1"

# Реплика без Patroni стримит с primary: лаг сразу после пересоздания пары измерим только при streaming.
pair_streaming() {
  local n
  n=$(docker exec gp-pg-primary psql -U postgres -tAc "select count(*) from pg_stat_replication where state = 'streaming'") || return 1
  [ "${n:-0}" -ge 1 ]
}
wait_for pair_streaming
echo "stand is ready"
