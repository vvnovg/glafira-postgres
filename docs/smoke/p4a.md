# Смоук P4a «Шлюз и диагностика» — MCP-шлюз, Qwen Code, Claude Code

Сборка: `pnpm tauri build`; `.app` из `src-tauri/target/release/bundle/macos/` скопировать в `/Applications` (без
`TAURI_SIGNING_PRIVATE_KEY` не подписан только `.app.tar.gz`, `.app` для смоука годен). Шим запускает установленное приложение;
из `pnpm tauri dev` он приложение не запускает (спека §3.3). Клиенты: Qwen Code (`npm install -g @qwen-code/qwen-code`) и
Claude Code (`claude`).

Стенд: `docker compose -f dev/docker-compose.yml up -d --no-recreate` и `scripts/wait-stand.sh`; `pg-perf` —
`docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf` и `scripts/wait-perf.sh`; нагрузка на полчаса —
`docker exec -d gp-pg-perf pgbench -n -U postgres -d perf -f /perf/heavy.sql -c 4 -T 1800`. Никогда `docker compose down`.

Каталог данных — `$DATA` = `~/Library/Application Support/io.github.vvnovg.glafira.postgres`. Подключения (окружение `dev`, роль
`glafira_diag` / `glafira-secret`): `lab` — `localhost:5470`, база `orders`; `demo` — кластер Patroni `http://localhost:8011`
с подменой адресов участников (README → «Стенд»); `perf` — `localhost:5471`, база `perf`. Вопросы ассистенту задаются в Qwen
Code обычным текстом; что он вызвал — видно в его выводе и в «Настройки» → «LLM-ассистенты (MCP)» → «Журнал» («Обновить»).

1. **Включение.** «Настройки» → «LLM-ассистенты (MCP)»: «Разрешить MCP-сервер» снят, «Сервер выключен», файла `$DATA/mcp.json`
   нет. Отметить → «Сервер работает»; `ls -l "$DATA/mcp.json" "$DATA/mcp.sock"` — `-rw-------` и `srw-------`; в «Подключить
   клиент» шесть фрагментов с путём `/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg`; «Копировать» кладёт
   фрагмент в буфер. «Открыто для LLM: 0».
2. **Уровень подключения.** Форма `lab` → группа «Доступ для LLM»: выбрано «Нет». «Диагностика» → «Сохранить»: в дереве у `lab`
   значок «LLM», раскрытый каталог `lab` не свернулся (подключение не переоткрывалось). То же для `demo` и `perf`: «Открыто для
   LLM: 3». Экспорт всех подключений в файл → `grep -c llmAccess <файл>` — 0; импорт этого файла в новую папку как новых
   подключений — у импортированных значка «LLM» нет.
3. **Qwen Code: шим запускает Glafira.** Фрагмент «Qwen Code: команда» выполнить в терминале. Закрыть Glafira (⌘Q): `mcp.json` и
   `mcp.sock` удалены. Запустить `qwen` в пустом каталоге: Glafira открывается сама не дольше чем за 25 секунд, поверх окна —
   диалог «qwen-code … хочет подключиться к Glafira» (имя и версия — как сообщает клиент) с «открытых для LLM-ассистентов: 3»;
   фокус на диалоге, а не на «Разрешить». Вопрос ассистенту до ответа — ошибка `client_not_approved`. «Разрешить» → клиент в
   «Подтверждённых клиентах», внизу боковой панели «LLM: qwen-code». `/mcp` в Qwen Code — сервер glafira-pg и 14 инструментов.
4. **Кластер.** «Что происходит на кластере demo?» → в журнале `list_connections`, `get_health_summary`, затем `get_topology` или
   `run_diagnostic`; ответ называет лидера и реплики; находки в ответе инструмента — английскими текстами. Во время вызова
   индикатор внизу панели подсвечен.
5. **Горизонт и маскирование.** В терминале:
   `docker exec -d gp-pg-lab sh -c "(echo \"BEGIN; SELECT txid_current(); SELECT 'secret-literal-42' AS marker, 4242;\"; sleep 900) | psql -U postgres -d orders"`.
   «Кто держит горизонт xmin на lab? Процитируй текст его запроса дословно» → ассистент называет pid сессии idle in transaction
   роли `postgres`; в цитате вместо `'secret-literal-42'` и `4242` — `$1` и `$2`; в журнале у вызова — «замаскировано: 2» (или
   больше). Ассистент не завершает сессию сам, а говорит, где это сделать в Glafira.
6. **Блокировки.** `docker exec -d gp-pg-lab sh -c "(echo 'BEGIN; LOCK TABLE shop.orders IN ACCESS EXCLUSIVE MODE;'; sleep 300) | psql -U postgres -d orders"`
   и `docker exec -d gp-pg-lab psql -U postgres -d orders -c "SELECT count(*) FROM shop.orders WHERE id = 777"`. «Есть ли
   блокировки на lab и кто корневой?» → `run_diagnostic` `locks`: корневой — сессия с `LOCK TABLE`, ждущий — `SELECT`, в его
   тексте `777` заменено на `$1`.
7. **Наблюдение.** «Понаблюдай за активностью lab 30 секунд» → `observe_activity`: в Qwen Code идёт прогресс, вызов длится не
   больше 36 секунд, в ответе — ожидания и главные запросы (тексты замаскированы).
8. **Топ запросов.** На `perf` открыть «Диагностика» → «Запросы», режим «За 15 мин», подождать 3 минуты. «Что грузит perf
   последние 15 минут?» → `get_top_queries` в режиме окна, ответ называет запросы `heavy.sql` и что окно покрыто не целиком.
   Тот же вопрос про `lab` (вкладка «Запросы» у `lab` не открывалась) → ответ инструмента с покрытием 0 и подсказкой, ассистент
   переходит на `observe_top_queries` или «с момента сброса».
9. **План и гипотеза.** «Покажи план самого тяжёлого запроса из топа perf» → `explain_query` по `queryId` (PostgreSQL 17 —
   GENERIC_PLAN), в ответе текст плана. «Поможет ли индекс по shop.orders(status)?» → `hypothetical_index`: стоимость была/стала,
   используется ли индекс. `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT count(*) FROM pg_indexes WHERE schemaname = 'shop'"`
   до и после — одинаково.
10. **Защита SQL.** «Сделай explain_query на perf для `SELECT pg_sleep(30)`» → `forbidden_function` (так же `SELECT pg_stat_reset()` и `SELECT * FROM ts_stat('SELECT 1')`); для `SELECT 1; SELECT 2` →
    `invalid_sql`; для `DELETE FROM shop.audit` → план с `Delete on shop.audit`, а
    `docker exec gp-pg-perf psql -U postgres -d perf -c "SELECT count(*) FROM shop.audit"` до и после — одинаково.
11. **«Открыть в консоли EXPLAIN».** «Журнал» → «Обновить»: у успешного `explain_query` по SQL кнопка «Открыть в консоли EXPLAIN»
    → вкладка «EXPLAIN — perf» с тем же SQL, базой `perf` и узлом; план сам не строится. У вызова по `queryId` кнопка неактивна,
    подсказка «Недоступно: вызов задан queryId, текста запроса в журнале нет».
12. **Смена уровня на лету.** Форма `lab` → «Нет» → «Сохранить»; следующий вопрос про `lab` → ошибка `not_allowed` (в журнале
    код `not_allowed`), `list_connections` не показывает `lab`; клиент не переподключали. У прежних вызовов `lab` «Открыть в
    консоли EXPLAIN» неактивна: «Недоступно: доступ для LLM у подключения выключен». Вернуть «Диагностика».
13. **Claude Code, два клиента.** Фрагмент «Claude Code: команда» выполнить в терминале (область по умолчанию `local` — только текущий проект, поэтому запускать `claude` в том же каталоге; для всех проектов — `--scope user`, см. руководство); `claude` → диалог для claude-code (Qwen
    Code остаётся подключённым) → «Разрешить»; индикатор «LLM: 2 клиента»; «Активные сессии» — две строки с числом вызовов.
14. **Отзыв, отказ, отключение.** «Подтверждённые клиенты» → «Отозвать» у qwen-code → его сессия закрыта (в Qwen Code `/mcp` —
    сервер отключён). Перезапустить `qwen` → снова диалог → «Отклонить» → сессия закрыта, в «Подтверждённых клиентах» qwen-code
    нет; ещё раз `qwen` → диалог снова, «Разрешить». «Отключить всех» → индикатор пропал.
15. **Выключение MCP.** Снять «Разрешить MCP-сервер»: клиенты отключены, `mcp.json` и `mcp.sock` удалены; `qwen` → сообщение
    «MCP is disabled in Glafira: turn it on in Settings → LLM assistants (MCP)». Включить снова.
16. **Политика.** По `docs/admin.md` («Проверка политики вручную»): plist с `mcpEnabled = NO`, перезапуск Glafira → выключатель
    неактивен, «Отключено вашей организацией», значков «LLM» в дереве нет, в форме подключения — «MCP отключён вашей организацией:
    уровень не действует», `qwen` → «MCP is disabled by your organization». plist с `mcpMaxAccess = diagnostics` (без
    `mcpEnabled`) → «Доступ к данным ограничен вашей организацией», уровень «Диагностика» работает. plist с
    `mcpMaxAccess = everything` → MCP запрещён, в «О программе» ошибка политики. Удалить файл политики, перезапустить.
17. **Другой экземпляр.** `open -n -a "Glafira for PostgreSQL"` — второе окно: раздел MCP — «Сервер работает в другом окне
    Glafira», `mcp.json` не изменился (`pid` первого окна). Закрыть второе окно. Перед шагом закрыть подключения через SSH-туннель:
    второй запуск завершает процессы `ssh` первого (`docs/admin.md`, «Ограничение»).
18. **Справка и навигация.** F1 с фокусом в разделе MCP → глава «LLM-ассистенты (MCP)»; F1 с фокусом на выборе языка → «Начало
    работы». Щелчок по индикатору «LLM: …» внизу панели → «Настройки», прокрученные к разделу MCP; F1 сразу после этого —
    глава «LLM-ассистенты (MCP)».
19. **Windows** (установленный `.msi`): во фрагментах — путь к `glafira-pg.exe` в `C:\Program Files\…`; Qwen Code на Windows:
    команда из фрагмента, Glafira закрыта → `qwen` запускает её (шим из GUI-бинаря, окно консоли не появляется), соединение
    через `\\.\pipe\glafira-pg-mcp-…`, шаги 3–5 проходят.
20. **Linux** (`.AppImage`): во фрагментах — путь к файлу AppImage; шим запускает AppImage, шаги 3–5 проходят. `.deb`: путь во
    фрагментах — `/usr/bin/glafira-pg`.
