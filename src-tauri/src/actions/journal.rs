//! Журнал действий (спец. P2 §8.8): `actions.jsonl`, строка — выполненное действие, ротация в `actions.1.jsonl`.
//! Логика — копия журнала MCP Glafira Kafka (`crates/glafira-mcp/src/audit.rs`). Запись — best effort: сбой не
//! откатывает действие, команда отвечает `journalWritten: false`. Учётные данные не пишутся.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::conn::profile::{ConnectionProfile, Environment};
use crate::error::ErrorCode;
use crate::sync::MutexExt;

pub const FILE_NAME: &str = "actions.jsonl";
pub const ROTATED_NAME: &str = "actions.1.jsonl";
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
/// Текст запроса в журнале — первые символы, литералы остаются (решение владельца, спец. §8.8).
pub const QUERY_CHARS: usize = 500;
pub const RESPONSE_CHARS: usize = 500;
pub const MAX_LIST: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    /// Часть пакета не выполнена.
    Partial,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordConnection {
    pub id: String,
    pub name: String,
    pub environment: Environment,
}

impl RecordConnection {
    pub fn of(p: &ConnectionProfile) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            environment: p.environment,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub pid: i32,
    pub backend_start_us: i64,
    pub user: Option<String>,
    pub database: Option<String>,
    pub application: Option<String>,
    pub client: Option<String>,
    pub state: Option<String>,
    pub xact_age_s: Option<f64>,
    /// Десятичной строкой, как в отчётах диагностики: 64-битный хеш не помещается в число JS.
    pub query_id: Option<String>,
    pub query: Option<String>,
    /// Итог сессии: `terminated` | `signalSent` | `gone` | `noLongerMatches` | `notAllowed` | `failed`.
    pub result: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotRecord {
    pub name: String,
    pub slot_type: String,
    pub plugin: Option<String>,
    pub database: Option<String>,
    pub retained_bytes: Option<i64>,
    pub wal_status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatroniRecord {
    pub method: String,
    /// Полный адрес без учётных данных.
    pub path: String,
    pub body: Option<serde_json::Value>,
    pub status: Option<u16>,
    pub response: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRecord {
    pub ts: i64,
    /// `cancel` | `terminate` | `dropSlot` | `switchover` | `cancelSwitchover` | `failover` | `restart` |
    /// `cancelRestart` | `pause` | `resume`.
    pub action: String,
    pub connection: RecordConnection,
    pub member: Option<String>,
    pub prod_confirmed: bool,
    pub outcome: Outcome,
    /// `AppError.code` при `outcome: error`.
    pub code: Option<String>,
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<SlotRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patroni: Option<PatroniRecord>,
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Код ошибки в том виде, в каком он уходит в JSON (`NOT_AUTHORIZED`): так он записывается в `ActionRecord.code`.
pub fn code_name(code: ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Первые `chars` символов (не байтов: литералы могут быть кириллицей).
pub fn cut(s: &str, chars: usize) -> String {
    s.chars().take(chars).collect()
}

pub struct ActionJournal {
    dir: PathBuf,
    file: Mutex<Option<File>>,
}

impl ActionJournal {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            file: Mutex::new(None),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Дописывает запись; `false` — не записано (причина — в журнале приложения).
    pub fn append(&self, r: &ActionRecord) -> bool {
        let mut bytes = match serde_json::to_vec(r) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("actions journal: {e}");
                return false;
            }
        };
        bytes.push(b'\n');
        match self.write(&bytes) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("actions journal not written: {e}");
                false
            }
        }
    }

    fn open(&self) -> io::Result<File> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(self.path())
    }

    fn write(&self, line: &[u8]) -> io::Result<()> {
        let mut guard = self.file.lock_safe();
        if guard.is_none() {
            *guard = Some(self.open()?);
        }
        let len = guard
            .as_ref()
            .map_or(0, |f| f.metadata().map_or(0, |m| m.len()));
        if len + line.len() as u64 > MAX_BYTES {
            *guard = None;
            std::fs::rename(self.path(), self.dir.join(ROTATED_NAME))?;
            *guard = Some(self.open()?);
        }
        let f = guard.as_mut().expect("file opened above");
        f.write_all(line)?;
        f.flush()
    }

    /// Последние `limit` (не больше 1000) записей подключения `connection_id` (или всех), новые первыми.
    pub fn list(&self, connection_id: Option<&str>, limit: usize) -> Vec<ActionRecord> {
        let mut out: Vec<ActionRecord> = Vec::new();
        for name in [ROTATED_NAME, FILE_NAME] {
            let Ok(f) = File::open(self.dir.join(name)) else {
                continue;
            };
            // По байтам, не `lines()`: строка с битым UTF-8 пропускается, а не обрывает чтение файла.
            out.extend(
                BufReader::new(f)
                    .split(b'\n')
                    .map_while(Result::ok)
                    .filter_map(|l| serde_json::from_slice::<ActionRecord>(&l).ok())
                    .filter(|r| connection_id.is_none_or(|id| r.connection.id == id)),
            );
        }
        let limit = limit.min(MAX_LIST);
        let start = out.len().saturating_sub(limit);
        out.split_off(start).into_iter().rev().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn record(conn: &str, action: &str) -> ActionRecord {
        ActionRecord {
            ts: 1_761_300_000_000,
            action: action.into(),
            connection: RecordConnection {
                id: conn.into(),
                name: format!("{conn}-name"),
                environment: Environment::Prod,
            },
            member: Some("pg-1".into()),
            prod_confirmed: true,
            outcome: Outcome::Ok,
            code: None,
            ms: 412,
            sessions: vec![SessionRecord {
                pid: 4242,
                backend_start_us: 1_761_299_000_000_000,
                user: Some("app".into()),
                database: Some("orders".into()),
                application: Some("api".into()),
                client: Some("10.0.0.5".into()),
                state: Some("idle in transaction".into()),
                xact_age_s: Some(3600.0),
                query_id: Some("-123".into()),
                query: Some("UPDATE shop.orders SET status = 'x'".into()),
                result: "terminated".into(),
            }],
            slot: None,
            patroni: None,
        }
    }

    #[test]
    fn records_lines_newest_first_and_filters_by_connection() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        assert!(j.append(&record("c1", "terminate")));
        assert!(j.append(&record("c2", "cancel")));
        assert!(j.append(&record("c1", "dropSlot")));
        let all = j.list(None, 10);
        assert_eq!(
            all.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(),
            ["dropSlot", "cancel", "terminate"]
        );
        let c1 = j.list(Some("c1"), 10);
        assert_eq!(c1.len(), 2);
        assert_eq!(j.list(None, 1).len(), 1);
        // JSON — camelCase, поля записи по спецификации §8.8.
        let line = std::fs::read_to_string(j.path()).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.lines().next().unwrap()).unwrap();
        assert_eq!(v["connection"]["environment"], "prod");
        assert_eq!(v["prodConfirmed"], true);
        assert_eq!(
            v["sessions"][0]["backendStartUs"],
            1_761_299_000_000_000_i64
        );
        assert_eq!(v["sessions"][0]["queryId"], "-123");
        assert!(
            v.get("slot").is_none() && v.get("patroni").is_none(),
            "пустые части не пишутся"
        );
    }

    #[test]
    fn limit_is_capped_at_1000() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        for _ in 0..1005 {
            j.append(&record("c", "cancel"));
        }
        assert_eq!(j.list(None, 5000).len(), 1000);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        let mode = std::fs::metadata(j.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn rotates_at_limit_and_reads_both_files() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        // Добить текущий файл до предела: следующая запись уйдёт в новый, старый станет actions.1.jsonl.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(j.path())
            .unwrap();
        f.write_all(&vec![b' '; MAX_BYTES as usize]).unwrap();
        drop(f);
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "terminate"));
        assert!(dir.path().join(ROTATED_NAME).exists());
        let r = j.list(None, 10);
        assert_eq!(
            r.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(),
            ["terminate", "cancel"]
        );
    }

    #[test]
    fn broken_lines_are_skipped_not_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let j = ActionJournal::new(dir.path());
        j.append(&record("c", "cancel"));
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(j.path())
            .unwrap();
        f.write_all(b"{\"action\":\"terminate\",\"query\":\"\xd0\n{not json\n")
            .unwrap();
        drop(f);
        j.append(&record("c", "terminate"));
        assert_eq!(j.list(None, 10).len(), 2);
    }

    #[test]
    fn cut_keeps_the_first_chars() {
        assert_eq!(cut("абвгд", 3), "абв");
        assert_eq!(cut("abc", 10), "abc");
    }

    #[test]
    fn unwritable_dir_reports_false() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no").join("such");
        let j = ActionJournal::new(&missing);
        assert!(!j.append(&record("c", "cancel")));
    }

    #[test]
    fn error_codes_are_written_as_in_json() {
        assert_eq!(code_name(ErrorCode::NotAuthorized), "NOT_AUTHORIZED");
        assert_eq!(code_name(ErrorCode::PatroniError), "PATRONI_ERROR");
        assert_eq!(code_name(ErrorCode::Timeout), "TIMEOUT");
    }
}
