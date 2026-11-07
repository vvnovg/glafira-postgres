import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentSummary, SUMMARY_LINES, plainSummary } from "./AgentSummary";

const EIGHT = ["Строка 1", "Строка 2", "Строка 3", "Строка 4", "Строка 5", "Строка 6", "Строка 7", "Строка 8"].join("\n");

describe("plainSummary", () => {
  it("снимает **, __, обратные кавычки, ограждения кода и маркеры заголовков", () => {
    expect(
      plainSummary(
        [
          "## Итог",
          "**Без предложений**: `shop.audit` не анализировалась.",
          "__Дальше__: выполните ANALYZE.",
          "```sql",
          "ANALYZE shop.audit;",
          "```",
          "### Шаг",
          "- p3cf855 — индекс по `created_at`",
        ].join("\n"),
      ),
    ).toBe(
      [
        "Итог",
        "Без предложений: shop.audit не анализировалась.",
        "Дальше: выполните ANALYZE.",
        "ANALYZE shop.audit;",
        "Шаг",
        "- p3cf855 — индекс по created_at",
      ].join("\n"),
    );
  });

  it("подчёркивания внутри имён не трогает; # не в начале строки — тоже", () => {
    expect(plainSummary("таблица order__items, столбец _id, тег #42")).toBe("таблица order__items, столбец _id, тег #42");
  });
});

describe("AgentSummary", () => {
  afterEach(() => vi.restoreAllMocks());

  it(`свёрнут до ${SUMMARY_LINES} строк: «Показать всё» раскрывает остальное, «Свернуть» — сворачивает; фокус на той же кнопке`, async () => {
    const user = userEvent.setup();
    render(<AgentSummary text={EIGHT} />);
    const text = screen.getByText(/Строка 1/);
    expect(text.textContent).toBe(EIGHT.split("\n").slice(0, 5).join("\n"));
    expect(text).toHaveClass("clamped");
    const toggle = screen.getByRole("button", { name: "Показать всё" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(toggle).toHaveAttribute("aria-controls", text.id);
    await user.click(toggle);
    expect(text.textContent).toBe(EIGHT);
    expect(text).not.toHaveClass("clamped");
    expect(toggle).toHaveTextContent("Свернуть");
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(document.activeElement).toBe(toggle);
    await user.click(toggle);
    expect(screen.queryByText(/Строка 6/)).toBeNull();
  });

  it("короткий итог — без кнопки; разметка снята", () => {
    render(<AgentSummary text={"**SUMMARY ok**\nПредложение `p3cf855`"} />);
    expect(screen.getByText(/SUMMARY ok/).textContent).toBe("SUMMARY ok\nПредложение p3cf855");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("длинные строки, которые не помещаются в пять строк экрана, тоже сворачиваются", () => {
    vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(300);
    vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100);
    render(<AgentSummary text={"очень длинная строка ".repeat(200)} />);
    expect(screen.getByRole("button", { name: "Показать всё" })).toHaveAttribute("aria-expanded", "false");
  });
});
