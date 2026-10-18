import { useEffect, useLayoutEffect, useRef, useState } from "react";
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

/** Закрывает открытую сейчас подсказку: в каждый момент открыта не больше одной. */
let closeOpenHint: (() => void) | null = null;

/** Значок «?» с короткой подсказкой: наведение (300 мс), фокус, клик закрепляет; Esc и клик вне закрывают. */
export function Hint({ k, label }: { k: string; label: string }) {
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
        className="hint-icon"
        ref={refs.setReference}
        aria-label={t("hints.label", { name: label })}
        {...getReferenceProps({ onClick, onKeyDown })}
      >
        ?
      </span>
      {open && (
        <FloatingPortal>
          <div ref={refs.setFloating} className="hint-tooltip" style={floatingStyles} {...getFloatingProps({ onClick: (e: React.MouseEvent) => e.stopPropagation() })}>
            <Trans i18nKey={k} components={{ code: <code /> }} />
          </div>
        </FloatingPortal>
      )}
    </>
  );
}
