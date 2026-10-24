//! Эталоны DDL: фикстура `TableDetails` (JSON) → `table_ddl` → сравнение с `*.sql`. Обновить эталон:
//! `UPDATE_SNAPSHOTS=1 cargo test --test ddl_snapshots`.

use glafira_pg_lib::catalog::ddl::table_ddl;
use glafira_pg_lib::catalog::table::TableDetails;

#[test]
fn ddl_matches_snapshots() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/ddl");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            let details: TableDetails =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let actual = table_ddl(&details);
            let expected_path = path.with_extension("sql");
            if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
                std::fs::write(&expected_path, &actual).unwrap();
            }
            let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();
            assert_eq!(actual, expected, "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 3, "фикстур меньше, чем ожидалось");
}
