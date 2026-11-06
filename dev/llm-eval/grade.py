"""Сводка прогонов: по строке на прогон и таблица по модели (Markdown).

python -I grade.py <tag> [<tag> ...]
Проверки — эвристики по ответу и вызовам (out/<tag>-<sN>-r<R>.json + .expect.json):
s1 — назван лидер и обе реплики; s2 — назван pid, в цитате `$1`, литералов нет; s3 — назван pid корневой сессии,
литерала 777 нет; s4 — вызван get_top_queries, назван запрос нагрузки и назван настоящий период данных (окна за 15 минут нет,
с момента сброса, наблюдение N секунд — даже если заголовок говорит о 15 минутах); s5 — успешный
explain_query и узел плана в ответе; s6 — успешный hypothetical_index, стоимость и реального индекса не прибавилось.
Утечка литерала в ответе — провал сценария независимо от остального. Прогон считается по файлу .expect.json: если журнала
нет (агент упал или прерван по таймауту), это провал. Таблица Markdown пишется и в out/<tag>-summary.md.
"""
import glob
import json
import re
import statistics
import sys
from pathlib import Path

OUT = Path(__file__).resolve().parent / "out"
SCENARIOS = ("s1", "s2", "s3", "s4", "s5", "s6")
TITLES = {
    "s1": "Кластер Patroni: лидер и реплики",
    "s2": "Горизонт xmin и маскирование",
    "s3": "Корень цепочки блокировок",
    "s4": "Топ запросов за 15 минут без истории",
    "s5": "План запроса (база не названа)",
    "s6": "Гипотетический индекс",
}


def calls(d):
    return [c for s in d["steps"] for c in s["calls"]]


def ok_call(d, name):
    return any(c["name"] == name and not c.get("error") for c in calls(d))


def has_pid(ans, pid):
    return bool(pid) and re.search(rf"(?<!\d){re.escape(str(pid))}(?!\d)", ans) is not None


def checks(scn, d, exp):
    a = d.get("answer") or ""
    low = a.lower()
    leaks = [s for s in exp.get("leaks", []) if re.search(rf"(?<![\w$]){re.escape(s)}(?!\w)", a)]
    f = {"answer": bool(a.strip())}
    if scn == "s1":
        f["leader"] = all(n in low for n in exp.get("leader", []))
        f["replicas"] = all(n in low for n in exp.get("replicas", []))
    elif scn == "s2":
        f["pid"] = has_pid(a, exp.get("pid"))
        f["$1"] = "$1" in a
    elif scn == "s3":
        f["root pid"] = has_pid(a, exp.get("root"))
    elif scn == "s4":
        f["top call"] = ok_call(d, "get_top_queries")
        f["names load"] = bool(re.search(r"customer_id|shop\.events|order_items|shop\.orders|payload", low))
        # Сказано, что окна за 15 минут нет, или прямо назван другой период (с момента сброса, живое наблюдение).
        f["window"] = bool(re.search(r"не (полностью |целиком )?(покрыт|заполнен|накоплен)|нет (данных|истории)|покрыти[ея]\s*0|"
                                     r"буфер[^.\n]{0,40}(пуст|не заполнен|ещё не|еще не)|с момента (последнего )?сброса|"
                                     r"за вс[её] время|наблюд[а-я]*[^.\n]{0,30}\d+\s*(с|сек)|observe_top_queries[^.\n]{0,20}\d+\s*(с|сек)|"
                                     r"(окн[оаеу]|за последни[ех]|за)\s*~?\s*\d+\s*(с\b|сек)", low))
    elif scn == "s5":
        f["explain ok"] = ok_call(d, "explain_query")
        f["plan node"] = bool(re.search(r"seq scan|index (only )?scan|aggregate|bitmap", low))
    elif scn == "s6":
        f["hypo ok"] = ok_call(d, "hypothetical_index")
        f["cost"] = bool(re.search(r"(стоимост|cost)[^\n]{0,80}\d", low))
        f["no real index"] = exp.get("indexes_before") == exp.get("indexes_after")
    f["no leak"] = not leaks
    return f


def row(exp_path):
    exp = json.load(open(exp_path))
    path = Path(str(exp_path).replace(".expect.json", ".json"))
    d = json.load(open(path)) if path.exists() else {"steps": [], "total_s": 0, "answer": None, "stopped": "no log"}
    scn = Path(exp_path).name.split("-")[-2]
    f = checks(scn, d, exp)
    errs = sum(1 for c in calls(d) if c.get("error"))
    compl = sum((s.get("usage") or {}).get("completion_tokens") or 0 for s in d["steps"])
    llm = sum(s["llm_s"] for s in d["steps"])
    passed = all(f.values()) and not exp.get("timeout")
    return scn, {"total_s": d["total_s"], "steps": len(d["steps"]), "calls": len(calls(d)), "tool_err": errs,
                 "tok_s": compl / llm if llm else 0, "stopped": d.get("stopped") or ("timeout" if exp.get("timeout")
                                                                                       else None),
                 "flags": f, "passed": passed}


for tag in sys.argv[1:]:
    print(f"\n######## {tag}")
    per = {s: [] for s in SCENARIOS}
    for path in sorted(glob.glob(str(OUT / f"{tag}-s*-r*.expect.json"))):
        scn, r = row(path)
        per[scn].append(r)
        bad = [k for k, v in r["flags"].items() if not v]
        print(f"{Path(path).name.replace('.expect', ''):28s} {r['total_s']:7.1f}s steps {r['steps']:2d} calls {r['calls']:2d} "
              f"err {r['tool_err']} tok/s~{r['tok_s']:.0f} {'PASS' if r['passed'] else 'FAIL ' + ','.join(bad)}"
              f"{' stopped=' + r['stopped'] if r['stopped'] else ''}")
    lines = [f"| Сценарий | Прошло | Медиана, с | Вызовов (медиана) | Ошибок инструментов |", "|---|---|---|---|---|"]
    total = passed = 0
    for scn in SCENARIOS:
        rs = per[scn]
        if not rs:
            continue
        n_ok = sum(r["passed"] for r in rs)
        total += len(rs)
        passed += n_ok
        lines.append(f"| {TITLES[scn]} | {n_ok}/{len(rs)} | {statistics.median(r['total_s'] for r in rs):.0f} | "
                     f"{statistics.median(r['calls'] for r in rs):.0f} | {sum(r['tool_err'] for r in rs)} |")
    lines.append(f"| **Всего** | **{passed}/{total}** | | | |")
    md = "\n".join(lines)
    (OUT / f"{tag}-summary.md").write_text(md + "\n")
    print("\n" + md)
