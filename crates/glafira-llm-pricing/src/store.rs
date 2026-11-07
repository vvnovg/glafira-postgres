//! Файл таблицы цен, загруженной со страницы («Обновить цены»): путь выбирает приложение (обычно `PRICES_FILE` в его
//! каталоге данных). Запись атомарная (временный файл → fsync → rename), права 0600 на unix. Действующая таблица —
//! сохранённая (с моделями встроенной, которых в ней нет); встроенная — если файла нет, он не читается или он старше
//! встроенной (вышел выпуск со свежей таблицей).

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::builtin::builtin;
use crate::table::{ParseError, PriceTable};

/// Имя файла таблицы в каталоге данных приложения.
pub const PRICES_FILE: &str = "ollama-prices.json";
/// Версия формата файла (`schemaVersion`); файл новее — не читается, действует встроенная таблица.
pub const SCHEMA_VERSION: u64 = 1;

/// Файл таблицы не прочитан или не записан.
#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// Не JSON таблицы.
    Format(serde_json::Error),
    /// Файл записала более новая версия (её `schemaVersion`).
    NewerSchema(u64),
    /// Таблица не прошла проверку здравого смысла.
    Invalid(ParseError),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "price table file: {e}"),
            StoreError::Format(e) => write!(f, "price table file is not valid: {e}"),
            StoreError::NewerSchema(v) => write!(
                f,
                "price table file was written by a newer version (schemaVersion {v})"
            ),
            StoreError::Invalid(e) => write!(f, "price table file: {e}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Io(e) => Some(e),
            StoreError::Format(e) => Some(e),
            StoreError::Invalid(e) => Some(e),
            StoreError::NewerSchema(_) => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError::Format(e)
    }
}

/// Таблица из файла; файла нет — `Ok(None)`. Файл новее (`schemaVersion`), не таблица или таблица неправдоподобная —
/// ошибка.
pub fn load(path: &Path) -> Result<Option<PriceTable>, StoreError> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let value: Value = serde_json::from_slice(&bytes)?;
    if let Some(v) = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .filter(|&v| v > SCHEMA_VERSION)
    {
        return Err(StoreError::NewerSchema(v));
    }
    let table: PriceTable = serde_json::from_value(value)?;
    table.validate().map_err(StoreError::Invalid)?;
    Ok(Some(table))
}

/// Атомарная запись таблицы (0600 на unix, и когда файл уже был с другими правами). Неправдоподобная таблица не
/// пишется. Каталог создаётся, если его нет. Блокирующий вызов (fsync).
pub fn save(path: &Path, table: &PriceTable) -> Result<(), StoreError> {
    table.validate().map_err(StoreError::Invalid)?;
    let mut value = serde_json::to_value(table)?;
    value["schemaVersion"] = SCHEMA_VERSION.into();
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)?;
    Ok(())
}

/// Чья таблица действует.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Загружена со страницы и сохранена.
    Saved,
    /// Встроенная в сборку.
    Builtin,
}

/// Действующая таблица.
#[derive(Debug)]
pub struct Effective {
    pub table: PriceTable,
    pub origin: Origin,
    /// Почему файл не прочитан (приложению — в журнал); нет файла — не ошибка.
    pub problem: Option<StoreError>,
}

/// Действующая таблица по файлу `path`: сохранённая, а если её нет, она не читается или день её цен раньше дня встроенной
/// — встроенная. Модели встроенной таблицы, которых нет в сохранённой (страница разобралась частично), дописываются в
/// конец (`PriceTable::supplemented_from`): оценки по ним не пропадают. Блокирующий вызов.
pub fn effective(path: &Path) -> Effective {
    let builtin = || builtin().clone();
    match load(path) {
        Ok(Some(saved)) if saved.date >= builtin().date => Effective {
            table: saved.supplemented_from(&builtin()),
            origin: Origin::Saved,
            problem: None,
        },
        Ok(_) => Effective {
            table: builtin(),
            origin: Origin::Builtin,
            problem: None,
        },
        Err(e) => Effective {
            table: builtin(),
            origin: Origin::Builtin,
            problem: Some(e),
        },
    }
}

/// Уникальное имя временного файла рядом: `<имя>.<pid>.<n>.tmp`.
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

/// Временный файл 0600 → запись → fsync → rename → fsync каталога; при сбое прежний файл цел, временный удаляется.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = tmp_path(path);
    let result = (|| {
        fs::create_dir_all(dir)?;
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        // fsync каталога фиксирует rename (POSIX); на Windows каталог так не открыть.
        #[cfg(unix)]
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
