import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { ConnectionKind, DiagReport, DiagTool, Finding, PulseData, PulseFacts, PulseNode, PulseRates, Severity, TopologyView } from "../../api/types";
import { i18n } from "../../i18n";
import { formatBytes, formatCount, formatDuration, formatPercent } from "../../lib/format";
import { roleLabel } from "../../lib/topology";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";

/** Содержимое клетки: текст и необязательная подсказка (наведение). */
interface Cell {
  text: string;
  detail?: string;
}

/** Колонка таблицы пульса: что показывает, какие находки её красят и в какой инструмент ведёт клик. */
export interface PulseColumn {
  key: string;
  /** Ключ подписи — литералом: так его видит `pnpm i18n:check`. */
  labelKey: string;
  /** Инструмент, который открывает клик по клетке (на узле строки). */
  tool: DiagTool;
  /** Находки, окрашивающие клетку (по `Finding.id`). */
  findings: string[];
  value(facts: PulseFacts, rates: PulseRates | null): Cell;
}

/** Скорости известны, когда есть сравнимый снимок счётчиков: у первого вызова все поля пусты, и «0» было бы ложью. */
const ratesKnown = (r: PulseRates | null): r is PulseRates => r !== null && r.tps !== null;
const pending = (): Cell => ({ text: i18n.t("diag.pulse.ratePending") });
const join = (parts: string[]) => (parts.length > 0 ? parts.join(" · ") : "—");

/**
 * Показатели пульса по порядку колонок (спец. P2 §7.4): значение, находки, которые красят клетку, и инструмент перехода.
 * Один показатель — один инструмент, где виновные видны поимённо.
 */
export const PULSE_COLUMNS: PulseColumn[] = [
  {
    key: "connections",
    labelKey: "diag.pulse.columns.connections",
    tool: "sessions",
    findings: ["connectionsSaturated"],
    value: (f) => ({
      text: f.available > 0 ? `${formatCount(f.used)} / ${formatCount(f.available)} (${formatPercent((f.used * 100) / f.available)})` : formatCount(f.used),
    }),
  },
  {
    key: "tps",
    labelKey: "diag.pulse.columns.tps",
    tool: "locks",
    findings: ["deadlocks"],
    value: (_f, r) =>
      ratesKnown(r)
        ? { text: formatCount(r.tps, 1), detail: i18n.t("diag.pulse.tpsTitle", { commits: formatCount(r.commitsPerS, 1), rollbacks: formatCount(r.rollbacksPerS, 1) }) }
        : pending(),
  },
  {
    key: "cache",
    labelKey: "diag.pulse.columns.cache",
    tool: "sessions",
    findings: ["cacheHitLow"],
    // Без чтений блоков доли нет: узел не работает с данными, это не «ещё не известно».
    value: (_f, r) => (ratesKnown(r) ? { text: r.cacheHitPct === null ? "—" : formatPercent(r.cacheHitPct) } : pending()),
  },
  {
    key: "wal",
    labelKey: "diag.pulse.columns.wal",
    tool: "replication",
    findings: [],
    value: (_f, r) =>
      ratesKnown(r) ? { text: r.walBytesPerS === null ? "—" : i18n.t("diag.pulse.perSecond", { value: formatBytes(Math.max(0, r.walBytesPerS)) }) } : pending(),
  },
  { key: "xact", labelKey: "diag.pulse.columns.xact", tool: "sessions", findings: ["longTransaction"], value: (f) => ({ text: formatDuration(f.longestTransactionS) }) },
  { key: "query", labelKey: "diag.pulse.columns.query", tool: "sessions", findings: ["longQuery"], value: (f) => ({ text: formatDuration(f.longestQueryS) }) },
  { key: "idle", labelKey: "diag.pulse.columns.idle", tool: "idleTx", findings: ["idleInTransaction"], value: (f) => ({ text: formatDuration(f.longestIdleInTransactionS) }) },
  {
    key: "locks",
    labelKey: "diag.pulse.columns.locks",
    tool: "locks",
    findings: ["lockWait"],
    value: (f) => ({ text: f.lockWaiting > 0 ? `${formatCount(f.lockWaiting)} · ${formatDuration(f.longestLockWaitS)}` : formatCount(0) }),
  },
  {
    key: "xid",
    labelKey: "diag.pulse.columns.xid",
    tool: "horizon",
    findings: ["xidAge"],
    value: (f) => ({ text: `${formatCount(f.maxXidAge)} (${formatPercent(f.xidAgePct)})` }),
  },
  {
    key: "walDir",
    labelKey: "diag.pulse.columns.walDir",
    tool: "slots",
    findings: ["walDirSize", "tempFiles"],
    value: (f) => ({
      text: f.walDirBytes === null ? "—" : formatBytes(f.walDirBytes),
      detail: f.maxWalSizeBytes === null ? undefined : i18n.t("diag.pulse.walDirTitle", { value: formatBytes(f.maxWalSizeBytes) }),
    }),
  },
  {
    key: "lag",
    labelKey: "diag.pulse.columns.lag",
    tool: "replication",
    findings: ["replicaLag"],
    value: (f) => ({
      text: join([...(f.lagBytes === null ? [] : [formatBytes(f.lagBytes)]), ...(f.replayDelayMs === null ? [] : [formatDuration(f.replayDelayMs / 1000)])]),
    }),
  },
];

const RANK: Record<Severity, number> = { info: 0, warning: 1, critical: 2 };
/** Метка строгости рядом со значением: цвет клетки не единственный признак. */
const MARK: Record<Severity, string> = { info: "i", warning: "!", critical: "!!" };

/** Худшая строгость находок узла по клетке; `null` — находок нет. */
function worstSeverity(findings: Finding[], member: string, ids: string[]): Severity | null {
  let worst: Severity | null = null;
  for (const f of findings) {
    if (f.member !== member || !ids.includes(f.id)) continue;
    if (worst === null || RANK[f.severity] > RANK[worst]) worst = f.severity;
  }
  return worst;
}

function PulseRow({ node, columns, connectionId, findings, kind }: { node: PulseNode; columns: PulseColumn[]; connectionId: string; findings: Finding[]; kind: ConnectionKind }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const { facts, rates } = node;
  return (
    <tr>
      <th scope="row">
        {node.member} <span className="muted">{roleLabel(node.role, kind)}</span>
      </th>
      {facts === null ? (
        <td colSpan={columns.length}>
          <span>{t("diag.pulse.unreachable")}</span>
          {node.error && <ErrorBanner error={node.error} />}
        </td>
      ) : (
        columns.map((col) => {
          const cell = col.value(facts, rates);
          const sev = worstSeverity(findings, node.member, col.findings);
          const tool = t(`diag.tools.${col.tool}`);
          return (
            <td key={col.key} className={sev ? `pulse-cell sev-${sev}` : "pulse-cell"}>
              <button type="button" title={cell.detail ?? t("diag.pulse.open", { tool, member: node.member })} onClick={() => open({ kind: "diag", connectionId, tool: col.tool, member: node.member })}>
                {sev && (
                  <>
                    <span className="sev-mark" aria-hidden="true">
                      {MARK[sev]}
                    </span>
                    <span className="sr-only">{t(`diag.severity.${sev}`)}: </span>
                  </>
                )}
                <span>{cell.text}</span>
              </button>
            </td>
          );
        })
      )}
    </tr>
  );
}

function PulseTable({ connectionId, data, findings }: { connectionId: string; data: PulseData; findings: Finding[] }) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  // Вид подключения нужен подписи роли («Leader» у Patroni, «Primary» у списка узлов); вкладка уже загрузила топологию.
  const kind = qc.getQueryData<TopologyView>(["topology", connectionId])?.topology?.kind ?? "server";
  return (
    <div className="table-scroll">
      <table className="pulse-table" aria-label={t("diag.pulse.table")}>
        <thead>
          <tr>
            <th scope="col">{t("diag.pulse.node")}</th>
            {PULSE_COLUMNS.map((c) => (
              <th key={c.key} scope="col">
                {t(c.labelKey)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {data.nodes.map((n) => (
            <PulseRow key={n.member} node={n} columns={PULSE_COLUMNS} connectionId={connectionId} findings={findings} kind={kind} />
          ))}
        </tbody>
      </table>
    </div>
  );
}

/**
 * Пульс (спец. P2 §7.4): строка на узел, колонка на показатель. Клетка окрашена худшей находкой узла по этому показателю
 * и открывает инструмент, где видны виновные, сразу на этом узле. Недоступный узел — строка с причиной.
 */
export function PulseView({ connectionId, report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "pulse") return null;
  return <PulseTable connectionId={connectionId} data={report.data} findings={report.findings} />;
}
