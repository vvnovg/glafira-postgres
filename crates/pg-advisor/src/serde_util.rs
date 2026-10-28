//! Помощники serde для JSON-контракта с интерфейсом.

/// `Option<i64>` как десятичная строка в JSON. 64-битное значение (например, `query_id` — хеш) обычно больше 2^53, а
/// JSON-число в JavaScript точно только до 2^53: числом оно округлилось бы по дороге в интерфейс. При чтении принимается
/// и строка, и число (совместимость со старыми данными).
pub mod opt_i64_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(n) => serializer.serialize_some(&n.to_string()),
            None => serializer.serialize_none(),
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Text(String),
        Number(i64),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<i64>, D::Error> {
        match Option::<StringOrNumber>::deserialize(deserializer)? {
            None => Ok(None),
            Some(StringOrNumber::Number(n)) => Ok(Some(n)),
            Some(StringOrNumber::Text(s)) => {
                s.trim().parse().map(Some).map_err(serde::de::Error::custom)
            }
        }
    }
}

/// `i64` как десятичная строка в JSON — для обязательных полей, как [`opt_i64_string`] для необязательных. Границы
/// последовательностей (`max` у `bigint` — 2^63 − 1) выше 2^53, где JSON-число в JavaScript теряет разряды. При чтении
/// принимается и строка, и число (совместимость со старыми данными).
pub mod i64_string {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        Text(String),
        Number(i64),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
        match StringOrNumber::deserialize(deserializer)? {
            StringOrNumber::Number(n) => Ok(n),
            StringOrNumber::Text(s) => s.trim().parse().map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Holder {
        #[serde(with = "super::opt_i64_string", default)]
        id: Option<i64>,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Plain {
        #[serde(with = "super::i64_string")]
        n: i64,
    }

    fn round_trip(id: Option<i64>) -> (String, Holder) {
        let json = serde_json::to_string(&Holder { id }).unwrap();
        let back = serde_json::from_str(&json).unwrap();
        (json, back)
    }

    #[test]
    fn extremes_round_trip_as_strings() {
        let (json, back) = round_trip(Some(i64::MAX));
        assert_eq!(json, r#"{"id":"9223372036854775807"}"#);
        assert_eq!(back.id, Some(i64::MAX));
        let (json, back) = round_trip(Some(i64::MIN));
        assert_eq!(json, r#"{"id":"-9223372036854775808"}"#);
        assert_eq!(back.id, Some(i64::MIN));
    }

    #[test]
    fn negative_hash_above_2_pow_53_keeps_every_digit() {
        // Хеш query_id бывает отрицательным: именно такое значение JS-числом потеряло бы младшие разряды.
        let (json, back) = round_trip(Some(-8_123_456_789_012_345_678));
        assert_eq!(json, r#"{"id":"-8123456789012345678"}"#);
        assert_eq!(back.id, Some(-8_123_456_789_012_345_678));
    }

    #[test]
    fn none_is_null() {
        let (json, back) = round_trip(None);
        assert_eq!(json, r#"{"id":null}"#);
        assert_eq!(back.id, None);
    }

    #[test]
    fn missing_field_is_none_and_numbers_are_still_accepted() {
        assert_eq!(serde_json::from_str::<Holder>("{}").unwrap().id, None);
        assert_eq!(
            serde_json::from_str::<Holder>(r#"{"id":42}"#).unwrap().id,
            Some(42)
        );
        assert!(serde_json::from_str::<Holder>(r#"{"id":"abc"}"#).is_err());
    }

    #[test]
    fn plain_i64_extremes_round_trip_as_strings() {
        for (n, text) in [
            (i64::MAX, "9223372036854775807"),
            (i64::MIN, "-9223372036854775808"),
            (0, "0"),
            (-1, "-1"),
        ] {
            let json = serde_json::to_string(&Plain { n }).unwrap();
            assert_eq!(json, format!(r#"{{"n":"{text}"}}"#));
            assert_eq!(serde_json::from_str::<Plain>(&json).unwrap().n, n);
        }
    }

    #[test]
    fn plain_i64_accepts_numbers_and_padded_strings_but_not_garbage() {
        assert_eq!(serde_json::from_str::<Plain>(r#"{"n":42}"#).unwrap().n, 42);
        assert_eq!(
            serde_json::from_str::<Plain>(r#"{"n":" -7 "}"#).unwrap().n,
            -7
        );
        assert!(serde_json::from_str::<Plain>(r#"{"n":"abc"}"#).is_err());
        assert!(serde_json::from_str::<Plain>(r#"{"n":null}"#).is_err());
        assert!(serde_json::from_str::<Plain>("{}").is_err());
        // За пределом i64 — ошибка, а не молчаливое усечение.
        assert!(serde_json::from_str::<Plain>(r#"{"n":"9223372036854775808"}"#).is_err());
    }
}
