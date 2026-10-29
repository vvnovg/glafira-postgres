import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "../api/client";
import { diagRefetchInterval } from "./useDiagnostic";

/** Экземпляры перечитываются раз в 5 с, пока вкладка видна (спец. P3 §7.3). */
const RUNNING_INTERVAL_S = 5;

/** Ключ карточки: первый элемент `queryDetail` — в `PG_QUERY_KEYS` (отключение удаляет, переподключение перечитывает). */
export const queryDetailKey = (connectionId: string, member: string, database: string, queryId: string) => ["queryDetail", connectionId, member, database, queryId] as const;

/**
 * Карточка запроса: перечитывается по интервалу (экземпляры) и по тику сэмплера pgss (`tickAtMs` — история и окна).
 * `live` — подключение открыто (иначе запрос не уходит; по умолчанию — открыто). После отказа во входе интервал
 * останавливается, как у отчётов диагностики: повтор входа блокирует учётную запись.
 */
export function useQueryDetail(connectionId: string, member: string, database: string, queryId: string, opts: { active: boolean; tickAtMs: number | null; live?: boolean }) {
  const live = opts.live ?? true;
  const q = useQuery({
    queryKey: queryDetailKey(connectionId, member, database, queryId),
    queryFn: () => api.queryDetail(connectionId, member, database, queryId),
    enabled: live,
    placeholderData: keepPreviousData,
    refetchInterval: (query) => diagRefetchInterval(opts.active && live, RUNNING_INTERVAL_S, query.state.error),
  });
  const { refetch } = q;
  const { active, tickAtMs } = opts;
  useEffect(() => {
    if (tickAtMs !== null && active && live) void refetch({ cancelRefetch: false });
  }, [tickAtMs, active, live, refetch]);
  return q;
}
