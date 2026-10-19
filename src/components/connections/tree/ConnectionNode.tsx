import { useQueryClient } from "@tanstack/react-query";
import { type HTMLAttributes, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AppError, ConnectionProfile, TopologyView } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { disconnectedView, useTopology } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { EnvBadge } from "../../common/EnvBadge";
import { Glyph } from "../../common/Glyph";
import { CatalogNodes } from "./CatalogNodes";
import { FolderMenu, type FolderMenuItem } from "./FolderMenu";

/**
 * Узел подключения: раскрытие открывает подключение (Rust поднимает наблюдение), дочерние узлы —
 * участники, базы, роли. rowProps — атрибуты строки (перетаскивание), как в Glafira.
 */
export function ConnectionNode({ connection: c, rowProps }: { connection: ConnectionProfile; rowProps?: HTMLAttributes<HTMLDivElement> }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const qc = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  const { data: view = disconnectedView } = useTopology(c.id);
  const live = view.state === "connected" || view.state === "degraded";

  const setView = (v: TopologyView) => qc.setQueryData(["topology", c.id], v);

  const connect = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      setView(await api.connect(c.id));
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  /** Смена узла, с которого читается каталог: новый вид в кэш, каталог — перечитать заново. */
  const pickCatalogMember = (member: string | null) => {
    setError(null);
    void api
      .setCatalogMember(c.id, member)
      .then((v) => {
        setView(v);
        void qc.invalidateQueries({ predicate: (q) => q.queryKey[0] === "catalog" && q.queryKey[1] === c.id });
      })
      .catch((e: unknown) => setError(toAppError(e)));
  };

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    if (next && !live && view.state !== "connecting") void connect();
  };

  const members = view.topology?.members ?? [];
  const menu: FolderMenuItem[] = [
    { label: t("sidebar.overview"), onSelect: () => open({ kind: "overview", connectionId: c.id }) },
    { label: t("sidebar.editConnection"), onSelect: () => open({ kind: "connection-form", connectionId: c.id }) },
    ...(live && c.kind !== "server"
      ? [
          { label: t("sidebar.catalogDefault"), onSelect: () => pickCatalogMember(null) },
          ...members.map((m) => ({
            label: t("sidebar.catalogFrom", { member: m.name }),
            onSelect: () => pickCatalogMember(m.name),
          })),
        ]
      : []),
    ...(live || view.state === "failed"
      ? [
          {
            label: t("sidebar.disconnect"),
            onSelect: () =>
              void api
                .disconnect(c.id)
                .then(() => {
                  setError(null);
                  setExpanded(false);
                })
                .catch((e: unknown) => setError(toAppError(e))),
          },
        ]
      : [
          {
            label: t("sidebar.connect"),
            onSelect: () => {
              setExpanded(true);
              void connect();
            },
          },
        ]),
  ];

  // Ошибка последней попытки или причина, по которой Rust пометил подключение как недоступное.
  const shownError = error ?? (view.state === "failed" ? view.error : null);

  return (
    <li>
      <div {...rowProps} className={rowProps?.className ? `tree-conn ${rowProps.className}` : "tree-conn"}>
        <button type="button" className="tree-folder-toggle conn-toggle" aria-expanded={expanded} aria-label={t("sidebar.connectionToggle", { name: c.name })} onClick={toggle}>
          <span className="tree-caret" aria-hidden="true">
            {expanded ? "▾" : "▸"}
          </span>
        </button>
        <Glyph className={`conn-state conn-state-${view.state}`} symbol="" label={t(`sidebar.state.${view.state}`)} />
        <span className="dot" style={{ background: c.color }} />
        <button type="button" className="link tree-name" onClick={() => open({ kind: "overview", connectionId: c.id })}>
          {c.name}
        </button>
        <EnvBadge environment={c.environment} readOnly={c.readOnly} />
        {c.kind === "patroni" && <span className="kind-badge">Patroni</span>}
        <span className="tree-menu-anchor">
          <button
            type="button"
            className="icon"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            aria-label={t("sidebar.connectionMenu", { name: c.name })}
            onClick={() => setMenuOpen(!menuOpen)}
          >
            ⋯
          </button>
          {menuOpen && <FolderMenu label={t("sidebar.connectionMenu", { name: c.name })} items={menu} onClose={() => setMenuOpen(false)} />}
        </span>
      </div>
      {expanded && (
        <>
          {(view.state === "connecting" || busy) && <p className="tree-notice">{t("sidebar.state.connecting")}</p>}
          {shownError && (
            <p className="tree-error" role="alert">
              {tr(shownError)}
            </p>
          )}
          {live && <CatalogNodes connection={c} view={view} />}
        </>
      )}
    </li>
  );
}
