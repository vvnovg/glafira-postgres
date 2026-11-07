//! Фальшивый агентский CLI для интеграционного теста фонового ассистента (`tests/it_agent.rs`, спец. P4c §12). Ведёт себя
//! как Qwen Code или Claude Code без окна: принимает любые флаги пресета, читает промпт из stdin до конца, находит MCP-конфиг
//! запуска по `--mcp-config <файл>`, запускает из него сервер `glafira-pg` (настоящий шим) с `env` конфига поверх своего
//! окружения, говорит с ним по MCP клиентом rmcp и печатает в stdout то, что печатает CLI пресета:
//! - `-o stream-json` / `--output-format stream-json` (оба пресета) — поток JSON по строке: первым событием
//!   `{"type":"system","subtype":"init","tools":[…]}` с инструментами от шлюза (`tools/list`, имена
//!   `mcp__glafira-pg__<имя>`), затем событие `assistant` на каждый вызов, последним —
//!   `{"type":"result","subtype":"success","result":…,"total_cost_usd":…}` (итог проверки CLI, задача 1);
//! - `--output-format json` / `-o json` — один объект `type: "result"`, как у Claude Code;
//! - иначе — итог простым текстом.
//!
//! Glafira считает поток сломанным, если до `init` пришла строка не JSON: заметки и отладка агента идут только в stderr
//! и в файл `FAKE_AGENT_NOTES`, не в stdout. Поведение — `FAKE_AGENT_MODE`:
//! - `propose` — `get_optimization_request` без id, `propose_query_rewrite` с id запроса (индексы — из
//!   `FAKE_AGENT_INDEXES` через `;`);
//! - `none` — `get_optimization_request`, итог «без предложений»;
//! - `hang` — один вызов и ожидание без конца; на Unix SIGTERM агент перехватывает (зависший CLI: нужен SIGKILL);
//! - `crash` — один вызов и выход с кодом 3 без события `result`;
//! - `flood` — один вызов, 1,5 МБ в stdout (предел журнала запуска — 1 МБ) и итог длиннее 4 КБ;
//! - `limit` — 41 вызов `list_connections` (предел запуска — 40);
//! - `foreign` — инструменты и prompts сессии, подключения, вызовы чужого подключения (`FAKE_AGENT_FOREIGN_CONNECTION`) и
//!   чужого запроса (`FAKE_AGENT_FOREIGN_REQUEST`), `run_select`, прямая форма предложения;
//! - `check` — `list_connections` и ответ `OK` (запуск «Проверить»);
//! - `unsafe` — в `init` есть ещё встроенный инструмент CLI (`run_shell_command`); первый «ход» (вызов) — только через
//!   5 с: Glafira должна остановить агента раньше;
//! - `twice` — вызов в первой сессии, затем вторая сессия с тем же билетом (второй шим): Glafira закрывает первую, её шим
//!   заканчивается сам; вызов через первую не доходит, через вторую — проходит.
//!
//! Для теста агент пишет в stderr (оно попадает в журнал запуска) строки `FAKE_AGENT ключ=значение` и их копию — в файл
//! `FAKE_AGENT_NOTES`; билет — только длиной. Без `GLAFIRA_TEST_DATA_DIR` агент шим не запускает: иначе шим искал бы
//! Glafira владельца в обычном каталоге данных. `<путь> --version` печатает версию (поиск программы в «Настройках»).
//! Только для тестов: бинарь собирается с feature `fake-agent` и в установщики не попадает.

use std::io::Write;
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientCapabilities, ClientConfig, Implementation,
};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// Имя сервера Glafira в MCP-конфиге запуска.
const SERVER: &str = "glafira-pg";
const NAME: &str = "glafira-fake-agent";
const VERSION: &str = "9.9.9";
/// Каталог данных шима в тесте (`mcp::shim::TEST_DATA_DIR_ENV`, feature `fake-agent`).
const TEST_DATA_DIR_ENV: &str = "GLAFIRA_TEST_DATA_DIR";
/// Итоги и стоимость — те же строки сверяет `tests/it_agent.rs`.
const PROPOSED: &str = "Fake agent summary: one rewrite proposed.";
const NO_PROPOSALS: &str = "Fake agent summary: no proposals, the plan already uses an index.";
const COST: f64 = 0.0042;
/// Встроенный инструмент Qwen Code, который режим `unsafe` объявляет в `init`.
const FOREIGN_TOOL: &str = "run_shell_command";
/// Режим `unsafe`: столько модель «думает» до первого вызова; Glafira останавливает агента сразу после `init`.
const UNSAFE_FIRST_TURN: Duration = Duration::from_secs(5);
/// Вызов без ответа дольше — ошибка (`error=timeout`), а не зависший тест.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const SESSION_ID: &str = "fake-agent-session";

type Client = RunningService<RoleClient, ClientConfig>;

/// Вывод CLI по флагам пресета.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Output {
    StreamJson,
    Json,
    Text,
}

/// Строка для теста — одной записью в stderr: `eprintln!` пишет кусками, и метки потока и времени журнала запуска могли бы
/// разрезать строку. Копия — в файл `FAKE_AGENT_NOTES`, если он задан: тесту pid агента и шима нужны во время запуска.
fn note(text: impl AsRef<str>) {
    let line = format!("FAKE_AGENT {}\n", text.as_ref());
    let _ = std::io::stderr().write_all(line.as_bytes());
    if let Some(path) = std::env::var_os("FAKE_AGENT_NOTES")
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Событие потока JSON — строкой в stdout, сразу (Glafira читает `init` до первого хода).
fn emit(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("{VERSION} ({NAME})");
        return ExitCode::SUCCESS;
    }
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            note(format!("error=runtime {e}"));
            return ExitCode::from(2);
        }
    };
    let code = match rt.block_on(run(&args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            note(format!("error={}", one_line(&e)));
            ExitCode::from(2)
        }
    };
    rt.shutdown_background();
    code
}

async fn run(args: &[String]) -> Result<(), String> {
    let mode = std::env::var("FAKE_AGENT_MODE").unwrap_or_else(|_| "none".into());
    let output = output_format(args);
    let mut prompt = String::new();
    tokio::io::stdin()
        .read_to_string(&mut prompt)
        .await
        .map_err(|e| format!("stdin: {e}"))?;
    note(format!(
        "pid={} mode={mode} prompt_bytes={} prompt_request={}",
        std::process::id(),
        prompt.len(),
        request_id_in(&prompt).as_deref().unwrap_or("-")
    ));
    if let Ok(dir) = std::env::current_dir() {
        note(format!("cwd={}", dir.display()));
    }
    if std::env::var_os(TEST_DATA_DIR_ENV).is_none_or(|d| d.is_empty()) {
        return Err(format!(
            "{TEST_DATA_DIR_ENV} is not set: the fake agent runs only in tests"
        ));
    }
    let server = server_config(args)?;
    let session = Session::open(&server, "shim_pid").await?;
    let mut agent = Agent {
        s: session,
        calls: 0,
        output,
    };
    let mut tools = agent.tools().await;
    if mode == "unsafe" {
        tools.push(FOREIGN_TOOL.to_string());
    }
    if output == Output::StreamJson {
        emit(&json!({
            "type": "system",
            "subtype": "init",
            "session_id": SESSION_ID,
            "model": flag_value(args, &["--model", "-m"]),
            "mcp_servers": [{ "name": SERVER, "status": "connected" }],
            "tools": tools,
        }));
    }
    let summary = match mode.as_str() {
        "propose" => agent.propose().await?,
        "none" => {
            let _ = agent.call("get_optimization_request", json!({})).await;
            NO_PROPOSALS.to_string()
        }
        "hang" => {
            agent.hang().await;
            unreachable!("hang does not return")
        }
        "crash" => {
            let _ = agent.call("get_optimization_request", json!({})).await;
            std::process::exit(3)
        }
        "flood" => {
            let _ = agent.call("get_optimization_request", json!({})).await;
            flood(output);
            format!(
                "Fake agent summary: flooded the output. {}",
                "y".repeat(10 * 1024)
            )
        }
        "limit" => agent.limit().await,
        "foreign" => agent.foreign().await,
        "check" => agent.check().await,
        "unsafe" => {
            tokio::time::sleep(UNSAFE_FIRST_TURN).await;
            let _ = agent.call("list_connections", json!({})).await;
            "Fake agent summary: a tool beyond Glafira was available.".to_string()
        }
        "twice" => agent.twice(&server).await?,
        other => return Err(format!("unknown FAKE_AGENT_MODE {other}")),
    };
    let calls = agent.calls;
    agent.s.close().await;
    print_result(output, &summary, calls);
    Ok(())
}

/// Значение флага: `--flag value` или `--flag=value` (первое из `names`).
fn flag_value(args: &[String], names: &[&str]) -> Option<String> {
    args.iter().enumerate().find_map(|(i, a)| {
        names.iter().find_map(|n| {
            if a == n {
                args.get(i + 1).cloned()
            } else {
                a.strip_prefix(n)
                    .and_then(|r| r.strip_prefix('='))
                    .map(str::to_string)
            }
        })
    })
}

fn output_format(args: &[String]) -> Output {
    match flag_value(args, &["--output-format", "-o"]).as_deref() {
        Some("stream-json") => Output::StreamJson,
        Some("json") => Output::Json,
        _ => Output::Text,
    }
}

/// Сервер `glafira-pg` из MCP-конфига запуска.
struct Server {
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

fn server_config(args: &[String]) -> Result<Server, String> {
    let path =
        flag_value(args, &["--mcp-config"]).ok_or("no --mcp-config <file> in the arguments")?;
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
    let s = &v["mcpServers"][SERVER];
    let command = s["command"]
        .as_str()
        .ok_or("no mcpServers.glafira-pg.command")?
        .to_string();
    let server_args = s["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env: Vec<(String, String)> = s["env"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, x)| x.as_str().map(|x| (k.clone(), x.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    let ticket = env
        .iter()
        .find(|(k, _)| k == "GLAFIRA_AGENT_TICKET")
        .map_or(0, |(_, x)| x.len());
    note(format!(
        "config_env={} ticket_len={ticket}",
        names.join(",")
    ));
    Ok(Server {
        command,
        args: server_args,
        env,
    })
}

/// Сессия MCP: процесс шима и клиент rmcp поверх его stdio.
struct Session {
    shim: Child,
    client: Client,
}

impl Session {
    /// Запускает сервер из конфига (`env` — поверх своего окружения, как у настоящих CLI; stderr шима — в stderr агента,
    /// то есть в журнал запуска) и проходит `initialize`. `pid_key` — ключ строки с pid шима.
    async fn open(s: &Server, pid_key: &str) -> Result<Self, String> {
        let mut cmd = Command::new(&s.command);
        cmd.args(&s.args)
            .envs(s.env.iter().map(|(k, x)| (k.as_str(), x.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            // Как у настоящих CLI на Windows: консольный шим без своего окна (CREATE_NO_WINDOW).
            cmd.creation_flags(0x0800_0000);
        }
        let mut shim = cmd.spawn().map_err(|e| format!("{}: {e}", s.command))?;
        note(format!("{pid_key}={}", shim.id().unwrap_or_default()));
        let stdin = shim.stdin.take().ok_or("shim stdin")?;
        let stdout = shim.stdout.take().ok_or("shim stdout")?;
        let client = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new(NAME, VERSION),
        )
        .serve((stdout, stdin))
        .await
        .map_err(|e| format!("initialize: {}", one_line(&e.to_string())))?;
        note("connected=1");
        Ok(Self { shim, client })
    }

    /// Клиент закрывает stdin шима — шим заканчивается сам; как настоящий CLI, агент ждёт его недолго.
    async fn close(mut self) {
        let _ = self.client.cancel().await;
        if tokio::time::timeout(Duration::from_secs(5), self.shim.wait())
            .await
            .is_err()
        {
            let _ = self.shim.kill().await;
        }
    }
}

/// Вызов инструмента; итог (`ok` или код ошибки `<code>: <message>`) — строкой в stderr. Успешный ответ — JSON одним
/// текстовым блоком без `structuredContent` (сверка с `feat/p4b` 4068590).
async fn call_on(client: &Client, n: u32, tool: &str, args: Value) -> Result<Value, String> {
    let mut p = CallToolRequestParams::new(tool.to_string());
    if let Value::Object(m) = args {
        p = p.with_arguments(m);
    }
    match tokio::time::timeout(CALL_TIMEOUT, client.call_tool(p)).await {
        Ok(Ok(r)) if r.is_error == Some(true) => {
            let text = text_of(&r);
            let code = text
                .split(':')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            note(format!("call={n} tool={tool} error={code}"));
            Err(text)
        }
        Ok(Ok(r)) => {
            note(format!("call={n} tool={tool} ok"));
            serde_json::from_str(&text_of(&r))
                .map_err(|e| format!("{tool}: the answer is not JSON: {e}"))
        }
        Ok(Err(e)) => {
            note(format!("call={n} tool={tool} error=protocol"));
            Err(one_line(&e.to_string()))
        }
        Err(_) => {
            note(format!("call={n} tool={tool} error=timeout"));
            Err(format!("{tool}: no answer within {CALL_TIMEOUT:?}"))
        }
    }
}

struct Agent {
    s: Session,
    calls: u32,
    output: Output,
}

impl Agent {
    /// Вызов в текущей сессии; в потоке JSON — событие `assistant` с вызовом, как у CLI.
    async fn call(&mut self, tool: &str, args: Value) -> Result<Value, String> {
        self.calls += 1;
        let n = self.calls;
        if self.output == Output::StreamJson {
            emit(&json!({
                "type": "assistant",
                "session_id": SESSION_ID,
                "message": { "content": [{
                    "type": "tool_use",
                    "id": format!("toolu_{n}"),
                    "name": format!("mcp__{SERVER}__{tool}"),
                    "input": args,
                }]},
            }));
        }
        call_on(&self.s.client, n, tool, args).await
    }

    /// Инструменты сессии (`tools/list`) — для строки теста и события `init` (с префиксом сервера, как у CLI).
    async fn tools(&self) -> Vec<String> {
        match self.s.client.list_all_tools().await {
            Ok(tools) => {
                let mut names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
                names.sort_unstable();
                note(format!("tools={} names={}", names.len(), names.join(",")));
                names
                    .iter()
                    .map(|n| format!("mcp__{SERVER}__{n}"))
                    .collect()
            }
            Err(e) => {
                note(format!("tools=error {}", one_line(&e.to_string())));
                Vec::new()
            }
        }
    }

    async fn propose(&mut self) -> Result<String, String> {
        // Без requestId сессия запуска получает запрос своего запуска (спец. §5.2).
        let req = self.call("get_optimization_request", json!({})).await?;
        let id = req["id"]
            .as_str()
            .ok_or("get_optimization_request: no id")?
            .to_string();
        note(format!("request={id}"));
        let sql = req["sql"]
            .as_str()
            .unwrap_or("SELECT 1")
            .trim()
            .trim_end_matches(';')
            .to_string();
        let out = self
            .call(
                "propose_query_rewrite",
                json!({
                    "requestId": id,
                    "rewrittenSql": format!("SELECT * FROM ({sql}) AS fake_agent_rewrite"),
                    "indexes": indexes(),
                    "rationale": "Fake agent: the same query wrapped in a subquery.",
                    "expectedEffect": "None: this is a test proposal.",
                }),
            )
            .await?;
        note(format!(
            "proposal={}",
            out["proposalId"].as_str().unwrap_or("-")
        ));
        Ok(PROPOSED.to_string())
    }

    /// Зависший CLI: на Unix обработчик tokio заменяет действие SIGTERM по умолчанию (до первого вызова, чтобы тест,
    /// дождавшийся `running`, не опередил его), и агент не завершается — Glafira должна дослать SIGKILL через 5 с. Шим
    /// SIGTERM не перехватывает и уходит сразу.
    async fn hang(&mut self) {
        #[cfg(unix)]
        let _term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate());
        let _ = self.call("get_optimization_request", json!({})).await;
        note("hanging=1");
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    }

    async fn limit(&mut self) -> String {
        for _ in 0..41 {
            let _ = self.call("list_connections", json!({})).await;
        }
        "Fake agent summary: stopped at the tool call limit.".to_string()
    }

    async fn foreign(&mut self) -> String {
        match self.s.client.list_all_prompts().await {
            Ok(p) => note(format!("prompts={}", p.len())),
            Err(e) => note(format!("prompts=error {}", one_line(&e.to_string()))),
        }
        let own = match self.call("list_connections", json!({})).await {
            Ok(v) => {
                let ids = connection_ids(&v);
                note(format!("connections={}", ids.join(",")));
                ids.into_iter().next().unwrap_or_default()
            }
            Err(_) => String::new(),
        };
        let conn = std::env::var("FAKE_AGENT_FOREIGN_CONNECTION").unwrap_or_default();
        let req = std::env::var("FAKE_AGENT_FOREIGN_REQUEST").unwrap_or_default();
        let _ = self
            .call(
                "explain_query",
                json!({ "connectionId": conn, "sql": "SELECT 1" }),
            )
            .await;
        let _ = self
            .call("get_optimization_request", json!({ "requestId": req }))
            .await;
        let _ = self
            .call(
                "run_select",
                json!({ "connectionId": own, "sql": "SELECT 1" }),
            )
            .await;
        // Прямая форма (без запроса) и предложение на чужой запрос.
        let _ = self
            .call(
                "propose_query_rewrite",
                json!({ "connectionId": own, "sql": "SELECT 1", "rewrittenSql": "SELECT 2",
                        "rationale": "Fake agent: direct form." }),
            )
            .await;
        let _ = self
            .call(
                "propose_query_rewrite",
                json!({ "requestId": req, "rewrittenSql": "SELECT 2", "rationale": "Fake agent: foreign request." }),
            )
            .await;
        "Fake agent summary: nothing to propose.".to_string()
    }

    async fn check(&mut self) -> String {
        if let Ok(v) = self.call("list_connections", json!({})).await {
            note(format!("connections={}", connection_ids(&v).len()));
        }
        "OK".to_string()
    }

    /// Вторая сессия того же запуска (второй шим с тем же билетом) сменяет первую: Glafira закрывает первую, её шим
    /// заканчивается сам (код выхода — строкой `first_shim_exit`), вызов через неё до Glafira не доходит.
    async fn twice(&mut self, server: &Server) -> Result<String, String> {
        self.call("list_connections", json!({})).await?;
        let second = Session::open(server, "shim2_pid").await?;
        let mut first = std::mem::replace(&mut self.s, second);
        let exit = match tokio::time::timeout(Duration::from_secs(20), first.shim.wait()).await {
            Ok(Ok(st)) => st.code().map_or("signal".to_string(), |c| c.to_string()),
            Ok(Err(e)) => format!("error:{}", one_line(&e.to_string())),
            Err(_) => "alive".to_string(),
        };
        note(format!("first_shim_exit={exit}"));
        self.calls += 1;
        let _ = call_on(&first.client, self.calls, "list_connections", json!({})).await;
        first.close().await;
        self.call("list_connections", json!({})).await?;
        Ok("Fake agent summary: the second session replaced the first one.".to_string())
    }
}

fn text_of(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect()
}

fn connection_ids(v: &Value) -> Vec<String> {
    v["connections"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn indexes() -> Vec<String> {
    std::env::var("FAKE_AGENT_INDEXES")
        .map(|s| {
            s.split(';')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Первый id запроса на оптимизацию (`r` и 6 строчных hex отдельным словом) в тексте промпта.
fn request_id_in(text: &str) -> Option<String> {
    let b = text.as_bytes();
    (0..b.len()).find_map(|i| {
        let end = i + 7;
        let found = end <= b.len()
            && b[i] == b'r'
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && (end == b.len() || !b[end].is_ascii_alphanumeric())
            && b[i + 1..end]
                .iter()
                .all(|&c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
        found.then(|| text[i..end].to_string())
    })
}

/// Итог в форме CLI: в потоке JSON — последнее событие `result` (`agent::process::final_text` берёт `result` и
/// `total_cost_usd`), у `--output-format json` — тот же объект один, иначе — текст.
fn print_result(output: Output, summary: &str, calls: u32) {
    let result = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "num_turns": calls,
        "result": summary,
        "session_id": SESSION_ID,
        "total_cost_usd": COST,
    });
    match output {
        Output::StreamJson | Output::Json => emit(&result),
        Output::Text => println!("{summary}"),
    }
}

/// 1,5 МБ в stdout — больше предела журнала запуска (1 МБ); в потоке JSON — событиями `assistant`.
fn flood(output: Output) {
    let filler = "x".repeat(960);
    let line = if output == Output::StreamJson {
        json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": filler }] } }).to_string()
    } else {
        filler
    };
    let mut out = std::io::stdout().lock();
    for _ in 0..1536 {
        if writeln!(out, "{line}").is_err() {
            break;
        }
    }
    let _ = out.flush();
}
