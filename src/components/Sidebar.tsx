import { type KeyboardEvent, type PointerEvent, useRef } from "react";
import { useTranslation } from "react-i18next";
import { openSupport } from "../lib/links";
import { SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN, useSidebarWidth } from "../lib/useSidebarWidth";
import { useWorkspace } from "../state/workspace";
import { ConnectionTree } from "./connections/tree/ConnectionTree";

/** Шаг изменения ширины стрелками, px; с Shift — крупный. */
const STEP = 16;
const BIG_STEP = 64;

/**
 * Граница левой панели: перетаскивание мышью, стрелки (Shift — крупнее), Home и End — до пределов, двойной щелчок —
 * ширина по умолчанию. Роль `separator` со значением: экранный диктор называет текущую ширину.
 */
function Resizer({ width, onWidth }: { width: number; onWidth: (w: number) => void }) {
  const { t } = useTranslation();
  /** Начало перетаскивания: позиция указателя и ширина в этот момент; `null` — не тащат. */
  const drag = useRef<{ x: number; w: number } | null>(null);

  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.shiftKey ? BIG_STEP : STEP;
    const next =
      e.key === "ArrowRight" ? width + step : e.key === "ArrowLeft" ? width - step : e.key === "Home" ? SIDEBAR_MIN : e.key === "End" ? SIDEBAR_MAX : null;
    if (next === null) return;
    e.preventDefault();
    onWidth(next);
  };

  return (
    <div
      className="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label={t("sidebar.resize")}
      aria-valuenow={width}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={SIDEBAR_MAX}
      tabIndex={0}
      title={t("sidebar.resizeHint")}
      onKeyDown={onKeyDown}
      onDoubleClick={() => onWidth(SIDEBAR_DEFAULT)}
      onPointerDown={(e: PointerEvent<HTMLDivElement>) => {
        if (e.button !== 0) return;
        e.preventDefault();
        drag.current = { x: e.clientX, w: width };
        e.currentTarget.setPointerCapture?.(e.pointerId);
      }}
      onPointerMove={(e) => {
        if (drag.current) onWidth(drag.current.w + e.clientX - drag.current.x);
      }}
      onPointerUp={(e) => {
        drag.current = null;
        e.currentTarget.releasePointerCapture?.(e.pointerId);
      }}
      onPointerCancel={() => {
        drag.current = null;
      }}
    />
  );
}

export function Sidebar() {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const [width, setWidth] = useSidebarWidth();

  return (
    <>
      <aside className="sidebar" style={{ width }}>
        <ConnectionTree />
        <div className="sidebar-footer">
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
      <Resizer width={width} onWidth={setWidth} />
    </>
  );
}
