import { useCallback, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { PatroniAction, WatchEvent, WatchStep } from "../../api/types";
import { tr } from "../../i18n/text";

/** Шаги наблюдения по действию — как `WatchPlan::steps` в Rust (спец. §8.6). */
export function WATCH_STEPS(kind: PatroniAction["kind"], onlyPending: boolean): WatchStep[] {
  switch (kind) {
    case "switchover":
    case "failover":
      return ["accepted", "leaderStepped", "newLeaderRunning", "timelineIncreased", "replicasStreaming", "done"];
    case "restart":
      return onlyPending ? ["accepted", "restarted", "memberRunning", "pendingCleared", "done"] : ["accepted", "restarted", "memberRunning", "done"];
    default:
      return ["accepted", "pauseApplied", "done"];
  }
}

// Ключи — литералами: проверка словарей (`pnpm i18n:check`) находит только их.
const STEP_KEYS: Record<WatchStep, string> = {
  accepted: "actions.watch.step.accepted",
  leaderStepped: "actions.watch.step.leaderStepped",
  newLeaderRunning: "actions.watch.step.newLeaderRunning",
  timelineIncreased: "actions.watch.step.timelineIncreased",
  replicasStreaming: "actions.watch.step.replicasStreaming",
  restarted: "actions.watch.step.restarted",
  memberRunning: "actions.watch.step.memberRunning",
  pendingCleared: "actions.watch.step.pendingCleared",
  pauseApplied: "actions.watch.step.pauseApplied",
  done: "actions.watch.step.done",
};

interface Props {
  action: PatroniAction["kind"];
  onlyPending?: boolean;
  events: WatchEvent[];
  watchId: string;
  onClose: () => void;
}

/**
 * Ход действия Patroni: шаги до «готово», срок. «Закрыть» и размонтирование панели (закрыта вкладка, открыто другое
 * действие) останавливают наблюдение и ускоренный опрос топологии — каждое наблюдение не больше одного раза.
 */
export function WatchPanel({ action, onlyPending = false, events, watchId, onClose }: Props) {
  const { t } = useTranslation();
  const steps = WATCH_STEPS(action, onlyPending);
  const last = events[events.length - 1];
  const reached = last ? steps.indexOf(last.step) : -1;

  const stopped = useRef(new Set<string>());
  // `stop_session` безопасен и для закончившейся сессии (вернёт false); сбой остановки закрытию не мешает: срок наблюдения
  // в Rust всё равно истечёт сам.
  const stop = useCallback((id: string) => {
    if (stopped.current.has(id)) return;
    stopped.current.add(id);
    void Promise.resolve(api.stopSession(id)).catch(() => {});
  }, []);

  // Размонтирование останавливает наблюдение, но не сразу, а микрозадачей: StrictMode размонтирует компонент и тут же
  // монтирует снова (это не закрытие), и наблюдение, остановленное в этот миг, не доработало бы до конца в режиме разработки.
  const mountedFor = useRef<string | null>(null);
  useEffect(() => {
    mountedFor.current = watchId;
    return () => {
      mountedFor.current = null;
      queueMicrotask(() => {
        if (mountedFor.current !== watchId) stop(watchId);
      });
    };
  }, [watchId, stop]);

  const close = () => {
    stop(watchId);
    onClose();
  };

  return (
    <section className="panel" aria-label={t("actions.watch.title")}>
      <h3>{t("actions.watch.title")}</h3>
      <ol className="steps">
        {steps.map((s, i) => {
          const done = i <= reached;
          return (
            <li key={s} aria-current={i === reached + 1 && last?.status === "running" ? "step" : undefined}>
              <span aria-hidden="true">{done ? "✓ " : "· "}</span>
              {t(STEP_KEYS[s])}
              {done && <span className="sr-only"> — {t("actions.watch.reached")}</span>}
            </li>
          );
        })}
      </ol>
      {last?.status === "timedOut" && (
        <p className="diag-warn" role="status">
          {t("actions.watch.timedOut")}
        </p>
      )}
      {last?.status === "failed" && (
        <p className="diag-warn" role="status">
          {last.detail ? tr(last.detail) : t("actions.watch.failed")}
        </p>
      )}
      <button type="button" onClick={close}>
        {t("common.close")}
      </button>
    </section>
  );
}
