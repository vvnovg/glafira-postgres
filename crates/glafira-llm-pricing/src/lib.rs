//! Цены облачных моделей Ollama Cloud (<https://ollama.com/pricing>) для инструментов семейства Glafira: оценка
//! стоимости запуска агента (Qwen Code, Claude Code через Ollama) по его usage.
//!
//! Что есть:
//! - встроенная таблица цен с датой и адресом источника (`builtin`), её обновляют при выпуске;
//! - ключ модели в таблице по имени модели Ollama (`model_key`: только облачные — тег `cloud` или `…-cloud`);
//! - токены из итога агента (`TokenUsage::from_json`: у Claude Code кэш считается отдельно от входа, у Qwen Code — внутри
//!   входа);
//! - оценка по usage и времени начала запуска (`estimate`: цены «вне пика» — будни вне 12:00–18:00 UTC и выходные);
//! - разбор страницы цен (`parse_pricing_page`) и загрузка её замыканием приложения (`fetch_table`): крейт не ходит в
//!   сеть сам, HTTP-клиент, прокси, таймаут и политику организации даёт приложение;
//! - файл таблицы (`store`: путь передаёт приложение, права 0600 на unix) и выбор действующей таблицы: сохранённая,
//!   а если её нет, она испорчена или старше встроенной — встроенная.
//!
//! Тексты для пользователя крейт не пишет: ошибки типизированы (`ParseError`, `RefreshError`, `StoreError`), а строки
//! интерфейса выбирает приложение.

mod builtin;
mod date;
mod estimate;
mod model;
mod page;
pub mod store;
mod table;
mod usage;

pub use builtin::{BUILTIN_DATE, builtin};
pub use date::{format_ymd, parse_ymd};
pub use estimate::{CostEstimate, PEAK_END_HOUR_UTC, PEAK_START_HOUR_UTC, estimate, is_off_peak};
pub use model::model_key;
pub use page::{RefreshError, fetch_table, parse_pricing_page};
pub use store::{Effective, Origin, PRICES_FILE, StoreError, effective, load, save};
pub use table::{MAX_PRICE, MIN_MODELS, ModelPrices, PRICING_URL, ParseError, PriceTable, Prices};
pub use usage::{CacheAccounting, TokenUsage};
