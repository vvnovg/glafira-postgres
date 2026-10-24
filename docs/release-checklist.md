# Чек-лист релиза

## Один раз (владелец)
1. Ключ updater: `pnpm tauri signer generate -w ~/.tauri/glafira-postgres-updater.key`. Открытый ключ — в `src-tauri/tauri.conf.json` →
   `plugins.updater.pubkey` вместо заглушки `REPLACE_WITH_UPDATER_PUBKEY`, отдельным коммитом в `main`.
2. Публичный репозиторий `vvnovg/glafira-postgres-releases`: README из `docs/releases-repo/README.md`, `SUPPORT.md` из
   `docs/releases-repo/SUPPORT.md`, `LICENSE`, `NOTICE` и `.github/FUNDING.yml` (как в этом репозитории). Каждый релиз
   обновляет README, SUPPORT.md, LICENSE, NOTICE и `guide/` сам (`release.yml`). `LICENSE` и `NOTICE` в корне этого
   репозитория должны быть закоммичены до первого тега: иначе шаг публикации руководства (`continue-on-error`) пропустит с ошибкой шага, не останавливающей релиз,
   обновление репозитория релизов.
3. Секреты репозитория `vvnovg/glafira-postgres` (Settings → Secrets and variables → Actions):
   `TAURI_SIGNING_PRIVATE_KEY` (содержимое файла ключа), `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`,
   `RELEASES_TOKEN` (fine-grained PAT: только `vvnovg/glafira-postgres-releases`, Contents: Read and write).
   Срок токена — 1 год; напоминание в календаре за 2 недели. Истёкший токен ломает только шаг publish:
   создать новый с теми же правами и `gh secret set RELEASES_TOKEN --repo vvnovg/glafira-postgres`, затем перезапустить job publish.
   Подпись Apple — все шесть или ни одного (с частью секретов сборка падает): `APPLE_CERTIFICATE` (.p12 в base64),
   `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` («Developer ID Application: Viacheslav Novgorodtsev (JHWW538CH9)»),
   `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID` (`JHWW538CH9`); при наличии — `APPLE_INSTALLER_IDENTITY`
   (Developer ID Installer) для подписи `.pkg`.
4. Резервная копия закрытого ключа updater вне компьютера разработки.
5. В настройках репозитория `vvnovg/glafira-postgres` после публикации: Settings → Code security → включить «Private vulnerability reporting»
   (это обещает `SECURITY.md`); установить приложение DCO (https://github.com/apps/dco) — проверка `Signed-off-by` в PR,
   которую обещает `CONTRIBUTING.md`.

## Пробный прогон
Нужен `release.yml` в ветке `main`. `gh workflow run release -f dry_run=true` собирает все платформы и `latest.json`
без публикации. `node scripts/check-release.mjs --dry-run vX.Y.Z` превращает проблему с ключом updater в предупреждение;
сборка по тегу всегда строгая.

## Каждый релиз
1. Версия в `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`; раздел `## X.Y.Z` в `CHANGELOG.md`
   покрывает всё с прошлого тега — из него собирается описание релиза (накопленный раздел `## Unreleased` переименовать в `## X.Y.Z`).
2. `cargo update -w --offline --manifest-path src-tauri/Cargo.toml` — новая версия попадает в `Cargo.lock`
   (без этого `cargo metadata --locked` в скрипте ниже падает), затем `node scripts/third-party.mjs` →
   обновлённый `THIRD_PARTY.md` (лицензии зависимостей); закоммитить. При обновлении зависимостей сверить список
   C-библиотек в `scripts/third-party.mjs` (OpenSSL 3 от `openssl-src`, AWS-LC от `aws-lc-sys`).
3. `node scripts/check-release.mjs vX.Y.Z` → «можно выпускать».
4. `git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z` → workflow `release`. Релиз создаётся черновиком со всеми ассетами
   и публикуется атомарно (`--draft=false --latest`); updater читает `releases/latest/download/latest.json`
   (артефакты updater — формат v2).
5. Смоук на трёх ОС (ниже) по установщикам из релиза.

## Никогда не менять
- `bundle.windows.wix.upgradeCode` в `src-tauri/tauri.conf.json` (`a5dce14e-dd43-4ce6-9144-8437d8df8476`):
  по нему MSI новой версии обновляет установленную, а не ставится рядом. Без явного значения WiX выводит код из
  имени продукта, поэтому он задан явно и не меняется даже при переименовании продукта.
- `mainBinaryName` (`glafira-pg`) и `identifier` (`io.github.vvnovg.glafira.postgres`): от них зависят путь
  установки, имя процесса, данные, keychain и политика.
- Ключ updater (`plugins.updater.pubkey`): со сменой ключа все установленные версии перестанут принимать обновления.

## Известные ограничения
- `.pkg` подписывается Developer ID Installer, если задан `APPLE_INSTALLER_IDENTITY`, но отдельно не нотаризуется.
- Установщик Windows не подписан: SmartScreen предупреждает при первом запуске.
- Локальный `pnpm tauri build` с `createUpdaterArtifacts` требует `TAURI_SIGNING_PRIVATE_KEY`: без него не подписан только `.app.tar.gz`,
  а `.app` и `.dmg` в `src-tauri/target/release/bundle/` годны для смоука.

## Смоук установщиков
Стенд: `dev/ssh/gen-keys.sh`, `docker compose -f dev/docker-compose.yml up -d` и `scripts/wait-stand.sh` (README → «Стенд»).
- [ ] macOS (`.dmg` и `.pkg`): установка; из `.dmg` запуск без предупреждений Gatekeeper (сборка подписана и нотаризована),
      `.pkg` без отдельной нотаризации может предупредить; подключение к серверу `localhost:5460` (`glafira_diag` / `glafira-secret`)
      и к кластеру Patroni (`http://localhost:8011`), «О программе» — версия.
- [ ] Windows (`.msi`, `msiexec /qn` из администраторской консоли): то же; в «Программах и компонентах» одна запись.
- [ ] Linux (`.deb`, `.rpm`, `.AppImage`): то же; бинарь `/usr/bin/glafira-pg`.
- [ ] SSH-туннель через `bastion` стенда (`localhost:2222`, пользователь `tester`; ключи — `dev/ssh/keys/`, пароль — `tester-secret`, пассфраза — `test-passphrase`):
      подключение сервера и кластера Patroni с входом по агенту, по ключу, по ключу с пассфразой и по паролю; диалог «Неизвестный ключ хоста» →
      «Доверять и подключиться» → подключение; при закрытии приложения процессы `ssh` завершаются. **Windows — только вручную перед релизом:**
      пароль и пассфраза через askpass при установке в каталог с пробелами (`C:\Program Files\…`; бинарь GUI-подсистемы), диалог ключа хоста,
      завершение процессов `ssh` при выходе (PowerShell) и очистка сирот при следующем запуске — в том числе под пользователем Windows
      с кириллицей в имени профиля (`C:\Users\Иван\…`: логи `ssh-<id>.log` лежат в его каталоге, а PowerShell выводит в OEM-кодировке).
- [ ] Политика `updatesEnabled = false` (по `docs/admin.md`): «Обновления управляются вашей организацией», в логе нет запросов к серверу обновлений.
- [ ] Политика с `updateUrl: "http://…"`: в «О программе» ошибка политики, обновления выключены.
- [ ] «Собрать диагностику» → архив открывается, паролей нет.
- [ ] Пути данных и логов из `docs/admin.md` совпадают с «О программе» на каждой ОС.
- [ ] Обновление (начиная со второго релиза): установить предыдущую версию → «Проверить обновления» → «Установить и перезапустить» →
      новая версия, подключения и настройки на месте (для `.deb` и `.rpm` — отдельно: ставится пакет того же типа,
      запрашивается пароль администратора).

## Сквозная проверка обновления (по решению владельца; рекомендуется до первого релиза с updater или при изменениях updater)
Цель — до тега убедиться, что выпущенная версия сможет обновиться. Нужны боевой ключ updater (pubkey уже в
`tauri.conf.json`) и секреты `TAURI_SIGNING_PRIVATE_KEY*` — шаги выполняет владелец.

1. **Старая версия (A).** Ветка с версией выпуска, например `0.1.0`. Пробный прогон: `gh workflow run release -f dry_run=true`;
   скачать артефакт `release-v0.1.0` (установщики A).
2. **Новая версия (B).** Временная ветка от той же ревизии, версия `0.1.1` в `package.json`, `src-tauri/Cargo.toml`,
   `src-tauri/tauri.conf.json` и раздел `## 0.1.1` в `CHANGELOG.md` (ветку не сливать). Пробный прогон из неё
   (`gh workflow run release --ref <ветка> -f dry_run=true`, workflow должен быть в `main`) → артефакт `release-v0.1.1`.
3. **Тестовое зеркало (https).** Один из вариантов:
   - тестовый релиз в отдельном публичном репозитории (например, `vvnovg/glafira-postgres-updater-test`):
     `gh release create v0.1.1 <ассеты B> --repo …`, затем пересобрать `latest.json` с адресами этого репозитория:
     `node scripts/make-latest-json.mjs --version 0.1.1 --notes-file notes.md --base-url https://github.com/<repo>/releases/download/v0.1.1 --dir <ассеты B> --require-all`;
   - статический https-сервер в локальной сети (с доверенным сертификатом) с теми же файлами и `latest.json`,
     пересобранным с его `--base-url`.
   Проверить: `curl -fsSL <url>/latest.json` открывается, `curl -fsSI` по каждому `url` из него — 200.
4. **Политика с зеркалом** на каждой тестовой машине (`docs/admin.md` → «Политика»): `updateUrl` = адрес
   `latest.json` зеркала (только `https://`).
5. **На каждой ОС и каждом типе установки** — macOS `.dmg` и `.pkg` (aarch64; x64 — если есть машина),
   Windows `.msi`, Linux `.deb`, `.rpm`, `.AppImage`:
   - [ ] установить A, создать подключение с паролем и открыть вкладку «Обзор» (подключение открыто);
   - [ ] «О программе» → «Проверить обновления» → «Доступна версия 0.1.1»;
   - [ ] «Установить и перезапустить»;
   - [ ] после перезапуска «О программе» показывает 0.1.1; подключения, пароли и настройки на месте;
   - [ ] на сервере PostgreSQL после перезапуска нет «зависших» сессий `application_name = glafira-pg` от прежнего процесса
         (`SELECT pid, state FROM pg_stat_activity WHERE application_name = 'glafira-pg';`) — на **Windows** это проверка хука
         выхода (`on_before_exit` закрывает подключения перед запуском MSI);
   - [ ] Windows: в «Программах и компонентах» одна запись версии 0.1.1 (upgradeCode сработал), установка
         требует UAC; под пользователем без прав администратора — понятная ошибка, A продолжает работать;
   - [ ] macOS `.pkg` (приложение в `/Applications` принадлежит root): запрос пароля администратора, после
         установки владелец и права `.app` не изменились неожиданно (`ls -ld /Applications/Glafira*.app`);
         отказ от ввода пароля — ошибка, A продолжает работать. macOS `.dmg` (приложение перетащено
         пользователем): обновление без пароля;
   - [ ] Linux `.deb`/`.rpm`: запрос пароля (pkexec), `dpkg -l | grep glafira` / `rpm -q` показывает 0.1.1;
         `.AppImage`: файл заменён на месте, права на исполнение сохранены.
6. **Отрицательные проверки:** `latest.json` с испорченной подписью → ошибка установки, A работает;
   политика `updatesEnabled = false` → проверки нет.
7. Удалить временную ветку и тестовый релиз/зеркало. Только после этого ставить тег.
