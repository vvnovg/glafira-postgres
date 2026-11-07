import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import type { DiagTool, Finding, FindingLink, FindingTarget, Severity } from "../../api/types";
import { formatDuration } from "../../lib/format";
import { useWorkspace } from "../../state/workspace";
import { CopyButton } from "../common/CopyButton";
import { queryTitle } from "../queries/queriesText";
import { findingFix, findingTitle } from "./findingText";
import { Icon } from "../common/Icon";

/** Сколько знаков текста запроса показывает находка (полный — в подсказке). */
const QUERY_PREVIEW = 90;
/** Сколько находок видно, пока список не раскрыт кнопкой «Ещё N» (смоук P4c, R13): таблица инструмента — на первом экране. */
const FIRST_SHOWN = 3;
/** Блоки находок, свёрнутые заголовком: список областей (инструмент; у карточки запроса — `query`). */
const COLLAPSED_KEY = "glafira.findings.collapsed";

function readCollapsed(): string[] {
  try {
    const raw = localStorage.getItem(COLLAPSED_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function writeCollapsed(scope: string, collapsed: boolean) {
  try {
    const rest = readCollapsed().filter((x) => x !== scope);
    localStorage.setItem(COLLAPSED_KEY, JSON.stringify(collapsed ? [...rest, scope] : rest));
  } catch {
    // Хранилище недоступно — состояние живёт, пока открыта вкладка.
  }
}

type ActionLink = Extract<FindingLink, { kind: "action" }>;

/** Порядок показа: сначала более строгие; внутри одной строгости остаётся порядок сервера (сортировка устойчива). */
const SEVERITY_ORDER: Record<Severity, number> = { critical: 0, warning: 1, info: 2 };

/** Часть ключа находки по её объекту: сессия — `pid` и время старта backend (pid переиспользуется), запрос — база и `queryId`, остальное — имя. */
const targetKey = (t: FindingTarget | null) =>
  t === null ? "" : t.kind === "session" ? `${t.pid}:${t.backendStartUs}` : t.kind === "query" ? `${t.database}:${t.queryId}` : t.name;

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
 * и кнопки: переход в другой инструмент, карточка запроса и действия над сессиями (только при `onAction`). Без находок —
 * «Проблем не найдено». `showMember` — у отчёта кластера: находки относятся к разным узлам, и узел показывается отдельной
 * меткой.
 * Смоук P4c, R13: видны первые три находки по строгости, остальные — по «Ещё N» («Свернуть» возвращает три). Весь блок
 * сворачивается заголовком; свёрнутость запоминается для инструмента (`tool`, у карточки запроса — своя) в localStorage.
 */
export function FindingsPanel({
  connectionId,
  findings,
  showMember = false,
  tool,
  member,
  onAction,
  queryText,
}: {
  connectionId: string;
  findings: Finding[];
  showMember?: boolean;
  /** Инструмент и узел вкладки, где показан блок: переход «в себя» не предлагается. */
  tool?: DiagTool;
  member?: string | null;
  /** Кнопки действий находок (отмена, завершение сессий); без обработчика действий нет — подключение только для чтения. */
  onAction?: (link: ActionLink) => void;
  /** Текст запроса по базе и `queryId` (из строк отчёта); нужен находкам о запросе, чтобы показать, о каком запросе речь. `null` — текста нет. */
  queryText?: (database: string, queryId: string) => string | null;
}) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const titleId = useId();
  const scope = tool ?? "query";
  const [expanded, setExpanded] = useState(() => !readCollapsed().includes(scope));
  const [showAll, setShowAll] = useState(false);
  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    writeCollapsed(scope, !next);
  };
  const sorted = [...findings].sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity]);
  const keys = findingKeys(sorted);
  const hidden = sorted.length - FIRST_SHOWN;
  const shown = showAll || hidden <= 0 ? sorted : sorted.slice(0, FIRST_SHOWN);
  return (
    <section className="findings" aria-labelledby={titleId}>
      {/* Заголовок — сама кнопка сворачивания: имя — видимый текст с числом находок, состояние — aria-expanded. */}
      <h3 id={titleId} className="findings-head">
        <button type="button" className="findings-toggle" aria-expanded={expanded} title={expanded ? t("diag.findings.collapse") : t("diag.findings.expand")} onClick={toggle}>
          <span className={expanded ? "findings-caret open" : "findings-caret"} aria-hidden="true">
            <Icon name="chevron" size={14} />
          </span>
          <span>
            {t("diag.findings.title")}
            {findings.length > 0 && (
              <>
                {" "}
                <span className="muted">({findings.length})</span>
              </>
            )}
          </span>
        </button>
      </h3>
      {expanded && findings.length === 0 && <p className="muted">{t("diag.findings.none")}</p>}
      {expanded && findings.length > 0 && (
        <ul className="findings-list">
          {shown.map((f, i) => {
            const command = typeof f.params.command === "string" ? f.params.command : null;
            // Ссылка без узла ведёт туда же, что и узел отчёта: вкладка инструмента одна на подключение.
            // Ссылки-действия (`kind: "action"`) — кнопки только при `onAction` и известном узле (свой у ссылки или узел отчёта):
            // без узла выполнять негде. Карточка запроса (`openQuery`) — только при известном узле и базе: пустая база
            // (строка pgss без `datname`) — база неизвестна. Переход «в себя» не предлагается.
            const links = f.links.filter((l) => {
              if (l.kind === "openQuery") return l.database !== "" && (l.member ?? member ?? null) !== null;
              if (l.kind === "action") return onAction !== undefined && (l.member ?? member ?? null) !== null;
              return !(l.tool === tool && (l.member == null || l.member === member));
            });
            return (
              <li key={keys[i]} className={`finding sev-${f.severity}`}>
                <div className="finding-main">
                  <span className="finding-sev">{t(`diag.severity.${f.severity}`)}</span>
                  <span className="finding-title">{findingTitle(f)}</span>
                  {showMember && f.member && <span className="muted">{f.member}</span>}
                </div>
                {queryText && f.target?.kind === "query" && (
                  <FindingQuery text={queryText(f.target.database, f.target.queryId)} queryId={f.target.queryId} />
                )}
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
                      ) : link.kind === "openQuery" ? (
                        <button
                          key={`query:${link.database}:${link.queryId}`}
                          type="button"
                          onClick={() => {
                            const node = link.member ?? member ?? null;
                            if (node !== null) open({ kind: "query", connectionId, member: node, database: link.database, queryId: link.queryId });
                          }}
                        >
                          {t("diag.findings.openQuery")}
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
      {/* Одна кнопка «Ещё N» / «Свернуть»: подпись меняется, фокус остаётся на ней. */}
      {expanded && hidden > 0 && (
        <button type="button" className="link findings-more" onClick={() => setShowAll(!showAll)}>
          {showAll ? t("diag.findings.less") : t("diag.findings.more", { count: hidden })}
        </button>
      )}
    </section>
  );
}

/** Начало текста запроса, о котором находка (mono, `title` — полный текст); без текста — `queryId`. */
function FindingQuery({ text, queryId }: { text: string | null; queryId: string }) {
  return (
    <p className="finding-query mono muted" title={text ?? undefined}>
      {queryTitle(text, queryId, QUERY_PREVIEW)}
    </p>
  );
}
