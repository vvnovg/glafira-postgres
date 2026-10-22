# Glafira for PostgreSQL — этап «Открытый код»: Apache 2.0, без ключей, «Поддержать проект», автообновление, первый релиз

**Дата:** 2026-10-22
**Зачем:** код открывается под Apache License 2.0, приложение остаётся бесплатным целиком и живёт на пожертвования —
та же политика, что принята для Glafira for Apache Kafka 2026-10-21 и реализована там этапом «Открытый код»
(`kafka-explorer/docs/superpowers/specs/2026-10-21-glafira-apache-2-design.md`). Подсистема лицензионных ключей,
унаследованная от Kafka 0.2.x, удаляется, а не отключается: в публичный репозиторий мёртвый код не попадает.
Корпоративная политика администратора остаётся. В отличие от Kafka, у этого приложения ещё не было ни одного
релиза: вместе с лицензией этап приносит релизную обвязку (workflow, скрипты, чек-лист, репозиторий релизов)
и автообновление, чтобы первая установленная версия 0.1.0 умела обновиться до следующих.
Публикация репозитория, секреты Apple и сам релиз v0.1.0 идут после этапа отдельными шагами (§13).

Решения владельца 2026-10-22: репозитории `vvnovg/glafira-postgres` и `vvnovg/glafira-postgres-releases`
(локальная папка `postgres-patroni-diag` не переименовывается); версия первого релиза `0.1.0`; `SUPPORT.md`
отдельный для PostgreSQL с текстом как у Kafka; руководство (en, ru) публикуется в `guide/` репозитория релизов;
updater переносится из Kafka в этот этап; правки ручного прохода P1a закоммичены в `main` (56e2485) до ветки этапа.

## 1. Решения

| Вопрос | Решение |
|---|---|
| Лицензия кода | Apache License 2.0: `LICENSE`, `NOTICE`, `license = "Apache-2.0"` и `repository = "https://github.com/vvnovg/glafira-postgres"` в `src-tauri/Cargo.toml` и `crates/pg-advisor/Cargo.toml`, `"license": "Apache-2.0"` в `package.json`. Имя Glafira и значок лицензией не покрываются (`TRADEMARK.md`) |
| Правообладатель | `NOTICE` и `bundle.copyright` в `tauri.conf.json`: «© 2026 Viacheslav Novgorodtsev» (сейчас там имя продукта); `bundle.publisher` остаётся именем продукта, как у Kafka |
| Функции | Все функции доступны всем: prod-подключения создаются и копируются без ключа, флажок «Только чтение» снимается у любого подключения, импорт prod-подключений сохраняет `readOnly` из файла |
| Подсистема ключей | Удаляется целиком: модуль `license`, команды `get_license_status`/`set_license_key`/`remove_license_key`, зависимость `license-format` (обычная и `dev-keys`) и `.cargo/config.toml`, оба файла `*.key`, проверка ключа в `build.rs`, код ошибки `ProRequired`, экран «Лицензия», уведомление `LicenseNotice`, замок `ProLock`, хук `useLicense` |
| Политика администратора | Остаётся: `updatesEnabled`, `updateUrl`, `language` (все три уже разбираются; первые два теперь документируются, потому что появляется updater). Ключ `licenseKey` удаляется из схемы; в уже развёрнутых политиках он идёт по существующей ветке «неизвестный ключ»: предупреждение в лог, не ошибка политики |
| Поддержка проекта | Один адрес `SUPPORT_URL` в `src/lib/links.ts`; три входа: ссылка внизу левой панели на месте «Лицензии», кнопка в «О программе», пункт меню Help на macOS. Страница — `SUPPORT.md` в репозитории релизов, источник `docs/releases-repo/SUPPORT.md`, публикует `release.yml` |
| «О программе» | Строка «Тариф» заменяется строкой «Лицензия: Apache 2.0» со ссылкой на `LICENSE` в GitHub; появляется раздел «Обновления» (§4); блок политики и её ошибок остаётся |
| Автообновление | `tauri-plugin-updater` как в Kafka: проверка один раз при запуске (уведомление над вкладками), ручная проверка и установка в «О программе», политика `updatesEnabled`/`updateUrl`, адрес `latest.json` в репозитории релизов. Открытый ключ — заглушка до ключа владельца (§4.4) |
| Хранилище | `license.json` в каталоге данных удаляется при запуске, если остался от сборок с ключами (`migrate::remove_legacy_license_file`, как в Kafka) |
| Руководство | Новая глава `updates` «Обновления и диагностика» (en, ru): обновления, «О программе», архив диагностики, поддержка проекта. F1 на «О программе» открывает её |
| Вклад и безопасность | `CONTRIBUTING.md` с DCO (Apache 2.0 §5 покрывает входящие изменения, CLA не нужен); `SECURITY.md` — приватные сообщения об уязвимостях через GitHub, без личной почты |
| Лицензии зависимостей | `deny.toml` в корне с белым списком Kafka (проверено 2026-10-22: все зависимости обоих крейтов проходят) и шаг `cargo deny check licenses` в CI для `src-tauri` и `crates/pg-advisor`; `THIRD_PARTY.md` генерирует `scripts/third-party.mjs` |
| Релиз | `release.yml`, `scripts/check-release.mjs`, `make-latest-json.mjs`, `make-pkg.sh`, `guide-publish.mjs`, `check-dist-compat.mjs` переносятся из Kafka; `docs/release-checklist.md`, `docs/releases-repo/README.md` и `SUPPORT.md`; `CHANGELOG.md` с разделом `0.1.0` |
| Версия | Остаётся `0.1.0` во всех трёх манифестах — это и есть версия первого релиза; тег `v0.1.0` ставится после слияния по чек-листу |

## 2. Что удаляется

| Область | Файлы и сущности |
|---|---|
| Rust | `src-tauri/src/license/` (весь модуль, включая `pro_for_tests`); `commands/license.rs` — кроме `get_policy_status`, которая переезжает в `commands/settings.rs`; `ErrorCode::ProRequired` и строка `PRO_REQUIRED` в `error.rs`; поле `AppState.license` и вызов `set_policy_key` в `with_policy`; `tracing::info!(... "license")` в `lib.rs`; `KEY_LICENSE_KEY`, `Policy.license_key`, `PolicyStatus.has_license_key` в `policy/mod.rs`; `was_prod`/`becomes_prod`/`becomes_writable` и `require_pro` в `commands/connections.rs`; `ImportOptions.is_pro`, `forced_read_only`, счётчик `prod_read_only` и предупреждение `warnings.import.prodReadOnly` в `storage/exchange.rs`; `is_pro` и помощник `pro_state` в `commands/folders.rs`; тест `pro_can_pick_any_language` в `commands/settings.rs`; `InfoFile.license` и параметр `license` у `write_archive` в `diagnostics.rs` |
| Ключи, сборка, зависимости | `src-tauri/license-public.key`, `src-tauri/license-dev-public.key`, `check_license_key` в `src-tauri/build.rs`, `.cargo/config.toml` (нужен был только git-зависимости), `license-format` в `[dependencies]` и `[dev-dependencies]` `src-tauri/Cargo.toml` |
| Фронтенд | `components/license/` (`LicensePanel`, `LicenseNotice`, тесты), `common/ProLock.tsx`, `lib/useLicense.ts`; вкладка `license` в `state/workspace.ts`, `Layout.tsx`, `TabContent.tsx`; ветка `PRO_REQUIRED` в `ErrorBanner.tsx`; `locked`, `proOnly`, `readOnlyLocked`, `proHint` и `ProLock` в `ConnectionForm.tsx`; типы `Tier`, `LicenseState`, `LicenseStatus`, код `PRO_REQUIRED`, `PolicyStatus.hasLicenseKey` и вызовы `getLicenseStatus`/`setLicenseKey`/`removeLicenseKey` в `api/` |
| Словари (en, ru) | Все ключи `license.*` и `errors.license.*`, `common.openLicense`, `common.proHint`, `connections.form.prodPro`, `sidebar.license`, `tabs.license`, `about.tier`, `warnings.import.prodReadOnly_one/_other` (и формы `_few`/`_many` в ru) |
| Документация | Строки про `KAFKA_EXPLORER_TOKEN`, доступ к `kafka-explorer` и `license.json` в `README.md`; строка `licenseKey` в таблице `docs/admin.md` и в примерах `docs/admin/policy.{json,reg,mobileconfig}`; «лицензия на выбор языка не влияет» и пример плюрала `license.expiring_*` в `docs/i18n/README.md`; `Pro`, `Free` в «Не переводится» и строка `plan | тариф` в `docs/i18n/glossary.md` |
| CI | В `ci.yml` — шаги «Доступ к kafka-explorer (license-format)» в обоих job, `env.CARGO_NET_GIT_FETCH_WITH_CLI`, шаг «Закрытые ключи лицензий не в репозитории» и комментарий про отдельный секрет у `permissions` |

Прежние спеки и план P1a в `docs/superpowers/` не правятся: это история проектирования, упоминания Pro там остаются.
Внутренний обзор `docs/licensing-review-2026-10-19.md` не публикуется: он добавляется в `.gitignore` (сейчас лежит
в рабочем каталоге незакоммиченным).

## 3. Что меняется в коде

### 3.1 Rust

- **Команды.** `connections.rs`: сохранение не проверяет тариф — prod создаётся и становится доступным для записи
  без ключа; три теста с `ProRequired` переворачиваются. `folders.rs` и `storage/exchange.rs`: импорт сохраняет
  `readOnly` как в файле; предупреждение про read-only не формируется; фикстура с ним в `ImportDialog.test.tsx`
  заменяется другим предупреждением (`warnings.import.missingFiles`).
- **Политика.** `policy/mod.rs`: ключ `licenseKey` не разбирается — он попадает в ветку
  `tracing::warn!("unknown policy key … ignored")`, не ломает политику и не выключает обновления. В тестах
  `policy/sources.rs` ключ-пример `licenseKey` заменяется на `updateUrl`; `documented_examples_parse` проверяет
  новые примеры (§8): `updatesEnabled = false`, `updateUrl`, `language = en` и наличие этих трёх ключей в
  `policy.mobileconfig` и `policy.reg`.
- **Запуск.** Новый `src-tauri/src/migrate.rs` с единственной функцией `remove_legacy_license_file(data_dir)`:
  удаляет `license.json`, если он есть; отсутствие файла — тишина, ошибка удаления — warn в лог, запуск не
  прерывается. Вызывается из `AppState::new` после создания каталога данных.
- **Диагностика.** `InfoFile { app, policy }`; `write_archive` теряет параметр `license`; `commands/app.rs`
  передаёт на один аргумент меньше; тесты проверяют, что в `info.json` нет ключа `license` (без учёта регистра).
- **Сборка.** `build.rs` оставляет только `git_commit` и `tauri_build::build()`.
- **Манифесты.** `license` и `repository` в обоих `Cargo.toml`; `"license": "Apache-2.0"` в `package.json`;
  `bundle.copyright` в `tauri.conf.json`.

### 3.2 Фронтенд

- **Разблокировка.** `ConnectionForm`: окружение `prod` доступно всегда и подписано просто `prod`; у нового
  подключения (в том числе копии) выбор `prod` устанавливает флажок «Только чтение», который можно снять до
  сохранения; у существующего подключения флажок всегда доступен. `ErrorBanner`: без кнопки «Открыть «Лицензия»».
- **Ссылки.** Новый `src/lib/links.ts`:
  `SUPPORT_URL = "https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md"`,
  `LICENSE_URL = "https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE"`, функции `openSupport()` и
  `openLicense()` через `@tauri-apps/plugin-opener`. Другие адреса в коде не появляются.
- **Левая панель.** Низ: «Настройки», «Поддержать проект», «Справка», «О программе». Ссылка поддержки — кнопка
  класса `link`, вызывает `openSupport()`; статус лицензии больше не запрашивается.
- **«О программе».** В списке: Версия, Сборка, Система, Лицензия (`Apache 2.0` кнопкой-ссылкой на `LICENSE_URL`),
  Политика. После ошибок политики — абзац: кнопка «Поддержать проект» и подсказка `support.hint`. Затем пути
  данных и логов, раздел «Обновления» (§4.3), раздел «Диагностика». Строка «Тариф» удаляется.
- **Над вкладками.** `LicenseNotice` заменяется `UpdateNotice` (§4.3).
- **Меню Help (macOS, `useAppMenu.ts`).** Порядок: Руководство, разделитель, Поддержать проект, О программе,
  Собрать диагностику. Подпись — `support.title`.
- **Справка.** `GUIDE_CHAPTERS = ["getting-started", "updates"]` в `src/guide/chapters.ts` и в
  `scripts/i18n-check.mjs`; `helpChapterFor`: вкладка `about` → `updates`, остальные → `getting-started`.

## 4. Автообновление

### 4.1 Поведение

- При запуске фронтенд один раз вызывает `check_update`; если есть новая версия, над вкладками появляется
  уведомление «Доступна версия X» со ссылкой «Подробнее» (открывает «О программе») и кнопкой «×». Ошибки проверки
  и состояния «обновления выключены/не настроены» при запуске ничего не показывают.
- В «О программе» раздел «Обновления»: кнопка «Проверить обновления» → «Установлена последняя версия», либо
  «Доступна версия X» с описанием из релиза и кнопкой «Установить и перезапустить», либо текст причины
  (`errors.updates.managed`, `errors.updates.notConfigured`), либо ошибка сети/сервера баннером с «Повторить».
- Установка: загрузка с повторами (до трёх попыток при сетевой ошибке, пауза 1 с и 2 с), проверка подписи,
  установка в блокирующем потоке, `app.restart()`. Подтверждения «остановить сессии» нет: у приложения нет
  длительных операций, открытые подключения закрываются при выходе, как сейчас в `RunEvent::Exit`.
- Политика: `updatesEnabled = false` — ни одного запроса к серверу, `check_update` возвращает `disabled` с
  `errors.updates.managed`; `updateUrl` подменяет адрес `latest.json`; адрес `http://` уже сейчас считается
  ошибкой политики и выключает обновления.
- Сборка с заглушкой ключа (dev, пробный прогон): `check_update` возвращает `disabled` с
  `errors.updates.notConfigured`, `install_update` — ошибку с тем же текстом; к серверу обращений нет.

### 4.2 Rust

- `src-tauri/Cargo.toml`: `tauri-plugin-updater = { version = "2.14", default-features = false, features = ["rustls-tls", "system-proxy"] }`.
  `lib.rs`: `.plugin(tauri_plugin_updater::Builder::new().build())`, команды `check_update` и `install_update`
  в `generate_handler!`, в записи лога `policy` добавляется `updates = policy.status.updates_enabled`.
- `commands/app.rs` — перенос из Kafka без подтверждения сессий: `UpdateCheck { Disabled { reason } | UpToDate |
  Available { version, notes, date } }` (serde `tag = "status"`, camelCase), `UpdatePlan`, `PUBKEY_PLACEHOLDER =
  "REPLACE_WITH_UPDATER_PUBKEY"`, `pubkey_configured`, `app_pubkey_configured` (читает `plugins.updater.pubkey` из
  конфигурации приложения), `updater(app, url)` с `on_before_exit` (на Windows плагин запускает установщик и
  вызывает `process::exit`, минуя `RunEvent::Exit`: хук закрывает подключения `runtimes.shutdown()` и вызывает
  `cleanup_before_exit()`), классификация ошибок проверки (`ReleaseNotFound` → `errors.updates.noRelease`,
  `NotFound`; иное → `errors.updates.checkFailed`, `ConnectionFailed`) и загрузки (сетевые → повтор,
  подпись → `errors.updates.badSignature`, иное → `errors.updates.installFailed`), `download_with_retry`,
  `check_update(app, state)`, `install_update(app, state)` (без параметра `force`; при отсутствии обновления —
  `errors.updates.none`).
- Тесты из Kafka, кроме теста подтверждения сессий: `policy_disables_check_without_network`,
  `policy_url_overrides_endpoint`, `update_check_json_shape`, `placeholder_pubkey_is_not_configured`,
  `check_error_classification`, `download_error_classification`, `shipped_config_uses_known_pubkey_and_endpoint`
  (адрес `latest.json` репозитория релизов, `createUpdaterArtifacts = true`, `pubkey` — строка; заглушка
  допускается, потому что ключ владельца появляется после этапа).

### 4.3 Фронтенд

- `api/types.ts`: `UpdateCheck = { status: "disabled"; reason } | { status: "upToDate" } | { status: "available"; version; notes; date }`.
  `api/client.ts`: `checkUpdate()`, `installUpdate()`.
- `components/about/UpdateNotice.tsx` (как в Kafka: `useQuery` с `staleTime: Infinity`, `retry: false`, скрытие
  по «×»), подключается в `Layout.tsx` на месте `LicenseNotice`.
- `AboutPanel.tsx`: раздел `UpdatesSection` как в Kafka без диалога подтверждения: кнопка проверки, состояния
  `disabled`/`upToDate`/`available`, кнопка установки, строка «Загрузка и установка обновления…», ошибки баннером.

### 4.4 Конфигурация и ключ

- `tauri.conf.json`: `bundle.createUpdaterArtifacts = true`; `bundle.windows.wix = { "upgradeCode":
  "a5dce14e-dd43-4ce6-9144-8437d8df8476", "language": ["en-US"] }` (сгенерирован 2026-10-22, после первого релиза
  не меняется); `plugins.updater = { "pubkey": "REPLACE_WITH_UPDATER_PUBKEY", "endpoints":
  ["https://github.com/vvnovg/glafira-postgres-releases/releases/latest/download/latest.json"] }`.
- Ключ updater создаёт владелец после этапа: `pnpm tauri signer generate -w ~/.tauri/glafira-postgres-updater.key`;
  открытая часть кладётся в `plugins.updater.pubkey` отдельным коммитом перед тегом, закрытая — в секреты
  `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`. `scripts/check-release.mjs` считает заглушку ошибкой по тегу и
  предупреждением в пробном прогоне, поэтому релиз с заглушкой невозможен.

## 5. Поддержка проекта

- **`docs/releases-repo/SUPPORT.md`** — текст Kafka с именем «Glafira for PostgreSQL»: английская часть «Support
  the project» (бесплатно и открыто, Apache License 2.0, платных тарифов нет, на что идут деньги — членство в Apple
  Developer Program и время на поддержку и выпуски; GitHub Sponsors `https://github.com/sponsors/vvnovg`), русская
  часть под заголовком «Поддержать проект» с теми же пунктами и «В рублях: https://pay.cloudtips.ru/p/0eee5125».
- **`release.yml`** копирует `SUPPORT.md`, `README.md` репозитория релизов, `LICENSE` и `NOTICE` и собранное
  руководство `guide/` в репозиторий релизов одним коммитом `guide: $TAG` (`git add -A guide README.md SUPPORT.md
  LICENSE NOTICE`). Отличие от Kafka: `LICENSE` и `NOTICE` там добавлялись вручную; здесь их приносит workflow,
  чтобы GitHub опознал репозиторий релизов как Apache-2.0 с первого релиза.
- **`.github/FUNDING.yml`** в этом репозитории: `github: vvnovg` и `custom:` со `SUPPORT_URL`. Такой же файл в
  репозитории релизов кладёт владелец или сессия при создании репозитория — это другой репозиторий.

## 6. Локализация (en, ru)

- **Удаляются** ключи из §2 в обоих словарях. Тест паритета ключей (`src/i18n/locales.test.ts`) — страховка.
- **Добавляются:** `support.title` («Поддержать проект»), `support.hint` (одна фраза: приложение бесплатное и
  открытое, пожертвования помогают его развивать), `about.license` («Лицензия»), `about.updates.{title, check,
  upToDate, available, install, installing}`, `about.notice.{available, more, hideAria}`, `errors.updates.{managed,
  notConfigured, badPolicyUrl, unavailable, noRelease, checkFailed, downloadInterrupted, badSignature, installFailed,
  none}` — тексты из словарей Kafka (en и ru), без `sessionsActive`, `confirmAria` и `stopAndUpdate`.
- **Переписываются:** `about.summary` — последняя фраза «Pro adds prod connections.» / «Pro добавляет
  prod-подключения.» убирается; `hints.connections.environment` — без «(Pro)».
- `docs/i18n/glossary.md`: из «Не переводится» уходят `Pro`, `Free`; строка `license | лицензия` остаётся (строка
  «Лицензия» в «О программе»), строка `plan | тариф` удаляется. `docs/i18n/README.md`: фраза про лицензию убирается,
  пример плюрала — `sidebar.partitions_*`.

## 7. Руководство (en, ru)

- Новая глава `docs/guide/<lang>/updates.md` «Обновления и диагностика» / «Updates and diagnostics» по образцу
  Kafka (`kafka-explorer/docs/guide/<lang>/updates.md`) с разделами `{#updates}`, `{#about}`, `{#diagnostics}`,
  `{#support}`. Отличия от Kafka: нет абзаца про активные сессии и кнопки «Остановить сессии и обновить»;
  в описании `connections.json` — имена, окружения, виды подключений, адреса узлов и REST API Patroni, имена
  пользователей, базы, пути к файлам сертификатов и подмены адресов; в логах — адреса узлов из сообщений об
  ошибках PostgreSQL и Patroni; ссылок на главу «Устранение неполадок» нет (её нет); адрес релизов —
  `https://github.com/vvnovg/glafira-postgres-releases/releases`, адрес поддержки — `SUPPORT_URL`.
  Метки `[[ui:…]]` — только существующие ключи словаря (`pnpm i18n:check` проверяет).
- `getting-started.md` не меняется: ссылок на экран «Лицензия» в нём нет.

## 8. Документация и корневые файлы

- **README.md.** Раздел «Требования» без доступа к `kafka-explorer` и `KAFKA_EXPLORER_TOKEN`; новый раздел
  «Установка» (установщики в `https://github.com/vvnovg/glafira-postgres-releases/releases`, macOS-сборки
  подписаны и нотаризованы, Windows — SmartScreen, для администраторов — `docs/admin.md`); раздел «Лицензия и
  поддержка проекта» (Apache 2.0, все функции бесплатны, `TRADEMARK.md`, `THIRD_PARTY.md`, три входа «Поддержать
  проект» и `SUPPORT_URL`, `CONTRIBUTING.md`, `SECURITY.md`); «Где хранятся данные» без `license.json`;
  в «Что умеет» — дублирование подключения и автообновление.
- **CHANGELOG.md** (новый). Раздел `## 0.1.0` — первый выпуск: подключения трёх видов, строка подключения,
  пароли в хранилище секретов и `~/.pgpass`, проверка подключения по шагам и чек-лист готовности, обзор кластера
  с журналом событий, дерево каталога, папки с экспортом и импортом без паролей, дублирование подключения,
  интерфейс на en и ru с руководством (F1, меню Help), политика администратора (`updatesEnabled`, `updateUrl`,
  `language`), автообновление, «О программе» с архивом диагностики, код под Apache 2.0 и пункт «Поддержать
  проект». Из этого раздела `release.yml` собирает описание релиза.
- **docs/admin.md.** По образцу Kafka: раздел «Установка» (имена файлов установщиков `Glafira_for_PostgreSQL_<версия>_…`,
  тихая установка, исполняемый файл `glafira-pg`), «Обновление установленного приложения», таблица политики с
  `updatesEnabled`, `updateUrl`, `language` (`en`, `ru`) и примечанием про `licenseKey`, «Проверка политики
  вручную (macOS)» с доменом `io.github.vvnovg.glafira.postgres`, «Внутреннее зеркало обновлений», в разделе
  «Сеть» — сервер обновлений (`github.com` и хосты загрузки ассетов) или хост из `updateUrl`; «Данные и логи»
  без `license.json`. Примеры `docs/admin/policy.{json,reg,mobileconfig}`: `updatesEnabled = false`,
  `updateUrl = "https://mirror.example.corp/glafira-postgres/latest.json"`, `language = "en"`.
- **docs/release-checklist.md** — чек-лист Kafka с именами этого продукта: ключ `~/.tauri/glafira-postgres-updater.key`,
  репозиторий релизов, секреты (`TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)`, `RELEASES_TOKEN`, `APPLE_*`), пробный
  прогон, шаги каждого релиза (версии, `cargo update -w --offline`, `third-party.mjs`, `check-release.mjs`, тег),
  «Никогда не менять» (`upgradeCode` из §4.4, `mainBinaryName` `glafira-pg`, `identifier`
  `io.github.vvnovg.glafira.postgres`, ключ updater), смоук установщиков на трёх ОС (подключение к серверу стенда
  `localhost:5460` и к кластеру Patroni), сквозная проверка обновления — по решению владельца.
- **docs/releases-repo/README.md** — «Glafira for PostgreSQL — releases»: что это, ссылка на релизы, руководство
  (English · Русский), раздел про Windows SmartScreen (macOS-сборки подписаны и нотаризованы), «Source code and
  support» со ссылками на `vvnovg/glafira-postgres` и `SUPPORT.md`, строка о товарном знаке PostgreSQL.
- **LICENSE** — канонический текст Apache License 2.0 с apache.org (202 строки, приложение со скобками `[]`),
  копия из `kafka-explorer/LICENSE`. **NOTICE** — «Glafira for PostgreSQL», «Copyright 2026 Viacheslav Novgorodtsev»,
  ссылка на `THIRD_PARTY.md`, строка о товарном знаке PostgreSQL (PostgreSQL Community Association of Canada).
- **TRADEMARK.md** — «Glafira», «Glafira for PostgreSQL» и значок не входят в лицензию; форки и пересобранные
  сборки распространяются под другим именем и значком; упоминание совместимости разрешено; PostgreSQL — товарный
  знак PostgreSQL Community Association of Canada, проект с ней не связан.
- **THIRD_PARTY.md** и **scripts/third-party.mjs** (+ тест) — скрипт Kafka с другим списком C-библиотек:
  OpenSSL 3 (Apache-2.0, `openssl-src`, feature `vendored` у `openssl`/`postgres-openssl`) и AWS-LC
  (ISC, Apache-2.0, BSD-3-Clause, MIT — см. лицензионное выражение `aws-lc-sys`; собирает `aws-lc-sys` для
  `reqwest`/`rustls`). Заголовок — «Glafira for PostgreSQL». Запускается вручную перед релизом после
  `cargo update -w --offline --manifest-path src-tauri/Cargo.toml` (иначе `cargo metadata --locked` падает).
- **CONTRIBUTING.md** — как собрать (ссылка на README), строки интерфейса идут в оба словаря (`docs/i18n/README.md`,
  `glossary.md`), команды проверок перед PR (vitest, `test:scripts`, `i18n:check`, `build`, `cargo fmt`/`clippy`/
  `test --lib` для `src-tauri`, `cargo fmt`/`clippy`/`test` для `crates/pg-advisor`, `cargo deny` для обоих;
  интеграционные тесты — со стендом по README), `cargo-deny` версии 0.20.2, DCO: `git commit -s`.
- **SECURITY.md** — поддерживается последняя версия в репозитории релизов; уязвимости — через «Report a vulnerability»;
  ответ в течение 7 дней; особенно важны пароли и `~/.pgpass`, хранилище секретов, логи и архив диагностики,
  проверка TLS, политика администратора, updater.
- **deny.toml** в корне: `[licenses] allow` как у Kafka (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception,
  BSD-2-Clause, BSD-3-Clause, ISC, Zlib, 0BSD, CC0-1.0, Unlicense, BSL-1.0, MIT-0, Unicode-DFS-2016, Unicode-3.0,
  MPL-2.0, CDLA-Permissive-2.0), `confidence-threshold = 0.8`, `unused-allowed-license = "allow"`,
  `[licenses.private] ignore = true`; комментарий про C-библиотеки OpenSSL 3 и AWS-LC.
- **.gitattributes** — `* text=auto eol=lf`, `*.reg text eol=crlf` (CRLF на Windows ломает импорт `scripts/*.mjs`
  в vitest). **.gitignore** — `docs/licensing-review-2026-10-19.md`, `*.glafira-postgres-connections.json`
  (экспорты подключений с адресами реальных кластеров).

## 9. CI и релиз

- **ci.yml.** Триггер `push` получает `tags: ["v*"]`. Удалённые шаги — §2. После clippy — шаг «Лицензии
  зависимостей»: `cargo deny --version | grep -q ' 0\.20\.2$' || cargo install cargo-deny --locked --version
  0.20.2 --force`, затем `cargo deny --manifest-path <крейт> --config deny.toml check licenses` для `src-tauri` и
  `crates/pg-advisor`. Job `platforms` (Windows, macOS) идёт на каждый push и PR, как сейчас: минуты на публичном
  репозитории бесплатны, а сборка на Windows ловит ошибки, которых нет на macOS и Linux.
- **release.yml** — workflow Kafka с `RELEASES_REPO: vvnovg/glafira-postgres-releases`: job `check` (тесты скриптов,
  сборка руководства, `check-release.mjs` по тегу или `--dry-run` вручную, секрет updater задан), `build` на
  `macos-14` (aarch64), `macos-15-intel` (x86_64), `windows-latest`, `ubuntu-22.04` без кэшей, с шагом «Подпись
  Apple задана?» (все шесть `APPLE_*` или ни одного), `tauri-action` той же закреплённой версии, переименование
  macOS-архива updater с архитектурой, `.pkg` через `scripts/make-pkg.sh`, тихая установка MSI как проверка,
  артефакты; `assemble` (плоский набор, имена без пробелов, описание из раздела `## <версия>` `CHANGELOG.md`,
  `make-latest-json.mjs --require-all`); `publish` по тегу (`gh release create --draft` → `--draft=false --latest`,
  затем `guide/`, README, SUPPORT, LICENSE, NOTICE в репозиторий релизов, §5). Системные зависимости Linux — как в
  `ci.yml` этого репозитория плюс `rpm`; cmake и NASM не нужны (комментарий из `ci.yml` переносится).
- **Скрипты** (из Kafka, с тестами `node --test`): `check-release.mjs` (версии в трёх манифестах равны тегу,
  `pubkey` не заглушка, раздел `CHANGELOG`), `make-latest-json.mjs` (записи для darwin-aarch64, darwin-x86_64,
  windows-x86_64, linux-x86_64-{appimage,deb,rpm} и linux-x86_64), `make-pkg.sh`, `guide-publish.mjs`
  (README руководства «Glafira for PostgreSQL — User guide», языки из `src/i18n/languages.ts`, главы из
  `GUIDE_CHAPTERS` в `i18n-check.mjs`), `check-dist-compat.mjs` (синтаксис и API новее Safari 14 в `dist/assets`;
  `minimumSystemVersion` — macOS 11). `package.json`: `build` = `tsc && vite build && node scripts/check-dist-compat.mjs
  dist/assets`, `guide:publish`.

## 10. Тесты

- **Rust, удаляются:** тесты модуля `license`, `policy_license_key_reaches_manager` (`state.rs`),
  `archive_does_not_contain_license_token` (`diagnostics.rs`), `pro_can_pick_any_language` (`commands/settings.rs`).
- **Rust, переворачиваются:** `commands/connections.rs` — prod создаётся и становится доступным для записи без
  ключа; `commands/folders.rs` и `storage/exchange.rs` — импорт prod сохраняет `readOnly` из файла, предупреждения
  про read-only нет; `policy/mod.rs` — `licenseKey` неизвестен и игнорируется (без ошибки, обновления не
  выключаются; соседние ключи применяются); `diagnostics.rs` — `info.json` без `license`; `settings.rs` —
  `priority_without_license` переименовывается в `language_priority`.
- **Rust, новые:** `remove_legacy_license_file` (файл есть → удалён, повторный вызов — тихий); тесты updater (§4.2);
  `documented_examples_parse` по новым примерам. Тесты не используют Unix-пути вида `/nonexistent/…`: на Windows
  они не абсолютны (временный каталог через `tempfile`).
- **Фронтенд:** удаляются `LicensePanel.test.tsx`; обновляются `Sidebar.test.tsx` (порядок ссылок, кнопка
  поддержки вызывает `openUrl` с `SUPPORT_URL`), `AboutPanel.test.tsx` (строка лицензии, кнопка поддержки, раздел
  «Обновления»: проверка → состояния, установка), `ErrorBanner.test.tsx`, `ConnectionForm.test.tsx` и
  `ConnectionFormTab.test.tsx` (prod доступен новому подключению и копии, флажок снимается), `ImportDialog.test.tsx`,
  `client.test.ts`, `workspace.test.ts`, `Layout.test.tsx`, `useAppMenu.test.ts` (новый пункт и порядок),
  `helpChapterFor.test.ts`; новые `UpdateNotice.test.tsx`, `links` покрывается через вызовы `openUrl`.
- **Скрипты:** тесты Kafka для `check-release`, `make-latest-json`, `guide-publish` (языки en/ru, две главы),
  `check-dist-compat`, `third-party` (сортировка по кодовым точкам, без dev-зависимостей, C-библиотеки этого
  продукта).
- **Проверки перед финальным ревью:** `pnpm test`, `pnpm test:scripts`, `pnpm i18n:check`, `pnpm build`,
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` и `cargo test --lib` для `src-tauri`,
  `cargo fmt`/`clippy`/`test` для `crates/pg-advisor`, `cargo deny check licenses` для обоих с `deny.toml`,
  `node scripts/third-party.mjs` детерминирован (повторный запуск не меняет файл), `node scripts/check-release.mjs
  --dry-run v0.1.0` — только предупреждение о заглушке ключа; grep по `src`, `src-tauri/src`, `src/locales`,
  `docs/guide`, `README.md`, `docs/admin.md`, `docs/i18n` — ни `license-format`, ни `ProRequired`, ни `ProLock`,
  ни `\bPro\b`, ни `KAFKA_EXPLORER_TOKEN`; `license` только в `about.license`, `LICENSE_URL`, `migrate.rs`,
  ветке `licenseKey` и разделах о лицензии кода.

## 11. Смоук (`docs/smoke/apache-2.md`)

Сборка `pnpm tauri dev`, стенд `docker compose -f dev/docker-compose.yml up -d`. Каталог данных — строка «Данные:»
в «О программе».

1. **Старый ключ.** Закрыть приложение, положить в каталог данных `license.json` с любым содержимым, запустить:
   файла нет, в логе запись уровня info «legacy license file removed», записей warn про него нет.
2. **prod без ключа.** Новое подключение: в поле «Окружение» вариант `prod` доступен и подписан `prod`; после выбора
   флажок «Только чтение» установлен, его можно снять до сохранения. Сохранить. У сохранённого prod-подключения
   с флажком открыть форму, снять флажок, сохранить — сохраняется. «Дублировать подключение» у prod-подключения:
   в копии `prod` доступен.
3. **Импорт prod.** prod-подключение без «Только чтение» экспортировать («Экспортировать всё…»), удалить,
   импортировать: в превью нет предупреждения про read-only, импортированное подключение доступно для записи.
4. **Три входа «Поддержать проект».** Внизу левой панели ссылка «Поддержать проект» (порядок: Настройки, Поддержать
   проект, Справка, О программе); в «О программе» кнопка с подсказкой; на macOS пункт в меню Help между разделителем
   и «О программе». Каждый вход открывает в браузере
   https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md (до публикации репозитория — 404, важно,
   что открылся именно этот адрес).
5. **«О программе».** Строка «Лицензия: Apache 2.0» открывает https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE;
   строки «Тариф» нет; ссылки «Лицензия» внизу панели нет.
6. **Обновления в dev-сборке.** При запуске уведомления над вкладками нет. «О программе» → «Проверить обновления» →
   «Обновления не настроены в этой сборке»; в логе нет запросов к `github.com`.
7. **Политика** (macOS): собрать plist по `docs/admin.md` («Проверка политики вручную») с `updatesEnabled = false`
   и ключом прошлых версий `plutil -insert licenseKey -string abc /tmp/policy.plist`. После перезапуска «О программе»:
   «Политика организации: задана», ошибок нет, «Проверить обновления» → «Обновления управляются вашей организацией»;
   в логе `unknown policy key “licenseKey” ignored`. Снять политику после проверки.
8. **F1 и руководство.** На вкладке «О программе» нажать F1: открывается глава «Обновления и диагностика» с разделами
   «Обновления», «О программе», «Архив диагностики», «Поддержать проект»; в оглавлении две главы.
9. **Языки.** Переключить язык на `en`: «Support the project», подсказка в «О программе», раздел «Updates» и меню Help
   на macOS переведены; вернуть `ru`.
10. **Архив диагностики.** «Собрать диагностику» → в `info.json` есть `policy`, нет поля `license`; паролей нет.

## 12. Риски

- **Updater не проверяется сквозным обновлением до первого релиза:** предыдущей версии нет. Код — перенос
  проверенного в Kafka (v0.2.0 → v0.3.0 на macOS), пробный прогон `release.yml` подтверждает подписи и `latest.json`.
  Сквозная проверка по чек-листу — по решению владельца; Windows и Linux пути updater впервые сработают при релизе 0.2.0.
- **Заглушка ключа updater в ветке.** Релиз с ней невозможен (`check-release.mjs`), сборка dev честно сообщает
  «не настроены». Ключ владелец создаёт после слияния.
- **Лицензии C-кода** (OpenSSL 3, AWS-LC) `cargo deny` не видит — они перечислены вручную в `THIRD_PARTY.md`
  и в комментарии `deny.toml`; обновление зависимостей требует обновить список (шаг чек-листа).
- **Объём переноса из Kafka** (workflow, 6 скриптов с тестами, updater, документация). Смягчение: образцы
  копируются и правятся, а не пишутся заново; тесты скриптов переносятся вместе с ними.
- **Политики с `licenseKey`** у организаций отсутствуют (релизов не было): предупреждения в логе достаточно.

## 13. Вне объёма

Создание и публикация репозиториев GitHub, секреты (`APPLE_*`, `TAURI_SIGNING_PRIVATE_KEY*`, `RELEASES_TOKEN`),
генерация ключа updater и его коммит, пробный релиз, тег v0.1.0, `FUNDING.yml`/`SUPPORT.md` в репозитории релизов до
первого релиза — отдельные шаги после слияния (ведёт та же сессия по чек-листу). Остальные 10 языков (P6), подпись
Windows, функции P1b, новые запреты политики, глава «Устранение неполадок».
