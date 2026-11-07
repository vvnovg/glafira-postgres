import { useId, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

/** Строк «Ответа ассистента» в свёрнутом виде (смоук P4c, R16). */
export const SUMMARY_LINES = 5;

/**
 * Итог агента без разметки Markdown (смоук P4c, R16): строки ограждений кода (```sql) убираются, маркеры заголовков в начале
 * строки, `**`, обратные кавычки и `__` вокруг слов снимаются. Остальное — как есть: текст модели — данные, ссылок и HTML
 * строка не создаёт. Подчёркивания внутри имён (`order__items`) не трогаются. Без lookbehind: сборка — для Safari 14.
 */
export function plainSummary(text: string): string {
  return text
    .split("\n")
    .filter((line) => !/^\s*```/.test(line))
    .map((line) =>
      line
        .replace(/^(\s{0,3})#{1,6}\s+/, "$1")
        .replace(/\*\*/g, "")
        .replace(/`/g, "")
        .replace(/(^|[\s(])__(?=\S)/g, "$1")
        .replace(/(\S)__(?=$|[\s.,:;!?)])/g, "$1"),
    )
    .join("\n")
    .trim();
}

/**
 * «Ответ ассистента» — итог фонового запуска (смоук P4c, R16) в строке статуса консоли и карточки и в «Запусках». Текст —
 * без разметки (`plainSummary`). Блок свёрнут до `SUMMARY_LINES` строк: в свёрнутом виде видны первые пять строк текста, а
 * длинные строки обрезает CSS (`.clamped`, line-clamp). «Показать всё» / «Свернуть» — одна кнопка с `aria-expanded`, фокус
 * остаётся на ней. Кнопка есть, если строк больше пяти или текст не помещается в пять строк экрана (`scrollHeight`
 * свёрнутого блока больше его высоты; пересчёт — при изменении ширины). Ширина — во всю строку статуса.
 */
export function AgentSummary({ text, id }: { text: string; id?: string }) {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const [overflow, setOverflow] = useState(false);
  const ref = useRef<HTMLParagraphElement>(null);
  const ownId = useId();
  const textId = `${id ?? ownId}-text`;
  const plain = plainSummary(text);
  const lines = plain.split("\n");
  const long = lines.length > SUMMARY_LINES;

  useLayoutEffect(() => {
    const el = ref.current;
    if (all || el === null) return;
    const measure = () => setOverflow(el.scrollHeight > el.clientHeight + 1);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [all, plain]);

  return (
    <div className="agent-run-summary" id={id}>
      <p id={textId} ref={ref} className={all ? "llm-text" : "llm-text clamped"}>
        {all ? plain : lines.slice(0, SUMMARY_LINES).join("\n")}
      </p>
      {(long || overflow || all) && (
        <button type="button" className="link" aria-expanded={all} aria-controls={textId} onClick={() => setAll(!all)}>
          {all ? t("llmAssist.agent.summaryLess") : t("llmAssist.agent.summaryAll")}
        </button>
      )}
    </div>
  );
}
