//! Компактный JSON ответа для модели (смоук P4b, G2–G4): локальной модели с контекстом 64k ответы инструментов
//! забивали контекст. Один рекурсивный проход перед отправкой: ключи объектов со значением `null` убираются (об этом
//! говорят `instructions` сервера), дробные числа округляются. Остальное не меняется: строки (id запросов — строки),
//! целые и целые по значению дробные числа, `false`, `0`, `""`, `[]` и `null` внутри массивов (позиция значима,
//! например строки `run_select`).

use serde_json::{Number, Value};

/// Компактный вид ответа (на месте): без ключей со значением `null`, дробные числа — [`round`].
pub fn compact(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.retain(|_, x| !x.is_null());
            m.values_mut().for_each(compact);
        }
        Value::Array(items) => items.iter_mut().for_each(compact),
        Value::Number(n) => {
            // Только дробные: целые (`i64` / `u64`) не проходят через `f64` и не теряют точность.
            if n.is_f64()
                && let Some(r) = n.as_f64().map(round).and_then(Number::from_f64)
            {
                *n = r;
            }
        }
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

/// Округление дробного числа: |x| ≥ 100 — до одного знака после запятой, 1 ≤ |x| < 100 — до трёх, |x| < 1 — до трёх
/// значащих цифр (ревью правки A, #6: `0.0014` остаётся `0.0014`, а не `0.001`; ненулевое малое число — например
/// частоты `mostCommonFreqs` — никогда не становится ложным нулём). Целые по значению и нечисловые (`inf`, `NaN`) — как
/// есть.
pub fn round(x: f64) -> f64 {
    if !x.is_finite() || x.fract() == 0.0 {
        return x;
    }
    if x.abs() < 1.0 {
        // Три значащие цифры через десятичную запись: `0.1 + 0.2` даёт ровно `0.3`, а не соседнее двоичное число.
        return format!("{x:.2e}").parse().unwrap_or(x);
    }
    let scale = if x.abs() >= 100.0 { 10.0 } else { 1000.0 };
    (x * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn compacted(mut v: Value) -> Value {
        compact(&mut v);
        v
    }

    #[test]
    fn null_keys_are_dropped_at_every_depth() {
        let v = compacted(json!({
            "a": null,
            "b": 1,
            "nested": {"c": null, "d": {"e": null, "f": "x"}},
            "list": [{"g": null, "h": 2}]
        }));
        assert_eq!(
            v,
            json!({"b": 1, "nested": {"d": {"f": "x"}}, "list": [{"h": 2}]})
        );
    }

    /// `null` в массиве — значение на своём месте (строка `run_select`, значение колонки): не убирается.
    #[test]
    fn nulls_inside_arrays_stay() {
        let v = compacted(json!({"rows": [["1", null, "x"], [null]], "items": [null, 1]}));
        assert_eq!(
            v,
            json!({"rows": [["1", null, "x"], [null]], "items": [null, 1]})
        );
    }

    #[test]
    fn falsy_values_other_than_null_stay() {
        let v = json!({"f": false, "z": 0, "zf": 0.0, "s": "", "a": [], "o": {}});
        assert_eq!(compacted(v.clone()), v);
    }

    #[test]
    fn fractions_are_rounded_by_magnitude() {
        let v = compacted(json!({
            "cpuSysMs": 30761.375999999233,
            "big": -123.456,
            "edge": 100.04,
            "meanExecMs": 1.23456789,
            "ratio": 0.99949,
            "neg": -0.12345,
            "nested": [{"x": 12.3456}]
        }));
        assert_eq!(
            v,
            json!({
                "cpuSysMs": 30761.4,
                "big": -123.5,
                "edge": 100.0,
                "meanExecMs": 1.235,
                "ratio": 0.999,
                "neg": -0.123,
                "nested": [{"x": 12.346}]
            })
        );
        assert!(v.to_string().contains("\"cpuSysMs\":30761.4"), "{v}");
    }

    #[test]
    fn integers_and_whole_floats_are_kept_as_they_are() {
        let v = json!({
            "i": 42,
            "neg": -7,
            "u": u64::MAX,
            "whole": 5.0,
            "bigWhole": 1.0e20
        });
        let out = compacted(v.clone());
        assert_eq!(out, v);
        assert_eq!(out["whole"].as_f64(), Some(5.0));
        assert!(out["i"].is_i64() && out["u"].is_u64());
    }

    /// Строки не трогаются, даже если похожи на числа: `queryId` — 64-битный хеш строкой.
    #[test]
    fn strings_are_never_touched() {
        let v = json!({"queryId": "-6302332885333924942", "s": "1.23456789", "n": "null"});
        assert_eq!(compacted(v.clone()), v);
    }

    /// |x| < 1 — три значащие цифры (ревью правки A, #6): полоса [0,0005; 0,0015) больше не сливается в `0.001`.
    #[test]
    fn fractions_below_one_keep_three_significant_digits() {
        assert_eq!(round(0.0014), 0.0014);
        assert_eq!(round(0.000_51), 0.000_51);
        assert_eq!(round(0.0005), 0.0005);
        assert_eq!(round(0.001_234_5), 0.001_23);
        assert_eq!(round(0.012_345), 0.012_3);
        assert_eq!(round(0.123_45), 0.123);
        assert_eq!(round(-0.123_45), -0.123);
        assert_eq!(round(0.999_49), 0.999);
        assert_eq!(round(0.999_6), 1.0);
        assert_eq!(round(0.000_123_45), 0.000_123);
        assert_eq!(round(-0.000_444_4), -0.000_444);
        assert_eq!(round(0.0004), 0.0004);
        assert_eq!(round(1.0e-300), 1.0e-300);
        assert_eq!(round(0.0), 0.0);
        let v = compacted(json!({"mostCommonFreqs": [0.000_133_33, 0.5], "nullFrac": 0.001_4}));
        assert_eq!(
            v,
            json!({"mostCommonFreqs": [0.000_133, 0.5], "nullFrac": 0.001_4})
        );
        assert!(v.to_string().contains("\"nullFrac\":0.0014"), "{v}");
    }

    /// 1 ≤ |x| < 100 — три знака после запятой, |x| ≥ 100 — один; округление идемпотентно на всех полосах.
    #[test]
    fn rounding_bands_and_idempotence() {
        assert_eq!(round(1.234_56), 1.235);
        assert_eq!(round(-1.000_4), -1.0);
        assert_eq!(round(99.999_4), 99.999);
        assert_eq!(round(99.999_6), 100.0);
        assert_eq!(round(100.04), 100.0);
        assert_eq!(round(1905.98), 1906.0);
        for x in [
            0.0014, 0.000_51, 0.123_45, 0.999_6, 1.234_56, 99.999_6, 1905.98, -0.012_345, 3.0e-7,
        ] {
            assert_eq!(round(round(x)), round(x), "{x}");
        }
    }

    #[test]
    fn rounding_matches_the_decimal_text() {
        assert_eq!(round(0.1 + 0.2), 0.3);
        assert_eq!(Value::from(round(0.1 + 0.2)).to_string(), "0.3");
        assert_eq!(Value::from(round(2.0 / 3.0)).to_string(), "0.667");
        assert_eq!(Value::from(round(1234.5678)).to_string(), "1234.6");
        assert_eq!(round(f64::INFINITY), f64::INFINITY);
        assert!(round(f64::NAN).is_nan());
    }

    #[test]
    fn a_top_level_scalar_or_array_is_compacted_too() {
        assert_eq!(compacted(json!(1.23456)), json!(1.235));
        assert_eq!(compacted(json!([{"a": null}])), json!([{}]));
        assert_eq!(compacted(Value::Null), Value::Null);
    }
}
