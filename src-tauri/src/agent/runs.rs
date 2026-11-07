//! Запуски фонового ассистента (спец. P4c §3.2, §8.3, §9.3): записи `agent-runs.json` (последние `MAX_RUNS`; `ListFile`
//! P4b, 0600), журналы `agent-runs/<runId>.log` (0600, до `MAX_LOG_BYTES`, хранятся журналы последних `MAX_LOGS`
//! запусков), MCP-конфиг запуска `agent-runs/<runId>.mcp.json` с билетом (0600 в каталоге 0700, удаляется в конце запуска).

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::settings::Preset;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::mcp::llm::list_file::ListFile;
use crate::product::MCP_SERVER_NAME;
use crate::storage::fs::write_private;
use crate::topology::model::now_ms;

pub const RUNS_FILE: &str = "agent-runs.json";
pub const RUNS_DIR: &str = "agent-runs";
pub const WORK_DIR: &str = "agent-work";
pub const MAX_RUNS: usize = 50;
pub const MAX_LOGS: usize = 20;
pub const MAX_LOG_BYTES: usize = 1 << 20;
pub const MAX_SUMMARY_BYTES: usize = 4096;
/// Хвост журнала для окна «Показать журнал».
pub const LOG_TAIL_BYTES: usize = 256 * 1024;
/// Переменная окружения шима с билетом запуска (задача 2, `ShimEnv::agent_ticket`).
pub const TICKET_ENV: &str = "GLAFIRA_AGENT_TICKET";

/// Запуск по запросу на оптимизацию или «Проверить» в «Настройках» (без подключения и запроса).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunKind {
    #[default]
    Optimize,
    Check,
}

/// Жизненный цикл запуска (спец. §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    Queued,
    Starting,
    Running,
    Done,
    Failed,
    Cancelled,
    Interrupted,
    Skipped,
}

impl RunStatus {
    /// В очереди или идёт: при выходе из Glafira такие запуски становятся `interrupted`.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            RunStatus::Queued | RunStatus::Starting | RunStatus::Running
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Queued => "queued",
            RunStatus::Starting => "starting",
            RunStatus::Running => "running",
            RunStatus::Done => "done",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "cancelled",
            RunStatus::Interrupted => "interrupted",
            RunStatus::Skipped => "skipped",
        }
    }
}

/// Причина `failed` (спец. §3.2); `notConfigured` — агент не настроен к моменту старта (уточнение плана);
/// `unsafeTools` — модели доступно что-то кроме инструментов Glafira (событие `init` потока, §4.1 «Проверка
/// инструментов»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailReason {
    CliNotFound,
    SpawnFailed,
    Timeout,
    Limit,
    Exit,
    NotConnected,
    McpOff,
    AccessOff,
    Policy,
    NotConfigured,
    UnsafeTools,
}

impl FailReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FailReason::CliNotFound => "cliNotFound",
            FailReason::SpawnFailed => "spawnFailed",
            FailReason::Timeout => "timeout",
            FailReason::Limit => "limit",
            FailReason::Exit => "exit",
            FailReason::NotConnected => "notConnected",
            FailReason::McpOff => "mcpOff",
            FailReason::AccessOff => "accessOff",
            FailReason::Policy => "policy",
            FailReason::NotConfigured => "notConfigured",
            FailReason::UnsafeTools => "unsafeTools",
        }
    }
}

/// Запись запуска (спец. §8.3). Промпт и вывод агента здесь не хранятся: вывод — в журнале запуска, итог — `summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRun {
    /// `a` + 6 hex.
    pub id: String,
    #[serde(default)]
    pub kind: RunKind,
    pub request_id: Option<String>,
    pub connection_id: Option<String>,
    pub preset: Preset,
    /// Модель запуска (у Qwen Code с пустым полем — `model.name` его настроек); `None` — модель CLI по умолчанию.
    pub model: Option<String>,
    /// Путь программы запуска (спец. §10): «программа не найдена: <путь>».
    #[serde(default)]
    pub path: Option<String>,
    pub cli_version: Option<String>,
    pub status: RunStatus,
    pub reason: Option<FailReason>,
    pub exit_code: Option<i32>,
    pub queued_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    #[serde(default)]
    pub calls: u32,
    pub last_tool: Option<String>,
    pub last_call_at_ms: Option<i64>,
    #[serde(default)]
    pub proposal_ids: Vec<String>,
    /// Итоговый текст агента (до `MAX_SUMMARY_BYTES`), простой текст.
    pub summary: Option<String>,
    /// Стоимость запуска — только у Claude Code без Ollama (через Ollama она выдуманная, §4.1).
    pub cost_usd: Option<f64>,
    /// Оценка стоимости облачного запуска через Ollama по ценам Ollama Cloud (`agent::pricing`, смоук P4c, sf-T6): у
    /// Qwen Code и у Claude Code через Ollama с облачной моделью. У записей до sf-T6 поля нет.
    #[serde(default)]
    pub cost_estimate: Option<CostEstimate>,
    /// Предупреждения запуска; строку переводит интерфейс (задача 9), прочие он показывает как есть:
    /// - `evict <другие модели через запятую> <модель запуска>` — Ollama может выгрузить другую модель (§6.3);
    /// - `unsafe <имена через запятую>` — агенту доступны не только инструменты Glafira, запуск остановлен до первого
    ///   хода; `unsafe no-init` — агент не сообщил список своих инструментов (§4.1 «Проверка инструментов»).
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Оценка стоимости запуска по таблице цен Ollama Cloud (встроенной или загруженной «Обновить цены»).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostEstimate {
    /// Сумма, $.
    pub usd: f64,
    /// День цен таблицы, по которой считали: `ГГГГ-ММ-ДД` (подсказка «Оценка по ценам Ollama Cloud на …»).
    pub prices_date: String,
    /// Взяты цены «вне пика» (будни вне 12:00–18:00 UTC и выходные по времени начала запуска).
    pub off_peak: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewRun {
    pub kind: RunKind,
    pub request_id: Option<String>,
    pub connection_id: Option<String>,
    pub preset: Preset,
    pub model: Option<String>,
}

/// Чьи запуски показать: подключение (консоль, карточка) и/или запросы; без условий — все (подраздел «Запуски»).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFilter {
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub request_ids: Option<Vec<String>>,
}

impl RunFilter {
    pub fn matches(&self, r: &AgentRun) -> bool {
        let by_connection = self
            .connection_id
            .as_ref()
            .is_none_or(|c| r.connection_id.as_ref() == Some(c));
        let by_request = self
            .request_ids
            .as_ref()
            .is_none_or(|ids| r.request_id.as_ref().is_some_and(|id| ids.contains(id)));
        by_connection && by_request
    }
}

pub fn run_not_found(id: &str) -> AppError {
    app_err!(
        ErrorCode::NotFound,
        "errors.agent.runNotFound",
        "Background run {{id}} not found",
        id = id
    )
}

/// Не больше `MAX_RUNS`: уходят самые старые законченные; идущие и ожидающие не вытесняются.
fn evict(items: &mut Vec<AgentRun>) {
    while items.len() > MAX_RUNS {
        let i = items
            .iter()
            .position(|r| !r.status.is_active())
            .unwrap_or(0);
        items.remove(i);
    }
}

pub struct RunStore(ListFile<AgentRun>);

impl RunStore {
    pub fn open(dir: &Path) -> Self {
        Self(ListFile::open(dir, RUNS_FILE, "runs"))
    }

    /// Блокирующий вызов (запись файла).
    pub fn create(&self, n: NewRun) -> AppResult<AgentRun> {
        self.0.update(|items| {
            let id = crate::mcp::llm::new_id('a', |id| items.iter().any(|r| r.id == id));
            let run = AgentRun {
                id,
                kind: n.kind,
                request_id: n.request_id,
                connection_id: n.connection_id,
                preset: n.preset,
                model: n.model,
                path: None,
                cli_version: None,
                status: RunStatus::Queued,
                reason: None,
                exit_code: None,
                queued_at_ms: now_ms(),
                started_at_ms: None,
                ended_at_ms: None,
                calls: 0,
                last_tool: None,
                last_call_at_ms: None,
                proposal_ids: Vec::new(),
                summary: None,
                cost_usd: None,
                cost_estimate: None,
                warnings: Vec::new(),
            };
            items.push(run.clone());
            evict(items);
            Ok(run)
        })
    }

    /// Блокирующий вызов (запись файла).
    pub fn update(&self, id: &str, f: impl FnOnce(&mut AgentRun)) -> AppResult<AgentRun> {
        self.0.update(|items| {
            let r = items
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or_else(|| run_not_found(id))?;
            f(r);
            Ok(r.clone())
        })
    }

    pub fn get(&self, id: &str) -> Option<AgentRun> {
        self.0
            .read(|items| items.iter().find(|r| r.id == id).cloned())
    }

    /// Сначала новые.
    pub fn list(&self, f: &RunFilter) -> Vec<AgentRun> {
        self.0.read(|items| {
            items
                .iter()
                .rev()
                .filter(|r| f.matches(r))
                .cloned()
                .collect()
        })
    }

    /// Id последних `n` запусков, сначала новые: их журналы хранятся.
    pub fn recent_ids(&self, n: usize) -> Vec<String> {
        self.0
            .read(|items| items.iter().rev().take(n).map(|r| r.id.clone()).collect())
    }

    /// При открытии: запуски, которые были в очереди или шли, когда Glafira закрылась, — `interrupted` (§3.2). Очередь не
    /// восстанавливается. Блокирующий вызов.
    pub fn mark_interrupted(&self) -> AppResult<usize> {
        if !self
            .0
            .read(|items| items.iter().any(|r| r.status.is_active()))
        {
            return Ok(0);
        }
        let now = now_ms();
        self.0.update(|items| {
            let mut n = 0;
            for r in items.iter_mut().filter(|r| r.status.is_active()) {
                r.status = RunStatus::Interrupted;
                r.ended_at_ms.get_or_insert(now);
                n += 1;
            }
            Ok(n)
        })
    }
}

/// Каталог только для владельца (0700 на Unix).
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn log_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.log"))
}

pub fn mcp_config_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.mcp.json"))
}

/// MCP-конфиг запуска (спец. §3.3): сервер `glafira-pg` — шим с билетом в `env`; файлом, а не строкой в аргументах:
/// аргументы процессов видны другим пользователям в `ps`. У Qwen Code — `"trust": true`: вызовы Glafira без
/// подтверждения. Билет — ровно 64 hex (`handshake::valid_ticket`): пустой шим счёл бы незаданным и подключился бы
/// обычной сессией, поэтому такой конфиг не пишется (`InvalidInput`). Блокирующий вызов.
pub fn write_mcp_config(
    runs_dir: &Path,
    run_id: &str,
    exe: &Path,
    ticket: &str,
    preset: Preset,
) -> io::Result<PathBuf> {
    if !glafira_mcp_core::handshake::valid_ticket(ticket) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the ticket of a background run must be 64 hex digits",
        ));
    }
    ensure_private_dir(runs_dir)?;
    let mut server = json!({
        "command": exe.to_string_lossy(),
        "args": ["mcp"],
        "env": { TICKET_ENV: ticket },
    });
    if preset == Preset::Qwen {
        server["trust"] = json!(true);
    }
    let body = json!({ "mcpServers": { MCP_SERVER_NAME: server } });
    let path = mcp_config_path(runs_dir, run_id);
    write_private(
        &path,
        &serde_json::to_vec_pretty(&body).map_err(io::Error::other)?,
    )?;
    Ok(path)
}

/// Конец запуска: конфиг с билетом удаляется.
pub fn remove_mcp_config(runs_dir: &Path, run_id: &str) {
    if let Err(e) = fs::remove_file(mcp_config_path(runs_dir, run_id))
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(run = %run_id, "mcp config of the run not removed: {e}");
    }
}

/// Конфиги, оставшиеся после аварийного завершения Glafira: при открытии живых запусков нет.
pub fn remove_stale_configs(runs_dir: &Path) {
    let Ok(entries) = fs::read_dir(runs_dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".mcp.json"))
            && let Err(err) = fs::remove_file(&p)
        {
            tracing::warn!("{} not removed: {err}", p.display());
        }
    }
}

/// Журналы хранятся только у `keep` (последние запуски); остальные `*.log` каталога удаляются.
pub fn prune_logs(runs_dir: &Path, keep: &[String]) {
    let Ok(entries) = fs::read_dir(runs_dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Some(id) = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".log"))
        else {
            continue;
        };
        if !keep.iter().any(|k| k == id)
            && let Err(err) = fs::remove_file(&p)
        {
            tracing::warn!("{} not removed: {err}", p.display());
        }
    }
}

/// Хвост журнала до `max` байт: обрезанное начало — с новой строки и с пометкой. Блокирующий вызов.
pub fn read_log_tail(path: &Path, max: usize) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max as u64);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if start == 0 {
        return Ok(text);
    }
    Ok(match text.split_once('\n') {
        Some((_, rest)) => format!("…[truncated]\n{rest}"),
        None => format!("…[truncated]\n{text}"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn new_run(request: &str) -> NewRun {
        NewRun {
            kind: RunKind::Optimize,
            request_id: Some(request.into()),
            connection_id: Some("c1".into()),
            preset: Preset::Qwen,
            model: Some("qwen3.6-64k:35b-a3b".into()),
        }
    }

    #[test]
    fn runs_get_short_ids_survive_a_restart_and_only_fifty_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let s = RunStore::open(dir.path());
        let first = s.create(new_run("r1")).unwrap();
        assert!(
            first.id.len() == 7
                && first.id.starts_with('a')
                && first.id[1..].chars().all(|c| c.is_ascii_hexdigit()),
            "{}",
            first.id
        );
        assert_eq!((first.status, first.calls), (RunStatus::Queued, 0));
        assert_eq!(
            RunStore::open(dir.path()).get(&first.id),
            Some(first.clone())
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join(RUNS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let mut done = Vec::new();
        for _ in 0..MAX_RUNS {
            let r = s.create(new_run("r2")).unwrap();
            done.push(s.update(&r.id, |r| r.status = RunStatus::Done).unwrap().id);
        }
        assert_eq!(s.list(&RunFilter::default()).len(), MAX_RUNS);
        assert!(
            s.get(&first.id).is_some(),
            "ожидающий запуск не вытесняется"
        );
        assert_eq!(s.get(&done[0]), None, "вытеснен самый старый законченный");
        assert_eq!(
            s.update("a000000", |_| {}).unwrap_err().key,
            "errors.agent.runNotFound"
        );
    }

    #[test]
    fn filter_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let s = RunStore::open(dir.path());
        let a = s.create(new_run("r1")).unwrap();
        let b = s.create(new_run("r2")).unwrap();
        let check = s
            .create(NewRun {
                kind: RunKind::Check,
                request_id: None,
                connection_id: None,
                preset: Preset::Claude,
                model: None,
            })
            .unwrap();
        let ids = |f: &RunFilter| s.list(f).into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(
            ids(&RunFilter::default()),
            [check.id.clone(), b.id.clone(), a.id.clone()]
        );
        assert_eq!(
            ids(&RunFilter {
                connection_id: Some("c1".into()),
                ..RunFilter::default()
            }),
            [b.id.clone(), a.id.clone()]
        );
        assert_eq!(
            ids(&RunFilter {
                request_ids: Some(vec!["r1".into()]),
                ..RunFilter::default()
            }),
            [a.id.as_str()]
        );
        assert_eq!(s.recent_ids(2), [check.id, b.id]);
        let f: RunFilter =
            serde_json::from_value(json!({ "connectionId": "c1", "requestIds": ["r2"] })).unwrap();
        assert_eq!(f.request_ids.as_deref(), Some(&["r2".to_string()][..]));
        let empty: RunFilter = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, RunFilter::default(), "пустой фильтр — все запуски");
    }

    #[test]
    fn active_runs_become_interrupted_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let s = RunStore::open(dir.path());
        let queued = s.create(new_run("r1")).unwrap();
        let running = s.create(new_run("r2")).unwrap();
        s.update(&running.id, |r| {
            r.status = RunStatus::Running;
            r.started_at_ms = Some(1);
        })
        .unwrap();
        let done = s.create(new_run("r3")).unwrap();
        s.update(&done.id, |r| r.status = RunStatus::Done).unwrap();
        let reopened = RunStore::open(dir.path());
        assert_eq!(reopened.mark_interrupted().unwrap(), 2);
        for id in [&queued.id, &running.id] {
            let r = reopened.get(id).unwrap();
            assert_eq!(r.status, RunStatus::Interrupted, "{id}");
            assert!(r.ended_at_ms.is_some());
        }
        assert_eq!(reopened.get(&done.id).unwrap().status, RunStatus::Done);
        assert_eq!(
            reopened.mark_interrupted().unwrap(),
            0,
            "второй раз — нечего прерывать"
        );
    }

    #[test]
    fn json_shape_of_a_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = RunStore::open(dir.path())
            .create(new_run("r3a9f1c"))
            .unwrap();
        r.status = RunStatus::Failed;
        r.reason = Some(FailReason::CliNotFound);
        r.path = Some("/opt/homebrew/bin/qwen".into());
        r.warnings = vec!["evict qwen3.8:27b qwen3.6-64k:35b-a3b".into()];
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(
            (&v["status"], &v["reason"], &v["kind"], &v["preset"]),
            (
                &json!("failed"),
                &json!("cliNotFound"),
                &json!("optimize"),
                &json!("qwen")
            )
        );
        for key in [
            "requestId",
            "connectionId",
            "model",
            "path",
            "cliVersion",
            "exitCode",
            "queuedAtMs",
            "startedAtMs",
            "endedAtMs",
            "calls",
            "lastTool",
            "lastCallAtMs",
            "proposalIds",
            "summary",
            "costUsd",
            "costEstimate",
            "warnings",
        ] {
            assert!(v.get(key).is_some(), "{key}: {v}");
        }
        assert_eq!(
            v["warnings"],
            json!(["evict qwen3.8:27b qwen3.6-64k:35b-a3b"])
        );
        // Запись до sf-T6 (без `costEstimate`) читается: оценки нет.
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("costEstimate");
        assert_eq!(
            serde_json::from_value::<AgentRun>(old)
                .unwrap()
                .cost_estimate,
            None
        );
        assert_eq!(serde_json::from_value::<AgentRun>(v).unwrap(), r);
        r.cost_estimate = Some(CostEstimate {
            usd: 0.0148,
            prices_date: "2026-11-07".into(),
            off_peak: true,
        });
        assert_eq!(
            serde_json::to_value(&r).unwrap()["costEstimate"],
            json!({"usd": 0.0148, "pricesDate": "2026-11-07", "offPeak": true})
        );
        for (s, name) in [
            (RunStatus::Queued, "queued"),
            (RunStatus::Starting, "starting"),
            (RunStatus::Running, "running"),
            (RunStatus::Done, "done"),
            (RunStatus::Failed, "failed"),
            (RunStatus::Cancelled, "cancelled"),
            (RunStatus::Interrupted, "interrupted"),
            (RunStatus::Skipped, "skipped"),
        ] {
            assert_eq!(serde_json::to_value(s).unwrap(), name);
            assert_eq!(s.as_str(), name);
        }
        for (f, name) in [
            (FailReason::CliNotFound, "cliNotFound"),
            (FailReason::SpawnFailed, "spawnFailed"),
            (FailReason::Timeout, "timeout"),
            (FailReason::Limit, "limit"),
            (FailReason::Exit, "exit"),
            (FailReason::NotConnected, "notConnected"),
            (FailReason::McpOff, "mcpOff"),
            (FailReason::AccessOff, "accessOff"),
            (FailReason::Policy, "policy"),
            (FailReason::NotConfigured, "notConfigured"),
            (FailReason::UnsafeTools, "unsafeTools"),
        ] {
            assert_eq!(serde_json::to_value(f).unwrap(), name);
            assert_eq!(f.as_str(), name);
        }
        assert_eq!(serde_json::to_value(RunKind::Check).unwrap(), "check");
    }

    #[test]
    fn the_mcp_config_is_private_and_carries_the_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join(RUNS_DIR);
        let ticket = "ab".repeat(32);
        let exe = Path::new("/Applications/Glafira for PostgreSQL.app/Contents/MacOS/glafira-pg");
        let p = write_mcp_config(&runs, "a1b2c3d", exe, &ticket, Preset::Qwen).unwrap();
        assert_eq!(p, mcp_config_path(&runs, "a1b2c3d"));
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&p).unwrap()).unwrap();
        let server = &v["mcpServers"]["glafira-pg"];
        assert_eq!(server["command"], exe.to_str().unwrap());
        assert_eq!(server["args"], json!(["mcp"]));
        assert_eq!(server["env"], json!({ "GLAFIRA_AGENT_TICKET": ticket }));
        assert_eq!(
            server["trust"], true,
            "вызовы Glafira у Qwen Code не ждут подтверждения"
        );
        let c = write_mcp_config(&runs, "a0000c1", exe, &ticket, Preset::Claude).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&fs::read(&c).unwrap()).unwrap();
        assert!(v["mcpServers"]["glafira-pg"].get("trust").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&runs).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        remove_mcp_config(&runs, "a1b2c3d");
        assert!(!p.exists());
        remove_mcp_config(&runs, "a1b2c3d");
        remove_stale_configs(&runs);
        assert!(
            !c.exists(),
            "конфиги прошлого сеанса удаляются при открытии"
        );
    }

    #[test]
    fn the_mcp_config_never_carries_a_ticket_the_shim_would_ignore() {
        // Пустой или неверный билет шим считает незаданным или отвергает: агент без билета стал бы обычной сессией
        // со всеми инструментами, если клиента раньше подтвердили в чате. Такой конфиг не пишется.
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join(RUNS_DIR);
        let exe = Path::new("/opt/glafira/glafira-pg");
        for bad in [
            String::new(),
            " ".repeat(64),
            "ab".repeat(31),
            "zz".repeat(32),
            format!(" {}", "ab".repeat(32)),
        ] {
            let e = write_mcp_config(&runs, "a1b2c3d", exe, &bad, Preset::Qwen).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
            assert!(!mcp_config_path(&runs, "a1b2c3d").exists(), "{bad:?}");
        }
    }

    #[test]
    fn only_the_logs_of_recent_runs_are_kept_and_the_tail_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join(RUNS_DIR);
        ensure_private_dir(&runs).unwrap();
        for id in ["a000001", "a000002", "a000003"] {
            fs::write(log_path(&runs, id), format!("log of {id}\n")).unwrap();
        }
        fs::write(runs.join("a000009.mcp.json"), "{}").unwrap();
        prune_logs(&runs, &["a000002".into(), "a000003".into()]);
        assert!(!log_path(&runs, "a000001").exists());
        assert!(log_path(&runs, "a000002").exists() && log_path(&runs, "a000003").exists());
        assert!(
            runs.join("a000009.mcp.json").exists(),
            "конфиги — не журналы"
        );
        let big = log_path(&runs, "a000004");
        fs::write(
            &big,
            format!("{}\nsecond line\nlast line\n", "x".repeat(300_000)),
        )
        .unwrap();
        let tail = read_log_tail(&big, LOG_TAIL_BYTES).unwrap();
        assert_eq!(tail, "…[truncated]\nsecond line\nlast line\n", "{tail:?}");
        assert_eq!(
            read_log_tail(&log_path(&runs, "a000002"), LOG_TAIL_BYTES).unwrap(),
            "log of a000002\n"
        );
    }
}
