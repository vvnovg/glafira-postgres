import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/**
 * Кнопка «Копировать»: буфер обмена может быть недоступен или отказать — об итоге сообщаем в любом случае
 * (запасной путь — видимое сообщение «Не удалось скопировать»). Плагин буфера обмена Tauri не подключён,
 * поэтому работает `navigator.clipboard`. Тексты итога — `test.copied` и `test.copyFailed` — общие для всех мест.
 */
export function CopyButton({ text, label }: { text: string; label: string }) {
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
      <button type="button" aria-label={label} onClick={() => void copy()}>
        {t("common.copy")}
      </button>
      <span role="status" className="muted">
        {state === "copied" ? t("test.copied") : state === "failed" ? t("test.copyFailed") : ""}
      </span>
    </>
  );
}
