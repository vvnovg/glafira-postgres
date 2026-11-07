//! Модели для поля «Модель» (спец. P4c §4.1, §8.3): у Qwen Code — `modelProviders.*[].id` и `model.name` из
//! `~/.qwen/settings.json` (файл только читается и в журнал не пишется), у Claude Code через Ollama — `/api/tags`
//! (`agent::ollama`, задача 7).

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use super::presets::valid_model;
use super::settings::{AgentSettings, Preset};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QwenModels {
    pub ids: Vec<String>,
    /// `model.name` — модель чата Qwen Code.
    pub default: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelsSource {
    Qwen,
    Ollama,
    None,
}

/// Ответ `list_agent_models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModels {
    pub ids: Vec<String>,
    pub default: Option<String>,
    pub source: ModelsSource,
}

impl AgentModels {
    /// Списка нет: свободный ввод.
    pub fn none() -> Self {
        Self {
            ids: Vec::new(),
            default: None,
            source: ModelsSource::None,
        }
    }

    pub fn from_qwen(q: QwenModels) -> Self {
        Self {
            ids: q.ids,
            default: q.default,
            source: ModelsSource::Qwen,
        }
    }
}

pub fn qwen_settings_path(home: &Path) -> PathBuf {
    home.join(".qwen").join("settings.json")
}

/// Модели Qwen Code из его настроек (комментарии `//` и `/* */` допустимы); нет файла или он не читается — пусто.
/// Маршруты только для картинок и голоса (`imageOnly`, `realtimeOnly`) и неверные имена пропускаются; список — без
/// повторов, по алфавиту. Блокирующий вызов.
pub fn qwen_models(home: &Path) -> QwenModels {
    let Ok(text) = std::fs::read_to_string(qwen_settings_path(home)) else {
        return QwenModels::default();
    };
    let Ok(v) = serde_json::from_str::<Value>(&strip_json_comments(&text)) else {
        return QwenModels::default();
    };
    let default = v
        .pointer("/model/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| valid_model(m))
        .map(String::from);
    let mut ids: Vec<String> = Vec::new();
    if let Some(providers) = v.get("modelProviders").and_then(Value::as_object) {
        for list in providers.values().filter_map(Value::as_array) {
            for m in list {
                let flag = |k: &str| m.get(k).and_then(Value::as_bool) == Some(true);
                if flag("imageOnly") || flag("realtimeOnly") {
                    continue;
                }
                if let Some(id) = m.get("id").and_then(Value::as_str).map(str::trim)
                    && valid_model(id)
                    && !ids.iter().any(|x| x == id)
                {
                    ids.push(id.to_string());
                }
            }
        }
    }
    ids.sort();
    QwenModels { ids, default }
}

/// Модель запуска: из настроек пресета; у Qwen Code пустое поле — `model.name` его настроек (та же, что в чате). У
/// Claude Code без модели — `None` (модель CLI по умолчанию). Блокирующий вызов (чтение файла).
pub fn effective_model(s: &AgentSettings, home: Option<&Path>) -> Option<String> {
    let m = s.model().trim();
    if !m.is_empty() {
        return Some(m.to_string());
    }
    match s.preset {
        Preset::Qwen => home.and_then(|h| qwen_models(h).default),
        Preset::Claude => None,
    }
}

/// Комментарии `//` и `/* */` вне строк JSON.
fn strip_json_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, chars.peek().copied()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::settings::ClaudeSettings;

    fn home_with(settings: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".qwen")).unwrap();
        std::fs::write(qwen_settings_path(home.path()), settings).unwrap();
        home
    }

    #[test]
    fn qwen_models_come_from_its_settings_with_comments() {
        let home = home_with(
            r#"{
  // модель чата
  "model": { "name": "qwen3.6-64k:35b-a3b" },
  "modelProviders": {
    "openai": [
      { "id": "qwen3.6-64k:35b-a3b", "baseUrl": "http://127.0.0.1:11434/v1" },
      { "id": "qwen3.8:27b" },
      { "id": "image-model", "imageOnly": true },
      { "id": "voice", "realtimeOnly": true },
      { "id": "bad id with spaces" }
    ],
    /* второй провайдер */
    "anthropic": [{ "id": "claude-sonnet-4-5" }, { "id": "qwen3.8:27b" }]
  }
}"#,
        );
        let m = qwen_models(home.path());
        assert_eq!(m.default.as_deref(), Some("qwen3.6-64k:35b-a3b"));
        assert_eq!(
            m.ids,
            ["claude-sonnet-4-5", "qwen3.6-64k:35b-a3b", "qwen3.8:27b"]
        );
    }

    #[test]
    fn comment_markers_inside_strings_are_kept() {
        let home = home_with(
            r#"{"model": {"name": "m1"}, "modelProviders": {"openai": [
  {"id": "m2", "baseUrl": "http://host:1/v1", "note": "a /* b */ \"c\" // d"}
]}}"#,
        );
        assert_eq!(qwen_models(home.path()).ids, ["m2"]);
    }

    #[test]
    fn a_missing_or_broken_file_gives_no_models() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(qwen_models(empty.path()), QwenModels::default());
        let broken = home_with("{ not json");
        assert_eq!(qwen_models(broken.path()), QwenModels::default());
        let odd = home_with(
            r#"{"model": {"name": "two words"}, "modelProviders": {"openai": {"id": "x"}}}"#,
        );
        assert_eq!(qwen_models(odd.path()), QwenModels::default());
    }

    #[test]
    fn the_run_model_is_the_setting_or_the_default_of_qwen_code() {
        let home = home_with(r#"{"model": {"name": "qwen3.6-64k:35b-a3b"}}"#);
        let mut s = AgentSettings::default();
        assert_eq!(
            effective_model(&s, Some(home.path())).as_deref(),
            Some("qwen3.6-64k:35b-a3b")
        );
        assert_eq!(effective_model(&s, None), None);
        s.qwen.model = "qwen3.8:27b".into();
        assert_eq!(
            effective_model(&s, Some(home.path())).as_deref(),
            Some("qwen3.8:27b")
        );
        s.preset = Preset::Claude;
        s.claude = ClaudeSettings::default();
        assert_eq!(
            effective_model(&s, Some(home.path())),
            None,
            "модель Claude Code по умолчанию неизвестна"
        );
    }

    #[test]
    fn models_wire_format() {
        let v = serde_json::to_value(AgentModels::from_qwen(QwenModels {
            ids: vec!["m".into()],
            default: Some("m".into()),
        }))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "ids": ["m"], "default": "m", "source": "qwen" })
        );
        assert_eq!(
            serde_json::to_value(AgentModels::none()).unwrap()["source"],
            "none"
        );
    }
}
