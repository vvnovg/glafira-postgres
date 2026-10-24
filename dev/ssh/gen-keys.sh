#!/usr/bin/env bash
# Тестовые ключи пользователя tester для bastion: без пассфразы и с пассфразой test-passphrase.
# Каталог keys/ в .gitignore — приватные ключи в репозиторий не попадают.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p keys
[ -f keys/tester_ed25519 ] || ssh-keygen -q -t ed25519 -N '' -C tester -f keys/tester_ed25519
[ -f keys/tester_ed25519_pass ] || ssh-keygen -q -t ed25519 -N 'test-passphrase' -C tester-pass -f keys/tester_ed25519_pass
chmod 600 keys/tester_ed25519 keys/tester_ed25519_pass
echo "keys ready in $(pwd)/keys"
