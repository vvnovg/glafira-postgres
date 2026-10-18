//! Имя продукта и идентификаторы — в одном месте. Переименование меняет только этот файл
//! и `tauri.conf.json`; тест ниже следит, чтобы они совпадали.

pub const PRODUCT_NAME: &str = "Glafira for PostgreSQL";
pub const IDENTIFIER: &str = "io.github.vvnovg.glafira.postgres";
pub const KEYCHAIN_SERVICE: &str = "glafira-postgres";
pub const POLICY_VENDOR: &str = "Glafira";
pub const POLICY_PRODUCT: &str = "PostgreSQL";
/// `application_name` соединений приложения: так их видно в `pg_stat_activity`.
pub const PG_APPLICATION_NAME: &str = "glafira-pg";

#[cfg(test)]
mod tests {
    #[test]
    fn matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], super::IDENTIFIER);
        assert_eq!(conf["productName"], super::PRODUCT_NAME);
        assert_eq!(conf["mainBinaryName"], "glafira-pg");
        assert_eq!(conf["app"]["windows"][0]["title"], super::PRODUCT_NAME);
    }
}
