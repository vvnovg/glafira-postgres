#!/bin/bash
# Patroni передаёт строку подключения суперпользователя первым аргументом (пароль — в PGPASSFILE).
set -euo pipefail
psql "$1" -v ON_ERROR_STOP=1 -f /etc/patroni/init.sql
