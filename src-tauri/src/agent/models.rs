//! Модели для поля «Модель» (спец. P4c §4.1, §8.3): у Qwen Code — `modelProviders.*[].id` и `model.name` из
//! `~/.qwen/settings.json` (файл только читается и в журнал не пишется) и группа «Ollama» — модели локальной Ollama
//! (`/api/tags`), которых нет в его настройках (итоговая волна правок смоука P4c); у Claude Code — `/api/tags` (`agent::ollama`,
//! задача 7). У обоих — ещё группа «Ollama Cloud»: облачные модели действующей таблицы цен (смоук P4c, sf-T7) — облачной
//! модели без `ollama pull` в `/api/tags` нет, а через Ollama она отвечает. Списки Ollama у Claude Code отдаются всегда:
//! интерфейс показывает их только при «Через Ollama» в черновике. Модель Ollama (облачную или из `/api/tags`), которой нет
//! в настройках Qwen Code, Glafira направляет к Ollama сама (`qwen_knows`, `presets::build`).

use std::path::{Path, PathBuf};

use glafira_llm_pricing::{PriceTable, Prices};
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
    /// Найденные модели: у Qwen Code — из его настроек, у Claude Code — `/api/tags` локальной Ollama.
    pub ids: Vec<String>,
    pub default: Option<String>,
    pub source: ModelsSource,
    /// Группа «Ollama» у Qwen Code (`with_ollama`): модели локальной Ollama (`/api/tags`), которых нет ни в его настройках,
    /// ни в группе «Ollama Cloud». У Claude Code пусто: его модели Ollama — сами `ids`.
    pub ollama: Vec<String>,
    /// Группа «Ollama Cloud» (`with_cloud`): облачные модели таблицы цен, которых нет в `ids`.
    pub cloud: Vec<CloudModel>,
}

impl AgentModels {
    /// Списка нет: свободный ввод.
    pub fn none() -> Self {
        Self {
            ids: Vec::new(),
            default: None,
            source: ModelsSource::None,
            ollama: Vec::new(),
            cloud: Vec::new(),
        }
    }

    pub fn from_qwen(q: QwenModels) -> Self {
        Self {
            ids: q.ids,
            default: q.default,
            source: ModelsSource::Qwen,
            ollama: Vec::new(),
            cloud: Vec::new(),
        }
    }

    /// Claude Code: модели локальной Ollama (`ollama::local_tags`, по алфавиту, без повторов); Ollama не ответила, адрес не
    /// локальный — свободный ввод.
    pub fn from_ollama(tags: Option<Vec<String>>) -> Self {
        match tags {
            Some(ids) => Self {
                ids,
                source: ModelsSource::Ollama,
                ..Self::none()
            },
            None => Self::none(),
        }
    }

    /// Группа «Ollama» у Qwen Code (вызывается после `with_cloud`): модели локальной Ollama `tags` без моделей его настроек
    /// (`ids`, `default`) и без моделей группы «Ollama Cloud» (у них подпись с ценой); имена Ollama — без учёта регистра.
    /// По алфавиту, без повторов.
    pub fn with_ollama(mut self, tags: Vec<String>) -> Self {
        let mut ollama: Vec<String> = Vec::new();
        for t in tags {
            let taken = |id: &str| t.eq_ignore_ascii_case(id);
            if self.ids.iter().any(|x| taken(x))
                || self.default.as_deref().is_some_and(taken)
                || self.cloud.iter().any(|m| taken(&m.id))
                || ollama.iter().any(|x| taken(x))
            {
                continue;
            }
            ollama.push(t);
        }
        ollama.sort();
        self.ollama = ollama;
        self
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

/// Модель есть в настройках Qwen Code — среди `modelProviders.*[].id` или в `model.name` (`qwen_models`): её адрес и ключ
/// Qwen Code знает сам, она запускается как в чате. Нет домашнего каталога или файла — не знает. Модель Ollama не из его
/// настроек адреса и ключа у Qwen Code нет («Missing API key for OpenAI-compatible auth», смоук P4c) — их даёт Glafira:
/// облачной модели (тег `cloud` — только у Ollama, смоук P4c, sf-T7) и модели из `/api/tags` локальной Ollama (итоговая
/// волна правок) — очередь, `presets::build`. Блокирующий вызов.
pub fn qwen_knows(model: &str, home: Option<&Path>) -> bool {
    let m = model.trim();
    let q = home.map(qwen_models).unwrap_or_default();
    q.ids.iter().any(|x| x == m) || q.default.as_deref() == Some(m)
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
            serde_json::json!({ "ids": ["m"], "default": "m", "source": "qwen", "ollama": [], "cloud": [] })
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
        let tags =
            AgentModels::from_ollama(Some(vec!["GPT-OSS:120b-cloud".into()])).with_cloud(table);
        assert!(!tags.cloud.iter().any(|m| m.id == "gpt-oss:120b-cloud"));
        assert_eq!(tags.cloud.len(), table.models.len() - 1);
    }

    /// Итоговая волна правок смоука P4c: у Claude Code модели Ollama — `/api/tags` (по алфавиту, без повторов), и отдаются
    /// они всегда, без оглядки на сохранённое «Через Ollama» (группы показывает интерфейс по черновику); Ollama не
    /// ответила — свободный ввод.
    #[test]
    fn claude_code_models_are_the_tags_of_ollama() {
        let m =
            AgentModels::from_ollama(Some(vec!["a:1".into(), "b:2".into()])).with_cloud(builtin());
        assert_eq!(
            (m.ids.as_slice(), m.source, m.ollama.len()),
            (
                ["a:1".to_string(), "b:2".to_string()].as_slice(),
                ModelsSource::Ollama,
                0
            )
        );
        assert_eq!(m.cloud.len(), builtin().models.len());
        assert_eq!(AgentModels::from_ollama(None), AgentModels::none());
    }

    /// Итоговая волна правок смоука P4c (решение владельца): у Qwen Code — группа «Ollama»: модели локальной Ollama, которых
    /// нет ни в его настройках (провайдеры и `model.name`), ни в группе «Ollama Cloud» (там они с ценой); облачная модель
    /// из `/api/tags` без цены — тоже здесь. Регистр имени Ollama не важен; по алфавиту, без повторов.
    #[test]
    fn local_ollama_models_follow_the_settings_of_qwen_code_without_repeats() {
        let m = AgentModels::from_qwen(QwenModels {
            ids: vec!["glm-5.3-flash:cloud".into(), "qwen3.6-64k:35b-a3b".into()],
            default: Some("qwen3-coder:30b".into()),
        })
        .with_cloud(builtin())
        .with_ollama(vec![
            "qwen3.8:27b".into(),
            "QWEN3.6-64K:35b-a3b".into(),
            "qwen3-coder:30b".into(),
            "gpt-oss:120b-cloud".into(),
            "glm-5.3-flash:cloud".into(),
            "kimi-k2:1t-cloud".into(),
            "gemma3:4b".into(),
            "qwen3.8:27b".into(),
        ]);
        assert_eq!(m.ollama, ["gemma3:4b", "kimi-k2:1t-cloud", "qwen3.8:27b"]);
        assert_eq!(m.ids, ["glm-5.3-flash:cloud", "qwen3.6-64k:35b-a3b"]);
        assert!(m.cloud.iter().any(|c| c.id == "gpt-oss:120b-cloud"));
        assert!(!m.cloud.iter().any(|c| c.id == "glm-5.3-flash:cloud"));
        let v = serde_json::to_value(&m).unwrap();
        assert_eq!(
            v["ollama"],
            serde_json::json!(["gemma3:4b", "kimi-k2:1t-cloud", "qwen3.8:27b"])
        );
    }

    /// Смоук P4c: `qwen -m glm-5.3-flash:cloud` с моделью не из `~/.qwen/settings.json` падал «Missing API key». Модели из
    /// настроек (провайдеры, `model.name`) Qwen Code знает и запускает как в чате; остальные — нет (их адрес и ключ даёт
    /// Glafira: облачные — по тегу, локальные Ollama — по `/api/tags`, очередь). Нет файла или домашнего каталога — не знает.
    #[test]
    fn qwen_code_knows_only_the_models_of_its_settings() {
        let home = home_with(
            r#"{"model": {"name": "kimi-k3:cloud"}, "modelProviders": {"openai": [
  {"id": "glm-5.3-flash:cloud", "baseUrl": "http://127.0.0.1:11434/v1", "envKey": "OLLAMA_API_KEY"},
  {"id": "qwen3.8:27b"}
]}}"#,
        );
        let h = Some(home.path());
        for m in ["glm-5.3-flash:cloud", " kimi-k3:cloud ", "qwen3.8:27b"] {
            assert!(qwen_knows(m, h), "{m}");
        }
        for m in [
            "deepseek-v4.1-flash:cloud",
            "gpt-oss:120b-cloud",
            "qwen3.6:35b",
            "gemma3:4b",
        ] {
            assert!(!qwen_knows(m, h), "{m}");
        }
        let empty = tempfile::tempdir().unwrap();
        assert!(!qwen_knows("glm-5.3-flash:cloud", Some(empty.path())));
        assert!(!qwen_knows("qwen3.8:27b", None));
    }
}
