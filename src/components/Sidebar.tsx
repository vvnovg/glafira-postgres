import { useTranslation } from "react-i18next";
import { openSupport } from "../lib/links";
import { isMacOS } from "../lib/platform";
import { shortcutActions } from "../lib/shortcuts";
import { useResizable } from "../lib/useResizable";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";
import { TreeRow } from "./connections/tree/TreeRow";
import { LlmIndicator } from "./mcp/LlmIndicator";

/** Ширина панели запоминается на этом компьютере (как свёрнутые папки дерева) — ключ тот же, что до U1. */
const WIDTH_KEY = "glafira.sidebar.width";

/**
 * Боковая панель (спец. U1 Kafka-версии §6–7, перенос — спец. 2026-11-07 §1): дерево подключений, нижние пункты и разделитель
 * с изменяемой шириной 200–400 px (стрелки по 16 px, Home и End — пределы, Enter и двойной щелчок — 248).
 */
export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const activeKind = useWorkspace((s) => s.tabs.find((x) => x.id === s.activeId)?.kind ?? null);
  const { width, dragging, splitterProps } = useResizable(WIDTH_KEY, { min: 200, max: 400, initial: 248 });

  return (
    <aside className="sidebar" style={{ width }}>
      {/* macOS: место светофора; за эту полосу окно перетаскивается. */}
      {isMacOS() && <div className="sidebar-top" data-tauri-drag-region="" />}
      <div className="sidebar-scroll">
        <ConnectionTree />
      </div>
      <div className="sidebar-footer">
        <LlmIndicator />
        {/* «Настройки» при открытой вкладке настроек только переключаются на неё: выбранный там раздел остаётся. */}
        <TreeRow chevron="none" icon="settings" name={t("sidebar.settings")} current={activeKind === "settings"} onActivate={shortcutActions.settings} />
        <TreeRow chevron="none" icon="heart" name={t("support.title")} onActivate={openSupport} />
        <TreeRow chevron="none" icon="book" name={t("sidebar.help")} current={activeKind === "help"} onActivate={() => open({ kind: "help" })} />
        <TreeRow chevron="none" icon="info" name={t("sidebar.about")} current={activeKind === "about"} onActivate={() => open({ kind: "about" })} />
      </div>
      <div className={dragging ? "sidebar-splitter active" : "sidebar-splitter"} aria-label={t("sidebar.resize")} title={t("sidebar.resizeHint")} {...splitterProps} />
    </aside>
  );
}
