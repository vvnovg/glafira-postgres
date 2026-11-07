import { act, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpStatus } from "../../api/types";
import { MCP_STATUS_KEY, useMcpEvents } from "../../lib/useMcpStatus";
import { useWorkspace } from "../../state/workspace";
import { agentStatus, mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";

let emit: ((e: { payload: McpStatus }) => void) | undefined;
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, cb: (e: { payload: McpStatus }) => void) => {
    if (name === "mcp://state") emit = cb;
    return () => {};
  }),
}));
vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getMcpStatus: vi.fn() } };
});

import { api } from "../../api/client";
import { LlmIndicator } from "./LlmIndicator";

function WithEvents() {
  useMcpEvents();
  return <LlmIndicator />;
}

const session = { id: "s1", clientName: "qwen-code", clientVersion: "0.26.7", startedMs: 0, calls: 0, active: false };

beforeEach(() => {
  vi.clearAllMocks();
  emit = undefined;
  useWorkspace.setState({ tabs: [], activeId: null });
});

describe("LlmIndicator", () => {
  it("появляется с сессией, подсвечивается во время вызова, щелчок открывает раздел MCP в «Настройках»", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    const user = userEvent.setup();
    const { client } = renderWithClient(<WithEvents />);
    // Сначала первый ответ get_mcp_status, затем события.
    await waitFor(() => expect(client.getQueryData(MCP_STATUS_KEY)).toBeDefined());
    await waitFor(() => expect(emit).toBeDefined());
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    act(() => emit!({ payload: mcpStatus({ sessions: [session] }) }));
    const button = await screen.findByRole("button", { name: "LLM: qwen-code" });
    expect(button).not.toHaveClass("active");
    // Две сессии одного клиента — один клиент; клиентов два — число.
    act(() =>
      emit!({
        payload: mcpStatus({ sessions: [{ ...session, active: true }, { ...session, id: "s2", clientName: "claude-code" }, { ...session, id: "s3" }] }),
      }),
    );
    expect(await screen.findByRole("button", { name: "LLM: 2 клиента" })).toHaveClass("active");
    await user.click(screen.getByRole("button"));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "settings", section: "mcp", id: "settings" }]);
    act(() => emit!({ payload: mcpStatus({ sessions: [] }) }));
    await waitFor(() => expect(screen.queryByRole("button")).not.toBeInTheDocument());
  });

  it("во время фонового запуска — «LLM: фоновый ассистент», подсветка во время вызова; щелчок открывает его подраздел", async () => {
    const run = { ...session, id: "s9", clientName: "qwen-cli-mcp-client-glafira-pg", runId: "a1b2c3d", active: true };
    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({ sessions: [run], agent: agentStatus({ enabled: true, configured: true, running: { runId: "a1b2c3d", requestId: "r3a9f1c" } }) }),
    );
    const user = userEvent.setup();
    renderWithClient(<LlmIndicator />);
    const button = await screen.findByRole("button", { name: "LLM: фоновый ассистент" });
    expect(button).toHaveClass("active");
    expect(button).toHaveAttribute("title", "Работает фоновый ассистент: открыть его подраздел в «Настройках»");
    await user.click(button);
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "settings", section: "agent", id: "settings" }]);
  });

  it("запуск ещё не подключился — индикатор уже виден; сессия запуска — не клиент чата", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ agent: agentStatus({ running: { runId: "a1b2c3d", requestId: "r3a9f1c" } }) }));
    const first = renderWithClient(<LlmIndicator />);
    expect(await screen.findByRole("button", { name: "LLM: фоновый ассистент" })).not.toHaveClass("active");
    first.unmount();
    // Запуск кончился, а его сессия ещё в списке: клиент чата — один.
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ sessions: [{ ...session, id: "s9", clientName: "claude-code", runId: "a1b2c3d" }, session] }));
    const second = renderWithClient(<LlmIndicator />);
    expect(await screen.findByRole("button", { name: "LLM: qwen-code" })).toBeInTheDocument();
    second.unmount();
    // Осталась только сессия кончившегося запуска — индикатора нет.
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ sessions: [{ ...session, id: "s9", clientName: "claude-code", runId: "a1b2c3d" }] }));
    renderWithClient(<LlmIndicator />);
    await waitFor(() => expect(api.getMcpStatus).toHaveBeenCalledTimes(3));
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("клиент без имени — «Неизвестный MCP-клиент»", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ sessions: [{ ...session, clientName: null, clientVersion: null }] }));
    renderWithClient(<LlmIndicator />);
    expect(await screen.findByRole("button", { name: "LLM: Неизвестный MCP-клиент" })).toBeInTheDocument();
  });
});
