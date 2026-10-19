//! Где ОС хранит политику администратора. Пользовательских уровней нет: политику задаёт только администратор.

use std::collections::BTreeMap;
use std::path::Path;

use super::{Policy, PolicyValue, RawPolicy, resolve};

/// Политика текущей ОС. Вызывается один раз при запуске.
pub fn load() -> Policy {
    #[cfg(target_os = "macos")]
    {
        use crate::product::IDENTIFIER;
        let path = format!("/Library/Managed Preferences/{IDENTIFIER}.plist");
        resolve(&path, read_plist_file(Path::new(&path)))
    }
    #[cfg(windows)]
    {
        use crate::product::{POLICY_PRODUCT, POLICY_VENDOR};
        let sub = format!(r"Software\Policies\{POLICY_VENDOR}\{POLICY_PRODUCT}");
        resolve(&format!(r"HKLM\{sub}"), read_registry(&sub))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use crate::product::IDENTIFIER;
        let path = format!("/etc/{IDENTIFIER}/policy.json");
        resolve(&path, read_json_file(Path::new(&path)))
    }
}

pub fn read_json_file(path: &Path) -> RawPolicy {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RawPolicy::Absent,
        Err(e) => return RawPolicy::Unreadable(e.to_string()),
    };
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return RawPolicy::Unreadable(e.to_string()),
    };
    let Some(obj) = value.as_object() else {
        return RawPolicy::Unreadable("expected a JSON object".into());
    };
    RawPolicy::Values(
        obj.iter()
            .map(|(k, v)| {
                let pv = match v {
                    serde_json::Value::Bool(b) => PolicyValue::Bool(*b),
                    serde_json::Value::String(s) => PolicyValue::Str(s.clone()),
                    serde_json::Value::Number(n) if n.is_i64() => {
                        PolicyValue::Int(n.as_i64().unwrap_or_default())
                    }
                    other => PolicyValue::Other(json_type(other).into()),
                };
                (k.clone(), pv)
            })
            .collect(),
    )
}

fn json_type(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
        _ => "unknown",
    }
}

pub fn read_plist_file(path: &Path) -> RawPolicy {
    // Только NotFound — «политики нет»; любая другая ошибка stat (нет прав и т.п.) — fail-closed.
    match std::fs::metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RawPolicy::Absent,
        Err(e) => return RawPolicy::Unreadable(e.to_string()),
    }
    let value = match plist::Value::from_file(path) {
        Ok(v) => v,
        Err(e) => return RawPolicy::Unreadable(e.to_string()),
    };
    let Some(dict) = value.as_dictionary() else {
        return RawPolicy::Unreadable("expected a dictionary (dict)".into());
    };
    RawPolicy::Values(
        dict.iter()
            .map(|(k, v)| {
                let pv = match v {
                    plist::Value::Boolean(b) => PolicyValue::Bool(*b),
                    plist::Value::String(s) => PolicyValue::Str(s.clone()),
                    plist::Value::Integer(i) => match i.as_signed() {
                        Some(n) => PolicyValue::Int(n),
                        None => PolicyValue::Other("big integer".into()),
                    },
                    _ => PolicyValue::Other("unsupported plist type".into()),
                };
                (k.clone(), pv)
            })
            .collect::<BTreeMap<_, _>>(),
    )
}

#[cfg(windows)]
pub fn read_registry(subkey: &str) -> RawPolicy {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, REG_DWORD, REG_EXPAND_SZ, REG_SZ};
    use winreg::types::FromRegValue;

    let key = match RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(subkey) {
        Ok(k) => k,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RawPolicy::Absent,
        Err(e) => return RawPolicy::Unreadable(e.to_string()),
    };
    let mut map = BTreeMap::new();
    for item in key.enum_values() {
        let (name, value) = match item {
            Ok(v) => v,
            Err(e) => return RawPolicy::Unreadable(e.to_string()),
        };
        let parsed = match value.vtype {
            REG_DWORD => u32::from_reg_value(&value).map(|d| PolicyValue::Int(i64::from(d))),
            REG_SZ | REG_EXPAND_SZ => String::from_reg_value(&value).map(PolicyValue::Str),
            other => Ok(PolicyValue::Other(format!("{other:?}"))),
        };
        match parsed {
            Ok(v) => {
                map.insert(name, v);
            }
            Err(e) => return RawPolicy::Unreadable(e.to_string()),
        }
    }
    RawPolicy::Values(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PolicyValue;

    #[test]
    fn json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        assert_eq!(read_json_file(&path), RawPolicy::Absent);
        std::fs::write(
            &path,
            r#"{"updatesEnabled": false, "updateUrl": "https://m/l.json", "x": [1]}"#,
        )
        .unwrap();
        let RawPolicy::Values(v) = read_json_file(&path) else {
            panic!()
        };
        assert_eq!(v["updatesEnabled"], PolicyValue::Bool(false));
        assert_eq!(v["updateUrl"], PolicyValue::Str("https://m/l.json".into()));
        assert!(matches!(v["x"], PolicyValue::Other(_)));
        std::fs::write(&path, "{broken").unwrap();
        assert!(matches!(read_json_file(&path), RawPolicy::Unreadable(_)));
        std::fs::write(&path, "[1, 2]").unwrap();
        assert!(matches!(read_json_file(&path), RawPolicy::Unreadable(_)));
    }

    #[test]
    fn plist_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.plist");
        assert_eq!(read_plist_file(&path), RawPolicy::Absent);
        std::fs::write(
            &path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>updatesEnabled</key><false/>
  <key>licenseKey</key><string>a.b</string>
  <key>n</key><integer>3</integer>
</dict></plist>"#,
        )
        .unwrap();
        let RawPolicy::Values(v) = read_plist_file(&path) else {
            panic!()
        };
        assert_eq!(v["updatesEnabled"], PolicyValue::Bool(false));
        assert_eq!(v["licenseKey"], PolicyValue::Str("a.b".into()));
        assert_eq!(v["n"], PolicyValue::Int(3));
        std::fs::write(&path, "not a plist").unwrap();
        assert!(matches!(read_plist_file(&path), RawPolicy::Unreadable(_)));
    }

    #[cfg(unix)]
    #[test]
    fn plist_stat_error_is_unreadable_not_absent() {
        // Путь через обычный файл даёт ENOTDIR: это не «политики нет», а ошибка (fail-closed).
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let path = file.join("p.plist");
        assert!(matches!(read_plist_file(&path), RawPolicy::Unreadable(_)));
    }

    #[test]
    fn load_on_this_machine_does_not_panic() {
        // На машине разработчика политики обычно нет; проверяем только, что чтение не падает.
        let _ = load();
    }

    #[test]
    fn documented_examples_parse() {
        let base = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/admin");
        let json = read_json_file(Path::new(&format!("{base}/policy.json")));
        let p = crate::policy::resolve("doc", json);
        assert!(p.status.errors.is_empty(), "{:?}", p.status.errors);
        assert!(p.status.has_license_key);
        assert_eq!(p.status.language.as_deref(), Some("en"));

        // Профиль MDM: содержимое PayloadContent[0] попадает в Managed Preferences как есть.
        let profile = plist::Value::from_file(format!("{base}/policy.mobileconfig")).unwrap();
        let content = profile
            .as_dictionary()
            .and_then(|d| d.get("PayloadContent"))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_dictionary())
            .expect("PayloadContent[0]");
        assert_eq!(
            content.get("PayloadType").and_then(|v| v.as_string()),
            Some(crate::product::IDENTIFIER)
        );
        for key in ["licenseKey", "language"] {
            assert!(content.contains_key(key), "{key}");
        }
        // Читается только политика уровня устройства (docs/admin.md): профиль ставится на всю систему.
        assert_eq!(
            profile
                .as_dictionary()
                .and_then(|d| d.get("PayloadScope"))
                .and_then(|v| v.as_string()),
            Some("System")
        );

        let reg = std::fs::read_to_string(format!("{base}/policy.reg")).unwrap();
        assert!(reg.contains(&format!(
            r"[HKEY_LOCAL_MACHINE\SOFTWARE\Policies\{}\{}]",
            crate::product::POLICY_VENDOR,
            crate::product::POLICY_PRODUCT
        )));
    }
}
