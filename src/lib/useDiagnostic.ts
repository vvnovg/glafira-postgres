import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { api, toAppError } from "../api/client";
import type { DiagTool, RustTool } from "../api/types";

/** Интервалы обновления в выборе шапки, секунды; «Пауза» — отдельный пункт (`null`). */
export const DIAG_INTERVALS_S = [2, 5, 10, 30] as const;

/** Окна профиля ожиданий в выборе шапки, секунды (5, 15 и 60 минут — те же, что понимает Rust). */
export const WAITS_WINDOWS_S = [300, 900, 3600] as const;

/** Окно профиля ожиданий по умолчанию — 15 минут. */
export const DEFAULT_WAITS_WINDOW_S = 900;

/** Вкладка `idleTx` читает тот же инструмент Rust, что и `sessions`. */
export const rustTool = (tool: DiagTool): RustTool => (tool === "idleTx" ? "sessions" : tool);

/** Инструмент кластера опрашивает участников сам и узла не берёт (`pulse`, `failover`, `health`). */
export const isClusterTool = (tool: DiagTool) => tool === "pulse" || tool === "failover" || tool === "health";

/** Patroni опрашивается раз в 10 с, как топология; остальное — раз в 5 с (спец. P2 §3.6). */
export const defaultIntervalS = (tool: DiagTool) => (tool === "health" || tool === "failover" ? 10 : 5);

/** Скрытая вкладка и пауза не опрашивают; после отказа во входе — тоже (повтор блокирует учётную запись). */
export function diagRefetchInterval(active: boolean, intervalS: number | null, error: unknown): number | false {
  if (!active || intervalS === null) return false;
  if (error != null && toAppError(error).code === "AUTH_FAILED") return false;
  return intervalS * 1000;
}

/**
 * Отчёт инструмента с периодическим обновлением. `member` — закреплённый узел, `null` — «за лидером»; `live` —
 * подключение открыто (иначе запрос не уходит). Вкладка, снова ставшая видимой, сразу перечитывает данные, которые
 * за время скрытия устарели больше чем на интервал: иначе до первого тика на экране остался бы старый отчёт.
 */
export function useDiagnostic(
  connectionId: string,
  tool: DiagTool,
  member: string | null,
  opts: { active: boolean; intervalS: number | null; windowS?: number; live: boolean },
) {
  const rt = rustTool(tool);
  const q = useQuery({
    queryKey: ["diag", connectionId, rt, member ?? "", opts.windowS ?? 0],
    queryFn: () => api.runDiagnostic(connectionId, { tool: rt, member, ...(opts.windowS ? { windowS: opts.windowS } : {}) }),
    enabled: opts.live,
    // Прошлый ответ остаётся на экране, пока идёт новый и если новый упал («устарело»).
    placeholderData: keepPreviousData,
    refetchInterval: (query) => diagRefetchInterval(opts.active, opts.intervalS, query.state.error),
  });
  const wasActive = useRef(opts.active);
  const { dataUpdatedAt, error, refetch } = q;
  const { active, intervalS, live } = opts;
  useEffect(() => {
    const shown = active && !wasActive.current;
    wasActive.current = active;
    // Пауза, закрытое подключение и отказ во входе отчёт не обновляют: повтор входа блокирует учётную запись.
    // Без данных (`dataUpdatedAt` = 0) читать нечего — это дело первого запроса.
    const ms = diagRefetchInterval(true, intervalS, error);
    if (!shown || !live || dataUpdatedAt === 0 || ms === false) return;
    // Идущий запрос не отменяется: он и принесёт свежие данные.
    if (Date.now() - dataUpdatedAt > ms) void refetch({ cancelRefetch: false });
  }, [active, intervalS, live, dataUpdatedAt, error, refetch]);
  return q;
}
