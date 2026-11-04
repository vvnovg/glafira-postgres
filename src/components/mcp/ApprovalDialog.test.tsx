import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { MCP_STATUS_KEY } from "../../lib/useMcpStatus";
import { mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getMcpStatus: vi.fn(), answerMcpApproval: vi.fn() } };
});

import { api } from "../../api/client";
import { ApprovalDialog } from "./ApprovalDialog";

const qwen = { sessionId: "s1", clientName: "qwen-code", clientVersion: "0.26.7" };
const claude = { sessionId: "s2", clientName: "claude-code", clientVersion: "2.1.0" };

beforeEach(() => vi.clearAllMocks());

describe("ApprovalDialog", () => {
  it("спрашивает о новом клиенте; после ответа сразу следующий в очереди, отвеченный не возвращается", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ openConnections: 2, pending: [qwen, claude] }));
    vi.mocked(api.answerMcpApproval).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderWithClient(<ApprovalDialog />);
    const dialog = await screen.findByRole("dialog", { name: "qwen-code 0.26.7 хочет подключиться к Glafira" });
    expect(dialog).toHaveTextContent("Он увидит диагностику подключений, открытых для LLM-ассистентов: 2.");
    expect(dialog).not.toHaveTextContent("и данные в");
    expect(dialog).toHaveTextContent("Изменить базы через Glafira ассистент не может.");
    expect(dialog).toHaveTextContent("Ждут ответа ещё: 1");
    await user.click(screen.getByRole("button", { name: "Разрешить" }));
    expect(api.answerMcpApproval).toHaveBeenCalledWith("s1", true);
    // Перечитанный статус ещё старый (событие в тесте не приходит) — а диалог уже о следующем клиенте.
    expect(await screen.findByRole("dialog", { name: "claude-code 2.1.0 хочет подключиться к Glafira" })).toBeInTheDocument();
    await waitFor(() => expect(api.getMcpStatus).toHaveBeenCalledTimes(2));
    expect(screen.getByRole("dialog", { name: "claude-code 2.1.0 хочет подключиться к Glafira" })).not.toHaveTextContent("Ждут ответа ещё");
  });

  it("называет подключения с данными, если они есть", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ openConnections: 3, dataConnections: 1, pending: [qwen] }));
    renderWithClient(<ApprovalDialog />);
    expect(await screen.findByRole("dialog")).toHaveTextContent(
      "Он увидит диагностику подключений, открытых для LLM-ассистентов: 3, и данные в 1 из них.",
    );
  });

  it("клиент без имени: ответ только на эту сессию; «Отклонить» отправляет отказ", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ pending: [{ sessionId: "s9", clientName: null, clientVersion: null }] }));
    vi.mocked(api.answerMcpApproval).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderWithClient(<ApprovalDialog />);
    expect(await screen.findByRole("dialog", { name: "Неизвестный MCP-клиент хочет подключиться к Glafira" })).toHaveTextContent(
      "ответ действует только на эту сессию",
    );
    await user.click(screen.getByRole("button", { name: "Отклонить" }));
    expect(api.answerMcpApproval).toHaveBeenCalledWith("s9", false);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("без ожидающих запросов ничего не показывает", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    const { client } = renderWithClient(<ApprovalDialog />);
    await waitFor(() => expect(client.getQueryData(MCP_STATUS_KEY)).toBeDefined());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("ошибка ответа показывается, вопрос остаётся", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ pending: [qwen] }));
    vi.mocked(api.answerMcpApproval).mockRejectedValue({ code: "INTERNAL", message: "session is gone", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<ApprovalDialog />);
    await user.click(await screen.findByRole("button", { name: "Разрешить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("session is gone");
    expect(screen.getByRole("dialog", { name: "qwen-code 0.26.7 хочет подключиться к Glafira" })).toBeInTheDocument();
  });

  describe("клавиатура и фокус", () => {
    const pendingOne = () => vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ pending: [qwen] }));

    it("при появлении фокус на самом диалоге, а не на «Разрешить»", async () => {
      pendingOne();
      renderWithClient(<ApprovalDialog />);
      const dialog = await screen.findByRole("dialog");
      expect(dialog).toHaveFocus();
      expect(screen.getByRole("button", { name: "Разрешить" })).not.toHaveFocus();
    });

    it("Escape отклоняет и никогда не разрешает; второй Escape, пока ответ в пути, ничего не шлёт", async () => {
      pendingOne();
      vi.mocked(api.answerMcpApproval).mockReturnValue(new Promise(() => {}));
      const user = userEvent.setup();
      renderWithClient(<ApprovalDialog />);
      await screen.findByRole("dialog");
      await user.keyboard("{Escape}");
      expect(api.answerMcpApproval).toHaveBeenCalledWith("s1", false);
      await waitFor(() => expect(screen.getByRole("button", { name: "Отклонить" })).toBeDisabled());
      await user.keyboard("{Escape}");
      expect(api.answerMcpApproval).toHaveBeenCalledTimes(1);
    });

    it("Tab ходит по кругу внутри диалога", async () => {
      pendingOne();
      const user = userEvent.setup();
      renderWithClient(
        <>
          <button type="button">Позади диалога</button>
          <ApprovalDialog />
        </>,
      );
      await screen.findByRole("dialog");
      await user.tab();
      expect(screen.getByRole("button", { name: "Разрешить" })).toHaveFocus();
      await user.tab();
      expect(screen.getByRole("button", { name: "Отклонить" })).toHaveFocus();
      await user.tab();
      expect(screen.getByRole("button", { name: "Разрешить" })).toHaveFocus();
      await user.tab({ shift: true });
      expect(screen.getByRole("button", { name: "Отклонить" })).toHaveFocus();
    });

    it("после ответа фокус возвращается туда, где был до диалога", async () => {
      pendingOne();
      vi.mocked(api.answerMcpApproval).mockResolvedValue(undefined);
      const user = userEvent.setup();
      renderWithClient(
        <>
          <button type="button">Страница</button>
          <ApprovalDialog />
        </>,
      );
      const page = screen.getByRole("button", { name: "Страница" });
      act(() => page.focus());
      await screen.findByRole("dialog");
      expect(page).not.toHaveFocus();
      await user.click(screen.getByRole("button", { name: "Разрешить" }));
      await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
      expect(page).toHaveFocus();
    });
  });
});
