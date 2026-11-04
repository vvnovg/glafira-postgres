import { type KeyboardEvent, type PointerEvent, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { openSupport } from "../lib/links";
import { clampSidebar, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN, useSidebarWidth } from "../lib/useSidebarWidth";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";
import { LlmIndicator } from "./mcp/LlmIndicator";

/** Шаг изменения ширины стрелками, px; с Shift — крупный. */
const STEP = 16;
const BIG_STEP = 64;

/**
 * Граница левой панели: перетаскивание мышью, стрелки (Shift — крупнее), Home и End — до пределов, двойной щелчок —
 * ширина по умолчанию. Роль `separator` со значением: экранный диктор называет текущую ширину. Значение и база
 * перетаскивания и стрелок — фактическая ширина панели (`measure`): в узком окне flex сжимает её меньше запомненной.
 */
function Resizer({
  width,
  rendered,
  measure,
  onWidth,
}: {
  width: number;
  rendered: number | null;
  measure: () => number;
  onWidth: (w: number, opts?: { persist?: boolean }) => void;
}) {
  const { t } = useTranslation();
  /** Начало перетаскивания: позиция указателя, ширина в этот момент и последняя заданная; `null` — не тащат. */
  const drag = useRef<{ x: number; w: number; last: number } | null>(null);

  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.shiftKey ? BIG_STEP : STEP;
    const base = measure();
    const next =
      e.key === "ArrowRight" ? base + step : e.key === "ArrowLeft" ? base - step : e.key === "Home" ? SIDEBAR_MIN : e.key === "End" ? SIDEBAR_MAX : null;
    if (next === null) return;
    e.preventDefault();
    onWidth(next);
  };

  const endDrag = () => {
    const d = drag.current;
    drag.current = null;
    if (d && d.last !== d.w) onWidth(d.last);
  };

  return (
    <div
      className="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label={t("sidebar.resize")}
      aria-valuenow={rendered ?? width}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      tabIndex={0}
      title={t("sidebar.resizeHint")}
      onKeyDown={onKeyDown}
      onDoubleClick={() => onWidth(SIDEBAR_DEFAULT)}
      onPointerDown={(e: PointerEvent<HTMLDivElement>) => {
        if (e.button !== 0) return;
        e.preventDefault();
        const w = measure();
        drag.current = { x: e.clientX, w, last: w };
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d) return;
        d.last = clampSidebar(d.w + e.clientX - d.x);
        onWidth(d.last, { persist: false });
      }}
      onPointerUp={(e) => {
        endDrag();
        e.currentTarget.releasePointerCapture?.(e.pointerId);
      }}
      onPointerCancel={endDrag}
    />
  );
}

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const [width, setWidth] = useSidebarWidth();
  const asideRef = useRef<HTMLElement>(null);
  /** Фактическая ширина панели (ResizeObserver); `null` — не измерена. */
  const [rendered, setRendered] = useState<number | null>(null);
  const measure = () => {
    const w = asideRef.current?.getBoundingClientRect().width ?? 0;
    return w > 0 ? Math.round(w) : width;
  };

  useEffect(() => {
    const el = asideRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => {
      const w = el.getBoundingClientRect().width;
      setRendered(w > 0 ? Math.round(w) : null);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  return (
    <>
      <aside className="sidebar" ref={asideRef} style={{ width }}>
        <ConnectionTree />
        <div className="sidebar-footer">
          <LlmIndicator />
          <button type="button" className="link" onClick={() => open({ kind: "settings" })}>
            {t("sidebar.settings")}
          </button>
          <button type="button" className="link" onClick={openSupport}>
            {t("support.title")}
          </button>
          <button type="button" className="link" onClick={() => open({ kind: "help" })}>
            {t("sidebar.help")}
          </button>
          <button type="button" className="link" onClick={() => open({ kind: "about" })}>
            {t("sidebar.about")}
          </button>
        </div>
      </aside>
      <Resizer width={width} rendered={rendered} measure={measure} onWidth={setWidth} />
    </>
  );
}
