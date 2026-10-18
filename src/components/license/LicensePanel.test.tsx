import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { LicenseStatus, PolicyStatus } from "../../api/types";
import { renderWithClient } from "../../test/render";

const FREE: LicenseStatus = { tier: "free", state: "none", source: null, licensee: null, kind: null, expires: null, daysLeft: null, error: null };
const PRO: LicenseStatus = { ...FREE, tier: "pro", state: "valid", source: "user", licensee: "a@b", kind: "personal", expires: "2027-03-31", daysLeft: 166 };
const UNMANAGED: PolicyStatus = { managed: false, source: null, updatesEnabled: true, updateUrl: null, hasLicenseKey: false, language: null, errors: [] };

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getLicenseStatus: vi.fn(),
      getPolicyStatus: vi.fn(),
      setLicenseKey: vi.fn(),
      removeLicenseKey: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { LicenseNotice } from "./LicenseNotice";
import { LicensePanel } from "./LicensePanel";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getPolicyStatus).mockResolvedValue(UNMANAGED);
});

describe("LicensePanel", () => {
  it("активация ключа", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue(FREE);
    vi.mocked(api.setLicenseKey).mockResolvedValue(PRO);
    const user = userEvent.setup();
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText("Free")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Лицензионный ключ"), " a.b ");
    await user.click(screen.getByRole("button", { name: "Активировать" }));
    expect(api.setLicenseKey).toHaveBeenCalledWith("a.b");
    expect(await screen.findByText("Pro")).toBeInTheDocument();
    expect(screen.getByText("a@b")).toBeInTheDocument();
    expect(screen.getByText("2027-03-31")).toBeInTheDocument();
  });

  it("ошибка ключа показывается", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue(FREE);
    vi.mocked(api.setLicenseKey).mockRejectedValue({ code: "INVALID_INPUT", message: "Ключ недействителен: подпись", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<LicensePanel />);
    await user.type(await screen.findByLabelText("Лицензионный ключ"), "x.y");
    await user.click(screen.getByRole("button", { name: "Активировать" }));
    expect(await screen.findByText("Ключ недействителен: подпись")).toBeInTheDocument();
  });

  it("недействительный ключ: причина переводится по ключу", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue({
      ...FREE,
      state: "invalid",
      source: "user",
      error: { key: "errors.license.unsupportedVersion", params: { version: "9" }, text: "Invalid key: a version 9 key requires a newer version of the app" },
    });
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText("Ключ недействителен: ключ версии 9 требует более новой версии приложения")).toBeInTheDocument();
  });

  it("ошибки политики переводятся по ключу", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue(FREE);
    vi.mocked(api.getPolicyStatus).mockResolvedValue({
      ...UNMANAGED,
      managed: true,
      source: "/etc/x/policy.json",
      errors: [{ key: "errors.policy.invalidNumber", params: { key: "updatesEnabled", value: "7" }, text: "updatesEnabled: invalid number 7" }],
    });
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText("updatesEnabled: недопустимое число 7")).toBeInTheDocument();
  });

  it("удаление ключа — с подтверждением на странице", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue(PRO);
    vi.mocked(api.removeLicenseKey).mockResolvedValue(FREE);
    const user = userEvent.setup();
    renderWithClient(<LicensePanel />);
    await user.click(await screen.findByRole("button", { name: "Удалить ключ" }));
    expect(api.removeLicenseKey).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Да, удалить" }));
    expect(api.removeLicenseKey).toHaveBeenCalled();
  });

  it("лицензия организации: ввод недоступен", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, source: "policy", licensee: "ACME", kind: "organization" });
    vi.mocked(api.getPolicyStatus).mockResolvedValue({ ...UNMANAGED, managed: true, source: "/etc/x/policy.json", hasLicenseKey: true });
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText("Лицензия управляется вашей организацией.")).toBeInTheDocument();
    expect(screen.queryByLabelText("Лицензионный ключ")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Удалить ключ" })).not.toBeInTheDocument();
  });

  it("истекает и истекла", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, state: "expiring", daysLeft: 3 });
    const { unmount } = renderWithClient(<LicensePanel />);
    expect(await screen.findByText(/истекает через 3 дн/)).toBeInTheDocument();
    unmount();
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, tier: "free", state: "expired", expires: "2026-10-01", daysLeft: -15 });
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText(/истекла 2026-10-01/)).toBeInTheDocument();
  });

  it("в последний день — «сегодня», а не «через 0 дн.»", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, state: "expiring", expires: "2026-10-17", daysLeft: 0 });
    const { unmount } = renderWithClient(<LicensePanel />);
    expect(await screen.findByText("Лицензия истекает сегодня (2026-10-17).")).toBeInTheDocument();
    expect(screen.queryByText(/через 0/)).not.toBeInTheDocument();
    unmount();
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, state: "expiring", expires: "2026-10-18", daysLeft: 1 });
    renderWithClient(<LicensePanel />);
    expect(await screen.findByText("Лицензия истекает через 1 дн. (2026-10-18).")).toBeInTheDocument();
  });

  it("плашка: daysLeft 0 и 1", async () => {
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, state: "expiring", daysLeft: 0 });
    const { unmount } = renderWithClient(<LicenseNotice />);
    expect(await screen.findByText("Лицензия Pro истекает сегодня.")).toBeInTheDocument();
    unmount();
    vi.mocked(api.getLicenseStatus).mockResolvedValue({ ...PRO, state: "expiring", daysLeft: 1 });
    renderWithClient(<LicenseNotice />);
    expect(await screen.findByText("Лицензия Pro истекает через 1 дн.")).toBeInTheDocument();
  });
});
