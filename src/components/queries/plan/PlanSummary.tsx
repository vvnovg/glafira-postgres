import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainMode, ExplainTier, PlanTree } from "../../../api/types";
import { formatMs } from "../../../lib/format";
import { CopyButton } from "../../common/CopyButton";
import { renderText } from "./planText";

/** Ключи предупреждений результата EXPLAIN — литералами (правило словарей). */
const WARNING_KEYS: Record<string, string> = {
  genericPlan: "diag.plan.warnings.genericPlan",
  probeWithoutReadOnly: "diag.plan.warnings.probeWithoutReadOnly",
  adminFunction: "diag.plan.warnings.adminFunction",
};

/** Режим запуска — подписями кнопок запуска (литералами: правило словарей). */
const MODE_KEYS: Record<ExplainMode, string> = { plan: "diag.plan.run.plan", analyze: "diag.plan.run.analyze" };

/** Предупреждение — справка, а не ограничение: нейтральная заметка; остальные — предупреждения. */
const NOTE_WARNINGS = new Set(["genericPlan"]);

/**
 * Сводка плана: режим запуска (если известен), узел (если план получен не на узле вкладки), планирование, выполнение, ярус, JIT, триггеры, settings,
 * предупреждения; «JSON» и «Как текст» — только копирование плана (кнопки запроса — в ряду формы `PlanRunner`, смоук P4b R1b).
 */
export function PlanSummary({ tree, tier, mode, warnings = [], raw, elapsedMs, member }: { tree: PlanTree; tier?: ExplainTier; mode?: ExplainMode; warnings?: string[]; raw?: unknown; elapsedMs?: number; member?: string }) {
  const { t } = useTranslation();
  // Текст и JSON большого плана собираются один раз на план, а не при каждой перерисовке сводки.
  const json = useMemo(() => (raw === undefined ? null : JSON.stringify(raw, null, 2)), [raw]);
  const text = useMemo(() => renderText(tree), [tree]);
  const parts = [
    mode !== undefined && t(MODE_KEYS[mode]),
    member !== undefined && t("diag.plan.summary.node", { member }),
    tree.planningMs !== null && t("diag.plan.summary.planning", { ms: formatMs(tree.planningMs) }),
    tree.executionMs !== null && t("diag.plan.summary.execution", { ms: formatMs(tree.executionMs) }),
    tier && t("diag.plan.summary.tier", { tier: t(`diag.plan.tiers.${tier}`) }),
    elapsedMs !== undefined && t("diag.plan.summary.elapsed", { ms: formatMs(elapsedMs) }),
    tree.jit &&
      (tree.jit.totalMs === null
        ? t("diag.plan.summary.jitFunctions", { functions: tree.jit.functions })
        : t("diag.plan.summary.jit", { functions: tree.jit.functions, ms: formatMs(tree.jit.totalMs) })),
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
        <p key={w} className={NOTE_WARNINGS.has(w) ? "diag-note" : "diag-warn"} role="status">{WARNING_KEYS[w] ? t(WARNING_KEYS[w]) : w}</p>
      ))}
      <div className="toolbar">
        {/* Кнопки копирования одинаково подписаны «Копировать»: что именно копируется, говорит подпись рядом (имя кнопки для скринридера — полное). */}
        {json !== null && (
          <span className="plan-copy">
            <span className="muted">{t("diag.plan.asJson")}</span>
            <CopyButton text={json} label={t("diag.plan.copyJson")} />
          </span>
        )}
        <span className="plan-copy">
          <span className="muted">{t("diag.plan.asText")}</span>
          <CopyButton text={text} label={t("diag.plan.copyText")} />
        </span>
      </div>
    </section>
  );
}
