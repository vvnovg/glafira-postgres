//! Токены запуска из `usage` итога агента (событие `"type": "result"` потока `stream-json`).

use serde_json::Value;

/// Как `usage` итога считает кэшированный вход: поля одни (`input_tokens`, `cache_read_input_tokens`,
/// `output_tokens`), смысл у агентов разный.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAccounting {
    /// Как Anthropic Messages API (Claude Code, и через Ollama тоже): `input_tokens` — вход без кэша,
    /// `cache_read_input_tokens` — отдельно, `cache_creation_input_tokens` (запись в кэш) — по цене входа.
    Separate,
    /// Как OpenAI и Gemini (Qwen Code): `input_tokens` — весь вход, кэшированный в том числе; `total_tokens` =
    /// `input_tokens` + `output_tokens`.
    Included,
}

/// Токены запуска для оценки стоимости.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    /// Вход без кэша.
    pub input: u64,
    /// Кэшированный вход.
    pub cached_input: u64,
    /// Выход (вместе с рассуждениями модели).
    pub output: u64,
}

impl TokenUsage {
    /// Все токены запуска.
    pub fn total(&self) -> u64 {
        self.input
            .saturating_add(self.cached_input)
            .saturating_add(self.output)
    }

    /// Токены из объекта `usage` итога агента. Нет `input_tokens` или `output_tokens` (целых неотрицательных) — `None`:
    /// оценивать не по чему. Кэша больше входа при `Included` не бывает; если пришло — весь вход кэшированный.
    pub fn from_json(usage: &Value, accounting: CacheAccounting) -> Option<Self> {
        let n = |k: &str| usage.get(k).and_then(Value::as_u64);
        let (input, output) = (n("input_tokens")?, n("output_tokens")?);
        let cache_read = n("cache_read_input_tokens").unwrap_or(0);
        Some(match accounting {
            CacheAccounting::Separate => Self {
                input: input.saturating_add(n("cache_creation_input_tokens").unwrap_or(0)),
                cached_input: cache_read,
                output,
            },
            CacheAccounting::Included => {
                let cached = cache_read.min(input);
                Self {
                    input: input - cached,
                    cached_input: cached,
                    output,
                }
            }
        })
    }
}
