import { afterEach, describe, expect, it } from "vitest";
import type { Role } from "../api/types";
import { i18n } from "../i18n";
import { member } from "../test/fixtures";
import { hasRunningLeader, isRunningLeader, isStandby, roleLabel } from "./topology";

describe("роли узлов", () => {
  afterEach(() => i18n.changeLanguage("ru"));

  it("isStandby: реплики всех видов, но не лидер standby-кластера", () => {
    const standby: Role[] = ["replica", "syncStandby", "quorumStandby"];
    const other: Role[] = ["primary", "standbyLeader", "unknown"];
    expect(standby.every(isStandby)).toBe(true);
    expect(other.some(isStandby)).toBe(false);
  });

  it("у кластера Patroni primary подписан «Leader», у остальных видов — «Primary»", async () => {
    await i18n.changeLanguage("en");
    expect(roleLabel("primary", "patroni")).toBe("Leader");
    expect(roleLabel("primary", "hostList")).toBe("Primary");
    expect(roleLabel("primary", "server")).toBe("Primary");
  });

  it("подписи остальных ролей по языкам", async () => {
    await i18n.changeLanguage("en");
    expect(roleLabel("standbyLeader", "patroni")).toBe("Standby leader");
    expect(roleLabel("replica", "patroni")).toBe("Replica");
    expect(roleLabel("unknown", "server")).toBe("—");
    await i18n.changeLanguage("ru");
    expect(roleLabel("standbyLeader", "patroni")).toBe("Лидер standby");
    expect(roleLabel("replica", "patroni")).toBe("Реплика");
    expect(roleLabel("syncStandby", "patroni")).toBe("Sync");
    expect(roleLabel("quorumStandby", "patroni")).toBe("Quorum");
  });
});

describe("лидер, принимающий запись (как running_leader в Rust)", () => {
  it("isRunningLeader: нужны и ответ на опрос, и состояние running", () => {
    expect(isRunningLeader(true, "running")).toBe(true);
    for (const [reachable, state] of [[false, "running"], [true, "stopped"], [true, null], [null, null], [undefined, "running"]] as const) {
      expect(isRunningLeader(reachable, state)).toBe(false);
    }
  });

  it("hasRunningLeader: первый primary, иначе первый standby leader; реплики лидером не считаются", () => {
    const leader = (patch = {}) => member("pg-1", { role: "primary", state: "running", ...patch });
    const replica = member("pg-2", { role: "replica", state: "running" });
    expect(hasRunningLeader([leader(), replica])).toBe(true);
    expect(hasRunningLeader([leader({ reachable: false }), replica])).toBe(false);
    expect(hasRunningLeader([leader({ state: "stopped" }), replica])).toBe(false);
    expect(hasRunningLeader([replica])).toBe(false);
    expect(hasRunningLeader([])).toBe(false);
    expect(hasRunningLeader([member("pg-1", { role: "standbyLeader", state: "running" }), replica])).toBe(true);
    // Как в Rust, лидер — первый primary: работающий второй (split-brain) работающего «первого» не заменяет.
    expect(hasRunningLeader([leader({ state: "stopped" }), member("pg-3", { role: "primary", state: "running" })])).toBe(false);
  });
});
