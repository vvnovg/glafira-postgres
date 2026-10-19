import type { ReactNode } from "react";

/** Значок с текстовой альтернативой: сам символ скринридер не читает, читает подпись (она же — подсказка). */
export function Glyph({ symbol, label, className }: { symbol: ReactNode; label: string; className?: string }) {
  return (
    <span role="img" aria-label={label} title={label} className={className}>
      {symbol}
    </span>
  );
}
