import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { LANGUAGES } from "./languages";

const modules = import.meta.glob<{ default: Record<string, unknown> }>("../locales/*.json", { eager: true });

/** Все словари src/locales/<код>.json по коду языка. Новый язык подключается появлением файла. */
export const DICTIONARIES: Record<string, Record<string, unknown>> = Object.fromEntries(
  Object.entries(modules).map(([path, m]) => [path.replace(/^.*\/([^/]+)\.json$/, "$1"), m.default]),
);

const PSEUDO = "x-pseudo";

/** Псевдоязык (только dev): текст в скобках, длиннее на ~30 % — видно забытые строки и узкие места. */
function pseudo(value: string): string {
  const map: Record<string, string> = { a: "á", e: "é", i: "í", o: "ó", u: "ú", A: "Á", E: "É", O: "Ó" };
  const body = value.replace(/\{\{[^}]+\}\}|[aeiouAEO]/g, (m) => (m.startsWith("{{") ? m : map[m] ?? m));
  return `[!! ${body}${"·".repeat(Math.ceil(value.length * 0.3))} !!]`;
}

const instance = i18next.use(initReactI18next);
if (import.meta.env.DEV) {
  instance.use({
    type: "postProcessor",
    name: "pseudo",
    process: (value: string) => (i18next.language === PSEUDO ? pseudo(value) : value),
  });
}

void instance.init({
  resources: {
    ...Object.fromEntries(Object.entries(DICTIONARIES).map(([code, d]) => [code, { translation: d }])),
    ...(import.meta.env.DEV ? { [PSEUDO]: { translation: DICTIONARIES.en } } : {}),
  },
  showSupportNotice: false,
  lng: "en",
  fallbackLng: "en",
  interpolation: { escapeValue: false }, // React уже экранирует
  postProcess: import.meta.env.DEV ? ["pseudo"] : [],
});

if (import.meta.env.DEV) {
  i18next.on("missingKey", (_lngs, _ns, key) => console.warn(`[i18n] missing key ${key}`));
}

export const i18n = i18next;

/** Язык документа следует за действующим языком интерфейса (скринридеры, орфография, переносы). */
function syncDocumentLang(): void {
  if (typeof document !== "undefined") document.documentElement.lang = currentLocale();
}
syncDocumentLang();

export async function setLanguage(code: string): Promise<void> {
  const known = LANGUAGES.some((l) => l.code === code) || (import.meta.env.DEV && code === PSEUDO);
  await i18next.changeLanguage(known ? code : "en");
  syncDocumentLang();
}

export function currentLocale(): string {
  return i18next.language === PSEUDO ? "en" : i18next.language || "en";
}
