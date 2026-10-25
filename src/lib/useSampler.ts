import { useEffect, useState } from "react";
import { api, toAppError } from "../api/client";
import type { AppError } from "../api/types";

/** Последние показания сэмплера: момент тика (часы приложения), активных сессий и ждущих блокировку. */
export interface SamplerLive {
  atMs: number;
  active: number;
  waiting: number;
}

/** Отписка. Сбой (сессии уже нет) вкладке не важен: сэмплер и так остановлен, поэтому ошибка не всплывает. */
function stop(session: string) {
  void Promise.resolve(api.stopSession(session)).catch(() => {});
}

/**
 * Подписка вкладки на сэмплер узла (спец. P2 §6): держится, пока вкладка смонтирована (в том числе скрытая —
 * история не прерывается). Смена узла — отписка от старого и подписка на новый. Ответ подписки, пришедший после
 * размонтирования, сразу отписывается: иначе сэмплер работал бы без вкладки.
 * События канала принимаются, только пока подписка действует: после `stopped` и после отписки (смена узла,
 * размонтирование) поздний тик или промежуток может ещё прийти, и показания прежнего узла не должны вернуться.
 */
export function useSampler(connectionId: string, member: string | null, enabled: boolean): { live: SamplerLive | null; error: AppError | null } {
  const [live, setLive] = useState<SamplerLive | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  useEffect(() => {
    if (!enabled || !member) return;
    let session: string | null = null;
    let cancelled = false;
    let stopped = false;
    // Показания и ошибка прежнего узла к новому не относятся.
    setLive(null);
    setError(null);
    api
      .startSampler(connectionId, member, (e) => {
        if (cancelled || stopped) return;
        if (e.kind === "tick") {
          setLive({ atMs: e.atMs, active: e.active, waiting: e.waiting });
          // Тики идут снова — сэмплер оправился от прежней ошибки.
          setError(null);
        } else if (e.kind === "error") {
          // Пока сэмплер не отвечает, прежние цифры были бы неправдой.
          setLive(null);
          setError(e.error);
        } else if (e.kind === "stopped") {
          stopped = true;
          setLive(null);
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
      if (session) stop(session);
    };
  }, [connectionId, member, enabled]);
  return { live, error };
}
