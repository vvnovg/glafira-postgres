# Glafira for PostgreSQL

Desktop-приложение (Tauri 2 + Rust + React) для диагностики PostgreSQL и кластеров Patroni.

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada; this project is not affiliated with it.

Концепция: `docs/superpowers/specs/2026-10-19-glafira-postgres-concept-design.md`; этап P1: `docs/superpowers/specs/2026-10-19-glafira-postgres-p1-design.md`.
Руководство администратора (установка, политика, роль для диагностики, сеть): `docs/admin.md`.

## Установка
Установщики (macOS `.dmg`/`.pkg`, Windows `.msi`, Linux `.deb`/`.rpm`/`.AppImage`) — в разделе релизов:
https://github.com/vvnovg/glafira-postgres-releases/releases

Сборка `.dmg` для macOS подписана Developer ID и нотаризована; `.pkg` предназначен для раздачи через MDM и отдельно не нотаризуется. Установщик Windows пока не подписан: SmartScreen → «Подробнее» → «Выполнить в любом случае».
Приложение проверяет обновления при запуске; вручную — «О программе» → «Проверить обновления».

Для администраторов (тихая установка, политика, зеркало обновлений): `docs/admin.md`.

## Что умеет (P1a)
- Подключения трёх видов: сервер, список узлов (primary определяется автоматически, как multi-host JDBC), кластер Patroni (REST API).
- Вставка строки подключения: JDBC, `postgresql://…`, `host=… port=…`; пароль из `~/.pgpass` (права 0600: `chmod 0600 ~/.pgpass`, иначе файл игнорируется), если не сохранён.
- Топология с ролями, лагом и таймлайном, опрос раз в 10 с, журнал событий (смена лидера, недоступные узлы, пауза).
- Проверка подключения по шагам и чек-лист готовности к диагностике.
- Дерево: папки → подключения → узлы / базы → схемы → таблицы, представления, функции, последовательности; роли, расширения.
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
- Для сборки vendored OpenSSL — Perl и C-компилятор (macOS: Xcode Command Line Tools; Windows: Visual Studio Build Tools и Perl, например Strawberry Perl;
  Linux: пакеты Tauri из `.github/workflows/ci.yml`). cmake и NASM не нужны.
- `cargo-deny` 0.20.2 для проверки лицензий зависимостей: `cargo install cargo-deny --locked --version 0.20.2`

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

Порты стенда опубликованы только на `127.0.0.1` (учётные данные известные), поэтому `localhost` в адресах ниже работает, а с другой машины стенд недоступен.

Учётные данные: PG `glafira_diag` / `glafira-secret` (роль для диагностики), суперпользователь `postgres` / `postgres-secret`;
REST API Patroni `patroni` / `patroni-secret`. Демо-база `orders`, схема `shop`.
Участники Patroni отдают внутренние адреса `patroniN`: в профиле кластера задайте подмену адресов
(`patroni1 → localhost:5441`, API `http://localhost:8011` и т. д.).

## Тесты
```bash
pnpm test                                               # фронтенд
pnpm test:scripts && pnpm i18n:check                     # скрипты, словари и руководство
cargo test --manifest-path crates/pg-advisor/Cargo.toml  # правила
cargo test --manifest-path src-tauri/Cargo.toml --lib    # Rust unit
cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml check licenses   # лицензии зависимостей
cargo deny --manifest-path crates/pg-advisor/Cargo.toml --config deny.toml check licenses
GPG_IT_SINGLE=localhost:5460 GPG_IT_PRIMARY=localhost:5451 GPG_IT_REPLICA=localhost:5452 \
GPG_IT_PATRONI=http://localhost:8011,http://localhost:8012,http://localhost:8013 GPG_IT_DOCKER=1 \
  cargo test --manifest-path src-tauri/Cargo.toml --tests -- --test-threads=1   # интеграционные (нужен стенд)
```
Без переменной `GPG_IT_*` интеграционный тест пропускается. `GPG_IT_DOCKER=1` разрешает тестам останавливать контейнеры,
делать switchover и promote; после них стенд возвращается в исходное состояние.

## Релиз
`docs/release-checklist.md`: версии в трёх манифестах, раздел в `CHANGELOG.md`, `THIRD_PARTY.md`,
`node scripts/check-release.mjs vX.Y.Z`, тег `vX.Y.Z` → workflow `release` собирает установщики на трёх ОС,
подписывает macOS-сборки и публикует релиз с `latest.json` в `vvnovg/glafira-postgres-releases`.

## Где хранятся данные
- Профили: `connections.json` в каталоге данных (macOS: `~/Library/Application Support/io.github.vvnovg.glafira.postgres/`).
- Пароли — в системном хранилище секретов, сервис `glafira-postgres`.
- Настройки: `settings.json`.
- Логи: `RUST_LOG=debug pnpm tauri dev`.
