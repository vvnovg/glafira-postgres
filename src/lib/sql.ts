/** Строковая константа SQL: кавычки удваиваются (при `standard_conforming_strings = on`, как в `quote_literal` Rust). */
export function quoteLiteral(s: string): string {
  return `'${s.replace(/'/g, "''")}'`;
}
