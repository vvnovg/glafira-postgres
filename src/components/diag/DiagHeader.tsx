import { useTranslation } from "react-i18next";
import type { ConnectionKind, DiagTool, Member } from "../../api/types";
import { formatTime, formatTimestamp } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { Hint } from "../common/Hint";
import { DIAG_INTERVALS_S, WAITS_WINDOWS_S } from "../../lib/useDiagnostic";

/** Пояснение «?» у названия инструмента. Ключи — литералами: `pnpm i18n:check` сверяет значки с ключами `hints`. */
const TOOL_HINTS: Record<DiagTool, string> = {
  pulse: "hints.diag.pulse",
  sessions: "hints.diag.sessions",
  idleTx: "hints.diag.idleTx",
  waits: "hints.diag.waits",
  locks: "hints.diag.locks",
  horizon: "hints.diag.horizon",
  replication: "hints.diag.replication",
  slots: "hints.diag.slots",
  failover: "hints.diag.failover",
  health: "hints.diag.health",
};

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
  /** Окно профиля ожиданий, секунды; `null` — у инструмента окна нет (выбор не показывается). */
  windowS: number | null;
  onWindow: (seconds: number) => void;
  onRefresh: () => void;
  /** Когда приложение получило последний отчёт (мс эпохи, часы приложения); `null` — отчёта ещё нет. */
  updatedAtMs: number | null;
  /** Обновление не удалось, на экране прошлые данные. */
  stale: boolean;
}

/**
 * Общая шапка вкладки инструмента (спец. P2 §7.2): название, выбор узла, интервал обновления, окно (у профиля
 * ожиданий), «Обновить», время получения последнего отчёта. Без `controls` (подключение закрыто) — только название.
 */
export function DiagHeader({ tool, controls }: { tool: DiagTool; controls?: DiagControls }) {
  const { t } = useTranslation();
  const c = controls;
  return (
    <div className="toolbar diag-head">
      <div className="diag-title">
        <h2>{t(`diag.tools.${tool}`)}</h2>
        <Hint k={TOOL_HINTS[tool]} label={t(`diag.tools.${tool}`)} />
      </div>
      {c?.members && (
        <label className="inline">
          <span>{t("diag.header.node")}</span>
          <select aria-label={t("diag.header.node")} value={c.selected ?? FOLLOW} onChange={(e) => c.onSelect(e.target.value === FOLLOW ? null : e.target.value)}>
            <option value={FOLLOW}>{c.leader ? t("diag.header.followLeader", { member: c.leader }) : t("diag.header.followLeaderPending")}</option>
            {c.members.map((m) => (
              <option key={m.name} value={m.name}>
                {t("diag.header.memberOption", { member: m.name, role: roleLabel(m.role, c.kind) })}
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
          {c.windowS !== null && (
            <label className="inline">
              <span>{t("diag.waits.window")}</span>
              <select aria-label={t("diag.waits.window")} value={String(c.windowS)} onChange={(e) => c.onWindow(Number(e.target.value))}>
                {WAITS_WINDOWS_S.map((s) => (
                  <option key={s} value={String(s)}>
                    {t("diag.duration.m", { n: s / 60 })}
                  </option>
                ))}
              </select>
            </label>
          )}
          <button type="button" onClick={c.onRefresh}>
            {t("diag.header.refresh")}
          </button>
          {c.updatedAtMs !== null && (
            <span className="muted" title={formatTimestamp(c.updatedAtMs)}>
              {t("diag.header.updated", { time: formatTime(c.updatedAtMs) })}
            </span>
          )}
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
