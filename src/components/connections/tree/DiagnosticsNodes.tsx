import { useTranslation } from "react-i18next";
import type { ConnectionKind, DiagTool } from "../../../api/types";
import { useWorkspace } from "../../../state/workspace";
import { TreeGroup } from "./TreeGroup";

/** Инструменты в дереве (спец. P2 §7.1); у кластера Patroni — ещё готовность к failover и матрица health. */
export function DIAG_TOOLS_FOR(kind: ConnectionKind): DiagTool[] {
  const common: DiagTool[] = ["pulse", "sessions", "idleTx", "waits", "locks", "horizon", "replication", "slots"];
  return kind === "patroni" ? [...common, "failover", "health"] : common;
}

/** Группа «Диагностика» раскрытого подключения: клик по инструменту открывает его вкладку «за лидером»; последний узел — журнал действий. */
export function DiagnosticsNodes({ connectionId, kind }: { connectionId: string; kind: ConnectionKind }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  return (
    <TreeGroup label={t("sidebar.diagnostics")}>
      {() => [
        ...DIAG_TOOLS_FOR(kind).map((tool) => (
          <li key={tool}>
            <button type="button" className="link" onClick={() => open({ kind: "diag", connectionId, tool })}>
              {t(`diag.tools.${tool}`)}
            </button>
          </li>
        )),
        <li key="actions">
          <button type="button" className="link" onClick={() => open({ kind: "actions", connectionId })}>
            {t("actions.journal.title")}
          </button>
        </li>,
      ]}
    </TreeGroup>
  );
}
