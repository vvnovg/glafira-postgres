import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import { formatDay } from "../../../lib/format";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Hint } from "../../common/Hint";
import { Icon } from "../../common/Icon";

/** Действующая таблица цен Ollama Cloud в кэше запросов. */
export const OLLAMA_PRICES_KEY = ["ollamaPrices"] as const;

/**
 * Цены Ollama Cloud, по которым «Запуски» оценивают облачные запуски через Ollama (смоук P4c, sf-T6): день цен действующей
 * таблицы (встроенной или загруженной) и «Обновить цены». Glafira загружает ollama.com/pricing только по этой кнопке; пока
 * загрузка идёт, кнопка `aria-disabled` (фокус остаётся). Политика без обновлений (`updatesEnabled = false`) — кнопка
 * неактивна с пояснением. Итог — в живой области рядом с кнопкой, ошибка — под ней; прежняя таблица при ошибке остаётся.
 * Рисуется внутри блока «Запусков» (`settings-block`) соседними элементами.
 */
export function OllamaPrices() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const noteId = useId();
  const prices = useQuery({ queryKey: OLLAMA_PRICES_KEY, queryFn: () => api.getOllamaPrices() });
  const refresh = useMutation({
    mutationFn: () => api.refreshOllamaPrices(),
    onSuccess: (next) => qc.setQueryData(OLLAMA_PRICES_KEY, next),
  });
  if (prices.isPending) return <p className="muted">{t("common.loading")}</p>;
  if (prices.isError) return <ErrorBanner error={prices.error} onRetry={() => void prices.refetch()} />;
  const p = prices.data;
  const date = formatDay(p.date);
  const label = t("settings.mcp.agent.prices.refresh");
  return (
    <>
      <p className="muted agent-note">{p.saved ? t("settings.mcp.agent.prices.saved", { date }) : t("settings.mcp.agent.prices.builtin", { date })}</p>
      <div className="toolbar">
        <button
          type="button"
          disabled={!p.refreshAllowed}
          aria-disabled={refresh.isPending || undefined}
          aria-describedby={p.refreshAllowed ? undefined : noteId}
          onClick={() => !refresh.isPending && refresh.mutate()}
        >
          <Icon name="refresh" />
          {refresh.isPending ? t("settings.mcp.agent.prices.refreshing") : label}
        </button>
        <Hint k="hints.settings.mcp.agent.prices" label={label} />
        <span role="status" className="muted">
          {refresh.isSuccess && t("settings.mcp.agent.prices.updated", { n: refresh.data.models })}
        </span>
      </div>
      {!p.refreshAllowed && (
        <p id={noteId} className="muted agent-note">
          {t("settings.mcp.agent.prices.policy")}
        </p>
      )}
      {refresh.error && <ErrorBanner error={refresh.error} />}
    </>
  );
}
