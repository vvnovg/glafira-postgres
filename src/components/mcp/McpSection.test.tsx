import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useWorkspace } from "../../state/workspace";
import { connection } from "../../test/fixtures";
import { agentSettings, agentStatus, explainCall, MCP_CONFIGS, mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      getMcpStatus: vi.fn(),
      setMcpEnabled: vi.fn(),
      revokeMcpClient: vi.fn(),
      disconnectMcpSessions: vi.fn(),
      getMcpAudit: vi.fn(),
      getMcpClientConfigs: vi.fn(),
      revealMcpAudit: vi.fn(),
      listConnections: vi.fn(),
      getAgentSettings: vi.fn(),
      setAgentSettings: vi.fn(),
      detectAgentCli: vi.fn(),
      listAgentModels: vi.fn(),
      checkAgent: vi.fn(),
      unloadAgentModel: vi.fn(),
      listAgentRuns: vi.fn(),
      stopAgentRun: vi.fn(),
      readAgentLog: vi.fn(),
      revealAgentLog: vi.fn(),
      getAppInfo: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { McpSection } from "./McpSection";

const toggleName = "Разрешить MCP-сервер";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.getMcpClientConfigs).mockResolvedValue(MCP_CONFIGS);
  vi.mocked(api.getMcpAudit).mockResolvedValue([]);
  vi.mocked(api.listConnections).mockResolvedValue([]);
  vi.mocked(api.getAgentSettings).mockResolvedValue(agentSettings());
  vi.mocked(api.listAgentModels).mockResolvedValue({ ids: [], default: null, source: "none" });
  vi.mocked(api.listAgentRuns).mockResolvedValue([]);
  vi.mocked(api.getAppInfo).mockResolvedValue({ name: "Glafira for PostgreSQL", version: "0.6.0", commit: "abc1234", os: "macos", arch: "aarch64", dataDir: "/d", logDir: "/l" });
});

describe("McpSection", () => {
  it("выключатель запускает сервер; статус и фрагменты клиентов с копированием", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ server: "stopped" }));
    vi.mocked(api.setMcpEnabled).mockResolvedValue(mcpStatus({ server: "running" }));
    const user = userEvent.setup();
    renderWithClient(<McpSection />);
    const toggle = await screen.findByRole("checkbox", { name: toggleName });
    expect(toggle).not.toBeChecked();
    expect(screen.getByText("Сервер выключен")).toBeInTheDocument();
    await user.click(toggle);
    expect(api.setMcpEnabled).toHaveBeenCalledWith(true);
    await waitFor(() => expect(toggle).toBeChecked());
    expect(screen.getByText("Сервер работает")).toBeInTheDocument();
    expect(await screen.findByText(MCP_CONFIGS.qwenCode)).toBeInTheDocument();
    expect(screen.getByText("Qwen Code: команда")).toBeInTheDocument();
    expect(screen.getByText("Qwen Code: файл ~/.qwen/settings.json")).toBeInTheDocument();
    expect(screen.getByText(MCP_CONFIGS.claudeCode)).toBeInTheDocument();
    expect(screen.getByText("Claude Desktop: файл claude_desktop_config.json")).toBeInTheDocument();
    expect(screen.getByText("Cursor: файл ~/.cursor/mcp.json")).toBeInTheDocument();
    // Любой stdio-клиент: команда и аргументы — JSON.
    const stdio = JSON.stringify({ command: MCP_CONFIGS.command, args: ["mcp"] }, null, 2);
    expect(screen.getByText((_, el) => el?.tagName === "PRE" && el.textContent === stdio)).toBeInTheDocument();
    // Буфер обмена — заглушкой, как в TestPanel.test (после userEvent.setup, который ставит свою).
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.click(screen.getByRole("button", { name: "Копировать: Qwen Code: команда" }));
    expect(writeText).toHaveBeenCalledWith(MCP_CONFIGS.qwenCode);
    expect(await screen.findByText("Скопировано")).toBeInTheDocument();
  });

  it("политика запрещает MCP: выключатель неактивен с пометкой, фрагментов нет", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ server: "stopped", policyEnabled: false }));
    renderWithClient(<McpSection />);
    const toggle = await screen.findByRole("checkbox", { name: toggleName });
    expect(toggle).toBeDisabled();
    expect(toggle).not.toBeChecked();
    expect(screen.getByText("Отключено вашей организацией")).toBeInTheDocument();
    expect(screen.queryByText("Подключить клиент")).not.toBeInTheDocument();
    expect(api.getMcpClientConfigs).not.toHaveBeenCalled();
  });

  it("счётчики подключений, потолок политики, другой экземпляр, ошибка сервера", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ openConnections: 3 }));
    const first = renderWithClient(<McpSection />);
    expect(await screen.findByText("Открыто для LLM: 3")).toBeInTheDocument();
    expect(screen.queryByText("Доступ к данным ограничен вашей организацией")).not.toBeInTheDocument();
    first.unmount();

    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({ openConnections: 3, dataConnections: 1, maxAccess: "diagnostics", server: "otherInstance" }),
    );
    const second = renderWithClient(<McpSection />);
    expect(await screen.findByText("Открыто для LLM: 3, из них с данными: 1")).toBeInTheDocument();
    expect(screen.getByText("Доступ к данным ограничен вашей организацией")).toBeInTheDocument();
    expect(screen.getByText("Сервер работает в другом окне Glafira")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: toggleName })).toBeChecked();
    second.unmount();

    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ server: "error", error: "Address already in use" }));
    renderWithClient(<McpSection />);
    expect(await screen.findByText("Ошибка сервера: Address already in use")).toBeInTheDocument();
  });

  it("сессии: ожидающая помечена; «Отключить» одну и «Отключить всех», затем статус перечитывается", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({
        sessions: [
          { id: "s1", clientName: "qwen-code", clientVersion: "0.26.7", startedMs: 1_700_000_000_000, calls: 7, active: true },
          { id: "s2", clientName: null, clientVersion: null, startedMs: 1_700_000_100_000, calls: 0, active: false },
        ],
        pending: [{ sessionId: "s2", clientName: null, clientVersion: null }],
      }),
    );
    vi.mocked(api.disconnectMcpSessions).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderWithClient(<McpSection />);
    const table = await screen.findByRole("table", { name: "Активные сессии" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("qwen-code 0.26.7");
    expect(rows[0]).toHaveTextContent("7");
    expect(rows[1]).toHaveTextContent("Неизвестный MCP-клиент · ждёт подтверждения");
    await user.click(screen.getByRole("button", { name: "Отключить сессию клиента qwen-code 0.26.7" }));
    expect(api.disconnectMcpSessions).toHaveBeenLastCalledWith("s1");
    await waitFor(() => expect(api.getMcpStatus).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Отключить всех" }));
    expect(api.disconnectMcpSessions).toHaveBeenLastCalledWith(undefined);
  });

  it("сессии фонового ассистента: «Фоновый ассистент · запрос r…», «… · проверка»; запуск кончился — без запроса", async () => {
    const runSession = { id: "s3", clientName: "qwen-cli-mcp-client-glafira-pg", clientVersion: "0.26.5", startedMs: 1_700_000_000_000, calls: 4, active: true, runId: "a1b2c3d" };
    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({ sessions: [runSession], agent: agentStatus({ enabled: true, configured: true, running: { runId: "a1b2c3d", requestId: "r3a9f1c" } }) }),
    );
    const first = renderWithClient(<McpSection />);
    const table = await screen.findByRole("table", { name: "Активные сессии" });
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent("Фоновый ассистент · запрос r3a9f1c");
    expect(within(table).getByRole("button", { name: "Отключить сессию клиента Фоновый ассистент · запрос r3a9f1c" })).toBeInTheDocument();
    first.unmount();
    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({ sessions: [{ ...runSession, runId: "a0c0c0c" }], agent: agentStatus({ running: { runId: "a0c0c0c", requestId: null } }) }),
    );
    const second = renderWithClient(<McpSection />);
    expect(await screen.findByText("Фоновый ассистент · проверка")).toBeInTheDocument();
    second.unmount();
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ sessions: [runSession] }));
    renderWithClient(<McpSection />);
    const rows = within(await screen.findByRole("table", { name: "Активные сессии" })).getAllByRole("row");
    expect(rows[1]).toHaveTextContent(/^Фоновый ассистент\d/);
  });

  it("подраздел «Фоновый ассистент» — после «Подключить клиент», до «Активных сессий»", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    renderWithClient(<McpSection />);
    const connect = await screen.findByRole("heading", { level: 4, name: "Подключить клиент" });
    const agent = await screen.findByRole("region", { name: "Фоновый ассистент" });
    const sessions = screen.getByRole("heading", { level: 4, name: "Активные сессии" });
    expect(connect.compareDocumentPosition(agent) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(agent.compareDocumentPosition(sessions) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(await within(agent).findByRole("checkbox", { name: "Запускать ассистента в фоне" })).toBeInTheDocument();
  });

  it("политика запрещает MCP: подраздел «Фоновый ассистент» виден, его выключатель неактивен с пометкой", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ server: "stopped", policyEnabled: false, agent: agentStatus({ policyEnabled: false }) }));
    renderWithClient(<McpSection />);
    const agent = await screen.findByRole("region", { name: "Фоновый ассистент" });
    expect(await within(agent).findByRole("checkbox", { name: "Запускать ассистента в фоне" })).toBeDisabled();
    expect(within(agent).getByText("Отключено вашей организацией")).toBeInTheDocument();
  });

  it("бэкенд без статуса фонового ассистента (до P4c) — раздел без подраздела, а не ошибка", async () => {
    const { agent: _agent, ...old } = mcpStatus();
    vi.mocked(api.getMcpStatus).mockResolvedValue(old as never);
    renderWithClient(<McpSection />);
    expect(await screen.findByRole("heading", { level: 4, name: "Активные сессии" })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Фоновый ассистент" })).toBeNull();
    expect(api.getAgentSettings).not.toHaveBeenCalled();
  });

  it("подтверждённые клиенты: «Отозвать»", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(
      mcpStatus({ approvedClients: [{ name: "qwen-code", version: "0.26.7", firstSeenMs: 1_700_000_000_000, lastSeenMs: 1_700_000_500_000 }] }),
    );
    vi.mocked(api.revokeMcpClient).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderWithClient(<McpSection />);
    const table = await screen.findByRole("table", { name: "Подтверждённые клиенты" });
    expect(table).toHaveTextContent("qwen-code");
    expect(table).toHaveTextContent("0.26.7");
    await user.click(within(table).getByRole("button", { name: "Отозвать разрешение клиента qwen-code" }));
    expect(api.revokeMcpClient).toHaveBeenCalledWith("qwen-code");
  });

  it("статус не загрузился — ошибка с «Повторить»", async () => {
    vi.mocked(api.getMcpStatus).mockRejectedValueOnce({ code: "INTERNAL", message: "boom", retryable: false }).mockResolvedValue(mcpStatus());
    const user = userEvent.setup();
    renderWithClient(<McpSection />);
    expect(await screen.findByRole("alert")).toHaveTextContent("boom");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByRole("checkbox", { name: toggleName })).toBeChecked();
  });
  it("«Открыть в консоли EXPLAIN» из журнала открывает консоль подключения с подстановкой", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c1", name: "perf", llmAccess: "diagnostics" })]);
    vi.mocked(api.getMcpAudit).mockResolvedValue([explainCall()]);
    useWorkspace.setState({ tabs: [], activeId: null });
    const user = userEvent.setup();
    renderWithClient(<McpSection />);
    const button = await screen.findByRole("button", { name: "Открыть в консоли EXPLAIN" });
    await waitFor(() => expect(button).toBeEnabled());
    await user.click(button);
    const { tabs, activeId } = useWorkspace.getState();
    expect(activeId).toBe("explain:c1");
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({
      kind: "explain",
      connectionId: "c1",
      prefill: { sql: "SELECT * FROM shop.orders WHERE status = 'new'", database: "perf", member: "pg-2", nonce: expect.any(Number) },
    });
  });
});
