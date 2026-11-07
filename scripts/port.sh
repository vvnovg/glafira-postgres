#!/usr/bin/env bash
# Переносит файл из другой версии Glafira с зафиксированного коммита (строку о нём в PORTED.md добавляет тот, кто переносит).
# Использование: scripts/port.sh <путь в источнике> <путь здесь> [коммит]
# Источник — $SOURCE_REPO (по умолчанию ~/projects/glafira-kafka, Glafira for Apache Kafka); коммит по умолчанию — 6a3fcfe
# (этап U1 «Ревизия интерфейса под macOS»). Kubernetes-версия: SOURCE_REPO=~/projects/glafira-openshift.
set -euo pipefail
repo="${SOURCE_REPO:-$HOME/projects/glafira-kafka}"
commit="${3:-6a3fcfe}"
src="$1"
dst="$2"
mkdir -p "$(dirname "$dst")"
git -C "$repo" show "$commit:$src" > "$dst"
