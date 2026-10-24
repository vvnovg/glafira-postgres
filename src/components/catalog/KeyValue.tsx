import { Fragment, type ReactNode } from "react";

export interface KeyValueItem {
  /** Подпись; в пределах одного списка уникальна. */
  label: string;
  /** `null` и `undefined` — значения нет, показывается «—». */
  value: ReactNode;
}

/** Список «подпись — значение» (`<dl>`); пустое значение — прочерк. */
export function KeyValue({ items }: { items: KeyValueItem[] }) {
  return (
    <dl className="kv">
      {items.map((item) => (
        <Fragment key={item.label}>
          <dt>{item.label}</dt>
          <dd>{item.value ?? "—"}</dd>
        </Fragment>
      ))}
    </dl>
  );
}
