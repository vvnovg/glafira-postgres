import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: vi.fn().mockResolvedValue("/tmp/diag.zip") }));
vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getAppInfo: vi.fn().mockResolvedValue({ name: "X", version: "0.1.0", commit: "abc123", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" }),
      getLicenseStatus: vi.fn().mockResolvedValue({ tier: "free", state: "none", source: null, licensee: null, kind: null, expires: null, daysLeft: null, error: null }),
      getPolicyStatus: vi.fn().mockResolvedValue({ managed: false, source: null, updatesEnabled: true, updateUrl: null, hasLicenseKey: false, language: null, errors: [] }),
      exportDiagnostics: vi.fn().mockResolvedValue(undefined),
    },
  };
});

import { save } from "@tauri-apps/plugin-dialog";
import { api } from "../../api/client";
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
    expect(screen.getByText("Free")).toBeInTheDocument();
    expect(screen.getByText("нет")).toBeInTheDocument();
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

  it("ошибки политики видны в «О программе»", async () => {
    vi.mocked(api.getPolicyStatus).mockResolvedValueOnce({
      managed: true,
      source: "/etc/x/policy.json",
      updatesEnabled: false,
      updateUrl: null,
      hasLicenseKey: false,
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
