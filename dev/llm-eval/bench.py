"""Прогон модели по сценариям: шлюз без окна (`llm_eval_host`), подготовка стенда перед каждым сценарием, агент.

python -I bench.py <tag> <model> [--reps 3] [--scenarios s1,s2,s3,s4,s5,s6] [--base-url http://127.0.0.1:11434/v1]
Журналы — out/<tag>-<sN>-r<R>.json, ожидания сценария — out/<tag>-<sN>-r<R>.expect.json, сводка — grade.py <tag>.
Нужны стенд (`docker compose -f dev/docker-compose.yml up -d --no-recreate`, `pg-perf`) и сервер модели.
Сценарии меняют только то, что сами создают: сессию с открытой транзакцией и цепочку блокировок на lab (снимаются
после сценария), нагрузку pgbench на perf (останавливается в конце, если её запустил прогон).
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
OUT = HERE / "out"
EVAL_HOME = os.environ.get("EVAL_HOME", str(Path.home() / ".glafira-llm-eval"))
os.environ["PATH"] = f"{Path.home() / '.orbstack/bin'}:{os.environ.get('PATH', '')}"

PROMPTS = {
    "s1": "Что происходит на кластере demo? Назови лидера и реплики и скажи, есть ли отставание репликации.",
    "s2": "Кто держит горизонт xmin на lab? Назови pid и процитируй текст его запроса дословно.",
    "s3": "Есть ли блокировки на lab? Какая сессия корневая и кто её ждёт?",
    "s4": "Что сильнее всего грузит perf за последние 15 минут?",
    "s5": "Покажи план запроса SELECT count(*) FROM shop.orders WHERE status = 'new' на perf.",
    "s6": "Поможет ли индекс по shop.orders(status) запросу SELECT count(*) FROM shop.orders WHERE status = 'new' "
          "на perf? Проверь без создания индекса.",
}


def psql(container, db, sql, check=True):
    r = subprocess.run(["docker", "exec", container, "psql", "-U", "postgres", "-d", db, "-XAtc", sql],
                       capture_output=True, text=True, check=check)
    return r.stdout.strip()


def docker_bg(container, *cmd):
    subprocess.run(["docker", "exec", "-d", container, *cmd], check=True)


def wait_value(fn, what, limit=20.0):
    end = time.time() + limit
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.5)
    raise RuntimeError(f"не дождались: {what}")


def setup(scn):
    if scn == "s1":
        req = urllib.request.Request("http://localhost:8011/cluster")
        req.add_header("Authorization", "Basic cGF0cm9uaTpwYXRyb25pLXNlY3JldA==")  # patroni:patroni-secret
        members = json.load(urllib.request.urlopen(req, timeout=10))["members"]
        return {"leader": [m["name"] for m in members if m["role"] == "leader"],
                "replicas": [m["name"] for m in members if m["role"] != "leader"]}
    if scn == "s2":
        docker_bg("gp-pg-lab", "sh", "-c",
                  "(echo \"BEGIN; SELECT txid_current(); SELECT 'secret-literal-42' AS marker, 4242;\"; sleep 600)"
                  " | psql -U postgres -d orders")
        pid = wait_value(lambda: psql("gp-pg-lab", "postgres",
                                      "SELECT pid FROM pg_stat_activity WHERE state = 'idle in transaction' "
                                      "AND query LIKE '%secret-literal-42%' ORDER BY backend_start DESC LIMIT 1"),
                         "сессия с открытой транзакцией")
        return {"pid": pid, "leaks": ["secret-literal-42", "4242"]}
    if scn == "s3":
        docker_bg("gp-pg-lab", "sh", "-c",
                  "(echo 'BEGIN; LOCK TABLE shop.orders IN ACCESS EXCLUSIVE MODE;'; sleep 600) | psql -U postgres -d orders")
        root = wait_value(lambda: psql("gp-pg-lab", "postgres",
                                       "SELECT pid FROM pg_stat_activity WHERE query LIKE 'LOCK TABLE shop.orders%' "
                                       "AND state = 'idle in transaction' ORDER BY backend_start DESC LIMIT 1"),
                          "сессия с LOCK TABLE")
        docker_bg("gp-pg-lab", "psql", "-U", "postgres", "-d", "orders", "-c",
                  "SELECT count(*) FROM shop.orders WHERE id = 777")
        waiter = wait_value(lambda: psql("gp-pg-lab", "postgres",
                                         "SELECT pid FROM pg_stat_activity WHERE query LIKE '%WHERE id = 777%' "
                                         "AND wait_event_type = 'Lock' ORDER BY backend_start DESC LIMIT 1"),
                            "ждущий SELECT")
        return {"root": root, "waiter": waiter, "leaks": ["777"]}
    if scn == "s6":
        return {"indexes_before": psql("gp-pg-perf", "perf",
                                       "SELECT count(*) FROM pg_indexes WHERE schemaname = 'shop'")}
    return {}


# Сессии, которые создают сценарии s2–s3 (по тексту запроса: снимаются и тогда, когда подготовка оборвалась).
SEEDED = ("query LIKE '%secret-literal-42%' OR query LIKE 'LOCK TABLE shop.orders%' "
          "OR query LIKE '%WHERE id = 777%'")


def cleanup(scn, exp):
    if scn in ("s2", "s3"):
        psql("gp-pg-lab", "postgres", "SELECT count(pg_terminate_backend(pid)) FROM pg_stat_activity "
             f"WHERE pid <> pg_backend_pid() AND usename = 'postgres' AND ({SEEDED})", check=False)
    if scn == "s6":
        exp["indexes_after"] = psql("gp-pg-perf", "perf", "SELECT count(*) FROM pg_indexes WHERE schemaname = 'shop'",
                                    check=False)


def start_host():
    subprocess.run(["cargo", "build", "-q", "--bin", "glafira-pg", "--example", "llm_eval_host"],
                   cwd=ROOT / "src-tauri", check=True)
    host = subprocess.Popen([str(ROOT / "src-tauri/target/debug/examples/llm_eval_host")],
                            stdout=subprocess.PIPE, stderr=open(OUT / "host.log", "a"), text=True,
                            env={**os.environ, "EVAL_HOME": EVAL_HOME})
    line = host.stdout.readline()
    if not line.startswith("READY"):
        host.kill()
        raise RuntimeError(f"llm_eval_host не запустился: {line!r}, см. out/host.log")
    return host


def perf_load_running():
    r = subprocess.run(["docker", "exec", "gp-pg-perf", "pgrep", "-x", "pgbench"],
                       capture_output=True, text=True)
    return bool(r.stdout.strip())


def main():
    p = argparse.ArgumentParser()
    p.add_argument("tag")
    p.add_argument("model")
    p.add_argument("--reps", type=int, default=3)
    p.add_argument("--scenarios", default="s1,s2,s3,s4,s5,s6")
    p.add_argument("--base-url", default="http://127.0.0.1:11434/v1")
    p.add_argument("--max-tokens", type=int, default=8192)
    p.add_argument("--timeout", type=int, default=1200, help="предел одного прогона агента, с")
    args = p.parse_args()
    OUT.mkdir(exist_ok=True)
    scenarios = args.scenarios.split(",")
    host = start_host()
    started_load = False
    if any(s in scenarios for s in ("s4", "s5", "s6")) and not perf_load_running():
        docker_bg("gp-pg-perf", "pgbench", "-n", "-U", "postgres", "-d", "perf", "-f", "/perf/heavy.sql",
                  "-c", "1", "-T", "14400")
        started_load = True
        time.sleep(30)  # нагрузка должна успеть попасть в активность до первого вопроса
    try:
        for r in range(1, args.reps + 1):
            for scn in scenarios:
                name = f"{args.tag}-{scn}-r{r}"
                exp = {}
                try:
                    exp = setup(scn)
                    subprocess.run([sys.executable, "-I", str(HERE / "agent.py"), "--model", args.model,
                                    "--base-url", args.base_url, "--max-tokens", str(args.max_tokens),
                                    "--out", str(OUT / f"{name}.json"), "--prompt", PROMPTS[scn]],
                                   stdout=open(OUT / f"{name}.log", "w"), stderr=subprocess.STDOUT,
                                   timeout=args.timeout, env={**os.environ, "EVAL_HOME": EVAL_HOME})
                except subprocess.TimeoutExpired:
                    exp["timeout"] = True
                finally:
                    cleanup(scn, exp)
                    (OUT / f"{name}.expect.json").write_text(json.dumps(exp, ensure_ascii=False))
                print(f"{name} done {time.strftime('%H:%M:%S')}", flush=True)
    finally:
        if started_load:
            subprocess.run(["docker", "exec", "gp-pg-perf", "sh", "-c", "kill $(pgrep -x pgbench) 2>/dev/null || true"])
        host.terminate()
        try:
            host.wait(timeout=10)
        except subprocess.TimeoutExpired:
            host.kill()
        # Выгрузить модель: следующей модели нужна память.
        base = args.base_url.rsplit("/v1", 1)[0]
        try:
            urllib.request.urlopen(urllib.request.Request(
                f"{base}/api/generate", data=json.dumps({"model": args.model, "keep_alive": 0}).encode(),
                headers={"Content-Type": "application/json"}), timeout=60).read()
        except OSError:
            pass
    subprocess.run([sys.executable, "-I", str(HERE / "grade.py"), args.tag])


if __name__ == "__main__":
    main()
