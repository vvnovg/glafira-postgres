import { useEffect, useState } from "react";
import { api, toAppError } from "../api/client";
import type { AppError } from "../api/types";
import { useConnectionEpoch } from "../state/connectionEpoch";
import { RESUBSCRIBE_DELAY_MS } from "./useSampler";

/** Последний тик сэмплера pgss: покрытие часового окна и число тиков с дельтами. */
export interface StatementsLive {
  atMs: number;
  coverageS: number;
  ticks: number;
}

/** Отписка. Сбой (сессии уже нет) вкладке не важен: сэмплер и так остановлен, поэтому ошибка не всплывает. */
function stop(session: string) {
  void Promise.resolve(api.stopSession(session)).catch(() => {});
}

/**
 * Подписка на сэмплер pg_stat_statements узла (спец. P3 §5): та же дисциплина, что у `useSampler` (держится, пока
 * вкладка смонтирована; после `stopped` — повтор через паузу, кроме отказа во входе и отсутствия расширения;
 * «Переподключить» перезапускает подписку). `member = null` — «за лидером»: узел выбирает Rust.
 */
export function useStatementsSampler(connectionId: string, member: string | null, enabled: boolean): { live: StatementsLive | null; error: AppError | null } {
  const [live, setLive] = useState<StatementsLive | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  // Номер подписки: растёт, когда сэмплер остановлен не по отказу во входе, и перезапускает эффект.
  const [round, setRound] = useState(0);
  // Номер подключения: растёт, когда пользователь его подключил или переподключил.
  const epoch = useConnectionEpoch(connectionId);
  useEffect(() => {
    if (!enabled) return;
    let session: string | null = null;
    let cancelled = false;
    let stopped = false;
    // Последняя ошибка этой подписки (сбрасывается тиком): по ней `stopped` отличается отказом во входе.
    let lastError: AppError | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    // Показания и ошибка прежнего узла к новому не относятся.
    setLive(null);
    setError(null);
    api
      .startStatementsSampler(connectionId, member, (e) => {
        if (cancelled || stopped) return;
        if (e.kind === "tick") {
          setLive({ atMs: e.atMs, coverageS: e.coverageS, ticks: e.ticks });
          lastError = null;
          setError(null);
        } else if (e.kind === "error") {
          lastError = e.error;
          setError(e.error);
        } else if (e.kind === "stopped") {
          stopped = true;
          if (lastError?.code !== "AUTH_FAILED" && lastError?.code !== "FEATURE_UNAVAILABLE") retry = setTimeout(() => setRound((n) => n + 1), RESUBSCRIBE_DELAY_MS);
        }
      })
      .then((id) => {
        if (cancelled) stop(id);
        else session = id;
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(toAppError(e));
      });
    return () => {
      cancelled = true;
      clearTimeout(retry);
      if (session) stop(session);
    };
  }, [connectionId, member, enabled, round, epoch]);
  return { live, error };
}
