# Glafira for PostgreSQL: руководство администратора

## Политика
| ОС | Где |
|---|---|
| macOS | профиль конфигурации MDM, домен `io.github.vvnovg.glafira.postgres` — пример `admin/policy.mobileconfig` |
| Windows | `HKLM\SOFTWARE\Policies\Glafira\PostgreSQL` — пример `admin/policy.reg` |
| Linux | `/etc/io.github.vvnovg.glafira.postgres/policy.json` — пример `admin/policy.json` |

| Ключ | Тип | По умолчанию | Смысл |
|---|---|---|---|
| `licenseKey` | строка | — | ключ организации: Pro у всех пользователей |
| `language` | строка: `en`, `ru` | язык ОС или выбор пользователя | язык интерфейса для всех; пользователь его не меняет. Неизвестный код — ошибка политики |

Политика читается один раз при запуске: после изменения перезапустите приложение. Неизвестные ключи игнорируются.
На macOS читается только политика уровня устройства: профиль должен быть установлен на устройство (канал устройства MDM,
`PayloadScope` = `System`), тогда он окажется в `/Library/Managed Preferences/io.github.vvnovg.glafira.postgres.plist`.
Профиль уровня пользователя (`/Library/Managed Preferences/<пользователь>/…`) приложение не читает.
Значения в реестре Windows — строки (`REG_SZ`). Состояние политики и её ошибки
(нечитаемый файл, неверный тип значения, неизвестный язык) видны в окне «О программе».

## Роль для диагностики
```sql
CREATE ROLE glafira_diag LOGIN PASSWORD '…';
GRANT pg_monitor TO glafira_diag;
GRANT pg_signal_backend TO glafira_diag;   -- отмена и завершение сессий
GRANT pg_maintain TO glafira_diag;         -- PostgreSQL 17+: VACUUM и ANALYZE
-- в каждой БД, где нужен анализ запросов (и pg_stat_statements в shared_preload_libraries):
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
```
В `pg_hba.conf` нужна строка, разрешающая роли вход с машин пользователей (`scram-sha-256`).

Роль нужна для полной диагностики; без неё приложение подключается, но чек-лист готовности в проверке подключения
покажет, чего не хватает, и подскажет точную команду (`GRANT … TO "glafira_diag";`). В P1 приложение само ничего не изменяет:
`pg_signal_backend` и `pg_maintain` чек-лист проверяет заранее. `pg_maintain` есть только в PostgreSQL 17 и новее;
в более ранних версиях `VACUUM` и `ANALYZE` могут запускать владелец таблицы, владелец базы или суперпользователь.
`CREATE EXTENSION` выполняет суперпользователь; `shared_preload_libraries` меняется только с перезапуском сервера.

### Настройки сервера, которые проверяет чек-лист
| Параметр | Значение | Зачем |
|---|---|---|
| `shared_preload_libraries` | содержит `pg_stat_statements` | статистика запросов; на каждом узле |
| `compute_query_id` | `on` (PostgreSQL 14+; `auto` подходит, если `pg_stat_statements` загружен на всех узлах) | связь запросов в `pg_stat_activity` и `pg_stat_statements` |
| `track_io_timing` | `on` | время ввода-вывода запросов (предупреждение, если выключено) |
| `track_activity_query_size` | не меньше `4096` | длинные тексты запросов не обрезаются (предупреждение) |

Необязательные расширения, которые приложение использует, если они установлены в базе:
`hypopg`, `pgstattuple`, `pg_buffercache`, `pg_wait_sampling`, `pg_qualstats`, `pg_stat_kcache`
(`CREATE EXTENSION IF NOT EXISTS <имя>;`, если пакет расширения установлен на сервере).

### Patroni
Роль и расширения создаются на лидере и доезжают до реплик репликацией. Параметры PostgreSQL в кластере Patroni задаёт
Patroni (`patronictl edit-config`, затем `patronictl restart` для `shared_preload_libraries`), а не `postgresql.conf` на узле.
Если в конфигурации Patroni есть раздел `postgresql.pg_hba`, Patroni сам пишет `pg_hba.conf` — строку для роли добавляйте туда.

## Сеть
| Куда | Порт | Зачем |
|---|---|---|
| Каждый узел PostgreSQL | 5432 (или ваш) | каталог, топология списка узлов, проверка подключения |
| REST API Patroni каждого узла | 8008 (или ваш) | обнаружение участников (`GET /cluster`, `GET /patroni`) |

Приложение вызывает только `GET /cluster` и `GET /patroni`; basic auth передаётся, если задан в профиле. В Patroni
(проверено на 4.1.5) `restapi.allowlist`, обязательная basic auth для небезопасных вызовов и `restapi.verify_client: optional`
действуют только на изменяющие запросы (PATCH, PUT, POST, DELETE), а чтение они не закрывают, поэтому для Glafira
`restapi.allowlist` менять не нужно. Не добавляйте в него адреса и подсети пользователей: он защищает switchover, restart и reinitialize.
Чтение закрывает только `restapi.verify_client: required` (TLS-рукопожатие требует клиентский сертификат): тогда выдайте
пользователям клиентский сертификат и укажите его в профиле кластера в разделе «TLS для API» — профиль поддерживает HTTPS,
свой CA и клиентский сертификат. Если Patroni отдаёт адреса, недоступные с машин пользователей (pod IP, NAT), в профиле
кластера задаётся подмена адресов участников.
SSH-туннели через бастион — в следующей версии.

Приложение не отправляет телеметрию и не загружает удалённое содержимое: сетевые соединения — только с серверами PostgreSQL
и REST API Patroni из профилей подключений.

### Как приложение выглядит на сервере
- Соединения идут с `application_name = glafira-pg`: по нему их видно в `pg_stat_activity`.
- Сессии открываются только на чтение: `default_transaction_read_only = on`, `statement_timeout = 5s`, `lock_timeout = 1s`.
- Не больше 3 соединений на каждую связку «подключение, узел, база»; простаивающее закрывается через 5 минут. Учитывайте их в `max_connections` и в лимитах роли.
- Проверка подключения («Проверить») не использует этот пул: она открывает свои короткоживущие соединения к узлам (на каждом узле
  несколько за время проверки) и закрывает их по окончании. Запас в `max_connections` и в лимитах роли нужен и для них.
- Топология опрашивается раз в 10 секунд.

## Данные и логи
| | macOS | Windows | Linux |
|---|---|---|---|
| Данные | `~/Library/Application Support/io.github.vvnovg.glafira.postgres` | `%APPDATA%\io.github.vvnovg.glafira.postgres` | `~/.local/share/io.github.vvnovg.glafira.postgres` |
| Логи | `~/Library/Logs/io.github.vvnovg.glafira.postgres` | `%LOCALAPPDATA%\io.github.vvnovg.glafira.postgres\logs` | `~/.local/share/io.github.vvnovg.glafira.postgres/logs` |
| Пароли | Keychain, сервис `glafira-postgres` | Credential Manager | Secret Service |

В каталоге данных лежат `connections.json` (профили без паролей), `settings.json` и, если ключ введён вручную, `license.json`.
Если пароль в хранилище не сохранён, используется `~/.pgpass` (Windows: `%APPDATA%\postgresql\pgpass.conf`) или файл из `PGPASSFILE`.
На Unix у такого файла должны быть права 0600 (`chmod 0600 ~/.pgpass`): как в libpq, файл, доступный группе или всем
(`mode & 0o077`), игнорируется, и в журнале остаётся только предупреждение.
Окно «О программе» показывает эти каталоги и собирает диагностический архив (версия, логи, профили без паролей);
приложение ничего не отправляет — архив пользователь прикладывает к обращению сам.
