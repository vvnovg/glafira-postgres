//! Настройки фонового ассистента (спец. P4c §8.3): раздел `mcp.agent` файла `settings.json` — режим, пресет, путь к
//! программе и модель каждого пресета, у Claude Code — «Через Ollama» и предел расходов, сроки, тёплая модель, адрес Ollama.
//! Секретов здесь нет: облачный Claude Code входит своей подпиской или своим ключом, Qwen Code берёт ключ из своих настроек.

use std::ops::RangeInclusive;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::presets::valid_model;
use crate::error::{AppError, AppResult, ErrorCode};

pub const DEFAULT_TIMEOUT_MIN: u32 = 15;
pub const TIMEOUT_MIN_RANGE: RangeInclusive<u32> = 2..=60;
pub const DEFAULT_KEEP_WARM_MIN: u32 = 10;
pub const KEEP_WARM_MIN_RANGE: RangeInclusive<u32> = 0..=120;
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const DEFAULT_BUDGET_USD: f64 = 1.0;
pub const MIN_BUDGET_USD: f64 = 0.01;
pub const MAX_BUDGET_USD: f64 = 100.0;
const MAX_PATH_CHARS: usize = 1024;
const MAX_URL_CHARS: usize = 256;

/// Пресет агентского CLI (спец. §4.1): флаги собирает Glafira.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    #[default]
    Qwen,
    Claude,
}

impl Preset {
    pub fn as_str(self) -> &'static str {
        match self {
            Preset::Qwen => "qwen",
            Preset::Claude => "claude",
        }
    }

    /// Имя программы для поиска: `command -v`, `PATH`, известные места.
    pub fn program_name(self) -> &'static str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct QwenSettings {
    /// Абсолютный путь к `qwen`; пусто — не настроен.
    pub path: String,
    /// Пусто — `model.name` из `~/.qwen/settings.json` (та же модель, что в чате).
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeSettings {
    pub path: String,
    /// Пусто — модель Claude Code по умолчанию; через Ollama модель обязательна.
    pub model: String,
    pub via_ollama: bool,
    /// «Предел расходов на запуск, $» без Ollama; `None` — без предела.
    pub budget_usd: Option<f64>,
}

impl Default for ClaudeSettings {
    fn default() -> Self {
        Self {
            path: String::new(),
            model: String::new(),
            via_ollama: false,
            budget_usd: Some(DEFAULT_BUDGET_USD),
        }
    }
}

/// Раздел `mcp.agent` (`#[serde(default)]`: старые файлы и недостающие поля — значения по умолчанию; версия схемы не
/// меняется).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSettings {
    /// «Запускать ассистента в фоне».
    pub enabled: bool,
    pub preset: Preset,
    pub qwen: QwenSettings,
    pub claude: ClaudeSettings,
    /// «Срок запуска, мин».
    pub timeout_min: u32,
    /// «Держать модель в памяти после запуска, мин»; 0 — Glafira в Ollama не ходит.
    pub keep_warm_min: u32,
    pub ollama_url: String,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: Preset::Qwen,
            qwen: QwenSettings::default(),
            claude: ClaudeSettings::default(),
            timeout_min: DEFAULT_TIMEOUT_MIN,
            keep_warm_min: DEFAULT_KEEP_WARM_MIN,
            ollama_url: DEFAULT_OLLAMA_URL.to_string(),
        }
    }
}

impl AgentSettings {
    /// Путь программы выбранного пресета.
    pub fn path(&self) -> &str {
        match self.preset {
            Preset::Qwen => &self.qwen.path,
            Preset::Claude => &self.claude.path,
        }
    }

    /// Модель выбранного пресета, как в настройках (пусто — модель CLI по умолчанию).
    pub fn model(&self) -> &str {
        match self.preset {
            Preset::Qwen => &self.qwen.model,
            Preset::Claude => &self.claude.model,
        }
    }

    /// Агент настроен: путь задан; Claude Code через Ollama — ещё и модель (у Ollama нет «модели по умолчанию»).
    pub fn configured(&self) -> bool {
        let model_needed = self.preset == Preset::Claude && self.claude.via_ollama;
        !self.path().trim().is_empty() && !(model_needed && self.claude.model.trim().is_empty())
    }

    /// Значения из «Настроек»: пробелы по краям убираются, пределы спеки §8.3, пути — абсолютные, имя модели — §4.1.
    pub fn validated(mut self) -> AppResult<Self> {
        for s in [
            &mut self.qwen.path,
            &mut self.qwen.model,
            &mut self.claude.path,
            &mut self.claude.model,
            &mut self.ollama_url,
        ] {
            *s = s.trim().to_string();
        }
        check_path(&self.qwen.path)?;
        check_path(&self.claude.path)?;
        for m in [&self.qwen.model, &self.claude.model] {
            if !m.is_empty() && !valid_model(m) {
                return Err(bad_model(m));
            }
        }
        if !TIMEOUT_MIN_RANGE.contains(&self.timeout_min) {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.agent.badTimeout",
                "The run time limit must be from {{min}} to {{max}} minutes",
                min = TIMEOUT_MIN_RANGE.start(),
                max = TIMEOUT_MIN_RANGE.end()
            ));
        }
        if !KEEP_WARM_MIN_RANGE.contains(&self.keep_warm_min) {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.agent.badKeepWarm",
                "Keeping the model in memory must be from {{min}} to {{max}} minutes",
                min = KEEP_WARM_MIN_RANGE.start(),
                max = KEEP_WARM_MIN_RANGE.end()
            ));
        }
        if let Some(b) = self.claude.budget_usd
            && !(b.is_finite() && (MIN_BUDGET_USD..=MAX_BUDGET_USD).contains(&b))
        {
            return Err(app_err!(
                ErrorCode::InvalidInput,
                "errors.agent.badBudget",
                "The spending limit per run must be from {{min}} to {{max}} $",
                min = MIN_BUDGET_USD,
                max = MAX_BUDGET_USD
            ));
        }
        check_ollama_url(&self.ollama_url)?;
        Ok(self)
    }
}

/// Путь задан пользователем: абсолютный (GUI-приложение не знает `PATH` оболочки, спец. §4.2), без управляющих символов.
fn check_path(p: &str) -> AppResult<()> {
    if p.is_empty() {
        return Ok(());
    }
    if p.chars().count() > MAX_PATH_CHARS
        || p.chars().any(char::is_control)
        || !Path::new(p).is_absolute()
    {
        return Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.agent.pathNotAbsolute",
            "The program path must be absolute: {{path}}",
            path = p
        ));
    }
    Ok(())
}

/// Адрес Ollama: `http(s)://хост[:порт]`. Удалённый адрес допустим (Claude Code ходит туда сам), но моделью там Glafira
/// не управляет (§6.1).
fn check_ollama_url(u: &str) -> AppResult<()> {
    let ok = u.len() <= MAX_URL_CHARS
        && reqwest::Url::parse(u).is_ok_and(|x| {
            matches!(x.scheme(), "http" | "https") && x.host_str().is_some_and(|h| !h.is_empty())
        });
    if ok {
        Ok(())
    } else {
        Err(app_err!(
            ErrorCode::InvalidInput,
            "errors.agent.badOllamaUrl",
            "Invalid Ollama address “{{url}}”: use http://host:port",
            url = u
        ))
    }
}

pub fn bad_model(m: &str) -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.badModel",
        "Invalid model name “{{model}}”: use letters, digits and ._:/@+- (up to 128 characters)",
        model = m
    )
}

pub fn disabled_by_policy() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.disabledByPolicy",
        "The background assistant is disabled by your organization"
    )
}

pub fn not_configured() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.notConfigured",
        "The background assistant is not set up: choose the program in Settings → LLM assistants (MCP)"
    )
}

pub fn model_required() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.agent.modelRequired",
        "Choose the model: Claude Code through Ollama needs a model name"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Абсолютный путь программы на этой ОС.
    fn abs(name: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\Tools\{name}.cmd")
        } else {
            format!("/usr/local/bin/{name}")
        }
    }

    #[test]
    fn defaults_and_wire_format() {
        let d = AgentSettings::default();
        assert_eq!(
            serde_json::to_value(&d).unwrap(),
            json!({
                "enabled": false,
                "preset": "qwen",
                "qwen": { "path": "", "model": "" },
                "claude": { "path": "", "model": "", "viaOllama": false, "budgetUsd": 1.0 },
                "timeoutMin": 15,
                "keepWarmMin": 10,
                "ollamaUrl": "http://127.0.0.1:11434"
            })
        );
        assert_eq!(
            serde_json::from_value::<AgentSettings>(json!({})).unwrap(),
            d
        );
        let s: AgentSettings = serde_json::from_value(json!({
            "preset": "claude",
            "claude": { "path": "/x/claude", "budgetUsd": null }
        }))
        .unwrap();
        assert_eq!(
            (s.preset, s.claude.budget_usd, s.claude.via_ollama),
            (Preset::Claude, None, false),
            "пустой предел — без предела"
        );
        assert_eq!(
            (s.timeout_min, s.keep_warm_min),
            (15, 10),
            "остальное — по умолчанию"
        );
        assert_eq!(
            (Preset::Qwen.as_str(), Preset::Claude.program_name()),
            ("qwen", "claude")
        );
    }

    #[test]
    fn path_and_model_follow_the_preset() {
        let mut s = AgentSettings {
            qwen: QwenSettings {
                path: "/q".into(),
                model: "qm".into(),
            },
            ..AgentSettings::default()
        };
        s.claude.path = "/c".into();
        s.claude.model = "cm".into();
        assert_eq!((s.path(), s.model()), ("/q", "qm"));
        s.preset = Preset::Claude;
        assert_eq!((s.path(), s.model()), ("/c", "cm"));
    }

    #[test]
    fn configured_needs_a_path_and_a_model_for_ollama() {
        let mut s = AgentSettings::default();
        assert!(!s.configured(), "путь не задан");
        s.qwen.path = abs("qwen");
        assert!(s.configured(), "у Qwen Code модель может быть пустой");
        s.preset = Preset::Claude;
        s.claude.path = abs("claude");
        assert!(s.configured(), "модель Claude Code по умолчанию");
        s.claude.via_ollama = true;
        assert!(!s.configured(), "через Ollama нужна модель");
        s.claude.model = "qwen3.6-64k:35b-a3b".into();
        assert!(s.configured());
    }

    #[test]
    fn validation_trims_and_checks_ranges() {
        let mut s = AgentSettings::default();
        s.qwen.path = format!("  {}  ", abs("qwen"));
        s.qwen.model = " qwen3.6-64k:35b-a3b ".into();
        s.ollama_url = " http://localhost:11434 ".into();
        let v = s.validated().unwrap();
        assert_eq!(v.qwen.path, abs("qwen"));
        assert_eq!(v.qwen.model, "qwen3.6-64k:35b-a3b");
        assert_eq!(v.ollama_url, "http://localhost:11434");

        let bad = |f: &dyn Fn(&mut AgentSettings)| {
            let mut s = AgentSettings::default();
            f(&mut s);
            s.validated().unwrap_err().key
        };
        assert_eq!(
            bad(&|s| s.qwen.path = "qwen".into()),
            "errors.agent.pathNotAbsolute"
        );
        assert_eq!(
            bad(&|s| s.claude.path = "bin/claude".into()),
            "errors.agent.pathNotAbsolute"
        );
        assert_eq!(
            bad(&|s| s.claude.model = "two words".into()),
            "errors.agent.badModel"
        );
        assert_eq!(bad(&|s| s.timeout_min = 1), "errors.agent.badTimeout");
        assert_eq!(bad(&|s| s.timeout_min = 61), "errors.agent.badTimeout");
        assert_eq!(bad(&|s| s.keep_warm_min = 121), "errors.agent.badKeepWarm");
        for budget in [0.0, 0.005, -1.0, f64::NAN, f64::INFINITY, 100.5] {
            assert_eq!(
                bad(&|s| s.claude.budget_usd = Some(budget)),
                "errors.agent.badBudget",
                "{budget}"
            );
        }
        for url in ["", "127.0.0.1:11434", "ftp://localhost", "http://"] {
            assert_eq!(
                bad(&|s| s.ollama_url = url.into()),
                "errors.agent.badOllamaUrl",
                "{url}"
            );
        }
        // Пределы включительно; 0 минут тёплой модели — «не управлять»; удалённая Ollama — допустимый адрес.
        let mut s = AgentSettings {
            timeout_min: 60,
            keep_warm_min: 0,
            ollama_url: "http://10.0.0.5:11434".into(),
            ..AgentSettings::default()
        };
        s.claude.budget_usd = None;
        assert!(s.validated().is_ok());
        for budget in [0.01, 100.0] {
            let mut s = AgentSettings::default();
            s.claude.budget_usd = Some(budget);
            assert!(s.validated().is_ok(), "{budget}");
        }
    }

    #[test]
    fn error_texts_name_the_limits() {
        let s = AgentSettings {
            timeout_min: 0,
            ..AgentSettings::default()
        };
        let e = s.validated().unwrap_err();
        assert_eq!(
            (e.params["min"].as_str(), e.params["max"].as_str()),
            ("2", "60")
        );
        assert_eq!(e.message, "The run time limit must be from 2 to 60 minutes");
        let mut s = AgentSettings::default();
        s.claude.budget_usd = Some(0.0);
        let e = s.validated().unwrap_err();
        assert_eq!(
            (e.params["min"].as_str(), e.params["max"].as_str()),
            ("0.01", "100")
        );
        for e in [
            disabled_by_policy(),
            not_configured(),
            model_required(),
            bad_model("x y"),
        ] {
            assert_eq!(e.code, ErrorCode::InvalidInput, "{}", e.key);
            assert!(e.key.starts_with("errors.agent."), "{}", e.key);
        }
    }
}
