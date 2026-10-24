#!/usr/bin/env bash
# Ждёт готовности стенда: все PG принимают соединения, у Patroni есть лидер и две потоковые реплики,
# реплика без Patroni получила демо-базу и подключилась к primary (потоковая репликация), bastion
# отвечает баннером SSH.
set -euo pipefail
# shellcheck source=scripts/lib-wait.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib-wait.sh"

for c in gp-pg-single gp-pg-primary gp-pg-replica gp-patroni1 gp-patroni2 gp-patroni3; do
  wait_for docker exec "$c" pg_isready -h localhost -p 5432 -q
done

# Порт bastion публикует docker-proxy и принимает соединение раньше sshd: готовым считается ответ баннером SSH.
bastion_ready() {
  local banner
  exec 3<>/dev/tcp/127.0.0.1/2222 || return 1
  IFS= read -r -t 3 -u 3 banner || { exec 3<&- 3>&-; return 1; }
  exec 3<&- 3>&-
  [[ "$banner" == SSH-* ]]
}
wait_for bash -c "$(declare -f bastion_ready); bastion_ready"

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
