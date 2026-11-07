//! Разбор страницы цен Ollama по образцу `testdata/ollama-pricing-2026-11-07.html` (страница от 2026-11-07, урезана:
//! начало тарифов, таблица цен моделей целиком, начало FAQ) и загрузка страницы замыканием приложения — без сети.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use glafira_llm_pricing::{
    PRICING_URL, ParseError, Prices, RefreshError, builtin, fetch_table, format_ymd,
    parse_pricing_page,
};
use time::OffsetDateTime;
use time::macros::datetime;

const SAMPLE: &str = include_str!("../testdata/ollama-pricing-2026-11-07.html");
const FETCHED: OffsetDateTime = datetime!(2026-11-07 12:57:03 UTC);

fn p(input: f64, cached_input: Option<f64>, output: f64) -> Prices {
    Prices {
        input,
        cached_input,
        output,
    }
}

/// Будущее, которое готово сразу (замыкание отвечает без ожиданий): исполнитель не нужен.
fn ready<T>(f: impl Future<Output = T>) -> T {
    match pin!(f).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("будущее не готово"),
    }
}

#[test]
fn the_sample_page_gives_every_model_with_its_off_peak_prices() {
    let t = parse_pricing_page(SAMPLE, FETCHED).unwrap();
    assert_eq!(t.source, PRICING_URL);
    assert_eq!(format_ymd(t.date), "2026-11-07");
    assert_eq!(t.fetched_at, Some(FETCHED));
    let names: Vec<&str> = t.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(
        names,
        [
            "beam",
            "deepseek-v4.1-flash",
            "deepseek-v4-pro",
            "gemma4",
            "glm-5.3",
            "glm-5.3-flash",
            "glm-5.2",
            "gpt-oss:120b",
            "gpt-oss:20b",
            "kimi-k3.1-pro",
            "kimi-k3",
            "kimi-k2.7-code",
            "kimi-k2.6",
            "mimo-v2.6-flash",
            "mimo-v2.6-pro",
            "minimax-m3.1-flash",
            "minimax-m3",
            "minimax-m2.7",
            "mistral-large-4",
            "mistral-large-3",
            "muse-spark-open",
            "nemotron-3-nano",
            "nemotron-3-super",
            "nemotron-3-ultra",
        ],
        "clef без цены выхода не входит"
    );
    let flash = t.find("deepseek-v4.1-flash").unwrap();
    assert_eq!(flash.standard, p(0.30, Some(0.006), 1.20));
    assert_eq!(flash.off_peak, Some(p(0.15, Some(0.003), 0.60)));
    let pro = t.find("deepseek-v4-pro").unwrap();
    assert_eq!(pro.standard, p(1.32, Some(0.044), 3.96));
    assert_eq!(pro.off_peak, Some(p(0.66, Some(0.022), 1.98)));
    assert_eq!(
        t.models.iter().filter(|m| m.off_peak.is_some()).count(),
        2,
        "«вне пика» — только у двух моделей"
    );
    assert_eq!(
        t.find("mistral-large-3").unwrap().standard,
        p(0.50, None, 1.50),
        "«-» в кэше — цены кэша нет"
    );
    assert_eq!(
        t.find("mimo-v2.6-flash").unwrap().standard,
        p(0.14, Some(0.0028), 0.28)
    );
    assert_eq!(
        t.find("kimi-k3.1-pro").unwrap().standard,
        p(2.00, Some(0.20), 10.00)
    );
}

/// Выпуск: встроенная таблица — это разбор образца страницы. Обновили образец — обновите `builtin.rs` (README).
#[test]
fn the_builtin_table_is_the_sample_page() {
    let t = parse_pricing_page(SAMPLE, FETCHED).unwrap();
    assert_eq!(builtin().models, t.models);
    assert_eq!(builtin().date, t.date, "дата встроенной — день образца");
    assert_eq!(builtin().source, PRICING_URL);
    assert_eq!(builtin().fetched_at, None);
    builtin().validate().unwrap();
}

#[test]
fn garbage_and_a_page_without_the_table_are_refused() {
    for html in [
        "",
        "not html at all $1 $2 $3",
        "<html><body><p>Ollama</p></body></html>",
        "\u{0}\u{1}<<<>>>&&&;;;",
        "{\"models\": [\"gemma4\", 0.14, 0.05, 0.40]}",
    ] {
        assert_eq!(
            parse_pricing_page(html, FETCHED),
            Err(ParseError::TooFewModels { found: 0 }),
            "{html:?}"
        );
    }
    // Та же страница без раздела цен моделей: тарифы ($20, $100, $500) и FAQ таблицей цен не считаются.
    let start = SAMPLE.find("<section id=\"model-pricing\"").unwrap();
    let end = start + SAMPLE[start..].find("</section>").unwrap() + "</section>".len();
    let without = format!("{}{}", &SAMPLE[..start], &SAMPLE[end..]);
    assert!(without.contains("$20") && without.contains("$500"));
    assert_eq!(
        parse_pricing_page(&without, FETCHED),
        Err(ParseError::TooFewModels { found: 0 })
    );
}

#[test]
fn fewer_than_five_models_or_an_implausible_price_are_refused() {
    let rows: String = [
        ("gemma4", "$0.14", "$0.05", "$0.40"),
        ("glm-5.3", "$1.40", "$0.26", "$4.40"),
        ("glm-5.3-flash", "$0.15", "$0.03", "$0.50"),
        ("gpt-oss:20b", "$0.07", "$0.035", "$0.30"),
    ]
    .iter()
    .map(|(m, a, b, c)| format!("<tr><td>{m}</td><td>{a}</td><td>{b}</td><td>{c}</td></tr>"))
    .collect();
    assert_eq!(
        parse_pricing_page(&format!("<table>{rows}</table>"), FETCHED),
        Err(ParseError::TooFewModels { found: 4 })
    );
    let huge = SAMPLE.replacen("$15.00", "$1500.00", 1);
    assert_eq!(
        parse_pricing_page(&huge, FETCHED),
        Err(ParseError::PriceOutOfRange {
            model: "kimi-k3".into(),
            price: 1500.0
        })
    );
    let thousand = SAMPLE.replacen("$15.00", "$1,000", 1);
    assert!(matches!(
        parse_pricing_page(&thousand, FETCHED),
        Err(ParseError::PriceOutOfRange { .. })
    ));
}

/// Разбор не держится за классы и теги таблицы: строки — это имя модели и три цены подряд (или «-»), пометка
/// «Off-Peak» — в имени или отдельным элементом.
#[test]
fn another_layout_parses_the_same() {
    let html = r#"
        <div class="grid"><div>Model</div><div>Input</div><div>Cached input</div><div>Output</div></div>
        <ul>
          <li><span>DeepSeek-V4.1-Flash</span><span>$0.30</span><span>$0.006</span><span>&#36;1.20</span></li>
          <li><span>deepseek-v4.1-flash</span> <em class="badge">Off-peak</em><span>$ 0.15</span><span>$0.003</span><span>$0.60</span></li>
          <li><span>gemma4</span><span>$0.14</span><span>$0.05</span><span>$0.40</span></li>
          <li><span>mistral-large-3</span><span>$0.50</span><span>&ndash;</span><span>$1.50</span></li>
          <li><span>nemotron-3-nano</span><span>$0.06</span><span>—</span><span>$0.24</span></li>
          <li><span>gpt-oss:120b</span><span>$0.15</span><span>$0.014</span><span>$0.60</span></li>
          <li><span>clef</span><span>$0.24</span><span>-</span><span>-</span></li>
        </ul>
        <script>var prices = ["<li>fake</li>", "$9", "$9", "$9"];</script>
        <!-- <li><span>hidden</span><span>$1</span><span>$1</span><span>$1</span></li> -->
    "#;
    let t = parse_pricing_page(html, FETCHED).unwrap();
    let names: Vec<&str> = t.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(
        names,
        [
            "deepseek-v4.1-flash",
            "gemma4",
            "mistral-large-3",
            "nemotron-3-nano",
            "gpt-oss:120b"
        ]
    );
    let flash = t.find("deepseek-v4.1-flash").unwrap();
    assert_eq!(flash.standard, p(0.30, Some(0.006), 1.20));
    assert_eq!(flash.off_peak, Some(p(0.15, Some(0.003), 0.60)));
    assert_eq!(
        t.find("mistral-large-3").unwrap().standard.cached_input,
        None
    );
    assert_eq!(
        t.find("nemotron-3-nano").unwrap().standard.cached_input,
        None
    );
}

#[test]
fn the_page_is_loaded_by_the_closure_of_the_app() {
    let mut asked = None;
    let t = ready(fetch_table(
        |url| {
            asked = Some(url);
            async { Ok::<_, String>(SAMPLE.to_string()) }
        },
        FETCHED,
    ))
    .unwrap();
    assert_eq!(asked, Some(PRICING_URL));
    assert_eq!(t, parse_pricing_page(SAMPLE, FETCHED).unwrap());
    match ready(fetch_table(
        |_| async { Err::<String, _>("offline".to_string()) },
        FETCHED,
    )) {
        Err(RefreshError::Network(e)) => assert_eq!(e, "offline"),
        other => panic!("{other:?}"),
    }
    match ready(fetch_table(
        |_| async { Ok::<_, String>("<html></html>".into()) },
        FETCHED,
    )) {
        Err(RefreshError::Parse(e)) => assert_eq!(e, ParseError::TooFewModels { found: 0 }),
        other => panic!("{other:?}"),
    }
}
