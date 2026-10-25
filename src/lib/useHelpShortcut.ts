import { useEffect } from "react";
import { helpAnchorFor, helpChapterFor } from "../components/help/helpChapterFor";
import { useHelpContext } from "../state/helpContext";
import { useWorkspace } from "../state/workspace";

/** F1 в любом месте окна — вкладка «Справка» с главой текущего экрана. */
export function useHelpShortcut(): void {
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "F1") return;
      e.preventDefault();
      const { tabs, activeId, open, activate } = useWorkspace.getState();
      const active = tabs.find((t) => t.id === activeId) ?? null;
      // На самой справке F1 не сбрасывает открытую главу.
      if (active?.kind === "help") return activate(active.id);
      // Экран может уточнить главу через useHelpChapter; тогда якорь инструмента к ней не относится.
      const override = active ? useHelpContext.getState().byTab[active.id] : undefined;
      open({ kind: "help", chapter: override ?? helpChapterFor(active), anchor: override ? undefined : helpAnchorFor(active) });
    };
    // capture — раньше обработчиков полей и компонентов, которые могут остановить всплытие.
    window.addEventListener("keydown", onKeyDown, { capture: true });
    return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
  }, []);
}
