import type { CertFormat, ConnectionKind, ConnectionProfile, ParsedConnection, PatroniSettings } from "../../api/types";

export function newProfile(): ConnectionProfile {
  return {
    id: "",
    name: "",
    color: "#3b82f6",
    environment: "local",
    readOnly: false,
    kind: "server",
    hosts: [{ host: "localhost", port: 5432 }],
    targetServerType: "primary",
    patroni: null,
    pg: {
      username: "",
      database: "postgres",
      sslMode: "prefer",
      tls: { caPath: null, clientCert: null },
      connectTimeoutS: 10,
      databases: null,
    },
    folderId: null,
    order: 0,
  };
}

export function emptyPatroni(): PatroniSettings {
  return { urls: ["http://localhost:8008"], username: null, tls: null, hostOverrides: [] };
}

/** «Узлы под управлением Patroni»: адреса API — хосты списка с портом 8008 (спец. P1 §3.1). */
export function toPatroniFromHosts(p: ConnectionProfile): PatroniSettings {
  const urls = p.hosts
    .filter((h) => h.host.trim())
    .map((h) => `http://${h.host.includes(":") ? `[${h.host}]` : h.host}:8008`);
  return { ...emptyPatroni(), urls: urls.length > 0 ? urls : emptyPatroni().urls };
}

/** Смена вида: узлы и настройки Patroni переносятся так, чтобы ввод не терялся. */
export function withKind(p: ConnectionProfile, kind: ConnectionKind): ConnectionProfile {
  if (kind === p.kind) return p;
  if (kind === "patroni") return { ...p, kind, patroni: p.patroni ?? toPatroniFromHosts(p) };
  const hosts = p.hosts.length > 0 ? p.hosts : [{ host: "localhost", port: 5432 }];
  return { ...p, kind, hosts: kind === "server" ? hosts.slice(0, 1) : hosts };
}

export function applyParsed(p: ConnectionProfile, parsed: ParsedConnection): ConnectionProfile {
  const x = parsed.patch;
  return {
    ...p,
    kind: x.kind,
    hosts: x.hosts,
    patroni: null,
    targetServerType: x.targetServerType ?? p.targetServerType,
    pg: {
      ...p.pg,
      username: x.username ?? p.pg.username,
      database: x.database ?? p.pg.database,
      sslMode: x.sslMode ?? p.pg.sslMode,
      tls: { caPath: x.caPath ?? p.pg.tls.caPath, clientCert: x.clientCert ?? p.pg.tls.clientCert },
      connectTimeoutS: x.connectTimeoutS ?? p.pg.connectTimeoutS,
    },
  };
}

/** Фильтр баз «orders, billing» → список; пусто — все базы. */
export function parseDatabases(text: string): string[] | null {
  const list = text
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  return list.length > 0 ? list : null;
}

export function formatDatabases(list: string[] | null): string {
  return (list ?? []).join(", ");
}

/** Формат сертификата по расширению; null — не угадали. */
export function formatFromPath(path: string): CertFormat | null {
  const ext = path.toLowerCase().split(/[\\/]/).pop()?.split(".").pop() ?? "";
  if (ext === "p12" || ext === "pfx") return "pkcs12";
  if (ext === "pem" || ext === "crt" || ext === "cer" || ext === "key") return "pem";
  return null;
}

export type SecretField = { text: string; clear: boolean };
export const emptySecret: SecretField = { text: "", clear: false };

/** null — не менять сохранённый секрет, "" — удалить, иначе — новый. */
export function secretValue(f: SecretField): string | null {
  if (f.clear) return "";
  return f.text ? f.text : null;
}
