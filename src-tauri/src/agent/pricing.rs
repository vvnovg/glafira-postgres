//! Оценка стоимости облачных запусков через Ollama (смоук P4c, sf-T6) по ценам Ollama Cloud: таблица, ключ модели, разбор
//! страницы и файл таблицы — крейт `glafira-llm-pricing`. Здесь — оценка для записи запуска, сведения о действующей
//! таблице для «Фонового ассистента» и «Обновить цены». Glafira загружает https://ollama.com/pricing только по этой
//! кнопке, никогда сама; тем же способом, что и проверка обновлений (HTTPS, системный прокси и прокси окружения), и не
//! загружает, если политика запрещает обновления (`updatesEnabled = false`). Таблица — `ollama-prices.json` в каталоге
//! данных (0600); нет её — встроенная.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use glafira_llm_pricing::{self as pricing, CacheAccounting, Origin, RefreshError, TokenUsage};
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use super::runs::CostEstimate;
use super::settings::Preset;
use crate::conn::errors::chain;
use crate::error::{AppResult, ErrorCode};
use crate::mcp::llm::off_thread;
use crate::policy::PolicyStatus;

/// Загрузка страницы цен целиком (соединение и ответ).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Больше страница цен не бывает (сейчас ~50 КБ): больший ответ — не она.
pub const MAX_PAGE_BYTES: usize = 4 << 20;

/// Файл загруженной таблицы в каталоге данных.
pub fn prices_path(data_dir: &Path) -> PathBuf {
    data_dir.join(pricing::PRICES_FILE)
}

/// Как `usage` итога агента считает кэш: у Qwen Code он внутри `input_tokens` (`total_tokens` = вход + выход, журналы
/// смоука `a946948`, `a68b7e1`), у Claude Code — отдельно (`a718ae5`: кэша 84 992 при входе 11 010).
fn accounting(preset: Preset) -> CacheAccounting {
    match preset {
        Preset::Qwen => CacheAccounting::Included,
        Preset::Claude => CacheAccounting::Separate,
    }
}

/// Предупреждение об испорченном файле таблицы уже записано в этом запуске приложения.
static BROKEN_FILE_REPORTED: AtomicBool = AtomicBool::new(false);

/// Первый раз за запуск: `true` и флаг поднят; дальше — `false`.
fn first_warning(flag: &AtomicBool) -> bool {
    !flag.swap(true, Ordering::Relaxed)
}

/// Действующая таблица; файл не прочитался — встроенная и предупреждение в журнал приложения, одно за запуск (таблицу
/// читают `info`, список моделей и оценка каждого запуска).
fn effective(data_dir: &Path) -> pricing::Effective {
    let e = pricing::effective(&prices_path(data_dir));
    if let Some(p) = &e.problem
        && first_warning(&BROKEN_FILE_REPORTED)
    {
        tracing::warn!("ollama price table not read, the built-in one is used: {p}");
    }
    e
}

/// Действующая таблица цен (`ollama-prices.json` или встроенная): облачные модели для поля «Модель». Блокирующий вызов.
pub fn table(data_dir: &Path) -> pricing::PriceTable {
    effective(data_dir).table
}

/// Оценка запуска облачной модели Ollama `model` по `usage` итога агента и времени начала. Нет оценки, если модель не
/// облачная или её нет в таблице, нет `usage` или начала. Блокирующий вызов: читает файл таблицы.
pub fn estimate_run(
    data_dir: &Path,
    preset: Preset,
    model: Option<&str>,
    usage: Option<&Value>,
    started_at_ms: Option<i64>,
) -> Option<CostEstimate> {
    let model = model?;
    // Локальная модель — без чтения файла.
    pricing::model_key(model)?;
    let usage = TokenUsage::from_json(usage?, accounting(preset))?;
    let started =
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(started_at_ms?) * 1_000_000).ok()?;
    let e = pricing::estimate(&effective(data_dir).table, model, &usage, started)?;
    Some(CostEstimate {
        usd: e.usd,
        prices_date: pricing::format_ymd(e.prices_date),
        off_peak: e.off_peak,
    })
}

/// Действующая таблица цен для «Фонового ассистента» (`get_ollama_prices`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaPrices {
    /// День цен, `ГГГГ-ММ-ДД`.
    pub date: String,
    /// Адрес страницы цен.
    pub source: String,
    /// Таблица загружена «Обновить цены»; иначе — встроенная.
    pub saved: bool,
    /// Когда загружена (мс эпохи); у встроенной — нет.
    pub fetched_at_ms: Option<i64>,
    /// Моделей в таблице.
    pub models: usize,
    /// «Обновить цены» разрешена: политика не запрещает обновления (`updatesEnabled`).
    pub refresh_allowed: bool,
}

/// Сведения о действующей таблице. Блокирующий вызов.
pub fn info(data_dir: &Path, policy: &PolicyStatus) -> OllamaPrices {
    let e = effective(data_dir);
    OllamaPrices {
        date: pricing::format_ymd(e.table.date),
        source: e.table.source.clone(),
        saved: e.origin == Origin::Saved,
        fetched_at_ms: e
            .table
            .fetched_at
            .and_then(|at| i64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok()),
        models: e.table.models.len(),
        refresh_allowed: policy.updates_enabled,
    }
}

/// «Обновить цены»: загрузка страницы замыканием `fetch` (`fetch_page` у приложения), разбор и запись таблицы. Политика
/// запрещает обновления — ошибка, страница не загружается. Не загрузилась или не разобралась — своя ошибка, прежняя
/// таблица остаётся.
pub async fn refresh_with<F, Fut>(
    data_dir: PathBuf,
    policy: &PolicyStatus,
    fetch: F,
) -> AppResult<OllamaPrices>
where
    F: FnOnce(&'static str) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    if !policy.updates_enabled {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.agent.pricesPolicy",
            "Updating prices is disabled by your organization"
        ));
    }
    let table = pricing::fetch_table(fetch, OffsetDateTime::now_utc())
        .await
        .map_err(|e| match e {
            RefreshError::Network(details) => app_err!(
                ErrorCode::ConnectionFailed,
                "errors.agent.pricesFetch",
                "Could not load the Ollama price page"
            )
            .with_details(details),
            RefreshError::Parse(p) => app_err!(
                ErrorCode::Internal,
                "errors.agent.pricesParse",
                "Could not parse the Ollama price page"
            )
            .with_details(p.to_string()),
        })?;
    let policy = policy.clone();
    off_thread(move || {
        pricing::save(&prices_path(&data_dir), &table).map_err(|e| {
            app_err!(
                ErrorCode::Internal,
                "errors.agent.pricesSave",
                "Could not save the Ollama price table"
            )
            .with_details(e.to_string())
        })?;
        Ok(info(&data_dir, &policy))
    })
    .await
}

/// Больше переадресаций страница цен не делает (столько же по умолчанию у reqwest).
const MAX_REDIRECTS: usize = 10;

/// Переадресация страницы цен — только на HTTPS ollama.com и его поддомены: чужая страница в разбор не попадает.
fn redirect_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|h| {
            let h = h.to_ascii_lowercase();
            h == "ollama.com" || h.ends_with(".ollama.com")
        })
}

/// Страница цен тем же способом, что и проверка обновлений (`tauri-plugin-updater`): общий `reqwest` с прокси окружения
/// и системным (`system-proxy`), только HTTPS, переадресации — только на ollama.com (`redirect_allowed`, иначе ошибка
/// сети), срок `FETCH_TIMEOUT`, ответ не больше `MAX_PAGE_BYTES`. Ошибка — технический текст для подробностей.
pub async fn fetch_page(url: &'static str) -> Result<String, String> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error(format!("more than {MAX_REDIRECTS} redirects"))
        } else if redirect_allowed(attempt.url()) {
            attempt.follow()
        } else {
            let to = attempt.url().host_str().unwrap_or_default().to_string();
            attempt.error(format!("redirect outside ollama.com: {to}"))
        }
    });
    let http = reqwest::Client::builder()
        .https_only(true)
        .redirect(redirects)
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("glafira-pg/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| chain(&e))?;
    let mut resp = http
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| chain(&e))?;
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| chain(&e))? {
        if body.len() + chunk.len() > MAX_PAGE_BYTES {
            return Err(format!("the page is larger than {MAX_PAGE_BYTES} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde_json::json;

    use super::*;

    /// Понедельник 2026-11-09 13:00 UTC (пик) и суббота 2026-11-07 14:22 UTC (вне пика), мс.
    const MONDAY_PEAK: i64 = 1_794_229_200_000;
    const SATURDAY: i64 = 1_794_061_320_000;

    fn qwen_usage() -> Value {
        json!({"input_tokens": 61077, "output_tokens": 19408, "cache_read_input_tokens": 33920, "total_tokens": 80485})
    }

    fn claude_usage() -> Value {
        json!({"input_tokens": 11010, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 84992, "output_tokens": 6984})
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    /// Страница цен, как её отдаёт ollama.com (образец крейта).
    const PAGE: &str =
        include_str!("../../../crates/glafira-llm-pricing/testdata/ollama-pricing-2026-11-07.html");

    /// Итоговое ревью бэкенда: страница цен — только с ollama.com; переадресация на другой хост (или не на HTTPS) — ошибка
    /// сети, а не чужая страница в разборе.
    #[test]
    fn redirects_of_the_price_page_stay_on_ollama_com() {
        let url = |s: &str| reqwest::Url::parse(s).unwrap();
        for ok in [
            "https://ollama.com/pricing",
            "https://www.ollama.com/pricing",
            "https://OLLAMA.com/pricing?x=1",
            "https://cdn.eu.ollama.com/p",
        ] {
            assert!(redirect_allowed(&url(ok)), "{ok}");
        }
        for bad in [
            "http://ollama.com/pricing",
            "https://ollama.com.evil.example/pricing",
            "https://evil-ollama.com/pricing",
            "https://example.com/ollama.com",
            "https://127.0.0.1/pricing",
        ] {
            assert!(!redirect_allowed(&url(bad)), "{bad}");
        }
    }

    /// Итоговое ревью бэкенда: испорченный `ollama-prices.json` — одно предупреждение в журнал приложения за запуск, а не
    /// на каждое чтение таблицы (`info`, `table`, оценка каждого запуска).
    #[test]
    fn a_broken_price_file_is_reported_once() {
        let flag = AtomicBool::new(false);
        assert!(first_warning(&flag));
        assert!(!first_warning(&flag));
        assert!(!first_warning(&flag));
    }

    #[test]
    fn the_times_of_the_tests_are_what_they_say() {
        let at = |ms: i64| OffsetDateTime::from_unix_timestamp(ms / 1000).unwrap();
        assert_eq!(
            (at(MONDAY_PEAK).weekday(), at(MONDAY_PEAK).hour()),
            (time::Weekday::Monday, 13)
        );
        assert_eq!(
            (at(SATURDAY).weekday(), at(SATURDAY).hour()),
            (time::Weekday::Saturday, 14)
        );
    }

    /// Запуски смоука: `a946948` (Qwen Code, glm-5.3-flash:cloud) и `a718ae5` (Claude Code через Ollama,
    /// deepseek-v4.1-flash:cloud, суббота — вне пика).
    #[test]
    fn the_runs_of_the_smoke_are_estimated_by_their_usage() {
        let dir = tempfile::tempdir().unwrap();
        let qwen = estimate_run(
            dir.path(),
            Preset::Qwen,
            Some("glm-5.3-flash:cloud"),
            Some(&qwen_usage()),
            Some(MONDAY_PEAK),
        )
        .unwrap();
        // Кэш внутри входа: (61 077 − 33 920) × 0,15 + 33 920 × 0,03 + 19 408 × 0,50 за млн.
        assert!(close(qwen.usd, 0.01479515), "{}", qwen.usd);
        assert_eq!(
            (qwen.prices_date.as_str(), qwen.off_peak),
            ("2026-11-07", false)
        );
        let claude = estimate_run(
            dir.path(),
            Preset::Claude,
            Some("deepseek-v4.1-flash:cloud"),
            Some(&claude_usage()),
            Some(SATURDAY),
        )
        .unwrap();
        // Кэш отдельно, цены вне пика: 11 010 × 0,15 + 84 992 × 0,003 + 6 984 × 0,60 за млн.
        assert!(close(claude.usd, 0.006096876), "{}", claude.usd);
        assert!(claude.off_peak);
        let peak = estimate_run(
            dir.path(),
            Preset::Claude,
            Some("deepseek-v4.1-flash:cloud"),
            Some(&claude_usage()),
            Some(MONDAY_PEAK),
        )
        .unwrap();
        assert!(close(peak.usd, 0.012193752) && !peak.off_peak, "{peak:?}");
    }

    #[test]
    fn no_estimate_without_a_cloud_model_usage_or_start() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let u = qwen_usage();
        for (model, usage, started) in [
            (Some("qwen3.6-64k:35b-a3b"), Some(&u), Some(MONDAY_PEAK)),
            (Some("unknown-model:cloud"), Some(&u), Some(MONDAY_PEAK)),
            (None, Some(&u), Some(MONDAY_PEAK)),
            (Some("glm-5.3-flash:cloud"), None, Some(MONDAY_PEAK)),
            (
                Some("glm-5.3-flash:cloud"),
                Some(&json!({})),
                Some(MONDAY_PEAK),
            ),
            (Some("glm-5.3-flash:cloud"), Some(&u), None),
        ] {
            assert_eq!(
                estimate_run(d, Preset::Qwen, model, usage, started),
                None,
                "{model:?} {usage:?} {started:?}"
            );
        }
    }

    #[test]
    fn the_saved_table_is_used_for_the_estimate_and_the_info() {
        let dir = tempfile::tempdir().unwrap();
        let policy = PolicyStatus::default();
        let builtin = info(dir.path(), &policy);
        assert_eq!(
            builtin,
            OllamaPrices {
                date: "2026-11-07".into(),
                source: "https://ollama.com/pricing".into(),
                saved: false,
                fetched_at_ms: None,
                models: 24,
                refresh_allowed: true,
            }
        );
        let mut t = pricing::builtin().clone();
        t.date = pricing::parse_ymd("2026-12-01").unwrap();
        t.fetched_at = Some(OffsetDateTime::from_unix_timestamp(1_796_117_400).unwrap());
        for m in t.models.iter_mut().filter(|m| m.model == "glm-5.3-flash") {
            m.standard.output = 1.0;
        }
        pricing::save(&prices_path(dir.path()), &t).unwrap();
        let saved = info(dir.path(), &policy);
        assert_eq!(
            (
                saved.date.as_str(),
                saved.saved,
                saved.fetched_at_ms,
                saved.models
            ),
            ("2026-12-01", true, Some(1_796_117_400_000), 24)
        );
        let e = estimate_run(
            dir.path(),
            Preset::Qwen,
            Some("glm-5.3-flash:cloud"),
            Some(&qwen_usage()),
            Some(MONDAY_PEAK),
        )
        .unwrap();
        assert!(
            close(
                e.usd,
                (27157.0 * 0.15 + 33920.0 * 0.03 + 19408.0 * 1.0) / 1e6
            ),
            "{}",
            e.usd
        );
        assert_eq!(e.prices_date, "2026-12-01");
        // Испорченный файл — встроенная таблица.
        std::fs::write(prices_path(dir.path()), "{").unwrap();
        assert_eq!(info(dir.path(), &policy), builtin);
    }

    #[test]
    fn the_policy_without_updates_disables_the_button_and_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let policy = PolicyStatus {
            updates_enabled: false,
            ..PolicyStatus::default()
        };
        assert!(!info(dir.path(), &policy).refresh_allowed);
        let called = Cell::new(false);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let e = rt
            .block_on(refresh_with(dir.path().to_path_buf(), &policy, |_| {
                called.set(true);
                async { Ok(PAGE.to_string()) }
            }))
            .unwrap_err();
        assert_eq!(e.key, "errors.agent.pricesPolicy");
        assert!(!called.get(), "страница не загружалась");
        assert!(!prices_path(dir.path()).exists());
    }

    #[tokio::test]
    async fn the_downloaded_table_is_saved_and_errors_keep_the_previous_one() {
        let dir = tempfile::tempdir().unwrap();
        let policy = PolicyStatus::default();
        let got = refresh_with(dir.path().to_path_buf(), &policy, |url| async move {
            assert_eq!(url, "https://ollama.com/pricing");
            Ok(PAGE.to_string())
        })
        .await
        .unwrap();
        assert!(got.saved && got.refresh_allowed && got.fetched_at_ms.is_some());
        assert_eq!(got.models, 24);
        assert_eq!(got, info(dir.path(), &policy));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(prices_path(dir.path()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let before = std::fs::read(prices_path(dir.path())).unwrap();
        let net = refresh_with(dir.path().to_path_buf(), &policy, |_| async {
            Err("connection refused".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(
            (net.key.as_str(), net.code, net.details.as_deref()),
            (
                "errors.agent.pricesFetch",
                ErrorCode::ConnectionFailed,
                Some("connection refused")
            )
        );
        let parse = refresh_with(dir.path().to_path_buf(), &policy, |_| async {
            Ok("<html><body>Maintenance</body></html>".to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(parse.key, "errors.agent.pricesParse");
        assert_eq!(
            std::fs::read(prices_path(dir.path())).unwrap(),
            before,
            "прежняя таблица осталась"
        );
    }
}
