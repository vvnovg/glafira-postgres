import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { readAgentLog: vi.fn(), revealAgentLog: vi.fn() } };
});

import { api } from "../../../api/client";
import { AgentLogDialog } from "./AgentLogDialog";

const LOG = '12:00:01 stdout {"type":"result"}\n12:00:02 stderr warning: slow model';

describe("AgentLogDialog", () => {
  beforeEach(() => vi.resetAllMocks());

  it("хвост журнала простым текстом, «Показать файл»; Escape закрывает, фокус возвращается", async () => {
    vi.mocked(api.readAgentLog).mockResolvedValue(LOG);
    vi.mocked(api.revealAgentLog).mockResolvedValue(undefined);
    const onClose = vi.fn();
    const user = userEvent.setup();
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const { unmount } = renderWithClient(<AgentLogDialog runId="a1b2c3d" onClose={onClose} />);
    expect(screen.getByRole("dialog", { name: "Журнал запуска a1b2c3d" })).toHaveFocus();
    expect(screen.getByText("Показан конец журнала (до 256 КБ). Модели журнал не передаётся.")).toBeInTheDocument();
    const pre = await screen.findByText((_, el) => el?.tagName === "PRE" && el.textContent === LOG);
    // Прокручиваемый журнал в порядке Tab — с именем окна и описанием «Показан конец журнала…».
    expect(pre).toHaveAccessibleName("Журнал запуска a1b2c3d");
    expect(pre).toHaveAccessibleDescription("Показан конец журнала (до 256 КБ). Модели журнал не передаётся.");
    expect(api.readAgentLog).toHaveBeenCalledWith("a1b2c3d");
    await user.click(screen.getByRole("button", { name: "Показать файл" }));
    expect(api.revealAgentLog).toHaveBeenCalledWith("a1b2c3d");
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    unmount();
    expect(opener).toHaveFocus();
    opener.remove();
  });

  it("Tab ходит по кругу внутри диалога", async () => {
    vi.mocked(api.readAgentLog).mockResolvedValue(LOG);
    const user = userEvent.setup();
    renderWithClient(<AgentLogDialog runId="a1b2c3d" onClose={() => {}} />);
    const pre = await screen.findByText((_, el) => el?.tagName === "PRE" && el.textContent === LOG);
    await user.tab();
    expect(pre).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Показать файл" })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole("button", { name: "Закрыть" })).toHaveFocus();
    await user.tab();
    expect(pre).toHaveFocus();
  });

  it("ошибка чтения — с «Повторить»; пустой журнал — пояснение", async () => {
    vi.mocked(api.readAgentLog).mockRejectedValueOnce({ code: "NOT_FOUND", message: "no log", retryable: false }).mockResolvedValue("");
    const user = userEvent.setup();
    renderWithClient(<AgentLogDialog runId="a1b2c3d" onClose={() => {}} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("no log");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText("Журнал пуст")).toBeInTheDocument();
  });

  it("ошибка «Показать файл» — баннером в диалоге", async () => {
    vi.mocked(api.readAgentLog).mockResolvedValue(LOG);
    vi.mocked(api.revealAgentLog).mockRejectedValue({ code: "NOT_FOUND", message: "gone", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentLogDialog runId="a1b2c3d" onClose={() => {}} />);
    await user.click(screen.getByRole("button", { name: "Показать файл" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("gone");
  });
});
