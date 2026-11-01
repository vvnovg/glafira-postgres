//! Файлы снимков: `data_dir/snapshots/<connectionId>/<member>/<yyyy-mm-ddThh-mm-ss>.json`, каталоги 0700, файлы 0600.
//! В архив диагностики не входят (`diagnostics::write_archive` берёт только сведения, профили и логи).

use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use time::OffsetDateTime;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::snapshots::Snapshot;
use crate::storage::fs::{atomic_write, newer_schema_error};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    /// `<member>/<имя>.json` — адрес файла в командах.
    pub file: String,
    pub member: String,
    pub taken_at_ms: i64,
    pub rows: usize,
    pub bytes: u64,
}

pub struct SnapshotStore {
    dir: PathBuf,
}

/// Имя файла по времени снятия (UTC): `2025-10-29T11-00-00.json`.
pub fn file_name(taken_at_ms: i64) -> String {
    let t = OffsetDateTime::from_unix_timestamp(taken_at_ms.div_euclid(1000))
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}.json",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Ровно один обычный компонент пути (имя каталога подключения или узла): без `..`, разделителей и корня.
fn single_component(s: &str) -> bool {
    let parts: Vec<Component> = Path::new(s).components().collect();
    !s.is_empty()
        && !s.contains(['/', '\\'])
        && parts.len() == 1
        && matches!(parts[0], Component::Normal(_))
}

/// Ровно два обычных компонента: `<member>/<name>.json`, без `..` и абсолютных путей.
pub fn valid_file(file: &str) -> bool {
    let mut parts = file.split('/');
    let (Some(member), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    single_component(member) && single_component(name) && name.ends_with(".json")
}

pub(crate) fn bad_file() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.snapshots.badFile",
        "Bad snapshot file name"
    )
}

fn not_found() -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.snapshots.notFound",
        "The snapshot file is gone"
    )
}

impl SnapshotStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("snapshots"),
        }
    }

    /// Каталог подключения; id подключения — один компонент пути (команды получают его от фронтенда).
    fn connection_dir(&self, connection_id: &str) -> AppResult<PathBuf> {
        if !single_component(connection_id) {
            return Err(bad_file());
        }
        Ok(self.dir.join(connection_id))
    }

    /// Каталог подключения и узла; на unix создаётся с правами 0700.
    fn ensure_dir(&self, connection_id: &str, member: &str) -> AppResult<PathBuf> {
        if !single_component(member) {
            return Err(bad_file());
        }
        let dir = self.connection_dir(connection_id)?.join(member);
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)?;
        Ok(dir)
    }

    /// Запись снимка (временный файл 0600 → rename). Снимок того же узла в ту же секунду заменяет прежний.
    pub fn write(&self, connection_id: &str, snap: &Snapshot) -> AppResult<SnapshotInfo> {
        let dir = self.ensure_dir(connection_id, &snap.member)?;
        let name = file_name(snap.taken_at_ms);
        let bytes = serde_json::to_vec(snap)?;
        atomic_write(&dir.join(&name), &bytes)?;
        Ok(SnapshotInfo {
            file: format!("{}/{name}", snap.member),
            member: snap.member.clone(),
            taken_at_ms: snap.taken_at_ms,
            rows: snap.rows.len(),
            bytes: bytes.len() as u64,
        })
    }

    /// Снимки подключения, новые первыми. Каждый файл читается целиком (время и число строк — из содержимого, имя
    /// файла — только адрес); нечитаемые файлы пропускаются.
    pub fn list(&self, connection_id: &str) -> AppResult<Vec<SnapshotInfo>> {
        let root = self.connection_dir(connection_id)?;
        let mut out = Vec::new();
        let Ok(members) = std::fs::read_dir(&root) else {
            return Ok(out);
        };
        for m in members.flatten() {
            let Ok(files) = std::fs::read_dir(m.path()) else {
                continue;
            };
            for f in files.flatten() {
                let file = format!(
                    "{}/{}",
                    m.file_name().to_string_lossy(),
                    f.file_name().to_string_lossy()
                );
                if !valid_file(&file) {
                    continue;
                }
                let Ok(meta) = f.metadata() else { continue };
                let Ok(snap) = read_file::<Snapshot>(&f.path()) else {
                    continue;
                };
                out.push(SnapshotInfo {
                    file,
                    member: snap.member,
                    taken_at_ms: snap.taken_at_ms,
                    rows: snap.rows.len(),
                    bytes: meta.len(),
                });
            }
        }
        out.sort_by(|a, b| {
            b.taken_at_ms
                .cmp(&a.taken_at_ms)
                .then_with(|| a.file.cmp(&b.file))
        });
        Ok(out)
    }

    fn path_of(&self, connection_id: &str, file: &str) -> AppResult<PathBuf> {
        if !valid_file(file) {
            return Err(bad_file());
        }
        Ok(self.connection_dir(connection_id)?.join(file))
    }

    pub fn read(&self, connection_id: &str, file: &str) -> AppResult<Snapshot> {
        let path = self.path_of(connection_id, file)?;
        if !path.is_file() {
            return Err(not_found());
        }
        let snap: Snapshot = read_file(&path)?;
        if snap.v > crate::snapshots::SNAPSHOT_VERSION {
            return Err(newer_schema_error(&path, snap.v));
        }
        Ok(snap)
    }

    /// `false` — файла уже не было.
    pub fn delete(&self, connection_id: &str, file: &str) -> AppResult<bool> {
        let path = self.path_of(connection_id, file)?;
        match std::fs::remove_file(&path) {
            Ok(()) => {
                // Пустой каталог узла убирается; непустой (или занятый) остаётся — это не ошибка.
                if let Some(dir) = path.parent() {
                    let _ = std::fs::remove_dir(dir);
                }
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(member: &str, taken_at_ms: i64) -> Snapshot {
        Snapshot {
            v: 1,
            taken_at_ms,
            connection: crate::actions::journal::RecordConnection {
                id: "c1".into(),
                name: "C".into(),
                environment: crate::conn::profile::Environment::Dev,
            },
            member: member.into(),
            server_version_num: 170_000,
            in_recovery: false,
            pgss_version: "1.11".into(),
            stats_reset_ms: Some(1),
            dealloc: Some(0),
            max: 5000,
            track_planning: false,
            hidden: false,
            rows: vec![],
            texts: Default::default(),
            truncated: Default::default(),
        }
    }

    #[test]
    fn file_names_and_validation() {
        assert_eq!(file_name(1_761_735_600_000), "2025-10-29T11-00-00.json");
        assert_eq!(file_name(1_761_735_600_999), "2025-10-29T11-00-00.json");
        assert!(valid_file("pg-1/2025-10-29T11-00-00.json"));
        assert!(!valid_file("../x/2025-10-29T11-00-00.json"));
        assert!(!valid_file("pg-1/../../etc/passwd"));
        assert!(!valid_file("2025-10-29T11-00-00.json"), "нужен узел");
        assert!(!valid_file("pg-1/a/b.json"));
        assert!(!valid_file("/pg-1/a.json"));
        assert!(!valid_file("pg-1/.."));
        assert!(!valid_file("pg-1/a.txt"));
        assert!(!valid_file(r"pg-1\..\a.json"));
        assert!(!valid_file("./a.json"));
    }

    #[test]
    fn store_writes_lists_reads_and_deletes_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let snap = snapshot("pg-1", 1_761_735_600_000);
        let info = store.write("c1", &snap).unwrap();
        assert_eq!(
            (info.file.as_str(), info.member.as_str(), info.rows),
            ("pg-1/2025-10-29T11-00-00.json", "pg-1", 0)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.path().join("snapshots/c1").join(&info.file);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            for d in [
                path.parent().unwrap(),
                &dir.path().join("snapshots/c1"),
                &dir.path().join("snapshots"),
            ] {
                assert_eq!(
                    std::fs::metadata(d).unwrap().permissions().mode() & 0o777,
                    0o700,
                    "{}",
                    d.display()
                );
            }
        }
        let later = store
            .write("c1", &snapshot("pg-1", 1_761_735_660_000))
            .unwrap();
        let list = store.list("c1").unwrap();
        assert_eq!(
            list.iter().map(|i| i.file.as_str()).collect::<Vec<_>>(),
            [later.file.as_str(), info.file.as_str()],
            "новые первыми"
        );
        assert_eq!(list[1], info);
        assert!(store.list("other").unwrap().is_empty());
        let back = store.read("c1", &info.file).unwrap();
        assert_eq!(back, snap);
        assert!(store.read("c1", "../c1/x.json").is_err());
        assert_eq!(
            store
                .read("c1", "pg-1/2000-01-01T00-00-00.json")
                .unwrap_err()
                .key,
            "errors.snapshots.notFound"
        );
        assert_eq!(
            store.list("../c1").unwrap_err().key,
            "errors.snapshots.badFile",
            "id подключения — один компонент"
        );
        assert!(store.write("c1", &snapshot("../x", 1)).is_err());
        assert!(store.delete("c1", &info.file).unwrap());
        assert!(!store.delete("c1", &info.file).unwrap());
        assert!(store.delete("c1", &later.file).unwrap());
        assert!(store.list("c1").unwrap().is_empty());
        assert!(
            !dir.path().join("snapshots/c1/pg-1").exists(),
            "пустой каталог узла убран"
        );
    }

    /// Битый файл (запись прервана, чужой файл) не мешает списку.
    #[test]
    fn list_skips_unreadable_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        store
            .write("c1", &snapshot("pg-1", 1_761_735_600_000))
            .unwrap();
        let member_dir = dir.path().join("snapshots/c1/pg-1");
        std::fs::write(member_dir.join("2025-10-29T12-00-00.json"), b"{").unwrap();
        std::fs::write(member_dir.join("notes.txt"), b"x").unwrap();
        assert_eq!(store.list("c1").unwrap().len(), 1);
    }
}
