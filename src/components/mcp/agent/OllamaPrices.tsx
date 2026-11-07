import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { OllamaPrices as Prices } from "../../../api/types";
import { formatDay } from "../../../lib/format";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Hint } from "../../common/Hint";

/** Действующая таблица цен Ollama Cloud в кэше запросов. */
export const OLLAMA_PRICES_KEY = ["ollamaPrices"] as const;
/** Сколько видно итог «Обновить модели и цены». */
export const UPDATED_NOTE_MS = 5000;

/** Итог последнего нажатия: перечитан ли список моделей и сколько моделей в загруженной таблице цен (null — цены не загружались). */
interface Updated {
  models: boolean;
  prices: number | null;
}

/**
 * «Обновить модели и цены» — сразу под полем «Модель» (смоук P4c, R25, решение владельца): одно нажатие загружает цены Ollama
 * Cloud (ollama.com/pricing, только по этой кнопке) и перечитывает список моделей (`refreshModels`: настройки Qwen Code,
 * `/api/tags` Ollama) — после цен, потому что подписи группы «Ollama Cloud» берут цены из действующей таблицы. Рядом — день
 * цен действующей таблицы (встроенной или загруженной), по которой «Запуски» оценивают облачные запуски через Ollama.
 * Политика без обновлений (`updatesEnabled = false`) цены не загружает: кнопка перечитывает только список, пояснение связано с
 * ней `aria-describedby`. Пока идёт обновление, кнопка `aria-disabled` (фокус остаётся). Итог — в живой области рядом с кнопкой,
 * виден `UPDATED_NOTE_MS` и до следующей правки в разделе (`changed` — снимок черновика), как «Сохранено». Ошибка загрузки
 * цен — под кнопкой; прежняя таблица остаётся, а список всё равно перечитывается.
 */
export function OllamaPrices({ changed, refreshModels }: { changed?: string; refreshModels: () => Promise<{ isError: boolean }> }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const noteId = useId();
  // Объект — чтобы срок показа шёл заново и при том же итоге.
  const [updated, setUpdated] = useState<Updated | null>(null);
  const prices = useQuery({ queryKey: OLLAMA_PRICES_KEY, queryFn: () => api.getOllamaPrices() });
  const allowed = prices.data?.refreshAllowed === true;
  const refresh = useMutation({
    mutationFn: async (): Promise<Updated> => {
      let next: Prices | null = null;
      let models = false;
      try {
        if (allowed) {
          next = await api.refreshOllamaPrices();
          qc.setQueryData(OLLAMA_PRICES_KEY, next);
        }
      } finally {
        models = !(await refreshModels()).isError;
      }
      return { models, prices: next?.models ?? null };
    },
    onMutate: () => setUpdated(null),
    onSuccess: setUpdated,
  });
  useEffect(() => {
    if (updated === null) return;
    const id = window.setTimeout(() => setUpdated(null), UPDATED_NOTE_MS);
    return () => window.clearTimeout(id);
  }, [updated]);
  useEffect(() => setUpdated(null), [changed]);
  const label = t("settings.mcp.agent.refresh");
  const p = prices.data;
  const done = updated
    ? [updated.models ? t("settings.mcp.agent.modelsUpdated") : "", updated.prices !== null ? t("settings.mcp.agent.prices.updated", { n: updated.prices }) : ""]
        .filter((x) => x !== "")
        .join(" · ")
    : "";
  return (
    <>
      <div className="toolbar agent-refresh">
        <button
          type="button"
          aria-disabled={refresh.isPending || undefined}
          aria-describedby={p && !p.refreshAllowed ? noteId : undefined}
          onClick={() => !refresh.isPending && refresh.mutate()}
        >
          {refresh.isPending ? t("settings.mcp.agent.refreshing") : label}
        </button>
        <Hint k="hints.settings.mcp.agent.refresh" label={label} />
        <span role="status" className="muted">
          {done}
        </span>
      </div>
      {prices.isError ? (
        <ErrorBanner error={prices.error} onRetry={() => void prices.refetch()} />
      ) : !p ? (
        <p className="muted agent-note">{t("common.loading")}</p>
      ) : (
        <p className="muted agent-note">{t(p.saved ? "settings.mcp.agent.prices.saved" : "settings.mcp.agent.prices.builtin", { date: formatDay(p.date) })}</p>
      )}
      {p && !p.refreshAllowed && (
        <p id={noteId} className="muted agent-note">
          {t("settings.mcp.agent.prices.policy")}
        </p>
      )}
      {refresh.error && <ErrorBanner error={refresh.error} />}
    </>
  );
}
