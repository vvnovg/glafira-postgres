//! Остатки прежних сборок в каталоге данных. Значения файлов в лог не пишутся.

use std::path::Path;

/// Файл лицензионного ключа сборок с подсистемой ключей: ключи больше не используются,
/// файл удаляется при запуске.
const LEGACY_LICENSE_FILE: &str = "license.json";

/// Удаляет `license.json` из каталога данных, если он остался от прежних сборок. Ошибка удаления
/// не мешает запуску: файл больше ни на что не влияет.
pub fn remove_legacy_license_file(data_dir: &Path) {
    let path = data_dir.join(LEGACY_LICENSE_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(path = %path.display(), "legacy license file removed"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(path = %path.display(), "legacy license file not removed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_license_file_is_removed_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.json");
        std::fs::write(&path, "{\"token\":\"a.b\"}").unwrap();
        remove_legacy_license_file(dir.path());
        assert!(!path.exists());
        // Повторный вызов без файла — тихий.
        remove_legacy_license_file(dir.path());
        assert!(!path.exists());
    }

    #[test]
    fn failure_to_remove_does_not_panic() {
        // Каталог на месте файла: remove_file даёт ошибку, отличную от NotFound; запуск продолжается.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("license.json");
        std::fs::create_dir(&path).unwrap();
        remove_legacy_license_file(dir.path());
        assert!(path.is_dir());
    }
}
