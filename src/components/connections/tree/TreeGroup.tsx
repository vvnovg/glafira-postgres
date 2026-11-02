import { type ReactNode, useId, useRef, useState } from "react";
import { FolderMenu, type FolderMenuItem } from "./FolderMenu";

/**
 * Ленивый узел дерева: содержимое строится только в раскрытом виде. `menu` — контекстное меню строки (правая кнопка,
 * Shift+F10 или клавиша меню на кнопке-заголовке); `menuLabel` — его имя. `defaultOpen` — узел раскрыт сразу (корень
 * ветки в режиме фокуса).
 */
export function TreeGroup({
  label,
  extra,
  menu,
  menuLabel,
  defaultOpen = false,
  children,
}: {
  label: string;
  extra?: ReactNode;
  menu?: FolderMenuItem[];
  menuLabel?: string;
  defaultOpen?: boolean;
  children: () => ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [menuOpen, setMenuOpen] = useState(false);
  const toggleId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const hasMenu = menu !== undefined && menu.length > 0;
  return (
    <li>
      <div
        className="tree-conn tree-wrap"
        onContextMenu={
          hasMenu
            ? (e) => {
                e.preventDefault();
                setMenuOpen(true);
              }
            : undefined
        }
        onKeyDown={
          hasMenu
            ? (e) => {
                if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
                  e.preventDefault();
                  setMenuOpen(true);
                }
              }
            : undefined
        }
      >
        <button type="button" id={toggleId} ref={toggleRef} className="tree-folder-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="tree-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
          <span className="tree-name" title={label}>
            {label}
          </span>
        </button>
        {extra && <span className="tree-extra">{extra}</span>}
        {hasMenu && menuOpen && (
          <span className="tree-menu-anchor">
            <FolderMenu label={menuLabel ?? label} items={menu} onClose={() => setMenuOpen(false)} trigger={() => toggleRef.current} />
          </span>
        )}
      </div>
      {/* Список назван по кнопке-заголовку: вложенные «Таблицы» разных схем различимы. */}
      {open && <ul aria-labelledby={toggleId}>{children()}</ul>}
    </li>
  );
}
