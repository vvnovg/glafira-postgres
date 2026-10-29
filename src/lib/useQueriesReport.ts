import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "../api/client";
import type { DiagReport, QueriesData, QueriesParams } from "../api/types";
import { diagRefetchInterval } from "./useDiagnostic";

export type QueriesReport = DiagReport<{ kind: "queries" } & QueriesData>;

/** Ключ запроса отчёта: первый элемент `diag` — его сбрасывает `refreshAfterConnect`. */
export const queriesKey = (connectionId: string, member: string | null, params: QueriesParams) => ["diag", connectionId, "queries", member ?? "", params] as const;

/**
 * Отчёт «Запросы» (спец. P3 §3.2): «с момента сброса» перечитывается по интервалу шапки, «за окно» — по тику сэмплера
 * (`tickAtMs`), потому что без нового тика буфер не меняется.
 */
export function useQueriesReport(
  connectionId: string,
  member: string | null,
  params: QueriesParams,
  opts: { active: boolean; live: boolean; intervalS: number | null; tickAtMs: number | null },
) {
  const window = params.mode.kind === "window";
  const q = useQuery({
    queryKey: queriesKey(connectionId, member, params),
    queryFn: async () => (await api.runDiagnostic(connectionId, { tool: "queries", member, queries: params })) as QueriesReport,
    enabled: opts.live,
    // Прошлый ответ остаётся на экране, пока идёт новый и если новый упал («устарело»).
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (window ? false : diagRefetchInterval(opts.active, opts.intervalS, query.state.error)),
  });
  const { refetch } = q;
  useEffect(() => {
    if (window && opts.live && opts.active && opts.tickAtMs !== null) void refetch({ cancelRefetch: false });
  }, [window, opts.live, opts.active, opts.tickAtMs, refetch]);
  return q;
}
