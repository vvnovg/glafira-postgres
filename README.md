# Glafira for PostgreSQL

Desktop-приложение (Tauri 2 + Rust + React) для диагностики PostgreSQL и кластеров Patroni.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada; this project is not affiliated with it.

Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`; этап P1: `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`.
Руководство администратора (роль для диагностики, политика, сеть): `docs/admin.md`.

## Что умеет (P1a)
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API).
- Вставка строки подключения: JDBC, `postgresql://…`, `host=… port=…`; пароль из `~/.pgpass`, если не сохранён.
- Топология с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Проверка подключения по шагам и чек-лист готовности к диагностике.
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, функции, последовательности; роли, расширения.
- Экспорт и импорт подключений без секретов.

## Требования
- Rust stable (`rustup`), Xcode Command Line Tools (macOS)
- Node 22+, pnpm
- Доступ на чтение к `github.com/vvnovg/kafka-explorer` (крейт `license-format`)
- Docker — для стенда и интеграционных тестов
- Для сборки vendored OpenSSL — Perl и C-компилятор (macOS: Xcode Command Line Tools; Windows: Visual Studio Build Tools и Perl, например Strawberry Perl;
  Linux: пакеты Tauri из `.github/workflows/ci.yml`). cmake и NASM не нужны.

## Запуск
```bash
pnpm install
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

Учётные данные: PG `glafira_diag` / `glafira-secret` (роль для диагностики), суперпользователь `postgres` / `postgres-secret`;
REST API Patroni `patroni` / `patroni-secret`. Демо-база `orders`, схема `shop`.
Участники Patroni отдают внутренние адреса `patroniN`: в профиле кластера задайте подмену адресов
(`patroni1 → localhost:5441`, API `http://localhost:8011` и т. д.).

## Тесты
```bash
pnpm test                                               # фронтенд
cargo test --manifest-path crates/pg-advisor/Cargo.toml  # правила
cargo test --manifest-path src-tauri/Cargo.toml --lib    # Rust unit
GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 \
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 \
  cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1   # интеграционные (нужен стенд)
```
Без переменной `GPG_IT_*` интеграционный тест пропускается. `GPG_IT_DOCKER=1` разрешает тестам останавливать контейнеры,
делать switchover и promote; после них стенд возвращается в исходное состояние.

## Где хранятся данные
- Профили: `connections.json` в каталоге данных (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres/`).
- Пароли — в системном хранилище секретов, сервис `glafira-postgres`.
- Настройки: `settings.json`; лицензия: `license.json` (если ключ введён вручную).
- Логи: `RUST_LOG=debug pnpm tauri dev`.
