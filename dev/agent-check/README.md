# Проверка агентских CLI для фонового ассистента (только для разработки)

`agent_check.py` прогоняет Qwen Code и Claude Code без окна с флагами пресетов фонового ассистента (спека P4c §4.1) и
локальной моделью Ollama против шлюза Glafira без окна. Нужен, чтобы подтвердить таблицу пресетов (план P4c, задача 1) и
перепроверять её на новых версиях CLI перед выпуском (`docs/release-checklist.md`).

## Правила

- Только на M4 Pro и только по слову владельца: прогон загружает модель (`qwen3.6-64k:35b-a3b`, около 23 ГБ).
- Перед прогоном — `ollama ps`. Если загружена другая модель, `run` останавливается: спросите владельца, с его согласия —
  `--allow-evict`. Модель, которую загрузил прогон, в конце выгружается; модель, которая была в памяти, остаётся.
- `--cloud` (Claude Code без Ollama) — платный вызов облачной модели: только по отдельному слову владельца.
- Стенд — только `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf`, никогда `down`.

## Как устроено

Шлюз — `src-tauri/examples/llm_eval_host.rs` из `dev/llm-eval` (тот же код `AppState` и `McpController`, что в приложении,
каталог данных `~/.glafira-llm-eval`, новый клиент подтверждается автоматически). `run` собирает его с `EVAL_REQUESTS=<n>`, и
шлюз создаёт n запросов на оптимизацию к `perf`: каждый вариант с промптом оптимизации берёт свой запрос.

MCP-сервер каждого варианта — `agent_check.py shim-probe` из MCP-конфига варианта (файл 0600, как у Glafira). Проба
записывает в `<вариант>.probe.<pid>.json`, дошли ли `GLAFIRA_CHECK_NONCE` и `GLAFIRA_AGENT_TICKET` из `env` конфига, имена
унаследованных переменных (без значений) и рабочую папку сервера, затем убирает обе переменные, ставит `HOME=~/.glafira-llm-eval`
и заменяет себя шимом `src-tauri/target/debug/glafira-pg mcp`. Не дошла переменная — проба выходит, не подключаясь: к Glafira
владельца прогон не ходит.

Qwen Code запускается с временным `HOME`: копия `~/.qwen/settings.json` без `tools.core|exclude|allowed|disabled|approvalMode`
(ограничения задаёт вариант) и с приманкой — сервером `glafira-pg` в `mcpServers`, который записывает `<вариант>.decoy.*.json` и
выходит. Запись приманки значит, что Qwen Code взял сервер из настроек пользователя, а не из `--mcp-config`. `.env` и
`oauth_creds.json` копируются, если есть; временный каталог удаляется после варианта. Claude Code запускается с `HOME`
владельца (`--strict-mcp-config` отбрасывает остальные серверы), у `--canary` — с временным `CLAUDE_CONFIG_DIR`.

Промпт идёт через stdin; id запроса есть только в промпте, поэтому вызов с этим `requestId` в журнале шлюза доказывает, что
промпт прочитан. Рабочая папка — пустой временный каталог. Предел одного варианта — `--timeout` (900 с); по нему группа
процессов останавливается, как в Glafira: `SIGTERM`, через 5 с `SIGKILL`.

## Подготовка

1. Стенд: `docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf && scripts/wait-perf.sh`.
2. Модель в Ollama: `ollama list | grep qwen3.6-64k:35b-a3b` (как создать — `dev/llm-eval/README.md`, «Подготовка»).
3. CLI: `qwen --version` (0.26 и новее), `claude --version` (2.1 и новее). Python — системный `python3` (только стандартная
   библиотека).

## Прогон

```bash
python3 -I dev/agent-check/agent_check.py run
python3 -I dev/agent-check/agent_check.py run --canary --only q-canary,q-canary-bare,q-canary-safe,c-canary,c-canary-sources,c-canary-nocmds
python3 -I dev/agent-check/agent_check.py summary dev/agent-check/out/<время>
```

Варианты (`VARIANTS` в скрипте): `q-inventory`, `q-core-*` (в том числе временная строка пресета задачи 5), `q-exclude` —
какие инструменты видит модель Qwen Code (событие
`init` вывода `stream-json`); `q-allowed`, `q-trust`, `q-none`, `q-yolo` — одобрение вызовов Glafira; `q-text`, `q-limit-*`,
`q-bare`, `q-safe`, `q-ext-none` — вывод, пределы и режимы Qwen Code; `c-*` — то же для Claude Code через Ollama; `*-canary*`
(с `--canary`) — читаются ли `QWEN.md` / `CLAUDE.md` и срабатывает ли хук `SessionStart` пользователя при флагах пресета;
`c-cloud` (с `--cloud`) — стоимость в JSON без Ollama. Варианты Qwen Code после описи берут из неё имена инструментов и способ
выключить встроенные (`context.json` прогона).

## Итог

`out/<время>/`: `facts.json` (версии, ключи настроек пользователя без значений, расширения Qwen Code, есть ли `QWEN.md` и
`CLAUDE.md`, `qwen hooks --help`), `<вариант>.result.json` (команда, код выхода, время, вызовы из журнала шлюза, инструменты
`init`, записи пробы и приманки, поля JSON-вывода, canary, хвост stderr), `.stdout`, `.stderr`, `c-preset.debug.log` (журнал
отладки Claude Code: какие хуки и `CLAUDE.md` он загрузил) и `summary.md` — таблица по вариантам.

**Хуки.** Хук canary записан в формате Claude Code (`hooks.SessionStart[].hooks[] {type: command}`). У Qwen Code формат
сверьте с `qwen hooks --help` и документацией версии; если он другой — поправьте `canary_hook` (или отдельную функцию для
Qwen Code) и повторите `--canary`.

Как записать итог — план P4c, задача 1, шаги 8–9: журнал плана, таблица §4.1 и подраздел §16 спеки.
