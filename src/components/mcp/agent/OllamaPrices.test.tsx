import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, OllamaPrices as Prices } from "../../../api/types";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { getOllamaPrices: vi.fn(), refreshOllamaPrices: vi.fn() } };
});

import { api } from "../../../api/client";
import { OllamaPrices, UPDATED_NOTE_MS } from "./OllamaPrices";

const builtin: Prices = { date: "2026-11-07", source: "https://ollama.com/pricing", saved: false, fetchedAtMs: null, models: 24, refreshAllowed: true };
const BUILTIN = "Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 07.11.2026, встроенным в Glafira";
const REFRESH = "Обновить цены";

function appError(key: string, message: string): AppError {
  return { code: "INTERNAL", key, message, retryable: false };
}

describe("OllamaPrices", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getOllamaPrices).mockResolvedValue(builtin);
  });

  it("называет действующую таблицу; «Обновить цены» загружает страницу и показывает новую таблицу", async () => {
    let resolve: (p: Prices) => void = () => {};
    vi.mocked(api.refreshOllamaPrices).mockImplementation(() => new Promise((r) => (resolve = r)));
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices />);
    expect(await screen.findByText(BUILTIN)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Подсказка: Обновить цены" })).toBeInTheDocument();
    const button = screen.getByRole("button", { name: REFRESH });
    expect(button).toBeEnabled();
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    // Пока страница загружается, кнопка не теряет фокус и повторно не нажимается.
    expect(button).toHaveTextContent("Загрузка цен…");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveFocus();
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    resolve({ ...builtin, date: "2026-12-01", saved: true, fetchedAtMs: 1_796_117_400_000, models: 25 });
    expect(await screen.findByText("Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 01.12.2026, загруженным с ollama.com/pricing")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25");
    expect(button).toHaveTextContent(REFRESH);
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it("«Цены обновлены…» исчезает через несколько секунд или при следующем изменении в разделе (итоговое ревью)", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      vi.mocked(api.refreshOllamaPrices).mockResolvedValue({ ...builtin, saved: true, fetchedAtMs: 1_796_117_400_000, models: 25 });
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const { rerender } = renderWithClient(<OllamaPrices changed="a" />);
      await user.click(await screen.findByRole("button", { name: REFRESH }));
      await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25"));
      act(() => vi.advanceTimersByTime(UPDATED_NOTE_MS - 100));
      expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25");
      act(() => vi.advanceTimersByTime(200));
      expect(screen.getByRole("status")).toHaveTextContent(/^$/);
      // Снова «Обновить цены», и раньше срока — правка в разделе: сообщение о прошлом действии уже не к месту.
      await user.click(screen.getByRole("button", { name: REFRESH }));
      await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25"));
      rerender(<OllamaPrices changed="b" />);
      expect(screen.getByRole("status")).toHaveTextContent(/^$/);
      expect(screen.getByRole("button", { name: REFRESH })).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("не разобралась страница или сеть — ошибка у кнопки, прежняя таблица остаётся", async () => {
    vi.mocked(api.refreshOllamaPrices).mockRejectedValueOnce(appError("errors.agent.pricesParse", "Could not parse the Ollama price page"));
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices />);
    await user.click(await screen.findByRole("button", { name: REFRESH }));
    expect(await screen.findByText("Не удалось разобрать страницу цен Ollama")).toBeInTheDocument();
    expect(screen.getByText(BUILTIN)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("");
    vi.mocked(api.refreshOllamaPrices).mockRejectedValueOnce({ ...appError("errors.agent.pricesFetch", "Could not load the Ollama price page"), code: "CONNECTION_FAILED" });
    await user.click(screen.getByRole("button", { name: REFRESH }));
    expect(await screen.findByText("Не удалось загрузить страницу цен Ollama")).toBeInTheDocument();
    expect(screen.queryByText("Не удалось разобрать страницу цен Ollama")).toBeNull();
  });

  it("политика запрещает обновления — кнопка неактивна с пояснением", async () => {
    vi.mocked(api.getOllamaPrices).mockResolvedValue({ ...builtin, refreshAllowed: false });
    renderWithClient(<OllamaPrices />);
    const button = await screen.findByRole("button", { name: REFRESH });
    await waitFor(() => expect(button).toBeDisabled());
    expect(button).toHaveAccessibleDescription("Политика организации не разрешает загружать цены: оценки — по этой таблице");
    expect(api.refreshOllamaPrices).not.toHaveBeenCalled();
  });

  it("таблица не прочиталась — ошибка с «Повторить»", async () => {
    vi.mocked(api.getOllamaPrices).mockRejectedValueOnce(appError("errors.io", "I/O error: {{error}}"));
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices />);
    await user.click(await screen.findByRole("button", { name: "Повторить" }));
    expect(await screen.findByText(BUILTIN)).toBeInTheDocument();
  });
});
