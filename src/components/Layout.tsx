import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api } from "../api/client";
import type { ConnectionProfile } from "../api/types";
import { ENV_COLORS } from "../lib/env";
import { type Tab, useWorkspace } from "../state/workspace";
import { EnvBadge } from "./common/EnvBadge";
import { LicenseNotice } from "./license/LicenseNotice";
import { Sidebar } from "./Sidebar";
import { i18n } from "../i18n";
import { TabContent } from "./TabContent";
import { useConnectionEvents } from "../lib/useConnectionEvents";
import { useHelpShortcut } from "../lib/useHelpShortcut";

function tabTitle(tab: Tab, byId: Map<string, ConnectionProfile>): string {
  const connId = "connectionId" in tab ? tab.connectionId : null;
  const conn = connId ? (byId.get(connId)?.name ?? "?") : "";
  switch (tab.kind) {
    case "settings":
      return i18n.t("tabs.settings");
    case "license":
      return i18n.t("tabs.license");
    case "about":
      return i18n.t("tabs.about");
    case "help":
      return i18n.t("tabs.help");
    case "connection-form":
      return tab.connectionId ? i18n.t("tabs.connection", { name: conn }) : i18n.t("tabs.newConnection");
    case "overview":
      return i18n.t("tabs.overview", { conn });
  }
}

export function Layout() {
  const { t } = useTranslation(); // заодно перерисовывает заголовки вкладок при смене языка
  const { tabs, activeId, activate, close } = useWorkspace();
  useHelpShortcut();
  useConnectionEvents();
  const { data: connections = [] } = useQuery({ queryKey: ["connections"], queryFn: api.listConnections });
  const byId = new Map(connections.map((c) => [c.id, c]));
  const active = tabs.find((x) => x.id === activeId);
  const activeConnId = active && "connectionId" in active ? active.connectionId : null;
  const activeConn = activeConnId ? byId.get(activeConnId) : undefined;

  return (
    <div className="layout">
      <Sidebar />
      <main className="workspace">
        <div className="env-stripe" style={{ background: activeConn ? ENV_COLORS[activeConn.environment] : "transparent" }} />
        <LicenseNotice />
        <nav className="tabbar" role="tablist">
          {tabs.map((tab) => {
            const tConnId = "connectionId" in tab ? tab.connectionId : null;
            const conn = tConnId ? byId.get(tConnId) : undefined;
            const isActive = tab.id === activeId;
            return (
              <div key={tab.id} className={isActive ? "tab active" : "tab"}>
                {conn && <EnvBadge environment={conn.environment} readOnly={conn.readOnly} />}
                <button type="button" role="tab" aria-selected={isActive} onClick={() => activate(tab.id)}>
                  {tabTitle(tab, byId)}
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
        {tabs.length === 0 && <p className="empty">{t("layout.empty")}</p>}
        {/* Неактивные вкладки скрыты, но не размонтированы — состояние форм сохраняется. */}
        {tabs.map((tab) => (
          <section key={tab.id} className="tab-content" hidden={tab.id !== activeId}>
            <TabContent tab={tab} />
          </section>
        ))}
      </main>
    </div>
  );
}
