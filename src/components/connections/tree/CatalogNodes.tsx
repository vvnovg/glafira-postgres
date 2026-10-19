import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../../../api/client";
import type { ConnectionProfile, ObjectKind, SchemaInfo, TopologyView } from "../../../api/types";
import { formatBytes } from "../../../lib/format";
import { roleLabel } from "../../../lib/topology";
import { useWorkspace } from "../../../state/workspace";
import { ErrorBanner } from "../../common/ErrorBanner";
import { Glyph } from "../../common/Glyph";
import { ObjectList } from "./ObjectList";

/** Лаг реплики, начиная с которого узел помечается предупреждением. */
const LAG_WARN_BYTES = 16 * 1024 * 1024;

/** Ленивый узел дерева: содержимое строится только в раскрытом виде. */
export function TreeGroup({ label, extra, children }: { label: string; extra?: ReactNode; children: () => ReactNode }) {
  const [open, setOpen] = useState(false);
  const toggleId = useId();
  return (
    <li>
      <div className="tree-conn tree-wrap">
        <button type="button" id={toggleId} className="tree-folder-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="tree-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
          <span className="tree-name">{label}</span>
        </button>
        {extra}
      </div>
      {/* Список назван по кнопке-заголовку: вложенные «Таблицы» разных схем различимы. */}
      {open && <ul aria-labelledby={toggleId}>{children()}</ul>}
    </li>
  );
}

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

function SchemaNode({ connectionId, db, schema }: { connectionId: string; db: string; schema: SchemaInfo }) {
  const { t } = useTranslation();
  return (
    <TreeGroup label={schema.name}>
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

function SchemaList({ connectionId, db }: { connectionId: string; db: string }) {
  const { showSystem, ready } = useShowSystem();
  const q = useQuery({
    queryKey: ["catalog", connectionId, "schemas", db, showSystem],
    queryFn: () => api.listSchemas(connectionId, db, showSystem),
    enabled: ready,
  });
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

function DatabaseList({ connectionId }: { connectionId: string }) {
  const { t } = useTranslation();
  const q = useQuery({ queryKey: ["catalog", connectionId, "databases"], queryFn: () => api.listDatabases(connectionId) });
  return (
    <>
      <Loading q={q} />
      {q.data?.map((d) => (
        <TreeGroup
          key={d.name}
          label={d.name}
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
      ))}
    </>
  );
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
      {member ? (
        <TreeGroup label={connection.kind === "server" ? t("sidebar.databases") : t("sidebar.databasesFrom", { member })}>
          {() => <DatabaseList connectionId={connection.id} />}
        </TreeGroup>
      ) : (
        <li className="tree-notice">{t("sidebar.noCatalogNode")}</li>
      )}
      {member && <TreeGroup label={t("sidebar.roles")}>{() => <RoleList connectionId={connection.id} />}</TreeGroup>}
    </ul>
  );
}
