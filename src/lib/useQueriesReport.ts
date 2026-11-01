import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { api } from "../api/client";
import type { DiagReport, QueriesData, QueriesParams } from "../api/types";
import { diagRefetchInterval } from "./useDiagnostic";

export type QueriesReport = DiagReport<{ kind: "queries" } & QueriesData>;

/** Ключ запроса отчёта: первый элемент `diag` — его сбрасывает `refreshAfterConnect`. */
export const queriesKey = (connectionId: string, member: string | null, params: QueriesParams) => ["diag", connectionId, "queries", member ?? "", params] as const;

/**
 * Отчёт «Запросы» (спец. P3 §3.2): «с момента сброса» перечитывается по интервалу шапки, «за окно» — по тику сэмплера
 * (`tickAtMs`), потому что без нового тика буфер не меняется. «Пауза» (`intervalS === null`) останавливает оба: и
 * интервал, и тики. Вкладка, снова ставшая видимой, догоняет: окно — по последнему тику (эффект тика зависит от
 * `active`), «с момента сброса» — если данные устарели больше чем на интервал, как у `useDiagnostic`. «Между снимками»
 * читается один раз (снимки на диске не меняются) и без открытого подключения.
 */
export function useQueriesReport(
  connectionId: string,
  member: string | null,
  params: QueriesParams,
  opts: { active: boolean; live: boolean; intervalS: number | null; tickAtMs: number | null },
) {
  const byTick = params.mode.kind === "window";
  // «Между снимками» читается с диска: подключение не нужно, а данные не меняются — без периодического обновления.
  const between = params.mode.kind === "between";
  const q = useQuery({
    queryKey: queriesKey(connectionId, member, params),
    queryFn: async () => (await api.runDiagnostic(connectionId, { tool: "queries", member, queries: params })) as QueriesReport,
    enabled: opts.live || between,
    // Прошлый ответ остаётся на экране, пока идёт новый и если новый упал («устарело»).
    placeholderData: keepPreviousData,
    refetchInterval: (query) => (byTick || between ? false : diagRefetchInterval(opts.active, opts.intervalS, query.state.error)),
  });
  const { refetch, dataUpdatedAt, error } = q;
  const { active, live, intervalS, tickAtMs } = opts;
  const paused = intervalS === null;
  // Отказ во входе (`AUTH_FAILED`) останавливает и чтение по тику — и при возвращении на вкладку: повтор входа блокирует
  // учётную запись. Читается из ссылки, чтобы удачное чтение, снявшее отказ, не запускало ещё одно.
  const locked = useRef(false);
  locked.current = diagRefetchInterval(true, intervalS ?? 1, error) === false;
  useEffect(() => {
    if (byTick && live && active && !paused && tickAtMs !== null && !locked.current) void refetch({ cancelRefetch: false });
  }, [byTick, live, active, paused, tickAtMs, refetch]);
  const wasActive = useRef(active);
  useEffect(() => {
    const shown = active && !wasActive.current;
    wasActive.current = active;
    // Пауза, закрытое подключение и отказ во входе отчёт не обновляют; без данных читать нечего — это дело первого запроса.
    const ms = diagRefetchInterval(true, intervalS, error);
    if (byTick || between || !shown || !live || dataUpdatedAt === 0 || ms === false) return;
    // Идущий запрос не отменяется: он и принесёт свежие данные.
    if (Date.now() - dataUpdatedAt > ms) void refetch({ cancelRefetch: false });
  }, [byTick, between, active, intervalS, live, dataUpdatedAt, error, refetch]);
  return q;
}
