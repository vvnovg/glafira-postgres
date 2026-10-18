//! Логи: stderr и файл в папке логов ОС с ротацией по размеру (5 × 5 МБ).
//! Секреты в логи не пишутся нигде в коде; it_log_redaction.rs это проверяет.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_FILES: usize = 5;
pub const LOG_BASE: &str = "app";

pub struct RotatingWriter {
    dir: PathBuf,
    base: String,
    max_bytes: u64,
    keep: usize,
    file: File,
    size: u64,
}

fn file_name(base: &str, n: usize) -> String {
    if n == 0 {
        format!("{base}.log")
    } else {
        format!("{base}.{n}.log")
    }
}

impl RotatingWriter {
    pub fn new(dir: &Path, base: &str, max_bytes: u64, keep: usize) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(file_name(base, 0));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            base: base.to_string(),
            max_bytes,
            keep: keep.max(1),
            file,
            size,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        let oldest = self.dir.join(file_name(&self.base, self.keep - 1));
        let _ = std::fs::remove_file(&oldest);
        for n in (0..self.keep - 1).rev() {
            let from = self.dir.join(file_name(&self.base, n));
            if from.exists() {
                std::fs::rename(&from, self.dir.join(file_name(&self.base, n + 1)))?;
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(file_name(&self.base, 0)))?;
        self.size = 0;
        Ok(())
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct SharedWriter(Mutex<RotatingWriter>);

impl<'a> MakeWriter<'a> for SharedWriter {
    type Writer = SharedGuard<'a>;
    fn make_writer(&'a self) -> Self::Writer {
        SharedGuard(&self.0)
    }
}

struct SharedGuard<'a>(&'a Mutex<RotatingWriter>);

impl Write for SharedGuard<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Отравленный мьютекс (паника во время записи) не должен глушить логи.
        let mut w = self.0.lock().unwrap_or_else(|e| e.into_inner());
        w.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).flush()
    }
}

/// Существующие файлы логов, от новых к старым.
pub fn log_files_named(dir: &Path, base: &str) -> Vec<PathBuf> {
    (0..MAX_FILES.max(16))
        .map(|n| dir.join(file_name(base, n)))
        .filter(|p| p.exists())
        .collect()
}

pub fn log_files(dir: &Path) -> Vec<PathBuf> {
    log_files_named(dir, LOG_BASE)
}

/// stderr + файл. Если файл открыть нельзя, остаётся только stderr (и предупреждение в нём).
pub fn init(log_dir: &Path) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer().with_writer(io::stderr);
    match RotatingWriter::new(log_dir, LOG_BASE, MAX_FILE_BYTES, MAX_FILES) {
        Ok(w) => {
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(SharedWriter(Mutex::new(w)));
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .with(file)
                .try_init();
        }
        Err(e) => {
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(stderr)
                .try_init();
            tracing::warn!("log file is unavailable ({}): {e}", log_dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn rotates_by_size_and_keeps_n_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut w = RotatingWriter::new(dir.path(), "app", 100, 3).unwrap();
        for i in 0..20 {
            writeln!(w, "{i:03} {}", "x".repeat(40)).unwrap();
        }
        w.flush().unwrap();
        let files = log_files_named(dir.path(), "app");
        assert_eq!(files.len(), 3, "{files:?}");
        assert!(files[0].ends_with("app.log"));
        assert!(
            files
                .iter()
                .all(|f| std::fs::metadata(f).unwrap().len() <= 100 + 50)
        );
        let newest = std::fs::read_to_string(&files[0]).unwrap();
        assert!(newest.contains("019"), "последняя запись в текущем файле");
    }

    #[test]
    fn appends_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        writeln!(
            RotatingWriter::new(dir.path(), "app", 1000, 3).unwrap(),
            "first"
        )
        .unwrap();
        writeln!(
            RotatingWriter::new(dir.path(), "app", 1000, 3).unwrap(),
            "second"
        )
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("app.log")).unwrap();
        assert!(text.contains("first") && text.contains("second"));
    }
}
