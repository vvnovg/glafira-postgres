import { type ReactNode, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AppError, McpStatus, PlanTree, RequestSource } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { effectiveLlmAccess } from "../../../lib/llmAccess";
import { useConnectionProfile } from "../../../lib/useConnectionProfile";
import { useOptimizationRequests, waitingRequest } from "../../../lib/useLlmProposals";
import { useMcpStatus } from "../../../lib/useMcpStatus";
import { Hint } from "../../common/Hint";

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

/** Строка для чата последнего созданного запроса и удалось ли её скопировать. */
export interface SentNotice {
  id: string;
  line: string;
  copied: boolean;
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
 * без маскирования. Пока запрос этого источника ждёт ответа, под кнопкой строка «Ждёт ответа LLM» с «Отменить запрос»,
 * второй запрос не создаётся. Строки ожидания и уведомления — отдельные, каждая в своей живой области (кнопки — вне их);
 * блок строк занимает свой ряд, только когда в нём что-то есть. Читает только запросы подключения (`useOptimizationRequests`), предложения кнопке не нужны.
 * Уведомление о скопированной строке по умолчанию живёт в кнопке; в консоли его держит `ExplainTab` (`sent`/`onSent`),
 * потому что кнопка перемонтируется, когда появляется результат плана. Статус ожидания выводится из списка запросов.
 * Между созданием и перечитыванием списка кнопка тоже неактивна: второй быстрый клик дубля не создаёт.
 */
export function AskLlmButton({ connectionId, member, database, source, sql, params, plan, sent: sentProp, onSent, extra }: AskLlmProps) {
  const { t } = useTranslation();
  const { status, loading: statusLoading, error: statusError, refetch: refetchStatus } = useMcpStatus();
  const profile = useConnectionProfile(connectionId);
  const { requests, state: requestsState, refresh, retry: retryRequests } = useOptimizationRequests(connectionId);
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
  const can = ready && off === null && sql.trim() !== "" && !busy && waiting === null && !justSent;

  const ask = async () => {
    setBusy(true);
    setError(null);
    try {
      const r = await api.createOptimizationRequest(connectionId, { member, database, source, sql, params, plan });
      const line = t("llmAssist.ask.chatLine", { id: r.id });
      let copied = true;
      try {
        if (!navigator.clipboard) throw new Error("clipboard unavailable");
        await navigator.clipboard.writeText(line);
      } catch {
        copied = false;
      }
      setSent({ id: r.id, line, copied });
      refresh();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  const cancel = async (id: string) => {
    setError(null);
    try {
      await api.cancelOptimizationRequest(id);
      setSent(null);
      refresh();
    } catch (e) {
      setError(toAppError(e));
    }
  };

  // Строка для чата видна, пока её запрос ждёт ответа (или список ещё не перечитан после создания).
  const fresh = sent !== null && (waiting?.id === sent.id || !requests.some((r) => r.id === sent.id));
  const notice = sent === null || !fresh ? "" : sent.copied ? t("llmAssist.ask.copied", { id: sent.id }) : t("llmAssist.ask.copyFailed", { id: sent.id, line: sent.line });
  const unreadable = off === "status" ? t("llmAssist.ask.statusUnavailable") : off === "requests" ? t("llmAssist.ask.requestsFailed") : "";
  const message = error ? tr(error) : notice;
  const shown = waiting !== null || unreadable !== "" || message !== "";
  return (
    <>
      <button type="button" disabled={!can} onClick={() => void ask()}>
        {t("llmAssist.ask.button")}
      </button>
      <Hint k={off ? OFF_HINTS[off] : "hints.llmAssist.ask"} label={t("llmAssist.ask.button")} />
      {extra}
      <div className={shown ? "llm-ask-notes shown" : "llm-ask-notes"}>
        <div className="llm-ask-line">
          <span role="status" className="muted">
            {waiting && <span>{t("llmAssist.ask.waiting", { id: waiting.id })}</span>}
            {waiting && unreadable && " "}
            {unreadable && <span>{unreadable}</span>}
          </span>
          {waiting && (
            <button type="button" className="link" onClick={() => void cancel(waiting.id)}>
              {t("llmAssist.ask.cancel")}
            </button>
          )}
          {unreadable && (
            <button type="button" className="link" onClick={off === "status" ? refetchStatus : retryRequests}>
              {t("common.retry")}
            </button>
          )}
        </div>
        <p role="status" className="muted llm-ask-notice">
          {message}
        </p>
      </div>
    </>
  );
}
