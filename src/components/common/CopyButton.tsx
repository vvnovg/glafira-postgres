import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/**
 * Кнопка «Копировать»: буфер обмена может быть недоступен или отказать — об итоге сообщаем в любом случае (запасной
 * путь — видимое сообщение «Не удалось скопировать»). `caption` — видимая подпись вместо «Копировать»; она же имя для
 * скринридера (видимая подпись входит в имя, WCAG 2.5.3), и `label` при ней не нужен. Без `caption` имя — `label`.
 * Плагин буфера обмена Tauri не подключён, поэтому работает `navigator.clipboard`. Тексты итога — `test.copied` и
 * `test.copyFailed` — общие для всех мест.
 */
export function CopyButton({ text, label, caption }: { text: string } & ({ label: string; caption?: undefined } | { label?: undefined; caption: string })) {
  const { t } = useTranslation();
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  async function copy() {
    let next: "copied" | "failed" = "copied";
    try {
      if (!navigator.clipboard) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(text);
    } catch {
      next = "failed";
    }
    setState(next);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setState("idle"), 2500);
  }
  return (
    <>
      <button type="button" aria-label={caption === undefined ? label : undefined} onClick={() => void copy()}>
        {caption ?? t("common.copy")}
      </button>
      <span role="status" className="muted">
        {state === "copied" ? t("test.copied") : state === "failed" ? t("test.copyFailed") : ""}
      </span>
    </>
  );
}
