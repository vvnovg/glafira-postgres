/** «<имя> <версия>» MCP-клиента или подпись «Неизвестный MCP-клиент» (unknown — переведённая строка). */
export function clientLabel(name: string | null, version: string | null, unknown: string): string {
  return name ? `${name} ${version ?? ""}`.trim() : unknown;
}
