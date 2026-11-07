# glafira-llm-pricing

Цены облачных моделей Ollama Cloud (https://ollama.com/pricing) для инструментов семейства Glafira: оценка стоимости
запуска агента (Qwen Code, Claude Code через Ollama) по его usage. Крейт не зависит от кода Glafira for PostgreSQL и не
ходит в сеть сам; зависимости — `serde`, `serde_json`, `time` (проверяет `tests/manifest.rs`).

## Что делает

- **Встроенная таблица** `builtin()`: цены за 1 млн токенов в долларах — вход, кэшированный вход, выход; у части моделей
  вторая тройка «вне пика». Источник — `PRICING_URL`, день сверки — `BUILTIN_DATE`.
- **Ключ модели** `model_key(name)`: только облачные модели Ollama — тег `cloud` или `…-cloud`:
  `glm-5.3-flash:cloud` → `glm-5.3-flash`, `gpt-oss:120b-cloud` → `gpt-oss:120b`. У локальной модели ключа нет.
  `PriceTable::find` ищет ключ, а если его нет — имя без тега (`gemma4:31b` → `gemma4`).
- **Имя облачной модели** `cloud_model_name(key)` — обратное `model_key`: ключ без размера получает тег `cloud`
  (`glm-5.3-flash` → `glm-5.3-flash:cloud`), ключ с размером — суффикс `-cloud` (`gpt-oss:120b` → `gpt-oss:120b-cloud`).
  `PriceTable::cloud_models()` — модели таблицы под этими именами с ценами, в порядке страницы: так приложение
  предлагает облачные модели в списке моделей агента, даже если их нет в `ollama list` (облачная модель отвечает через
  Ollama без `ollama pull`).
- **Токены** `TokenUsage::from_json(usage, accounting)` из объекта `usage` события `"type": "result"` потока
  `stream-json`. Смысл полей у агентов разный:
  - Claude Code (`CacheAccounting::Separate`, как Anthropic API): `input_tokens` — вход без кэша,
    `cache_read_input_tokens` — отдельно, `cache_creation_input_tokens` — по цене входа;
  - Qwen Code (`CacheAccounting::Included`, как OpenAI и Gemini): `input_tokens` включает кэш, `total_tokens` =
    вход + выход.
- **Оценка** `estimate(table, model, usage, started_at)`: вход × цена входа + кэш × цена кэша (нет цены кэша — по цене
  входа) + выход × цена выхода. Цены «вне пика» — если они у модели есть и запуск начался вне пика: в будни вне
  12:00–18:00 UTC и в выходные (`is_off_peak`). Нет модели в таблице, модель локальная или токенов нет — `None`.
- **Разбор страницы** `parse_pricing_page(html, fetched_at)`. Разбор не держится за вёрстку: строка цен — имя модели и
  три цены подряд (`$` и число или прочерк). Строка «вне пика» — та же модель с пометкой Off-Peak. Проверка здравого
  смысла: каждая цена от 0 до 1000 $ (не включая), моделей не меньше 5. Не разобралось — `ParseError`.
- **Загрузка** `fetch_table(fetch, now)`: страницу загружает замыкание приложения `fetch(PRICING_URL)` — его
  HTTP-клиент, прокси, таймаут и политика. Ошибка замыкания — `RefreshError::Network`, разбора — `RefreshError::Parse`.
- **Файл таблицы** `save(path, table)` / `load(path)`: путь передаёт приложение (`PRICES_FILE` = `ollama-prices.json` в
  каталоге данных). Запись атомарная, права 0600 на unix, в файле `schemaVersion`, дата цен, время загрузки и адрес.
- **Действующая таблица** `effective(path)`: сохранённая; встроенная — если файла нет, он не читается (`problem` —
  почему) или день его цен раньше дня встроенной (вышел выпуск со свежей таблицей). Моделей встроенной таблицы, которых
  нет в сохранённой (страница сменила вёрстку и разобралась частично), сохранённая не теряет: они дописываются в конец по
  ценам встроенной (`PriceTable::supplemented_from`).

Тексты для пользователя крейт не пишет: ошибки типизированы (`ParseError`, `RefreshError<E>`, `StoreError`), строки
интерфейса — у приложения.

## Почему HTTP — замыканием

У приложений семейства уже есть HTTP-клиент со своими прокси (системный прокси, переменные окружения), TLS и
политикой организации (например, запрет обращений к сети). Если бы клиент был в крейте, у него были бы другие настройки
прокси, а в дереве зависимостей появилась бы вторая сборка `reqwest`. Feature-флаг с `reqwest` не помог бы: версия и
настройки клиента всё равно разошлись бы с клиентом приложения. Поэтому крейт принимает `async`-замыкание, которое
возвращает текст страницы, и сам ничего не загружает. Файл пишется отдельным вызовом `save`: приложение решает, где
блокировать поток.

## Как подключить в другом инструменте

```toml
[dependencies]
# Из соседнего репозитория (путь) или из git:
glafira-llm-pricing = { path = "../glafira-postgres/crates/glafira-llm-pricing" }
# glafira-llm-pricing = { git = "https://github.com/vvnovg/glafira-postgres", tag = "vX.Y.Z" }
```

```rust
use glafira_llm_pricing::{CacheAccounting, TokenUsage, effective, estimate, fetch_table, save, PRICES_FILE};

// Оценка в конце запуска (блокирующий вызов: читает файл таблицы).
let table = effective(&data_dir.join(PRICES_FILE)).table;
let usage = TokenUsage::from_json(&result_event["usage"], CacheAccounting::Separate)?;
let cost = estimate(&table, "deepseek-v4.1-flash:cloud", &usage, started_at)?; // cost.usd, cost.off_peak

// «Обновить цены» — только по действию пользователя и если политика разрешает сеть.
let table = fetch_table(|url| async move { my_http_get(url).await }, OffsetDateTime::now_utc()).await?;
save(&data_dir.join(PRICES_FILE), &table)?; // в блокирующем потоке
```

В Glafira for PostgreSQL так сделано в `src-tauri/src/agent/pricing.rs`: оценка у записи запуска, кнопка «Обновить
цены» в «Фоновом ассистенте», запрет при `updatesEnabled = false`.

## Как обновлять встроенную таблицу при выпуске

1. Загрузить страницу: `curl -s https://ollama.com/pricing -o /tmp/pricing.html`.
2. Урезать её до раздела цен моделей (можно оставить начало тарифов и FAQ — это шум для разбора) и положить вместо
   `testdata/ollama-pricing-ГГГГ-ММ-ДД.html` (поправить имя в `tests/page.rs`).
3. `cargo test --manifest-path crates/glafira-llm-pricing/Cargo.toml`: тест `the_builtin_table_is_the_sample_page`
   покажет разницу встроенной таблицы и страницы.
4. Поправить `MODELS` и `BUILTIN_DATE` в `src/builtin.rs` до зелёного теста. Если на странице поменялось правило «вне
   пика», поправить `PEAK_START_HOUR_UTC` и `PEAK_END_HOUR_UTC` в `src/estimate.rs`.
5. Если страницу больше не разобрать (другая вёрстка), тесты разбора упадут на образце: поправить `src/page.rs`.
