import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AgentPreset, AppError, McpStatus, OptimizationRequest, PlanTree, RequestSource } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { effectiveLlmAccess } from "../../../lib/llmAccess";
import { isActive, latestRunFor, queueAhead, useAgentRuns } from "../../../lib/useAgentRuns";
import { useConnectionProfile } from "../../../lib/useConnectionProfile";
import { sourceMatches, useOptimizationRequests, waitingRequest } from "../../../lib/useLlmProposals";
import { useMcpStatus } from "../../../lib/useMcpStatus";
import { Hint } from "../../common/Hint";
import { useShownProposals } from "../../../state/shownProposals";
import { AgentRunStatus, agentRunVisible } from "./AgentRunStatus";

/** Почему запрос не отправить. Ключи подсказок — литералами (правило словарей и проверка подсказок `i18n:check`). */
type Off = "policy" | "server" | "otherInstance" | "error" | "access" | "status" | "requests";
const OFF_HINTS: Record<Off, string> = {
  policy: "hints.llmAssist.askPolicy",
  server: "hints.llmAssist.askServer",
  otherInstance: "hints.llmAssist.askOtherInstance",
  error: "hints.llmAssist.askError",
  access: "hints.llmAssist.askAccess",
  status: "hints.llmAssist.askStatus",
  requests: "hints.llmAssist.askRequests",
};

/** Подсказка активной кнопки в режиме фона называет агента; ключи — литералами (`Hint` не передаёт параметры). */
const BACKGROUND_HINTS: Record<AgentPreset, string> = {
  qwen: "hints.llmAssist.askBackgroundQwen",
  claude: "hints.llmAssist.askBackgroundClaude",
};

/** Политика → состояние сервера (спец. P4 §11: активна, если MCP включён); `null` — сервер работает и политика разрешает. */
function serverOff(status: McpStatus): Off | null {
  if (!status.policyEnabled) return "policy";
  switch (status.server) {
    case "running":
      return null;
    case "otherInstance":
      return "otherInstance";
    case "error":
      return "error";
    default:
      return "server";
  }
}

/**
 * Режим фона (спец. P4c §8.1): политика разрешает фонового ассистента, выключатель «Запускать ассистента в фоне» включён
 * и агент настроен (`configured`: путь программы задан, а у Claude Code через Ollama — ещё и модель). Иначе кнопка работает
 * как в P4b — копирует строку для чата.
 */
export function backgroundMode(status: McpStatus | undefined): boolean {
  const a = status?.agent;
  return a !== undefined && a.policyEnabled && a.enabled && a.configured;
}

/** Последний запрос источника в любом статусе или null. */
function latestRequest(requests: OptimizationRequest[], source: RequestSource, database: string): OptimizationRequest | null {
  let found: OptimizationRequest | null = null;
  for (const r of requests) if (sourceMatches(r, source, database) && (found === null || r.createdAtMs > found.createdAtMs)) found = r;
  return found;
}

/** Строка для чата последнего созданного запроса и удалось ли её скопировать. */
export interface SentNotice {
  id: string;
  line: string;
  copied: boolean;
  /** Запрос ушёл фоновому ассистенту: в буфер ничего не клали (спец. P4c §8.1). */
  background?: boolean;
}

export interface AskLlmProps {
  connectionId: string;
  member: string;
  database: string;
  source: RequestSource;
  sql: string;
  /** Значения `$N` последнего плана (пустое поле — null); нет плана — пустой список. */
  params: (string | null)[];
  /** Последний план вкладки, пока SQL, узел и база не менялись (как у «Скопировать для LLM»); иначе Rust строит пробный. */
  plan: PlanTree | null;
  /**
   * Уведомление о скопированной строке можно держать снаружи (родитель, переживающий перемонтирование кнопки); без
   * `onSent` кнопка хранит его сама.
   */
  sent?: SentNotice | null;
  onSent?: (sent: SentNotice | null) => void;
  /** Элементы того же ряда (в карточке — «Предложения LLM: N»): после кнопки и подсказки, до строк уведомлений. */
  extra?: ReactNode;
}

/**
 * «Передать LLM-ассистенту» (спец. P4 §8.1): сохраняет снимок запроса на оптимизацию (`create_optimization_request`) и
 * кладёт в буфер строку для чата ассистента на языке интерфейса; уведомление начинается с того, что делать: сказать
 * ассистенту «оптимизируй запрос из Glafira» — без id `get_optimization_request` берёт последний ожидающий запрос, — а
 * строка с id в буфере — запасной путь.
 * Активна, когда политика разрешает MCP, MCP-сервер этого окна работает и действующий уровень подключения не `off`
 * (`effectiveLlmAccess`). Строго: пока статус MCP, профиль или список запросов читаются — неактивна без пояснения; если
 * статус или список не прочитались — неактивна с пояснением и «Повторить» (неудачное чтение не значит «ничего не ждёт»:
 * иначе кнопка позволила бы дубль запроса). Значок «?» объясняет причину, а у активной кнопки — что снимок уходит модели
 * без маскирования. Пока запрос этого источника ждёт ответа, под кнопкой строка «Ждёт ответа LLM» с «Отменить запрос»;
 * кнопка ждущим запросом не блокируется (смоук P4c, R15): нажатие сначала закрывает ждущий запрос этого источника (Rust
 * останавливает и его фоновый запуск, как «Отменить запрос»), потом создаёт новый; «?» об этом предупреждает. Не закрылся
 * — ошибка у кнопки, новый не создаётся. Строки ожидания и уведомления — отдельные, каждая в своей живой области (кнопки — вне их);
 * блок строк занимает свой ряд, только когда в нём что-то есть. Читает только запросы подключения (`useOptimizationRequests`), предложения кнопке не нужны.
 * Уведомление о скопированной строке по умолчанию живёт в кнопке; в консоли его держит `ExplainTab` (`sent`/`onSent`),
 * потому что кнопка перемонтируется вместе с запуском плана при смене узла или базы, а id отправленного запроса нужен панели
 * предложений (автовыбор ответа, смоук P4b R2). Статус ожидания выводится из списка запросов.
 * Между созданием и перечитыванием списка кнопка тоже неактивна: второй быстрый клик дубля не создаёт.
 *
 * Режим фона (спец. P4c §8.1, `backgroundMode`): кнопка создаёт запрос и ставит запуск в очередь (`start_agent_run`), в
 * буфер ничего не кладёт; уведомление — «Запрос r… передан фоновому ассистенту»; вместо строки «Ждёт ответа LLM» — строка
 * статуса запуска `AgentRunStatus` в том же блоке строк. Запуски читаются в режиме фона и пока у Glafira есть идущий или
 * ждущий запуск (режим могли выключить во время запуска — он доводится до конца, и его статус виден до конца). После ответа
 * строка статуса остаётся, пока запуск идёт или его предложения не показаны панелью. Не встал в очередь (очередь полна,
 * политика) — текст ошибки в уведомлении, запрос ждёт, в строке статуса — «Запустить фонового ассистента».
 */
export function AskLlmButton({ connectionId, member, database, source, sql, params, plan, sent: sentProp, onSent, extra }: AskLlmProps) {
  const { t } = useTranslation();
  const { status, loading: statusLoading, error: statusError, refetch: refetchStatus } = useMcpStatus();
  const profile = useConnectionProfile(connectionId);
  const { requests, state: requestsState, refresh, retry: retryRequests } = useOptimizationRequests(connectionId);
  const background = backgroundMode(status);
  const agent = status?.agent;
  const runsWanted = background || (agent !== undefined && (agent.running != null || agent.queued > 0));
  const { all: runs, state: runsState, refresh: refreshRuns } = useAgentRuns(connectionId, { enabled: runsWanted });
  const shownProposals = useShownProposals((s) => s.shown);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [ownSent, setOwnSent] = useState<SentNotice | null>(null);
  const controlled = onSent !== undefined;
  const sent = controlled ? (sentProp ?? null) : ownSent;
  const setSent = controlled ? onSent : setOwnSent;
  const waiting = waitingRequest(requests, source, database);

  // Что мешает отправить. `null` вместе с `ready === false` — ещё читается, без пояснения.
  let off: Off | null = null;
  let ready = true;
  if (status === undefined) {
    ready = false;
    if (!statusLoading && statusError) off = "status";
  } else {
    off = serverOff(status);
    if (off === null && profile === undefined) ready = false;
    else if (off === null && profile && effectiveLlmAccess(profile, status) === "off") off = "access";
  }
  if (off === null && ready) {
    if (requestsState === "failed") off = "requests";
    else if (requestsState === "loading") ready = false;
  }
  // Запрос создан, но список ещё не перечитан: ждём его, иначе быстрый второй клик создал бы дубль.
  const justSent = sent !== null && !requests.some((r) => r.id === sent.id);
  const can = ready && off === null && sql.trim() !== "" && !busy && !justSent;

  // Строка статуса запуска. У ждущего запроса — в режиме фона, а без него — пока его запуск ещё идёт (режим выключили во
  // время запуска); у отвеченного — если у него был запуск и строке есть что сказать (идущий запуск или непоказанные
  // предложения, `agentRunVisible`): иначе пустой блок строк занял бы ряд. Пока список запросов не перечитан после
  // создания, прежний запрос источника не показывается.
  const current = justSent ? null : (waiting ?? latestRequest(requests, source, database));
  const runsReady = runsWanted && runsState === "ready";
  const run = current !== null && runsReady ? latestRunFor(runs, current.id) : null;
  const showRun =
    current !== null &&
    runsReady &&
    (current.status === "waiting" ? background || (run !== null && isActive(run)) : current.status === "answered" && run !== null) &&
    agentRunVisible(current, run, shownProposals);

  const ask = async () => {
    setBusy(true);
    setError(null);
    // Ждущий запрос источника заменяется новым: сначала закрыть (с его запуском), не закрылся — нового не создавать.
    const replaced = waiting;
    let closed = false;
    try {
      if (replaced !== null) {
        await api.cancelOptimizationRequest(replaced.id);
        closed = true;
      }
      const r = await api.createOptimizationRequest(connectionId, { member, database, source, sql, params, plan });
      const line = t("llmAssist.ask.chatLine", { id: r.id });
      if (background) {
        // Режим фона: запуск — в очередь Glafira, буфер не трогаем. Не встал в очередь — запрос остаётся ждущим: в строке
        // статуса «Запустить фонового ассистента» и «Скопировать строку для чата».
        let startError: AppError | null = null;
        try {
          await api.startAgentRun(r.id);
        } catch (e) {
          startError = toAppError(e);
        }
        setSent({ id: r.id, line, copied: false, background: true });
        setError(startError);
        refresh();
        refreshRuns();
        return;
      }
      let copied = true;
      try {
        if (!navigator.clipboard) throw new Error("clipboard unavailable");
        await navigator.clipboard.writeText(line);
      } catch {
        copied = false;
      }
      setSent({ id: r.id, line, copied });
      refresh();
      if (closed && runsWanted) refreshRuns();
    } catch (e) {
      setError(toAppError(e));
      // Прежний запрос закрыт, а новый не создан: список и запуски должны это показать.
      if (closed) {
        setSent(null);
        refresh();
        if (runsWanted) refreshRuns();
      }
    } finally {
      setBusy(false);
    }
  };

  // «Повторить» и «Запустить фонового ассистента»: новый запуск того же запроса (снимок прежний, спец. P4c §7).
  const startRun = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await api.startAgentRun(id);
      refreshRuns();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  // Отмена запроса останавливает и его запуск (Rust, `cancel_optimization_request`).
  const cancel = async (id: string) => {
    setError(null);
    try {
      await api.cancelOptimizationRequest(id);
      setSent(null);
      refresh();
      if (runsWanted) refreshRuns();
    } catch (e) {
      setError(toAppError(e));
    }
  };

  // Уведомление видно, пока его запрос ждёт ответа (или список ещё не перечитан после создания).
  const fresh = sent !== null && (waiting?.id === sent.id || !requests.some((r) => r.id === sent.id));
  const notice =
    sent === null || !fresh
      ? ""
      : sent.background
        ? t("llmAssist.ask.queuedNotice", { id: sent.id })
        : sent.copied
          ? t("llmAssist.ask.copied", { id: sent.id })
          : t("llmAssist.ask.copyFailed", { id: sent.id, line: sent.line });
  // Запуски не прочитались — пояснение, только если от них зависит строка ждущего запроса (иначе ждать нечего).
  const runsUnreadable = runsWanted && runsState === "failed" && waiting !== null;
  const unreadable =
    off === "status" ? t("llmAssist.ask.statusUnavailable") : off === "requests" ? t("llmAssist.ask.requestsFailed") : runsUnreadable ? t("llmAssist.ask.runsFailed") : "";
  const retryRead = off === "status" ? refetchStatus : off === "requests" ? retryRequests : refreshRuns;
  // Ждущий запрос: «?» говорит, что новый запрос его заменит (причина неактивности важнее).
  const hint = off ? OFF_HINTS[off] : waiting ? "hints.llmAssist.askReplace" : background && agent ? BACKGROUND_HINTS[agent.preset] : "hints.llmAssist.ask";
  const message = error ? tr(error) : notice;
  // Строка ожидания P4b — когда строки статуса запуска нет.
  const plainWaiting = showRun ? null : waiting;
  const shown = plainWaiting !== null || showRun || unreadable !== "" || message !== "";
  return (
    <>
      <button type="button" disabled={!can} onClick={() => void ask()}>
        {t("llmAssist.ask.button")}
      </button>
      <Hint k={hint} label={t("llmAssist.ask.button")} values={!off && waiting ? { id: waiting.id } : undefined} />
      {extra}
      <div className={shown ? "llm-ask-notes shown" : "llm-ask-notes"}>
        <div className="llm-ask-line">
          <span role="status" className="muted">
            {plainWaiting && <span>{t("llmAssist.ask.waiting", { id: plainWaiting.id })}</span>}
            {plainWaiting && unreadable && " "}
            {unreadable && <span>{unreadable}</span>}
          </span>
          {plainWaiting && (
            <button type="button" className="link" onClick={() => void cancel(plainWaiting.id)}>
              {t("llmAssist.ask.cancel")}
            </button>
          )}
          {unreadable && (
            <button type="button" className="link" onClick={retryRead}>
              {t("common.retry")}
            </button>
          )}
        </div>
        {showRun && current !== null && (
          <AgentRunStatus
            key={current.id}
            request={current}
            run={run}
            ahead={run !== null && run.status === "queued" ? queueAhead(runs, run) : 0}
            chatLine={t("llmAssist.ask.chatLine", { id: current.id })}
            busy={busy}
            onRetry={() => void startRun(current.id)}
            onCancel={() => void cancel(current.id)}
          />
        )}
        <p role="status" className="muted llm-ask-notice">
          {message}
        </p>
      </div>
    </>
  );
}
