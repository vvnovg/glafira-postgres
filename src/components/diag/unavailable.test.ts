import { describe, expect, it } from "vitest";
import type { Unavailable } from "../../api/types";
import { UNAVAILABLE_PARTS, unavailableText } from "./unavailable";

const version = (part: string, version: string): Unavailable => ({
  part,
  reason: { key: "diag.unavailable.version", text: `${part}: PostgreSQL ${version}+`, params: { part, version } },
});

describe("unavailableText", () => {
  it("вместо внутреннего имени части — подпись", () => {
    expect(unavailableText(version("slotInactiveSince", "17"))).toBe("Время простоя слота: только с PostgreSQL 17");
    expect(unavailableText(version("queryId", "14"))).toBe("Идентификатор запроса (query_id): только с PostgreSQL 14");
  });

  it("неизвестная часть остаётся как есть", () => {
    expect(unavailableText(version("somethingNew", "19"))).toBe("somethingNew: только с PostgreSQL 19");
  });

  it("у каждой части, которую шлёт Rust, есть подпись", () => {
    // Имена частей из src-tauri/src/diag (caps.missing, needs_monitor, Unavailable { part }).
    const rust = ["queryId", "walDir", "otherSessionsXmin", "replicationDetails", "replayPauseState", "conflictActiveLogicalSlot", "slotTwoPhase", "slotConflicting", "slotInactiveSince", "subscriptionErrors", "patroniSlots"];
    expect(Object.keys(UNAVAILABLE_PARTS).sort()).toEqual([...rust].sort());
  });
});
