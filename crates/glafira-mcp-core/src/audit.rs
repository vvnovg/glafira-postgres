//! Перенос из Glafira for Kafka (crates/glafira-mcp/src/audit.rs): поля чтения сообщений Kafka
//! (`scanned`/`returned`/`masked`) заменены общим `extra` (счётчики приложения: `masked`, `rows`), у вызова есть
//! `member`, событие вызова — структура `CallRecord`, события приложения — `App`, имена файлов — параметр,
//! `recent` → `tail`, поля `sql` / `rewrittenSql` пишутся целиком до 16 КБ.
//! Журнал MCP (спец. P4 §10.3): строка — событие, ротация при 5 МБ во второй файл. Результаты вызовов не пишутся.
//! P4c: у записей сессии фонового запуска — поле `run` (id запуска), у `approved` по билету — ещё `by: "run"`; у
//! обычных сессий этих полей нет.
//! Сбой записи журнала не ломает вызов (best effort).

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::limits::{MAX_ARGS_AUDIT_BYTES, MAX_SQL_AUDIT_BYTES};

pub const FILE_NAME: &str = "mcp-audit.jsonl";
pub const ROTATED_NAME: &str = "mcp-audit.1.jsonl";
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Ok,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditConnection {
    pub id: String,
    pub name: String,
}

/// Вызов инструмента. `client`, `connection`, `code` пишутся `null`, когда их нет; `run` (P4c), `member` и счётчики
/// `extra` (`masked`, `rows`) — только когда есть, на верхнем уровне строки.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    pub session: String,
    /// Id фонового запуска (P4c §9.2); у обычной сессии поля нет.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub client: Option<String>,
    pub tool: String,
    pub connection: Option<AuditConnection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    pub args: Value,
    pub outcome: Outcome,
    pub code: Option<String>,
    pub bytes: usize,
    pub ms: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum AuditEvent {
    #[serde(rename_all = "camelCase")]
    ServerStarted {
        endpoint: String,
    },
    ServerStopped,
    #[serde(rename_all = "camelCase")]
    SessionStarted {
        session: String,
        /// Id фонового запуска (P4c §9.2).
        #[serde(skip_serializing_if = "Option::is_none")]
        run: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Approved {
        session: String,
        client: Option<String>,
        /// Id фонового запуска (P4c §9.2).
        #[serde(skip_serializing_if = "Option::is_none")]
        run: Option<String>,
        /// Кто подтвердил: `"run"` — билет фонового запуска (P4c §5.1); без поля — пользователь в диалоге.
        #[serde(skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Rejected {
        session: String,
        client: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Revoked {
        client: String,
    },
    #[serde(rename_all = "camelCase")]
    SessionEnded {
        session: String,
        client: Option<String>,
        calls: u64,
    },
    Call(CallRecord),
    /// Событие приложения (P4b: `requestCreated`, `proposalAnswered`): в строке `event` = `kind`, поля `fields` — на
    /// верхнем уровне. Строку собирает [`AuditLog::record`], а не derive.
    #[serde(skip)]
    App {
        kind: String,
        fields: Map<String, Value>,
    },
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Начало строки не длиннее `max` байт по границе символа; второе значение — что-то отрезано.
fn cut_utf8(mut s: String, max: usize) -> (String, bool) {
    if s.len() <= max {
        return (s, false);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    (s, true)
}

/// Аргументы вызова для журнала (спец. §10.3): строковые поля `full_fields` (`sql`, `rewrittenSql`) — целиком до
/// [`MAX_SQL_AUDIT_BYTES`] (длиннее — с `…`), остальные — целиком, если их JSON не длиннее [`MAX_ARGS_AUDIT_BYTES`],
/// иначе одной строкой `_rest` (JSON с `…`). Что-то усечено — `_truncated: true`. Не объект — строка JSON до
/// [`MAX_ARGS_AUDIT_BYTES`], как в Kafka.
pub fn truncate_args(args: Value, full_fields: &[&str]) -> Value {
    let mut rest = match args {
        Value::Object(m) => m,
        other => {
            return match cut_utf8(other.to_string(), MAX_ARGS_AUDIT_BYTES) {
                (s, true) => Value::String(format!("{s}…")),
                (_, false) => other,
            };
        }
    };
    let mut cut = false;
    let mut full = Map::new();
    for name in full_fields {
        match rest.remove(*name) {
            Some(Value::String(s)) => {
                let (s, c) = cut_utf8(s, MAX_SQL_AUDIT_BYTES);
                cut |= c;
                full.insert(
                    (*name).to_string(),
                    Value::String(if c { format!("{s}…") } else { s }),
                );
            }
            Some(other) => {
                rest.insert((*name).to_string(), other);
            }
            None => {}
        }
    }
    let rest_json = serde_json::to_string(&rest).unwrap_or_default();
    let mut out = if rest_json.len() <= MAX_ARGS_AUDIT_BYTES {
        rest
    } else {
        cut = true;
        let (s, _) = cut_utf8(rest_json, MAX_ARGS_AUDIT_BYTES);
        Map::from_iter([("_rest".to_string(), Value::String(format!("{s}…")))])
    };
    out.extend(full);
    if cut {
        out.insert("_truncated".into(), Value::Bool(true));
    }
    Value::Object(out)
}

/// Строка журнала: поля события, `event` и `ts`. У события приложения `event` и `ts` из `fields` не подменяют настоящие.
fn line(ts: u64, event: &AuditEvent) -> serde_json::Result<Value> {
    let mut map = match event {
        AuditEvent::App { kind, fields } => {
            let mut m = fields.clone();
            m.insert("event".into(), Value::String(kind.clone()));
            m
        }
        e => {
            let Value::Object(m) = serde_json::to_value(e)? else {
                unreachable!("событие с тегом — объект JSON")
            };
            m
        }
    };
    map.insert("ts".into(), ts.into());
    Ok(Value::Object(map))
}

pub struct AuditLog {
    path: PathBuf,
    rotated: PathBuf,
    file: Mutex<Option<File>>,
}

impl AuditLog {
    /// Журнал `mcp-audit.jsonl` (после ротации — ещё `mcp-audit.1.jsonl`) в каталоге `dir`.
    pub fn new(dir: &Path) -> Self {
        Self::with_names(dir, FILE_NAME, ROTATED_NAME)
    }

    pub fn with_names(dir: &Path, file_name: &str, rotated_name: &str) -> Self {
        Self {
            path: dir.join(file_name),
            rotated: dir.join(rotated_name),
            file: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn record(&self, event: AuditEvent) {
        let mut bytes = match line(now_ms(), &event).and_then(|v| serde_json::to_vec(&v)) {
            Ok(b) => b,
            Err(e) => return tracing::warn!("mcp audit: {e}"),
        };
        bytes.push(b'\n');
        if let Err(e) = self.write(&bytes) {
            tracing::warn!("mcp audit not written: {e}");
        }
    }

    fn open(&self) -> io::Result<File> {
        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        opts.open(&self.path)
    }

    fn write(&self, line: &[u8]) -> io::Result<()> {
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(self.open()?);
        }
        let len = guard
            .as_ref()
            .map_or(0, |f| f.metadata().map_or(0, |m| m.len()));
        if len + line.len() as u64 > MAX_BYTES {
            *guard = None;
            std::fs::rename(&self.path, &self.rotated)?;
            *guard = Some(self.open()?);
        }
        let f = guard.as_mut().expect("file opened above");
        f.write_all(line)?;
        f.flush()
    }

    /// Последние `limit` записей, новые первыми.
    pub fn tail(&self, limit: usize) -> Vec<Value> {
        let mut lines: Vec<Value> = Vec::new();
        for path in [&self.rotated, &self.path] {
            let Ok(f) = File::open(path) else {
                continue;
            };
            // По байтам, не `lines()`: строка с битым UTF-8 (оборванная запись) пропускается,
            // а не обрывает чтение остального файла.
            lines.extend(
                BufReader::new(f)
                    .split(b'\n')
                    .map_while(Result::ok)
                    .filter_map(|l| serde_json::from_slice(&l).ok()),
            );
        }
        let start = lines.len().saturating_sub(limit);
        lines.split_off(start).into_iter().rev().collect()
    }
}
