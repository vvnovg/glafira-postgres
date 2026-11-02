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
