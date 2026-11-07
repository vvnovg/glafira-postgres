import { type KeyboardEvent, type MouseEvent as ReactMouseEvent, useCallback, useEffect, useRef, useState } from "react";

interface Options {
  min: number;
  max: number;
  initial: number;
  /** Шаг стрелок на разделителе с клавиатуры. */
  step?: number;
}

function clamp(v: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, v));
}

function read(key: string, { min, max, initial }: Options): number {
  try {
    const raw = localStorage.getItem(key);
    const n = raw === null ? NaN : Number(raw);
    return Number.isFinite(n) ? clamp(n, min, max) : initial;
  } catch {
    return initial;
  }
}

function write(key: string, value: number) {
  try {
    localStorage.setItem(key, String(value));
  } catch {
    // Хранилище недоступно — ширина живёт до перезапуска.
  }
}

/**
 * Изменяемая ширина панели (спец. U1 §7): перетаскивание разделителя мышью, стрелки с клавиатуры, двойной клик
 * возвращает значение по умолчанию. Ширина хранится в `localStorage` под `key`.
 */
export function useResizable(key: string, options: Options) {
  const { min, max, initial, step = 16 } = options;
  const [width, setWidth] = useState(() => read(key, options));
  const [dragging, setDragging] = useState(false);
  const start = useRef<{ x: number; width: number } | null>(null);

  useEffect(() => write(key, width), [key, width]);

  const onMouseDown = useCallback(
    (e: ReactMouseEvent) => {
      if (e.button !== 0) return;
      e.preventDefault();
      start.current = { x: e.clientX, width };
      setDragging(true);
    },
    [width],
  );

  useEffect(() => {
    if (!dragging) return;
    const onMove = (e: MouseEvent) => {
      const s = start.current;
      if (s) setWidth(clamp(s.width + e.clientX - s.x, min, max));
    };
    const onUp = () => {
      start.current = null;
      setDragging(false);
    };
    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    return () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, [dragging, min, max]);

  const reset = useCallback(() => setWidth(initial), [initial]);

  const onKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (e.key === "ArrowLeft") setWidth((w) => clamp(w - step, min, max));
      else if (e.key === "ArrowRight") setWidth((w) => clamp(w + step, min, max));
      else if (e.key === "Home") setWidth(min);
      else if (e.key === "End") setWidth(max);
      else if (e.key === "Enter") reset();
      else return;
      e.preventDefault();
    },
    [min, max, step, reset],
  );

  return {
    width,
    dragging,
    splitterProps: {
      role: "separator" as const,
      "aria-orientation": "vertical" as const,
      "aria-valuenow": width,
      "aria-valuemin": min,
      "aria-valuemax": max,
      tabIndex: 0,
      onMouseDown,
      onDoubleClick: reset,
      onKeyDown,
    },
  };
}
