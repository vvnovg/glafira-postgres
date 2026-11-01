//! Имя продукта и идентификаторы — в одном месте. Переименование меняет только этот файл
//! и `tauri.conf.json`; тест ниже следит, чтобы они совпадали.

pub const PRODUCT_NAME: &str = "Glafira for PostgreSQL";
pub const IDENTIFIER: &str = "io.github.vvnovg.glafira.postgres";
pub const KEYCHAIN_SERVICE: &str = "glafira-postgres";
pub const POLICY_VENDOR: &str = "Glafira";
pub const POLICY_PRODUCT: &str = "PostgreSQL";
/// `application_name` соединений приложения: так их видно в `pg_stat_activity`.
pub const PG_APPLICATION_NAME: &str = "glafira-pg";

/// Литерал SQL с ведущей меткой периодических запросов Глафиры: `marked_sql!("SELECT …")`. Метка — комментарий:
/// `pg_stat_statements` хранит его в тексте строки, а `queryid` его не учитывает (спец. P3 §4).
#[macro_export]
macro_rules! marked_sql {
    ($sql:literal) => {
        concat!("/* glafira */ ", $sql)
    };
}

/// Метка периодических запросов Глафиры (опрос топологии, сэмплеры, диагностика по таймеру); в строках `format!` —
/// `{SQL_MARKER}` в начале.
pub const SQL_MARKER: &str = marked_sql!("");

/// Текст запроса начинается с метки Глафиры (ведущие пробелы и переводы строк не мешают).
pub fn is_marked_sql(text: &str) -> bool {
    text.trim_start().starts_with(SQL_MARKER.trim_end())
}

#[cfg(test)]
mod tests {
    #[test]
    fn marked_sql_is_recognised() {
        assert_eq!(marked_sql!("SELECT 1"), "/* glafira */ SELECT 1");
        assert!(super::is_marked_sql(marked_sql!("SELECT 1")));
        assert!(super::is_marked_sql(&format!(
            "{}SELECT $1",
            super::SQL_MARKER
        )));
        assert!(super::is_marked_sql("  \n\t/* glafira */ SELECT 1"));
        assert!(super::is_marked_sql("/* glafira */\nSELECT 1"));
        assert!(!super::is_marked_sql("SELECT 1 /* glafira */"));
        assert!(!super::is_marked_sql("/* glafira-like */ SELECT 1"));
        assert!(!super::is_marked_sql(""));
    }

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
