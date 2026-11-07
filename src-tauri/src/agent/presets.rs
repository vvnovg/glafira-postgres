//! Командная строка и окружение пресетов фонового ассистента (спец. P4c §4.1, итог проверки CLI — §16). Флаги собирает
//! только Glafira: текста пользователя и данных в аргументах нет — промпт идёт через stdin, MCP-конфиг с билетом — файлом
//! (`agent-runs/<runId>.mcp.json`). Чистые функции с «золотыми» тестами: новая версия CLI меняет флаги здесь и только здесь.

use std::path::PathBuf;

use glafira_pg_mcp::server::RUN_TOOL_NAMES;

use super::settings::{AgentSettings, Preset, bad_model, model_required, not_configured};
use super::tickets::MAX_RUN_CALLS;
use crate::error::AppResult;
use crate::product::MCP_SERVER_NAME;

/// Запасной предел CLI (`--max-wall-time`) — срок Glafira плюс столько секунд: первым срабатывает предел Glafira (§4.4).
pub const CLI_EXTRA_S: u64 = 30;
pub const MAX_MODEL_CHARS: usize = 128;

/// Встроенные инструменты Qwen Code 0.26.5 (опись `dev/agent-check`, вариант `q-inventory`). Выключаются только
/// запрещающим списком `--exclude-tools` со всеми именами версии: разрешающий `--core-tools` (пустой или с несуществующим
/// именем) оставляет 12–13 служебных инструментов (спека §4.1, §16). Список привязан к версии: инструмент новой версии
/// ловит проверка по событию `init` потока (`unsafeTools`, задача 6); перед выпуском список сверяется с описью.
pub const QWEN_BUILTIN_TOOLS: [&str; 31] = [
    "web_fetch",
    "get_goal",
    "update_goal",
    "tool_call",
    "tool_search",
    "read_mcp_resource",
    "agent",
    "list_agents",
    "task_stop",
    "send_message",
    "skill",
    "read_file",
    "manage_memory",
    "search_memory",
    "zoom_image",
    "grep_search",
    "edit",
    "notebook_edit",
    "write_file",
    "run_shell_command",
    "report_findings",
    "enter_worktree",
    "exit_worktree",
    "record_artifact",
    "cron_create",
    "cron_list",
    "cron_delete",
    "loop_wakeup",
    "autofix_pr",
    "monitor",
    "glob",
];

/// Формат вывода CLI. Оба пресета пишут поток JSON по строке (`stream-json`): первым — событие `system`/`init` со
/// списком инструментов, последним — `result` с итоговым текстом (`process::final_text`, задача 6). `Text` пресеты не
/// используют.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    StreamJson,
    Text,
}

/// Готовый к запуску процесс агента (`process::AgentSpawner::spawn`).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Переменные поверх окружения логин-оболочки (`shell_env`).
    pub env_set: Vec<(String, String)>,
    /// Переменные, которые убираются из окружения процесса.
    pub env_remove: Vec<String>,
    /// `agent-work/`: пустая папка без `QWEN.md` и `CLAUDE.md` проекта.
    pub cwd: PathBuf,
    pub output: OutputFormat,
}

/// Данные одного запуска для командной строки.
#[derive(Debug, Clone, PartialEq)]
pub struct RunInputs {
    /// `agent-runs/<runId>.mcp.json`.
    pub mcp_config: PathBuf,
    /// `agent-work/`.
    pub cwd: PathBuf,
    /// Модель, которую CLI получает флагом; `None` — модель CLI по умолчанию (у Qwen Code — `model.name` его настроек).
    pub model: Option<String>,
    /// Срок запуска Glafira, с.
    pub timeout_s: u64,
    /// Адрес Ollama (Claude Code «Через Ollama»; Qwen Code с `qwen_via_ollama`).
    pub ollama_url: String,
    /// Qwen Code: модель — модель Ollama (облачная или из `/api/tags` локальной Ollama), которой нет в его настройках
    /// (`models::qwen_knows`, очередь). Адрес Ollama и ключ — окружением процесса, протокол OpenAI — флагом.
    pub qwen_via_ollama: bool,
}

/// Ключ, который Ollama принимает от клиентов OpenAI и Anthropic API: не секрет, но в аргументы процесса не идёт —
/// окружением, как у Claude Code «Через Ollama».
pub const OLLAMA_KEY: &str = "ollama";

/// Минимальная версия CLI (спец. §4.1): ниже — предупреждение в «Настройках».
pub fn min_version(p: Preset) -> (u32, u32) {
    match p {
        Preset::Qwen => (0, 26),
        Preset::Claude => (2, 1),
    }
}

/// Имя модели — одна строка из `[A-Za-z0-9._:/@+-]`, до 128 символов, первый символ — латинская буква или цифра: в
/// аргументах процесса нет ничего, кроме того, что собрала Glafira. Имя на `-` CLI прочёл бы флагом, а не значением
/// `-m` / `--model` (`-m --yolo` у Qwen Code — флаг `--yolo`).
pub fn valid_model(m: &str) -> bool {
    m.starts_with(|c: char| c.is_ascii_alphanumeric())
        && m.chars().count() <= MAX_MODEL_CHARS
        && m.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/@+-".contains(c))
}

fn strings<'a>(items: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
    items.iter().map(|s| s.to_string())
}

/// Командная строка и окружение пресета (таблица §4.1).
pub fn build(s: &AgentSettings, i: &RunInputs) -> AppResult<CommandSpec> {
    let path = s.path().trim();
    if path.is_empty() {
        return Err(not_configured());
    }
    if let Some(m) = &i.model
        && !valid_model(m)
    {
        return Err(bad_model(m));
    }
    let config = i.mcp_config.to_string_lossy().into_owned();
    let mut args: Vec<String> = Vec::new();
    let mut env_set: Vec<(String, String)> = Vec::new();
    let mut env_remove: Vec<String> = Vec::new();
    match s.preset {
        Preset::Qwen => {
            args.extend([
                "--mcp-config".to_string(),
                config,
                "--allowed-mcp-server-names".into(),
                MCP_SERVER_NAME.into(),
                // Хуки, расширения и навыки пользователя выключены (проверено на хуке); файлы контекста — по описанию флага,
                // прогон этого не подтвердил (спека §16, «Итог проверки CLI», п. 10). Провайдер модели и сервер из
                // `--mcp-config` остаются. `--bare` не годится: Qwen Code теряет провайдера.
                "--safe-mode".into(),
                "--exclude-tools".into(),
            ]);
            args.extend(strings(&QWEN_BUILTIN_TOOLS));
            // Вызовы Glafira одобрены заранее (вместе с `"trust": true` у сервера в конфиге запуска), без `yolo` и без
            // `--approval-mode`: без окна Qwen Code 0.26.5 подтверждения не спрашивает (§16, п. 2).
            args.push("--allowed-tools".into());
            args.extend(
                RUN_TOOL_NAMES
                    .iter()
                    .map(|t| format!("mcp__{MCP_SERVER_NAME}__{t}")),
            );
            args.extend([
                "--max-wall-time".to_string(),
                format!("{}s", i.timeout_s + CLI_EXTRA_S),
                "--max-tool-calls".into(),
                MAX_RUN_CALLS.to_string(),
            ]);
            args.extend(strings(&["--chat-recording", "false", "-o", "stream-json"]));
            if i.qwen_via_ollama {
                // Модель Ollama не из настроек Qwen Code (смоук P4c: «Missing API key»): у модели без
                // `modelProviders` Qwen Code берёт ключ и адрес из `--openai-api-key` / `--openai-base-url`, затем из
                // `OPENAI_API_KEY` / `OPENAI_BASE_URL`, затем из `security.auth` своих настроек (Qwen Code 0.26.5).
                // Окружение процесса перекрывает и настройки, и ключ облака из профиля оболочки (он не уйдёт в Ollama);
                // ключ — не в аргументах. `--auth-type openai` — протокол Ollama `/v1`, даже если в Qwen Code выбран
                // другой провайдер (Qwen OAuth подменил бы модель своей). Файлы Qwen Code не меняются.
                args.extend(strings(&["--auth-type", "openai"]));
                env_set = vec![
                    (
                        "OPENAI_BASE_URL".to_string(),
                        format!("{}/v1", i.ollama_url.trim().trim_end_matches('/')),
                    ),
                    ("OPENAI_API_KEY".to_string(), OLLAMA_KEY.to_string()),
                ];
            }
            if let Some(m) = &i.model {
                args.extend(["-m".to_string(), m.clone()]);
            }
        }
        Preset::Claude => {
            args.extend([
                "-p".to_string(),
                "--mcp-config".into(),
                config,
                "--strict-mcp-config".into(),
            ]);
            // Пользовательские хуки, плагины и разрешения не читаются (рабочая папка пустая — проектных настроек нет).
            // `--bare` не берётся: он выключает вход по подписке.
            args.extend(strings(&["--setting-sources", "project,local"]));
            // `--tools ""` — ни одного встроенного инструмента; вызовы `glafira-pg` одобрены без вопросов.
            args.extend(strings(&["--tools", "", "--allowedTools"]));
            args.push(format!("mcp__{MCP_SERVER_NAME}"));
            args.extend(strings(&[
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence",
                "--output-format",
                "stream-json",
                "--verbose",
            ]));
            if let Some(m) = &i.model {
                args.extend(["--model".to_string(), m.clone()]);
            }
            if s.claude.via_ollama {
                let m = i.model.clone().ok_or_else(model_required)?;
                let pairs = [
                    ("ANTHROPIC_BASE_URL", i.ollama_url.as_str()),
                    ("ANTHROPIC_AUTH_TOKEN", OLLAMA_KEY),
                    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                    ("ANTHROPIC_MODEL", m.as_str()),
                    ("ANTHROPIC_DEFAULT_OPUS_MODEL", m.as_str()),
                    ("ANTHROPIC_DEFAULT_SONNET_MODEL", m.as_str()),
                    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", m.as_str()),
                    ("CLAUDE_CODE_SUBAGENT_MODEL", m.as_str()),
                ];
                env_set = pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                // Окружение — из профиля оболочки пользователя (§4.2), а «через Ollama» — это локально (fail-safe):
                // - ключ облака не должен уйти в Ollama и не должен перебить её адрес;
                // - с `CLAUDE_CODE_USE_BEDROCK` / `CLAUDE_CODE_USE_VERTEX` / `CLAUDE_CODE_USE_FOUNDRY` Claude Code идёт в
                //   Bedrock / Vertex / Microsoft Foundry мимо `ANTHROPIC_BASE_URL` — запуск ушёл бы в платное облако;
                // - `ANTHROPIC_CUSTOM_HEADERS` (бывает, с токеном шлюза) ушли бы на адрес Ollama, в том числе удалённый.
                env_remove.extend(strings(&[
                    "ANTHROPIC_API_KEY",
                    "CLAUDE_CODE_USE_BEDROCK",
                    "CLAUDE_CODE_USE_VERTEX",
                    "CLAUDE_CODE_USE_FOUNDRY",
                    "ANTHROPIC_CUSTOM_HEADERS",
                ]));
            } else if let Some(b) = s.claude.budget_usd {
                // Только без Ollama: через Ollama Claude Code считает стоимость по ценам своей модели (§4.1).
                args.extend(["--max-budget-usd".to_string(), format!("{b}")]);
            }
        }
    }
    Ok(CommandSpec {
        program: PathBuf::from(path),
        args,
        env_set,
        env_remove,
        cwd: i.cwd.clone(),
        output: OutputFormat::StreamJson,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::settings::{ClaudeSettings, QwenSettings};

    fn inputs(model: Option<&str>) -> RunInputs {
        RunInputs {
            mcp_config: PathBuf::from("/data/agent-runs/a1b2c3d.mcp.json"),
            cwd: PathBuf::from("/data/agent-work"),
            model: model.map(String::from),
            timeout_s: 900,
            ollama_url: "http://127.0.0.1:11434".into(),
            qwen_via_ollama: false,
        }
    }

    fn qwen() -> AgentSettings {
        AgentSettings {
            qwen: QwenSettings {
                path: "/opt/homebrew/bin/qwen".into(),
                model: String::new(),
            },
            ..AgentSettings::default()
        }
    }

    fn claude(via_ollama: bool, budget: Option<f64>) -> AgentSettings {
        AgentSettings {
            preset: Preset::Claude,
            claude: ClaudeSettings {
                path: "/Users/u/.local/bin/claude".into(),
                model: String::new(),
                via_ollama,
                budget_usd: budget,
            },
            ..AgentSettings::default()
        }
    }

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn qwen_golden_command_line() {
        let spec = build(&qwen(), &inputs(None)).unwrap();
        assert_eq!(spec.program, PathBuf::from("/opt/homebrew/bin/qwen"));
        assert_eq!(spec.cwd, PathBuf::from("/data/agent-work"));
        assert_eq!(spec.output, OutputFormat::StreamJson);
        let mut want = v(&[
            "--mcp-config",
            "/data/agent-runs/a1b2c3d.mcp.json",
            "--allowed-mcp-server-names",
            "glafira-pg",
            "--safe-mode",
            "--exclude-tools",
            "web_fetch",
            "get_goal",
            "update_goal",
            "tool_call",
            "tool_search",
            "read_mcp_resource",
            "agent",
            "list_agents",
            "task_stop",
            "send_message",
            "skill",
            "read_file",
            "manage_memory",
            "search_memory",
            "zoom_image",
            "grep_search",
            "edit",
            "notebook_edit",
            "write_file",
            "run_shell_command",
            "report_findings",
            "enter_worktree",
            "exit_worktree",
            "record_artifact",
            "cron_create",
            "cron_list",
            "cron_delete",
            "loop_wakeup",
            "autofix_pr",
            "monitor",
            "glob",
            "--allowed-tools",
        ]);
        want.extend(
            RUN_TOOL_NAMES
                .iter()
                .map(|t| format!("mcp__glafira-pg__{t}")),
        );
        want.extend(v(&[
            "--max-wall-time",
            "930s",
            "--max-tool-calls",
            "40",
            "--chat-recording",
            "false",
            "-o",
            "stream-json",
        ]));
        assert_eq!(spec.args, want);
        assert!(
            spec.env_set.is_empty() && spec.env_remove.is_empty(),
            "провайдер — из настроек Qwen Code, как в чате"
        );
        // Разрешены ровно девять инструментов сессии запуска (спека §5.2).
        let mut tools: Vec<&str> = RUN_TOOL_NAMES.to_vec();
        tools.sort_unstable();
        assert_eq!(
            tools,
            [
                "explain_query",
                "get_optimization_request",
                "get_query_details",
                "get_settings",
                "get_table_details",
                "hypothetical_index",
                "list_connections",
                "list_proposals",
                "propose_query_rewrite",
            ]
        );
        let spec = build(&qwen(), &inputs(Some("qwen3.6-64k:35b-a3b"))).unwrap();
        assert_eq!(
            spec.args[spec.args.len() - 2..],
            v(&["-m", "qwen3.6-64k:35b-a3b"])
        );
        assert_eq!(
            spec.args[..spec.args.len() - 2],
            want,
            "модель — только в конце"
        );
    }

    /// Смоук P4c, sf-T7: облачная модель Ollama, которой нет в настройках Qwen Code, — адрес Ollama (настройка «Адрес
    /// Ollama» + `/v1`) и ключ `ollama` окружением процесса (не аргументами), протокол OpenAI — флагом `--auth-type`,
    /// чтобы выбранный в Qwen Code провайдер (Qwen OAuth, Anthropic) не увёл модель мимо Ollama. Модель из настроек Qwen
    /// Code — как в чате, без окружения и флага.
    #[test]
    fn qwen_with_a_cloud_model_outside_its_settings_gets_the_ollama_address_and_key() {
        let m = "glm-5.3-flash:cloud";
        let plain = build(&qwen(), &inputs(Some(m))).unwrap();
        let mut i = inputs(Some(m));
        i.qwen_via_ollama = true;
        let spec = build(&qwen(), &i).unwrap();
        let pairs = |items: &[(&str, &str)]| -> Vec<(String, String)> {
            items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            spec.env_set,
            pairs(&[
                ("OPENAI_BASE_URL", "http://127.0.0.1:11434/v1"),
                ("OPENAI_API_KEY", "ollama"),
            ])
        );
        assert!(spec.env_remove.is_empty(), "{:?}", spec.env_remove);
        let mut want = plain.args[..plain.args.len() - 2].to_vec();
        want.extend(v(&["--auth-type", "openai", "-m", m]));
        assert_eq!(spec.args, want, "модель — по-прежнему в конце");
        for a in &spec.args {
            assert!(
                a != "ollama" && !a.starts_with("--openai-"),
                "ключ и адрес — не в аргументах: {a}"
            );
        }
        // Удалённая Ollama — её адрес; косая черта в конце не удваивается.
        i.ollama_url = "http://192.168.1.10:11434/".into();
        assert_eq!(
            build(&qwen(), &i).unwrap().env_set[0],
            (
                "OPENAI_BASE_URL".to_string(),
                "http://192.168.1.10:11434/v1".to_string()
            )
        );
        // Модель из настроек Qwen Code (`qwen_via_ollama: false`) — как в чате.
        assert!(plain.env_set.is_empty() && plain.env_remove.is_empty());
        assert!(!plain.args.iter().any(|a| a == "--auth-type"));
        // У Claude Code признака нет: «Через Ollama» — свои переменные.
        let mut c = inputs(Some(m));
        c.qwen_via_ollama = true;
        let claude = build(&claude(false, None), &c).unwrap();
        assert!(claude.env_set.is_empty() && !claude.args.iter().any(|a| a == "--auth-type"));
    }

    #[test]
    fn qwen_flags_switch_off_every_builtin_tool_and_user_settings() {
        // Итог проверки CLI (спека §4.1, §16): только запрещающий список со всеми встроенными инструментами версии;
        // `--core-tools` оставляет служебные, `--approval-mode` не нужен (без окна Qwen Code не спрашивает), `--bare`
        // теряет провайдера; `--safe-mode` выключает хуки, расширения и навыки пользователя.
        let spec = build(&qwen(), &inputs(Some("m1"))).unwrap();
        let args = &spec.args;
        let at = args
            .iter()
            .position(|a| a == "--exclude-tools")
            .expect("--exclude-tools");
        let excluded: Vec<&str> = args[at + 1..]
            .iter()
            .take_while(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        assert_eq!(
            excluded, QWEN_BUILTIN_TOOLS,
            "весь список подряд, в одном флаге"
        );
        for flag in ["--core-tools", "--approval-mode", "--bare", "--yolo", "-y"] {
            assert!(!args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        assert_eq!(args.iter().filter(|a| *a == "--safe-mode").count(), 1);
        let unique: std::collections::BTreeSet<&str> = QWEN_BUILTIN_TOOLS.iter().copied().collect();
        assert_eq!(unique.len(), QWEN_BUILTIN_TOOLS.len(), "без повторов");
        for t in RUN_TOOL_NAMES {
            assert!(
                !unique.contains(t),
                "{t}: инструмент Glafira не исключается"
            );
        }
        assert!(QWEN_BUILTIN_TOOLS.iter().all(|t| !t.starts_with("mcp__")));
    }

    #[test]
    fn claude_golden_command_line_with_a_budget() {
        let spec = build(&claude(false, Some(1.0)), &inputs(Some("sonnet"))).unwrap();
        assert_eq!(spec.program, PathBuf::from("/Users/u/.local/bin/claude"));
        assert_eq!(spec.output, OutputFormat::StreamJson);
        assert_eq!(
            spec.args,
            v(&[
                "-p",
                "--mcp-config",
                "/data/agent-runs/a1b2c3d.mcp.json",
                "--strict-mcp-config",
                "--setting-sources",
                "project,local",
                "--tools",
                "",
                "--allowedTools",
                "mcp__glafira-pg",
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence",
                "--output-format",
                "stream-json",
                "--verbose",
                "--model",
                "sonnet",
                "--max-budget-usd",
                "1",
            ])
        );
        assert!(spec.env_set.is_empty() && spec.env_remove.is_empty());
        // Без модели — модель Claude Code по умолчанию; без предела — без флага; дробный предел — как есть.
        let spec = build(&claude(false, None), &inputs(None)).unwrap();
        assert_eq!(
            spec.args[spec.args.len() - 3..],
            v(&["--output-format", "stream-json", "--verbose"])
        );
        let spec = build(&claude(false, Some(2.5)), &inputs(None)).unwrap();
        assert_eq!(
            spec.args[spec.args.len() - 2..],
            v(&["--max-budget-usd", "2.5"])
        );
    }

    #[test]
    fn claude_flags_keep_user_hooks_and_builtin_tools_out() {
        // `--setting-sources project,local` — без пользовательских хуков, плагинов и разрешений; `--bare` выключил бы
        // вход по подписке (спека §4.1, §16).
        for spec in [
            build(&claude(false, Some(1.0)), &inputs(None)).unwrap(),
            build(&claude(true, None), &inputs(Some("m1"))).unwrap(),
        ] {
            let args = &spec.args;
            assert!(!args.iter().any(|a| a == "--bare"), "{args:?}");
            let at = args
                .iter()
                .position(|a| a == "--setting-sources")
                .expect("--setting-sources");
            assert_eq!(args[at + 1], "project,local");
            let at = args.iter().position(|a| a == "--tools").expect("--tools");
            assert_eq!(args[at + 1], "", "ни одного встроенного инструмента");
        }
    }

    #[test]
    fn claude_through_ollama_sets_the_environment_and_has_no_budget() {
        let m = "qwen3.6-64k:35b-a3b";
        let spec = build(&claude(true, Some(1.0)), &inputs(Some(m))).unwrap();
        assert!(
            !spec.args.iter().any(|a| a == "--max-budget-usd"),
            "у локальной модели расходов нет"
        );
        assert_eq!(spec.args[spec.args.len() - 2..], v(&["--model", m]));
        let pairs = |items: &[(&str, &str)]| -> Vec<(String, String)> {
            items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            spec.env_set,
            pairs(&[
                ("ANTHROPIC_BASE_URL", "http://127.0.0.1:11434"),
                ("ANTHROPIC_AUTH_TOKEN", "ollama"),
                ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                ("ANTHROPIC_MODEL", m),
                ("ANTHROPIC_DEFAULT_OPUS_MODEL", m),
                ("ANTHROPIC_DEFAULT_SONNET_MODEL", m),
                ("ANTHROPIC_DEFAULT_HAIKU_MODEL", m),
                ("CLAUDE_CODE_SUBAGENT_MODEL", m),
            ])
        );
        assert_eq!(
            spec.env_remove,
            v(&[
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_USE_BEDROCK",
                "CLAUDE_CODE_USE_VERTEX",
                "CLAUDE_CODE_USE_FOUNDRY",
                "ANTHROPIC_CUSTOM_HEADERS"
            ])
        );
        assert_eq!(
            build(&claude(true, None), &inputs(None)).unwrap_err().key,
            "errors.agent.modelRequired"
        );
    }

    #[test]
    fn model_names_and_missing_paths_are_checked() {
        for good in [
            "qwen3.6-64k:35b-a3b",
            "claude-sonnet-4-5",
            "hf.co/org/model:Q4_K_M",
            "user@host/m+x",
        ] {
            assert!(valid_model(good), "{good}");
        }
        let long = "m".repeat(MAX_MODEL_CHARS + 1);
        for bad in ["", "two words", "a;b", "x\ny", "модель", long.as_str()] {
            assert!(!valid_model(bad), "{bad}");
        }
        // Первый символ — буква или цифра: имя на `-` CLI прочёл бы флагом (`-m --yolo`), а не значением модели.
        for flag in [
            "--yolo",
            "-y",
            "--mcp-config=/tmp/x.json",
            "-",
            ".hidden",
            "/abs",
            ":tag",
            "@x",
            "+x",
            "_x",
        ] {
            assert!(!valid_model(flag), "{flag}");
        }
        assert!(valid_model("7b-model") && valid_model("Q"));
        for model in ["two words", "--yolo"] {
            assert_eq!(
                build(&qwen(), &inputs(Some(model))).unwrap_err().key,
                "errors.agent.badModel",
                "{model}"
            );
            assert_eq!(
                build(&claude(true, None), &inputs(Some(model)))
                    .unwrap_err()
                    .key,
                "errors.agent.badModel",
                "{model}"
            );
        }
        let mut empty = qwen();
        empty.qwen.path = "  ".into();
        assert_eq!(
            build(&empty, &inputs(None)).unwrap_err().key,
            "errors.agent.notConfigured"
        );
    }

    #[test]
    fn the_prompt_never_goes_to_the_arguments() {
        // У `build` нет параметра промпта: в аргументах — флаги пресета, путь конфига, числа и имя модели.
        for spec in [
            build(&qwen(), &inputs(Some("m1"))).unwrap(),
            build(&claude(true, None), &inputs(Some("m1"))).unwrap(),
            build(&claude(false, Some(1.0)), &inputs(None)).unwrap(),
        ] {
            assert!(
                spec.args
                    .iter()
                    .all(|a| !a.chars().any(char::is_whitespace)),
                "{:?}",
                spec.args
            );
            assert!(
                spec.args
                    .iter()
                    .all(|a| a.len() <= 64 || a.ends_with(".mcp.json")),
                "{:?}",
                spec.args
            );
        }
    }

    #[test]
    fn minimum_versions() {
        assert_eq!(min_version(Preset::Qwen), (0, 26));
        assert_eq!(min_version(Preset::Claude), (2, 1));
    }
}
