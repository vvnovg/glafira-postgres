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
const later: McpAuditEntry = { ts: 1_700_000_000_400, event: "futureEvent" };

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
    expect(rows[3]).toHaveTextContent("futureEvent");
    // Без обработчика раздела кнопки «Открыть в консоли» нет, и список подключений не нужен.
    expect(screen.queryByRole("button", { name: "Открыть в консоли EXPLAIN" })).not.toBeInTheDocument();
    expect(api.listConnections).not.toHaveBeenCalled();
  });

  it("события P4b: запрос передан LLM и ответ на предложение — подписями, а не именами из файла", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([
      { ts: 1_700_000_000_500, event: "requestCreated", connection: { id: "c1", name: "perf" } },
      { ts: 1_700_000_000_600, event: "proposalAnswered", connection: { id: "c1", name: "perf" } },
    ]);
    renderWithClient(<McpAudit status={mcpStatus()} />);
    const table = await screen.findByRole("table", { name: "Журнал" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Запрос передан LLM");
    expect(rows[1]).toHaveTextContent("Ответ на предложение");
    expect(table).not.toHaveTextContent("requestCreated");
    expect(table).not.toHaveTextContent("proposalAnswered");
  });

  it("записи фонового ассистента: начало и конец запуска — подписями, а не именами из файла", async () => {
    vi.mocked(api.getMcpAudit).mockResolvedValue([
      { ts: 1_700_000_000_500, event: "agentRunStarted", connection: { id: "c1", name: "perf" } },
      { ts: 1_700_000_000_600, event: "agentRunFinished", runId: "a1", requestId: "r1", connection: { id: "c1", name: "perf" }, status: "done", calls: 7, proposals: 2, ms: 83_000 },
      { ts: 1_700_000_000_700, event: "agentRunFinished", runId: "a2", requestId: "r2", connection: { id: "c1", name: "perf" }, status: "failed", reason: "timeout", calls: 3, proposals: 0, ms: 15 * 60_000 + 3_000 },
      { ts: 1_700_000_000_800, event: "agentRunFinished", runId: "a3", requestId: null, connection: null, status: "done", calls: 1, proposals: 0, ms: 23_000 },
      { ts: 1_700_000_000_900, event: "agentRunFinished", runId: "a4", requestId: "r4", connection: null, status: "exploded", calls: 0, proposals: 0, ms: 0 },
    ]);
    renderWithClient(<McpAudit status={mcpStatus()} />);
    const table = await screen.findByRole("table", { name: "Журнал" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("Фоновый запуск начат");
    expect(rows[0]).toHaveTextContent("perf");
    expect(rows[1]).toHaveTextContent("Фоновый запуск завершён");
    // Итог запуска — тем же текстом, что в «Запусках»; статус, которого версия не знает, — как в файле.
    expect(rows[1]).toHaveTextContent("предложений: 2");
    expect(rows[2]).toHaveTextContent("ошибка: превышен срок 15 мин");
    expect(rows[3]).toHaveTextContent("проверка пройдена");
    expect(rows[4]).toHaveTextContent("exploded");
    expect(rows[0]).not.toHaveTextContent("успешно");
    expect(table).not.toHaveTextContent("agentRunStarted");
    expect(table).not.toHaveTextContent("agentRunFinished");
  });

  it("конец фонового запуска со стоимостью — в «Подробностях»: оценка «≈ …» с подсказкой о дне цен, точная — без «≈»", async () => {
    const finished = (runId: string, extra: Partial<McpAuditEntry>): McpAuditEntry => ({
      ts: 1_700_000_000_600,
      event: "agentRunFinished",
      runId,
      requestId: "r1",
      connection: { id: "c1", name: "perf" },
      status: "done",
      calls: 7,
      proposals: 1,
      ms: 40_000,
      ...extra,
    });
    vi.mocked(api.getMcpAudit).mockResolvedValue([
      finished("a1", { costUsd: 0.0148, costEstimated: true, pricesDate: "2026-11-07", offPeak: false }),
      finished("a2", { costUsd: 0.0061, costEstimated: true, pricesDate: "2026-11-07", offPeak: true }),
      finished("a3", { costUsd: 0.42 }),
      finished("a4", {}),
      // Запись сборки до итоговой волны: оценка без дня цен.
      finished("a5", { costUsd: 0.0123, costEstimated: true }),
      // Поле стоимости у вызова инструмента ничего не значит.
      { ...activity, costUsd: 1 },
    ]);
    renderWithClient(<McpAudit status={mcpStatus()} />);
    const table = await screen.findByRole("table", { name: "Журнал" });
    const details = (i: number) => within(within(table).getAllByRole("row")[i + 1]).getAllByRole("cell")[6];
    expect(details(0)).toHaveTextContent(/^≈ 0,01 \$$/);
    expect(details(0).querySelector("[title]")?.getAttribute("title")).toBe("Оценка по ценам Ollama Cloud на 07.11.2026 (ollama.com/pricing)");
    expect(details(1)).toHaveTextContent(/^≈ 0,006 \$$/);
    expect(details(1).querySelector("[title]")?.getAttribute("title")).toBe("Оценка по ценам Ollama Cloud вне пика на 07.11.2026 (ollama.com/pricing)");
    expect(details(2)).toHaveTextContent(/^0,42 \$$/);
    expect(details(2)).not.toHaveTextContent("≈");
    expect(details(2).querySelector("[title]")).toBeNull();
    expect(details(3)).toHaveTextContent(/^$/);
    expect(details(4)).toHaveTextContent(/^≈ 0,01 \$$/);
    expect(details(4).querySelector("[title]")?.getAttribute("title")).toBe("Оценка по ценам Ollama Cloud (ollama.com/pricing)");
    expect(details(5)).toHaveTextContent(/^замаскировано: 3$/);
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
