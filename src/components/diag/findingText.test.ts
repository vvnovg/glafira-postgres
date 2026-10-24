import { describe, expect, it } from "vitest";
import type { Finding } from "../../api/types";
import { i18n } from "../../i18n";
import { formatCount } from "../../lib/format";
import { findingFix, findingParams, findingTitle } from "./findingText";

const f = (patch: Partial<Finding>): Finding => ({ id: "idleInTransaction", severity: "warning", member: "pg-1", target: null, params: {}, links: [], ...patch });

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
    expect(findingTitle(f({ id: "healthPrimary", params: { count: 0 } }))).toBe("The leader address answers 200 on 0 members");
    await i18n.changeLanguage("ru");
  });

  it("тексты есть у каждой находки в обоих языках (ключ не показывается как есть)", async () => {
    const ids = [
      "connectionsSaturated", "idleInTransaction", "longQuery", "longTransaction", "xidAge", "lockWait", "walDirSize", "deadlocks", "lockShare",
      "rootBlocker", "ddlQueue", "xminHorizon", "replicaLag", "replicaNotStreaming", "replicaReplyStale", "syncStandbyMissing", "walReceiverDown",
      "replayPaused", "recoveryConflicts", "recoveryConflictsTotal", "slotRetainsWal", "slotLost", "subscriptionErrors", "failoverCandidates",
      "clusterPaused", "syncModeWithoutSync", "patroniTiming", "healthPrimary", "healthUnhealthy", "replicaOutOfPool", "cacheHitLow", "tempFiles",
      "noIdleTimeout", "hiddenQueries", "lockCycle",
    ];
    expect(ids).toHaveLength(35);
    for (const lng of ["ru", "en"]) {
      await i18n.changeLanguage(lng);
      for (const id of ids) {
        expect(findingTitle(f({ id })), `${lng}: findings.${id}.title`).not.toMatch(/^findings\./);
        expect(findingFix(f({ id })), `${lng}: findings.${id}.fix`).not.toMatch(/^findings\./);
      }
    }
    await i18n.changeLanguage("ru");
  });
});
