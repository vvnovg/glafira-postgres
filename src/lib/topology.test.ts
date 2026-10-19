import { afterEach, describe, expect, it } from "vitest";
import type { Role } from "../api/types";
import { i18n } from "../i18n";
import { isStandby, roleLabel } from "./topology";

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
