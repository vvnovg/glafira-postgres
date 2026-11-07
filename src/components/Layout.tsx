import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import type { ConnectionProfile } from "../api/types";
import { ENV_COLORS } from "../lib/env";
import { type Tab, useWorkspace } from "../state/workspace";
import { useWorkspacePersistence } from "../state/workspacePersist";
import { UpdateNotice } from "./about/UpdateNotice";
import { EnvBadge } from "./common/EnvBadge";
import { FolderMenu } from "./connections/tree/FolderMenu";
import { ApprovalDialog } from "./mcp/ApprovalDialog";
import { Sidebar } from "./Sidebar";
import { i18n } from "../i18n";
import { useTabMarks } from "../state/tabMarks";
import { Glyph } from "./common/Glyph";
import { TabContent } from "./TabContent";
import { useConnectionEvents } from "../lib/useConnectionEvents";
import { useMcpEvents } from "../lib/useMcpStatus";
import { useHelpShortcut } from "../lib/useHelpShortcut";
import { hiddenTabIds, revealScrollLeft } from "../lib/tabOverflow";

function tabTitle(tab: Tab, byId: Map<string, ConnectionProfile>): string {
  const connId = "connectionId" in tab ? tab.connectionId : null;
  const conn = connId ? (byId.get(connId)?.name ?? i18n.t("tabs.unknownConnection")) : "";
  switch (tab.kind) {
    case "settings":
      return i18n.t("tabs.settings");
    case "about":
      return i18n.t("tabs.about");
    case "help":
      return i18n.t("tabs.help");
    case "connection-form":
      if (tab.copyOf) return i18n.t("tabs.copyConnection", { name: byId.get(tab.copyOf)?.name ?? i18n.t("tabs.unknownConnection") });
      return tab.connectionId ? i18n.t("tabs.connection", { name: conn }) : i18n.t("tabs.newConnection");
    case "overview":
      return i18n.t("tabs.overview", { conn });
    case "table":
    case "view":
    case "sequence":
      return `${tab.schema}.${tab.name}`;
    case "function":
      return tab.name;
    case "diag":
      return i18n.t("tabs.diag", { tool: i18n.t(`diag.tools.${tab.tool}`), conn });
    case "actions":
      return i18n.t("tabs.actions", { conn });
    case "query":
      return i18n.t("tabs.query", { text: tab.title ?? tab.queryId, conn });
    case "explain":
      return i18n.t("tabs.explain", { conn });
  }
}

/**
 * Переполнение строки вкладок: какие вкладки не видны целиком (пересчёт при прокрутке, изменении размера и составе
 * вкладок) и прокрутка активной вкладки в видимую часть. Вкладки строки помечены `data-tab-id`.
 */
function useTabOverflow(tabIds: string[], activeId: string | null) {
  const stripRef = useRef<HTMLElement>(null);
  const [hidden, setHidden] = useState<string[]>([]);
  const activeRef = useRef(activeId);
  activeRef.current = activeId;
  const measure = useCallback(() => {
    const strip = stripRef.current;
    if (!strip) return;
    const tabs = [...strip.querySelectorAll<HTMLElement>("[data-tab-id]")].map((el) => {
      const r = el.getBoundingClientRect();
      return { id: el.dataset.tabId ?? "", left: r.left, right: r.right };
    });
    const next = hiddenTabIds(strip.getBoundingClientRect(), tabs);
    setHidden((prev) => (prev.length === next.length && prev.every((id, i) => id === next[i]) ? prev : next));
  }, []);
  /** Активная вкладка — в видимую часть строки (смещение считается вручную, см. revealScrollLeft), затем пересчёт скрытых. */
  const reveal = useCallback(() => {
    const strip = stripRef.current;
    const id = activeRef.current;
    if (strip && id !== null) {
      const el = strip.querySelector<HTMLElement>(`[data-tab-id="${CSS.escape(id)}"]`);
      if (el) {
        const next = revealScrollLeft(strip.getBoundingClientRect(), el.getBoundingClientRect(), strip.scrollLeft);
        if (next !== strip.scrollLeft) strip.scrollLeft = next;
      }
    }
    measure();
  }, [measure]);
  const key = tabIds.join("\n");
  // При выборе вкладки, открытии новой и закрытии: сразу после раскладки и ещё раз в следующем кадре (шрифт и ширина
  // вкладок в WebKit могут досчитаться позже).
  useLayoutEffect(() => {
    reveal();
    if (typeof requestAnimationFrame === "undefined") return;
    const frame = requestAnimationFrame(reveal);
    return () => cancelAnimationFrame(frame);
  }, [reveal, activeId, key]);
  useEffect(() => {
    const strip = stripRef.current;
    if (!strip) return;
    // Прокрутка пользователем только пересчитывает скрытые; изменение размера окна или строки — ещё и возвращает активную.
    strip.addEventListener("scroll", measure, { passive: true });
    window.addEventListener("resize", reveal);
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(reveal);
    ro?.observe(strip);
    return () => {
      strip.removeEventListener("scroll", measure);
      window.removeEventListener("resize", reveal);
      ro?.disconnect();
    };
  }, [measure, reveal]);
  return { stripRef, hidden };
}

export function Layout() {
  const { t } = useTranslation(); // заодно перерисовывает заголовки вкладок при смене языка
  const { tabs, activeId, activate, close } = useWorkspace();
  const marks = useTabMarks((s) => s.marks);
  useHelpShortcut();
  useConnectionEvents();
  useMcpEvents();
  const connectionsQ = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  const connections = connectionsQ.data ?? [];
  // Вкладки прошлого запуска — когда известен список подключений (смоук P4c, R18); дальше рабочая область пишется сама.
  useWorkspacePersistence(connectionsQ.data);
  const byId = new Map(connections.map((c) => [c.id, c]));
  const active = tabs.find((x) => x.id === activeId);
  const activeConnId = active && "connectionId" in active ? active.connectionId : null;
  const activeConn = activeConnId ? byId.get(activeConnId) : undefined;
  const { stripRef, hidden } = useTabOverflow(
    tabs.map((x) => x.id),
    activeId,
  );
  const [moreOpen, setMoreOpen] = useState(false);
  // Меню не переживает исчезновение скрытых вкладок (окно расширили, вкладки закрыли).
  useEffect(() => {
    if (hidden.length === 0) setMoreOpen(false);
  }, [hidden.length]);
  const moreLabel = t("layout.hiddenTabs", { count: hidden.length });

  return (
    <div className="layout">
      <Sidebar />
      <main className="workspace">
        <div className="env-stripe" style={{ background: activeConn ? ENV_COLORS[activeConn.environment] : "transparent" }} />
        <UpdateNotice />
        <div className="tabbar-wrap">
          <nav
            className="tabbar"
            role="tablist"
            ref={stripRef}
            // Колесо мыши без Shift прокручивает строку вкладок по горизонтали: вертикально ей прокручиваться некуда.
            onWheel={(e) => {
              if (e.deltaY !== 0 && Math.abs(e.deltaY) > Math.abs(e.deltaX)) e.currentTarget.scrollLeft += e.deltaY;
            }}
          >
            {tabs.map((tab) => {
              const tConnId = "connectionId" in tab ? tab.connectionId : null;
              const conn = tConnId ? byId.get(tConnId) : undefined;
              const isActive = tab.id === activeId;
              return (
                <div key={tab.id} className={isActive ? "tab active" : "tab"} data-tab-id={tab.id} title={tabTitle(tab, byId)}>
                  {conn && <EnvBadge environment={conn.environment} readOnly={conn.readOnly} />}
                  <button type="button" role="tab" aria-selected={isActive} onClick={() => activate(tab.id)}>
                    {tabTitle(tab, byId)}
                    {marks[tab.id] && <Glyph symbol="●" label={t("llmAssist.panel.newDot")} className="tab-dot" />}
                  </button>
                  <button
                    type="button"
                    className="icon"
                    aria-label={t("layout.closeTab")}
                    onClick={(e) => {
                      e.stopPropagation();
                      close(tab.id);
                    }}
                  >
                    ×
                  </button>
                </div>
              );
            })}
          </nav>
          {hidden.length > 0 && (
            <span className="tree-menu-anchor tabbar-more">
              <button
                type="button"
                className="icon"
                title={moreLabel}
                aria-label={moreLabel}
                aria-haspopup="menu"
                aria-expanded={moreOpen}
                onClick={() => setMoreOpen((v) => !v)}
              >
                » {hidden.length}
              </button>
              {moreOpen && (
                <FolderMenu
                  label={moreLabel}
                  items={tabs
                    .filter((x) => hidden.includes(x.id))
                    .map((x) => ({ key: x.id, label: tabTitle(x, byId), onSelect: () => activate(x.id) }))}
                  onClose={() => setMoreOpen(false)}
                />
              )}
            </span>
          )}
        </div>
        {tabs.length === 0 && <p className="empty">{t("layout.empty")}</p>}
        {/* Неактивные вкладки скрыты, но не размонтированы — состояние форм сохраняется. */}
        {tabs.map((tab) => (
          <section key={tab.id} className="tab-content" hidden={tab.id !== activeId}>
            <TabContent tab={tab} active={tab.id === activeId} />
          </section>
        ))}
      </main>
      <ApprovalDialog />
    </div>
  );
}
