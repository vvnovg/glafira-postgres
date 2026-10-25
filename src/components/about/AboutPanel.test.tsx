import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn().mockResolvedValue("/tmp/diag.zip") }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getAppInfo: vi.fn().mockResolvedValue({ name: "X", version: "0.1.0", commit: "abc123", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" }),
      getPolicyStatus: vi.fn().mockResolvedValue({ managed: false, source: null, updatesEnabled: true, updateUrl: null, language: null, errors: [] }),
      exportDiagnostics: vi.fn().mockResolvedValue(undefined),
      checkUpdate: vi.fn().mockResolvedValue({ status: "upToDate" }),
      installUpdate: vi.fn().mockResolvedValue(undefined),
    },
  };
});

import { save } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../../api/client";
import { i18n } from "../../i18n";
import { LICENSE_URL, SUPPORT_URL } from "../../lib/links";
import { useDiagnosticsRequest } from "../../state/diagnosticsRequest";
import { AboutPanel } from "./AboutPanel";

describe("AboutPanel", () => {
  it("версия, сборка и папки данных и логов", async () => {
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText("0.1.0")).toBeInTheDocument();
    expect(screen.getByText(/помогает разобраться, что происходит с PostgreSQL/)).toBeInTheDocument();
    expect(screen.getByText("abc123")).toBeInTheDocument();
    expect(screen.getByText("/d")).toBeInTheDocument();
    expect(screen.getByText("/l")).toBeInTheDocument();
    expect(screen.queryByText("Тариф")).not.toBeInTheDocument();
    expect(screen.getByText("нет")).toBeInTheDocument();
  });

  it("строка лицензии и кнопка поддержки открывают страницы в браузере", async () => {
    vi.mocked(openUrl).mockClear();
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText("Лицензия")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Apache 2.0" }));
    expect(openUrl).toHaveBeenCalledWith(LICENSE_URL);
    await user.click(screen.getByRole("button", { name: "Поддержать проект" }));
    expect(openUrl).toHaveBeenCalledWith(SUPPORT_URL);
    expect(screen.getByText(/пожертвования помогают/)).toBeInTheDocument();
    expect(screen.queryByText("Тариф")).not.toBeInTheDocument();
  });

  it("диагностика: диалог сохранения, экспорт в выбранный файл и путь архива", async () => {
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Собрать диагностику" }));
    expect(api.exportDiagnostics).toHaveBeenCalledWith("/tmp/diag.zip");
    expect(await screen.findByText(/Архив сохранён: \/tmp\/diag\.zip/)).toBeInTheDocument();
  });

  it("отмена диалога: экспорта и ошибки нет", async () => {
    vi.mocked(save).mockResolvedValueOnce(null);
    vi.mocked(api.exportDiagnostics).mockClear();
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Собрать диагностику" }));
    expect(api.exportDiagnostics).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.queryByText(/Архив сохранён/)).not.toBeInTheDocument();
  });

  it("ошибка экспорта — баннер", async () => {
    vi.mocked(api.exportDiagnostics).mockRejectedValueOnce({ code: "INTERNAL", message: "Внутренняя ошибка", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Собрать диагностику" }));
    expect(await screen.findByText("Внутренняя ошибка")).toBeInTheDocument();
    expect(screen.queryByText(/Архив сохранён/)).not.toBeInTheDocument();
  });

  it("запрос из меню Help запускает сбор диагностики один раз", async () => {
    vi.mocked(save).mockClear();
    vi.mocked(api.exportDiagnostics).mockClear();
    useDiagnosticsRequest.getState().request();
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText(/Архив сохранён/)).toBeInTheDocument();
    expect(save).toHaveBeenCalledTimes(1);
    expect(api.exportDiagnostics).toHaveBeenCalledWith("/tmp/diag.zip");
    expect(useDiagnosticsRequest.getState().pending).toBe(false);
  });

  it("запрос из меню во время сбора не запускает второй сбор", async () => {
    vi.mocked(save).mockClear();
    let resolveSave: (p: string) => void = () => {};
    vi.mocked(save).mockImplementationOnce(() => new Promise((r) => (resolveSave = r)));
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Собрать диагностику" }));
    expect(save).toHaveBeenCalledTimes(1);
    act(() => useDiagnosticsRequest.getState().request());
    expect(useDiagnosticsRequest.getState().pending).toBe(false);
    resolveSave("/tmp/diag.zip");
    expect(await screen.findByText(/Архив сохранён/)).toBeInTheDocument();
    expect(save).toHaveBeenCalledTimes(1);
  });

  it("обновление доступно: описание и установка", async () => {
    vi.mocked(api.installUpdate).mockClear();
    // Релиз без notes_i18n (0.2.0 и раньше): показывается обычный notes.
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: "Исправления", notesI18n: {}, date: null });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Доступна версия 0.2.0")).toBeInTheDocument();
    expect(await screen.findByText("Исправления")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Установить и перезапустить" }));
    expect(api.installUpdate).toHaveBeenCalledTimes(1);
  });

  it("заметки к обновлению — Markdown на языке интерфейса, ссылки — в системный браузер", async () => {
    vi.mocked(openUrl).mockClear();
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({
      status: "available",
      version: "0.4.0",
      notes: "- **Fixes**",
      notesI18n: { en: "- **Fixes**", ru: "- **Исправления** для `pg_stat_activity`\n- [Подробнее](https://example.com/r)" },
      date: null,
    });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    const notes = await screen.findByRole("region", { name: "Что нового в версии 0.4.0" });
    expect((await screen.findByText("Исправления")).tagName).toBe("STRONG");
    expect(screen.getByText("pg_stat_activity").tagName).toBe("CODE");
    expect(notes.querySelectorAll("li")).toHaveLength(2);
    expect(notes.textContent).not.toContain("**");
    expect(screen.queryByText(/Fixes/)).not.toBeInTheDocument();
    await user.click(screen.getByRole("link", { name: "Подробнее" }));
    expect(openUrl).toHaveBeenCalledWith("https://example.com/r");
  });

  it("смена языка меняет заметки без новой проверки; нет перевода — английские", async () => {
    vi.mocked(api.checkUpdate).mockClear();
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({
      status: "available",
      version: "0.4.0",
      notes: "- Fixes",
      notesI18n: { en: "- Fixes", ru: "- Исправления" },
      date: null,
    });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Исправления")).toBeInTheDocument();
    try {
      await act(() => i18n.changeLanguage("en"));
      expect(await screen.findByText("Fixes")).toBeInTheDocument();
      // Языка нет в notes_i18n — английский раздел.
      await act(() => i18n.changeLanguage("de"));
      expect(await screen.findByText("Fixes")).toBeInTheDocument();
      expect(api.checkUpdate).toHaveBeenCalledTimes(1);
    } finally {
      await act(() => i18n.changeLanguage("ru"));
    }
  });

  it("политика выключила обновления", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({
      status: "disabled",
      reason: { key: "errors.updates.managed", params: {}, text: "Updates are managed by your organization" },
    });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Обновления управляются вашей организацией")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Установить и перезапустить" })).not.toBeInTheDocument();
  });

  it("последняя версия; ошибка сети — баннер с повтором", async () => {
    vi.mocked(api.checkUpdate)
      .mockRejectedValueOnce({ code: "CONNECTION_FAILED", message: "Не удалось проверить обновления", retryable: true })
      .mockResolvedValueOnce({ status: "upToDate" });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Не удалось проверить обновления")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("Установлена последняя версия")).toBeInTheDocument();
  });

  it("ошибка установки — баннер", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: null, notesI18n: {}, date: null });
    vi.mocked(api.installUpdate).mockRejectedValueOnce({ code: "INTERNAL", message: "Не удалось установить обновление", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    await user.click(await screen.findByRole("button", { name: "Установить и перезапустить" }));
    expect(await screen.findByText("Не удалось установить обновление")).toBeInTheDocument();
  });

  it("установка идёт: кнопки заблокированы, виден статус", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: null, notesI18n: {}, date: null });
    vi.mocked(api.installUpdate).mockImplementationOnce(() => new Promise(() => {}));
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    await user.click(await screen.findByRole("button", { name: "Установить и перезапустить" }));
    expect(await screen.findByText("Загрузка и установка обновления…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Установить и перезапустить" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Проверить обновления" })).toBeDisabled();
  });

  it("новая проверка убирает ошибку установки", async () => {
    vi.mocked(api.checkUpdate)
      .mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: null, notesI18n: {}, date: null })
      .mockResolvedValueOnce({ status: "available", version: "0.2.0", notes: null, notesI18n: {}, date: null });
    vi.mocked(api.installUpdate).mockRejectedValueOnce({ code: "INTERNAL", message: "Не удалось установить обновление", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AboutPanel />);
    await user.click(await screen.findByRole("button", { name: "Проверить обновления" }));
    await user.click(await screen.findByRole("button", { name: "Установить и перезапустить" }));
    expect(await screen.findByText("Не удалось установить обновление")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Проверить обновления" }));
    expect(await screen.findByText("Доступна версия 0.2.0")).toBeInTheDocument();
    expect(screen.queryByText("Не удалось установить обновление")).not.toBeInTheDocument();
  });

  it("ошибки политики видны в «О программе»", async () => {
    vi.mocked(api.getPolicyStatus).mockResolvedValueOnce({
      managed: true,
      source: "/etc/x/policy.json",
      updatesEnabled: false,
      updateUrl: null,
      language: null,
      errors: [
        {
          key: "errors.policy.httpsRequired",
          params: { key: "updateUrl" },
          text: "updateUrl: an https:// address is required — updates are not downloaded over http",
        },
      ],
    });
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText(/updateUrl: нужен адрес https/)).toBeInTheDocument();
    expect(screen.getByText("задана")).toBeInTheDocument();
  });

  it("ошибка загрузки сведений — баннер с повтором", async () => {
    vi.mocked(api.getAppInfo).mockRejectedValueOnce({ code: "INTERNAL", message: "Внутренняя ошибка", retryable: false });
    renderWithClient(<AboutPanel />);
    expect(await screen.findByText("Внутренняя ошибка")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Повторить" })).toBeInTheDocument();
  });
});
