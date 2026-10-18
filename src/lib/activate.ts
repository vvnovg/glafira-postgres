import type { KeyboardEvent } from "react";

/** Делает некнопочный элемент (строку таблицы/списка) активируемым мышью и с клавиатуры. */
export function activateProps(onActivate: () => void) {
  return {
    role: "button" as const,
    tabIndex: 0,
    onClick: onActivate,
    onKeyDown: (e: KeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        onActivate();
      }
    },
  };
}
