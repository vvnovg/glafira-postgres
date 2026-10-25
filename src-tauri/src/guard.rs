//! Защита изменяющих действий (спец. P2 §8.1): проверяется в Rust до любого обращения к серверу, а не только в UI.
//! Перенос `guard.rs` Glafira for Kafka без изменения интерфейса.

use crate::conn::profile::{ConnectionProfile, Environment};
use crate::error::{AppResult, ErrorCode};

/// read-only запрещает любые действия; prod требует ввести имя подключения.
pub fn ensure_mutation_allowed(p: &ConnectionProfile, confirm: Option<&str>) -> AppResult<()> {
    if p.read_only {
        return Err(app_err!(
            ErrorCode::ReadOnly,
            "errors.guard.readOnly",
            "Connection “{{name}}” is read-only",
            name = p.name
        ));
    }
    if p.environment == Environment::Prod && confirm.map(str::trim) != Some(p.name.trim()) {
        return Err(app_err!(
            ErrorCode::ConfirmationRequired,
            "errors.guard.prodConfirm",
            "To change a prod connection, type its name “{{name}}”",
            name = p.name
        ));
    }
    Ok(())
}

/// Удаление слота, failover и пакет на prod требуют ввести имя объекта (или число сессий).
pub fn ensure_name_confirmed(expected: &str, typed: Option<&str>) -> AppResult<()> {
    if typed.map(str::trim) == Some(expected.trim()) {
        Ok(())
    } else {
        Err(app_err!(
            ErrorCode::ConfirmationRequired,
            "errors.guard.typeToConfirm",
            "Type “{{expected}}” to confirm",
            expected = expected
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conn::profile::{Environment, sample_profile};
    use crate::error::ErrorCode;

    #[test]
    fn read_only_blocks_even_with_confirmation() {
        let mut p = sample_profile("a");
        p.read_only = true;
        let e = ensure_mutation_allowed(&p, Some(&p.name.clone())).unwrap_err();
        assert_eq!(e.code, ErrorCode::ReadOnly);
        assert_eq!(e.key, "errors.guard.readOnly");
        assert_eq!(e.params["name"], p.name);
        assert_eq!(e.message, format!("Connection “{}” is read-only", p.name));
    }

    #[test]
    fn prod_requires_connection_name() {
        let mut p = sample_profile("a");
        p.environment = Environment::Prod;
        p.name = "billing-prod".into();
        for confirm in [None, Some(""), Some("billing-pro"), Some("BILLING-PROD")] {
            let e = ensure_mutation_allowed(&p, confirm).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{confirm:?}");
            assert_eq!(e.key, "errors.guard.prodConfirm");
            assert_eq!(
                e.message,
                "To change a prod connection, type its name “billing-prod”"
            );
        }
        assert!(ensure_mutation_allowed(&p, Some("billing-prod")).is_ok());
        assert!(ensure_mutation_allowed(&p, Some(" billing-prod ")).is_ok());
    }

    #[test]
    fn other_environments_need_no_confirmation() {
        for env in [
            Environment::Local,
            Environment::Dev,
            Environment::Test,
            Environment::Stage,
        ] {
            let mut p = sample_profile("a");
            p.environment = env;
            assert!(ensure_mutation_allowed(&p, None).is_ok(), "{env:?}");
        }
    }

    #[test]
    fn object_name_must_match_exactly() {
        assert!(ensure_name_confirmed("it_dead_slot", Some("it_dead_slot")).is_ok());
        assert!(ensure_name_confirmed("it_dead_slot", Some(" it_dead_slot ")).is_ok());
        for typed in [None, Some("it_dead"), Some("IT_DEAD_SLOT")] {
            let e = ensure_name_confirmed("it_dead_slot", typed).unwrap_err();
            assert_eq!(e.code, ErrorCode::ConfirmationRequired, "{typed:?}");
            assert_eq!(e.key, "errors.guard.typeToConfirm");
            assert_eq!(e.message, "Type “it_dead_slot” to confirm");
        }
    }
}
