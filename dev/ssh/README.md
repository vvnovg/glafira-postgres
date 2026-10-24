# Бастион стенда (`bastion`)

SSH-сервер (OpenSSH, Debian bookworm) в сети compose для проверки туннеля `ssh -D`: через него видны узлы стенда по
внутренним именам (`pg-single`, `pg-primary`, `pg-replica`, `patroni1`…`patroni3`). Опубликован только на
`127.0.0.1:2222` (учётные данные известные).

| Что | Значение |
|---|---|
| Адрес | `localhost:2222` |
| Пользователь | `tester` |
| Пароль | `tester-secret` |
| Ключ без пассфразы | `dev/ssh/keys/tester_ed25519` |
| Ключ с пассфразой | `dev/ssh/keys/tester_ed25519_pass`, пассфраза `test-passphrase` |

Ключи не хранятся в репозитории (каталог `dev/ssh/keys/` в `.gitignore`): их создаёт скрипт, и **до** сборки образа,
потому что публичные ключи копируются в образ.

```bash
dev/ssh/gen-keys.sh
docker compose -f dev/docker-compose.yml up -d --build bastion
```

Проверка руками:

```bash
ssh -p 2222 -i dev/ssh/keys/tester_ed25519 tester@localhost                       # вход по ключу
ssh -p 2222 tester@localhost                                                      # вход по паролю
ssh -N -D 127.0.0.1:1080 -p 2222 -i dev/ssh/keys/tester_ed25519 tester@localhost  # SOCKS5, как у приложения
```

Интеграционные тесты (`src-tauri/tests/it_ssh.rs`) читают `GPG_IT_BASTION=localhost:2222` и
`GPG_IT_SSH_KEYS=$PWD/dev/ssh/keys` (абсолютный путь). Свой `known_hosts` они создают во временном каталоге:
`~/.ssh/known_hosts` разработчика не меняется. При пересборке образа ключи хоста bastion меняются — это нормально для
стенда.
