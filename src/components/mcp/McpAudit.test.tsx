import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpAuditEntry } from "../../api/types";
import { connection } from "../../test/fixtures";
import { explainCall, mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getMcpAudit: vi.fn(), revealMcpAudit: vi.fn(), listConnections: vi.fn() } };
});

import { api } from "../../api/client";
import { AUDIT_LIMIT, McpAudit } from "./McpAudit";

const activity: McpAuditEntry = {
  ts: 1_700_000_000_100,
  event: "call",
  session: "s1",
  client: "qwen-code",
  tool: "get_activity",
  connection: { id: "c1", name: "perf" },
  args: { connectionId: "c1" },
  outcome: "ok",
  code: null,
  bytes: 900,
  ms: 12,
  masked: 3,
};
const refused = explainCall({ ts: 1_700_000_000_200, outcome: "error", code: "forbidden_function", ms: 5 });
const approved: McpAuditEntry = { ts: 1_700_000_000_300, event: "approved", session: "s1", client: "qwen-code" };
const later: McpAuditEntry = { ts: 1_700_000_000_400, event: "requestCreated" };

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.listConnections).mockResolvedValue([
    connection({ id: "c1", name: "perf", llmAccess: "diagnostics" }),
    connection({ id: "c2", name: "closed", llmAccess: "off" }),
  ]);
});

describe("McpAudit", () => {
  it("строки: инструмент или событие, подключение, итог, мс, замаскировано", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([activity, refused, approved, later]);
    renderWithClient(<McpAudit status={mcpStatus()} />);
    const table = await screen.findByRole("table", { name: "Журнал" });
    expect(api.getMcpAudit).toHaveBeenCalledWith(AUDIT_LIMIT);
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows).toHaveLength(4);
    expect(rows[0]).toHaveTextContent("get_activity");
    expect(rows[0]).toHaveTextContent("perf");
    expect(rows[0]).toHaveTextContent("успешно");
    expect(rows[0]).toHaveTextContent("12");
    expect(rows[0]).toHaveTextContent("замаскировано: 3");
    expect(rows[1]).toHaveTextContent("explain_query");
    expect(rows[1]).toHaveTextContent("forbidden_function");
    expect(rows[2]).toHaveTextContent("Клиент подтверждён");
    // Событие, которого эта версия не знает, — как в файле.
    expect(rows[3]).toHaveTextContent("requestCreated");
    // Без обработчика раздела кнопки «Открыть в консоли» нет, и список подключений не нужен.
    expect(screen.queryByRole("button", { name: "Открыть в консоли EXPLAIN" })).not.toBeInTheDocument();
    expect(api.listConnections).not.toHaveBeenCalled();
  });

  it("пустой журнал, «Обновить» и «Показать файл»", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([]);
    vi.mocked(api.revealMcpAudit).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderWithClient(<McpAudit status={mcpStatus()} />);
    expect(await screen.findByText("Записей нет")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Обновить" }));
    await waitFor(() => expect(api.getMcpAudit).toHaveBeenCalledTimes(2));
    await user.click(screen.getByRole("button", { name: "Показать файл" }));
    expect(api.revealMcpAudit).toHaveBeenCalledTimes(1);
  });

  it("«Открыть в консоли EXPLAIN» — только у успешных explain_query; неактивная называет причину", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([
      explainCall(),
      explainCall({ connection: { id: "c2", name: "closed" } }),
      explainCall({ args: '{"connectionId":"c1","sql":"SELECT …' }),
      explainCall({ args: { connectionId: "c1", database: "perf", queryId: "-42" } }),
      explainCall({ connection: { id: "c9", name: "gone" } }),
      refused,
      activity,
    ]);
    const onOpen = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<McpAudit status={mcpStatus()} onOpenInConsole={onOpen} />);
    const buttons = await screen.findAllByRole("button", { name: "Открыть в консоли EXPLAIN" });
    expect(buttons).toHaveLength(5);
    await waitFor(() => expect(buttons[0]).toBeEnabled());
    expect(buttons[0]).not.toHaveAttribute("title");
    expect(buttons[1]).toBeDisabled();
    expect(buttons[1]).toHaveAttribute("title", "Недоступно: доступ для LLM у подключения выключен");
    expect(buttons[2]).toHaveAttribute("title", "Недоступно: аргументы вызова в журнале усечены");
    expect(buttons[3]).toHaveAttribute("title", "Недоступно: вызов задан queryId, текста запроса в журнале нет");
    expect(buttons[4]).toHaveAttribute("title", "Недоступно: подключения больше нет");
    await user.click(buttons[0]);
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ tool: "explain_query", connection: { id: "c1", name: "perf" } }));
  });

  it("список подключений не прочитан — кнопка неактивна с причиной", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([explainCall()]);
    vi.mocked(api.listConnections).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    renderWithClient(<McpAudit status={mcpStatus()} onOpenInConsole={vi.fn()} />);
    const button = await screen.findByRole("button", { name: "Открыть в консоли EXPLAIN" });
    await waitFor(() => expect(button).toHaveAttribute("title", "Недоступно: список подключений не прочитан"));
    expect(button).toBeDisabled();
  });
});
