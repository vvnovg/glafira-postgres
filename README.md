# Glafira for PostgreSQL

Desktop-приложение (Tauri 2 + Rust + React) для диагностики PostgreSQL и кластеров Patroni.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada; this project is not affiliated with it.

Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`; этап P1: `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`; этап P2: `docs/superpowers/specs/2026-10-24-glafira-postgres-p2-design.md`.
Руководство администратора (установка, политика, роль для диагностики, сеть, OpenSSH): `docs/admin.md`.
Руководство пользователя (в приложении — вкладка «Справка», F1): `docs/guide/en/` и `docs/guide/ru/` — «Начало работы», «Подключения», «Дерево и объекты», «Диагностика», «Обновления и диагностика».

## Установка
Установщики (macOS `.dmg`/`.pkg`, Windows `.msi`, Linux `.deb`/`.rpm`/`.AppImage`) — в разделе релизов:
https://github.com/vvnovg/glafira-postgres-releases/releases

Сборка `.dmg` для macOS подписана Developer ID и нотаризована; `.pkg` предназначен для раздачи через MDM и отдельно не нотаризуется. Установщик Windows пока не подписан: SmartScreen → «Подробнее» → «Выполнить в любом случае».
Приложение проверяет обновления при запуске; вручную — «О программе» → «Проверить обновления».

Для администраторов (тихая установка, политика, зеркало обновлений): `docs/admin.md`.

## Что умеет
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API). PostgreSQL 13–18, Patroni 3.0 и новее (включая 4.x).
- SSH-туннель через системный OpenSSH: агент, файл ключа (в том числе с пассфразой), пароль; подтверждение ключа хоста по отпечатку; автоматический перезапуск туннеля. Имена узлов и адреса REST API Patroni разрешает бастион.
- Вставка строки подключения: JDBC, `postgresql://…`, `host=… port=…`; пароль из `~/.pgpass` (права 0600: `chmod 0600 ~/.pgpass`, иначе файл игнорируется), если не сохранён.
- Топология с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Проверка подключения по шагам и чек-лист готовности к диагностике.
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, функции, последовательности; партиции, роли, расширения.
- Вкладки таблицы (колонки, индексы, ограничения, размер и статистика, партиционирование, триггеры), представления, функции и последовательности; DDL таблицы, построенный по каталогу (не замена `pg_dump`).
- Диагностика дежурного (группа «Диагностика» в дереве): пульс узлов кластера, сессии и idle in transaction, профиль ожиданий (сэмплер раз в секунду, история в памяти), дерево блокировок и DDL в очереди, горизонт xmin, реплики, слоты репликации и подписки; у кластера Patroni — готовность к failover и матрица health. Находки с порогами и подсказками «что сделать»; запланированные switchover и restart Patroni — в обзоре. Инструменты диагностики только читают (`SELECT` и `GET`), для полной картины нужна роль `pg_monitor`.
- Действия дежурного: отмена запроса и завершение сессий (поштучно и пакетом «idle in transaction старше N минут»), удаление неактивного слота репликации; у кластера Patroni — switchover (сейчас и по расписанию), failover, restart, пауза и отмена запланированных действий, с наблюдением за ходом до «готово». Каждому действию предшествует предпросмотр и подтверждение (на prod — именем подключения); на подключении «только чтение» действий нет; журнал `actions.jsonl` и его вкладка «Журнал действий». Права и риски — в `docs/admin.md`.
- Папки, экспорт и импорт подключений без секретов, дублирование подключения.
- Интерфейс на английском и русском, встроенное руководство (вкладка «Справка», F1, меню Help на macOS), автообновление.

## Лицензия и поддержка проекта
- Код открыт под [Apache License 2.0](LICENSE). Все функции приложения бесплатны и доступны всем: платных тарифов и лицензионных ключей нет.
- Имя Glafira и значок приложения лицензией не покрываются — см. `TRADEMARK.md`. Лицензии зависимостей — `THIRD_PARTY.md`.
- Проект живёт на пожертвования: пункт «Поддержать проект» в приложении (внизу левой панели, в «О программе» и в меню Help на macOS) ведёт на https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md.
- Как участвовать — `CONTRIBUTING.md`; об уязвимостях — `SECURITY.md`.

## Требования
- Rust stable (`rustup`), Xcode Command Line Tools (macOS)
- Node 22+, pnpm
- Docker — для стенда и интеграционных тестов
- Клиент OpenSSH (`ssh`) в `PATH` — для подключений через SSH-туннель; пароль и пассфраза ключа требуют OpenSSH 8.4+ (агент и ключ без пассфразы работают на любой версии; Windows: компонент «Клиент OpenSSH»). Интеграционный тест `it_ssh` тоже запускает настоящий `ssh`.
- Для сборки vendored OpenSSL — Perl и C-компилятор (macOS: Xcode Command Line Tools; Windows: Visual Studio Build Tools и Perl, например Strawberry Perl;
  Linux: пакеты Tauri из `.github/workflows/ci.yml`). cmake и NASM не нужны.
- `cargo-deny` 0.20.2 для проверки лицензий зависимостей: `cargo install cargo-deny --locked --version 0.20.2`

## Запуск
```bash
pnpm install
dev/ssh/gen-keys.sh          # тестовые ключи для bastion (один раз; в репозиторий не попадают)
docker compose -f dev/docker-compose.yml up -d --build
scripts/wait-stand.sh
pnpm tauri dev
```

## Стенд (`dev/docker-compose.yml`)
| Что | Адрес |
|---|---|
| Кластер Patroni `demo` (PG 17, etcd) | PG `localhost:5441–5443`, REST API `localhost:8011–8013` |
| Primary и реплика без Patroni (PG 16) | `localhost:5451`, `localhost:5452` |
| Одиночный сервер без `pg_stat_statements` (PG 15) | `localhost:5460` |
| Лаборатория P2: логические слоты, подписка, prepared transactions (PG 16) | `localhost:5470` |
| SSH-бастион `bastion` (OpenSSH, пользователь `tester`) | `localhost:2222` |
| Матрица версий PG 13…18 (профиль `matrix`, `gp-pg13`…`gp-pg18`) | `localhost:5513`…`localhost:5518` |

Порты стенда опубликованы только на `127.0.0.1` (учётные данные известные), поэтому `localhost` в адресах стенда выше работает, а с другой машины стенд недоступен.

На стенде, поднятом до P2, лаборатория запускается отдельно: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-lab`
(образ собирается из `dev/lab/Dockerfile`); роль `glafira_app` интеграционные тесты создают сами (`common::ensure_fixtures`).

Учётные данные: PG `glafira_diag` / `glafira-secret` (роль для диагностики), роль приложения `glafira_app` / `app-secret`
(сценарии P2: сессии, блокировки, idle in transaction; не суперпользователь), суперпользователь `postgres` / `postgres-secret`;
REST API Patroni `patroni` / `patroni-secret`. Демо-база `orders`, схема `shop`.
Участники Patroni отдают внутренние адреса `patroniN`: в профиле кластера задайте подмену адресов
(`patroni1 → localhost:5441`, API `http://localhost:8011` и т. д.).

`bastion` проверяет SSH-туннель: он входит в сеть compose и разрешает имена узлов (`pg-single`, `pg-primary`, `pg-replica`,
`patroni1`…`patroni3`). Вход — пароль `tester-secret` или ключи из `dev/ssh/keys/`, которые создаёт `dev/ssh/gen-keys.sh`
(`tester_ed25519` без пассфразы, `tester_ed25519_pass` с пассфразой `test-passphrase`); `gen-keys.sh` нужно выполнить до
`docker compose up --build`. Подробности — `dev/ssh/README.md`.

### Матрица версий (профиль `matrix`)
Шесть одиночных серверов PostgreSQL 13…18 с той же демо-схемой `shop` (роль `glafira_diag`, база `orders`, `pg_stat_statements`);
порт — `55` и номер версии (`gp-pg13` — `localhost:5513`, …, `gp-pg18` — `localhost:5518`). В обычный `up` они не входят:
```bash
docker compose -f dev/docker-compose.yml --profile matrix up -d --no-recreate
scripts/wait-matrix.sh        # можно назвать часть версий: scripts/wait-matrix.sh 15 16
```
С флагом `--no-recreate` запуск профиля не трогает остальной стенд. Без него compose пересоздаёт любой сервис, у которого
изменился хеш конфигурации (например, контейнеры, созданные до перевода портов на `127.0.0.1`, будут пересозданы). Явная
альтернатива — перечислить только сервисы матрицы: `docker compose -f dev/docker-compose.yml --profile matrix up -d pg13 pg14 pg15 pg16 pg17 pg18`.
Интеграционные тесты каталога
проходят по узлам матрицы, если задать `GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518`.

Схема `wide` с 10 000 таблиц для проверки прокрутки и пагинации дерева создаётся вручную в отдельной базе (на любом узле стенда или матрицы),
по умолчанию ни в один сервис не входит; повторный запуск безопасен (около 10 секунд):
```bash
docker exec gp-pg-single createdb -U postgres wide_demo
docker exec -i gp-pg-single psql -U postgres -d wide_demo -v ON_ERROR_STOP=1 < dev/sql/many-tables.sql
docker exec gp-pg-single dropdb -U postgres wide_demo     # убрать, когда не нужна
```
Убрать схему целиком `DROP SCHEMA wide CASCADE` нельзя (не хватит блокировок), поэтому общую демо-базу `orders` под неё не берите:
создавайте отдельную базу и удаляйте базу целиком.

## Тесты
```bash
pnpm test                                               # фронтенд
pnpm test:scripts && pnpm i18n:check                     # скрипты, словари и руководство
cargo test --manifest-path crates/pg-advisor/Cargo.toml  # правила
cargo test --manifest-path src-tauri/Cargo.toml --lib    # Rust unit
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses   # лицензии зависимостей
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 \
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_LAB=localhost:5470 GPG_IT_DOCKER=1 \
GPG_IT_BASTION=localhost:2222 GPG_IT_SSH_KEYS=$PWD/dev/ssh/keys \
GPG_IT_MATRIX=localhost:5513,localhost:5514,localhost:5515,localhost:5516,localhost:5517,localhost:5518 \
  cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1   # интеграционные (нужен стенд)
```
Без переменной `GPG_IT_*` интеграционный тест пропускается (`it_ssh` — без `GPG_IT_BASTION` или `GPG_IT_SSH_KEYS`; он запускает
настоящий OpenSSH 8.4+ из `PATH`; проход по матрице версий — без `GPG_IT_MATRIX`: по ней идут тесты каталога, `it_diag` (SQL каждого инструмента диагностики) и `it_sampler` (запрос сэмплера) на PG 13–18; сценарии лаборатории — без `GPG_IT_LAB`). `GPG_IT_DOCKER=1` разрешает тестам останавливать контейнеры,
делать switchover и promote; после них стенд возвращается в исходное состояние.

Действия: `it_actions` (сессии и слоты; нужны `GPG_IT_SINGLE`, `GPG_IT_PRIMARY`, `GPG_IT_LAB`) и `it_patroni_actions` (Patroni; нужен `GPG_IT_PATRONI`). Сценарии `it_patroni_actions`, которые меняют кластер (switchover сейчас и по расписанию, restart реплики, пауза), идут только с `GPG_IT_DOCKER=1` и возвращают стенд сами: восстановление при любом выходе и ожидание, пока стенд придёт в норму. Failover останавливает PostgreSQL лидера и идёт только ещё и с `GPG_IT_FAILOVER=1`:
```bash
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 GPG_IT_FAILOVER=1 \
  cargo test --manifest-path src-tauri/Cargo.toml --test it_patroni_actions -- --test-threads=1
```

## Релиз
`docs/release-checklist.md`: версии в трёх манифестах, раздел в `CHANGELOG.md`, `THIRD_PARTY.md`,
`node scripts/check-release.mjs vX.Y.Z`, тег `vX.Y.Z` → workflow `release` собирает установщики на трёх ОС,
подписывает macOS-сборки и публикует релиз с `latest.json` в `vvnovg/glafira-postgres-releases`.

## Где хранятся данные
- Профили: `connections.json` в каталоге данных (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres/`).
- Пароли — в системном хранилище секретов, сервис `glafira-postgres`.
- Настройки: `settings.json`.
- Логи: `RUST_LOG=debug pnpm tauri dev`.
