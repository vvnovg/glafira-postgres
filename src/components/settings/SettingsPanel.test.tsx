import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LanguageSettings } from "../../api/types";
import { i18n } from "../../i18n";
import { renderWithClient } from "../../test/render";

const base: LanguageSettings = {
  system: "ru-RU", systemMatch: "ru", chosen: null, policy: null, effective: "ru", reason: "system",
  languages: ["en", "ru"],
};

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getLanguageSettings: vi.fn(),
      setLanguage: vi.fn(),
      getUiSettings: vi.fn(),
      setUiSettings: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { SettingsPanel } from "./SettingsPanel";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getUiSettings).mockResolvedValue({ showSystemObjects: false });
});

// Выбор «en» в тесте переключает глобальный язык; сброс идёт в act, пока компоненты ещё
// смонтированы (useTranslation обновляет их по languageChanged).
afterEach(() => act(() => i18n.changeLanguage("ru")));

describe("SettingsPanel", () => {
  it("выбор языка; доступны все языки", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    vi.mocked(api.setLanguage).mockResolvedValue({ ...base, chosen: "en", effective: "en", reason: "user" });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const select = await screen.findByLabelText("Язык интерфейса");
    for (const name of ["English", "Русский"]) {
      const option = screen.getByRole("option", { name });
      expect(option).toBeEnabled();
    }
    expect(screen.getByRole("option", { name: /Как в системе/ })).toBeInTheDocument();
    await user.selectOptions(select, "en");
    expect(api.setLanguage).toHaveBeenCalledWith("en");
  });

  it("язык задан организацией", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue({ ...base, policy: "ru", reason: "policy" });
    renderWithClient(<SettingsPanel />);
    expect(await screen.findByLabelText("Язык интерфейса")).toBeDisabled();
    expect(screen.getByText(/задан вашей организацией/)).toBeInTheDocument();
  });

  it("«Показывать системные объекты» сохраняется и отражается в флажке", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    vi.mocked(api.setUiSettings).mockResolvedValue({ showSystemObjects: true });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const box = await screen.findByRole("checkbox", { name: /Показывать системные объекты/ });
    await vi.waitFor(() => expect(box).toBeEnabled());
    expect(box).not.toBeChecked();
    await user.click(box);
    expect(api.setUiSettings).toHaveBeenCalledWith({ showSystemObjects: true });
    await vi.waitFor(() => expect(box).toBeChecked());
  });

  it("ошибка сохранения настройки показывается, флажок остаётся прежним", async () => {
    vi.mocked(api.getLanguageSettings).mockResolvedValue(base);
    vi.mocked(api.setUiSettings).mockRejectedValue({ code: "INTERNAL", message: "disk full", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<SettingsPanel />);
    const box = await screen.findByRole("checkbox", { name: /Показывать системные объекты/ });
    await vi.waitFor(() => expect(box).toBeEnabled());
    await user.click(box);
    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    expect(box).not.toBeChecked();
  });
});
