import { useTranslation } from "react-i18next";
import type { PlanNode, PlanTree, ProposalComparison, Recommendation } from "../../../api/types";
import { currentLocale } from "../../../i18n";
import { formatTime } from "../../../lib/format";
import { nodeLabel } from "../plan/planText";
import { isKnownRecommendation, recommendTitle } from "../plan/recommendText";

/** Стоимость планировщика: до двух знаков после запятой, без хвостовых нулей (как в окне проверки гипотезы). */
const cost = (v: number) => v.toLocaleString(currentLocale(), { maximumFractionDigits: 2 });

/** Горячие узлы плана в порядке обхода: без ANALYZE pg-advisor отмечает их по стоимости. */
function hotNodes(n: PlanNode, out: PlanNode[] = []): PlanNode[] {
  if (n.hot) out.push(n);
  for (const c of n.children) hotNodes(c, out);
  return out;
}

function PlanSide({ title, tree, recs }: { title: string; tree: PlanTree; recs: Recommendation[] }) {
  const { t } = useTranslation();
  const hot = hotNodes(tree.root);
  return (
    <section className="llm-compare-side" aria-label={title}>
      <h5>{title}</h5>
      <p className="muted">{t("llmAssist.compare.hot")}</p>
      {hot.length === 0 ? (
        <p className="muted">{t("llmAssist.compare.noHot")}</p>
      ) : (
        <ul className="plain-list">
          {hot.map((n) => (
            <li key={n.id}>
              <span className="mono">{nodeLabel(n)}</span> <span className="muted">{t("llmAssist.compare.nodeCost", { cost: cost(n.totalCost) })}</span>
            </li>
          ))}
        </ul>
      )}
      <p className="muted">{t("llmAssist.compare.recs")}</p>
      {recs.length === 0 ? (
        <p className="muted">{t("llmAssist.compare.noRecs")}</p>
      ) : (
        <ul className="plain-list">
          {recs.map((r, i) => (
            <li key={`${r.id}:${i}`}>{isKnownRecommendation(r) ? recommendTitle(r) : t("recommend.unknown.title", { id: r.id })}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * Итог «Сравнить планы» (спец. P4 §8.4): стоимость «была / стала», узел и время сравнения, использован ли каждый индекс
 * предложения (виртуальный, hypopg), пометка «без hypopg», горячие узлы и рекомендации P3b обоих планов рядом. Планы —
 * EXPLAIN без выполнения: времён и долей в них нет.
 */
export function ProposalCompare({ comparison: c }: { comparison: ProposalComparison }) {
  const { t } = useTranslation();
  return (
    <section className="llm-compare" aria-label={t("llmAssist.compare.title")}>
      <h4>{t("llmAssist.compare.title")}</h4>
      <p>
        {t("llmAssist.compare.cost", { before: cost(c.costBefore), after: cost(c.costAfter) })}{" "}
        <span className="muted">{t("llmAssist.compare.on", { member: c.member, time: formatTime(c.atMs) })}</span>
      </p>
      {c.withoutHypopg && (
        <p className="diag-warn">
          {t("llmAssist.compare.withoutHypopg")}
        </p>
      )}
      {c.indexesUsed.length > 0 && (
        <ul className="plain-list">
          {c.indexesUsed.map((u, i) => (
            <li key={`${u.ddl}:${i}`}>
              <code>{u.ddl}</code> <span className={u.used ? "hypo-used" : "diag-warn"}>{u.used ? t("recommend.hypo.used") : t("recommend.hypo.notUsed")}</span>
            </li>
          ))}
        </ul>
      )}
      <div className="llm-compare-cols">
        <PlanSide title={t("llmAssist.compare.before")} tree={c.planBefore} recs={c.recommendationsBefore} />
        <PlanSide title={t("llmAssist.compare.after")} tree={c.planAfter} recs={c.recommendationsAfter} />
      </div>
    </section>
  );
}
