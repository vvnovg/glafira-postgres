"""Прогон настоящих клиентов (Qwen Code, Claude Code) с локальной моделью на тех же сценариях, что bench.py.

python -I client_bench.py <qwen|claude> <model> [--reps 3] [--scenarios s1,s2,s5] [--base-url http://127.0.0.1:11434]
Клиент запускается без окна (`qwen "<вопрос>"`, `claude -p`), сервер Glafira — шлюз llm_eval_host (как в bench.py),
встроенные инструменты оболочки, правки файлов и веб-запросов у клиента выключены. Вызовы инструментов берутся из
журнала MCP шлюза, ответ — из вывода клиента. Журналы — out/<client>-<sN>-r<R>.json в формате agent.py, сводка —
grade.py <client>.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bench  # noqa: E402

os.environ["PATH"] = f"{Path.home() / '.local/bin'}:/opt/homebrew/bin:{os.environ['PATH']}"
OUT = bench.OUT
SHIM = str(bench.ROOT / "src-tauri/target/debug/glafira-pg")
AUDIT = Path(bench.EVAL_HOME) / "Library/Application Support/io.github.vvnovg.glafira.postgres/mcp-audit.jsonl"
SERVER = "glafira-pg-eval"


def audit_calls(since_ms, client_prefix):
    calls = []
    if not AUDIT.exists():
        return calls
    for line in AUDIT.read_text().splitlines():
        r = json.loads(line)
        if r.get("event") == "call" and r["ts"] >= since_ms and (r.get("client") or "").startswith(client_prefix):
            calls.append({"name": r["tool"], "args": r.get("args"), "error": r.get("code"),
                          "result_chars": r.get("bytes"), "result_head": ""})
    return calls


def run_qwen(prompt, model, base_url, workdir):
    cfg = {"mcpServers": {SERVER: {"command": SHIM, "args": ["mcp"], "env": {"HOME": bench.EVAL_HOME},
                                   "timeout": 120000, "trust": True}}}
    cmd = ["qwen", prompt, "--auth-type", "openai", "--openai-base-url", f"{base_url}/v1", "--openai-api-key", "ollama",
           "-m", model, "--mcp-config", json.dumps(cfg), "--allowed-mcp-server-names", SERVER,
           "--exclude-tools", "run_shell_command", "edit", "write_file", "web_fetch", "web_search", "agent",
           "--approval-mode", "yolo", "--chat-recording", "false", "-o", "text"]
    return subprocess.run(cmd, cwd=workdir, capture_output=True, text=True, timeout=1500)


def run_claude(prompt, model, base_url, workdir):
    cfg = {"mcpServers": {SERVER: {"command": SHIM, "args": ["mcp"], "env": {"HOME": bench.EVAL_HOME}}}}
    env = {**os.environ, "ANTHROPIC_BASE_URL": base_url, "ANTHROPIC_AUTH_TOKEN": "ollama",
           "ANTHROPIC_MODEL": model, "ANTHROPIC_DEFAULT_OPUS_MODEL": model, "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
           "ANTHROPIC_DEFAULT_HAIKU_MODEL": model, "CLAUDE_CODE_SUBAGENT_MODEL": model,
           "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1"}
    env.pop("ANTHROPIC_API_KEY", None)
    cmd = ["claude", "-p", prompt, "--mcp-config", json.dumps(cfg), "--strict-mcp-config",
           "--allowedTools", f"mcp__{SERVER}", "--disallowedTools", "Bash Edit Write WebFetch WebSearch Task",
           "--output-format", "text"]
    return subprocess.run(cmd, cwd=workdir, capture_output=True, text=True, timeout=1500, env=env)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("client", choices=["qwen", "claude"])
    p.add_argument("model")
    p.add_argument("--reps", type=int, default=3)
    p.add_argument("--scenarios", default="s1,s2,s5")
    p.add_argument("--base-url", default="http://127.0.0.1:11434")
    args = p.parse_args()
    OUT.mkdir(exist_ok=True)
    runner = run_qwen if args.client == "qwen" else run_claude
    prefix = "qwen" if args.client == "qwen" else "claude"
    host = bench.start_host()
    workdir = tempfile.mkdtemp(prefix="glafira-eval-")
    started_load = False
    if any(s in args.scenarios for s in ("s4", "s5", "s6")) and not bench.perf_load_running():
        bench.docker_bg("gp-pg-perf", "pgbench", "-n", "-U", "postgres", "-d", "perf", "-f", "/perf/heavy.sql",
                        "-c", "1", "-T", "14400")
        started_load = True
    try:
        for r in range(1, args.reps + 1):
            for scn in args.scenarios.split(","):
                name = f"{args.client}-{scn}-r{r}"
                exp = bench.setup(scn)
                since = int(time.time() * 1000)
                t0 = time.time()
                answer, stopped = None, None
                try:
                    res = runner(bench.PROMPTS[scn], args.model, args.base_url, workdir)
                    answer = res.stdout.strip()
                    if res.returncode != 0:
                        stopped = f"exit {res.returncode}: {res.stderr.strip()[-300:]}"
                except subprocess.TimeoutExpired:
                    exp["timeout"] = True
                finally:
                    bench.cleanup(scn, exp)
                total = round(time.time() - t0, 1)
                time.sleep(1)
                log = {"prompt": bench.PROMPTS[scn], "model": args.model, "client": args.client, "answer": answer,
                       "stopped": stopped, "total_s": total,
                       "steps": [{"step": 0, "llm_s": total, "usage": None, "content": None,
                                  "calls": audit_calls(since, prefix)}]}
                (OUT / f"{name}.json").write_text(json.dumps(log, ensure_ascii=False, indent=1))
                (OUT / f"{name}.expect.json").write_text(json.dumps(exp, ensure_ascii=False))
                print(f"{name} done {time.strftime('%H:%M:%S')} {total}s", flush=True)
    finally:
        if started_load:
            subprocess.run(["docker", "exec", "gp-pg-perf", "sh", "-c", "kill $(pgrep -f 'pgbench -n') 2>/dev/null"])
        host.terminate()
        shutil.rmtree(workdir, ignore_errors=True)
    subprocess.run([sys.executable, "-I", str(bench.HERE / "grade.py"), args.client])


if __name__ == "__main__":
    main()
