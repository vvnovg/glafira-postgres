import { afterEach, describe, expect, it } from "vitest";
import type { Recommendation } from "../../../api/types";
import { i18n } from "../../../i18n";
import { recommendEffect, recommendEvidence, recommendParams, recommendRisks, recommendTitle } from "./recommendText";

const rec = (id: string, params: Recommendation["params"], patch: Partial<Recommendation> = {}): Recommendation => ({ id, severity: "warning", nodeIds: [1], params, command: null, hypo: null, ...patch });

/** Все правила `pg_advisor::plan_rules` с параметрами, как их отдаёт Rust (включая варианты). */
const ALL: Recommendation[] = [
  rec("plan_seq_scan_filter", { table: "shop.orders", rowsRead: 500_000, rowsReturned: 500, loops: 1, columns: "status" }),
  rec("plan_seq_scan_filter", { partitions: 24, table: "shop.events_p1", tables: "shop.events_p1, shop.events_p2", rowsRead: 900_000, rowsReturned: 10, loops: 1 }, { nodeIds: [3, 4] }),
  rec("plan_index_filter", { table: "shop.orders", index: "orders_customer_idx", rowsRemoved: 90_000, rowsReturned: 12, loops: 1, columns: "customer_id, status" }),
  rec("plan_nested_loop_seq", { table: "shop.items", loops: 2000, rowsPerLoop: 5000 }, { nodeIds: [1, 2] }),
  rec("plan_misestimate", { table: "shop.orders", planRows: 100, actualRows: 120_000, ratio: 1200, direction: "under" }),
  rec("plan_spill", { kind: "sort", spaceKb: 20_480, workMemKb: 40_960, keys: "o.created_at" }),
  rec("plan_spill", { kind: "hash", spaceKb: 2048, workMemKb: 4096, keys: "" }),
  rec("plan_spill", { kind: "bitmap", lossyBlocks: 1200, exactBlocks: 300 }),
  rec("plan_sort_limit", { inputRows: 500_001, limitRows: 10, keys: "o.created_at DESC", table: "shop.orders", columns: "created_at" }, { severity: "info" }),
  rec("plan_heap_fetches", { table: "shop.balances", heapFetches: 32_074, rows: 19_001 }, { severity: "info" }),
  rec("plan_no_pruning", { children: 24, emptyChildren: 23 }),
  rec("plan_no_pruning", { children: 24, emptyChildren: 0 }, { severity: "info" }),
  rec("plan_workers_short", { planned: 4, launched: 2 }, { severity: "info" }),
  rec("plan_subplan_per_row", { loops: 3000, name: "SubPlan 1" }),
  rec("plan_jit_heavy", { jitMs: 121.8, execMs: 60.2, processes: 1, functions: 14 }, { severity: "info" }),
  rec("plan_jit_heavy", { jitMs: 121.8, execMs: 50.1, processes: 2, functions: 14 }, { severity: "info" }),
];

describe("recommendText", () => {
  afterEach(() => i18n.changeLanguage("ru"));

  it.each(["en", "ru"])("%s: у каждого правила все четыре текста, параметры подставлены", async (lang) => {
    await i18n.changeLanguage(lang);
    for (const r of ALL) {
      for (const fn of [recommendTitle, recommendEvidence, recommendEffect, recommendRisks]) {
        const s = fn(r);
        expect(s, `${r.id}`).not.toMatch(/\{\{|recommend\.|undefined/);
        expect(s.trim(), `${r.id}`).not.toBe("");
      }
    }
  });

  it("варианты: партиции, bitmap, хеш без ключей, без ANALYZE, параллельный JIT", () => {
    const [, grouped, , , mis, sort, hash, bitmap, , , pruned, pruneNoActual, , , , jitPar] = ALL;
    expect(recommendTitle(grouped)).toBe("Индекс по колонкам фильтра в партициях (24): shop.events_p1 и другие");
    expect(recommendEvidence(grouped)).toMatch(/shop\.events_p1, shop\.events_p2: прочитано строк 900\s000/);
    expect(recommendTitle(mis)).toBe("Оценка строк занижена в 1 200 раз: shop.orders");
    expect(recommendTitle(sort)).toBe("Сортировка уходит на диск");
    expect(recommendEvidence(sort)).toBe("На диске 20,0 MB; work_mem для сессии или роли: 40,0 MB");
    expect(recommendEffect(sort)).toMatch(/индекс по o\.created_at/);
    expect(recommendEffect(hash)).toBe("Достаточный work_mem держит операцию в памяти");
    // Ключ сортировки — выражение: бэкенд не отдаёт `keys`, эффект без индекса.
    expect(recommendEffect(rec("plan_spill", { kind: "sort", spaceKb: 4520, workMemKb: 6780 }))).toBe("Достаточный work_mem держит операцию в памяти");
    expect(recommendTitle(bitmap)).toBe("Битовая карта стала неточной: мало work_mem");
    expect(recommendEvidence(bitmap)).toMatch(/неточных 1\s200, точных 300/);
    expect(recommendEvidence(pruned)).toBe("Просканировано партиций 24, из них ничего не вернули 23");
    expect(recommendEvidence(pruneNoActual)).toBe("План читает все партиции: 24");
    expect(recommendEvidence(jitPar)).toMatch(/в процессах \(2; функций: 14\)/);
  });

  it("параметры: килобайты как байты, ratio с одним знаком до 10, null пропущен", () => {
    const p = recommendParams(rec("plan_misestimate", { ratio: 9.5, spaceKb: 1024, table: "t", missing: null, direction: "over" }));
    expect(p.ratio).toBe("9,5");
    expect(p.spaceKb).toBe("1,0 MB");
    expect(p.directionLabel).toBe("завышена");
    expect("missing" in p).toBe(false);
  });
});
