import { useTranslation } from "react-i18next";
import { useWorkspace } from "../state/workspace";
import { useLicense } from "../lib/useLicense";
import { ConnectionTree } from "./connections/tree/ConnectionTree";

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const { status } = useLicense();

  return (
    <aside className="sidebar">
      <ConnectionTree />
      <div className="sidebar-footer">
        <button type="button" className="link" onClick={() => open({ kind: "settings" })}>
          {t("sidebar.settings")}
        </button>
        <button type="button" className="link" onClick={() => open({ kind: "license" })}>
          {t("sidebar.license", { tier: status ? (status.tier === "pro" ? "Pro" : "Free") : "…" })}
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
