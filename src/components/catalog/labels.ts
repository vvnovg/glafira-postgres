import type { i18n as I18n, TFunction } from "i18next";

/** Перевод по ключу или, если такого ключа нет (код из каталога более новой версии PostgreSQL), сам код без предупреждения о пропавшем ключе. */
export function labelOr(i18n: I18n, t: TFunction, key: string, fallback: string): string {
  return i18n.exists(key) ? t(key) : fallback;
}
