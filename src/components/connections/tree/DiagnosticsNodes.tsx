import { useTranslation } from "react-i18next";
import type { ConnectionKind, DiagTool } from "../../../api/types";
import { useWorkspace } from "../../../state/workspace";
import type { IconName } from "../../common/Icon";
import { TreeGroup } from "./TreeGroup";
import { TreeRow } from "./TreeRow";
import { diagLeafIsActive, inDiagnostics, useActiveTab } from "./treeActive";

/** Инструменты в дереве (спец. P2 §7.1); у кластера Patroni — ещё готовность к failover и матрица health. */
export function DIAG_TOOLS_FOR(kind: ConnectionKind): DiagTool[] {
  const common: DiagTool[] = ["pulse", "sessions", "idleTx", "waits", "locks", "horizon", "replication", "slots", "queries"];
  return kind === "patroni" ? [...common, "failover", "health"] : common;
}

/** Значки инструментов (спец. переноса U1 §3.2). */
const TOOL_ICON: Record<DiagTool, IconName> = {
  pulse: "pulse",
  sessions: "sessions",
  idleTx: "clock",
  waits: "hourglass",
  locks: "lock",
  horizon: "horizon",
  replication: "replication",
  slots: "slot",
  queries: "code",
  failover: "failover",
  health: "health",
};

/**
 * Группа «Диагностика» раскрытого подключения: клик по инструменту открывает его вкладку «за лидером»; предпоследний — консоль
 * EXPLAIN, последний — журнал действий. Строка активной вкладки подсвечена (карточка запроса — «Запросы»), свёрнутая группа
 * с ней — сама.
 */
export function DiagnosticsNodes({ connectionId, kind }: { connectionId: string; kind: ConnectionKind }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const active = useActiveTab();
  return (
    <TreeGroup label={t("sidebar.diagnostics")} icon="gauge" holdsActive={inDiagnostics(active, connectionId)}>
      {() => [
        ...DIAG_TOOLS_FOR(kind).map((tool) => (
          <li key={tool}>
            <TreeRow
              icon={TOOL_ICON[tool]}
              name={t(`diag.tools.${tool}`)}
              current={diagLeafIsActive(active, connectionId, { kind: "diag", tool })}
              onActivate={() => open({ kind: "diag", connectionId, tool })}
            />
          </li>
        )),
        <li key="explain">
          <TreeRow
            icon="plan"
            name={t("diag.explain.title")}
            current={diagLeafIsActive(active, connectionId, { kind: "explain" })}
            onActivate={() => open({ kind: "explain", connectionId })}
          />
        </li>,
        <li key="actions">
          <TreeRow
            icon="logs"
            name={t("actions.journal.title")}
            current={diagLeafIsActive(active, connectionId, { kind: "actions" })}
            onActivate={() => open({ kind: "actions", connectionId })}
          />
        </li>,
      ]}
    </TreeGroup>
  );
}
