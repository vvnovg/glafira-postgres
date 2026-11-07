//! Оценка стоимости запуска: токены из usage итога агента, цены модели, «вне пика» по времени начала.

use glafira_llm_pricing::{
    CacheAccounting, TokenUsage, builtin, estimate, format_ymd, is_off_peak,
};
use serde_json::json;
use time::OffsetDateTime;
use time::macros::datetime;

/// Qwen Code, `glm-5.3-flash:cloud`, запуск смоука `a946948`: `input_tokens` включает кэш
/// (`total_tokens` = вход + выход).
fn qwen_usage() -> serde_json::Value {
    json!({"input_tokens": 61077, "output_tokens": 19408, "cache_read_input_tokens": 33920, "total_tokens": 80485})
}

/// Claude Code через Ollama, `deepseek-v4.1-flash:cloud`, запуск смоука `a718ae5`: кэш отдельно от входа.
fn claude_usage() -> serde_json::Value {
    json!({
        "input_tokens": 11010, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 84992,
        "output_tokens": 6984, "output_tokens_details": {"thinking_tokens": 0}, "service_tier": "standard"
    })
}

/// Понедельник, 2026-11-09, `HH:MM` UTC.
fn monday(h: u8, m: u8) -> OffsetDateTime {
    datetime!(2026-11-09 0:00 UTC)
        .replace_hour(h)
        .unwrap()
        .replace_minute(m)
        .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

#[test]
fn usage_of_qwen_code_counts_the_cache_inside_the_input() {
    let u = TokenUsage::from_json(&qwen_usage(), CacheAccounting::Included).unwrap();
    assert_eq!(
        u,
        TokenUsage {
            input: 61077 - 33920,
            cached_input: 33920,
            output: 19408
        }
    );
    assert_eq!(u.total(), 80485);
    // Кэша больше входа не бывает; если пришло — весь вход кэшированный.
    let odd = json!({"input_tokens": 10, "cache_read_input_tokens": 50, "output_tokens": 1});
    assert_eq!(
        TokenUsage::from_json(&odd, CacheAccounting::Included).unwrap(),
        TokenUsage {
            input: 0,
            cached_input: 10,
            output: 1
        }
    );
}

#[test]
fn usage_of_claude_code_counts_the_cache_apart_and_cache_writes_as_input() {
    let u = TokenUsage::from_json(&claude_usage(), CacheAccounting::Separate).unwrap();
    assert_eq!(
        u,
        TokenUsage {
            input: 11010,
            cached_input: 84992,
            output: 6984
        }
    );
    let writes = json!({"input_tokens": 100, "cache_creation_input_tokens": 50, "cache_read_input_tokens": 10, "output_tokens": 5});
    assert_eq!(
        TokenUsage::from_json(&writes, CacheAccounting::Separate).unwrap(),
        TokenUsage {
            input: 150,
            cached_input: 10,
            output: 5
        }
    );
}

#[test]
fn usage_without_tokens_is_none() {
    for usage in [
        json!(null),
        json!("61077"),
        json!({}),
        json!({"input_tokens": 10}),
        json!({"output_tokens": 10}),
        json!({"input_tokens": "10", "output_tokens": 1}),
        json!({"input_tokens": -1, "output_tokens": 1}),
    ] {
        for acc in [CacheAccounting::Included, CacheAccounting::Separate] {
            assert_eq!(TokenUsage::from_json(&usage, acc), None, "{usage} {acc:?}");
        }
    }
    let no_cache = json!({"input_tokens": 10, "output_tokens": 2});
    assert_eq!(
        TokenUsage::from_json(&no_cache, CacheAccounting::Included).unwrap(),
        TokenUsage {
            input: 10,
            cached_input: 0,
            output: 2
        }
    );
}

#[test]
fn off_peak_is_outside_12_to_18_utc_on_weekdays_and_all_weekend() {
    assert!(is_off_peak(monday(11, 59)), "будни 11:59");
    assert!(!is_off_peak(monday(12, 0)), "будни 12:00");
    assert!(!is_off_peak(monday(17, 59)), "будни 17:59");
    assert!(is_off_peak(monday(18, 0)), "будни 18:00");
    assert!(is_off_peak(monday(0, 0)), "будни 00:00");
    assert!(is_off_peak(datetime!(2026-11-07 13:00 UTC)), "суббота днём");
    assert!(
        is_off_peak(datetime!(2026-11-08 15:00 UTC)),
        "воскресенье днём"
    );
    assert!(
        !is_off_peak(datetime!(2026-11-13 17:00 UTC)),
        "пятница днём"
    );
    // Время берётся в UTC: 15:00 +03:00 — это 12:00 UTC, пик.
    assert!(!is_off_peak(datetime!(2026-11-09 15:00 +3)));
    // Суббота 01:00 +03:00 — это пятница 22:00 UTC, вне пика; понедельник 02:00 +03:00 — воскресенье UTC.
    assert!(is_off_peak(datetime!(2026-11-14 01:00 +3)));
    assert!(is_off_peak(datetime!(2026-11-09 02:00 +3)));
    // Понедельник 00:30 −10:00 — это понедельник 10:30 UTC: вне пика.
    assert!(is_off_peak(datetime!(2026-11-09 00:30 -10)));
}

#[test]
fn the_estimate_is_input_plus_cache_plus_output_at_the_model_prices() {
    let t = builtin();
    let qwen = TokenUsage::from_json(&qwen_usage(), CacheAccounting::Included).unwrap();
    let e = estimate(t, "glm-5.3-flash:cloud", &qwen, monday(13, 0)).unwrap();
    // 27 157 × 0,15 + 33 920 × 0,03 + 19 408 × 0,50 за млн.
    assert!(close(e.usd, 0.01479515), "{}", e.usd);
    assert_eq!(e.model, "glm-5.3-flash");
    assert!(!e.off_peak);
    assert_eq!(format_ymd(e.prices_date), "2026-11-07");
    // У модели без своих цен «вне пика» в выходные — те же цены, и оценка не помечена «вне пика».
    let weekend = estimate(
        t,
        "glm-5.3-flash:cloud",
        &qwen,
        datetime!(2026-11-07 13:00 UTC),
    )
    .unwrap();
    assert!(close(weekend.usd, e.usd) && !weekend.off_peak);
}

#[test]
fn off_peak_prices_apply_by_the_start_time() {
    let t = builtin();
    let claude = TokenUsage::from_json(&claude_usage(), CacheAccounting::Separate).unwrap();
    let peak = estimate(t, "deepseek-v4.1-flash:cloud", &claude, monday(12, 0)).unwrap();
    // 11 010 × 0,30 + 84 992 × 0,006 + 6 984 × 1,20 за млн.
    assert!(close(peak.usd, 0.012193752), "{}", peak.usd);
    assert!(!peak.off_peak);
    for at in [
        monday(11, 59),
        monday(18, 0),
        datetime!(2026-11-07 14:22 UTC),
    ] {
        let off = estimate(t, "deepseek-v4.1-flash:cloud", &claude, at).unwrap();
        // 11 010 × 0,15 + 84 992 × 0,003 + 6 984 × 0,60 за млн.
        assert!(close(off.usd, 0.006096876), "{at}: {}", off.usd);
        assert!(off.off_peak, "{at}");
    }
}

#[test]
fn cached_tokens_without_a_cache_price_go_at_the_input_price() {
    let u = TokenUsage {
        input: 1000,
        cached_input: 1000,
        output: 0,
    };
    let e = estimate(builtin(), "mistral-large-3:cloud", &u, monday(13, 0)).unwrap();
    assert!(close(e.usd, 0.001), "{}", e.usd);
    let size = estimate(builtin(), "gpt-oss:120b-cloud", &u, monday(13, 0)).unwrap();
    assert!(close(size.usd, (1000.0 * 0.15 + 1000.0 * 0.014) / 1e6));
    assert_eq!(size.model, "gpt-oss:120b");
}

#[test]
fn no_estimate_for_an_unknown_or_local_model_or_without_tokens() {
    let t = builtin();
    let u = TokenUsage {
        input: 10,
        cached_input: 0,
        output: 10,
    };
    let at = monday(13, 0);
    assert_eq!(estimate(t, "unknown-model:cloud", &u, at), None);
    assert_eq!(estimate(t, "glm-5.3-flash", &u, at), None, "без тега cloud");
    assert_eq!(estimate(t, "qwen3.6-64k:35b-a3b", &u, at), None);
    assert_eq!(
        estimate(t, "glm-5.3-flash:cloud", &TokenUsage::default(), at),
        None,
        "токенов нет"
    );
}
