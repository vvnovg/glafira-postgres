import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../api/client";
import type { Confirmation, ConnectionProfile, PatroniAction, PatroniActionPreview, WatchEvent } from "../../api/types";
import { formatScheduled } from "../../lib/format";
import { toRfc3339 } from "../../lib/schedule";
import { isRunningLeader, roleLabel } from "../../lib/topology";
import { ConfirmDialog } from "../common/ConfirmDialog";
import { ErrorBanner } from "../common/ErrorBanner";
import { REASON_KEYS, bytesText } from "../diag/viewText";
import { WatchPanel } from "./WatchPanel";

// Ключи — литералами: проверка словарей (`pnpm i18n:check`) находит только их. «Снятие паузы» — тоже `pause`, но с `paused: false`.
// Заголовок диалога — приказ («Cancel switchover»), а не подпись журнала в прошедшем времени (`actions.kind.*`: «Switchover cancelled»).
const TITLE_KEYS = {
  switchover: "actions.patroni.titles.switchover",
  cancelSwitchover: "actions.patroni.titles.cancelSwitchover",
  failover: "actions.patroni.titles.failover",
  restart: "actions.patroni.titles.restart",
  cancelRestart: "actions.patroni.titles.cancelRestart",
  pause: "actions.patroni.titles.pause",
  resume: "actions.patroni.titles.resume",
} as const;
const titleKey = (a: PatroniAction) => TITLE_KEYS[a.kind === "pause" && !a.paused ? "resume" : a.kind];

/** Время из `datetime-local` в RFC 3339; пустое или неразборчивое — `null` (ввод проверяется до отправки, а не бросает в отрисовке). */
function scheduleOf(local: string): string | null {
  try {
    return toRfc3339(local);
  } catch {
    return null;
  }
}

const isLeaderRole = (role: string) => role === "primary" || role === "standbyLeader";

interface Props {
  connectionId: string;
  profile: ConnectionProfile;
  /** Действие, с которого открыт диалог: его вид и узел; кандидата, время и флажок пользователь выбирает в форме. */
  action: PatroniAction;
  /** Область ключа мутации: пока действие идёт, страница-владелец не открывает над ним другой диалог (`useIsMutating`). */
  scope?: string;
  onClose: () => void;
}

/**
 * Действие Patroni (спец. P2 §8.5): предпросмотр заново читает кластер (лидер, готовность кандидатов, сессии лидера,
 * что запланировано), форма собирает итоговое действие, подтверждение — по матрице §8.1 (failover — вводом имени
 * кандидата). Если Patroni начал наблюдаемое действие, ниже открывается панель хода.
 */
export function PatroniActionDialog({ connectionId, profile, action, scope = "", onClose }: Props) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const rootRef = useRef<HTMLElement>(null);
  // Диалог открывается над таблицей участников, а кнопка могла быть внизу длинного списка: фокус и прокрутка — к нему.
  // При закрытии фокус возвращается на кнопку, открывшую диалог (если она осталась на странице).
  useEffect(() => {
    const returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    rootRef.current?.focus({ preventScroll: true });
    rootRef.current?.scrollIntoView?.({ block: "nearest" });
    return () => {
      if (returnTo?.isConnected) returnTo.focus({ preventScroll: true });
    };
  }, []);

  // Кластер читается один раз и не меняется под рукой у пользователя, пока он подтверждает. Экземпляр диалога входит
  // в ключ: повторное открытие читает кластер заново, а не берёт кэш прошлого диалога.
  const instance = useId();
  const preview = useQuery({
    queryKey: ["patroniPreview", connectionId, action, instance],
    queryFn: () => api.previewPatroniAction(connectionId, action),
    staleTime: Infinity,
    gcTime: 0,
  });
  const p = preview.data;

  // Форма. Что не выбрано (`null`) — берётся по умолчанию из предпросмотра: failover — первый готовый кандидат,
  // «только если нужен» — по отметке участника.
  const [chosen, setChosen] = useState<string | null>(null);
  const [when, setWhen] = useState<"now" | "scheduled">("now");
  const [local, setLocal] = useState("");
  const [pendingOnly, setPendingOnly] = useState<boolean | null>(null);

  // Диалог ещё на экране? Ответ команды может прийти и после того, как его убрали. Ref выставляется в эффекте, поэтому
  // двойной монтаж StrictMode (размонтирование и тут же монтаж) оставляет его истинным.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const [events, setEvents] = useState<WatchEvent[]>([]);
  const run = useMutation({
    mutationKey: ["patroniAction", connectionId, scope],
    mutationFn: ({ a, c }: { a: PatroniAction; c: Confirmation }) =>
      api.executePatroniAction(connectionId, a, c, (e) => {
        setEvents((x) => [...x, e]);
        // Конечное событие наблюдения: вид кластера перечитывается ещё раз (между опросами он мог не успеть обновиться).
        if (e.status !== "running") void qc.invalidateQueries({ queryKey: ["topology", connectionId] });
      }),
    onSuccess: (res) => {
      // Успех и неизвестный исход: кластер мог измениться, а запись о действии появилась в журнале подключения.
      void qc.invalidateQueries({ queryKey: ["topology", connectionId] });
      void qc.invalidateQueries({ queryKey: ["actions", connectionId] });
      // Этот обработчик — в настройках мутации и переживает диалог. Если его убрали, пока команда шла (закрыта вкладка,
      // действия перестали предлагаться), панели хода, которая остановила бы наблюдение, уже не будет: останавливаем здесь,
      // иначе ускоренный опрос шёл бы до конца срока. Сбой остановки не важен: срок в Rust истечёт сам.
      if (!mounted.current && res.watch) void Promise.resolve(api.stopSession(res.watch)).catch(() => {});
    },
    // Отказ Patroni (`actionRejected`) тоже записан в журнал; отказы до отправки (защита, предусловия) — нет, лишнее чтение безвредно.
    onError: () => void qc.invalidateQueries({ queryKey: ["actions", connectionId] }),
  });

  // Подтверждение исчезает вместе с появлением итога, и фокус с его кнопки уходит на body: возвращаем его в диалог,
  // чтобы Escape и Tab продолжали работать отсюда. Фокус, который пользователь уже унёс в другое место, не отбирается.
  useEffect(() => {
    if (!run.data) return;
    const active = document.activeElement;
    if (active === null || active === document.body || rootRef.current?.contains(active)) rootRef.current?.focus({ preventScroll: true });
  }, [run.data]);

  // Escape закрывает диалог, где бы ни стоял фокус внутри него, — не только внутри группы подтверждения. Перехват в фазе
  // погружения: Escape в группе иначе обработал бы ещё и ConfirmDialog, и onClose вызвался бы дважды. Пока команда
  // выполняется, диалог не закрывается (как и в ConfirmDialog).
  const onKeyDownCapture = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    e.stopPropagation();
    if (!run.isPending) onClose();
  };

  const kind = action.kind;
  const isSwitchover = kind === "switchover";
  const isFailover = kind === "failover";
  const isRestart = kind === "restart";
  const candidates = p?.candidates ?? [];
  const firstReady = candidates.find((c) => c.ready)?.member ?? null;
  // Switchover: `null` — «Patroni выберет сам». Failover без кандидата невозможен: берётся первый готовый.
  const candidate = isFailover ? (chosen ?? firstReady) : chosen;
  const scheduling = isSwitchover || isRestart;
  const scheduledAt = scheduling && when === "scheduled" ? scheduleOf(local) : null;
  const scheduleMissing = scheduling && when === "scheduled" && scheduledAt === null;
  const memberName = p?.member?.name ?? (action.kind === "restart" || action.kind === "cancelRestart" ? action.member : "");
  const onlyPending = pendingOnly ?? p?.member?.pendingRestart ?? (action.kind === "restart" ? action.onlyPending : false);
  const leaderRestart = isRestart && p?.member != null && isLeaderRole(p.member.role);

  /** Итоговое действие по форме; `null` — выполнять пока нечего (время расписания не задано). */
  const actionToSend = (): PatroniAction | null => {
    switch (action.kind) {
      case "switchover":
        return scheduleMissing ? null : { kind: "switchover", candidate, scheduledAt };
      case "failover":
        return candidate === null ? null : { kind: "failover", candidate };
      case "restart":
        return scheduleMissing ? null : { kind: "restart", member: memberName, onlyPending, scheduledAt };
      default:
        return action;
    }
  };
  const toSend = p ? actionToSend() : null;

  /** Выполнять нечего: нужное в кластере уже не так, как при открытии кнопки. Текст — причина. */
  const nothingToDo = (data: PatroniActionPreview): string | null => {
    switch (action.kind) {
      case "cancelSwitchover":
        return data.scheduledSwitchover ? null : t("errors.actions.nothingScheduled");
      case "cancelRestart":
        return data.member?.scheduledRestart ? null : t("errors.actions.nothingScheduled");
      case "restart":
        return data.member ? null : t("errors.actions.nodeUnavailable", { member: action.member });
      case "pause":
        return data.paused === action.paused ? t("errors.actions.pauseUnchanged") : null;
      default:
        return null;
    }
  };

  const groupId = useId();
  const scheduleHintId = useId();
  const title = t(titleKey(action));

  /** Пояснения последствий: стоят в рамке подтверждения, рядом с кнопкой. */
  const consequences = (data: PatroniActionPreview) => {
    if (isSwitchover || isFailover) {
      // Сессии и недоступная запись — последствия «сейчас»: у switchover по расписанию они наступят потом, а число сессий
      // к тому времени будет другим. Лидера нет (failover при упавшем лидере) — рвать нечего, и «старого лидера» тоже нет.
      const immediate = !(isSwitchover && when === "scheduled");
      const hasLeader = data.leader !== null;
      return (
        <ul className="plain-list">
          {immediate && hasLeader && (
            <li>{data.leaderClientSessions === null ? t("actions.patroni.sessionsUnknown") : t("actions.patroni.sessions", { count: data.leaderClientSessions })}</li>
          )}
          {immediate && <li>{t("actions.patroni.writeDown")}</li>}
          {hasLeader && <li>{t("actions.patroni.oldLeader")}</li>}
        </ul>
      );
    }
    if (leaderRestart) return <p className="diag-warn">{t("actions.patroni.leaderRestart")}</p>;
    if (action.kind === "pause") return <p>{t(action.paused ? "actions.patroni.pauseNote" : "actions.patroni.resumeNote")}</p>;
    return null;
  };

  const details = (data: PatroniActionPreview, refused: boolean) => (
    <>
      {(isSwitchover || isFailover) && (
        <>
          <p>
            {t("actions.patroni.leader", { name: data.leader ?? "—" })}
            {data.leaderState && <span className="muted"> ({data.leaderState})</span>}
          </p>
          {/* «Действие может не пройти» рядом с отказом — лишнее: причина отказа названа ниже. */}
          {!refused && data.leaderReachable === false && <p className="diag-warn">{t("actions.patroni.leaderUnreachable")}</p>}
        </>
      )}
      {data.member && (
        <p className="muted">
          {t("actions.sessions.node", { member: data.member.name })}
          <span>
            {" "}
            ({roleLabel(data.member.role, "patroni")}
            {data.member.state ? `, ${data.member.state}` : ""})
          </span>
        </p>
      )}
      {action.kind === "cancelSwitchover" && data.scheduledSwitchover && (
        <p>
          {t("overview.scheduledSwitchover", {
            at: formatScheduled(data.scheduledSwitchover.at),
            from: data.scheduledSwitchover.from ?? "—",
            to: data.scheduledSwitchover.to ?? "—",
          })}
        </p>
      )}
      {action.kind === "cancelRestart" && data.member?.scheduledRestart && (
        <p>{t("overview.scheduledRestart", { at: formatScheduled(data.member.scheduledRestart.at) })}</p>
      )}
    </>
  );

  const candidateChoice = (
    <fieldset className="choice">
      <legend>{t("actions.patroni.candidate")}</legend>
      {isSwitchover && (
        <label>
          <input type="radio" name={`${groupId}-candidate`} checked={candidate === null} onChange={() => setChosen(null)} />
          {t("actions.patroni.candidateAuto")}
        </label>
      )}
      {candidates.map((c) => (
        <label key={c.member}>
          <input type="radio" name={`${groupId}-candidate`} checked={candidate === c.member} disabled={!c.ready} onChange={() => setChosen(c.member)} />
          {c.member}
          <span className="muted"> · {t("actions.patroni.candidateLag", { lag: bytesText(c.lagBytes) })}</span>
          {/* Причина отказа — внутри подписи: она входит в имя недоступного варианта и видна без наведения. */}
          {!c.ready && c.reasons.length > 0 && <span className="diag-warn"> — {c.reasons.map((r) => t(REASON_KEYS[r])).join("; ")}</span>}
        </label>
      ))}
    </fieldset>
  );

  const whenChoice = (
    <fieldset className="choice">
      <legend>{t("actions.patroni.when")}</legend>
      <label>
        <input type="radio" name={`${groupId}-when`} checked={when === "now"} onChange={() => setWhen("now")} />
        {t("actions.patroni.now")}
      </label>
      <label>
        <input type="radio" name={`${groupId}-when`} checked={when === "scheduled"} onChange={() => setWhen("scheduled")} />
        {t("actions.patroni.scheduled")}
      </label>
      {when === "scheduled" && (
        <label>
          {t("actions.patroni.scheduleAt")}
          <input type="datetime-local" value={local} required aria-describedby={scheduleMissing ? scheduleHintId : undefined} onChange={(e) => setLocal(e.target.value)} />
        </label>
      )}
      {scheduleMissing && (
        <p id={scheduleHintId} className="diag-warn">
          {t("actions.patroni.scheduleInvalid")}
        </p>
      )}
    </fieldset>
  );

  const closeButton = (
    <button type="button" onClick={onClose}>
      {t("common.close")}
    </button>
  );

  /** Что показать вместо формы, если выполнять нельзя; `null` — форма и подтверждение. */
  const blocked = (data: PatroniActionPreview) => {
    if (!data.canSend) return <p className="diag-warn">{t("actions.patroni.authRequired")}</p>;
    const nothing = nothingToDo(data);
    if (nothing) return <p className="muted">{nothing}</p>;
    // Те же предусловия, что в Rust (`running_leader`): иначе отказ пришёл бы только после ввода имени кандидата.
    // Решает Rust; ошибка выполнения по-прежнему показывается как есть.
    const leaderRunning = data.leader !== null && isRunningLeader(data.leaderReachable, data.leaderState);
    if (isFailover && leaderRunning) return <p className="diag-warn">{t("errors.actions.leaderHealthy")}</p>;
    if (isSwitchover && !leaderRunning) return <p className="diag-warn">{t("errors.actions.noLeader")}</p>;
    if (isFailover && firstReady === null) {
      return (
        <>
          <p className="diag-warn">{t("actions.patroni.noReadyCandidate")}</p>
          {data.candidates.length > 0 && (
            <ul className="plain-list">
              {data.candidates.map((c) => (
                <li key={c.member}>{c.reasons.length > 0 ? `${c.member} — ${c.reasons.map((r) => t(REASON_KEYS[r])).join("; ")}` : c.member}</li>
              ))}
            </ul>
          )}
        </>
      );
    }
    return null;
  };

  const block = p ? blocked(p) : null;

  return (
    // Имя секции не повторяет заголовок группы подтверждения внутри неё: два одинаковых имени читались бы дважды.
    <section ref={rootRef} tabIndex={-1} className="panel" aria-label={title} onKeyDownCapture={onKeyDownCapture}>
      {preview.isLoading && <p className="muted">{t("common.loading")}</p>}
      {preview.error && (
        <>
          <ErrorBanner error={preview.error} />
          {closeButton}
        </>
      )}
      {p && !run.data && (
        <>
          {details(p, block !== null)}
          {block ?? (
            <>
              {(isSwitchover || isFailover) && candidateChoice}
              {scheduling && whenChoice}
              {isRestart && (
                <label className="inline">
                  <input type="checkbox" checked={onlyPending} onChange={(e) => setPendingOnly(e.target.checked)} />
                  {t("actions.patroni.onlyPending")}
                </label>
              )}
              <ConfirmDialog
                connection={profile}
                title={t("actions.patroni.title", { action: title })}
                confirmLabel={t("actions.patroni.run")}
                objectName={isFailover ? (candidate ?? undefined) : undefined}
                objectLabel={isFailover ? t("actions.patroni.candidateLabel") : undefined}
                danger={isFailover || leaderRestart}
                busy={run.isPending}
                disabled={toSend === null}
                onConfirm={(c) => toSend && run.mutate({ a: toSend, c })}
                onCancel={onClose}
              >
                {consequences(p)}
              </ConfirmDialog>
            </>
          )}
          {block !== null && closeButton}
        </>
      )}
      {run.error && <ErrorBanner error={run.error} />}
      {run.data && (
        <>
          {run.data.outcomeUnknown ? (
            <>
              <p className="diag-warn" role="status">
                {t(run.data.watch ? "actions.patroni.outcomeUnknown" : "actions.patroni.outcomeUnknownNoWatch")}
              </p>
              {run.data.error && <ErrorBanner error={run.data.error} />}
            </>
          ) : (
            !run.data.watch && (
              <p role="status">{run.data.response.trim() === "" ? t("actions.patroni.accepted") : t("actions.patroni.response", { text: run.data.response })}</p>
            )
          )}
          {!run.data.journalWritten && <p className="diag-warn">{t("actions.journalNotWritten")}</p>}
          {run.data.watch ? (
            <WatchPanel action={kind} onlyPending={onlyPending} events={events} watchId={run.data.watch} onClose={onClose} />
          ) : (
            closeButton
          )}
        </>
      )}
    </section>
  );
}
