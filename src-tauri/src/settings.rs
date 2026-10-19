//! Пользовательские настройки (язык интерфейса) и выбор действующего языка.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

use crate::error::{AppResult, ErrorCode};
use crate::storage::fs::{read_json_or_quarantine, write_json};
use crate::sync::RwLockExt;

pub const LANGUAGES: [&str; 2] = ["en", "ru"];

fn is_known(code: &str) -> bool {
    LANGUAGES.contains(&code)
}

pub fn match_language(tag: &str) -> Option<&'static str> {
    match_language_in(tag, &LANGUAGES)
}

/// Язык ОС → код из `langs`. Китайский: явный Hans → zh-CN; Hant/TW/HK/MO → zh-TW; иначе zh-CN.
fn match_language_in(tag: &str, langs: &[&'static str]) -> Option<&'static str> {
    let lower = tag.replace('_', "-").to_ascii_lowercase();
    let mut parts = lower.split('-');
    let primary = parts.next().unwrap_or("");
    let rest: Vec<&str> = parts.collect();
    let pick = |code: &'static str| langs.iter().copied().find(|c| *c == code);
    match primary {
        "pt" => pick("pt-BR"),
        "zh" => {
            let traditional = !rest.contains(&"hans")
                && rest
                    .iter()
                    .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"));
            pick(if traditional { "zh-TW" } else { "zh-CN" })
        }
        _ => langs
            .iter()
            .copied()
            .find(|c| c.eq_ignore_ascii_case(primary)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguageReason {
    Policy,
    User,
    System,
    Default,
}

/// Политика → выбор → ОС → en. Все языки доступны без лицензии.
pub fn resolve(
    system: Option<&str>,
    chosen: Option<&str>,
    policy: Option<&str>,
) -> (String, LanguageReason) {
    let candidate = policy
        .map(|p| (p.to_string(), LanguageReason::Policy))
        .or_else(|| chosen.map(|c| (c.to_string(), LanguageReason::User)))
        .or_else(|| {
            system
                .and_then(match_language)
                .map(|s| (s.to_string(), LanguageReason::System))
        });
    match candidate {
        Some((code, reason)) if is_known(&code) => (code, reason),
        _ => ("en".into(), LanguageReason::Default),
    }
}

/// Настройки интерфейса, влияющие на то, что показывает дерево.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiSettings {
    /// Системные схемы (pg_catalog, information_schema…) и роли pg_*.
    #[serde(default)]
    pub show_system_objects: bool,
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    schema_version: u32,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    ui: UiSettings,
}

pub struct SettingsStore {
    path: PathBuf,
    data: RwLock<SettingsFile>,
}

impl SettingsStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let mut data = match read_json_or_quarantine::<SettingsFile>(&path, 1) {
            Ok(f) => f.unwrap_or_default(),
            Err(e) => {
                tracing::warn!("settings not read: {}", e.message);
                SettingsFile::default()
            }
        };
        data.language = data.language.filter(|l| is_known(l));
        Self {
            path,
            data: RwLock::new(data),
        }
    }

    pub fn language(&self) -> Option<String> {
        self.data.read_safe().language.clone()
    }

    /// Блокирующий вызов: меняет копию, пишет файл и только потом обновляет кэш.
    fn update(&self, f: impl FnOnce(&mut SettingsFile)) -> AppResult<()> {
        let mut data = self.data.write_safe();
        let mut next = data.clone();
        f(&mut next);
        next.schema_version = 1;
        write_json(&self.path, &next)?;
        *data = next;
        Ok(())
    }

    /// Блокирующий вызов (запись файла).
    pub fn set_language(&self, lang: Option<&str>) -> AppResult<()> {
        if let Some(l) = lang
            && !is_known(l)
        {
            return Err(crate::app_err!(
                ErrorCode::InvalidInput,
                "errors.settings.unknownLanguage",
                "Unknown language “{{code}}”",
                code = l
            ));
        }
        self.update(|d| d.language = lang.map(String::from))
    }

    pub fn ui(&self) -> UiSettings {
        self.data.read_safe().ui.clone()
    }

    /// Блокирующий вызов (запись файла).
    pub fn set_ui(&self, ui: UiSettings) -> AppResult<()> {
        self.update(|d| d.ui = ui)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageSettings {
    pub system: Option<String>,
    pub system_match: Option<String>,
    pub chosen: Option<String>,
    pub policy: Option<String>,
    pub effective: String,
    pub reason: LanguageReason,
    pub languages: Vec<String>,
}

pub fn system_locale() -> Option<String> {
    sys_locale::get_locale()
}

pub fn language_settings(
    system: Option<String>,
    chosen: Option<String>,
    policy: Option<String>,
) -> LanguageSettings {
    let (effective, reason) = resolve(system.as_deref(), chosen.as_deref(), policy.as_deref());
    LanguageSettings {
        system_match: system.as_deref().and_then(match_language).map(String::from),
        system,
        chosen,
        policy,
        effective,
        reason,
        languages: LANGUAGES.iter().map(|c| c.to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_system_tags() {
        assert_eq!(match_language("ru_RU"), Some("ru"));
        assert_eq!(match_language("en-GB"), Some("en"));
        assert_eq!(match_language("uk-UA"), None);
        // Языков, которых пока нет в списке, приложение не угадывает.
        assert_eq!(match_language("de-AT"), None);
        assert_eq!(match_language("zh-CN"), None);
    }

    const ALL: [&str; 12] = [
        "en", "ru", "de", "fr", "es", "it", "pl", "pt-BR", "zh-CN", "zh-TW", "ja", "ko",
    ];

    #[test]
    fn matching_asian_tags() {
        for tag in [
            "zh-Hans-CN",
            "zh-CN",
            "zh-SG",
            "zh",
            "zh-Hans-HK",
            "zh-Hans",
        ] {
            assert_eq!(match_language_in(tag, &ALL), Some("zh-CN"), "{tag}");
        }
        for tag in ["zh-Hant-TW", "zh-TW", "zh-HK", "zh-MO", "zh_TW", "zh-Hant"] {
            assert_eq!(match_language_in(tag, &ALL), Some("zh-TW"), "{tag}");
        }
        assert_eq!(match_language_in("ja-JP", &ALL), Some("ja"));
        assert_eq!(match_language_in("ko-KR", &ALL), Some("ko"));
        assert_eq!(match_language_in("pt-PT", &ALL), Some("pt-BR"));
        assert_eq!(match_language_in("de-AT", &ALL), Some("de"));
        assert_eq!(match_language_in("xx", &ALL), None);
    }

    #[test]
    fn asian_tags_without_asian_languages() {
        const BEFORE_E: [&str; 8] = ["en", "ru", "de", "fr", "es", "it", "pl", "pt-BR"];
        assert_eq!(match_language_in("zh-CN", &BEFORE_E), None);
        assert_eq!(match_language_in("zh-Hant-TW", &BEFORE_E), None);
        assert_eq!(match_language_in("ja-JP", &BEFORE_E), None);
    }

    #[test]
    fn priority_without_license() {
        use LanguageReason::*;
        assert_eq!(resolve(Some("ru-RU"), None, None), ("ru".into(), System));
        assert_eq!(resolve(Some("ru"), Some("en"), None), ("en".into(), User));
        assert_eq!(resolve(None, Some("ru"), None), ("ru".into(), User));
        assert_eq!(
            resolve(Some("en"), Some("ru"), Some("en")),
            ("en".into(), Policy)
        );
        assert_eq!(resolve(Some("en"), None, Some("ru")), ("ru".into(), Policy));
        // Неизвестный язык не выбирается ни из выбора пользователя, ни из ОС.
        assert_eq!(
            resolve(Some("ru"), Some("fr"), None),
            ("en".into(), Default)
        );
        assert_eq!(resolve(Some("uk"), None, None), ("en".into(), Default));
        assert_eq!(resolve(None, None, None), ("en".into(), Default));
    }

    #[test]
    fn store_persists_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert_eq!(s.language(), None);
        s.set_language(Some("ru")).unwrap();
        assert_eq!(
            SettingsStore::open(dir.path()).language().as_deref(),
            Some("ru")
        );
        assert_eq!(
            s.set_language(Some("xx")).unwrap_err().code,
            crate::error::ErrorCode::InvalidInput
        );
        s.set_language(None).unwrap();
        assert_eq!(SettingsStore::open(dir.path()).language(), None);
    }

    #[test]
    fn ui_settings_persist() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert!(!s.ui().show_system_objects);
        s.set_ui(UiSettings {
            show_system_objects: true,
        })
        .unwrap();
        assert!(SettingsStore::open(dir.path()).ui().show_system_objects);
        // Язык и настройки интерфейса живут в одном файле и не затирают друг друга.
        s.set_language(Some("ru")).unwrap();
        let reopened = SettingsStore::open(dir.path());
        assert!(reopened.ui().show_system_objects);
        assert_eq!(reopened.language().as_deref(), Some("ru"));
    }

    #[test]
    fn ui_settings_wire_format_is_camel_case() {
        let on = UiSettings {
            show_system_objects: true,
        };
        assert_eq!(
            serde_json::to_string(&on).unwrap(),
            r#"{"showSystemObjects":true}"#
        );
        // Поле необязательное: пустой объект — значения по умолчанию.
        assert_eq!(
            serde_json::from_str::<UiSettings>("{}").unwrap(),
            UiSettings::default()
        );
    }

    #[test]
    fn old_settings_file_without_ui_reads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"schemaVersion":1,"language":"ru"}"#,
        )
        .unwrap();
        let s = SettingsStore::open(dir.path());
        assert_eq!(s.language().as_deref(), Some("ru"));
        assert_eq!(s.ui(), UiSettings::default());
    }

    #[test]
    fn corrupt_settings_do_not_block() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("settings.json"), "{broken").unwrap();
        assert_eq!(SettingsStore::open(dir.path()).language(), None);
    }
}
