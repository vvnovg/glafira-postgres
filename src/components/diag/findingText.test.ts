import { describe, expect, it } from "vitest";
import type { Finding } from "../../api/types";
import { DICTIONARIES, i18n } from "../../i18n";
import { formatCount } from "../../lib/format";
import { findingFix, findingParams, findingTitle } from "./findingText";

const f = (patch: Partial<Finding>): Finding => ({ id: "idleInTransaction", severity: "warning", member: "pg-1", target: null, params: {}, links: [], ...patch });

/** Все id находок (`findings.<id>.title|fix` в словарях). */
const IDS = [
  "connectionsSaturated", "idleInTransaction", "longQuery", "longTransaction", "xidAge", "lockWait", "walDirSize", "deadlocks", "lockShare",
  "rootBlocker", "ddlQueue", "xminHorizon", "replicaLag", "replicaNotStreaming", "replicaReplyStale", "syncStandbyMissing", "walReceiverDown",
  "replayPaused", "recoveryConflicts", "recoveryConflictsTotal", "slotRetainsWal", "slotLost", "subscriptionErrors", "failoverCandidates",
  "clusterPaused", "syncModeWithoutSync", "patroniTiming", "healthPrimary", "healthUnhealthy", "replicaOutOfPool", "cacheHitLow", "tempFiles",
  "noIdleTimeout", "hiddenQueries", "lockCycle",
];

const GIB = 1024 ** 3;
const session = { kind: "session", pid: 4242, backendStartUs: 1_700_000_000_000_000 } as const;
const slot = { kind: "slot", name: "old_slot" } as const;
const node = { kind: "member", name: "pg-2" } as const;
const CLUSTER = { member: null } as const;

/**
 * Находки с полным набором параметров — как их выдаёт Rust (`crates/pg-advisor/src/rules/*`, `failover.rs`).
 * Где форма зависит от данных (имя или oid отношения, вид держателя горизонта, лаг в байтах и/или секундах), — по варианту на форму.
 */
const EMITTED: (Pick<Finding, "id"> & Partial<Finding>)[] = [
  { id: "connectionsSaturated", params: { usedPct: 91.5, used: 183, available: 200 } },
  { id: "idleInTransaction", target: session, params: { maxS: 2000, count: 3 } },
  { id: "longQuery", target: session, params: { maxS: 1000, count: 2 } },
  { id: "longTransaction", target: session, params: { maxS: 7200, count: 1 } },
  { id: "xidAge", params: { agePct: 62.5, maxXidAge: 1_342_177_280 } },
  { id: "lockWait", params: { maxS: 75, count: 4 } },
  { id: "walDirSize", params: { walDirBytes: 3 * GIB, maxWalSizeBytes: GIB } },
  { id: "deadlocks", params: { count: 2 } },
  { id: "lockShare", params: { sharePct: 35.5 } },
  { id: "rootBlocker", target: session, params: { count: 4, maxS: 75 } },
  { id: "ddlQueue", target: session, params: { relation: "shop.orders", count: 3, maxS: 12 } },
  { id: "ddlQueue", target: session, params: { relationOid: 16384, relationDatabase: "orders", count: 3, maxS: 12 } },
  { id: "xminHorizon", target: session, params: { kind: "session", name: "4242", xminAge: 12_345_678, maxS: 4000 } },
  { id: "xminHorizon", target: slot, params: { kind: "slot", name: "old_slot", xminAge: 12_345_678, slotType: "logical", active: false } },
  { id: "xminHorizon", params: { kind: "preparedXact", name: "gid-1", xminAge: 12_345_678, maxS: 4000, command: "ROLLBACK PREPARED 'gid-1';" } },
  { id: "xminHorizon", target: node, params: { kind: "standbyFeedback", name: "pg-2", xminAge: 12_345_678, slotType: "physical", active: true } },
  { id: "replicaLag", target: node, params: { count: 2, lagBytes: 20 * 1024 * 1024, delayS: 40 } },
  { id: "replicaLag", params: { lagBytes: 20 * 1024 * 1024 } },
  { id: "replicaLag", params: { delayS: 40 } },
  { id: "replicaNotStreaming", target: node, params: { count: 1, state: "catchup" } },
  { id: "replicaReplyStale", params: { count: 1, maxS: 90 } },
  { id: "syncStandbyMissing" },
  { id: "walReceiverDown" },
  { id: "replayPaused" },
  { id: "recoveryConflicts", params: { count: 5 } },
  { id: "recoveryConflictsTotal", params: { count: 17 } },
  { id: "slotRetainsWal", target: slot, params: { count: 2, retainedBytes: 5 * GIB, walStatus: "extended" } },
  { id: "slotLost", target: slot, params: { count: 1 } },
  { id: "subscriptionErrors", params: { count: 3 } },
  { id: "failoverCandidates", ...CLUSTER, params: { count: 0 } },
  { id: "failoverCandidates", ...CLUSTER, params: { count: 1 } },
  { id: "clusterPaused", ...CLUSTER },
  { id: "syncModeWithoutSync", ...CLUSTER },
  { id: "patroniTiming", ...CLUSTER, params: { ttlS: 20, loopWaitS: 10, retryTimeoutS: 10 } },
  { id: "healthPrimary", ...CLUSTER, params: { count: 2 } },
  { id: "healthUnhealthy" },
  { id: "replicaOutOfPool" },
  { id: "cacheHitLow", params: { hitPct: 85.5 } },
  { id: "tempFiles", params: { count: 12 } },
  { id: "noIdleTimeout", params: { command: "ALTER ROLE \"app\" SET idle_in_transaction_session_timeout = '10min';" } },
  { id: "hiddenQueries", params: { count: 7 } },
  { id: "lockCycle", params: { count: 2 } },
];

describe("тексты находок", () => {
  it("параметры форматируются по суффиксу имени", async () => {
    await i18n.changeLanguage("ru");
    const p = findingParams(f({ params: { maxS: 3725, retainedBytes: 2048, usedPct: 96.5, count: 1200, state: "catchup" }, target: { kind: "slot", name: "dead" } }));
    // Разделитель разрядов в ru — неразрывный пробел: ожидание строится той же функцией.
    expect(p).toMatchObject({ maxS: "1 ч 2 мин", retainedBytes: "2,0 KB", usedPct: "96,5 %", count: formatCount(1200), state: "catchup", name: "dead", member: "pg-1" });
  });

  it("лаг собирается из байт и времени, вид держателя горизонта переводится", async () => {
    await i18n.changeLanguage("ru");
    expect(findingParams(f({ id: "replicaLag", params: { lagBytes: 1024 * 1024 * 20, delayS: 40 } })).lag).toBe("20,0 MB, 40 с");
    expect(findingParams(f({ id: "xminHorizon", params: { kind: "slot", name: "s", xminAge: 5 } })).kindLabel).toBe("слот");
    expect(findingTitle(f({ id: "deadlocks", params: { count: 2 } }))).toBe("Взаимных блокировок за интервал: 2");
  });

  it("лаг только в байтах или только во времени не оставляет запятых; без обоих параметров лага нет", async () => {
    await i18n.changeLanguage("ru");
    expect(findingParams(f({ id: "replicaLag", params: { lagBytes: 2048 } })).lag).toBe("2,0 KB");
    expect(findingParams(f({ id: "replicaLag", params: { delayS: 40 } })).lag).toBe("40 с");
    expect(findingParams(f({ id: "replicaLag", params: { count: 1 } })).lag).toBeUndefined();
  });

  it("oid показывается как есть, без разделителей разрядов; процент — с одним знаком", async () => {
    await i18n.changeLanguage("ru");
    const p = findingParams(f({ id: "ddlQueue", params: { relationOid: 1234567, count: 12345, agePct: 50 } }));
    expect(p.relationOid).toBe("1234567");
    expect(p.count).toBe(formatCount(12345));
    expect(p.agePct).toBe("50 %");
  });

  it("отношение другой базы: без имени relation собирается из oid и БД", async () => {
    await i18n.changeLanguage("ru");
    const other = findingParams(f({ id: "ddlQueue", params: { relationOid: 16384, relationDatabase: "orders", count: 3 } }));
    expect(other.relation).toBe("oid 16384 в БД orders");
    expect(findingTitle(f({ id: "ddlQueue", params: { relationOid: 16384, relationDatabase: "orders", count: 3 } }))).toBe(
      `DDL на oid 16384 в БД orders стоит в очереди и блокирует сессий: 3`,
    );
    await i18n.changeLanguage("en");
    expect(findingParams(f({ id: "ddlQueue", params: { relationOid: 16384, relationDatabase: "orders" } })).relation).toBe("oid 16384 in database orders");
    await i18n.changeLanguage("ru");
    // Имя отношения есть — оно и показывается; одного oid без БД для подстановки мало.
    expect(findingParams(f({ id: "ddlQueue", params: { relation: "public.t", relationOid: 1, relationDatabase: "orders" } })).relation).toBe("public.t");
    expect(findingParams(f({ id: "ddlQueue", params: { relationOid: 1 } })).relation).toBeUndefined();
  });

  it("объект находки: pid сессии, имя цели; имя из параметров не затирается", async () => {
    await i18n.changeLanguage("ru");
    const s = findingParams(f({ id: "rootBlocker", params: { count: 4, maxS: 75 }, target: { kind: "session", pid: 4242, backendStartUs: 1 } }));
    expect(s.pid).toBe("4242");
    expect(findingTitle(f({ id: "rootBlocker", params: { count: 4, maxS: 75 }, target: { kind: "session", pid: 4242, backendStartUs: 1 } }))).toBe(
      "Сессия 4242 блокирует сессий: 4, дольше всех ждут 1 мин 15 с",
    );
    expect(findingParams(f({ params: { name: "from-params" }, target: { kind: "slot", name: "from-target" } })).name).toBe("from-params");
    expect(findingParams(f({ member: null })).member).toBeUndefined();
  });

  it("null пропускается, булевы и строки печатаются как есть; команда не форматируется", async () => {
    await i18n.changeLanguage("ru");
    const p = findingParams(f({ params: { active: false, slotType: "physical", walStatus: null, command: "ALTER ROLE \"app\" SET x = '10min';" } }));
    expect(p).toMatchObject({ active: "false", slotType: "physical", command: "ALTER ROLE \"app\" SET x = '10min';" });
    expect("walStatus" in p).toBe(false);
  });

  it("нейтральный заголовок healthPrimary и совет replicaOutOfPool про noloadbalance", async () => {
    await i18n.changeLanguage("ru");
    expect(findingTitle(f({ id: "healthPrimary", params: { count: 2 } }))).toBe("Адрес лидера отвечает 200 у участников: 2");
    expect(findingFix(f({ id: "replicaOutOfPool" }))).toContain("noloadbalance");
    await i18n.changeLanguage("en");
    expect(findingTitle(f({ id: "healthPrimary", params: { count: 0 } }))).toBe("The leader address answers 200 on members: 0");
    await i18n.changeLanguage("ru");
  });

  it("английские заголовки с числом объектов не согласуют существительное: при count = 1 нет «1 sessions» и «1 members»", async () => {
    await i18n.changeLanguage("en");
    const session1 = { kind: "session", pid: 7, backendStartUs: 1 } as const;
    expect([
      findingTitle(f({ id: "lockWait", params: { count: 1, maxS: 75 } })),
      findingTitle(f({ id: "rootBlocker", target: session1, params: { count: 1, maxS: 75 } })),
      findingTitle(f({ id: "ddlQueue", target: session1, params: { relation: "shop.orders", count: 1, maxS: 12 } })),
      findingTitle(f({ id: "healthPrimary", params: { count: 1 } })),
      findingTitle(f({ id: "hiddenQueries", params: { count: 1 } })),
      findingTitle(f({ id: "lockCycle", params: { count: 1 } })),
    ]).toEqual([
      "Sessions waiting for locks: 1, the longest 1 min 15 s",
      "Session 7 blocks sessions: 1, the longest wait 1 min 15 s",
      "DDL on shop.orders is queued and blocks sessions: 1",
      "The leader address answers 200 on members: 1",
      "Texts of sessions are hidden: 1",
      "Sessions waiting for each other: 1",
    ]);
    await i18n.changeLanguage("ru");
  });

  it("тексты есть у каждой находки в обоих языках (ключ не показывается как есть)", async () => {
    expect(IDS).toHaveLength(35);
    // Словарь и список id не расходятся: новая находка без записи в этом тесте не пройдёт.
    expect(Object.keys(DICTIONARIES.en.findings as object).sort()).toEqual([...IDS].sort());
    for (const lng of ["ru", "en"]) {
      await i18n.changeLanguage(lng);
      for (const id of IDS) {
        expect(findingTitle(f({ id })), `${lng}: findings.${id}.title`).not.toMatch(/^findings\./);
        expect(findingFix(f({ id })), `${lng}: findings.${id}.fix`).not.toMatch(/^findings\./);
      }
    }
    await i18n.changeLanguage("ru");
  });

  it("с полным набором параметров, как их выдаёт Rust, в текстах нет остатков подстановки, пустых скобок и двойных пробелов", async () => {
    // Каждая находка представлена хотя бы одним вариантом, и каждый вариант проверен в обоих языках.
    expect([...new Set(EMITTED.map((e) => e.id))].sort()).toEqual([...IDS].sort());
    for (const lng of ["ru", "en"]) {
      await i18n.changeLanguage(lng);
      for (const e of EMITTED) {
        const finding = f(e);
        const params = findingParams(finding);
        for (const part of ["title", "fix"] as const) {
          const where = `${lng}: findings.${e.id}.${part} ${JSON.stringify(e.params)}`;
          // Каждая подстановка шаблона получает значение (иначе i18next подставит пустую строку).
          const template = String((DICTIONARIES[lng].findings as Record<string, Record<string, string>>)[e.id][part]);
          for (const [, name] of template.matchAll(/\{\{\s*(\w+)\s*\}\}/g)) expect(params, `${where}: нет значения для {{${name}}}`).toHaveProperty(name);
          const text = part === "title" ? findingTitle(finding) : findingFix(finding);
          expect(text, where).not.toMatch(/\{\{|\}\}/);
          // Пустые скобки — признак потерянной подстановки; вызов функции («pg_wal_replay_resume()») скобками после слова — законный.
          expect(text, where).not.toMatch(/(^|\W)\(\s*\)/);
          expect(text, where).not.toMatch(/\s{2}/);
          expect(text, where).not.toMatch(/undefined|NaN|\[object/);
          expect(text, where).toBe(text.trim());
          expect(text, where).not.toBe("");
        }
      }
    }
    await i18n.changeLanguage("ru");
  });
});
