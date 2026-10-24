#!/bin/bash
# Логическая репликация внутри pg-lab: публикация в orders, слот pgoutput и подписка на неё из БД lab_sub.
# Подписка подключается через unix-сокет (local trust образа postgres): пароль в строке подключения не нужен,
# и сокет доступен уже во время инициализации, когда TCP ещё выключен.
set -euo pipefail
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname orders <<'SQL'
CREATE PUBLICATION lab_pub FOR TABLE shop.customers;
SELECT pg_create_logical_replication_slot('lab_sub_slot', 'pgoutput');
SQL
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname postgres -c "CREATE DATABASE lab_sub"
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname lab_sub <<'SQL'
CREATE SCHEMA shop;
CREATE TABLE shop.customers (id bigint PRIMARY KEY, email text NOT NULL UNIQUE, created_at timestamptz NOT NULL DEFAULT now());
CREATE SUBSCRIPTION lab_sub CONNECTION 'host=/var/run/postgresql dbname=orders user=postgres'
  PUBLICATION lab_pub WITH (create_slot = false, slot_name = 'lab_sub_slot', copy_data = false);
SQL
