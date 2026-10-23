import { describe, expect, it } from "vitest";
import type { ParsedConnection } from "../../api/types";
import type { ConnectionProfile, HostPort } from "../../api/types";
import {
  applyParsed,
  copyProfile,
  emptyKindMemo,
  emptyPatroni,
  formatDatabases,
  formatFromPath,
  hostsFromPatroni,
  type KindMemo,
  newProfile,
  parseDatabases,
  portProblem,
  secretValue,
  switchKind,
  toPatroniFromHosts,
  visibleProfile,
  withKind,
} from "./profile";

const hp = (...names: string[]): HostPort[] => names.map((host) => ({ host, port: 5432 }));
const listOf = (...names: string[]): ConnectionProfile => ({ ...newProfile(), kind: "hostList", hosts: hp(...names) });

/** Цепочка смен вида с памятью формы — как в ConnectionForm. */
function walk(start: ConnectionProfile, ...kinds: ConnectionProfile["kind"][]) {
  let state: { profile: ConnectionProfile; memo: KindMemo } = { profile: start, memo: emptyKindMemo };
  for (const k of kinds) state = switchKind(state.profile, k, state.memo);
  return state;
}

describe("модель формы", () => {
  it("копия подключения — новый профиль с теми же настройками в той же папке", () => {
    const source: ConnectionProfile = {
      ...newProfile(),
      id: "c1",
      name: "billing",
      environment: "stage",
      kind: "hostList",
      hosts: hp("a", "b"),
      pg: { ...newProfile().pg, username: "glafira_diag", databases: ["orders"] },
      folderId: "f1",
      order: 7,
    };
    const copy = copyProfile(source, "billing (копия)");
    expect(copy).toEqual({ ...source, id: "", name: "billing (копия)", order: 0 });
    // Источник не меняется.
    expect(source.id).toBe("c1");
  });

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

  it("адреса API: пробелы обрезаются, IPv6 в скобках не удваивает скобки", () => {
    const urls = (...hosts: string[]) => toPatroniFromHosts({ ...newProfile(), hosts: hp(...hosts) }).urls;
    expect(urls("[::1]")).toEqual(["http://[::1]:8008"]);
    expect(urls("  [fe80::2]  ")).toEqual(["http://[fe80::2]:8008"]);
    expect(urls("  db1 ", "   ")).toEqual(["http://db1:8008"]);
    expect(urls("  ")).toEqual(emptyPatroni().urls);
  });

  it("сервер → список возвращает весь список узлов, правка первого узла сохраняется", () => {
    const toServer = walk(listOf("a", "b", "c"), "server");
    expect(toServer.profile.hosts).toEqual(hp("a"));
    expect(walk(listOf("a", "b", "c"), "server", "hostList").profile.hosts).toEqual(hp("a", "b", "c"));
    // Узел правили, пока был виден один, — список возвращается с правкой, хвост остаётся.
    const edited = { ...toServer.profile, hosts: [{ host: "z", port: 6432 }] };
    expect(switchKind(edited, "hostList", toServer.memo).profile.hosts).toEqual([{ host: "z", port: 6432 }, ...hp("b", "c")]);
    // Одиночный узел запоминать нечего.
    expect(walk(listOf("a"), "server").memo.hosts).toBeNull();
  });

  it("сервер → Patroni берёт адреса API по всему запомненному списку", () => {
    expect(walk(listOf("a", "b", "c"), "server", "patroni").profile.patroni!.urls).toEqual(["http://a:8008", "http://b:8008", "http://c:8008"]);
    // …и обратно в сервер не теряет их тоже.
    expect(walk(listOf("a", "b", "c"), "server", "patroni", "server", "hostList").profile.hosts).toEqual(hp("a", "b", "c"));
  });

  it("адреса API пересчитываются по узлам, пока их не правили руками", () => {
    const first = walk(listOf("a"), "patroni");
    expect(first.profile.patroni!.urls).toEqual(["http://a:8008"]);
    // Вернулись к списку, добавили узел, снова в Patroni: адреса всё ещё «вычисленные» — пересчитываются.
    const back = switchKind(first.profile, "hostList", first.memo);
    const grown = { ...back.profile, hosts: hp("a", "b") };
    const again = switchKind(grown, "patroni", back.memo);
    expect(again.profile.patroni!.urls).toEqual(["http://a:8008", "http://b:8008"]);
    // Остальные настройки Patroni при пересчёте остаются.
    const withUser = { ...first.profile, patroni: { ...first.profile.patroni!, username: "api", hostOverrides: [{ member: "m", host: null, port: null, apiUrl: null }] } };
    const back2 = switchKind(withUser, "hostList", first.memo);
    const again2 = switchKind({ ...back2.profile, hosts: hp("a", "b") }, "patroni", back2.memo).profile.patroni!;
    expect(again2).toMatchObject({ username: "api", urls: ["http://a:8008", "http://b:8008"] });
    expect(again2.hostOverrides).toHaveLength(1);
  });

  it("адреса API, исправленные вручную, не затираются", () => {
    const first = walk(listOf("a"), "patroni");
    const edited = { ...first.profile, patroni: { ...first.profile.patroni!, urls: ["https://pg-a:8443"] } };
    const back = switchKind(edited, "hostList", first.memo);
    const again = switchKind({ ...back.profile, hosts: hp("a", "b") }, "patroni", back.memo);
    expect(again.profile.patroni!.urls).toEqual(["https://pg-a:8443"]);
    // Адреса из сохранённого профиля (памяти формы о них нет) — тоже чужие.
    const stored = { ...newProfile(), kind: "hostList" as const, hosts: hp("x", "y"), patroni: { ...emptyPatroni(), urls: ["http://x:8008"] } };
    expect(switchKind(stored, "patroni").profile.patroni!.urls).toEqual(["http://x:8008"]);
  });

  it("Patroni → список/сервер без узлов: хосты берутся из адресов API", () => {
    const stored: ConnectionProfile = {
      ...newProfile(),
      kind: "patroni",
      hosts: [],
      patroni: { ...emptyPatroni(), urls: ["https://pg-1.example:8443/", "http://pg-2:8008", "http://pg-1.example:8009", "http://user:pw@[::1]:8008/x", "oops"] },
    };
    expect(withKind(stored, "hostList").hosts).toEqual(hp("pg-1.example", "pg-2", "::1"));
    const server = switchKind(stored, "server");
    expect(server.profile.hosts).toEqual(hp("pg-1.example"));
    // Остальные хосты запомнены: список возвращается целиком.
    expect(switchKind(server.profile, "hostList", server.memo).profile.hosts).toEqual(hp("pg-1.example", "pg-2", "::1"));
    // Ничего не разобрали — как раньше, localhost.
    const junk = { ...stored, patroni: { ...emptyPatroni(), urls: ["", "nonsense"] } };
    expect(withKind(junk, "server").hosts).toEqual(hp("localhost"));
    // Узлы уже есть — остаются как есть.
    expect(withKind({ ...stored, hosts: hp("keep") }, "hostList").hosts).toEqual(hp("keep"));
  });

  it("hostsFromPatroni: схема обязательна, IPv6 без скобок", () => {
    expect(hostsFromPatroni(null)).toEqual([]);
    expect(hostsFromPatroni({ ...emptyPatroni(), urls: ["http://[2001:db8::1]:8008", "pg-3:8008", "http://"] })).toEqual(hp("2001:db8::1"));
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

  it("строка подключения: пустые поля патча оставляют прежние значения", () => {
    const prev: ConnectionProfile = {
      ...newProfile(),
      targetServerType: "standby",
      pg: { username: "old", database: "olddb", sslMode: "require", tls: { caPath: "/old-ca.pem", clientCert: { format: "pem", certificatePath: "/c.pem", keyPath: "/k.pem" } }, connectTimeoutS: 33, databases: ["x"] },
    };
    const parsed: ParsedConnection = {
      patch: { kind: "server", hosts: hp("new-host"), targetServerType: null, username: null, database: null, sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: null,
      ignored: [],
    };
    const p = applyParsed({ ...prev, patroni: emptyPatroni() }, parsed);
    expect(p.kind).toBe("server");
    expect(p.hosts).toEqual(hp("new-host"));
    expect(p.patroni).toBeNull();
    expect(p.targetServerType).toBe("standby");
    expect(p.pg).toEqual(prev.pg);
    // Заданные поля патча перекрывают прежние, остальные остаются.
    const q = applyParsed(prev, { ...parsed, patch: { ...parsed.patch, username: "new", sslMode: "disable", clientCert: null } });
    expect(q.pg).toMatchObject({ username: "new", database: "olddb", sslMode: "disable", connectTimeoutS: 33 });
    expect(q.pg.tls.clientCert).toEqual(prev.pg.tls.clientCert);
  });

  it("sslmode=disable: скрытые CA и клиентский сертификат в профиль не уходят", () => {
    const cert = { format: "pem" as const, certificatePath: "", keyPath: "" };
    const p: ConnectionProfile = { ...newProfile(), pg: { ...newProfile().pg, sslMode: "disable", tls: { caPath: "/ca.pem", clientCert: cert } } };
    expect(visibleProfile(p).pg.tls).toEqual({ caPath: null, clientCert: null });
    expect(visibleProfile(p).pg.sslMode).toBe("disable");
    // Другие режимы — без изменений.
    const prefer = { ...p, pg: { ...p.pg, sslMode: "prefer" as const } };
    expect(visibleProfile(prefer)).toEqual(prefer);
    // Исходный профиль формы не меняется: при возврате на другой режим ввод цел.
    expect(p.pg.tls.clientCert).toBe(cert);
  });

  it("у профиля Patroni скрытые узлы в Rust не уходят: порт из прежнего вида не ломает сохранение", () => {
    const p: ConnectionProfile = { ...withKind(newProfile(), "patroni"), hosts: [{ host: "h", port: 70000 }] };
    expect(visibleProfile(p).hosts).toEqual([]);
    expect(portProblem(visibleProfile(p))).toBeNull();
    // Сама форма узлы помнит: исходный профиль не меняется.
    expect(p.hosts).toEqual([{ host: "h", port: 70000 }]);
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

describe("portProblem", () => {
  const server = (port: number): ConnectionProfile => ({ ...newProfile(), hosts: [{ host: "h", port }] });
  it.each([0, 65536, 1.5, Number.NaN, -1])("порт %s — ошибка с ключом словаря", (port) => {
    expect(portProblem(server(port))).toMatchObject({ code: "INVALID_INPUT", key: "errors.connections.portInvalid" });
  });
  it.each([1, 5432, 65535])("порт %s — допустим", (port) => {
    expect(portProblem(server(port))).toBeNull();
  });
  it("у Patroni проверяются подмены; null — порт не меняется", () => {
    const p = (port: number | null): ConnectionProfile => ({
      ...newProfile(),
      kind: "patroni",
      hosts: [],
      patroni: { ...emptyPatroni(), hostOverrides: [{ member: "m", host: null, port, apiUrl: null }] },
    });
    expect(portProblem(p(null))).toBeNull();
    expect(portProblem(p(6432))).toBeNull();
    expect(portProblem(p(70000))).not.toBeNull();
  });
});
