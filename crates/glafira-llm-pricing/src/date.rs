//! Даты в файле таблицы: день цен — `ГГГГ-ММ-ДД`, время загрузки — RFC 3339 (UTC).

use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime, UtcOffset};

/// `2026-11-07`.
pub fn format_ymd(date: Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

/// `2026-11-07` → дата; другой вид или несуществующий день — `None`.
pub fn parse_ymd(s: &str) -> Option<Date> {
    let mut parts = s.trim().split('-');
    let (y, m, d) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let month = Month::try_from(m.parse::<u8>().ok()?).ok()?;
    Date::from_calendar_date(y.parse().ok()?, month, d.parse().ok()?).ok()
}

pub(crate) mod ymd {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;

    pub fn serialize<S: Serializer>(date: &Date, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::format_ymd(*date))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Date, D::Error> {
        let s = String::deserialize(d)?;
        super::parse_ymd(&s).ok_or_else(|| D::Error::custom(format!("invalid date {s:?}")))
    }
}

pub(crate) mod rfc3339_opt {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};
    use time::OffsetDateTime;

    pub fn serialize<S: Serializer>(at: &Option<OffsetDateTime>, s: S) -> Result<S::Ok, S::Error> {
        match at {
            Some(at) => {
                s.serialize_str(&super::format_rfc3339(*at).map_err(serde::ser::Error::custom)?)
            }
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<OffsetDateTime>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                OffsetDateTime::parse(&s, &super::Rfc3339)
                    .map_err(|e| D::Error::custom(format!("invalid time {s:?}: {e}")))
            })
            .transpose()
    }
}

/// Время в UTC, RFC 3339.
pub(crate) fn format_rfc3339(at: OffsetDateTime) -> Result<String, time::error::Format> {
    at.to_offset(UtcOffset::UTC).format(&Rfc3339)
}
