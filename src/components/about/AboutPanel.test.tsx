import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getAppInfo: vi.fn().mockResolvedValue({ name: "X", version: "0.1.0", commit: "abc123", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" }),
      getLicenseStatus: vi.fn().mockResolvedValue({ tier: "free", state: "none", source: null, licensee: null, kind: null, expires: null, daysLeft: null, error: null }),
      getPolicyStatus: vi.fn().mockResolvedValue({ managed: false, source: null, updatesEnabled: true, updateUrl: null, hasLicenseKey: false, language: null, errors: [] }),
    },
  };
});

import { api } from "../../api/client";
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
