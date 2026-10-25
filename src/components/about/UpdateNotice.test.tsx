import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { checkUpdate: vi.fn() } };
});

import { api } from "../../api/client";
import { UpdateNotice } from "./UpdateNotice";

describe("UpdateNotice", () => {
  beforeEach(() => {
    vi.mocked(api.checkUpdate).mockReset();
    useWorkspace.setState({ tabs: [], activeId: null });
  });

  it("показывает доступную версию и открывает «О программе»", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValue({ status: "available", version: "0.2.0", notes: null, notesI18n: {}, date: null });
    const user = userEvent.setup();
    renderWithClient(<UpdateNotice />);
    expect(await screen.findByText("Доступна версия 0.2.0.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Подробнее" }));
    expect(useWorkspace.getState().tabs.some((t) => t.kind === "about")).toBe(true);
    await user.click(screen.getByRole("button", { name: "Скрыть уведомление" }));
    expect(screen.queryByText("Доступна версия 0.2.0.")).not.toBeInTheDocument();
  });

  it("«обновления выключены» и ошибка проверки ничего не показывают", async () => {
    vi.mocked(api.checkUpdate).mockResolvedValue({
      status: "disabled",
      reason: { key: "errors.updates.notConfigured", params: {}, text: "Updates are not configured in this build" },
    });
    const first = renderWithClient(<UpdateNotice />);
    // Ждём завершения запроса, а не только его вызова: иначе пустой DOM — это просто состояние загрузки.
    await waitFor(() => expect(first.client.getQueryState(["update-check"])?.status).toBe("success"));
    expect(first.container).toBeEmptyDOMElement();
    first.unmount();

    vi.mocked(api.checkUpdate).mockRejectedValue({ code: "CONNECTION_FAILED", message: "Не удалось проверить обновления", retryable: true });
    const second = renderWithClient(<UpdateNotice />);
    await waitFor(() => expect(second.client.getQueryState(["update-check"])?.status).toBe("error"));
    expect(second.container).toBeEmptyDOMElement();
  });
});
