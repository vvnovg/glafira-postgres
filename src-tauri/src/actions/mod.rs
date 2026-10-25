//! Действия дежурного (спец. P2 §8): отмена и завершение сессий, удаление слота, действия Patroni, наблюдение и журнал.

pub mod conn;
pub mod journal;
pub mod sessions;

use serde::Deserialize;

/// Подтверждение действия (спец. §8.1): имя подключения на prod (`confirm`) и имя объекта или число сессий
/// (`typed_name`). Проверяется `guard` в Rust до обращения к серверу.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Confirmation {
    pub confirm: Option<String>,
    pub typed_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_reads_camel_case_and_defaults_to_empty() {
        let c: Confirmation =
            serde_json::from_str(r#"{"confirm":"billing-prod","typedName":"it_dead_slot"}"#)
                .unwrap();
        assert_eq!(c.confirm.as_deref(), Some("billing-prod"));
        assert_eq!(c.typed_name.as_deref(), Some("it_dead_slot"));
        let empty: Confirmation = serde_json::from_str("{}").unwrap();
        assert_eq!((empty.confirm, empty.typed_name), (None, None));
        let nulls: Confirmation =
            serde_json::from_str(r#"{"confirm":null,"typedName":null}"#).unwrap();
        assert_eq!((nulls.confirm, nulls.typed_name), (None, None));
    }
}
