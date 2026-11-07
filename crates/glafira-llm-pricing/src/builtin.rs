//! Встроенная таблица цен Ollama Cloud: действует, пока приложение не загрузило свою («Обновить цены»). Сверяется со
//! страницей при выпуске (README крейта): образец страницы — `testdata/`, тест `the_builtin_table_is_the_sample_page`
//! проверяет, что таблица совпадает с разбором образца.

use std::sync::OnceLock;

use time::{Date, Month};

use crate::table::{ModelPrices, PRICING_URL, PriceTable, Prices};

/// День сверки встроенной таблицы со страницей цен (год, месяц, день).
pub const BUILTIN_DATE: (i32, u8, u8) = (2026, 11, 7);

/// Тройка цен: вход, кэшированный вход (`None` — на странице «-»), выход.
type Triple = (f64, Option<f64>, f64);

/// Модели в порядке страницы: имя, цены, цены «вне пика». Строки страницы без цены выхода (`clef`: «-») не входят —
/// по ним стоимость не оценить.
const MODELS: &[(&str, Triple, Option<Triple>)] = &[
    ("beam", (0.125, Some(0.015), 0.40), None),
    (
        "deepseek-v4.1-flash",
        (0.30, Some(0.006), 1.20),
        Some((0.15, Some(0.003), 0.60)),
    ),
    (
        "deepseek-v4-pro",
        (1.32, Some(0.044), 3.96),
        Some((0.66, Some(0.022), 1.98)),
    ),
    ("gemma4", (0.14, Some(0.05), 0.40), None),
    ("glm-5.3", (1.40, Some(0.26), 4.40), None),
    ("glm-5.3-flash", (0.15, Some(0.03), 0.50), None),
    ("glm-5.2", (1.40, Some(0.26), 4.40), None),
    ("gpt-oss:120b", (0.15, Some(0.014), 0.60), None),
    ("gpt-oss:20b", (0.07, Some(0.035), 0.30), None),
    ("kimi-k3.1-pro", (2.00, Some(0.20), 10.00), None),
    ("kimi-k3", (3.00, Some(0.30), 15.00), None),
    ("kimi-k2.7-code", (0.95, Some(0.19), 4.00), None),
    ("kimi-k2.6", (0.95, Some(0.16), 4.00), None),
    ("mimo-v2.6-flash", (0.14, Some(0.0028), 0.28), None),
    ("mimo-v2.6-pro", (0.435, Some(0.0036), 0.87), None),
    ("minimax-m3.1-flash", (0.15, Some(0.015), 0.60), None),
    ("minimax-m3", (0.60, Some(0.12), 2.40), None),
    ("minimax-m2.7", (0.30, Some(0.06), 1.20), None),
    ("mistral-large-4", (0.70, Some(0.07), 2.10), None),
    ("mistral-large-3", (0.50, None, 1.50), None),
    ("muse-spark-open", (1.25, Some(0.15), 4.25), None),
    ("nemotron-3-nano", (0.06, None, 0.24), None),
    ("nemotron-3-super", (0.015, Some(0.015), 0.60), None),
    ("nemotron-3-ultra", (0.10, Some(0.10), 3.00), None),
];

fn prices((input, cached_input, output): Triple) -> Prices {
    Prices {
        input,
        cached_input,
        output,
    }
}

/// Встроенная таблица: источник — `PRICING_URL`, дата — `BUILTIN_DATE`, без времени загрузки.
pub fn builtin() -> &'static PriceTable {
    static TABLE: OnceLock<PriceTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let (y, m, d) = BUILTIN_DATE;
        let month = Month::try_from(m).expect("BUILTIN_DATE: month");
        PriceTable {
            source: PRICING_URL.to_string(),
            date: Date::from_calendar_date(y, month, d).expect("BUILTIN_DATE: day"),
            fetched_at: None,
            models: MODELS
                .iter()
                .map(|&(model, standard, off_peak)| ModelPrices {
                    model: model.to_string(),
                    standard: prices(standard),
                    off_peak: off_peak.map(prices),
                })
                .collect(),
        }
    })
}
