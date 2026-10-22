import { useTranslation } from "react-i18next";
import { openSupport } from "../lib/links";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);

  return (
    <aside className="sidebar">
      <ConnectionTree />
      <div className="sidebar-footer">
        <button type="button" className="link" onClick={() => open({ kind: "settings" })}>
          {t("sidebar.settings")}
        </button>
        <button type="button" className="link" onClick={openSupport}>
          {t("support.title")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "help" })}>
          {t("sidebar.help")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "about" })}>
          {t("sidebar.about")}
        </button>
      </div>
    </aside>
  );
}
