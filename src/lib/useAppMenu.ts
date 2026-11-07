import { useEffect } from "react";
import { Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { i18n } from "../i18n";
import { openSupport } from "./links";
import { isMacOS } from "./platform";
import { shortcutActions } from "./shortcuts";
import { useDiagnosticsRequest } from "../state/diagnosticsRequest";
import { useWorkspace } from "../state/workspace";
import { openContextHelp } from "./useHelpShortcut";

export { isMacOS } from "./platform";

/**
 * Номер сборки меню — суффикс id пунктов. Tauri хранит обработчики кликов по id пункта и снимает их,
 * когда пункт уничтожается: с одинаковыми id закрытие прежнего меню отключило бы клики нового.
 */
let generation = 0;

/** Собранное меню и хэндлы его пунктов: их закрываем, когда меню заменено или хук размонтирован. */
interface BuiltMenu {
  menu: Menu;
  parts: { close(): Promise<void> }[];
}

type Predefined = NonNullable<Parameters<typeof PredefinedMenuItem.new>[0]>["item"];

/**
 * Меню приложения на macOS (спец. U1 §8) собирается целиком, а не из `Menu.default()`: у стандартных «File»
 * и «Window» пункт «Закрыть окно» занимает ⌘W, которое здесь закрывает вкладку. Подписи — из словаря текущего
 * языка. Help с пунктами Glafira не регистрируется как меню справки NSApp: иначе macOS добавляет в него
 * системный поиск, который ищет по пунктам меню и справке macOS, а не по нашему руководству.
 */
async function buildAppMenu(): Promise<BuiltMenu> {
  const t = i18n.t.bind(i18n);
  const { open } = useWorkspace.getState();
  const g = ++generation;
  const id = (base: string) => `${base}-${g}`;
  const parts: { close(): Promise<void> }[] = [];
  const pre = async (item: Predefined) => {
    const m = await PredefinedMenuItem.new({ item });
    parts.push(m);
    return m;
  };
  const sep = () => pre("Separator");
  const item = async (base: string, text: string, action: () => void, accelerator?: string) => {
    const m = await MenuItem.new({ id: id(base), text, action, accelerator });
    parts.push(m);
    return m;
  };
  const sub = async (base: string, text: string, items: (MenuItem | PredefinedMenuItem | Submenu)[]) => {
    const s = await Submenu.new({ id: id(base), text, items });
    parts.push(s);
    return s;
  };

  const app = await sub("app", "Glafira", [
    await pre({ About: null }),
    await sep(),
    await item("settings", t("menu.settings"), shortcutActions.settings, "CmdOrCtrl+Comma"),
    await sep(),
    await pre("Services"),
    await sep(),
    await pre("Hide"),
    await pre("HideOthers"),
    await pre("ShowAll"),
    await sep(),
    await pre("Quit"),
  ]);
  const file = await sub("file", t("menu.file"), [
    await item("new-connection", t("menu.newConnection"), shortcutActions.newConnection, "CmdOrCtrl+N"),
    await sep(),
    await item("close-tab", t("menu.closeTab"), shortcutActions.closeTab, "CmdOrCtrl+W"),
    await item("close-window", t("menu.closeWindow"), shortcutActions.closeWindow, "CmdOrCtrl+Shift+W"),
  ]);
  const edit = await sub("edit", t("menu.edit"), [
    await pre("Undo"),
    await pre("Redo"),
    await sep(),
    await pre("Cut"),
    await pre("Copy"),
    await pre("Paste"),
    await pre("SelectAll"),
  ]);
  const view = await sub("view", t("menu.view"), [
    await item("refresh", t("menu.refresh"), shortcutActions.refresh, "CmdOrCtrl+R"),
    // ⌘⏎ — «План» консоли EXPLAIN и карточки запроса (PostgreSQL-версия, спец. переноса U1 §6).
    await item("run", t("menu.run"), shortcutActions.run, "CmdOrCtrl+Enter"),
    await item("find", t("menu.find"), shortcutActions.find, "CmdOrCtrl+F"),
    await item("filter-connections", t("menu.filterConnections"), shortcutActions.filterConnections, "CmdOrCtrl+Shift+F"),
    await sep(),
    await pre("Fullscreen"),
  ]);
  const window = await sub("window", t("menu.window"), [
    await pre("Minimize"),
    await pre("Maximize"),
    await sep(),
    await item("next-tab", t("menu.nextTab"), shortcutActions.nextTab, "Ctrl+Tab"),
    await item("prev-tab", t("menu.prevTab"), shortcutActions.prevTab, "Ctrl+Shift+Tab"),
  ]);
  const help = await sub("glafira-help", t("menu.help"), [
    // ⌘? — на раскладке US это ⇧⌘/; символ «?» как клавишу Tauri не принимает. Как F1: глава и раздел текущего экрана.
    await item("guide", t("menu.guide"), openContextHelp, "CmdOrCtrl+Shift+/"),
    await sep(),
    await item("support", t("support.title"), openSupport),
    await item("about", t("menu.about"), () => open({ kind: "about" })),
    await item("diagnostics", t("menu.diagnostics"), () => {
      useDiagnosticsRequest.getState().request();
      open({ kind: "about" });
    }),
  ]);

  const menu = await Menu.new({ id: id("app-menu"), items: [app, file, edit, view, window, help] });
  // Список окон macOS дописывает в это подменю сам.
  await window.setAsWindowsMenuForNSApp();
  // setAsAppMenu возвращает новый хэндл прежнего меню — его закрываем сразу.
  const previous = await menu.setAsAppMenu();
  await previous?.close();
  return { menu, parts };
}

/** Закрывает хэндлы меню и его пунктов. Только для заменённого меню: у пунктов действующего Tauri снимет обработчики. */
async function release(built: BuiltMenu | null): Promise<void> {
  if (!built) return;
  for (const r of [...built.parts, built.menu]) {
    try {
      await r.close();
    } catch (e) {
      console.warn("app menu close", e);
    }
  }
}

/** Меню приложения на macOS; перестраивается при смене языка интерфейса. */
export function useAppMenu(): void {
  useEffect(() => {
    if (!isMacOS()) return;
    let disposed = false;
    let current: BuiltMenu | null = null;
    // Сборки идут по очереди: при быстрой смене языка последним установится меню последнего языка.
    let queue: Promise<void> = Promise.resolve();
    const rebuild = () => {
      queue = queue.then(async () => {
        if (disposed) return;
        try {
          const next = await buildAppMenu();
          // Хук размонтирован, пока меню собиралось: новое освобождается сразу, прежнее — ниже, в очистке.
          if (disposed) return release(next);
          const previous = current;
          current = next;
          await release(previous);
        } catch (e) {
          console.warn("app menu", e);
        }
      });
    };
    rebuild();
    i18n.on("languageChanged", rebuild);
    return () => {
      disposed = true;
      i18n.off("languageChanged", rebuild);
      // После сборки, которая уже идёт: освобождаем последнее меню и его пункты.
      queue = queue.then(async () => {
        const last = current;
        current = null;
        await release(last);
      });
    };
  }, []);
}
