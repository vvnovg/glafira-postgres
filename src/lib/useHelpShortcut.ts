import { useEffect } from "react";
import { helpAnchorFor, helpChapterFor } from "../components/help/helpChapterFor";
import { useHelpContext } from "../state/helpContext";
import { useWorkspace } from "../state/workspace";

/**
 * Справка по текущему экрану: вкладка «Справка» с главой (и якорем инструмента диагностики) активной вкладки.
 * Общая для F1 и пункта меню «Руководство», чтобы оба открывали одно и то же.
 */
export function openContextHelp(): void {
  const { tabs, activeId, open, activate } = useWorkspace.getState();
  const active = tabs.find((t) => t.id === activeId) ?? null;
  // На самой справке вызов не сбрасывает открытую главу.
  if (active?.kind === "help") return activate(active.id);
  // Экран может уточнить главу через useHelpChapter.
  const override = active ? useHelpContext.getState().byTab[active.id] : undefined;
  open({ kind: "help", chapter: override ?? helpChapterFor(active), anchor: override ? undefined : helpAnchorFor(active) });
}

/** F1 в любом месте окна — вкладка «Справка» с главой текущего экрана. */
export function useHelpShortcut(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "F1") return;
      e.preventDefault();
      openContextHelp();
    };
    // capture — раньше обработчиков полей и компонентов, которые могут остановить всплытие.
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, []);
}
