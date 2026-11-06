# Проверка моделей на MCP Glafira (только для разработки)

Скриптовый прогон шести сценариев: модель за OpenAI-совместимым API (Ollama, vLLM и т. п.) вызывает инструменты
Glafira через шим `glafira-pg mcp` и отвечает по-русски. Нужен, чтобы сравнивать модели и проверять правки описаний и
схем инструментов. Сценарии взяты из смоука P4a (`docs/smoke/p4a.md`).

| Сценарий | Вопрос | Что считается верным ответом |
|---|---|---|
| s1 | что происходит на кластере `demo` | назван лидер и обе реплики (по Patroni API в момент прогона) |
| s2 | кто держит горизонт xmin на `lab`, цитата запроса | назван pid сессии с открытой транзакцией, в цитате `$1`, литералов `secret-literal-42` и `4242` нет |
| s3 | блокировки на `lab`, кто корневой | назван pid сессии с `LOCK TABLE`, литерала `777` нет |
| s4 | что грузит `perf` за 15 минут | вызван `get_top_queries`, назван запрос нагрузки, сказано, что окно не покрыто (истории у шлюза нет) |
| s5 | план запроса на `perf`, база не названа | успешный `explain_query`, узел плана в ответе |
| s6 | поможет ли индекс по `shop.orders(status)` | успешный `hypothetical_index`, стоимость в ответе, реальных индексов не прибавилось |

Утечка литерала в ответе — провал сценария независимо от остального.

## Как устроено

Прогон не трогает установленное приложение и связку ключей: `bench.py` собирает и запускает шлюз без окна
`src-tauri/examples/llm_eval_host.rs` — тот же код `AppState` и `McpController`, что в приложении, но каталог данных —
`~/.glafira-llm-eval` (`EVAL_HOME`), пароли стенда — в памяти, три подключения стенда (`lab`, `demo`, `perf`) с уровнем
«Диагностика», новый клиент подтверждается автоматически. Агент `agent.py` запускает шим отладочной сборки
(`src-tauri/target/debug/glafira-pg`, он не запускает приложение) с `HOME=$EVAL_HOME`, и шим находит `mcp.json` шлюза.

Перед каждым сценарием `bench.py` готовит стенд и записывает ожидания в `out/<tag>-<sN>-r<R>.expect.json`:
s1 — лидер и реплики из Patroni API; s2 — сессия с открытой транзакцией на `lab`; s3 — цепочка блокировок на `lab`;
s4–s6 — нагрузка `heavy.sql` на `perf` (один клиент pgbench, если нагрузки нет). После сценария сессии s2–s3
завершаются (по тексту запроса — и когда подготовка оборвалась), нагрузка останавливается в конце прогона, если её
запустил прогон: останавливается любой `pgbench` в `gp-pg-perf`, поэтому свою нагрузку на время прогона не запускайте. Модель после прогона выгружается
из Ollama (`keep_alive: 0`).

## Подготовка

1. Стенд: `docker compose -f dev/docker-compose.yml up -d --no-recreate` и `pg-perf`
   (`docker compose -f dev/docker-compose.yml up -d --no-recreate pg-perf`, `scripts/wait-perf.sh`).
2. Модель с контекстом не меньше 32 тыс. токенов, например в Ollama:

   ```bash
   printf 'FROM qwen3.6:35b-a3b\nPARAMETER num_ctx 65536\n' > Modelfile
   ollama create qwen3.6-64k:35b-a3b -f Modelfile
   ```

3. Python: `python3 -m venv dev/llm-eval/.venv && dev/llm-eval/.venv/bin/pip install -r dev/llm-eval/requirements.txt`.

## Прогон

```bash
dev/llm-eval/.venv/bin/python -I dev/llm-eval/bench.py q36 qwen3.6-64k:35b-a3b
```

Аргументы: `--reps` (по умолчанию 3), `--scenarios s1,s2,…`, `--base-url` (`http://127.0.0.1:11434/v1`),
`--max-tokens` (8192: модели с рассуждением на 4096 не успевают), `--timeout` (предел одного прогона, 1200 с).
Журналы прогонов — `out/<tag>-<sN>-r<R>.json` (шаги, вызовы, ответ, токены), лог агента — `.log`, лог шлюза —
`out/host.log`, журнал MCP шлюза — `~/.glafira-llm-eval/Library/Application Support/io.github.vvnovg.glafira.postgres/mcp-audit.jsonl`.
Сводка — `grade.py <tag> [<tag> …]`: строка на прогон и таблица Markdown в `out/<tag>-summary.md`. Проверки —
эвристики: ответы с пометкой FAIL стоит прочитать.

Агент — `agent.py`: системная подсказка, инструкции сервера, до 15 шагов, температура 0,2.

## Клиенты

`client_bench.py` прогоняет те же сценарии настоящими клиентами — Qwen Code и Claude Code — с той же локальной моделью:

```bash
dev/llm-eval/.venv/bin/python -I dev/llm-eval/client_bench.py qwen qwen3.6-64k:35b-a3b
dev/llm-eval/.venv/bin/python -I dev/llm-eval/client_bench.py claude qwen3.6-64k:35b-a3b
```

Клиент запускается без окна (`qwen "<вопрос>"` с `--approval-mode yolo`, `claude -p`), сервер Glafira подключается к
нему как `glafira-pg-eval` (шим отладочной сборки с `HOME=$EVAL_HOME`), встроенные инструменты оболочки, правки и
чтения файлов, веб-запросов и агентов выключены. Сами клиенты работают с вашим `HOME`: загружаются настройки
`~/.qwen` и `~/.claude`, журналы Claude Code пишутся в `~/.claude/projects`. Ответ берётся из вывода клиента, вызовы — из журнала MCP шлюза. По умолчанию
сценарии s1, s2, s5, по три прогона; сводка — `grade.py qwen claude`.
