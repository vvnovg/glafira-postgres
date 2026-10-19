import { type KeyboardEvent, useEffect, useRef } from "react";

export interface FolderMenuItem {
  label: string;
  onSelect: () => void;
}

/**
 * Выпадающее меню дерева (у папки и в заголовке панели): абсолютный `role="menu"`.
 * Рендерится внутри якоря (`.tree-menu-anchor`) рядом с кнопкой-триггером (`aria-haspopup="menu"`).
 * Фокус — на первом пункте, стрелки переходят между пунктами. Esc и Tab закрывают меню и возвращают
 * фокус триггеру (Esc не уходит дальше меню), клик вне — просто закрывает.
 */
export function FolderMenu({ label, items, onClose }: { label: string; items: FolderMenuItem[]; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  // Свежий onClose без переподписки слушателя при каждом рендере родителя.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  /** Якорь (родитель меню вместе с триггером) — не «вне»: повторный клик по триггеру сам закроет меню. */
  const anchor = () => ref.current?.parentElement ?? ref.current;

  const close = (returnFocus: boolean) => {
    // Фокус возвращается до закрытия: поле с autoFocus, которое откроет пункт меню, всё равно заберёт его.
    if (returnFocus) anchor()?.querySelector<HTMLElement>("[aria-haspopup]")?.focus();
    closeRef.current();
  };

  useEffect(() => {
    ref.current?.querySelector<HTMLButtonElement>("[role=menuitem]")?.focus();
    const onDown = (e: MouseEvent) => {
      const a = ref.current?.parentElement ?? ref.current;
      if (a && !a.contains(e.target as Node)) closeRef.current();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close(true);
      return;
    }
    if (e.key === "Tab") {
      close(true);
      return;
    }
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const list = [...(ref.current?.querySelectorAll<HTMLButtonElement>("[role=menuitem]") ?? [])];
    const idx = list.indexOf(document.activeElement as HTMLButtonElement);
    const next = e.key === "ArrowDown" ? (idx + 1) % list.length : (idx - 1 + list.length) % list.length;
    list[next]?.focus();
  };

  return (
    <div
      className="tree-menu"
      role="menu"
      aria-label={label}
      ref={ref}
      draggable={false}
      // Меню лежит внутри перетаскиваемой строки папки: из него перетаскивание не начинается.
      onDragStart={(e) => {
        e.preventDefault();
        e.stopPropagation();
      }}
      onKeyDown={onKeyDown}
    >
      {items.map((item) => (
        <button
          key={item.label}
          type="button"
          role="menuitem"
          onClick={() => {
            close(true);
            item.onSelect();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}
