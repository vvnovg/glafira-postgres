#!/usr/bin/env bash
# .pkg для раздачи через Jamf/Intune: ставит приложение в /Applications.
# Использование: make-pkg.sh <App.app> <out.pkg> [Developer ID Installer identity]
set -euo pipefail
APP="$1"
OUT="$2"
IDENTITY="${3:-}"
ARGS=(--component "$APP" /Applications)
if [ -n "$IDENTITY" ]; then
  ARGS+=(--sign "$IDENTITY")
else
  echo "warning: .pkg без подписи (нет Developer ID Installer)" >&2
fi
productbuild "${ARGS[@]}" "$OUT"
