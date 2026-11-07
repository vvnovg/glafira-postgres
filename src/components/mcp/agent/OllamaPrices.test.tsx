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
const REFRESH = "Обновить модели и цены";

function appError(key: string, message: string): AppError {
  return { code: "INTERNAL", key, message, retryable: false };
}

/** Перечитывание списка моделей: удачное, если не сказано иначе. */
const models = (isError = false) => vi.fn(async () => ({ isError }));

describe("OllamaPrices", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getOllamaPrices).mockResolvedValue(builtin);
  });

  it("называет действующую таблицу; «Обновить модели и цены» загружает цены, потом перечитывает список моделей", async () => {
    let resolve: (p: Prices) => void = () => {};
    vi.mocked(api.refreshOllamaPrices).mockImplementation(() => new Promise((r) => (resolve = r)));
    const refreshModels = models();
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices refreshModels={refreshModels} />);
    expect(await screen.findByText(BUILTIN)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Подсказка: Обновить модели и цены" })).toBeInTheDocument();
    const button = screen.getByRole("button", { name: REFRESH });
    expect(button).toBeEnabled();
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    // Пока страница загружается, кнопка не теряет фокус и повторно не нажимается; список ждёт новых цен.
    expect(button).toHaveTextContent("Обновление моделей и цен…");
    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveFocus();
    expect(refreshModels).not.toHaveBeenCalled();
    await user.click(button);
    expect(api.refreshOllamaPrices).toHaveBeenCalledTimes(1);
    act(() => resolve({ ...builtin, date: "2026-12-01", saved: true, fetchedAtMs: 1_796_117_400_000, models: 25 }));
    expect(await screen.findByText("Стоимость облачных запусков через Ollama — оценка по ценам Ollama Cloud на 01.12.2026, загруженным с ollama.com/pricing")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Список моделей обновлён · Цены обновлены: моделей 25"));
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(button).toHaveTextContent(REFRESH);
    expect(button).not.toHaveAttribute("aria-disabled");
  });

  it("итог исчезает через несколько секунд или при следующем изменении в разделе (итоговое ревью)", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      vi.mocked(api.refreshOllamaPrices).mockResolvedValue({ ...builtin, saved: true, fetchedAtMs: 1_796_117_400_000, models: 25 });
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      const refreshModels = models();
      const { rerender } = renderWithClient(<OllamaPrices changed="a" refreshModels={refreshModels} />);
      await user.click(await screen.findByRole("button", { name: REFRESH }));
      await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25"));
      act(() => vi.advanceTimersByTime(UPDATED_NOTE_MS - 100));
      expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25");
      act(() => vi.advanceTimersByTime(200));
      expect(screen.getByRole("status")).toHaveTextContent(/^$/);
      // Снова кнопка, и раньше срока — правка в разделе: сообщение о прошлом действии уже не к месту.
      await user.click(screen.getByRole("button", { name: REFRESH }));
      await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Цены обновлены: моделей 25"));
      rerender(<OllamaPrices changed="b" refreshModels={refreshModels} />);
      expect(screen.getByRole("status")).toHaveTextContent(/^$/);
      expect(screen.getByRole("button", { name: REFRESH })).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("не разобралась страница или сеть — ошибка у кнопки, прежняя таблица остаётся, список моделей перечитан", async () => {
    vi.mocked(api.refreshOllamaPrices).mockRejectedValueOnce(appError("errors.agent.pricesParse", "Could not parse the Ollama price page"));
    const refreshModels = models();
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices refreshModels={refreshModels} />);
    await user.click(await screen.findByRole("button", { name: REFRESH }));
    expect(await screen.findByText("Не удалось разобрать страницу цен Ollama")).toBeInTheDocument();
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(screen.getByText(BUILTIN)).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("");
    vi.mocked(api.refreshOllamaPrices).mockRejectedValueOnce({ ...appError("errors.agent.pricesFetch", "Could not load the Ollama price page"), code: "CONNECTION_FAILED" });
    await user.click(screen.getByRole("button", { name: REFRESH }));
    expect(await screen.findByText("Не удалось загрузить страницу цен Ollama")).toBeInTheDocument();
    expect(screen.queryByText("Не удалось разобрать страницу цен Ollama")).toBeNull();
  });

  it("политика запрещает загрузку цен — кнопка перечитывает только список моделей, пояснение связано с ней", async () => {
    vi.mocked(api.getOllamaPrices).mockResolvedValue({ ...builtin, refreshAllowed: false });
    const refreshModels = models();
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices refreshModels={refreshModels} />);
    const button = await screen.findByRole("button", { name: REFRESH });
    await waitFor(() => expect(button).toHaveAccessibleDescription("Политика организации не разрешает загружать цены: оценки — по этой таблице"));
    expect(button).toBeEnabled();
    await user.click(button);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Список моделей обновлён"));
    expect(refreshModels).toHaveBeenCalledTimes(1);
    expect(api.refreshOllamaPrices).not.toHaveBeenCalled();
  });

  it("список моделей не перечитался — об этом не сказано, что он обновлён", async () => {
    vi.mocked(api.refreshOllamaPrices).mockResolvedValue({ ...builtin, saved: true, fetchedAtMs: 1_796_117_400_000, models: 25 });
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices refreshModels={models(true)} />);
    await user.click(await screen.findByRole("button", { name: REFRESH }));
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent(/^Цены обновлены: моделей 25$/));
  });

  it("таблица не прочиталась — ошибка с «Повторить», кнопка остаётся", async () => {
    vi.mocked(api.getOllamaPrices).mockRejectedValueOnce(appError("errors.io", "I/O error: {{error}}"));
    const user = userEvent.setup();
    renderWithClient(<OllamaPrices refreshModels={models()} />);
    expect(screen.getByRole("button", { name: REFRESH })).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: "Повторить" }));
    expect(await screen.findByText(BUILTIN)).toBeInTheDocument();
  });
});
