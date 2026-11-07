/** macOS: заголовок окна слит с панелями, меню приложения и ⌘ вместо Ctrl. */
export function isMacOS(): boolean {
  return typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");
}
