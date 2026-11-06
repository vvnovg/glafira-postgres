"""Минимальный MCP-агент: модель за OpenAI-совместимым API (Ollama, vLLM…) + шим `glafira-pg mcp` по stdio.

python -I agent.py --model <id> --prompt "..." --out run.json [--base-url http://127.0.0.1:11434/v1]
Шим — GLAFIRA (по умолчанию отладочная сборка `src-tauri/target/debug/glafira-pg`: она не запускает приложение);
каталог данных шима — HOME=EVAL_HOME, где работает `cargo run --example llm_eval_host`.
"""
import argparse
import asyncio
import json
import os
import time
from pathlib import Path

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from openai import BadRequestError, OpenAI

ROOT = Path(__file__).resolve().parents[2]
GLAFIRA = os.environ.get("GLAFIRA", str(ROOT / "src-tauri/target/debug/glafira-pg"))
EVAL_HOME = os.environ.get("EVAL_HOME", str(Path.home() / ".glafira-llm-eval"))
SYSTEM = (
    "Ты помощник инженера по PostgreSQL. У тебя есть инструменты Glafira для чтения кластеров и баз. "
    "Сначала собери факты инструментами, потом ответь по-русски: что нашёл, почему это важно, что сделать. "
    "Не выдумывай данные, которых не получил из инструментов."
)


def tool_text(result) -> str:
    parts = []
    for c in result.content:
        parts.append(getattr(c, "text", None) or json.dumps(c.model_dump(), ensure_ascii=False))
    text = "\n".join(parts)
    return ("ERROR: " + text) if result.is_error else text


async def run(args):
    client = OpenAI(base_url=args.base_url, api_key=args.api_key)
    log = {"prompt": args.prompt, "model": args.model, "steps": [], "started": time.time()}
    params = StdioServerParameters(command=GLAFIRA, args=["mcp"], env={"HOME": EVAL_HOME})
    async with stdio_client(params) as (r, w), ClientSession(r, w) as session:
        init = await session.initialize()
        system = SYSTEM + ("\n\n" + init.instructions if init.instructions else "")
        tools = (await session.list_tools()).tools
        oa_tools = [
            {"type": "function", "function": {"name": t.name, "description": t.description or "",
                                              "parameters": t.input_schema}}
            for t in tools
        ]
        log["tools"] = [t.name for t in tools]
        messages = [{"role": "system", "content": system}, {"role": "user", "content": args.prompt}]
        for step in range(args.max_steps):
            t0 = time.time()
            try:
                resp = client.chat.completions.create(
                    model=args.model, messages=messages, tools=oa_tools, temperature=0.2, max_tokens=args.max_tokens)
            except BadRequestError as e:
                # Переполнение контекста после крупного ответа инструмента — исход прогона, а не сбой агента.
                log["answer"] = None
                log["stopped"] = f"bad_request: {e}"[:500]
                break
            dt = time.time() - t0
            msg = resp.choices[0].message
            usage = resp.usage.model_dump() if resp.usage else None
            entry = {"step": step, "llm_s": round(dt, 1), "usage": usage, "content": msg.content, "calls": []}
            messages.append(msg.model_dump(exclude_none=True))
            if not msg.tool_calls:
                log["steps"].append(entry)
                log["answer"] = msg.content
                break
            for call in msg.tool_calls:
                try:
                    call_args = json.loads(call.function.arguments or "{}")
                except json.JSONDecodeError as e:
                    out = f"ERROR: arguments are not valid JSON: {e}"
                    call_args = call.function.arguments
                else:
                    t1 = time.time()
                    out = tool_text(await session.call_tool(call.function.name, call_args))
                    entry["tool_s"] = round(entry.get("tool_s", 0) + time.time() - t1, 1)
                code = out[7:].split(":", 1)[0] if out.startswith("ERROR: ") else None
                entry["calls"].append({"name": call.function.name, "args": call_args, "error": code,
                                       "result_chars": len(out), "result_head": out[:600]})
                messages.append({"role": "tool", "tool_call_id": call.id, "content": out})
            log["steps"].append(entry)
            print(f"step {step}: {dt:.1f}s {[c['name'] for c in entry['calls']]}", flush=True)
        else:
            log["answer"] = None
            log["stopped"] = "max_steps"
    log["total_s"] = round(time.time() - log["started"], 1)
    with open(args.out, "w") as f:
        json.dump(log, f, ensure_ascii=False, indent=1)
    print("ANSWER:\n", log.get("answer"))


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--prompt", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--base-url", default="http://127.0.0.1:11434/v1")
    p.add_argument("--api-key", default="ollama")
    p.add_argument("--model", required=True)
    p.add_argument("--max-steps", type=int, default=15)
    p.add_argument("--max-tokens", type=int, default=8192)
    asyncio.run(run(p.parse_args()))
