import { useEffect } from "react";
import { Menu, MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { i18n } from "../i18n";
import { useDiagnosticsRequest } from "../state/diagnosticsRequest";
import { useWorkspace } from "../state/workspace";

/** id пустого подменю Help в меню Tauri по умолчанию (`tauri::menu::HELP_SUBMENU_ID`). */
const TAURI_HELP_ID = "__tauri_help_menu__";

/** Меню Help строится только на macOS: на Windows и Linux у окна нет строки меню. */
export function isMacOS(): boolean {
  return typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");
}

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

/**
 * Меню приложения: стандартные подменю Tauri + Help с пунктами Glafira (подписи — из словаря текущего языка).
 * Подменю не регистрируется как меню справки NSApp: иначе macOS добавляет в него системный поиск,
 * который ищет по пунктам меню и справке macOS, а не по нашему руководству.
 */
async function buildAppMenu(): Promise<BuiltMenu> {
  const t = i18n.t.bind(i18n);
  const { open } = useWorkspace.getState();
  const g = ++generation;
  const id = (base: string) => `${base}-${g}`;
  const menu = await Menu.default();
  const stock = await menu.get(TAURI_HELP_ID);
  if (stock) {
    await menu.remove(stock);
    await stock.close();
  }
  const items = [
    // ⌘? — на раскладке US это ⇧⌘/; символ «?» как клавишу Tauri не принимает.
    await MenuItem.new({ id: id("guide"), text: t("menu.guide"), accelerator: "CmdOrCtrl+Shift+/", action: () => open({ kind: "help" }) }),
    await PredefinedMenuItem.new({ item: "Separator" }),
    await MenuItem.new({ id: id("about"), text: t("menu.about"), action: () => open({ kind: "about" }) }),
    await MenuItem.new({
      id: id("diagnostics"),
      text: t("menu.diagnostics"),
      action: () => {
        useDiagnosticsRequest.getState().request();
        open({ kind: "about" });
      },
    }),
  ];
  const help = await Submenu.new({ id: id("glafira-help"), text: t("menu.help"), items });
  await menu.append(help);
  // setAsAppMenu возвращает новый хэндл прежнего меню — его закрываем сразу.
  const previous = await menu.setAsAppMenu();
  await previous?.close();
  return { menu, parts: [help, ...items] };
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

/** Меню Help на macOS; перестраивается при смене языка интерфейса. */
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
