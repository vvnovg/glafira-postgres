import { type ReactNode, useId, useState } from "react";

/** Ленивый узел дерева: содержимое строится только в раскрытом виде. */
export function TreeGroup({ label, extra, children }: { label: string; extra?: ReactNode; children: () => ReactNode }) {
  const [open, setOpen] = useState(false);
  const toggleId = useId();
  return (
    <li>
      <div className="tree-conn tree-wrap">
        <button type="button" id={toggleId} className="tree-folder-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
          <span className="tree-caret" aria-hidden="true">
            {open ? "▾" : "▸"}
          </span>
          <span className="tree-name">{label}</span>
        </button>
        {extra}
      </div>
      {/* Список назван по кнопке-заголовку: вложенные «Таблицы» разных схем различимы. */}
      {open && <ul aria-labelledby={toggleId}>{children()}</ul>}
    </li>
  );
}
