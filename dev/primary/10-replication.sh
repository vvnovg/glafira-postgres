#!/bin/bash
# Пользователь и слот репликации для pg-replica; доступ к репликации по сети.
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres <<'SQL'
CREATE ROLE replicator WITH REPLICATION LOGIN PASSWORD 'replicator-secret';
SELECT pg_create_physical_replication_slot('replica1');
SQL
echo "host replication replicator all scram-sha-256" >> "$PGDATA/pg_hba.conf"
