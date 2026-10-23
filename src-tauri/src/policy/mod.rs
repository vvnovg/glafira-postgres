//! Корпоративная политика (спец. P1 §2, docs/admin.md): читается один раз при запуске, приоритетнее настроек пользователя.

pub mod sources;

use std::collections::BTreeMap;

use serde::Serialize;

use crate::i18n::LocalizedText;

pub const KEY_UPDATES_ENABLED: &str = "updatesEnabled";
pub const KEY_UPDATE_URL: &str = "updateUrl";
pub const KEY_LANGUAGE: &str = "language";

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
    pub errors: Vec<LocalizedText>,
}

impl Default for PolicyStatus {
    fn default() -> Self {
        Self {
            managed: false,
            source: None,
            updates_enabled: true,
            update_url: None,
            language: None,
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub status: PolicyStatus,
}

/// Политика, которую задал администратор, но которую не удалось понять, выключает обновления:
/// раз политика есть, администратор хотел ограничений.
pub fn resolve(source: &str, raw: RawPolicy) -> Policy {
    let map = match raw {
        RawPolicy::Absent => return Policy::default(),
        RawPolicy::Unreadable(e) => {
            return Policy {
                status: PolicyStatus {
                    managed: true,
                    source: Some(source.to_string()),
                    updates_enabled: false,
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
}
