import { i18n } from "./index";

type Localizable = { key?: string; params?: Record<string, string>; text?: string; message?: string };

/**
 * Перевод текста из Rust: по ключу, а если ключа нет в словаре — английский текст как есть.
 * Параметры из Rust — строки; `count` превращается в число, иначе i18next не выбирает плюральную форму. */
export function tr(t: Localizable): string {
  const params: Record<string, unknown> = { ...t.params };
  const count = Number(t.params?.count);
  if (t.params?.count !== undefined && t.params.count.trim() !== "" && Number.isFinite(count)) params.count = count;
  if (t.key && i18n.exists(t.key, params)) return i18n.t(t.key, params);
  return t.text ?? t.message ?? "";
}
