import { useTranslation } from "react-i18next";
import type { ExplainTier, PlanTree } from "../../../api/types";
import { formatMs } from "../../../lib/format";
import { CopyButton } from "../../common/CopyButton";
import { renderText } from "./planText";

/** Ключи предупреждений результата EXPLAIN — литералами (правило словарей). */
const WARNING_KEYS: Record<string, string> = {
  genericPlan: "diag.plan.warnings.genericPlan",
  probeWithoutReadOnly: "diag.plan.warnings.probeWithoutReadOnly",
};

/** Сводка плана: планирование, выполнение, ярус, JIT, триггеры, settings, предупреждения; «JSON» и «Как текст». */
export function PlanSummary({ tree, tier, warnings = [], raw, elapsedMs }: { tree: PlanTree; tier?: ExplainTier; warnings?: string[]; raw?: unknown; elapsedMs?: number }) {
  const { t } = useTranslation();
  const parts = [
    tree.planningMs !== null && t("diag.plan.summary.planning", { ms: formatMs(tree.planningMs) }),
    tree.executionMs !== null && t("diag.plan.summary.execution", { ms: formatMs(tree.executionMs) }),
    tier && t("diag.plan.summary.tier", { tier: t(`diag.plan.tiers.${tier}`) }),
    elapsedMs !== undefined && t("diag.plan.summary.elapsed", { ms: formatMs(elapsedMs) }),
    tree.jit && t("diag.plan.summary.jit", { functions: tree.jit.functions, ms: formatMs(tree.jit.totalMs) }),
  ].filter((p): p is string => typeof p === "string");
  return (
    <section className="plan-summary">
      <p>
        {parts.map((p) => (
          <span key={p} className="plan-summary-part">{p}</span>
        ))}
      </p>
      {tree.triggers.length > 0 && <p className="muted">{tree.triggers.map((tr) => `${tr.name}: ${formatMs(tr.timeMs)} × ${tr.calls}`).join("; ")}</p>}
      {tree.settings.length > 0 && (
        <p className="muted">
          {tree.settings.map((s) => (
            <code key={s.name} className="plan-setting">{s.name} = {s.value}</code>
          ))}
        </p>
      )}
      {warnings.map((w) => (
        <p key={w} className="diag-note" role="status">{WARNING_KEYS[w] ? t(WARNING_KEYS[w]) : w}</p>
      ))}
      <div className="toolbar">
        {/* Кнопки копирования одинаково подписаны «Копировать»: что именно копируется, говорит подпись рядом (имя кнопки для скринридера — полное). */}
        {raw !== undefined && (
          <span className="plan-copy">
            <span className="muted">{t("diag.plan.asJson")}</span>
            <CopyButton text={JSON.stringify(raw, null, 2)} label={t("diag.plan.copyJson")} />
          </span>
        )}
        <span className="plan-copy">
          <span className="muted">{t("diag.plan.asText")}</span>
          <CopyButton text={renderText(tree)} label={t("diag.plan.copyText")} />
        </span>
      </div>
    </section>
  );
}
