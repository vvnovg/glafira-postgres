import { useQuery } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { HypoRequest, PlanNode } from "../../../api/types";
import { currentLocale } from "../../../i18n";
import { trapTab } from "../../../lib/focusTrap";
import { formatBytes } from "../../../lib/format";
import { ErrorBanner } from "../../common/ErrorBanner";
import { PlanNodeDetails } from "./PlanNodeDetails";
import { PlanTreeView } from "./PlanTree";

function findNode(n: PlanNode, id: number): PlanNode | null {
  if (n.id === id) return n;
  for (const c of n.children) {
    const f = findNode(c, id);
    if (f) return f;
  }
  return null;
}

/** Стоимость планировщика: до двух знаков после запятой, без хвостовых нулей («12 000», «120,5»). */
const cost = (v: number) => v.toLocaleString(currentLocale(), { maximumFractionDigits: 2 });

/**
 * Окно проверки гипотезы (спец. P3 §9.2): DDL виртуального индекса, стоимость «была / стала», использован ли индекс, оценка
 * размера и план «после». Сервер ничего не создаёт: hypopg держит индекс в памяти сессии проверки (Rust — `hypo_check`).
 * Модальное: фокус при открытии — на «Закрыть», Tab ходит по кругу внутри, Escape закрывает, фокус возвращается туда, где был.
 */
export function HypoCheck({ connectionId, request, onClose }: { connectionId: string; request: HypoRequest; onClose: () => void }) {
  const { t } = useTranslation();
  const titleId = useId();
  const q = useQuery({ queryKey: ["hypo", connectionId, request], queryFn: () => api.hypoCheck(connectionId, request), retry: false, staleTime: Infinity, gcTime: 0 });
  const [selected, setSelected] = useState<number | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    return () => {
      if (returnTo?.isConnected) returnTo.focus();
    };
  }, []);
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Tab") trapTab(rootRef.current, e);
    if (e.key === "Escape") onClose();
  };
  const r = q.data;
  const node = r && selected !== null ? findNode(r.planAfter.root, selected) : null;
  return (
    <div className="modal-backdrop">
      <div ref={rootRef} role="dialog" aria-modal="true" aria-labelledby={titleId} className="modal hypo-check" onKeyDown={onKeyDown}>
        <h3 id={titleId}>{t("recommend.hypo.title")}</h3>
        <p className="muted">{t("recommend.hypo.intro")}</p>
        <pre className="query-text">
          <code>{request.indexDdl}</code>
        </pre>
        {q.isLoading && <p className="muted">{t("common.loading")}</p>}
        {q.error && <ErrorBanner error={q.error} />}
        {r && (
          <>
            <p>
              {t("recommend.hypo.cost", { before: cost(r.costBefore), after: cost(r.costAfter) })}
              {" · "}
              <span className={r.used ? "hypo-used" : "diag-warn"}>{r.used ? t("recommend.hypo.used") : t("recommend.hypo.notUsed")}</span>
              {" · "}
              {t("recommend.hypo.size", { size: formatBytes(r.estimatedBytes) })}
            </p>
            <h4>{t("recommend.hypo.planAfter")}</h4>
            <PlanTreeView tree={r.planAfter} selectedId={selected} onSelect={setSelected} />
            {node && <PlanNodeDetails node={node} />}
          </>
        )}
        <div className="toolbar">
          <button ref={closeRef} type="button" onClick={onClose}>
            {t("common.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
