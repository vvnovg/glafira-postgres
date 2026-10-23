import type { AppError, CertFormat, ConnectionKind, ConnectionProfile, HostPort, ParsedConnection, PatroniSettings } from "../../api/types";

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

/** Копия подключения: те же настройки и папка, но новый профиль (id выдаст Rust при сохранении). Секреты не копируются. */
export function copyProfile(source: ConnectionProfile, name: string): ConnectionProfile {
  return { ...source, id: "", name, order: 0 };
}

export function emptyPatroni(): PatroniSettings {
  return { urls: ["http://localhost:8008"], username: null, tls: null, hostOverrides: [] };
}

/** Хост для URL: пробелы обрезаются, IPv6 берётся в скобки (уже заключённый в скобки не трогаем). */
function urlHost(host: string): string {
  const h = host.trim();
  return h.includes(":") && !(h.startsWith("[") && h.endsWith("]")) ? `[${h}]` : h;
}

/** «Узлы под управлением Patroni»: адреса API — хосты списка с портом 8008 (спец. P1 §3.1). */
export function toPatroniFromHosts(p: ConnectionProfile): PatroniSettings {
  const urls = p.hosts.filter((h) => h.host.trim()).map((h) => `http://${urlHost(h.host)}:8008`);
  return { ...emptyPatroni(), urls: urls.length > 0 ? urls : emptyPatroni().urls };
}

/** Обратное преобразование: хосты из адресов API (порт PostgreSQL — 5432); не-адреса пропускаются, повторы схлопываются. */
export function hostsFromPatroni(pt: PatroniSettings | null): HostPort[] {
  const hosts: HostPort[] = [];
  for (const raw of pt?.urls ?? []) {
    const m = /^https?:\/\/(?:[^/?#@]*@)?(\[[^\]]*\]|[^/?#:]+)/i.exec(raw.trim());
    // IPv6 в хранимом профиле — без скобок (скобки добавляются только в URL).
    const host = m?.[1].replace(/^\[|\]$/g, "").trim();
    if (host && !hosts.some((h) => h.host === host)) hosts.push({ host, port: 5432 });
  }
  return hosts;
}

/**
 * Память формы между сменами вида — то, что нельзя хранить в самом профиле (оно уйдёт в Rust и на диск).
 * `hosts` — полный список узлов, который вид «сервер» урезал до одного;
 * `derivedUrls` — адреса API, которые форма вычислила из узлов сама (если их правили руками — они другие).
 */
export type KindMemo = { hosts: HostPort[] | null; derivedUrls: string[] | null };
export const emptyKindMemo: KindMemo = { hosts: null, derivedUrls: null };

const sameList = (a: string[], b: string[]) => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * Смена вида: узлы и настройки Patroni переносятся так, чтобы ввод не терялся ни при каких обходах.
 * Возвращает и новую память формы — её хранит вызывающий.
 */
export function switchKind(p: ConnectionProfile, kind: ConnectionKind, memo: KindMemo = emptyKindMemo): { profile: ConnectionProfile; memo: KindMemo } {
  if (kind === p.kind) return { profile: p, memo };
  // Полный список: в «сервере» один узел (его правки сохраняются), остальные — из памяти.
  const full = p.kind === "server" && memo.hosts && p.hosts.length > 0 ? [p.hosts[0], ...memo.hosts.slice(1)] : p.hosts;

  if (kind === "patroni") {
    const derived = toPatroniFromHosts({ ...p, hosts: full }).urls;
    // Адреса, которые форма вычислила сама и которые не правили, пересчитываются по нынешним узлам; правленые — нет.
    const rederive = p.patroni !== null && memo.derivedUrls !== null && sameList(p.patroni.urls, memo.derivedUrls);
    const patroni = p.patroni === null ? toPatroniFromHosts({ ...p, hosts: full }) : rederive ? { ...p.patroni, urls: derived } : p.patroni;
    const derivedUrls = p.patroni === null || rederive ? derived : memo.derivedUrls;
    return { profile: { ...p, kind, hosts: full, patroni }, memo: { hosts: null, derivedUrls } };
  }

  // У подключения Patroni узлов может не быть (Rust очищает их при сохранении) — берём хосты из адресов API.
  const fromUrls = full.length > 0 ? full : hostsFromPatroni(p.patroni);
  const hosts = fromUrls.length > 0 ? fromUrls : [{ host: "localhost", port: 5432 }];
  if (kind === "server") return { profile: { ...p, kind, hosts: hosts.slice(0, 1) }, memo: { ...memo, hosts: hosts.length > 1 ? hosts : null } };
  return { profile: { ...p, kind, hosts }, memo: { ...memo, hosts: null } };
}

/** Смена вида без памяти формы (для кода, которому не нужен возврат к урезанному списку). */
export function withKind(p: ConnectionProfile, kind: ConnectionKind): ConnectionProfile {
  return switchKind(p, kind).profile;
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

/**
 * Профиль в том виде, в каком его видит пользователь: при sslmode=disable поля CA и клиентского сертификата
 * скрыты и в профиль не попадают, а у Patroni нет узлов (Rust их всё равно очищает, а порт из прежнего вида
 * формы иначе ломал бы разбор аргументов команды). Rust при `disable` TLS не использует (`pg_connector` → `None`),
 * но `validate_tls` проверил бы и скрытые пути, а `uses_secret(pgKeyPassword)` считал бы сертификат «в работе».
 * Сама форма скрытое помнит — при возврате на другой режим или вид ввод не теряется.
 */
export function visibleProfile(p: ConnectionProfile): ConnectionProfile {
  const hosts = p.kind === "patroni" ? [] : p.hosts;
  const tls = p.pg.sslMode === "disable" ? { caPath: null, clientCert: null } : p.pg.tls;
  return { ...p, hosts, pg: { ...p.pg, tls } };
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

/** Порт TCP: целое от 1 до 65535 (`Number("")` — 0, слишком большое или дробное число Rust как u16 не примет). */
export function isValidPort(port: number): boolean {
  return Number.isInteger(port) && port >= 1 && port <= 65535;
}

/**
 * Порты формы, которые нельзя отправить в Rust: узлы сервера и списка, подмены адресов Patroni (пустая подмена порта —
 * «не менять», это null). Иначе вместо понятной ошибки пришёл бы сбой разбора аргументов команды.
 */
export function portProblem(p: ConnectionProfile): AppError | null {
  const ports = p.kind === "patroni" ? (p.patroni?.hostOverrides ?? []).flatMap((o) => (o.port === null ? [] : [o.port])) : p.hosts.map((h) => h.port);
  if (ports.every(isValidPort)) return null;
  return { code: "INVALID_INPUT", key: "errors.connections.portInvalid", message: "Port must be from 1 to 65535", retryable: false };
}
