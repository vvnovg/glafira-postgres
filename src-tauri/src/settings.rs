//! Пользовательские настройки (язык интерфейса, вид дерева, MCP) и выбор действующего языка.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{AppResult, ErrorCode};
use crate::storage::fs::{read_json, read_json_or_quarantine, write_json};
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

/// Политика → выбор → ОС → en. Все языки доступны всем.
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

/// Клиент MCP, которому пользователь разрешил подключаться (спец. P4 §4.4). Имя и версия — как их прислал клиент
/// (до 128 символов, обрезает ядро шлюза).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedClient {
    pub name: String,
    pub version: String,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
}

/// Раздел `mcp` файла `settings.json` (спец. P4 §3.3): выключатель сервера, подтверждённые клиенты и фоновый ассистент
/// (P4c §8.3). Без `Eq`: в настройках ассистента есть `f64`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub approved_clients: Vec<ApprovedClient>,
    /// Фоновый ассистент (спец. P4c §8.3); в старых файлах раздела нет — значения по умолчанию.
    #[serde(default, deserialize_with = "agent_or_default")]
    pub agent: crate::agent::settings::AgentSettings,
}

/// Раздел `agent`, который не читается (файл новой версии Glafira: другой пресет, иной тип поля), — значения по
/// умолчанию с предупреждением в лог: иначе в карантин ушёл бы весь `settings.json` (язык, выключатель MCP,
/// подтверждённые клиенты), а шим счёл бы MCP выключенным.
fn agent_or_default<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<crate::agent::settings::AgentSettings, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_else(|e| {
        tracing::warn!("settings.json: mcp.agent not read, defaults are used: {e}");
        Default::default()
    }))
}

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    schema_version: u32,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    ui: UiSettings,
    #[serde(default)]
    mcp: McpSettings,
}

/// Включён ли MCP в `settings.json` каталога `dir` — для шима `glafira-pg mcp`: без карантина и без записи
/// (файл принадлежит приложению); нечитаемый или отсутствующий файл означает «выключено».
pub fn mcp_enabled_in(dir: &Path) -> bool {
    read_json::<SettingsFile>(&dir.join("settings.json"))
        .ok()
        .flatten()
        .is_some_and(|f| f.mcp.enabled)
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
        self.update_if(|d| {
            f(d);
            true
        })
        .map(|_| ())
    }

    /// Как `update`, но файл пишется, только если `f` что-то изменил (`true`); проверка и запись идут под одной
    /// блокировкой. Блокирующий вызов.
    fn update_if(&self, f: impl FnOnce(&mut SettingsFile) -> bool) -> AppResult<bool> {
        let mut data = self.data.write_safe();
        let mut next = data.clone();
        if !f(&mut next) {
            return Ok(false);
        }
        next.schema_version = 1;
        write_json(&self.path, &next)?;
        *data = next;
        Ok(true)
    }

    pub fn mcp(&self) -> McpSettings {
        self.data.read_safe().mcp.clone()
    }

    /// Блокирующий вызов (запись файла).
    pub fn set_mcp_enabled(&self, on: bool) -> AppResult<()> {
        self.update(|d| d.mcp.enabled = on)
    }

    /// Настройки фонового ассистента (спец. P4c §8.3).
    pub fn agent(&self) -> crate::agent::settings::AgentSettings {
        self.data.read_safe().mcp.agent.clone()
    }

    /// Блокирующий вызов (запись файла). Значения проверяет вызывающий (`AgentSettings::validated`).
    pub fn set_agent(&self, a: crate::agent::settings::AgentSettings) -> AppResult<()> {
        self.update(|d| d.mcp.agent = a)
    }

    pub fn is_client_approved(&self, name: &str) -> bool {
        self.data
            .read_safe()
            .mcp
            .approved_clients
            .iter()
            .any(|c| c.name == name)
    }

    /// «Разрешить» в диалоге подтверждения: запоминает клиента или обновляет его версию и время последней
    /// сессии (`firstSeenMs` не меняется). Блокирующий вызов.
    pub fn approve_client(&self, name: &str, version: &str, now_ms: i64) -> AppResult<()> {
        self.update(|d| {
            let clients = &mut d.mcp.approved_clients;
            match clients.iter_mut().find(|c| c.name == name) {
                Some(c) => {
                    c.version = version.to_string();
                    c.last_seen_ms = now_ms;
                }
                None => clients.push(ApprovedClient {
                    name: name.to_string(),
                    version: version.to_string(),
                    first_seen_ms: now_ms,
                    last_seen_ms: now_ms,
                }),
            }
        })
    }

    /// Новая сессия уже подтверждённого клиента: обновляет версию и `lastSeenMs`. Проверка и запись — под одной
    /// блокировкой, поэтому параллельный отзыв клиента не отменяется. `Ok(false)` — клиента нет (файл не пишется).
    /// Блокирующий вызов.
    pub fn touch_client(&self, name: &str, version: &str, now_ms: i64) -> AppResult<bool> {
        self.update_if(
            |d| match d.mcp.approved_clients.iter_mut().find(|c| c.name == name) {
                Some(c) => {
                    c.version = version.to_string();
                    c.last_seen_ms = now_ms;
                    true
                }
                None => false,
            },
        )
    }

    /// «Отозвать» в «Настройках». `Ok(false)` — такого клиента не было. Блокирующий вызов.
    pub fn revoke_client(&self, name: &str) -> AppResult<bool> {
        self.update_if(|d| {
            let before = d.mcp.approved_clients.len();
            d.mcp.approved_clients.retain(|c| c.name != name);
            d.mcp.approved_clients.len() != before
        })
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
    fn language_priority() {
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

    #[test]
    fn mcp_is_off_by_default_and_the_switch_persists() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert_eq!(s.mcp(), McpSettings::default());
        assert!(!mcp_enabled_in(dir.path()));
        s.set_mcp_enabled(true).unwrap();
        assert!(SettingsStore::open(dir.path()).mcp().enabled);
        assert!(mcp_enabled_in(dir.path()));
        // Язык и MCP живут в одном файле и не затирают друг друга.
        s.set_language(Some("ru")).unwrap();
        let reopened = SettingsStore::open(dir.path());
        assert!(reopened.mcp().enabled);
        assert_eq!(reopened.language().as_deref(), Some("ru"));
        s.set_mcp_enabled(false).unwrap();
        assert!(!mcp_enabled_in(dir.path()));
    }

    #[test]
    fn approved_clients_are_remembered_touched_and_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert!(
            !s.touch_client("qwen-code", "0.26.7", 5).unwrap(),
            "незнакомого клиента touch не запоминает"
        );
        assert!(
            !dir.path().join("settings.json").exists(),
            "файл не пишется"
        );
        s.approve_client("qwen-code", "0.26.7", 10).unwrap();
        assert!(s.is_client_approved("qwen-code"));
        assert!(s.touch_client("qwen-code", "0.27.0", 20).unwrap());
        s.approve_client("claude-code", "2.1.0", 30).unwrap();
        assert_eq!(
            SettingsStore::open(dir.path()).mcp().approved_clients,
            vec![
                ApprovedClient {
                    name: "qwen-code".into(),
                    version: "0.27.0".into(),
                    first_seen_ms: 10,
                    last_seen_ms: 20,
                },
                ApprovedClient {
                    name: "claude-code".into(),
                    version: "2.1.0".into(),
                    first_seen_ms: 30,
                    last_seen_ms: 30,
                },
            ]
        );
        // Повторное «Разрешить» не дублирует клиента и не меняет firstSeen.
        s.approve_client("claude-code", "2.1.1", 40).unwrap();
        let c = s.mcp().approved_clients;
        assert_eq!(c.len(), 2);
        assert_eq!((c[1].first_seen_ms, c[1].last_seen_ms), (30, 40));
        assert!(s.revoke_client("qwen-code").unwrap());
        assert!(
            !s.revoke_client("qwen-code").unwrap(),
            "второй отзыв — нечего отзывать"
        );
        assert!(!s.is_client_approved("qwen-code"));
        assert_eq!(
            SettingsStore::open(dir.path()).mcp().approved_clients.len(),
            1
        );
    }

    #[test]
    fn mcp_settings_wire_format_is_camel_case() {
        let m = McpSettings {
            enabled: true,
            approved_clients: vec![ApprovedClient {
                name: "qwen-code".into(),
                version: "0.26.7".into(),
                first_seen_ms: 1,
                last_seen_ms: 2,
            }],
            agent: Default::default(),
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({
                "enabled": true,
                "approvedClients": [
                    {"name": "qwen-code", "version": "0.26.7", "firstSeenMs": 1, "lastSeenMs": 2}
                ],
                "agent": serde_json::to_value(crate::agent::settings::AgentSettings::default()).unwrap(),
            })
        );
        assert_eq!(
            serde_json::from_str::<McpSettings>("{}").unwrap(),
            McpSettings::default()
        );
    }

    #[test]
    fn old_settings_file_without_mcp_reads_as_off() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"schemaVersion":1,"language":"ru"}"#,
        )
        .unwrap();
        assert_eq!(
            SettingsStore::open(dir.path()).mcp(),
            McpSettings::default()
        );
        assert!(!mcp_enabled_in(dir.path()));
    }

    #[test]
    fn agent_settings_live_next_to_mcp() {
        use crate::agent::settings::{AgentSettings, Preset};
        let dir = tempfile::tempdir().unwrap();
        let s = SettingsStore::open(dir.path());
        assert_eq!(s.agent(), AgentSettings::default());
        s.set_mcp_enabled(true).unwrap();
        s.approve_client("qwen-code", "0.26.5", 1).unwrap();
        let mut a = AgentSettings {
            enabled: true,
            preset: Preset::Claude,
            ..AgentSettings::default()
        };
        a.claude.via_ollama = true;
        a.claude.model = "qwen3.6-64k:35b-a3b".into();
        s.set_agent(a.clone()).unwrap();
        let reopened = SettingsStore::open(dir.path());
        assert_eq!(reopened.agent(), a);
        let mcp = reopened.mcp();
        assert!(mcp.enabled, "раздел agent не затирает выключатель MCP");
        assert_eq!(mcp.approved_clients.len(), 1);
        assert!(
            mcp_enabled_in(dir.path()),
            "шим по-прежнему читает выключатель"
        );
    }

    #[test]
    fn an_unknown_agent_section_does_not_break_the_settings_file() {
        // Файл новой версии Glafira (другой пресет, иной тип поля) читает старая: раздел `agent` — по умолчанию,
        // язык, выключатель MCP и подтверждённые клиенты остаются, файл не уходит в карантин, шим видит выключатель.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"schemaVersion":1,"language":"ru","mcp":{"enabled":true,
               "approvedClients":[{"name":"q","version":"1","firstSeenMs":1,"lastSeenMs":2}],
               "agent":{"enabled":true,"preset":"gemini","timeoutMin":"x"}}}"#,
        )
        .unwrap();
        assert!(mcp_enabled_in(dir.path()));
        let s = SettingsStore::open(dir.path());
        assert_eq!(s.language().as_deref(), Some("ru"));
        assert!(s.mcp().enabled);
        assert_eq!(s.mcp().approved_clients.len(), 1);
        assert_eq!(s.agent(), crate::agent::settings::AgentSettings::default());
        assert!(path.exists(), "файл не переименован в .corrupt");
    }

    #[test]
    fn shim_check_never_touches_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{broken").unwrap();
        assert!(!mcp_enabled_in(dir.path()));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{broken",
            "шим не переименовывает файл в .corrupt"
        );
        assert!(!mcp_enabled_in(&dir.path().join("missing")));
    }
}
