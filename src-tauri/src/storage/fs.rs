use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{AppError, AppResult, ErrorCode};

/// Уникальное имя временного файла в том же каталоге: `<name>.<pid>.<n>.tmp`.
/// Параллельные писатели в один путь не делят временный файл.
fn tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Новый файл с правами только для владельца (0600 на unix): в данных приложения бывают
/// подставленные значения `{{env}}` и заголовков. rename сохраняет права временного файла.
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)
}

/// Атомарная запись: временный файл → fsync → rename → fsync каталога.
/// При сбое на любом шаге исходный файл остаётся нетронутым.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = tmp_path(path);
    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let mut f = create_private(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        sync_dir(dir);
        Ok(())
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        app_err!(
            ErrorCode::Internal,
            "errors.storage.writeFailed",
            "Failed to write {{path}}: {{error}}",
            path = path.display(),
            error = e
        )
    })
}

/// fsync каталога фиксирует rename на диске (POSIX). На Windows каталог так не открыть — пропускаем.
fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Файл записан более новой версией приложения (`schemaVersion` выше поддерживаемой).
pub fn newer_schema_error(path: &Path, version: impl std::fmt::Display) -> AppError {
    app_err!(
        ErrorCode::Internal,
        "errors.storage.newerSchema",
        "{{file}} was created by a newer version of the app (schemaVersion {{version}})",
        file = path.display(),
        version = version
    )
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Читает JSON; при повреждённом файле (не парсится) переименовывает его в
/// `<name>.corrupt-<unix-ms>` (сохраняя для пользователя), пишет предупреждение и возвращает None.
/// Файл с `schemaVersion` выше `max_schema` — не повреждение: возвращается ошибка (fail-closed).
pub fn read_json_or_quarantine<T: DeserializeOwned>(
    path: &Path,
    max_schema: u32,
) -> AppResult<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let err = match serde_json::from_slice::<T>(&bytes) {
        Ok(v) => return Ok(Some(v)),
        Err(e) => e,
    };
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && v.get("schemaVersion")
            .and_then(|x| x.as_u64())
            .is_some_and(|x| x > u64::from(max_schema))
    {
        return Err(newer_schema_error(path, &v["schemaVersion"]));
    }
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".corrupt-{ms}"));
    let backup = path.with_file_name(name);
    fs::rename(path, &backup).map_err(|e| {
        app_err!(
            ErrorCode::Internal,
            "errors.storage.quarantineFailed",
            "File {{path}} is corrupted ({{parseError}}) and a backup copy could not be saved: {{renameError}}",
            path = path.display(),
            parseError = err,
            renameError = e
        )
    })?;
    tracing::warn!(
        "file {} is corrupted ({err}); saved as {}, starting empty",
        path.display(),
        backup.display()
    );
    Ok(None)
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    atomic_write(path, &serde_json::to_vec_pretty(value)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn leftover_tmps(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "tmp"))
            .collect()
    }

    #[test]
    fn writes_replaces_and_leaves_no_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        atomic_write(&p, b"1").unwrap();
        atomic_write(&p, b"2").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"2");
        assert!(leftover_tmps(dir.path()).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn written_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.json");
        std::fs::write(&p, b"old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write(&p, b"new").unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "заменённый файл тоже 0600");
    }

    #[test]
    fn creates_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history").join("c1.json");
        atomic_write(&p, b"[]").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"[]");
    }

    #[cfg(unix)]
    #[test]
    fn failure_keeps_original_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("d");
        let p = sub.join("a.json");
        atomic_write(&p, b"old").unwrap();
        // Каталог только для чтения: временный файл не создать.
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = atomic_write(&p, b"new");
        std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(err.unwrap_err().code, ErrorCode::Internal);
        assert_eq!(std::fs::read(&p).unwrap(), b"old");
        assert!(leftover_tmps(&sub).is_empty());
    }

    #[test]
    fn failed_rename_removes_tmp() {
        let dir = tempfile::tempdir().unwrap();
        // На месте целевого файла — непустой каталог: rename не пройдёт.
        let p = dir.path().join("a.json");
        std::fs::create_dir(&p).unwrap();
        std::fs::write(p.join("x"), b"x").unwrap();
        let err = atomic_write(&p, b"new").unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(leftover_tmps(dir.path()).is_empty());
    }

    #[test]
    fn concurrent_writers_do_not_clobber_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.json");
        let a = vec!["a".repeat(64); 256];
        let b = vec!["b".repeat(64); 256];
        std::thread::scope(|s| {
            for v in [&a, &b] {
                let p = &p;
                s.spawn(move || {
                    for _ in 0..100 {
                        write_json(p, v).unwrap();
                    }
                });
            }
        });
        let got: Vec<String> = read_json(&p).unwrap().unwrap();
        assert!(got == a || got == b);
        assert!(leftover_tmps(dir.path()).is_empty());
    }

    #[test]
    fn json_roundtrip_and_missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("v.json");
        assert_eq!(read_json::<Vec<i32>>(&p).unwrap(), None);
        write_json(&p, &vec![1, 2]).unwrap();
        assert_eq!(read_json::<Vec<i32>>(&p).unwrap(), Some(vec![1, 2]));
    }
}
