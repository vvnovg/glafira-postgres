import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { api, toAppError } from "../api/client";
import type { AppError, McpStatus } from "../api/types";

/** Событие статуса MCP из Rust (задача 8): статус целиком при каждом изменении. */
export const MCP_STATUS_EVENT = "mcp://state";
export const MCP_STATUS_KEY = ["mcpStatus"] as const;
/** Ключ журнала MCP раздела «Настроек»: действия раздела (отзыв, отключение, выключатель) его перечитывают. */
export const MCP_AUDIT_KEY = ["mcpAudit"] as const;

/**
 * Статус MCP-шлюза: первое значение — командой `get_mcp_status`, дальше его заменяют события (`useMcpEvents` в Layout).
 * `status` undefined — ещё не загружен (`loading`) или не загрузился (`error`): потребители считают его неизвестным и
 * ничего не утверждают о политике и сервере (строгий учёт, спец. P4 §11).
 */
export function useMcpStatus(): { status: McpStatus | undefined; loading: boolean; error: AppError | null; refetch: () => void } {
  const q = useQuery({ queryKey: MCP_STATUS_KEY, queryFn: () => api.getMcpStatus(), staleTime: Infinity });
  return { status: q.data, loading: q.isPending, error: q.error ? toAppError(q.error) : null, refetch: () => void q.refetch() };
}

/**
 * Подписка на `mcp://state` — одна на приложение (Layout). Событие новее любого ответа, запрошенного до него: загрузка,
 * которая ещё идёт, отменяется с откатом до записи (как `supersede` в useConnectionEvents), иначе запоздавший ответ
 * затёр бы событие, а при `staleTime: Infinity` это осталось бы до следующего события. Вне Tauri (тесты, браузер)
 * подписка отклоняется — это не ошибка.
 */
export function useMcpEvents(): void {
  const qc = useQueryClient();
  useEffect(() => {
    let unmounted = false;
    let off: (() => void) | undefined;
    listen<McpStatus>(MCP_STATUS_EVENT, (e) => {
      void qc.cancelQueries({ queryKey: MCP_STATUS_KEY, exact: true }, { revert: true });
      qc.setQueryData(MCP_STATUS_KEY, e.payload);
    })
      .then((unlisten) => (unmounted ? unlisten() : (off = unlisten)))
      .catch(() => {});
    return () => {
      unmounted = true;
      off?.();
    };
  }, [qc]);
}
