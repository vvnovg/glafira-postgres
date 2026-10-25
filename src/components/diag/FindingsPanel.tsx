import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DiagTool, Finding, FindingLink, FindingTarget, Severity } from "../../api/types";
import { formatDuration } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { CopyButton } from "../common/CopyButton";
import { findingFix, findingTitle } from "./findingText";

type ActionLink = Extract<FindingLink, { kind: "action" }>;

/** Порядок показа: сначала более строгие; внутри одной строгости остаётся порядок сервера (сортировка устойчива). */
const SEVERITY_ORDER: Record<Severity, number> = { critical: 0, warning: 1, info: 2 };

/** Часть ключа находки по её объекту: сессия — `pid` и время старта backend (pid переиспользуется), остальное — имя. */
const targetKey = (t: FindingTarget | null) => (t === null ? "" : t.kind === "session" ? `${t.pid}:${t.backendStartUs}` : t.name);

/** Ключ кнопки действия: тип и сессия (у корня — pid и время старта: pid переиспользуется). */
const actionKey = (l: ActionLink) =>
  l.action.type === "terminateRootBlocker" ? `action:${l.action.type}:${l.action.pid}:${l.action.backendStartUs}` : `action:${l.action.type}:${l.member ?? ""}`;

/**
 * Устойчивые ключи списка: id находки, узел и объект, а не место в массиве, — раскрытое «Что сделать» остаётся раскрытым
 * при обновлении, даже если порядок находок изменился. Счётчик добавляется только настоящим дубликатам (совпали все три части).
 */
function findingKeys(findings: Finding[]): string[] {
  const seen = new Map<string, number>();
  return findings.map((f) => {
    const base = `${f.id}|${f.member ?? ""}|${targetKey(f.target)}`;
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n === 0 ? base : `${base}#${n}`;
  });
}

/**
 * Находки отчёта (спец. P2 §7.3): строгость (цвет и текстовая метка), заголовок, «что сделать», команда для копирования
 * и кнопки: переход в другой инструмент и действия над сессиями (только при `onAction`). Блок сворачивается;
 * без находок — «Проблем не найдено». `showMember` — у отчёта кластера: находки относятся к разным узлам, и узел
 * показывается отдельной меткой.
 */
export function FindingsPanel({
  connectionId,
  findings,
  showMember = false,
  tool,
  member,
  onAction,
}: {
  connectionId: string;
  findings: Finding[];
  showMember?: boolean;
  /** Инструмент и узел вкладки, где показан блок: переход «в себя» не предлагается. */
  tool?: DiagTool;
  member?: string | null;
  /** Кнопки действий находок (отмена, завершение сессий); без обработчика действий нет — подключение только для чтения. */
  onAction?: (link: ActionLink) => void;
}) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const titleId = useId();
  const [expanded, setExpanded] = useState(true);
  const sorted = [...findings].sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity]);
  const keys = findingKeys(sorted);
  return (
    <section className="findings" aria-labelledby={titleId}>
      <div className="findings-head">
        <h3 id={titleId}>
          {t("diag.findings.title")}
          {findings.length > 0 && <span className="muted"> ({findings.length})</span>}
        </h3>
        <button type="button" className="icon" aria-expanded={expanded} aria-label={expanded ? t("diag.findings.collapse") : t("diag.findings.expand")} onClick={() => setExpanded(!expanded)}>
          <span aria-hidden="true">{expanded ? "▾" : "▸"}</span>
        </button>
      </div>
      {expanded && findings.length === 0 && <p className="muted">{t("diag.findings.none")}</p>}
      {expanded && findings.length > 0 && (
        <ul className="findings-list">
          {sorted.map((f, i) => {
            const command = typeof f.params.command === "string" ? f.params.command : null;
            // Ссылка без узла ведёт туда же, что и узел отчёта: вкладка инструмента одна на подключение.
            // Ссылки-действия (`kind: "action"`) — кнопки только при `onAction` и известном узле (свой у ссылки или узел отчёта):
            // без узла выполнять негде. Переход «в себя» не предлагается.
            const links = f.links.filter((l) =>
              l.kind === "action" ? onAction !== undefined && (l.member ?? member ?? null) !== null : !(l.tool === tool && (l.member == null || l.member === member)),
            );
            return (
              <li key={keys[i]} className={`finding sev-${f.severity}`}>
                <div className="finding-main">
                  <span className="finding-sev">{t(`diag.severity.${f.severity}`)}</span>
                  <span className="finding-title">{findingTitle(f)}</span>
                  {showMember && f.member && <span className="muted">{f.member}</span>}
                </div>
                <details>
                  <summary>{t("diag.findings.fix")}</summary>
                  <p>{findingFix(f)}</p>
                </details>
                {(command !== null || links.length > 0) && (
                  <div className="finding-actions">
                    {command !== null && <CopyButton text={command} label={t("diag.findings.copy")} />}
                    {links.map((link) =>
                      link.kind === "action" ? (
                        <button key={actionKey(link)} type="button" onClick={() => onAction?.(link)}>
                          {link.action.type === "terminateIdleInTransaction"
                            ? t("actions.findings.terminateIdle", { duration: formatDuration(link.action.olderThanS) })
                            : t("actions.findings.terminateRoot", { pid: link.action.pid })}
                        </button>
                      ) : (
                        <button
                          key={`${link.tool}:${link.member ?? ""}`}
                          type="button"
                          onClick={() => open({ kind: "diag", connectionId, tool: link.tool, member: link.member })}
                        >
                          {t("diag.findings.open", { tool: t(`diag.tools.${link.tool}`) })}
                        </button>
                      ),
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
