import { useTranslation } from "react-i18next";
import type { AgentRun } from "../../../api/types";
import { formatDay, formatUsd } from "../../../lib/format";

/** У запуска есть стоимость: точная от CLI (`costUsd`) или оценка по ценам Ollama Cloud (`costEstimate`). */
export function hasCost(run: AgentRun): boolean {
  return run.costUsd != null || run.costEstimate != null;
}

/**
 * Стоимость запуска: точная, как её сообщил CLI (Claude Code без Ollama), — как есть; оценка облачного запуска через Ollama
 * по ценам Ollama Cloud (смоук P4c, sf-T6) — «≈ 0,01 $» с подсказкой о дне цен таблицы (и «вне пика»). В «Запусках» и в
 * строке статуса консоли и карточки после конца запуска (дополнение владельца к sf-T7).
 */
export function RunCost({ run }: { run: AgentRun }) {
  const { t } = useTranslation();
  if (run.costUsd != null) return <>{formatUsd(run.costUsd)}</>;
  const e = run.costEstimate;
  if (!e) return null;
  const hint = e.offPeak ? "settings.mcp.agent.runs.costEstimateHintOffPeak" : "settings.mcp.agent.runs.costEstimateHint";
  return <span title={t(hint, { date: formatDay(e.pricesDate) })}>{t("settings.mcp.agent.runs.costEstimate", { cost: formatUsd(e.usd, "estimate") })}</span>;
}
