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

  // Ход для скринридера — одна вежливая область `role="status"`: пока идёт — последний пройденный шаг, в конце — «Готово»,
  // срок вышел или причина сбоя. Список шагов меняется молча, без области конец наблюдения не был бы объявлен. Срок и сбой
  // видны и глазами (как раньше), остальное область говорит только скринридеру: на экране это уже показывают отметки.
  const ending = last?.status === "timedOut" || last?.status === "failed";
  const statusText = (): string => {
    if (!last) return "";
    switch (last.status) {
      case "done":
        return t(STEP_KEYS.done);
      case "timedOut":
        return t("actions.watch.timedOut");
      case "failed":
        return last.detail ? tr(last.detail) : t("actions.watch.failed");
      default:
        return t("actions.watch.current", { step: t(STEP_KEYS[last.step]) });
    }
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
      <p className={ending ? "diag-warn" : "sr-only"} role="status" aria-live="polite">
        {statusText()}
      </p>
      <button type="button" onClick={close}>
        {t("common.close")}
      </button>
    </section>
  );
}
