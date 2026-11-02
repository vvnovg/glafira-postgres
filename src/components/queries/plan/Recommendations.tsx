import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ExplainParams, ExplainResult, HypoRequest, Recommendation } from "../../../api/types";
import { CopyButton } from "../../common/CopyButton";
import { HypoCheck } from "./HypoCheck";
import { isKnownRecommendation, isTemplate, recommendEffect, recommendEvidence, recommendRisks, recommendTitle } from "./recommendText";

/** Где получен план: на этом же проверяется гипотеза hypopg. */
export interface HypoTarget {
  connectionId: string;
  member: string;
  database: string;
  sql: string;
  params: ExplainParams;
  searchPath: string | null;
}

const SEVERITY_ORDER = { critical: 0, warning: 1, info: 2 } as const;

/** Ключи подписей важности — литералами (правило словарей); те же, что у находок. */
const SEVERITY_KEYS = { critical: "diag.severity.critical", warning: "diag.severity.warning", info: "diag.severity.info" } as const;

/**
 * Блок рекомендаций под сводкой плана (спец. P3 §9.1): карточка на рекомендацию — важность и заголовок, доказательство
 * (цифры узла), команда с копированием, эффект, риски, «Проверить гипотезу» (hypopg, если у рекомендации есть DDL
 * виртуального индекса) и «Показать узел» (`onShowNode` получает все `nodeIds`: первый — узел совета, он выбирается, остальные
 * помечаются). Команды приложение не выполняет: только показывает и копирует; `<columns>` / `<work_mem>` в команде — шаблон,
 * его заполняет пользователь. Правило без текстов в словаре (бэкенд новее приложения) — карточка с id и командой. Без
 * ANALYZE правила, которым нужен факт, пропущены — блок говорит, сколько их. У сгруппированной по партициям рекомендации
 * доказательство называет первые партиции, полный список — в разворачиваемом блоке; условие-диапазон (`rewrite`) вместо
 * выражения над колонкой — отдельным блоком с копированием.
 * `target` — подключение, узел, база, SQL, параметры и `search_path`, с которыми получен план: гипотеза проверяется на них же.
 * У вставленного плана («Вставить план») его нет: советы те же, а «Проверить гипотезу» (нужен сервер) скрыта, и подсказка о
 * пропущенных правилах просит вставить план с ANALYZE, а не выполнить его.
 */
export function Recommendations({
  target,
  result,
  onShowNode,
}: {
  target: HypoTarget | null;
  result: Pick<ExplainResult, "recommendations" | "recommendationsSkipped">;
  onShowNode: (ids: number[]) => void;
}) {
  const { t } = useTranslation();
  const [hypo, setHypo] = useState<HypoRequest | null>(null);
  const recs = [...result.recommendations].sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity]);
  const check = (r: Recommendation) => {
    if (r.hypo && target) {
      const { member, database, sql, params, searchPath } = target;
      setHypo({ member, database, sql, params, searchPath, indexDdl: r.hypo.indexDdl });
    }
  };
  const skipped = result.recommendationsSkipped.length;
  return (
    <section className="recommendations" aria-label={t("recommend.title")}>
      <h3>
        {t("recommend.title")}
        {recs.length > 0 && <span className="muted"> ({recs.length})</span>}
      </h3>
      {recs.length === 0 && <p className="muted">{t("recommend.none")}</p>}
      {skipped > 0 && <p className="diag-note">{target ? t("recommend.skipped", { count: skipped }) : t("recommend.skippedPasted", { count: skipped })}</p>}
      {recs.length > 0 && (
        <ul className="recommend-list">
          {recs.map((r, i) => {
            const known = isKnownRecommendation(r);
            return (
              <li key={`${r.id}:${r.nodeIds.join(",")}:${i}`} className={`finding recommend sev-${r.severity}`}>
                <div className="finding-main">
                  <span className="finding-sev">{t(SEVERITY_KEYS[r.severity])}</span>
                  <h4>{known ? recommendTitle(r) : t("recommend.unknown.title", { id: r.id })}</h4>
                </div>
                <p>{known ? recommendEvidence(r) : t("recommend.unknown.evidence")}</p>
                {known && typeof r.params.tablesMore === "number" && r.params.tablesMore > 0 && typeof r.params.tables === "string" && (
                  <details className="recommend-partitions">
                    <summary>{t("recommend.partitionsAll", { count: Number(r.params.partitions ?? 0) })}</summary>
                    <p className="muted">{r.params.tables}</p>
                  </details>
                )}
                {known && typeof r.params.rewrite === "string" && (
                  <>
                    <p className="muted">{t("recommend.rewrite", { condition: String(r.params.condition ?? "") })}</p>
                    {typeof r.params.periodStart === "string" && <p className="muted">{t("recommend.rewritePeriodStart", { value: r.params.periodStart })}</p>}
                    <div className="diag-command">
                      <pre className="query-text">
                        <code>{r.params.rewrite}</code>
                      </pre>
                      <CopyButton text={r.params.rewrite} label={t("recommend.copyCondition")} />
                    </div>
                  </>
                )}
                {r.command && (
                  <>
                    <div className="diag-command">
                      <pre className="query-text">
                        <code>{r.command}</code>
                      </pre>
                      <CopyButton text={r.command} label={t("recommend.copyCommand")} />
                    </div>
                    <p className="muted">{isTemplate(r.command) ? `${t("recommend.notRun")}. ${t("recommend.template")}` : t("recommend.notRun")}</p>
                  </>
                )}
                {known && (
                  <>
                    <p>
                      <strong>{t("recommend.effect")}:</strong> {recommendEffect(r)}
                    </p>
                    <p className="muted">
                      <strong>{t("recommend.risks")}:</strong> {recommendRisks(r)}
                    </p>
                  </>
                )}
                <div className="finding-actions">
                  {r.hypo && target && (
                    <button type="button" onClick={() => check(r)}>
                      {t("recommend.hypo.check")}
                    </button>
                  )}
                  {r.nodeIds.length > 0 && (
                    <button type="button" className="link" onClick={() => onShowNode(r.nodeIds)}>
                      {t("recommend.showNode")}
                    </button>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {hypo && target && <HypoCheck connectionId={target.connectionId} request={hypo} onClose={() => setHypo(null)} />}
    </section>
  );
}
