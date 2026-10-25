import { formatBytes } from "../../lib/format";

/** Размер в байтах; `null` — «—» (значение недоступно). */
export const bytesText = (n: number | null): string => (n === null ? "—" : formatBytes(n));

/** Непустые части через « · » (имена из каталога — пользователь, приложение, база: фразой они не являются). */
export const metaText = (parts: (string | null | undefined)[]): string => parts.filter((p): p is string => Boolean(p)).join(" · ");

/**
 * Устойчивые ключи строк по их содержимому, а не по месту в массиве. Счётчик добавляется только настоящим дубликатам
 * (например, две реплики с одинаковым `application_name`): остальные ключи не зависят от порядка строк.
 */
export function uniqueKeys(bases: string[]): string[] {
  const seen = new Map<string, number>();
  return bases.map((base) => {
    const n = seen.get(base) ?? 0;
    seen.set(base, n + 1);
    return n === 0 ? base : `${base}#${n}`;
  });
}
