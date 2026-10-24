import type { Finding } from "../../api/types";
import { i18n } from "../../i18n";
import { formatBytes, formatCount, formatDuration, formatPercent } from "../../lib/format";

/**
 * Параметры текста находки: значения форматируются по суффиксу имени (`…Bytes` — байты, `…Pct` — проценты,
 * `…S` — длительность, `…Oid` — как есть, остальные числа — с разделителями), добавляются узел, объект (`name`
 * или `pid`), подпись вида держателя горизонта (`kindLabel`), общий «лаг» из байт и времени и, для отношения другой
 * базы, `relation` вида «oid N в БД X».
 */
export function findingParams(f: Finding): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, v] of Object.entries(f.params)) {
    if (v === null) continue;
    if (typeof v === "number" && k.endsWith("Bytes")) out[k] = formatBytes(v);
    else if (typeof v === "number" && k.endsWith("Pct")) out[k] = formatPercent(v);
    else if (typeof v === "number" && k.endsWith("S")) out[k] = formatDuration(v);
    // Идентификатор объекта, а не количество: разделители разрядов сделали бы его нечитаемым как oid.
    else if (typeof v === "number" && k.endsWith("Oid")) out[k] = String(v);
    else if (typeof v === "number") out[k] = formatCount(v);
    else out[k] = String(v);
  }
  if (f.member) out.member = f.member;
  if (f.target?.kind === "session") out.pid = String(f.target.pid);
  else if (f.target && !("name" in out)) out.name = f.target.name;
  if (typeof f.params.kind === "string") out.kindLabel = i18n.t(`diag.horizon.kinds.${f.params.kind}`);
  if ("lagBytes" in f.params || "delayS" in f.params) out.lag = [out.lagBytes, out.delayS].filter(Boolean).join(", ");
  // Отношение другой базы по имени не определяется: сервер присылает oid и имя БД.
  if (!("relation" in out) && out.relationOid && out.relationDatabase) {
    out.relation = i18n.t("diag.locks.otherDb", { oid: out.relationOid, db: out.relationDatabase });
  }
  return out;
}

export const findingTitle = (f: Finding) => i18n.t(`findings.${f.id}.title`, findingParams(f));
export const findingFix = (f: Finding) => i18n.t(`findings.${f.id}.fix`, findingParams(f));
