#!/bin/bash
# Реплика без Patroni: при пустом каталоге данных — pg_basebackup с primary, затем обычный запуск.
set -euo pipefail
export PGPASSWORD="$REPLICATION_PASSWORD"
until pg_isready -h pg-primary -p 5432 -q; do sleep 1; done
if [ ! -s "$PGDATA/PG_VERSION" ]; then
  # -R пишет primary_conninfo и standby.signal; слот replica1 создал primary.
  until rm -rf "${PGDATA:?}"/* && pg_basebackup -h pg-primary -U replicator -D "$PGDATA" -X stream -R -S replica1; do
    sleep 2
  done
fi
chmod 0700 "$PGDATA"
exec postgres -c hot_standby=on -c shared_preload_libraries=pg_stat_statements
