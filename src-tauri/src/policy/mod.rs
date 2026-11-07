//! Корпоративная политика (спец. P1 §2, docs/admin.md): читается один раз при запуске, приоритетнее настроек пользователя.

pub mod sources;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::i18n::LocalizedText;

pub const KEY_UPDATES_ENABLED: &str = "updatesEnabled";
pub const KEY_UPDATE_URL: &str = "updateUrl";
pub const KEY_LANGUAGE: &str = "language";
pub const KEY_MCP_ENABLED: &str = "mcpEnabled";
pub const KEY_MCP_MAX_ACCESS: &str = "mcpMaxAccess";
pub const KEY_MCP_AGENT_ENABLED: &str = "mcpAgentEnabled";

/// Потолок уровня доступа LLM, заданный администратором (спец. P4 §4.2): действующий уровень подключения —
/// минимум из `llmAccess` и потолка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpMaxAccess {
    Diagnostics,
    #[default]
    Data,
}

impl McpMaxAccess {
    fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "diagnostics" => Some(McpMaxAccess::Diagnostics),
            "data" => Some(McpMaxAccess::Data),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PolicyValue {
    Bool(bool),
    Int(i64),
    Str(String),
    /// Значение неподдерживаемого типа (техническое имя типа по-английски: `array`, `float`…).
    Other(String),
}

impl PolicyValue {
    /// «Недопустимое значение» ключа политики `key`.
    fn invalid(&self, key: &str) -> LocalizedText {
        match self {
            PolicyValue::Bool(b) => ltext!(
                "errors.policy.invalidBool",
                "{{key}}: invalid boolean {{value}}",
                key = key,
                value = b
            ),
            PolicyValue::Int(i) => ltext!(
                "errors.policy.invalidNumber",
                "{{key}}: invalid number {{value}}",
                key = key,
                value = i
            ),
            PolicyValue::Str(s) => ltext!(
                "errors.policy.invalidString",
                "{{key}}: invalid string “{{value}}”",
                key = key,
                value = s
            ),
            PolicyValue::Other(t) => ltext!(
                "errors.policy.invalidType",
                "{{key}}: invalid value of type {{kind}}",
                key = key,
                kind = t
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawPolicy {
    Absent,
    Values(BTreeMap<String, PolicyValue>),
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyStatus {
    pub managed: bool,
    pub source: Option<String>,
    pub updates_enabled: bool,
    pub update_url: Option<String>,
    /// Язык интерфейса, заданный администратором.
    pub language: Option<String>,
    /// `false` — MCP-шлюз запрещён администратором (спец. P4 §4.2): сервер не запускается, шим выходит с ошибкой.
    pub mcp_enabled: bool,
    /// Потолок уровня доступа LLM к подключениям.
    pub mcp_max_access: McpMaxAccess,
    /// `false` — фоновый ассистент запрещён администратором (спец. P4c §9.1): подраздел неактивен, `start_agent_run`
    /// отвечает ошибкой, билеты не выдаются. Действует только вместе с `mcp_enabled` (`agent_allowed`).
    pub mcp_agent_enabled: bool,
    pub errors: Vec<LocalizedText>,
}

impl PolicyStatus {
    /// MCP разрешён политикой.
    pub fn mcp_allowed(&self) -> bool {
        self.mcp_enabled
    }

    /// Фоновый ассистент разрешён политикой: разрешены и MCP, и сам ассистент (спец. P4c §9.1).
    pub fn agent_allowed(&self) -> bool {
        self.mcp_enabled && self.mcp_agent_enabled
    }
}

impl Default for PolicyStatus {
    fn default() -> Self {
        Self {
            managed: false,
            source: None,
            updates_enabled: true,
            update_url: None,
            language: None,
            mcp_enabled: true,
            mcp_max_access: McpMaxAccess::Data,
            mcp_agent_enabled: true,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub status: PolicyStatus,
}

/// Политика, которую задал администратор, но которую не удалось понять, выключает обновления и MCP:
/// раз политика есть, администратор хотел ограничений. Неверный ключ MCP запрещает только MCP (и вместе с ним фонового
/// ассистента), неверный `mcpAgentEnabled` — только фонового ассистента.
pub fn resolve(source: &str, raw: RawPolicy) -> Policy {
    let map = match raw {
        RawPolicy::Absent => return Policy::default(),
        RawPolicy::Unreadable(e) => {
            return Policy {
                status: PolicyStatus {
                    managed: true,
                    source: Some(source.to_string()),
                    updates_enabled: false,
                    mcp_enabled: false,
                    mcp_agent_enabled: false,
                    errors: vec![ltext!(
                        "errors.policy.unreadable",
                        "Policy cannot be read: {{reason}}",
                        reason = e
                    )],
                    ..PolicyStatus::default()
                },
            };
        }
        RawPolicy::Values(map) => map,
    };
    let mut status = PolicyStatus {
        managed: true,
        source: Some(source.to_string()),
        ..PolicyStatus::default()
    };
    let mut broken = false;
    let mut mcp_broken = false;
    let mut agent_broken = false;
    for (key, value) in map {
        match (key.as_str(), value) {
            (KEY_UPDATES_ENABLED, PolicyValue::Bool(b)) => status.updates_enabled = b,
            (KEY_UPDATES_ENABLED, PolicyValue::Int(i @ (0 | 1))) => status.updates_enabled = i == 1,
            (KEY_UPDATE_URL, PolicyValue::Str(u)) if u.starts_with("https://") => {
                status.update_url = Some(u)
            }
            (KEY_UPDATE_URL, PolicyValue::Str(u)) if u.starts_with("http://") => {
                broken = true;
                status.errors.push(ltext!(
                    "errors.policy.httpsRequired",
                    "{{key}}: an https:// address is required — updates are not downloaded over http",
                    key = key
                ));
            }
            (KEY_LANGUAGE, PolicyValue::Str(s))
                if crate::settings::LANGUAGES.contains(&s.trim()) =>
            {
                status.language = Some(s.trim().to_string());
            }
            // Неверный язык не отключает обновления: это не ограничение, а только настройка вида.
            (KEY_LANGUAGE, PolicyValue::Str(s)) => status.errors.push(ltext!(
                "errors.policy.badLanguage",
                "{{key}}: unknown language “{{value}}”",
                key = key,
                value = s
            )),
            (KEY_LANGUAGE, other) => status.errors.push(other.invalid(&key)),
            (KEY_MCP_ENABLED, PolicyValue::Bool(b)) => status.mcp_enabled = b,
            (KEY_MCP_ENABLED, PolicyValue::Int(i @ (0 | 1))) => status.mcp_enabled = i == 1,
            (KEY_MCP_MAX_ACCESS, PolicyValue::Str(s)) => match McpMaxAccess::parse(&s) {
                Some(level) => status.mcp_max_access = level,
                None => {
                    mcp_broken = true;
                    status.errors.push(PolicyValue::Str(s).invalid(&key));
                }
            },
            // Непонятое значение ключа MCP запрещает MCP целиком, но обновления не выключает.
            (KEY_MCP_ENABLED | KEY_MCP_MAX_ACCESS, other) => {
                mcp_broken = true;
                status.errors.push(other.invalid(&key));
            }
            (KEY_MCP_AGENT_ENABLED, PolicyValue::Bool(b)) => status.mcp_agent_enabled = b,
            (KEY_MCP_AGENT_ENABLED, PolicyValue::Int(i @ (0 | 1))) => {
                status.mcp_agent_enabled = i == 1
            }
            // Непонятое значение запрещает только фонового ассистента: MCP и обновления остаются, как заданы.
            (KEY_MCP_AGENT_ENABLED, other) => {
                agent_broken = true;
                status.errors.push(other.invalid(&key));
            }
            (KEY_UPDATES_ENABLED | KEY_UPDATE_URL, other) => {
                broken = true;
                status.errors.push(other.invalid(&key));
            }
            (unknown, _) => tracing::warn!("unknown policy key “{unknown}” ignored"),
        }
    }
    if broken {
        status.updates_enabled = false;
    }
    if mcp_broken {
        status.mcp_enabled = false;
    }
    if agent_broken {
        status.mcp_agent_enabled = false;
    }
    Policy { status }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, PolicyValue)]) -> RawPolicy {
        RawPolicy::Values(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn absent_policy_is_unmanaged_with_updates() {
        let p = resolve("/etc/x/policy.json", RawPolicy::Absent);
        assert!(!p.status.managed);
        assert!(p.status.updates_enabled);
    }

    #[test]
    fn values_are_applied() {
        let p = resolve(
            "src",
            values(&[
                ("updatesEnabled", PolicyValue::Bool(false)),
                (
                    "updateUrl",
                    PolicyValue::Str("https://mirror.corp/latest.json".into()),
                ),
                ("language", PolicyValue::Str("en".into())),
            ]),
        );
        assert!(p.status.managed);
        assert_eq!(p.status.source.as_deref(), Some("src"));
        assert!(!p.status.updates_enabled);
        assert_eq!(
            p.status.update_url.as_deref(),
            Some("https://mirror.corp/latest.json")
        );
        assert_eq!(p.status.language.as_deref(), Some("en"));
        assert!(p.status.errors.is_empty());
    }

    #[test]
    fn language_policy() {
        let p = resolve(
            "src",
            values(&[("language", PolicyValue::Str("ru".into()))]),
        );
        assert_eq!(p.status.language.as_deref(), Some("ru"));
        assert!(p.status.errors.is_empty());

        let p = resolve(
            "src",
            values(&[("language", PolicyValue::Str("xx".into()))]),
        );
        assert_eq!(p.status.language, None);
        assert!(
            p.status.updates_enabled,
            "неизвестный язык не выключает обновления"
        );
        assert_eq!(p.status.errors[0].key, "errors.policy.badLanguage");
        assert_eq!(p.status.errors[0].params["value"], "xx");

        let p = resolve("src", values(&[("language", PolicyValue::Bool(true))]));
        assert_eq!(p.status.language, None);
        assert!(p.status.updates_enabled);
        assert_eq!(p.status.errors[0].key, "errors.policy.invalidBool");
    }

    #[test]
    fn registry_dword_is_bool() {
        let p = resolve("src", values(&[("updatesEnabled", PolicyValue::Int(0))]));
        assert!(!p.status.updates_enabled);
        let p = resolve("src", values(&[("updatesEnabled", PolicyValue::Int(1))]));
        assert!(p.status.updates_enabled);
    }

    #[test]
    fn broken_policy_disables_updates() {
        let p = resolve("src", RawPolicy::Unreadable("plist: unexpected EOF".into()));
        assert!(p.status.managed);
        assert!(!p.status.updates_enabled);
        assert_eq!(p.status.errors[0].key, "errors.policy.unreadable");
        assert_eq!(p.status.errors[0].params["reason"], "plist: unexpected EOF");
        // Неверный тип или адрес — тоже «безопасно закрыто», даже если updatesEnabled = true.
        for bad in [
            values(&[("updatesEnabled", PolicyValue::Str("yes".into()))]),
            values(&[
                ("updatesEnabled", PolicyValue::Bool(true)),
                ("updateUrl", PolicyValue::Str("ftp://x".into())),
            ]),
        ] {
            let p = resolve("src", bad);
            assert!(!p.status.updates_enabled);
            assert_eq!(p.status.errors.len(), 1, "{:?}", p.status.errors);
        }
        let p = resolve(
            "src",
            values(&[("updatesEnabled", PolicyValue::Str("yes".into()))]),
        );
        assert_eq!(p.status.errors[0].key, "errors.policy.invalidString");
        assert_eq!(p.status.errors[0].params["key"], "updatesEnabled");
        assert_eq!(p.status.errors[0].params["value"], "yes");
    }

    #[test]
    fn plain_http_update_url_is_rejected() {
        // Релизный updater отвергает не-https (InsecureTransportProtocol): ошибка видна сразу в политике.
        let p = resolve(
            "src",
            values(&[(
                "updateUrl",
                PolicyValue::Str("http://mirror.corp/latest.json".into()),
            )]),
        );
        assert!(!p.status.updates_enabled);
        assert_eq!(p.status.update_url, None);
        assert_eq!(p.status.errors.len(), 1);
        assert_eq!(p.status.errors[0].key, "errors.policy.httpsRequired");
        assert_eq!(p.status.errors[0].params["key"], "updateUrl");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let p = resolve("src", values(&[("futureKey", PolicyValue::Bool(true))]));
        assert!(p.status.managed);
        assert!(p.status.updates_enabled);
        assert!(p.status.errors.is_empty());
    }

    #[test]
    fn legacy_license_key_is_ignored() {
        // Ключ политик прежних сборок: неизвестный ключ — предупреждение в лог; соседние ключи применяются,
        // обновления не выключаются, в интерфейс значение не уходит.
        let p = resolve(
            "src",
            values(&[
                ("licenseKey", PolicyValue::Str("secret.key".into())),
                ("language", PolicyValue::Str("en".into())),
            ]),
        );
        assert!(p.status.managed);
        assert!(p.status.updates_enabled);
        assert!(p.status.errors.is_empty());
        assert_eq!(p.status.language.as_deref(), Some("en"));
        let json = serde_json::to_string(&p.status).unwrap();
        assert!(!json.contains("secret.key"));
        assert!(!json.to_lowercase().contains("license"));
        // Нестроковое значение — тоже просто неизвестный ключ, а не ошибка политики.
        let p = resolve("src", values(&[("licenseKey", PolicyValue::Bool(true))]));
        assert!(p.status.updates_enabled);
        assert!(p.status.errors.is_empty());
    }

    #[test]
    fn mcp_is_allowed_with_full_ceiling_by_default() {
        for p in [
            resolve("src", RawPolicy::Absent),
            resolve("src", values(&[])),
        ] {
            assert!(p.status.mcp_enabled);
            assert!(p.status.mcp_allowed());
            assert_eq!(p.status.mcp_max_access, McpMaxAccess::Data);
        }
    }

    #[test]
    fn mcp_policy_values_are_applied() {
        let p = resolve(
            "src",
            values(&[
                ("mcpEnabled", PolicyValue::Bool(false)),
                ("mcpMaxAccess", PolicyValue::Str("diagnostics".into())),
            ]),
        );
        assert!(!p.status.mcp_allowed());
        assert_eq!(p.status.mcp_max_access, McpMaxAccess::Diagnostics);
        assert!(p.status.errors.is_empty());
        assert!(p.status.updates_enabled, "ключи MCP обновлений не касаются");
        // Реестр Windows: dword 0/1; строка уровня — с пробелами по краям.
        let p = resolve(
            "src",
            values(&[
                ("mcpEnabled", PolicyValue::Int(1)),
                ("mcpMaxAccess", PolicyValue::Str(" data ".into())),
            ]),
        );
        assert!(p.status.mcp_enabled);
        assert_eq!(p.status.mcp_max_access, McpMaxAccess::Data);
        let p = resolve("src", values(&[("mcpEnabled", PolicyValue::Int(0))]));
        assert!(!p.status.mcp_enabled);
    }

    #[test]
    fn unreadable_policy_forbids_mcp() {
        let p = resolve("src", RawPolicy::Unreadable("plist: unexpected EOF".into()));
        assert!(!p.status.mcp_allowed());
    }

    #[test]
    fn invalid_mcp_values_forbid_mcp_but_keep_updates() {
        let cases = [
            (
                "mcpEnabled",
                PolicyValue::Str("yes".into()),
                "errors.policy.invalidString",
            ),
            (
                "mcpEnabled",
                PolicyValue::Int(2),
                "errors.policy.invalidNumber",
            ),
            (
                "mcpMaxAccess",
                PolicyValue::Str("full".into()),
                "errors.policy.invalidString",
            ),
            (
                "mcpMaxAccess",
                PolicyValue::Bool(true),
                "errors.policy.invalidBool",
            ),
            (
                "mcpMaxAccess",
                PolicyValue::Other("array".into()),
                "errors.policy.invalidType",
            ),
        ];
        for (key, value, error) in cases {
            // mcpEnabled = true рядом не спасает: раз политика есть, администратор хотел ограничений.
            let p = resolve(
                "src",
                values(&[
                    ("mcpEnabled", PolicyValue::Bool(true)),
                    (key, value.clone()),
                ]),
            );
            assert!(!p.status.mcp_enabled, "{key}: {value:?}");
            assert!(p.status.updates_enabled, "{key}: обновления не выключаются");
            assert_eq!(p.status.errors.len(), 1, "{:?}", p.status.errors);
            assert_eq!(p.status.errors[0].key, error);
            assert_eq!(p.status.errors[0].params["key"], key);
        }
    }

    #[test]
    fn mcp_policy_wire_format() {
        let p = resolve(
            "src",
            values(&[("mcpMaxAccess", PolicyValue::Str("diagnostics".into()))]),
        );
        let v = serde_json::to_value(&p.status).unwrap();
        assert_eq!(v["mcpEnabled"], true);
        assert_eq!(v["mcpMaxAccess"], "diagnostics");
        assert_eq!(
            serde_json::to_value(PolicyStatus::default()).unwrap()["mcpMaxAccess"],
            "data"
        );
    }

    #[test]
    fn the_background_assistant_is_allowed_by_default_and_needs_mcp() {
        for p in [
            resolve("src", RawPolicy::Absent),
            resolve("src", values(&[])),
        ] {
            assert!(p.status.mcp_agent_enabled);
            assert!(p.status.agent_allowed());
        }
        let p = resolve(
            "src",
            values(&[("mcpAgentEnabled", PolicyValue::Bool(false))]),
        );
        assert!(!p.status.agent_allowed());
        assert!(p.status.mcp_allowed(), "MCP остаётся разрешён");
        assert!(p.status.errors.is_empty());
        // Реестр Windows: dword 0/1.
        let p = resolve("src", values(&[("mcpAgentEnabled", PolicyValue::Int(0))]));
        assert!(!p.status.mcp_agent_enabled);
        let p = resolve("src", values(&[("mcpAgentEnabled", PolicyValue::Int(1))]));
        assert!(p.status.agent_allowed());
        // Запрет MCP запрещает и фонового ассистента.
        let p = resolve(
            "src",
            values(&[
                ("mcpEnabled", PolicyValue::Bool(false)),
                ("mcpAgentEnabled", PolicyValue::Bool(true)),
            ]),
        );
        assert!(p.status.mcp_agent_enabled);
        assert!(!p.status.agent_allowed());
    }

    #[test]
    fn unreadable_policy_or_a_bad_value_forbid_the_assistant() {
        let p = resolve("src", RawPolicy::Unreadable("plist: unexpected EOF".into()));
        assert!(!p.status.mcp_agent_enabled);
        assert!(!p.status.agent_allowed());
        for (value, error) in [
            (
                PolicyValue::Str("yes".into()),
                "errors.policy.invalidString",
            ),
            (PolicyValue::Int(2), "errors.policy.invalidNumber"),
            (
                PolicyValue::Other("array".into()),
                "errors.policy.invalidType",
            ),
        ] {
            let p = resolve("src", values(&[("mcpAgentEnabled", value.clone())]));
            assert!(!p.status.mcp_agent_enabled, "{value:?}");
            assert!(p.status.mcp_enabled, "{value:?}: MCP не выключается");
            assert!(
                p.status.updates_enabled,
                "{value:?}: обновления не выключаются"
            );
            assert_eq!(p.status.errors.len(), 1, "{:?}", p.status.errors);
            assert_eq!(p.status.errors[0].key, error);
            assert_eq!(p.status.errors[0].params["key"], "mcpAgentEnabled");
        }
        // Неверное значение рядом с `mcpAgentEnabled = true` (ключи читаются по алфавиту) — всё равно запрет.
        let p = resolve(
            "src",
            values(&[
                ("mcpAgentEnabled", PolicyValue::Bool(true)),
                ("mcpMaxAccess", PolicyValue::Str("full".into())),
            ]),
        );
        assert!(
            !p.status.agent_allowed(),
            "запрет MCP запрещает и ассистента"
        );
    }

    #[test]
    fn agent_policy_wire_format() {
        let v = serde_json::to_value(PolicyStatus::default()).unwrap();
        assert_eq!(v["mcpAgentEnabled"], true);
    }
}
