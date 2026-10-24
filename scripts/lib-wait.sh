#!/usr/bin/env bash
# Общие функции ожидания для wait-stand.sh и wait-matrix.sh (подключается через `source`, сам не запускается).
# WAIT_TIMEOUT — предел ожидания в секундах (по умолчанию 240), отсчитывается от подключения файла.
deadline=$((SECONDS + ${WAIT_TIMEOUT:-240}))

# Повторяет команду раз в 2 с, пока она не завершится успешно; по истечении предела завершает скрипт с кодом 1.
wait_for() {
  until "$@" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "timeout: $*" >&2
      exit 1
    fi
    sleep 2
  done
}
