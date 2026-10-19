import { Component, lazy, type ReactNode, Suspense } from "react";
import { useTranslation } from "react-i18next";
import type { Tab } from "../state/workspace";
import { AboutPanel } from "./about/AboutPanel";
import { ConnectionFormTab } from "./connections/ConnectionFormTab";
import { Overview } from "./overview/Overview";
import { SettingsPanel } from "./settings/SettingsPanel";
import { LicensePanel } from "./license/LicensePanel";

// Справка (react-markdown и главы) грузится отдельным чанком при первом открытии.
const HelpPanel = lazy(() => import("./help/HelpPanel").then((m) => ({ default: m.HelpPanel })));

/** Сбой чанка или рендера справки показывает «Не удалось загрузить руководство», не роняя окно. */
class HelpBoundary extends Component<{ fallback: ReactNode; children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

function LazyHelp({ tab }: { tab: Extract<Tab, { kind: "help" }> }) {
  const { t } = useTranslation();
  return (
    <HelpBoundary fallback={<p className="empty">{t("help.loadFailed")}</p>}>
      <Suspense fallback={<p className="empty">{t("common.loading")}</p>}>
        <HelpPanel spec={tab} />
      </Suspense>
    </HelpBoundary>
  );
}

/** active — вкладка сейчас видна (неактивные остаются смонтированными, но скрыты). */
export function TabContent({ tab, active = true }: { tab: Tab; active?: boolean }) {
  switch (tab.kind) {
    case "about":
      return <AboutPanel />;
    case "help":
      return <LazyHelp tab={tab} />;
    case "settings":
      return <SettingsPanel />;
    case "license":
      return <LicensePanel />;
    case "connection-form":
      return <ConnectionFormTab connectionId={tab.connectionId} folderId={tab.folderId ?? null} tabId={tab.id} />;
    case "overview":
      return <Overview connectionId={tab.connectionId} member={tab.member} active={active} />;
  }
}
