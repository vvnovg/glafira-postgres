//! Оценка стоимости запуска по ценам таблицы: токены × цена за 1 млн, «вне пика» — по времени начала запуска.

use time::{Date, OffsetDateTime, UtcOffset, Weekday};

use crate::model::model_key;
use crate::table::PriceTable;
use crate::usage::TokenUsage;

/// Пик Ollama Cloud — будни с 12:00 до 18:00 UTC (конец не входит); вне пика — остальное время будней и выходные
/// целиком (страница цен: «Off-peak pricing apply outside 12:00 and 18:00 UTC on weekdays and all day on weekends»).
pub const PEAK_START_HOUR_UTC: u8 = 12;
/// Конец пика, час UTC (не входит).
pub const PEAK_END_HOUR_UTC: u8 = 18;

/// Оценка стоимости запуска.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    /// Сумма, $.
    pub usd: f64,
    /// Модель таблицы, по ценам которой считали (`gpt-oss:120b`, `gemma4`).
    pub model: String,
    /// Взяты цены «вне пика» (у модели они свои, и запуск начался вне пика).
    pub off_peak: bool,
    /// День цен таблицы (`PriceTable::date`).
    pub prices_date: Date,
}

/// Время вне пика Ollama Cloud: будни до 12:00 и с 18:00 UTC, суббота и воскресенье (по UTC).
pub fn is_off_peak(at: OffsetDateTime) -> bool {
    let utc = at.to_offset(UtcOffset::UTC);
    matches!(utc.weekday(), Weekday::Saturday | Weekday::Sunday)
        || !(PEAK_START_HOUR_UTC..PEAK_END_HOUR_UTC).contains(&utc.hour())
}

/// Оценка запуска облачной модели Ollama `model` (имя, как у агента: `glm-5.3-flash:cloud`) по `usage`, начатого в
/// `started_at`. Нет оценки (`None`), если модель не облачная (`model_key`), её нет в таблице или токенов нет. Кэшированный
/// вход без своей цены идёт по цене входа.
pub fn estimate(
    table: &PriceTable,
    model: &str,
    usage: &TokenUsage,
    started_at: OffsetDateTime,
) -> Option<CostEstimate> {
    if usage.total() == 0 {
        return None;
    }
    let m = table.find(&model_key(model)?)?;
    let off_peak = m.off_peak.filter(|_| is_off_peak(started_at));
    let p = off_peak.unwrap_or(m.standard);
    // u64 → f64: токенов меньше 2^53, точность не теряется.
    let usd = (usage.input as f64 * p.input
        + usage.cached_input as f64 * p.cached_or_input()
        + usage.output as f64 * p.output)
        / 1_000_000.0;
    Some(CostEstimate {
        usd,
        model: m.model.clone(),
        off_peak: off_peak.is_some(),
        prices_date: table.date,
    })
}
