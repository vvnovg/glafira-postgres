import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { api } from "../api/client";
import { diagRefetchInterval } from "./useDiagnostic";

/** Экземпляры перечитываются раз в 5 с, пока вкладка видна (спец. P3 §7.3). */
const RUNNING_INTERVAL_S = 5;

/** Ключ карточки: первый элемент `queryDetail` — в `PG_QUERY_KEYS` (отключение удаляет, переподключение перечитывает). */
export const queryDetailKey = (connectionId: string, member: string, database: string, queryId: string) => ["queryDetail", connectionId, member, database, queryId] as const;

/**
 * Карточка запроса: перечитывается по интервалу (экземпляры) и по тику сэмплера pgss (`tickAtMs` — история и окна).
 * `live` — подключение открыто (иначе запрос не уходит; по умолчанию — открыто). После отказа во входе останавливаются и
 * интервал, и чтение по тику (в том числе при возвращении на вкладку), как у отчётов диагностики: повтор входа блокирует
 * учётную запись. Снимает остановку «Переподключить» (перечитывание ключей `PG_QUERY_KEYS`).
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
  // Отказ во входе читается из ссылки, а не из зависимостей эффекта: снятие отказа удачным чтением не должно запускать
  // ещё одно чтение.
  const locked = useRef(false);
  locked.current = diagRefetchInterval(true, RUNNING_INTERVAL_S, q.error) === false;
  useEffect(() => {
    if (tickAtMs !== null && active && live && !locked.current) void refetch({ cancelRefetch: false });
  }, [tickAtMs, active, live, refetch]);
  return q;
}
