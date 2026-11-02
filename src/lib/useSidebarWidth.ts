import { useState } from "react";

/** Ширина левой панели: по умолчанию и пределы, px. */
export const SIDEBAR_DEFAULT = 260;
export const SIDEBAR_MIN = 180;
export const SIDEBAR_MAX = 640;
const KEY = "glafira.sidebar.width";

export const clampSidebar = (w: number) => Math.round(Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, w)));

function readWidth(): number {
  try {
    const raw = localStorage.getItem(KEY);
    const n = raw === null ? Number.NaN : Number(raw);
    return Number.isFinite(n) ? clampSidebar(n) : SIDEBAR_DEFAULT;
  } catch {
    return SIDEBAR_DEFAULT;
  }
}

function saveWidth(w: number) {
  try {
    localStorage.setItem(KEY, String(w));
  } catch {
    // Хранилище недоступно — ширина живёт до перезапуска.
  }
}

/**
 * Ширина левой панели, запоминаемая на этом компьютере (как свёрнутые папки дерева — в localStorage). `persist: false` —
 * промежуточное значение (перетаскивание): запись в хранилище одна, по отпусканию.
 */
export function useSidebarWidth(): [number, (w: number, opts?: { persist?: boolean }) => void] {
  const [width, setWidth] = useState(readWidth);
  return [
    width,
    (w, opts) => {
      const next = clampSidebar(w);
      setWidth(next);
      if (opts?.persist !== false) saveWidth(next);
    },
  ];
}
