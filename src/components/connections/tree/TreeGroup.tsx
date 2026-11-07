import { type ReactNode, useId, useRef, useState } from "react";
import type { IconName } from "../../common/Icon";
import { FolderMenu, type FolderMenuItem } from "./FolderMenu";
import { TreeRow } from "./TreeRow";

/**
 * Ленивый узел дерева на `TreeRow` (спец. переноса U1 §3.1): содержимое строится только в раскрытом виде. `label` — имя
 * кнопки раскрытия для экранного диктора («Таблицы (12)»), `name` — видимое имя, если оно короче (число — в `extra`).
 * `menu` — контекстное меню строки (правая кнопка, Shift+F10 или клавиша меню на кнопке-заголовке); `menuLabel` — его имя.
 * `defaultOpen` — узел раскрыт сразу (корень ветки в режиме фокуса). `holdsActive` — внутри ветки лежит узел активной
 * вкладки: пока ветка свёрнута, подсвечена она сама.
 */
export function TreeGroup({
  label,
  name,
  icon,
  extra,
  menu,
  menuLabel,
  defaultOpen = false,
  holdsActive = false,
  children,
}: {
  label: string;
  name?: string;
  icon?: IconName;
  extra?: ReactNode;
  menu?: FolderMenuItem[];
  menuLabel?: string;
  defaultOpen?: boolean;
  holdsActive?: boolean;
  children: () => ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const [menuOpen, setMenuOpen] = useState(false);
  const toggleId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const hasMenu = menu !== undefined && menu.length > 0;
  const shown = name ?? label;
  return (
    <li>
      <TreeRow
        chevron={{ expanded: open, onToggle: () => setOpen(!open), label, id: toggleId, buttonRef: toggleRef }}
        icon={icon}
        name={shown}
        title={shown}
        extra={extra}
        bold
        current={holdsActive && !open}
        rowProps={
          hasMenu
            ? {
                onContextMenu: (e) => {
                  e.preventDefault();
                  setMenuOpen(true);
                },
                onKeyDown: (e) => {
                  if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
                    e.preventDefault();
                    setMenuOpen(true);
                  }
                },
              }
            : undefined
        }
      >
        {hasMenu && menuOpen && (
          <span className="tree-menu-anchor">
            <FolderMenu label={menuLabel ?? label} items={menu} onClose={() => setMenuOpen(false)} trigger={() => toggleRef.current} />
          </span>
        )}
      </TreeRow>
      {/* Список назван по кнопке-заголовку: вложенные «Таблицы» разных схем различимы. */}
      {open && <ul aria-labelledby={toggleId}>{children()}</ul>}
    </li>
  );
}
