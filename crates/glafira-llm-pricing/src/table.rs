//! Таблица цен: цены за 1 млн токенов в долларах — вход, кэшированный вход, выход; у части моделей — вторая тройка
//! «вне пика».

use std::fmt;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::cloud_model_name;

/// Страница цен Ollama Cloud.
pub const PRICING_URL: &str = "https://ollama.com/pricing";
/// Здравый смысл: цена за 1 млн токенов — не меньше 0 и меньше этого числа долларов.
pub const MAX_PRICE: f64 = 1000.0;
/// Здравый смысл: моделей в таблице не меньше. Меньше — разобралось что-то не то.
pub const MIN_MODELS: usize = 5;

/// Тройка цен за 1 млн токенов, $.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Prices {
    pub input: f64,
    /// Кэшированный вход; `None` — у модели нет своей цены кэша, кэшированные токены идут по цене входа.
    #[serde(default)]
    pub cached_input: Option<f64>,
    pub output: f64,
}

impl Prices {
    /// Цена кэшированного входа; нет своей — цена входа.
    pub fn cached_or_input(&self) -> f64 {
        self.cached_input.unwrap_or(self.input)
    }

    fn all(&self) -> impl Iterator<Item = f64> {
        [Some(self.input), self.cached_input, Some(self.output)]
            .into_iter()
            .flatten()
    }
}

/// Цены одной модели.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrices {
    /// Имя модели на странице цен, в нижнем регистре: `deepseek-v4.1-flash`, `gpt-oss:120b`.
    pub model: String,
    pub standard: Prices,
    /// Цены «вне пика» (`is_off_peak`); `None` — у модели они те же.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off_peak: Option<Prices>,
}

/// Таблица цен с источником и датой.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceTable {
    /// Адрес страницы цен.
    pub source: String,
    /// День, на который верны цены (UTC): у встроенной — день сверки при выпуске, у загруженной — день загрузки.
    #[serde(with = "crate::date::ymd")]
    pub date: Date,
    /// Когда таблица загружена со страницы; у встроенной — `None`.
    #[serde(
        default,
        with = "crate::date::rfc3339_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub fetched_at: Option<OffsetDateTime>,
    /// Модели в порядке страницы.
    pub models: Vec<ModelPrices>,
}

impl PriceTable {
    /// Цены по ключу модели (`model_key`): точное имя, а если его нет и у ключа есть тег — имя без тега
    /// (`gemma4:31b` → `gemma4`): страница называет модели без размера, если облачный размер один.
    pub fn find(&self, key: &str) -> Option<&ModelPrices> {
        let key = key.trim().to_ascii_lowercase();
        let exact = |k: &str| self.models.iter().find(|m| m.model == k);
        exact(&key).or_else(|| key.split_once(':').and_then(|(base, _)| exact(base)))
    }

    /// Облачные модели Ollama таблицы в её порядке: имя модели Ollama (`cloud_model_name`: `glm-5.3-flash:cloud`,
    /// `gpt-oss:120b-cloud`) и цены. Так приложение предлагает их в списке моделей агента.
    pub fn cloud_models(&self) -> impl Iterator<Item = (String, &ModelPrices)> {
        self.models.iter().map(|m| (cloud_model_name(&m.model), m))
    }

    /// Здравый смысл (и для разобранной страницы, и для прочитанного файла): каждая цена — конечное число от 0 до
    /// `MAX_PRICE` (не включая), моделей не меньше `MIN_MODELS`.
    pub fn validate(&self) -> Result<(), ParseError> {
        for m in &self.models {
            let prices = m
                .standard
                .all()
                .chain(m.off_peak.iter().flat_map(Prices::all));
            for price in prices {
                if !(price.is_finite() && (0.0..MAX_PRICE).contains(&price)) {
                    return Err(ParseError::PriceOutOfRange {
                        model: m.model.clone(),
                        price,
                    });
                }
            }
        }
        if self.models.len() < MIN_MODELS {
            return Err(ParseError::TooFewModels {
                found: self.models.len(),
            });
        }
        Ok(())
    }
}

/// Страница цен (или файл таблицы) не прошла разбор и проверку здравого смысла.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Строк с моделью и тремя ценами нашлось меньше `MIN_MODELS` (0 — таблицы нет вовсе).
    TooFewModels { found: usize },
    /// Цена вне `0 ≤ цена < MAX_PRICE`.
    PriceOutOfRange { model: String, price: f64 },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::TooFewModels { found } => write!(
                f,
                "price table not found: {found} models with prices, at least {MIN_MODELS} expected"
            ),
            ParseError::PriceOutOfRange { model, price } => write!(
                f,
                "implausible price {price} per million tokens for {model}"
            ),
        }
    }
}

impl std::error::Error for ParseError {}
