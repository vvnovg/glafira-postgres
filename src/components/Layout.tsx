import { useTranslation } from "react-i18next";
import { type Tab, useWorkspace } from "../state/workspace";
import { LicenseNotice } from "./license/LicenseNotice";
import { Sidebar } from "./Sidebar";
import { i18n } from "../i18n";
import { TabContent } from "./TabContent";
import { useHelpShortcut } from "../lib/useHelpShortcut";

function tabTitle(tab: Tab): string {
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
      return tab.connectionId ? i18n.t("tabs.connection", { name: tab.connectionId }) : i18n.t("tabs.newConnection");
  }
}

export function Layout() {
  const { t } = useTranslation(); // заодно перерисовывает заголовки вкладок при смене языка
  const { tabs, activeId, activate, close } = useWorkspace();
  useHelpShortcut();

  return (
    <div className="layout">
      <Sidebar />
      <main className="workspace">
        <LicenseNotice />
        <nav className="tabbar" role="tablist">
          {tabs.map((tab) => {
            const isActive = tab.id === activeId;
            return (
              <div key={tab.id} className={isActive ? "tab active" : "tab"}>
                <button type="button" role="tab" aria-selected={isActive} onClick={() => activate(tab.id)}>
                  {tabTitle(tab)}
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
