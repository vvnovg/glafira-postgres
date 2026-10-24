import { useTranslation } from "react-i18next";
import type { ConnectionKind, DiagTool, Member } from "../../api/types";
import { formatTimestamp } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { DIAG_INTERVALS_S } from "../../lib/useDiagnostic";

/** Значение пункта «За лидером» в выборе узла и пункта «Пауза» в выборе интервала. */
const FOLLOW = "";
const PAUSE = "pause";

export interface DiagControls {
  /** Участники для выбора узла; `null` — выбора нет (одиночный сервер и инструменты кластера). */
  members: Member[] | null;
  kind: ConnectionKind;
  /** Закреплённый узел; `null` — за лидером. */
  selected: string | null;
  /** Имя лидера из последнего отчёта «за лидером» (до первого отчёта его нет). */
  leader: string | null;
  onSelect: (member: string | null) => void;
  intervalS: number | null;
  onInterval: (seconds: number | null) => void;
  onRefresh: () => void;
  /** Время ответа последнего отчёта (мс эпохи). */
  observedAtMs: number | null;
  /** Обновление не удалось, на экране прошлые данные. */
  stale: boolean;
}

/**
 * Общая шапка вкладки инструмента (спец. P2 §7.2): название, выбор узла, интервал обновления, «Обновить», время
 * последнего ответа. Без `controls` (подключение закрыто) — только название.
 */
export function DiagHeader({ tool, controls }: { tool: DiagTool; controls?: DiagControls }) {
  const { t } = useTranslation();
  const c = controls;
  return (
    <div className="toolbar diag-head">
      <h2>{t(`diag.tools.${tool}`)}</h2>
      {c?.members && (
        <label className="inline">
          <span>{t("diag.header.node")}</span>
          <select aria-label={t("diag.header.node")} value={c.selected ?? FOLLOW} onChange={(e) => c.onSelect(e.target.value === FOLLOW ? null : e.target.value)}>
            <option value={FOLLOW}>{c.leader ? t("diag.header.followLeader", { member: c.leader }) : t("diag.header.followLeaderPending")}</option>
            {c.members.map((m) => (
              <option key={m.name} value={m.name}>
                {m.name} — {roleLabel(m.role, c.kind)}
              </option>
            ))}
          </select>
        </label>
      )}
      {c && (
        <>
          <label className="inline">
            <span>{t("diag.header.interval")}</span>
            <select
              aria-label={t("diag.header.interval")}
              value={c.intervalS === null ? PAUSE : String(c.intervalS)}
              onChange={(e) => c.onInterval(e.target.value === PAUSE ? null : Number(e.target.value))}
            >
              {DIAG_INTERVALS_S.map((s) => (
                <option key={s} value={String(s)}>
                  {t("diag.header.seconds", { n: s })}
                </option>
              ))}
              <option value={PAUSE}>{t("diag.header.pause")}</option>
            </select>
          </label>
          <button type="button" onClick={c.onRefresh}>
            {t("diag.header.refresh")}
          </button>
          {c.observedAtMs !== null && <span className="muted">{t("diag.header.updated", { time: formatTimestamp(c.observedAtMs) })}</span>}
          {c.stale && (
            <span className="warn" role="status">
              {t("diag.header.stale")}
            </span>
          )}
        </>
      )}
    </div>
  );
}
