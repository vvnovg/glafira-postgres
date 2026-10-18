import type { KeyboardEvent } from "react";

/** Элементы, на которые попадает фокус по Tab (отключённые пропускаются). */
export const FOCUSABLE =
  "button:not([disabled]), select:not([disabled]), input:not([disabled]), textarea:not([disabled]), summary, a[href], [tabindex]:not([tabindex='-1'])";

/**
 * Tab и Shift+Tab ходят по кругу внутри модального диалога `root`: страница за ним недоступна, пока он открыт.
 * Сам контейнер (tabIndex=-1, фокус при открытии) считается «до первого / после последнего» элемента.
 */
export function trapTab(root: HTMLElement | null, e: KeyboardEvent): void {
  const items = [...(root?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
  if (items.length === 0) return;
  const first = items[0];
  const last = items[items.length - 1];
  const active = document.activeElement;
  const outside = !root?.contains(active) || active === root;
  if (e.shiftKey && (active === first || outside)) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (active === last || outside)) {
    e.preventDefault();
    first.focus();
  }
}
