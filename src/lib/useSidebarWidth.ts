import { useEffect, useState } from "react";

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

/** Ширина левой панели, запоминаемая на этом компьютере (как свёрнутые папки дерева — в localStorage). */
export function useSidebarWidth(): [number, (w: number) => void] {
  const [width, setWidth] = useState(readWidth);
  useEffect(() => {
    try {
      localStorage.setItem(KEY, String(width));
    } catch {
      // Хранилище недоступно — ширина живёт до перезапуска.
    }
  }, [width]);
  return [width, (w) => setWidth(clampSidebar(w))];
}
