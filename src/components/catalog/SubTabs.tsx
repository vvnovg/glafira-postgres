import { type KeyboardEvent, type ReactNode, useId } from "react";

export interface SubTab<K extends string> {
  id: K;
  label: string;
}

/**
 * Подвкладки внутри вкладки объекта: `tablist` с переходом стрелками, Home и End; в порядке Tab — только выбранная.
 * Панель одна, её имя — выбранная подвкладка. value, которого нет среди tabs (подвкладка исчезла после обновления), заменяется первой.
 */
export function SubTabs<K extends string>({
  label,
  tabs,
  value,
  onChange,
  children,
}: {
  /** Имя списка подвкладок. */
  label: string;
  tabs: SubTab<K>[];
  value: K;
  onChange: (id: K) => void;
  children: ReactNode;
}) {
  const base = useId();
  const active = tabs.some((t) => t.id === value) ? value : tabs[0].id;

  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const i = tabs.findIndex((t) => t.id === active);
    const n = tabs.length;
    const next = e.key === "ArrowRight" ? (i + 1) % n : e.key === "ArrowLeft" ? (i - 1 + n) % n : e.key === "Home" ? 0 : e.key === "End" ? n - 1 : -1;
    if (next < 0) return;
    e.preventDefault();
    onChange(tabs[next].id);
    e.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]')[next]?.focus();
  }

  return (
    <>
      <div className="subtabs" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {tabs.map((tab) => (
          <button
            key={tab.id}
            type="button"
            role="tab"
            id={`${base}-tab-${tab.id}`}
            aria-selected={tab.id === active}
            aria-controls={`${base}-panel`}
            tabIndex={tab.id === active ? 0 : -1}
            className={tab.id === active ? "active" : undefined}
            onClick={() => onChange(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-tab-${active}`} tabIndex={0}>
        {children}
      </div>
    </>
  );
}
