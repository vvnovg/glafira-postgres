import type { TFunction } from "i18next";
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AppError, LlmProposal, ProposalComparison, ProposalStatus } from "../../../api/types";
import { formatTimestamp } from "../../../lib/format";
import { type DiffCell, type DiffRow, lineDiff, sideBySide } from "../../../lib/lineDiff";
import { consoleProposals, useLlmProposals } from "../../../lib/useLlmProposals";
import { useTabMarks } from "../../../state/tabMarks";
import { openExplainWithPrefill } from "../../../state/workspace";
import { CopyButton } from "../../common/CopyButton";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Glyph } from "../../common/Glyph";
import { ProposalCompare } from "./ProposalCompare";

/** Подписи статусов — литералами (правило словарей). */
const STATUS_KEYS = {
  new: "llmAssist.panel.status.new",
  accepted: "llmAssist.panel.status.accepted",
  rejected: "llmAssist.panel.status.rejected",
} as const;

/** Причина отклонения — не длиннее (её видит модель, спец. P4 §8.4). */
export const REASON_MAX = 500;

/** Предложение, которое панель покажет выбранным (ссылка «Предложения LLM: N» карточки): один раз на `nonce`. */
export interface ProposalFocus {
  id: string;
  nonce: number;
}

/** Время без миллисекунд: «2026-11-04 12:03:04». */
const when = (ms: number) => formatTimestamp(ms).slice(0, 19);
/** Начало переписанного текста для списка: первая непустая строка, до 80 символов (дальше обрезает «…» стиля). */
const head = (sql: string) => (sql.split("\n").find((l) => l.trim() !== "") ?? "").trim().slice(0, 80);
const clientName = (p: LlmProposal, t: TFunction) => p.client ?? t("llmAssist.panel.unknownClient");

/**
 * Чип статуса: цвет (новое — акцент, принято — зелёный, отклонено — красный) и слово — смысл не держится на одном цвете.
 * Причина отклонения — не здесь, а отдельной строкой в карточке.
 */
function StatusChip({ status }: { status: ProposalStatus }) {
  const { t } = useTranslation();
  return <span className={`llm-status llm-status-${status}`}>{t(STATUS_KEYS[status])}</span>;
}

/**
 * «Предложения LLM» под редактором консоли EXPLAIN (спец. P4 §8.4): предложения на запросы этой консоли и предложения без
 * запроса для её базы (`consoleProposals`); `focus` — предложение, выбранное ссылкой карточки: оно закрепляется в списке и
 * выбирается один раз на `nonce`. Свёрнута, пока предложений нет, если пользователь не раскрыл её сам. Новое предложение
 * приходит по событию `mcp://proposals`; пока консоль в фоне (`active = false`), предложения, которых она ещё не показала,
 * ставят пометку её вкладке (`tabId`) — строка вкладок рисует точку.
 */
export function ProposalsPanel({ connectionId, database, active, tabId, focus }: { connectionId: string; database: string | null; active: boolean; tabId: string; focus: ProposalFocus | null }) {
  const { t } = useTranslation();
  const { requests, proposals, ready, refresh } = useLlmProposals(connectionId, { connectionId });
  const [pinned, setPinned] = useState<string[]>([]);
  const [open, setOpen] = useState<boolean | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const appliedNonce = useRef<number | null>(null);
  const focusId = focus?.id;
  const focusNonce = focus?.nonce;
  useEffect(() => {
    if (focusId === undefined || focusNonce === undefined || appliedNonce.current === focusNonce) return;
    appliedNonce.current = focusNonce;
    setPinned((p) => (p.includes(focusId) ? p : [...p, focusId]));
    setSelected(focusId);
    setOpen(true);
  }, [focusId, focusNonce]);
  const list = useMemo(() => consoleProposals(proposals, requests, database, pinned), [proposals, requests, database, pinned]);
  // Выбор держится за id: ответ или новое предложение пересортируют список, а показанное предложение остаётся.
  useEffect(() => {
    if (selected === null && list.length > 0) setSelected(list[0].id);
  }, [selected, list]);
  // Всё, что консоль показала активной, увидено; пометка — для пришедших, пока она в фоне. Первый прочитанный список —
  // уже существовавшие предложения: их консоль, смонтированная в фоне, новыми не считает.
  const seen = useRef(new Set<string>());
  const seeded = useRef(false);
  useEffect(() => {
    if (!ready) return;
    if (active || !seeded.current) for (const p of list) seen.current.add(p.id);
    seeded.current = true;
  }, [ready, active, list]);
  const unseen = !active && seeded.current && list.some((p) => !seen.current.has(p.id));
  const setMark = useTabMarks((s) => s.setMark);
  useEffect(() => setMark(tabId, unseen), [setMark, tabId, unseen]);
  useEffect(() => () => setMark(tabId, false), [setMark, tabId]);

  const bodyId = useId();
  const isOpen = open ?? list.length > 0;
  const current = list.find((p) => p.id === selected) ?? list[0] ?? null;
  return (
    <section className="llm-proposals" aria-label={t("llmAssist.panel.title")}>
      <button type="button" className="llm-proposals-toggle" aria-expanded={isOpen} aria-controls={bodyId} onClick={() => setOpen(!isOpen)}>
        <span className="tree-caret" aria-hidden="true">
          {isOpen ? "▾" : "▸"}
        </span>
        {t("llmAssist.panel.heading", { count: list.length })}
      </button>
      <div id={bodyId} className="llm-proposals-region">
        {isOpen &&
          (list.length === 0 ? (
            <p className="llm-proposals-empty">{t("llmAssist.panel.empty")}</p>
          ) : (
            <div className="llm-proposals-body">
              <ul className="plain-list llm-proposals-list" aria-label={t("llmAssist.panel.list")}>
                {list.map((p) => (
                  <li key={p.id}>
                    {/* Пункт — карточка в две строки: статус, клиент и время; начало предложенного текста. */}
                    <button type="button" className="llm-item" aria-current={p.id === current?.id} onClick={() => setSelected(p.id)}>
                      <span className="llm-item-meta">
                        <StatusChip status={p.status} /> <span className="llm-item-client">{clientName(p, t)}</span>{" "}
                        <span className="llm-item-time">{when(p.createdAtMs)}</span>
                      </span>{" "}
                      <code className="llm-item-sql">{head(p.rewrittenSql)}</code>
                    </button>
                  </li>
                ))}
              </ul>
              {current && <ProposalView key={current.id} proposal={current} connectionId={connectionId} onChanged={refresh} />}
            </div>
          ))}
      </div>
    </section>
  );
}

/** Две клетки стороны: номер строки и текст; изменённая строка подсвечена и помечена значком для скринридера. */
function DiffCells({ cell }: { cell: DiffCell | null }) {
  const { t } = useTranslation();
  if (cell === null) {
    return (
      <>
        <td className="num" />
        <td className="diff-empty" />
      </>
    );
  }
  return (
    <>
      <td className="num">{cell.n}</td>
      <td className={cell.kind === "same" ? undefined : `diff-${cell.kind}`}>
        {cell.kind === "del" && <Glyph symbol="−" label={t("llmAssist.panel.removed")} className="diff-mark" />}
        {cell.kind === "add" && <Glyph symbol="+" label={t("llmAssist.panel.added")} className="diff-mark" />}
        <code>{cell.text}</code>
      </td>
    </>
  );
}

function DiffTable({ rows }: { rows: DiffRow[] }) {
  const { t } = useTranslation();
  return (
    <div className="table-scroll">
      <table className="line-diff" aria-label={t("llmAssist.panel.diff")}>
        {/* Ширины колонок: при table-layout: fixed их задаёт colgroup (иначе — первая строка, где у заголовков colspan). */}
        <colgroup>
          <col className="diff-num" />
          <col className="diff-text" />
          <col className="diff-num" />
          <col className="diff-text" />
        </colgroup>
        <thead>
          <tr>
            <th scope="col" colSpan={2}>
              {t("llmAssist.panel.original")}
            </th>
            <th scope="col" colSpan={2}>
              {t("llmAssist.panel.rewritten")}
            </th>
          </tr>
        </thead>
        <tbody>
          {rows.map((r, i) => (
            <tr key={i}>
              <DiffCells cell={r.left} />
              <DiffCells cell={r.right} />
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Выбранное предложение — карточка. Шапка: чип статуса, кто и когда прислал, «Принять» (основная) и «Отклонить»; под ней
 * причина отклонения своей строкой и второй ряд — «Сравнить планы», «Открыть в консоли», «Копировать»; форма причины
 * открывается сразу под шапкой. Дальше — исходный и предложенный текст рядом с подсветкой изменённых строк, постоянная
 * пометка о неравенстве результатов, разделы «Обоснование», «Ожидаемый эффект» (как текст, разметку модели не применяем),
 * «Индексы» и итог сравнения отдельным блоком. Итог сравнения из ответа команды виден сразу, не дожидаясь перечитывания
 * списка. Ответ можно изменить (принято ↔ отклонено): у отвеченного — противоположное действие. Статус, причина и кнопки
 * ответа в шапке берутся из ответа команды, пока список его не перечитал: фокус после ответа приходит на новый статус.
 */
function ProposalView({ proposal: p, connectionId, onChanged }: { proposal: LlmProposal; connectionId: string; onChanged: () => void }) {
  const { t } = useTranslation();
  const rows = useMemo(() => sideBySide(lineDiff(p.originalSql, p.rewrittenSql)), [p.originalSql, p.rewrittenSql]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const [compared, setCompared] = useState<ProposalComparison | null>(null);
  const [comparing, setComparing] = useState(false);
  const [rejecting, setRejecting] = useState(false);
  const [reason, setReason] = useState("");
  // Фокус: открытая причина получает его сразу; после ответа и отмены он возвращается на осмысленное место, а не на <body>.
  const reasonRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    if (rejecting) reasonRef.current?.focus();
  }, [rejecting]);
  const rejectRef = useRef<HTMLButtonElement>(null);
  // После ответа фокус — на шапке: читаются новый статус и кто прислал.
  const headRef = useRef<HTMLDivElement>(null);
  const comparison = compared ?? p.comparison ?? null;
  // Ответ, который список ещё не перечитал (перечитывание асинхронно): шапка показывает его сразу, а не прежний статус.
  const [answer, setAnswer] = useState<LlmProposal | null>(null);
  const { status, reason: rejectReason } = answer ?? p;
  useEffect(() => {
    if (answer && answer.status === p.status && (answer.reason ?? null) === (p.reason ?? null)) setAnswer(null);
  }, [answer, p.status, p.reason]);

  const perform = async (call: () => Promise<LlmProposal>, answered = false) => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const next = await call();
      if (next.comparison) setCompared(next.comparison);
      if (answered) {
        // Форму причины закрывает только ответ: сравнение при открытой форме её не трогает. Шапка с новым статусом
        // отрисована до переноса фокуса (flushSync), поэтому скринридер читает «Принято» / «Отклонено», а не прежний статус.
        flushSync(() => {
          setAnswer(next);
          setRejecting(false);
        });
        headRef.current?.focus();
      }
      onChanged();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };
  // Текст консоли заменяется предложенным (прежние запуски остаются в истории), узел и база — предложения.
  const openInConsole = () =>
    openExplainWithPrefill(connectionId, { sql: p.rewrittenSql, database: p.database, member: p.member ?? undefined });

  return (
    <div className="llm-proposal">
      <div className="llm-proposal-head">
        <div className="llm-proposal-title">
          <div className="llm-proposal-meta" tabIndex={-1} ref={headRef}>
            <StatusChip status={status} /> <span className="muted">{t("llmAssist.panel.from", { client: clientName(p, t), time: when(p.createdAtMs) })}</span>
          </div>
          <div className="llm-proposal-answer">
            {status !== "accepted" && (
              <button type="button" className="primary" aria-disabled={busy} onClick={() => void perform(() => api.answerLlmProposal(p.id, "accepted", null), true)}>
                {t("llmAssist.panel.accept")}
              </button>
            )}
            {status !== "rejected" && (
              <button type="button" ref={rejectRef} aria-expanded={rejecting} aria-disabled={busy} onClick={() => setRejecting(true)}>
                {t("llmAssist.panel.reject")}
              </button>
            )}
          </div>
        </div>
        {status === "rejected" && rejectReason && <p className="muted llm-proposal-reason">{t("llmAssist.panel.rejectReason", { reason: rejectReason })}</p>}
        <div className="toolbar">
          <button
            type="button"
            aria-disabled={busy}
            onClick={() => {
              if (busy) return;
              setComparing(true);
              void perform(() => api.compareLlmProposal(p.id)).finally(() => setComparing(false));
            }}
          >
            {t("llmAssist.panel.compare")}
          </button>
          <button type="button" onClick={openInConsole}>
            {t("llmAssist.panel.openInConsole")}
          </button>
          <CopyButton text={p.rewrittenSql} label={t("llmAssist.panel.copy")} />
        </div>
      </div>
      {rejecting && (
        <div className="llm-reject">
          <label>
            <span>{t("llmAssist.panel.reason")}</span>
            <textarea ref={reasonRef} rows={3} maxLength={REASON_MAX} value={reason} onChange={(e) => setReason(e.target.value)} />
          </label>
          <div className="toolbar">
            <button type="button" className="primary" aria-disabled={busy} onClick={() => void perform(() => api.answerLlmProposal(p.id, "rejected", reason.trim() || null), true)}>
              {t("llmAssist.panel.rejectConfirm")}
            </button>
            <button
              type="button"
              onClick={() => {
                setRejecting(false);
                rejectRef.current?.focus();
              }}
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
      {error && <ErrorBanner error={error} />}
      <DiffTable rows={rows} />
      <p className="diag-note">{t("llmAssist.panel.sameRows")}</p>
      <div className="llm-proposal-section">
        <h4>{t("llmAssist.panel.rationale")}</h4>
        <p className="llm-text">{p.rationale}</p>
      </div>
      {p.expectedEffect && (
        <div className="llm-proposal-section">
          <h4>{t("llmAssist.panel.effect")}</h4>
          <p className="llm-text">{p.expectedEffect}</p>
        </div>
      )}
      <div className="llm-proposal-section">
        <h4>{t("llmAssist.panel.indexes")}</h4>
        {p.indexes.length === 0 ? (
          <p className="muted">{t("llmAssist.panel.noIndexes")}</p>
        ) : (
          <ul className="plain-list">
            {p.indexes.map((ddl, i) => (
              <li key={`${ddl}:${i}`}>
                <code>{ddl}</code>
              </li>
            ))}
          </ul>
        )}
      </div>
      {/* Единственная живая область: «Сравнение…» и итог (с заметкой hypopg) вставляются в неё и озвучиваются. */}
      <div aria-live="polite">
        {comparing && <p className="muted">{t("llmAssist.panel.comparing")}</p>}
        {comparison && <ProposalCompare comparison={comparison} />}
      </div>
    </div>
  );
}
