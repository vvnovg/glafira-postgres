import { useEffect, useState } from "react";
import { api, toAppError } from "../api/client";
import type { AppError } from "../api/types";
import { useConnectionEpoch } from "../state/connectionEpoch";

/** Последние показания сэмплера: момент тика (часы приложения), активных сессий и ждущих блокировку. */
export interface SamplerLive {
  atMs: number;
  active: number;
  waiting: number;
}

/**
 * Пауза перед повторной подпиской после `stopped`. Она же — защита от цикла: узел, который раз за разом останавливает
 * сэмплер, получает не больше одной подписки за паузу; заодно переоткрывающееся подключение успевает открыться.
 */
export const RESUBSCRIBE_DELAY_MS = 2000;

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
 * `stopped` не несёт причины. При закрытии и повторном открытии подключения тело вкладки размонтируется и подписывается
 * заново само; «Переподключить» живого подключения `stopped` не шлёт. Остаются исчезновение участника и отклонённая
 * запоздавшая подписка: вкладка при этом остаётся на экране, и без новой подписки она осталась бы мёртвой. Поэтому, пока
 * вкладка включена, через `RESUBSCRIBE_DELAY_MS` она подписывается заново. Исключение — отказ во входе (`AUTH_FAILED`,
 * который сэмплер присылает событием `error` перед `stopped`): повтор подбирал бы пароль (LDAP, fail2ban), как и повтор
 * отчётов (`diagRefetchInterval`). Эту остановку снимает только пользователь: «Переподключить» (и «Подключиться») через
 * `refreshAfterConnect` увеличивает номер подключения (`useConnectionEpoch`), и подписка перезапускается, как перечитываются
 * отчёты. Отказ самой повторной подписки (подключение ещё не открыто, участника нет) — ошибка хука, как у первой подписки;
 * новых попыток нет.
 */
export function useSampler(connectionId: string, member: string | null, enabled: boolean): { live: SamplerLive | null; error: AppError | null } {
  const [live, setLive] = useState<SamplerLive | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  // Номер подписки: растёт, когда сэмплер остановлен не по отказу во входе, и перезапускает эффект.
  const [round, setRound] = useState(0);
  // Номер подключения: растёт, когда пользователь его подключил или переподключил.
  const epoch = useConnectionEpoch(connectionId);
  useEffect(() => {
    if (!enabled || !member) return;
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
      .startSampler(connectionId, member, (e) => {
        if (cancelled || stopped) return;
        if (e.kind === "tick") {
          setLive({ atMs: e.atMs, active: e.active, waiting: e.waiting });
          // Тики идут снова — сэмплер оправился от прежней ошибки.
          lastError = null;
          setError(null);
        } else if (e.kind === "error") {
          // Пока сэмплер не отвечает, прежние цифры были бы неправдой.
          setLive(null);
          lastError = e.error;
          setError(e.error);
        } else if (e.kind === "stopped") {
          stopped = true;
          setLive(null);
          if (lastError?.code !== "AUTH_FAILED") retry = setTimeout(() => setRound((n) => n + 1), RESUBSCRIBE_DELAY_MS);
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
