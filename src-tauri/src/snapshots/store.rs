//! Файлы снимков: `data_dir/snapshots/<connectionId>/<member>/<yyyy-mm-ddThh-mm-ss>.json`, каталоги 0700, файлы 0600.
//! Каталог узла — имя узла в переносимой форме (`member_dir`: `db1:5432` → `db1%3A5432`, на Windows `:` в имени
//! нельзя); настоящее имя узла — в содержимом файла. Каталоги, созданные до кодирования (на unix — имя как есть),
//! читаются и удаляются как раньше: адрес файла — имя каталога на диске, а не имя узла.
//! В архив диагностики не входят (`diagnostics::write_archive` берёт только сведения, профили и логи).

use std::path::{Component, Path, PathBuf};

use serde::de::{IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use time::OffsetDateTime;

use crate::error::{AppError, AppResult, ErrorCode};
use crate::snapshots::Snapshot;
use crate::storage::fs::{atomic_write, newer_schema_error};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotInfo {
    /// `<каталог узла>/<имя>.json` — адрес файла в командах (каталог — `member_dir(member)` или старое имя как есть).
    pub file: String,
    pub member: String,
    pub taken_at_ms: i64,
    /// `None` — файл не прочитан (`unreadable`).
    pub rows: Option<usize>,
    pub bytes: u64,
    /// Файл есть, но не открывается: в списке — по имени и размеру, чтобы его можно было удалить; сравнение и
    /// режим «между снимками» его не принимают.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<Unreadable>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Unreadable {
    /// Больше `MAX_SNAPSHOT_BYTES`.
    TooLarge,
    /// Не разбирается как снимок (запись прервана, чужой или испорченный файл).
    Corrupt,
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

/// Время снятия по имени файла (`file_name`, UTC); `None` — имя не по шаблону.
pub fn time_of_name(name: &str) -> Option<i64> {
    let stem = name.strip_suffix(".json")?;
    let b = stem.as_bytes();
    if b.len() != 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b'-'
        || b[16] != b'-'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| stem.get(r)?.parse::<u32>().ok();
    let date = time::Date::from_calendar_date(
        i32::try_from(num(0..4)?).ok()?,
        time::Month::try_from(u8::try_from(num(5..7)?).ok()?).ok()?,
        u8::try_from(num(8..10)?).ok()?,
    )
    .ok()?;
    let t = time::Time::from_hms(
        u8::try_from(num(11..13)?).ok()?,
        u8::try_from(num(14..16)?).ok()?,
        u8::try_from(num(17..19)?).ok()?,
    )
    .ok()?;
    Some(
        time::PrimitiveDateTime::new(date, t)
            .assume_utc()
            .unix_timestamp()
            * 1000,
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

/// Имена устройств Windows: каталог с таким именем (и с любым расширением) создать нельзя.
const WINDOWS_DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

fn is_windows_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    WINDOWS_DEVICES.contains(&stem.as_str())
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
}

fn push_encoded(out: &mut String, c: char) {
    let mut buf = [0u8; 4];
    for b in c.encode_utf8(&mut buf).bytes() {
        out.push_str(&format!("%{b:02X}"));
    }
}

/// Имя каталога узла, допустимое на Windows, macOS и Linux: ASCII-буквы и цифры, `-`, `_`, `.`, пробел и не-ASCII
/// символы — как есть, остальное (в том числе `%`, `:`, `[`, `]`, разделители и управляющие) — `%XX` по байтам UTF-8.
/// Точка или пробел в конце и имя устройства Windows (`CON`, `nul.txt`) кодируются тоже. Обратное — `member_of_dir`.
pub fn member_dir(member: &str) -> String {
    let chars: Vec<char> = member.chars().collect();
    let mut out = String::with_capacity(member.len());
    for (i, &c) in chars.iter().enumerate() {
        let last = i + 1 == chars.len();
        let keep =
            (c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') || !c.is_ascii())
                && !c.is_control()
                && !(last && matches!(c, '.' | ' '))
                && !(i == 0 && is_windows_device(member));
        if keep {
            out.push(c);
        } else {
            push_encoded(&mut out, c);
        }
    }
    out
}

/// Имя узла по имени каталога (`member_dir` наоборот). Каталог не в закодированной форме (создан до кодирования или
/// чужой: `%` без двух шестнадцатеричных цифр, не UTF-8 после раскодирования) — имя как есть.
pub fn member_of_dir(dir: &str) -> String {
    let b = dir.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = dir
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            let Some(v) = hex else {
                return dir.to_string();
            };
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| dir.to_string())
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

/// Предел размера файла снимка при чтении: больше — файл не наш или повреждён (pgss.max строк по ~1 КБ и тексты).
pub const MAX_SNAPSHOT_BYTES: u64 = 128 * 1024 * 1024;

fn too_large() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.snapshots.tooLarge",
        "The snapshot file is too large (over {{count}} MiB)",
        count = MAX_SNAPSHOT_BYTES / (1024 * 1024)
    )
}

fn already_exists() -> AppError {
    app_err!(
        ErrorCode::InvalidInput,
        "errors.snapshots.exists",
        "A snapshot of this node was taken this second: wait a second and try again"
    )
}

/// Права только для владельца на каталог, в том числе уже существовавший.
fn restrict_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Шапка файла для списка: время, узел и число строк без разбора самих строк и текстов. `rowCount` нет (файл
/// ранней сборки) — строки считаются без выделения памяти.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    member: String,
    taken_at_ms: i64,
    #[serde(default)]
    row_count: Option<usize>,
    #[serde(default)]
    rows: RowCount,
}

#[derive(Default)]
struct RowCount(usize);

impl<'de> Deserialize<'de> for RowCount {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Count;
        impl<'de> Visitor<'de> for Count {
            type Value = RowCount;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an array")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<RowCount, A::Error> {
                let mut n = 0;
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    n += 1;
                }
                Ok(RowCount(n))
            }
        }
        d.deserialize_seq(Count)
    }
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

    /// Каталог подключения и узла (`member_dir`); на unix — 0700 у `snapshots/`, каталога подключения и узла, даже
    /// если они уже были.
    fn ensure_dir(&self, connection_id: &str, member_dir: &str) -> AppResult<PathBuf> {
        if !single_component(member_dir) {
            return Err(bad_file());
        }
        let conn = self.connection_dir(connection_id)?;
        let dir = conn.join(member_dir);
        let mut b = std::fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
        b.create(&dir)?;
        for d in [&self.dir, &conn, &dir] {
            restrict_dir(d)?;
        }
        Ok(dir)
    }

    /// Запись снимка (временный файл 0600 → rename). Снимок того же узла в ту же секунду уже есть — отказ: имя файла
    /// с точностью до секунды, а молча заменять снимок нельзя.
    pub fn write(&self, connection_id: &str, snap: &Snapshot) -> AppResult<SnapshotInfo> {
        let member_dir = member_dir(&snap.member);
        let dir = self.ensure_dir(connection_id, &member_dir)?;
        let name = file_name(snap.taken_at_ms);
        let path = dir.join(&name);
        if path.exists() {
            return Err(already_exists());
        }
        let bytes = if snap.row_count == snap.rows.len() {
            serde_json::to_vec(snap)?
        } else {
            serde_json::to_vec(&Snapshot {
                row_count: snap.rows.len(),
                ..snap.clone()
            })?
        };
        atomic_write(&path, &bytes)?;
        Ok(SnapshotInfo {
            file: format!("{member_dir}/{name}"),
            member: snap.member.clone(),
            taken_at_ms: snap.taken_at_ms,
            rows: Some(snap.rows.len()),
            bytes: bytes.len() as u64,
            unreadable: None,
        })
    }

    /// Снимки подключения, новые первыми. Из файла читается шапка (`Header`: время, узел, число строк — из
    /// содержимого, имя файла — только адрес). Нечитаемые и слишком большие файлы — в списке с `unreadable` (время —
    /// из имени файла), чтобы их можно было удалить; файлы не по шаблону `<member>/<имя>.json` пропускаются.
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
                if !meta.is_file() {
                    continue;
                }
                let info = match read_file::<Header>(&f.path()) {
                    Ok(h) => SnapshotInfo {
                        file,
                        member: h.member,
                        taken_at_ms: h.taken_at_ms,
                        rows: Some(h.row_count.unwrap_or(h.rows.0)),
                        bytes: meta.len(),
                        unreadable: None,
                    },
                    // Нечитаемый файл — по имени каталога, имени файла (или времени изменения) и размеру.
                    Err(_) => SnapshotInfo {
                        member: member_of_dir(&m.file_name().to_string_lossy()),
                        taken_at_ms: time_of_name(&f.file_name().to_string_lossy())
                            .or_else(|| {
                                let t = meta.modified().ok()?;
                                let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                                i64::try_from(d.as_millis()).ok()
                            })
                            .unwrap_or(0),
                        file,
                        rows: None,
                        bytes: meta.len(),
                        unreadable: Some(if meta.len() > MAX_SNAPSHOT_BYTES {
                            Unreadable::TooLarge
                        } else {
                            Unreadable::Corrupt
                        }),
                    },
                };
                out.push(info);
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

    /// Все снимки подключения — при его удалении; каталога нет — не ошибка.
    pub fn remove_connection(&self, connection_id: &str) -> AppResult<()> {
        match std::fs::remove_dir_all(self.connection_dir(connection_id)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

fn read_file<T: serde::de::DeserializeOwned>(path: &Path) -> AppResult<T> {
    if std::fs::metadata(path)?.len() > MAX_SNAPSHOT_BYTES {
        return Err(too_large());
    }
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
            kcache: false,
            kcache_since_ms: None,
            row_count: 0,
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
            ("pg-1/2025-10-29T11-00-00.json", "pg-1", Some(0))
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
        assert_eq!(
            store.write("c1", &snap).unwrap_err().key,
            "errors.snapshots.exists",
            "та же секунда — отказ, а не замена"
        );
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
        let odd = store.write("c1", &snapshot("../x", 1)).unwrap();
        assert_eq!(
            odd.file, "..%2Fx/1970-01-01T00-00-00.json",
            "имя узла не выходит из каталога"
        );
        assert!(store.delete("c1", &odd.file).unwrap());
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
    fn list_shows_corrupt_files_so_they_can_be_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let good = store
            .write("c1", &snapshot("pg-1", 1_761_735_600_000))
            .unwrap();
        let member_dir = dir.path().join("snapshots/c1/pg-1");
        std::fs::write(member_dir.join("2025-10-29T12-00-00.json"), b"{").unwrap();
        std::fs::write(member_dir.join("notes.txt"), b"x").unwrap();
        let list = store.list("c1").unwrap();
        assert_eq!(list.len(), 2, "файл не по шаблону пропускается");
        assert_eq!(
            list[0],
            SnapshotInfo {
                file: "pg-1/2025-10-29T12-00-00.json".into(),
                member: "pg-1".into(),
                taken_at_ms: 1_761_739_200_000,
                rows: None,
                bytes: 1,
                unreadable: Some(Unreadable::Corrupt),
            },
            "время — из имени файла"
        );
        assert_eq!(list[1], good);
        let v = serde_json::to_value(&list[0]).unwrap();
        assert_eq!(
            (&v["unreadable"], &v["rows"]),
            (&"corrupt".into(), &serde_json::Value::Null)
        );
        assert!(
            serde_json::to_value(&list[1])
                .unwrap()
                .get("unreadable")
                .is_none()
        );
        assert!(
            store.read("c1", &list[0].file).is_err(),
            "сравнение его не примет"
        );
        assert!(store.delete("c1", &list[0].file).unwrap());
        assert_eq!(store.list("c1").unwrap(), [good]);
    }

    #[test]
    fn member_dir_names_are_portable_and_reversible() {
        for (member, dir) in [
            ("pg-1", "pg-1"),
            ("db1:5432", "db1%3A5432"),
            ("[::1]:5432", "%5B%3A%3A1%5D%3A5432"),
            ("c:5432", "c%3A5432"),
            ("my node", "my node"),
            ("node. ", "node.%20"),
            ("..", ".%2E"),
            ("a/b\\c*?\"<>|", "a%2Fb%5Cc%2A%3F%22%3C%3E%7C"),
            ("100%", "100%25"),
            ("CON", "%43ON"),
            ("nul.txt", "%6Eul.txt"),
            ("узел-1", "узел-1"),
        ] {
            assert_eq!(member_dir(member), dir, "{member}");
            assert_eq!(member_of_dir(dir), member, "{dir}");
            assert!(single_component(dir), "{dir}");
            assert!(
                !dir.contains([':', '/', '\\', '*', '?', '"', '<', '>', '|']),
                "{dir}"
            );
        }
        assert_eq!(member_of_dir("db1:5432"), "db1:5432", "старый каталог");
        assert_eq!(member_of_dir("50%zz"), "50%zz", "не код — как есть");
    }

    /// `host:port`, IPv6 и имя с пробелом: запись, список, чтение и удаление; в каталоге — закодированное имя.
    #[test]
    fn host_port_members_round_trip_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        for member in ["db1:5432", "[::1]:5432", "my node"] {
            let snap = snapshot(member, 1_761_735_600_000);
            let info = store.write("c1", &snap).unwrap();
            let enc = member_dir(member);
            assert_eq!(info.file, format!("{enc}/2025-10-29T11-00-00.json"));
            assert_eq!(info.member, member);
            assert!(dir.path().join("snapshots/c1").join(&enc).is_dir());
            let list = store.list("c1").unwrap();
            assert_eq!(list, std::slice::from_ref(&info));
            assert_eq!(store.read("c1", &info.file).unwrap(), snap);
            assert!(store.delete("c1", &info.file).unwrap());
            assert!(store.list("c1").unwrap().is_empty());
        }
    }

    /// Файлы, записанные на unix до кодирования (каталог — имя узла как есть), читаются и удаляются по-прежнему;
    /// нечитаемый файл показывает узел по имени каталога.
    #[cfg(unix)]
    #[test]
    fn legacy_raw_member_dirs_are_still_listed_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let snap = snapshot("db1:5432", 1_761_735_600_000);
        let legacy = dir.path().join("snapshots/c1/db1:5432");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("2025-10-29T11-00-00.json"),
            serde_json::to_vec(&snap).unwrap(),
        )
        .unwrap();
        std::fs::write(legacy.join("2025-10-29T12-00-00.json"), b"{").unwrap();
        let list = store.list("c1").unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            (list[0].member.as_str(), list[0].unreadable),
            ("db1:5432", Some(Unreadable::Corrupt))
        );
        assert_eq!(
            (list[1].file.as_str(), list[1].member.as_str()),
            ("db1:5432/2025-10-29T11-00-00.json", "db1:5432")
        );
        assert_eq!(store.read("c1", &list[1].file).unwrap(), snap);
        let new = store
            .write("c1", &snapshot("db1:5432", 1_761_735_660_000))
            .unwrap();
        assert_eq!(new.file, "db1%3A5432/2025-10-29T11-01-00.json");
        assert_eq!(store.list("c1").unwrap().len(), 3);
        for i in store.list("c1").unwrap() {
            assert!(store.delete("c1", &i.file).unwrap());
        }
        assert!(!legacy.exists());
    }

    #[test]
    fn file_names_parse_back_to_time() {
        assert_eq!(
            time_of_name(&file_name(1_761_735_600_000)),
            Some(1_761_735_600_000)
        );
        assert_eq!(time_of_name("2025-13-29T11-00-00.json"), None);
        assert_eq!(time_of_name("x.json"), None);
        assert_eq!(time_of_name("2025-10-29T11-00-00.txt"), None);
    }

    /// Список берёт число строк из `rowCount`, а у файла без него — считает массив `rows`.
    #[test]
    fn list_reads_the_row_count_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let mut snap = snapshot("pg-1", 1_761_735_600_000);
        snap.rows = vec![
            pg_advisor::queries::Cumulative {
                key: pg_advisor::queries::QueryKey {
                    query_id: 1,
                    dbid: 1,
                    userid: 1,
                    toplevel: true,
                },
                datname: None,
                rolname: None,
                counters: Default::default(),
                min_exec_ms: 0.0,
                max_exec_ms: 0.0,
                stddev_exec_ms: 0.0,
            };
            3
        ];
        let info = store.write("c1", &snap).unwrap();
        assert_eq!(info.rows, Some(3), "rowCount выставляется при записи");
        assert_eq!(store.read("c1", &info.file).unwrap().row_count, 3);
        let mut v = serde_json::to_value(&snap).unwrap();
        v.as_object_mut().unwrap().remove("rowCount");
        v["takenAtMs"] = 1_761_735_660_000_i64.into();
        let member_dir = dir.path().join("snapshots/c1/pg-1");
        std::fs::write(
            member_dir.join("2025-10-29T11-01-00.json"),
            serde_json::to_vec(&v).unwrap(),
        )
        .unwrap();
        let list = store.list("c1").unwrap();
        assert_eq!(
            list.iter().map(|i| i.rows).collect::<Vec<_>>(),
            [Some(3), Some(3)]
        );
    }

    #[test]
    fn oversized_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let info = store
            .write("c1", &snapshot("pg-1", 1_761_735_600_000))
            .unwrap();
        let path = dir.path().join("snapshots/c1").join(&info.file);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_SNAPSHOT_BYTES + 1)
            .unwrap();
        assert_eq!(
            store.read("c1", &info.file).unwrap_err().key,
            "errors.snapshots.tooLarge"
        );
        let list = store.list("c1").unwrap();
        assert_eq!(
            (list.len(), list[0].unreadable, list[0].rows, list[0].bytes),
            (1, Some(Unreadable::TooLarge), None, MAX_SNAPSHOT_BYTES + 1),
            "в списке — чтобы удалить"
        );
        assert!(store.delete("c1", &info.file).unwrap());
    }

    /// Каталоги, созданные раньше с другими правами, получают 0700; удаление подключения убирает его снимки.
    #[test]
    fn existing_dirs_are_restricted_and_connection_removal_clears_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::new(dir.path());
        let member_dir = dir.path().join("snapshots/c1/pg-1");
        std::fs::create_dir_all(&member_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in ["snapshots", "snapshots/c1", "snapshots/c1/pg-1"] {
                std::fs::set_permissions(
                    dir.path().join(d),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
            store
                .write("c1", &snapshot("pg-1", 1_761_735_600_000))
                .unwrap();
            for d in ["snapshots", "snapshots/c1", "snapshots/c1/pg-1"] {
                assert_eq!(
                    std::fs::metadata(dir.path().join(d))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700,
                    "{d}"
                );
            }
        }
        #[cfg(not(unix))]
        store
            .write("c1", &snapshot("pg-1", 1_761_735_600_000))
            .unwrap();
        store
            .write("c2", &snapshot("pg-1", 1_761_735_600_000))
            .unwrap();
        store.remove_connection("c1").unwrap();
        assert!(!dir.path().join("snapshots/c1").exists());
        assert_eq!(store.list("c2").unwrap().len(), 1, "чужие снимки на месте");
        store.remove_connection("c1").unwrap();
        assert!(store.remove_connection("..").is_err());
    }
}
