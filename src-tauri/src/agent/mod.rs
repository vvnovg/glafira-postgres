//! Фоновый LLM-ассистент (спец. P4c): Glafira сама запускает агентский CLI на один запрос на оптимизацию. Здесь
//! билеты и область сессии запуска, настройки и пресеты, поиск программы и окружение логин-оболочки, запуски и их
//! журналы, процессы агента, очередь, тёплая модель и промпт.

pub mod detect;
pub mod models;
pub mod presets;
pub mod process;
pub mod prompt;
pub mod queue;
pub mod runs;
pub mod settings;
pub mod shell_env;
pub mod tickets;
pub mod warmth;
