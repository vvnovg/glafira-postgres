/**
 * Поля приложения — адреса, имена топиков, пути, JSON: автокоррекция и заглавная буква
 * в начале (macOS WebView) только мешают. Отключаем их для каждого поля при фокусе.
 */
const ATTRS: Record<string, string> = {
  autocapitalize: "off",
  autocorrect: "off",
  autocomplete: "off",
  spellcheck: "false",
};

export function makePlain(el: Element) {
  if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return;
  for (const [name, value] of Object.entries(ATTRS)) {
    if (!el.hasAttribute(name)) el.setAttribute(name, value);
  }
}

export function installPlainInputs(doc: Document = document) {
  doc.addEventListener("focusin", (e) => {
    if (e.target instanceof Element) makePlain(e.target);
  });
}
