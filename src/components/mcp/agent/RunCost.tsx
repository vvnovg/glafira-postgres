import { useTranslation } from "react-i18next";
import type { AgentRun, McpAuditEntry } from "../../../api/types";
import { formatDay, formatUsd } from "../../../lib/format";
import { Hint } from "../../common/Hint";

/** Оценка по ценам Ollama Cloud: сумма, день цен таблицы (у записи журнала прежней сборки его нет), «вне пика». */
interface Estimate {
  usd: number;
  pricesDate?: string;
  offPeak?: boolean;
}

/** У запуска есть стоимость: точная от CLI (`costUsd`) или оценка по ценам Ollama Cloud (`costEstimate`). */
export function hasCost(run: AgentRun): boolean {
  return run.costUsd != null || run.costEstimate != null;
}

/**
 * Стоимость: точная, как её сообщил CLI (Claude Code без Ollama), — как есть; оценка облачного запуска через Ollama по ценам
 * Ollama Cloud (смоук P4c, sf-T6) — «≈ 0,01 $» с подсказкой о дне цен таблицы (и «вне пика»). Подсказка — `Hint` у самого
 * текста: наведение, фокус с клавиатуры, имя для скринридера «≈ 0,01 $: Оценка по ценам …» (итоговое ревью фронтенда).
 */
function CostText({ exact, estimate }: { exact?: number | null; estimate?: Estimate | null }) {
  const { t } = useTranslation();
  if (exact != null) return <>{formatUsd(exact)}</>;
  if (!estimate) return null;
  const values = estimate.pricesDate ? { date: formatDay(estimate.pricesDate) } : undefined;
  const k = !estimate.pricesDate
    ? "settings.mcp.agent.runs.costEstimateHintNoDate"
    : estimate.offPeak
      ? "settings.mcp.agent.runs.costEstimateHintOffPeak"
      : "settings.mcp.agent.runs.costEstimateHint";
  const text = t("settings.mcp.agent.runs.costEstimate", { cost: formatUsd(estimate.usd, "estimate") });
  return (
    <Hint k={k} values={values} label={t("settings.mcp.agent.runs.costEstimateLabel", { cost: text, hint: t(k, values) })}>
      {text}
    </Hint>
  );
}

/** Стоимость запуска — в «Запусках» и в строке статуса консоли и карточки после конца запуска (дополнение владельца к sf-T7). */
export function RunCost({ run }: { run: AgentRun }) {
  return <CostText exact={run.costUsd} estimate={run.costEstimate} />;
}

/**
 * Стоимость конца фонового запуска в «Журнале» «Настроек» (`agentRunFinished`, итоговая волна правок смоука P4c): `costUsd`
 * с `costEstimated: true` — оценка с днём цен `pricesDate` и `offPeak`, без признака — точная. Нет стоимости — ничего.
 */
export function AuditCost({ entry }: { entry: McpAuditEntry }) {
  const usd = entry.costUsd;
  if (entry.event !== "agentRunFinished" || usd == null) return null;
  return entry.costEstimated ? <CostText estimate={{ usd, pricesDate: entry.pricesDate, offPeak: entry.offPeak }} /> : <CostText exact={usd} />;
}
