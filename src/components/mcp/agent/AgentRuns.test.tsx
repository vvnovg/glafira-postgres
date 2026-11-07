import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { connection } from "../../../test/fixtures";
import { agentRun } from "../../../test/llm";
import { renderWithClient } from "../../../test/render";

vi.mock("../../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../../api/client")>();
  return { ...orig, api: { listConnections: vi.fn(), stopAgentRun: vi.fn(), listAgentRuns: vi.fn() } };
});

import { api } from "../../../api/client";
import { AgentRuns, RUNS_SHOWN } from "./AgentRuns";

const onShowLog = vi.fn();

describe("AgentRuns", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c", name: "perf" })]);
    vi.mocked(api.listAgentRuns).mockResolvedValue([]);
  });

  it("строки: время, подключение, запрос, агент и модель, итог, длительность, вызовы, стоимость; действия по строке", async () => {
    vi.mocked(api.stopAgentRun).mockResolvedValue(agentRun({ id: "a0000a1", status: "cancelled" }));
    const runs = [
      agentRun({ id: "a0000a1", status: "running", startedAtMs: 1, calls: 3, queuedAtMs: 6 }),
      agentRun({ id: "a0000a2", preset: "claude", model: null, status: "done", proposalIds: ["p1", "p2"], startedAtMs: 0, endedAtMs: 83_000, calls: 12, costUsd: 0.42, queuedAtMs: 5 }),
      agentRun({ id: "a0000a3", status: "done", summary: "План уже хороший: индекс по status есть", startedAtMs: 0, endedAtMs: 30_000, queuedAtMs: 4 }),
      agentRun({ id: "a0000a4", status: "failed", reason: "timeout", startedAtMs: 0, endedAtMs: 15 * 60_000 + 3_000, queuedAtMs: 3 }),
      agentRun({ id: "a0000a5", kind: "check", requestId: null, connectionId: null, status: "done", startedAtMs: 0, endedAtMs: 23_000, queuedAtMs: 2 }),
      agentRun({ id: "a0000a6", status: "interrupted", queuedAtMs: 1 }),
      agentRun({ id: "a0000a7", connectionId: "gone", status: "queued", queuedAtMs: 0 }),
    ];
    const user = userEvent.setup();
    renderWithClient(<AgentRuns runs={runs} onShowLog={onShowLog} />);
    const table = screen.getByRole("table", { name: "Запуски" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(await within(rows[0]).findByText("perf")).toBeInTheDocument();
    expect(rows[0]).toHaveTextContent("r3a9f1c");
    expect(rows[0]).toHaveTextContent("Qwen Code, qwen3.6-64k:35b-a3b");
    expect(rows[0]).toHaveTextContent("работает");
    expect(rows[1]).toHaveTextContent("Claude Code");
    expect(rows[1]).toHaveTextContent("предложений: 2");
    expect(rows[1]).toHaveTextContent("1 мин 23 с");
    expect(rows[1]).toHaveTextContent("12");
    expect(rows[1]).toHaveTextContent("$0,42");
    expect(rows[2]).toHaveTextContent("без предложений");
    expect(rows[3]).toHaveTextContent("ошибка: превышен срок 15 мин");
    expect(rows[4]).toHaveTextContent("Проверка");
    expect(rows[4]).toHaveTextContent("проверка пройдена");
    expect(rows[5]).toHaveTextContent("прерван");
    // Подключения уже нет — его id; ждущий запуск можно остановить, журнала у него ещё нет.
    expect(rows[6]).toHaveTextContent("gone");
    expect(within(rows[6]).getByRole("button", { name: "Остановить запуск a0000a7" })).toBeInTheDocument();
    expect(within(rows[6]).queryByRole("button", { name: "Показать журнал запуска a0000a7" })).toBeNull();
    // Прерванный в очереди не начинался: ни журнала, ни остановки.
    expect(within(rows[5]).queryByRole("button")).toBeNull();
    // Кончившийся не останавливается.
    expect(within(rows[1]).queryByRole("button", { name: /Остановить/ })).toBeNull();
    await user.click(within(table).getByRole("button", { name: "Остановить запуск a0000a1" }));
    expect(api.stopAgentRun).toHaveBeenCalledWith("a0000a1");
    await user.click(within(table).getByRole("button", { name: "Показать журнал запуска a0000a4" }));
    expect(onShowLog).toHaveBeenCalledWith("a0000a4");
    const toggle = within(table).getByRole("button", { name: "Ответ ассистента, запуск a0000a3" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(within(table).getByText("План уже хороший: индекс по status есть")).toBeInTheDocument();
    await user.click(toggle);
    expect(within(table).queryByText("План уже хороший: индекс по status есть")).toBeNull();
  });

  it("ошибка «Остановить» показывается под списком", async () => {
    vi.mocked(api.stopAgentRun).mockRejectedValue({ code: "NOT_FOUND", message: "Background run a0000a1 not found", retryable: false });
    const user = userEvent.setup();
    renderWithClient(<AgentRuns runs={[agentRun({ id: "a0000a1", status: "running", startedAtMs: 1 })]} onShowLog={onShowLog} />);
    await user.click(screen.getByRole("button", { name: "Остановить запуск a0000a1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Background run a0000a1 not found");
  });

  it(`показывает не больше ${RUNS_SHOWN} последних; без запусков — пояснение`, () => {
    const many = Array.from({ length: 25 }, (_, i) => agentRun({ id: `a${String(i).padStart(6, "0")}`, status: "done", startedAtMs: 0, endedAtMs: 1_000, queuedAtMs: 100 - i }));
    const first = renderWithClient(<AgentRuns runs={many} onShowLog={onShowLog} />);
    expect(within(screen.getByRole("table", { name: "Запуски" })).getAllByRole("row")).toHaveLength(RUNS_SHOWN + 1);
    first.unmount();
    renderWithClient(<AgentRuns runs={[]} onShowLog={onShowLog} />);
    expect(screen.getByText("Запусков пока нет")).toBeInTheDocument();
  });
});
