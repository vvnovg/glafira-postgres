import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import {
  FloatingPortal,
  autoUpdate,
  flip,
  offset,
  safePolygon,
  shift,
  useDismiss,
  useFloating,
  useFocus,
  useHover,
  useInteractions,
  useRole,
} from "@floating-ui/react";
import { Icon } from "./Icon";

/** Закрывает открытую сейчас подсказку: в каждый момент открыта не больше одной. */
let closeOpenHint: (() => void) | null = null;

/**
 * Значок подсказки (`help`, спец. U1 §4) с короткой подсказкой: наведение (300 мс), фокус, клик закрепляет; Esc и клик вне закрывают. `values` —
 * параметры текста подсказки (`{{id}}`), если они есть. С `children` вместо значка — сам текст (например, «≈ 0,01 $»):
 * подчёркнут точками, фокусируется и показывает подсказку так же; `label` тогда — его полное имя для скринридера (видимый
 * текст и подсказка).
 */
export function Hint({ k, label, values, children }: { k: string; label: string; values?: Record<string, string>; children?: ReactNode }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const close = () => {
    setPinned(false);
    setOpen(false);
  };
  const closeRef = useRef(close);
  useLayoutEffect(() => {
    closeRef.current = close;
  });
  const self = useRef(() => closeRef.current());

  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: (next, _event, reason) => {
      if (!next && pinned && (reason === "hover" || reason === "focus" || reason === "safe-polygon")) return;
      if (next) setOpen(true);
      else close();
    },
    placement: "top",
    middleware: [offset(6), flip(), shift({ padding: 8 })],
    whileElementsMounted: autoUpdate,
  });
  const hover = useHover(context, { delay: { open: 300, close: 100 }, handleClose: safePolygon() });
  const focus = useFocus(context);
  const dismiss = useDismiss(context);
  const role = useRole(context, { role: "tooltip" });
  const { getReferenceProps, getFloatingProps } = useInteractions([hover, focus, dismiss, role]);

  useEffect(() => {
    if (!open) return;
    const me = self.current;
    if (closeOpenHint && closeOpenHint !== me) closeOpenHint();
    closeOpenHint = me;
    return () => {
      if (closeOpenHint === me) closeOpenHint = null;
    };
  }, [open]);

  const toggle = () => {
    if (pinned) close();
    else {
      setPinned(true);
      setOpen(true);
    }
  };
  const onClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    toggle();
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    e.preventDefault(); // Space не прокручивает панель
    e.stopPropagation(); // не активирует строку с activateProps вокруг значка
    if (!e.repeat) toggle();
  };

  // span, а не button: `<fieldset disabled>` отключает вложенные кнопки, а подсказка должна работать и там.
  return (
    <>
      <span
        role="button"
        tabIndex={0}
        className={children === undefined ? "hint-icon" : "hint-text"}
        ref={refs.setReference}
        aria-label={children === undefined ? t("hints.label", { name: label }) : label}
        {...getReferenceProps({ onClick, onKeyDown })}
      >
        {children ?? <Icon name="help" />}
      </span>
      {open && (
        <FloatingPortal>
          <div ref={refs.setFloating} className="hint-tooltip" style={floatingStyles} {...getFloatingProps({ onClick: (e: React.MouseEvent) => e.stopPropagation() })}>
            <Trans i18nKey={k} values={values} components={{ code: <code /> }} />
          </div>
        </FloatingPortal>
      )}
    </>
  );
}
