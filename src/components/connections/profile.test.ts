import { describe, expect, it } from "vitest";
import type { ParsedConnection } from "../../api/types";
import { applyParsed, formatDatabases, formatFromPath, newProfile, parseDatabases, secretValue, toPatroniFromHosts, withKind } from "./profile";

describe("модель формы", () => {
  it("смена вида не теряет ввод", () => {
    const list = { ...withKind(newProfile(), "hostList"), hosts: [{ host: "a", port: 5432 }, { host: "b", port: 5433 }] };
    expect(withKind(list, "server").hosts).toEqual([{ host: "a", port: 5432 }]);
    const patroni = withKind(list, "patroni");
    expect(patroni.patroni!.urls).toEqual(["http://a:8008", "http://b:8008"]);
    expect(withKind(patroni, "hostList").hosts).toHaveLength(2);
  });

  it("адреса API из IPv6", () => {
    expect(toPatroniFromHosts({ ...newProfile(), hosts: [{ host: "::1", port: 5432 }] }).urls).toEqual(["http://[::1]:8008"]);
  });

  it("строка подключения заполняет форму", () => {
    const parsed: ParsedConnection = {
      patch: {
        kind: "hostList",
        hosts: [{ host: "h1", port: 5432 }, { host: "h2", port: 5433 }],
        targetServerType: "preferStandby",
        username: "app",
        database: "orders",
        sslMode: "verify-full",
        caPath: "/ca.pem",
        clientCert: null,
        connectTimeoutS: 7,
      },
      password: "pw",
      ignored: [],
    };
    const p = applyParsed(withKind(newProfile(), "patroni"), parsed);
    expect(p.kind).toBe("hostList");
    expect(p.patroni).toBeNull();
    expect(p.hosts).toHaveLength(2);
    expect(p.targetServerType).toBe("preferStandby");
    expect(p.pg).toMatchObject({ username: "app", database: "orders", sslMode: "verify-full", connectTimeoutS: 7 });
    expect(p.pg.tls.caPath).toBe("/ca.pem");
  });

  it("фильтр баз и форматы", () => {
    expect(parseDatabases(" orders, ,billing ")).toEqual(["orders", "billing"]);
    expect(parseDatabases("  ")).toBeNull();
    expect(formatDatabases(["a", "b"])).toBe("a, b");
    expect(formatFromPath("/x/client.P12")).toBe("pkcs12");
    expect(formatFromPath("/x/c.crt")).toBe("pem");
    expect(formatFromPath("/x/c.bin")).toBeNull();
  });

  it("семантика секрета", () => {
    expect(secretValue({ text: "", clear: false })).toBeNull();
    expect(secretValue({ text: "", clear: true })).toBe("");
    expect(secretValue({ text: "x", clear: false })).toBe("x");
  });
});
