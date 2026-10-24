import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { ErrorBanner } from "../common/ErrorBanner";

/** Общая обёртка вкладки объекта: заголовок и вид объекта, загрузка, ошибка с повтором, тело по готовности данных. */
export function ObjectTab<T>({ title, kind, query, children }: { title: string; kind: string; query: UseQueryResult<T>; children: (data: T) => ReactNode }) {
  const { t } = useTranslation();
  return (
    <div className="page">
      <div className="toolbar">
        <h2>{title}</h2>
        <span className="muted">{kind}</span>
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
