/** Горизонтальные границы элемента (как у `getBoundingClientRect`). */
export interface Span {
  left: number;
  right: number;
}

/** Допуск на дробные пиксели: вкладка, выступающая за край меньше чем на пиксель, считается видимой. */
const EPS = 1;

/**
 * Вкладки, которые не видны в строке вкладок целиком (прокручены за левый или правый край): они попадают в меню
 * скрытых. Строка без ширины (ещё не разложена или скрыта) скрытых не даёт.
 */
export function hiddenTabIds(strip: Span, tabs: (Span & { id: string })[]): string[] {
  if (strip.right - strip.left <= 0) return [];
  return tabs.filter((t) => t.left < strip.left - EPS || t.right > strip.right + EPS).map((t) => t.id);
}

/** Отступ от края строки, с которым показывается вкладка, прокрученная в видимую часть, px. */
export const REVEAL_PAD = 8;

/**
 * Новое смещение прокрутки строки вкладок, при котором вкладка видна целиком (с отступом от края). Считается вручную, а не
 * `scrollIntoView`: в WebKit тот для вкладки во вложенной горизонтальной прокрутке иногда ничего не делает. Видная
 * вкладка смещение не меняет; вкладка шире строки выравнивается по левому краю. Строка без ширины — без изменений.
 */
export function revealScrollLeft(strip: Span, tab: Span, scrollLeft: number): number {
  if (strip.right - strip.left <= 0) return scrollLeft;
  if (tab.left < strip.left - EPS || tab.right - tab.left > strip.right - strip.left) {
    return Math.max(0, scrollLeft - (strip.left - tab.left) - REVEAL_PAD);
  }
  if (tab.right > strip.right + EPS) return scrollLeft + (tab.right - strip.right) + REVEAL_PAD;
  return scrollLeft;
}
