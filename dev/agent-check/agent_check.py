"""Проверка агентских CLI для фонового ассистента P4c (спека P4c §4.1; план P4c, задача 1): Qwen Code и Claude Code без окна,
с локальной моделью Ollama, против шлюза Glafira без окна (`llm_eval_host` из dev/llm-eval) с подготовленными запросами на
оптимизацию к pg-perf. Только для разработки; модель запускается на M4 Pro и только по слову владельца.

python3 -I dev/agent-check/agent_check.py run [--model qwen3.6-64k:35b-a3b] [--only q-allowed,c-preset] [--canary] [--cloud]
                                              [--allow-evict] [--timeout 900]
python3 -I dev/agent-check/agent_check.py summary dev/agent-check/out/<время>

`run`: `ollama ps` (загружена другая модель — остановка, пока владелец не разрешит `--allow-evict`), сведения о CLI
(facts.json), шлюз с запросами на оптимизацию, варианты VARIANTS по очереди; итог — out/<время>/<вариант>.result.json,
.stdout, .stderr, .probe.*.json и summary.md. Модель, которую загрузил прогон, в конце выгружается; модель, которая была в
памяти до прогона, остаётся.
Служебные режимы (их запускают CLI как MCP-сервер): `shim-probe` — записывает, какие переменные из `env` MCP-конфига дошли до
сервера, и заменяет себя шимом `glafira-pg mcp`; `decoy` — сервер-приманка `glafira-pg` в настройках Qwen Code: записывает,
что его запустили, и выходит с ошибкой.
"""
import argparse
import json
import os
import secrets
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUT = HERE / "out"
SCRIPT = str(Path(__file__).resolve())
EVAL_HOME = os.environ.get("EVAL_HOME", str(Path.home() / ".glafira-llm-eval"))
AUDIT = Path(EVAL_HOME) / "Library/Application Support/io.github.vvnovg.glafira.postgres/mcp-audit.jsonl"
SHIM = str(ROOT / "src-tauri/target/debug/glafira-pg")
HOST = ROOT / "src-tauri/target/debug/examples/llm_eval_host"
OLLAMA = os.environ.get("OLLAMA_URL", "http://127.0.0.1:11434")
os.environ["PATH"] = f"{Path.home() / '.local/bin'}:/opt/homebrew/bin:/usr/local/bin:{os.environ.get('PATH', '')}"

SERVER = "glafira-pg"
# Не билет: фиксированное значение, по которому проба видит, что переменная из `env` MCP-конфига дошла до сервера.
TICKET = "ab" * 32
TOOL_NAMES = [
    "list_connections", "get_topology", "get_health_summary", "list_diagnostics", "run_diagnostic", "get_activity",
    "observe_activity", "get_top_queries", "observe_top_queries", "get_query_details", "get_table_details", "get_settings",
    "explain_query", "hypothetical_index", "run_select", "get_optimization_request", "propose_query_rewrite",
    "list_proposals",
]
# Девять инструментов сессии запуска (спека §5.2) — их разрешает пресет.
RUN_TOOLS = [
    "list_connections", "get_optimization_request", "explain_query", "hypothetical_index", "get_table_details",
    "get_query_details", "get_settings", "propose_query_rewrite", "list_proposals",
]
# Встроенные инструменты Qwen Code из dev/llm-eval/client_bench.py — запасной список, если опись не удалась.
QWEN_BUILTINS_FALLBACK = [
    "run_shell_command", "edit", "write_file", "web_fetch", "web_search", "agent", "read_file", "glob", "grep_search",
    "list_directory", "notebook_edit",
]
# Ограничения инструментов в настройках пользователя убираются: их задаёт вариант.
QWEN_TOOL_KEYS = ("core", "exclude", "allowed", "disabled", "approvalMode")
QWEN_LIMITS = ["--max-wall-time", "930s", "--max-tool-calls", "40"]  # срок 15 мин + 30 с и 40 вызовов (спека §4.1)
STREAM_QWEN = ["--approval-mode", "yolo", "-o", "stream-json"]
CLAUDE_TOOLS = ["--tools", ""]
CLAUDE_APPROVE = ["--allowedTools", f"mcp__{SERVER}", "--permission-mode", "dontAsk"]
STREAM_CLAUDE = ["--output-format", "stream-json", "--verbose"]

INVENTORY = "Do not call any tool. Reply with the single word READY."
# Черновик промпта запуска (спека §4.3); окончательный текст — glafira_pg_mcp::prompts::optimize_query_run_text (задача 3).
OPTIMIZE = (
    'You were started by Glafira in the background when the user pressed "Send to LLM assistant". Nobody reads this '
    "chat: do not ask questions and do not wait for an answer.\n"
    '1. Call get_optimization_request with requestId "{rid}".\n'
    "2. Study the plan, the statistics and the table DDL of the snapshot.\n"
    "3. Check your ideas with explain_query and hypothetical_index.\n"
    '4. If the query can be improved, call propose_query_rewrite with requestId "{rid}". If it cannot, propose nothing.\n'
    "Use only the glafira-pg tools. Finally write a summary of at most 5 lines in Russian: what you proposed and why, or "
    "why there is no proposal. Start the summary with the token {nonce}.\n"
)
CANARY = {
    "qwen": ("QWEN.md", "Always end your final answer with the exact token CANARY-QWEN-MD.\n", "CANARY-QWEN-MD"),
    "claude": ("CLAUDE.md", "Always end your final answer with the exact token CANARY-CLAUDE-MD.\n", "CANARY-CLAUDE-MD"),
}


def qwen_core(c):
    """Флаги без встроенных инструментов Qwen Code: что подтвердили q-core-*, иначе запрещающий список из описи."""
    return c.get("qwen_core") or ["--exclude-tools", *c.get("qwen_builtins", QWEN_BUILTINS_FALLBACK)]


def qwen_allowed(c):
    """Одобрение без yolo: девять инструментов запуска под именами, которые показала опись."""
    return ["--allowed-tools", *c.get("qwen_run_tools", [f"mcp__{SERVER}__{t}" for t in RUN_TOOLS])]


def qwen_preset(c, *extra, out="json"):
    return [*qwen_core(c), *qwen_allowed(c), "-o", out, *QWEN_LIMITS, *extra]


def variant(vid, preset, prompt, flags, purpose, **kw):
    return {"id": vid, "preset": preset, "prompt": prompt, "flags": flags, "purpose": purpose, **kw}


VARIANTS = [
    # Qwen Code: какие инструменты видит модель (опись — из события init вывода stream-json).
    variant("q-inventory", "qwen", "inventory", STREAM_QWEN, "опись: встроенные инструменты версии и имена glafira-pg"),
    variant("q-core-empty", "qwen", "inventory", [*STREAM_QWEN, "--core-tools", ""], 'разрешающий список: --core-tools ""',
            core=["--core-tools", ""]),
    variant("q-core-bogus", "qwen", "inventory", [*STREAM_QWEN, "--core-tools", "glafira_none"],
            "разрешающий список из несуществующего имени", core=["--core-tools", "glafira_none"]),
    variant("q-core-agent", "qwen", "inventory", [*STREAM_QWEN, "--core-tools", "glafira_none", "--exclude-tools", "agent"],
            "временная золотая строка задачи 5: разрешающий список и без субагентов",
            core=["--core-tools", "glafira_none", "--exclude-tools", "agent"]),
    variant("q-exclude", "qwen", "inventory",
            lambda c: [*STREAM_QWEN, "--exclude-tools", *c.get("qwen_builtins", QWEN_BUILTINS_FALLBACK)],
            "запрещающий список всех встроенных"),
    # Qwen Code: одобрение без yolo, полный запуск, пределы.
    variant("q-allowed", "qwen", "optimize", lambda c: qwen_preset(c), "кандидат пресета: --allowed-tools и trust, без yolo"),
    variant("q-trust", "qwen", "optimize", lambda c: [*qwen_core(c), "-o", "json", *QWEN_LIMITS], "только trust у сервера"),
    variant("q-none", "qwen", "optimize", lambda c: [*qwen_core(c), "-o", "json", *QWEN_LIMITS],
            "без trust и --allowed-tools: неодобренный вызов отклоняется или ждёт?", trust=False),
    variant("q-yolo", "qwen", "optimize", lambda c: [*qwen_core(c), "--approval-mode", "yolo", "-o", "json", *QWEN_LIMITS],
            "запасной вариант: yolo"),
    variant("q-text", "qwen", "optimize", lambda c: qwen_preset(c, out="text"), "вывод text: итоговый текст"),
    variant("q-limit-calls", "qwen", "optimize",
            lambda c: [*qwen_core(c), *qwen_allowed(c), "-o", "json", "--max-tool-calls", "1"],
            "код выхода при пределе вызовов (ждём 55)"),
    variant("q-limit-wall", "qwen", "optimize",
            lambda c: [*qwen_core(c), *qwen_allowed(c), "-o", "json", "--max-wall-time", "10s"],
            "код выхода при пределе времени (ждём 55)"),
    variant("q-bare", "qwen", "optimize", lambda c: qwen_preset(c, "--bare"),
            "--bare: настройки провайдера и сервер из --mcp-config остаются?"),
    variant("q-safe", "qwen", "optimize", lambda c: qwen_preset(c, "--safe-mode"), "--safe-mode: сервер из --mcp-config остаётся?"),
    variant("q-ext-none", "qwen", "inventory", lambda c: [*STREAM_QWEN, *qwen_core(c), "-e", "none"],
            "-e none: принимается, расширения выключаются?"),
    # Claude Code через Ollama.
    variant("c-inventory", "claude", "inventory", [*CLAUDE_TOOLS, *CLAUDE_APPROVE, *STREAM_CLAUDE],
            "опись: только mcp__glafira-pg__*?"),
    variant("c-preset", "claude", "optimize", [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json"],
            "пресет спеки: вызовы без подтверждения, поля JSON", debug=True),
    variant("c-text", "claude", "optimize", [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "text"], "вывод text"),
    variant("c-no-allowed", "claude", "optimize", [*CLAUDE_TOOLS, "--permission-mode", "dontAsk", "--output-format", "json"],
            "dontAsk без --allowedTools: вызовы отклоняются?"),
    variant("c-prompts-none", "claude", "optimize",
            [*CLAUDE_TOOLS, "--allowedTools", f"mcp__{SERVER}", "--permission-prompts", "none", "--output-format", "json"],
            "--permission-prompts none вместо dontAsk"),
    variant("c-budget", "claude", "optimize",
            [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json", "--max-budget-usd", "0.000001"],
            "код выхода и subtype при пределе расходов (через Ollama)"),
    # --canary: QWEN.md / CLAUDE.md и хук SessionStart во временном HOME (Qwen Code) или CLAUDE_CONFIG_DIR (Claude Code).
    variant("q-canary", "qwen", "optimize", lambda c: qwen_preset(c), "QWEN.md и хук пользователя при флагах пресета",
            canary=True),
    variant("q-canary-bare", "qwen", "optimize", lambda c: qwen_preset(c, "--bare"), "то же с --bare", canary=True),
    variant("q-canary-safe", "qwen", "optimize", lambda c: qwen_preset(c, "--safe-mode"), "то же с --safe-mode", canary=True),
    variant("c-canary", "claude", "optimize", [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json"],
            "CLAUDE.md и хук пользователя при флагах пресета", canary=True),
    variant("c-canary-sources", "claude", "optimize",
            [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json", "--setting-sources", "project,local"],
            "то же без пользовательских настроек", canary=True),
    variant("c-canary-nocmds", "claude", "optimize",
            [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json", "--disable-slash-commands"],
            "то же с --disable-slash-commands (навыки)", canary=True),
    # --cloud — только по слову владельца: платный вызов, Claude Code входит своей подпиской или ключом.
    variant("c-cloud", "claude", "optimize",
            [*CLAUDE_TOOLS, *CLAUDE_APPROVE, "--output-format", "json", "--max-budget-usd", "0.05"],
            "стоимость в JSON и предел расходов без Ollama", cloud=True),
]


def write_private(path, text):
    """Файл с правами 0600, как MCP-конфиг запуска в Glafira."""
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(text)


def ollama(path, body=None, timeout=10):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(OLLAMA + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def loaded_models():
    return [m["name"] for m in ollama("/api/ps").get("models", [])]


def is_glafira(name):
    return SERVER in name or name in TOOL_NAMES


def base(name):
    return name.rsplit("__", 1)[-1]


def facts(model):
    """Версии и что лежит в настройках пользователя (только имена ключей и наличие файлов, без значений)."""
    def out(cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.strip()[:4000]
        except (OSError, subprocess.TimeoutExpired) as e:
            return f"error: {e}"

    def keys(path):
        try:
            return sorted(json.loads(Path(path).expanduser().read_text()))
        except (OSError, ValueError):
            return None

    ext = Path.home() / ".qwen/extensions"
    return {
        "model": model,
        "qwen": out(["qwen", "--version"]),
        "claude": out(["claude", "--version"]),
        "ollama": ollama("/api/version").get("version"),
        "qwen_settings_keys": keys("~/.qwen/settings.json"),
        "qwen_extensions": sorted(os.listdir(ext)) if ext.is_dir() else [],
        "qwen_md": (Path.home() / ".qwen/QWEN.md").exists(),
        "qwen_hooks_help": out(["qwen", "hooks", "--help"]),
        "claude_settings_keys": keys("~/.claude/settings.json"),
        "claude_md": (Path.home() / ".claude/CLAUDE.md").exists(),
    }


def start_host(n, log):
    subprocess.run(["cargo", "build", "-q", "--bin", "glafira-pg", "--example", "llm_eval_host"],
                   cwd=ROOT / "src-tauri", check=True)
    env = {**os.environ, "EVAL_HOME": EVAL_HOME, "EVAL_REQUESTS": str(n)}
    host = subprocess.Popen([str(HOST)], stdout=subprocess.PIPE, stderr=open(log, "a"), text=True, env=env)
    line = host.stdout.readline()
    ids = next((t.split("=", 1)[1].split(",") for t in line.split() if t.startswith("requests=")), [])
    if not line.startswith("READY") or len(ids) < n:
        host.kill()
        sys.exit(f"llm_eval_host не запустился или не создал {n} запросов: {line!r}, см. {log}")
    return host, ids


def stop_host(host):
    host.terminate()
    try:
        host.wait(timeout=10)
    except subprocess.TimeoutExpired:
        host.kill()


def mcp_config(preset, probe, nonce, trust):
    server = {"command": sys.executable,
              "args": ["-I", SCRIPT, "shim-probe", "--out", str(probe), "--nonce", nonce, "--home", EVAL_HOME],
              "env": {"GLAFIRA_AGENT_TICKET": TICKET, "GLAFIRA_CHECK_NONCE": nonce}}
    if preset == "qwen":
        server["timeout"] = 120000
        if trust:
            server["trust"] = True
    return {"mcpServers": {SERVER: server}}


def canary_hook(marker):
    # Формат хуков Claude Code; у Qwen Code сверить с `qwen hooks --help` (facts.json, qwen_hooks_help) — README, «Хуки».
    return {"SessionStart": [{"hooks": [{"type": "command", "command": f"touch '{marker}'"}]}]}


def qwen_home(rdir, vid, canary):
    """Временный HOME Qwen Code: копия ~/.qwen/settings.json без ограничений инструментов и с приманкой `glafira-pg` в
    `mcpServers`, `.env` и `oauth_creds.json`, если есть; у --canary — ещё QWEN.md и хук SessionStart."""
    home = Path(tempfile.mkdtemp(prefix="agent-check-qwen-"))
    qdir = home / ".qwen"
    qdir.mkdir(mode=0o700)
    src = Path.home() / ".qwen"
    cfg = json.loads((src / "settings.json").read_text()) if (src / "settings.json").exists() else {}
    if isinstance(cfg.get("tools"), dict):
        for k in QWEN_TOOL_KEYS:
            cfg["tools"].pop(k, None)
    # Приманка: сервер с тем же именем в настройках пользователя. Если Qwen Code возьмёт его, а не сервер из --mcp-config,
    # появится запись <вариант>.decoy.*.json; к Glafira владельца прогон при этом не подключится.
    cfg["mcpServers"] = {SERVER: {"command": sys.executable,
                                  "args": ["-I", SCRIPT, "decoy", "--out", str(rdir / f"{vid}.decoy")]}}
    marker = rdir / f"{vid}.hook-fired"
    if canary:
        name, text, _ = CANARY["qwen"]
        (qdir / name).write_text(text)
        cfg["hooks"] = canary_hook(marker)
    write_private(qdir / "settings.json", json.dumps(cfg, indent=1))
    for name in (".env", "oauth_creds.json"):
        if (src / name).exists():
            shutil.copy2(src / name, qdir / name)
            os.chmod(qdir / name, 0o600)
    return home, marker


def claude_home(rdir, vid):
    """Временный CLAUDE_CONFIG_DIR (только --canary): CLAUDE.md и хук SessionStart. Через Ollama вход не нужен."""
    home = Path(tempfile.mkdtemp(prefix="agent-check-claude-"))
    cdir = home / ".claude"
    cdir.mkdir(mode=0o700)
    name, text, _ = CANARY["claude"]
    (cdir / name).write_text(text)
    marker = rdir / f"{vid}.hook-fired"
    write_private(cdir / "settings.json", json.dumps({"hooks": canary_hook(marker)}, indent=1))
    return home, marker


def command(v, ctx, cfg, model):
    flags = v["flags"](ctx) if callable(v["flags"]) else list(v["flags"])
    env = dict(os.environ)
    if v["preset"] == "qwen":
        cmd = [shutil.which("qwen") or "qwen", "--mcp-config", str(cfg), "--allowed-mcp-server-names", SERVER,
               "--chat-recording", "false", "-m", model]
        return cmd + flags, env
    cmd = [shutil.which("claude") or "claude", "-p", "--mcp-config", str(cfg), "--strict-mcp-config",
           "--no-session-persistence"]
    if not v.get("cloud"):
        cmd += ["--model", model]
        env.update({"ANTHROPIC_BASE_URL": OLLAMA, "ANTHROPIC_AUTH_TOKEN": "ollama", "ANTHROPIC_MODEL": model,
                    "ANTHROPIC_DEFAULT_OPUS_MODEL": model, "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
                    "ANTHROPIC_DEFAULT_HAIKU_MODEL": model, "CLAUDE_CODE_SUBAGENT_MODEL": model,
                    "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"})
        env.pop("ANTHROPIC_API_KEY", None)
    return cmd + flags, env


def stop_group(pid):
    """Как Glafira (спека §4.4): SIGTERM группе процессов, через 5 с — SIGKILL."""
    for sig, pause in ((signal.SIGTERM, 5), (signal.SIGKILL, 0)):
        try:
            os.killpg(pid, sig)
        except ProcessLookupError:
            return
        time.sleep(pause)


def audit_calls(since_ms):
    calls = []
    if not AUDIT.exists():
        return calls
    for line in AUDIT.read_text().splitlines():
        try:
            r = json.loads(line)
        except json.JSONDecodeError:
            continue
        if r.get("event") == "call" and r.get("ts", 0) >= since_ms:
            args = r.get("args") if isinstance(r.get("args"), dict) else {}
            calls.append({"tool": r.get("tool"), "code": r.get("code"), "client": r.get("client"),
                          "requestId": args.get("requestId")})
    return calls


def messages(out):
    """Сообщения вывода: один объект, массив или JSON по строке (stream-json)."""
    text = out.strip()
    if not text:
        return "empty", []
    try:
        v = json.loads(text)
        return ("array", v) if isinstance(v, list) else ("object", [v])
    except json.JSONDecodeError:
        msgs = []
        for line in text.splitlines():
            try:
                msgs.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return ("ndjson" if msgs else "text"), msgs


def init_tools(out):
    """Инструменты из события `system`/`init` (stream-json) или None."""
    for m in messages(out)[1]:
        if isinstance(m, dict) and m.get("type") == "system" and m.get("subtype") == "init":
            return [t if isinstance(t, str) else t.get("name", "") for t in m.get("tools", [])]
    return None


def json_fields(out):
    shape, msgs = messages(out)
    if shape in ("empty", "text"):
        return {"shape": shape}
    res = msgs[0] if shape == "object" else next(
        (m for m in reversed(msgs) if isinstance(m, dict) and m.get("type") == "result"), None)
    if not isinstance(res, dict):
        return {"shape": shape, "keys": []}
    pick = {k: res[k] for k in ("type", "subtype", "is_error", "num_turns", "total_cost_usd", "duration_ms",
                                "stop_reason") if k in res}
    return {"shape": shape, "keys": sorted(res), "fields": pick, "result_head": str(res.get("result", ""))[:300]}


def records(rdir, pattern):
    return [json.loads(p.read_text()) for p in sorted(rdir.glob(pattern))]


def run_variant(v, ctx, rdir, args):
    vid = v["id"]
    nonce = "STDIN-" + secrets.token_hex(3)
    rid = None
    if v["prompt"] == "optimize":
        if not ctx["requests"]:
            return {"id": vid, "preset": v["preset"], "purpose": v["purpose"], "exit": "skipped: нет запросов"}
        rid = ctx["requests"].pop(0)
    prompt = INVENTORY if v["prompt"] == "inventory" else OPTIMIZE.format(rid=rid, nonce=nonce)
    cfg = rdir / f"{vid}.mcp.json"
    write_private(cfg, json.dumps(mcp_config(v["preset"], rdir / f"{vid}.probe", nonce, v.get("trust", True)), indent=1))
    cmd, env = command(v, ctx, cfg, args.model)
    if v.get("debug"):
        cmd += ["--debug-file", str(rdir / f"{vid}.debug.log")]
    temp, marker = [], None
    if v["preset"] == "qwen":
        home, marker = qwen_home(rdir, vid, v.get("canary", False))
        env["HOME"] = str(home)
        temp.append(home)
    elif v.get("canary"):
        home, marker = claude_home(rdir, vid)
        env["CLAUDE_CONFIG_DIR"] = str(home / ".claude")
        temp.append(home)
    work = Path(tempfile.mkdtemp(prefix="agent-check-work-"))  # пустая рабочая папка, как agent-work/ в Glafira
    temp.append(work)
    since, t0 = int(time.time() * 1000), time.time()
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd=work,
                         env=env, start_new_session=True)
    try:
        out, err = p.communicate(prompt, timeout=args.timeout)
        code = p.returncode
    except subprocess.TimeoutExpired:
        stop_group(p.pid)
        out, err = p.communicate()
        code = "timeout"
    seconds = round(time.time() - t0, 1)
    time.sleep(1)  # шлюз пишет журнал после ответа
    for d in temp:
        shutil.rmtree(d, ignore_errors=True)
    (rdir / f"{vid}.stdout").write_text(out or "")
    (rdir / f"{vid}.stderr").write_text(err or "")
    tools = init_tools(out or "")
    calls = audit_calls(since)
    canary = None
    if v.get("canary"):
        canary = {"md": CANARY[v["preset"]][2] in (out or ""), "hook": bool(marker and marker.exists())}
    stdin_ok = ("READY" in (out or "")) if v["prompt"] == "inventory" else (
        any(c["requestId"] == rid for c in calls) or nonce in (out or ""))
    r = {"id": vid, "preset": v["preset"], "purpose": v["purpose"], "cmd": cmd, "exit": code, "seconds": seconds,
         "request": rid, "calls": calls, "tools": tools,
         "glafira_tools": [t for t in tools or [] if is_glafira(t)],
         "builtin_tools": [t for t in tools or [] if not is_glafira(t)],
         "stdin_ok": stdin_ok, "probe": records(rdir, f"{vid}.probe.*.json"),
         "decoy": len(records(rdir, f"{vid}.decoy.*.json")), "json": json_fields(out or ""), "canary": canary,
         "stderr_tail": (err or "")[-1500:]}
    (rdir / f"{vid}.result.json").write_text(json.dumps(r, ensure_ascii=False, indent=1))
    return r


def learn(v, r, ctx):
    """Опись Qwen Code уточняет флаги следующих вариантов: встроенные инструменты, имена инструментов запуска, способ
    выключить встроенные."""
    tools = r.get("tools")
    if not tools:
        return
    if v["id"] == "q-inventory":
        ctx["qwen_builtins"] = r["builtin_tools"]
        names = [n for n in r["glafira_tools"] if base(n) in RUN_TOOLS]
        if len(names) == len(RUN_TOOLS):
            ctx["qwen_run_tools"] = names
    if v.get("core") and "qwen_core" not in ctx and not r["builtin_tools"] and len(r["glafira_tools"]) == len(TOOL_NAMES):
        ctx["qwen_core"] = v["core"]


def summary(rdir):
    rows = [json.loads(p.read_text()) for p in sorted(rdir.glob("*.result.json"), key=lambda p: p.stat().st_mtime)]
    lines = ["| Вариант | Код | с | Вызовы ok / ошибки | Коды ошибок | glafira / встроенные | stdin | env → шим | Приманка | "
             "JSON | Canary | Назначение |", "|" + "---|" * 12]
    for r in rows:
        calls = r.get("calls", [])
        bad = sorted({c["code"] for c in calls if c.get("code")})
        tools = "—" if r.get("tools") is None else f"{len(r['glafira_tools'])} / {len(r['builtin_tools'])}"
        probe = r.get("probe", [])
        env_ok = "—" if not probe else ("да" if all(p["nonce_ok"] and p["ticket_ok"] for p in probe) else "НЕТ")
        js = r.get("json") or {}
        jtxt = js.get("shape", "—") + (": " + ", ".join(sorted(js.get("fields", {}))) if js.get("fields") else "")
        can = "—" if not r.get("canary") else f"md {'да' if r['canary']['md'] else 'нет'}, хук {'да' if r['canary']['hook'] else 'нет'}"
        lines.append(f"| {r['id']} | {r.get('exit')} | {r.get('seconds', '—')} | "
                     f"{sum(1 for c in calls if not c.get('code'))} / {sum(1 for c in calls if c.get('code'))} | "
                     f"{', '.join(bad) or '—'} | {tools} | {'да' if r.get('stdin_ok') else 'нет'} | {env_ok} | "
                     f"{r.get('decoy', 0) or '—'} | {jtxt} | {can} | {r['purpose']} |")
    (rdir / "summary.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


def cmd_run(args):
    variants = [v for v in VARIANTS if (v["id"] in args.only if args.only else not v.get("canary") or args.canary)
                and (not v.get("cloud") or args.cloud)]
    unknown = set(args.only) - {v["id"] for v in VARIANTS}
    if unknown:
        sys.exit(f"Нет вариантов {sorted(unknown)}; есть: {', '.join(v['id'] for v in VARIANTS)}")
    try:
        loaded = loaded_models()
    except OSError as e:
        sys.exit(f"Ollama не отвечает на {OLLAMA}: {e}")
    others = [m for m in loaded if m != args.model]
    if others and not args.allow_evict:
        sys.exit(f"В Ollama загружены {others}: прогон загрузит {args.model}, и Ollama может их выгрузить. Спросите "
                 "владельца; с его согласия — --allow-evict.")
    was_loaded = args.model in loaded
    rdir = OUT / time.strftime("%Y%m%d-%H%M%S")
    rdir.mkdir(parents=True)
    os.chmod(rdir, 0o700)
    (rdir / "facts.json").write_text(json.dumps(facts(args.model), ensure_ascii=False, indent=1))
    host, ids = start_host(sum(1 for v in variants if v["prompt"] == "optimize"), rdir / "host.log")
    ctx = {"requests": ids}
    try:
        for v in variants:
            r = run_variant(v, ctx, rdir, args)
            learn(v, r, ctx)
            print(f"{v['id']}: exit {r['exit']}, {r.get('seconds', '—')} s, вызовов {len(r.get('calls', []))}", flush=True)
    finally:
        stop_host(host)
        if not was_loaded:
            try:
                ollama("/api/generate", {"model": args.model, "keep_alive": 0}, timeout=60)
            except OSError:
                pass
    (rdir / "context.json").write_text(json.dumps({k: v for k, v in ctx.items() if k != "requests"}, indent=1))
    summary(rdir)
    print(f"\nИтог: {rdir / 'summary.md'}")


def shim_probe(a):
    rec = {"pid": os.getpid(), "ppid": os.getppid(), "cwd": os.getcwd(), "ts": int(time.time() * 1000),
           "nonce_ok": os.environ.get("GLAFIRA_CHECK_NONCE") == a.nonce,
           "ticket_ok": os.environ.get("GLAFIRA_AGENT_TICKET") == TICKET,
           # Только имена: значения унаследованных переменных могут быть секретами.
           "env_names": sorted(os.environ)}
    write_private(f"{a.out}.{os.getpid()}.json", json.dumps(rec, indent=1))
    if not rec["nonce_ok"]:
        print("agent_check shim-probe: GLAFIRA_CHECK_NONCE from the MCP config did not reach the server", file=sys.stderr)
        return 1
    env = {k: v for k, v in os.environ.items() if k not in ("GLAFIRA_AGENT_TICKET", "GLAFIRA_CHECK_NONCE")}
    env["HOME"] = a.home
    os.execve(SHIM, [SHIM, "mcp"], env)


def decoy(a):
    write_private(f"{a.out}.{os.getpid()}.json", json.dumps({"pid": os.getpid(), "ts": int(time.time() * 1000)}))
    print("agent_check decoy: the glafira-pg server from the user settings was started instead of --mcp-config",
          file=sys.stderr)
    return 1


def main():
    p = argparse.ArgumentParser(description="Проверка агентских CLI для фонового ассистента P4c")
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="прогнать варианты")
    r.add_argument("--model", default="qwen3.6-64k:35b-a3b")
    r.add_argument("--only", type=lambda s: [x for x in s.split(",") if x], default=[], help="варианты через запятую")
    r.add_argument("--canary", action="store_true", help="варианты с QWEN.md / CLAUDE.md и хуком во временных настройках")
    r.add_argument("--cloud", action="store_true", help="Claude Code без Ollama: платный вызов, только по слову владельца")
    r.add_argument("--allow-evict", action="store_true", help="владелец разрешил вытеснить загруженные модели")
    r.add_argument("--timeout", type=int, default=900, help="предел одного варианта, с")
    s = sub.add_parser("summary", help="заново собрать summary.md прогона")
    s.add_argument("dir", type=Path)
    pr = sub.add_parser("shim-probe", help="служебный: MCP-сервер вариантов")
    pr.add_argument("--out", required=True)
    pr.add_argument("--nonce", required=True)
    pr.add_argument("--home", required=True)
    d = sub.add_parser("decoy", help="служебный: приманка в настройках Qwen Code")
    d.add_argument("--out", required=True)
    a = p.parse_args()
    if a.cmd == "run":
        cmd_run(a)
    elif a.cmd == "summary":
        summary(a.dir)
    elif a.cmd == "shim-probe":
        sys.exit(shim_probe(a))
    else:
        sys.exit(decoy(a))


if __name__ == "__main__":
    main()
