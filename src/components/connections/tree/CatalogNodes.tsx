import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ConnectionProfile, DatabaseInfo, ObjectKind, SchemaInfo, TopologyView } from "../../../api/types";
import { formatBytes } from "../../../lib/format";
import { roleLabel } from "../../../lib/topology";
import { disconnectedView, useTopology } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Glyph } from "../../common/Glyph";
import { DiagnosticsNodes } from "./DiagnosticsNodes";
import { ObjectList } from "./ObjectList";
import { TreeGroup } from "./TreeGroup";
import { type TreeFocus, useFocusMenuItem } from "./treeFocus";

/** Лаг реплики, начиная с которого узел помечается предупреждением. */
const LAG_WARN_BYTES = 16 * 1024 * 1024;

/**
 * Настройка «Показывать системные объекты». Пока она не прочитана, запросы каталога не стартуют:
 * иначе первый ушёл бы с неверным значением и тут же был бы повторён. Ошибка чтения не блокирует
 * дерево — действует значение по умолчанию.
 */
function useShowSystem(): { showSystem: boolean; ready: boolean } {
  const q = useQuery({ queryKey: ["uiSettings"], queryFn: api.getUiSettings });
  return { showSystem: q.data?.showSystemObjects ?? false, ready: !q.isPending };
}

function Loading({ q }: { q: { isPending: boolean; error: unknown; refetch: () => unknown } }) {
  const { t } = useTranslation();
  if (q.isPending) {
    return (
      <li>
        <span className="tree-note" role="status">
          {t("common.loading")}
        </span>
      </li>
    );
  }
  if (q.error) {
    return (
      <li>
        <ErrorBanner error={q.error} onRetry={() => void q.refetch()} />
      </li>
    );
  }
  return null;
}

const KINDS: { kind: ObjectKind; count: (s: SchemaInfo) => number }[] = [
  { kind: "tables", count: (s) => s.counts.tables },
  { kind: "views", count: (s) => s.counts.views },
  { kind: "matviews", count: (s) => s.counts.matviews },
  { kind: "functions", count: (s) => s.counts.functions },
  { kind: "sequences", count: (s) => s.counts.sequences },
];

function SchemaNode({ connectionId, db, schema, defaultOpen }: { connectionId: string; db: string; schema: SchemaInfo; defaultOpen?: boolean }) {
  const { t } = useTranslation();
  const menu = useFocusMenuItem({ kind: "schema", connectionId, db, schema: schema.name });
  return (
    <TreeGroup label={schema.name} menu={menu} menuLabel={t("sidebar.objectMenu", { name: schema.name })} defaultOpen={defaultOpen}>
      {() =>
        KINDS.filter((k) => k.count(schema) > 0).map((k) => {
          const label = t(`sidebar.kinds.${k.kind}`);
          return (
            <TreeGroup key={k.kind} label={`${label} (${k.count(schema)})`}>
              {() => (
                <li>
                  <ObjectList connectionId={connectionId} db={db} schema={schema.name} kind={k.kind} count={k.count(schema)} label={label} />
                </li>
              )}
            </TreeGroup>
          );
        })
      }
    </TreeGroup>
  );
}

function useSchemas(connectionId: string, db: string) {
  const { showSystem, ready } = useShowSystem();
  return useQuery({
    queryKey: ["catalog", connectionId, "schemas", db, showSystem],
    queryFn: () => api.listSchemas(connectionId, db, showSystem),
    enabled: ready,
  });
}

function SchemaList({ connectionId, db }: { connectionId: string; db: string }) {
  const q = useSchemas(connectionId, db);
  return (
    <>
      <Loading q={q} />
      {q.data?.map((s) => <SchemaNode key={s.name} connectionId={connectionId} db={db} schema={s} />)}
    </>
  );
}

function ExtensionList({ connectionId, db }: { connectionId: string; db: string }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["catalog", connectionId, "extensions", db], queryFn: () => api.listExtensions(connectionId, db) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((e) => (
        <li key={e.name}>
          <span className="tree-name">{e.name}</span> <span className="muted">{e.version}</span>
          {e.updateAvailable && (
            <>
              {" "}
              <Glyph className="warn" symbol="↑" label={t("sidebar.extensionUpdate", { version: e.defaultVersion })} />
            </>
          )}
        </li>
      ))}
    </>
  );
}

function DatabaseNode({ connectionId, db: d, defaultOpen }: { connectionId: string; db: DatabaseInfo; defaultOpen?: boolean }) {
  const { t } = useTranslation();
  const menu = useFocusMenuItem({ kind: "database", connectionId, db: d.name });
  return (
    <TreeGroup
      label={d.name}
      menu={menu}
      menuLabel={t("sidebar.objectMenu", { name: d.name })}
      defaultOpen={defaultOpen}
      extra={
        d.sizeBytes === null ? (
          <Glyph className="muted" symbol="—" label={t("sidebar.sizeUnavailable")} />
        ) : (
          <span className="muted">{formatBytes(d.sizeBytes)}</span>
        )
      }
    >
      {() => (
        <>
          <SchemaList connectionId={connectionId} db={d.name} />
          <TreeGroup label={t("sidebar.extensions")}>{() => <ExtensionList connectionId={connectionId} db={d.name} />}</TreeGroup>
        </>
      )}
    </TreeGroup>
  );
}

function useDatabases(connectionId: string) {
  return useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId) });
}

function DatabaseList({ connectionId }: { connectionId: string }) {
  const q = useDatabases(connectionId);
  return (
    <>
      <Loading q={q} />
      {q.data?.map((d) => <DatabaseNode key={d.name} connectionId={connectionId} db={d} />)}
    </>
  );
}

function FocusedDatabase({ connectionId, db }: { connectionId: string; db: string }) {
  const { t } = useTranslation();
  const q = useDatabases(connectionId);
  const found = q.data?.find((d) => d.name === db);
  if (!q.data) return <Loading q={q} />;
  return found ? <DatabaseNode connectionId={connectionId} db={found} defaultOpen /> : <li className="tree-notice">{t("sidebar.focus.gone")}</li>;
}

function FocusedSchema({ connectionId, db, schema }: { connectionId: string; db: string; schema: string }) {
  const { t } = useTranslation();
  const q = useSchemas(connectionId, db);
  const found = q.data?.find((s) => s.name === schema);
  if (!q.data) return <Loading q={q} />;
  return found ? <SchemaNode connectionId={connectionId} db={db} schema={found} defaultOpen /> : <li className="tree-notice">{t("sidebar.focus.gone")}</li>;
}

/**
 * Ветка базы или схемы в режиме фокуса дерева: корень раскрыт сразу. Каталог читается с узла каталога подключения;
 * подключение закрыто или узла каталога нет — сообщение вместо ветки (вернуться — кнопкой «Назад» над деревом).
 */
export function FocusedCatalog({ focus }: { focus: Extract<TreeFocus, { kind: "database" | "schema" }> }) {
  const { t } = useTranslation();
  const { data: view = disconnectedView } = useTopology(focus.connectionId);
  const live = view.state === "connected" || view.state === "degraded";
  let body;
  if (!live) body = <li className="tree-notice">{t("sidebar.focus.offline")}</li>;
  else if (!view.catalogMember) body = <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>;
  else if (focus.kind === "database") body = <FocusedDatabase connectionId={focus.connectionId} db={focus.db} />;
  else body = <FocusedSchema connectionId={focus.connectionId} db={focus.db} schema={focus.schema} />;
  return <ul className="tree">{body}</ul>;
}

function RoleList({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const { showSystem, ready } = useShowSystem();
  const q = useQuery({
    queryKey: ["catalog", connectionId, "roles", showSystem],
    queryFn: () => api.listRoles(connectionId, showSystem),
    enabled: ready,
  });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((r) => (
        <li key={r.name}>
          <span className="tree-name">{r.name}</span>
          {r.superuser && (
            <span className="env-badge" style={{ background: "var(--danger)" }}>
              {t("sidebar.superuser")}
            </span>
          )}
          {!r.login && <span className="muted"> {t("sidebar.noLogin")}</span>}
        </li>
      ))}
    </>
  );
}

/** Дочерние узлы подключения: участники кластера и каталог (базы, роли) с выбранного узла. */
export function CatalogNodes({ connection, view }: { connection: ConnectionProfile; view: TopologyView }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const topo = view.topology;
  const member = view.catalogMember;
  return (
    // Список назван по подключению: «Узлы», «Базы данных» и «Роли» разных подключений различимы.
    <ul className="tree-catalog" aria-label={t("sidebar.catalogOf", { name: connection.name })}>
      {connection.kind !== "server" && topo && (
        <TreeGroup label={t("sidebar.members")}>
          {() =>
            topo.members.map((m) => (
              <li key={m.name}>
                <button type="button" className="link" onClick={() => open({ kind: "overview", connectionId: connection.id, member: m.name })}>
                  {m.role === "primary" || m.role === "standbyLeader" ? "★ " : "○ "}
                  {m.name}
                </button>{" "}
                <span className="muted">{roleLabel(m.role, topo.kind)}</span>
                {m.timeline !== null && m.role === "primary" && (
                  <span className="muted" title={t("sidebar.timelineTitle")}>
                    {" "}
                    {t("sidebar.timeline", { timeline: m.timeline })}
                  </span>
                )}
                {m.lagBytes !== null && m.role !== "primary" && <span className="muted"> {formatBytes(m.lagBytes)}</span>}
                {(!m.reachable || (m.lagBytes ?? 0) > LAG_WARN_BYTES) && (
                  <>
                    {" "}
                    <Glyph className="warn" symbol="⚠" label={m.reachable ? t("sidebar.lagHigh") : t("sidebar.unreachable")} />
                  </>
                )}
              </li>
            ))
          }
        </TreeGroup>
      )}
      {view.catalogFallback && member && <li className="tree-notice">{t("sidebar.catalogFallback", { member })}</li>}
      {!member && <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>}
      {/* Группы не размонтируются, когда узел каталога пропал (например, на время перезапуска сервера): */}
      {/* раскрытое состояние переживает один неудачный опрос. Без узла внутри — то же сообщение, запросов нет. */}
      {topo && (
        <>
          <TreeGroup label={member && connection.kind !== "server" ? t("sidebar.databasesFrom", { member }) : t("sidebar.databases")}>
            {() => (member ? <DatabaseList connectionId={connection.id} /> : <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>)}
          </TreeGroup>
          <TreeGroup label={t("sidebar.roles")}>
            {() => (member ? <RoleList connectionId={connection.id} /> : <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>)}
          </TreeGroup>
          {/* Инструменты диагностики сами выбирают узел («за лидером»), узел каталога им не нужен. */}
          <DiagnosticsNodes connectionId={connection.id} kind={connection.kind} />
        </>
      )}
    </ul>
  );
}
