import type { UseQueryResult } from "@tanstack/react-query";
import { type ReactNode, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { ConnState } from "../../api/types";
import { useTopology } from "../../lib/useConnectionEvents";
import { ErrorBanner } from "../common/ErrorBanner";

const isLive = (state: ConnState) => state === "connected" || state === "degraded";

/**
 * Общая обёртка вкладки объекта: заголовок и вид объекта, кнопка «Обновить», загрузка, ошибка с повтором, тело по готовности данных.
 * connectionId — подключение объекта: когда оно снова становится живым, вкладка читает данные заново.
 */
export function ObjectTab<T>({
  connectionId,
  title,
  kind,
  query,
  children,
}: {
  connectionId: string;
  title: string;
  kind: string;
  query: UseQueryResult<T>;
  children: (data: T) => ReactNode;
}) {
  const { t } = useTranslation();
  const state = useTopology(connectionId).data?.state;
  const prevState = useRef(state);
  // Вкладки скрываются, но остаются смонтированными. Отключение удаляет запрос из кэша, а наблюдатель вкладки держит
  // удалённый: на экране остаются данные до отключения, и сброс кэша при подключении (refreshAfterConnect) до него не
  // дотягивается. Если же чтение случилось, пока подключение закрыто, в кэше остаётся ошибка «нет подключения».
  // Возврат подключения (из заведомо неживого состояния в живое) перечитывает вкладку в обоих случаях; чтение, которое
  // уже запустил сброс кэша, не дублируется. Пока состояние неизвестно (вид ещё не загружен), перехода нет.
  useEffect(() => {
    const was = prevState.current;
    prevState.current = state;
    if (was !== undefined && !isLive(was) && state !== undefined && isLive(state)) void query.refetch({ cancelRefetch: false });
    // Срабатывает только на смене состояния подключения; query на каждой отрисовке новый объект.
  }, [state]);
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{title}</h2>
        <span className="muted">{kind}</span>
        <button type="button" disabled={query.isFetching} aria-busy={query.isFetching} onClick={() => void query.refetch()}>
          {t("object.refresh")}
        </button>
      </div>
      {query.isPending && (
        <p className="empty" role="status">
          {t("common.loading")}
        </p>
      )}
      {query.error && <ErrorBanner error={query.error} onRetry={() => void query.refetch()} />}
      {query.data !== undefined && children(query.data)}
    </div>
  );
}
