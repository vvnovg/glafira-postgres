//! Модели для поля «Модель» (спец. P4c §4.1, §8.3): у Qwen Code — `modelProviders.*[].id` и `model.name` из
//! `~/.qwen/settings.json` (файл только читается и в журнал не пишется), у Claude Code через Ollama — `/api/tags`
//! (`agent::ollama`, задача 7). У обоих (кроме Claude Code без Ollama) — ещё группа «Ollama Cloud»: облачные модели
//! действующей таблицы цен (смоук P4c, sf-T7) — облачной модели без `ollama pull` в `/api/tags` нет, а через Ollama она
//! отвечает. Облачную модель, которой нет в настройках Qwen Code, Glafira направляет к Ollama сама
//! (`qwen_cloud_via_ollama`, `presets::build`).

use std::path::{Path, PathBuf};

use glafira_llm_pricing::{PriceTable, Prices};
use serde::Serialize;
use serde_json::Value;

use super::ollama::is_cloud;
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

/// Облачная модель Ollama из таблицы цен (группа «Ollama Cloud» поля «Модель»).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudModel {
    /// Имя модели Ollama: `glm-5.3-flash:cloud`, `gpt-oss:120b-cloud`.
    pub id: String,
    /// Цены за 1 млн токенов, $.
    pub prices: Prices,
    /// Цены «вне пика», если у модели они свои.
    pub off_peak: Option<Prices>,
}

/// Ответ `list_agent_models`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModels {
    pub ids: Vec<String>,
    pub default: Option<String>,
    pub source: ModelsSource,
    /// Группа «Ollama Cloud» (`with_cloud`): облачные модели таблицы цен, которых нет в `ids`; у Claude Code без Ollama —
    /// пусто (`cloud_offered`).
    pub cloud: Vec<CloudModel>,
}

impl AgentModels {
    /// Списка нет: свободный ввод.
    pub fn none() -> Self {
        Self {
            ids: Vec::new(),
            default: None,
            source: ModelsSource::None,
            cloud: Vec::new(),
        }
    }

    pub fn from_qwen(q: QwenModels) -> Self {
        Self {
            ids: q.ids,
            default: q.default,
            source: ModelsSource::Qwen,
            cloud: Vec::new(),
        }
    }

    /// Группа «Ollama Cloud»: облачные модели таблицы `table` под именами Ollama (`PriceTable::cloud_models`), в её
    /// порядке, кроме уже найденных в `ids` (имена Ollama — без учёта регистра).
    pub fn with_cloud(mut self, table: &PriceTable) -> Self {
        let found = |id: &str| self.ids.iter().any(|x| x.eq_ignore_ascii_case(id));
        let cloud = table
            .cloud_models()
            .filter(|(id, _)| !found(id))
            .map(|(id, m)| CloudModel {
                id,
                prices: m.standard,
                off_peak: m.off_peak,
            })
            .collect();
        self.cloud = cloud;
        self
    }
}

/// Группа «Ollama Cloud» в списке: у Qwen Code (облачную модель не из его настроек Glafira направит к Ollama,
/// `qwen_cloud_via_ollama`) и у Claude Code «Через Ollama» (по сохранённой настройке). Claude Code без Ollama ходит в
/// облако Anthropic — облачные модели Ollama ему не годятся.
pub fn cloud_offered(preset: Preset, s: &AgentSettings) -> bool {
    match preset {
        Preset::Qwen => true,
        Preset::Claude => s.claude.via_ollama,
    }
}

/// Qwen Code и облачная модель Ollama (`…:cloud`, `…-cloud`), которой нет в его настройках — ни среди
/// `modelProviders.*[].id`, ни в `model.name` (`qwen_models`): адреса и ключа этой модели Qwen Code не знает («Missing
/// API key for OpenAI-compatible auth», смоук P4c), их даёт Glafira — адрес Ollama из настроек ассистента
/// (`presets::build`). Модель из настроек Qwen Code и локальная модель запускаются как в чате: локальное имя не из
/// настроек может быть моделью другого провайдера Qwen Code, а облачный тег — только у Ollama. Нет домашнего каталога
/// или файла — модель Qwen Code не знает. Блокирующий вызов (чтение файла — только для облачной модели).
pub fn qwen_cloud_via_ollama(model: &str, home: Option<&Path>) -> bool {
    let m = model.trim();
    if !is_cloud(m, &Value::Null) {
        return false;
    }
    let q = home.map(qwen_models).unwrap_or_default();
    !q.ids.iter().any(|x| x == m) && q.default.as_deref() != Some(m)
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
    use glafira_llm_pricing::builtin;

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
            serde_json::json!({ "ids": ["m"], "default": "m", "source": "qwen", "cloud": [] })
        );
        assert_eq!(
            serde_json::to_value(AgentModels::none()).unwrap()["source"],
            "none"
        );
        let v = serde_json::to_value(AgentModels::none().with_cloud(builtin())).unwrap();
        let deepseek = v["cloud"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == "deepseek-v4.1-flash:cloud")
            .unwrap()
            .clone();
        assert_eq!(
            deepseek,
            serde_json::json!({
                "id": "deepseek-v4.1-flash:cloud",
                "prices": { "input": 0.30, "cachedInput": 0.006, "output": 1.20 },
                "offPeak": { "input": 0.15, "cachedInput": 0.003, "output": 0.60 }
            })
        );
        let glm = &v["cloud"].as_array().unwrap()[5];
        assert_eq!(glm["id"], "glm-5.3-flash:cloud");
        assert_eq!(glm["offPeak"], Value::Null, "{glm}");
    }

    /// Смоук P4c, sf-T7: группа «Ollama Cloud» — облачные модели действующей таблицы цен под именами Ollama (облачной
    /// модели без `ollama pull` нет в `/api/tags`), в порядке таблицы, без моделей, которые уже нашлись (настройки Qwen
    /// Code, `/api/tags`).
    #[test]
    fn cloud_models_of_the_price_table_follow_the_found_ones_without_repeats() {
        let table = builtin();
        let found = AgentModels::from_qwen(QwenModels {
            ids: vec!["glm-5.3-flash:cloud".into(), "qwen3.8:27b".into()],
            default: Some("qwen3.8:27b".into()),
        })
        .with_cloud(table);
        assert_eq!(found.ids, ["glm-5.3-flash:cloud", "qwen3.8:27b"]);
        assert_eq!(found.source, ModelsSource::Qwen);
        let ids: Vec<&str> = found.cloud.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids.len(), table.models.len() - 1, "{ids:?}");
        assert!(
            !ids.contains(&"glm-5.3-flash:cloud"),
            "уже найдена: {ids:?}"
        );
        assert_eq!(ids[0], "beam:cloud");
        assert!(ids.contains(&"gpt-oss:120b-cloud") && ids.contains(&"deepseek-v4.1-flash:cloud"));
        let gpt = found
            .cloud
            .iter()
            .find(|m| m.id == "gpt-oss:120b-cloud")
            .unwrap();
        assert_eq!(
            (gpt.prices.input, gpt.prices.output, gpt.off_peak),
            (0.15, 0.60, None)
        );
        // Имя из `/api/tags` в другом регистре — та же модель.
        let tags = AgentModels {
            ids: vec!["GPT-OSS:120b-cloud".into()],
            default: None,
            source: ModelsSource::Ollama,
            cloud: Vec::new(),
        }
        .with_cloud(table);
        assert!(!tags.cloud.iter().any(|m| m.id == "gpt-oss:120b-cloud"));
        assert_eq!(tags.cloud.len(), table.models.len() - 1);
    }

    #[test]
    fn cloud_models_are_offered_to_qwen_code_and_to_claude_code_through_ollama() {
        let mut s = AgentSettings::default();
        assert!(cloud_offered(Preset::Qwen, &s));
        assert!(!cloud_offered(Preset::Claude, &s), "Claude Code без Ollama");
        s.claude.via_ollama = true;
        assert!(cloud_offered(Preset::Claude, &s));
    }

    /// Смоук P4c: `qwen -m glm-5.3-flash:cloud` с моделью не из `~/.qwen/settings.json` падал «Missing API key». Облачной
    /// модели Ollama не из настроек Glafira даёт адрес и ключ Ollama; модели из настроек (провайдеры, `model.name`) и
    /// локальные модели — как в чате Qwen Code.
    #[test]
    fn a_cloud_model_outside_the_settings_of_qwen_code_goes_through_ollama() {
        let home = home_with(
            r#"{"model": {"name": "kimi-k3:cloud"}, "modelProviders": {"openai": [
  {"id": "glm-5.3-flash:cloud", "baseUrl": "http://127.0.0.1:11434/v1", "envKey": "OLLAMA_API_KEY"},
  {"id": "qwen3.8:27b"}
]}}"#,
        );
        let h = Some(home.path());
        for m in [
            "deepseek-v4.1-flash:cloud",
            "gpt-oss:120b-cloud",
            " glm-5.3:cloud ",
        ] {
            assert!(qwen_cloud_via_ollama(m, h), "{m}");
        }
        for m in [
            "glm-5.3-flash:cloud",
            "kimi-k3:cloud",
            "qwen3.8:27b",
            "qwen3.6:35b",
            "gpt-oss:120b",
        ] {
            assert!(!qwen_cloud_via_ollama(m, h), "{m}");
        }
        // Настроек Qwen Code нет (или домашний каталог неизвестен) — облачную модель он не знает.
        let empty = tempfile::tempdir().unwrap();
        assert!(qwen_cloud_via_ollama(
            "glm-5.3-flash:cloud",
            Some(empty.path())
        ));
        assert!(qwen_cloud_via_ollama("glm-5.3-flash:cloud", None));
        assert!(!qwen_cloud_via_ollama("qwen3.8:27b", None));
    }
}
