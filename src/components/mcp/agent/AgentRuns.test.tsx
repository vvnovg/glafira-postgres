import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { connection } from "../../../test/fixtures";
import { i18n } from "../../../i18n";
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
      agentRun({ id: "a0000a3", status: "done", summary: "**План уже хороший**: индекс по `status` есть", startedAtMs: 0, endedAtMs: 30_000, queuedAtMs: 4 }),
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
    expect(rows[1]).toHaveTextContent("0,42 $");
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
    // Итог — без разметки модели (смоук P4c, R16).
    expect(within(table).getByText("План уже хороший: индекс по status есть")).toBeInTheDocument();
    await user.click(toggle);
    expect(within(table).queryByText("План уже хороший: индекс по status есть")).toBeNull();
  });

  it("ошибка «Остановить» показывается под списком; фокус потом никуда не переводится", async () => {
    vi.mocked(api.stopAgentRun).mockRejectedValue({ code: "NOT_FOUND", message: "Background run a0000a1 not found", retryable: false });
    const user = userEvent.setup();
    const run = agentRun({ id: "a0000a1", status: "running", startedAtMs: 1 });
    const { rerender } = renderWithClient(<AgentRuns runs={[run]} onShowLog={onShowLog} />);
    await user.click(screen.getByRole("button", { name: "Остановить запуск a0000a1" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Background run a0000a1 not found");
    // Фокус ушёл на <body> (щелчок по тексту), а запуск потом кончился сам: фокус не прыгает на его журнал.
    act(() => (document.activeElement as HTMLElement | null)?.blur());
    rerender(<AgentRuns runs={[{ ...run, status: "done", endedAtMs: 2 }]} onShowLog={onShowLog} />);
    expect(document.activeElement).toBe(document.body);
  });

  it("«Остановить» во время записи — aria-disabled (фокус остаётся); запуск остановлен — фокус на его журнал или на заголовок", async () => {
    let finish: () => void = () => {};
    vi.mocked(api.stopAgentRun).mockImplementation(() => new Promise((r) => (finish = () => r(agentRun({ id: "a0000a1", status: "cancelled" })))));
    const user = userEvent.setup();
    const running = agentRun({ id: "a0000a1", status: "running", startedAtMs: 1 });
    const queued = agentRun({ id: "a0000a2", status: "queued", queuedAtMs: 0 });
    const { rerender } = renderWithClient(<AgentRuns runs={[running, queued]} onShowLog={onShowLog} />);
    const stop = screen.getByRole("button", { name: "Остановить запуск a0000a1" });
    await user.click(stop);
    expect(stop).toHaveAttribute("aria-disabled", "true");
    expect(stop).toHaveFocus();
    await user.click(stop);
    expect(api.stopAgentRun).toHaveBeenCalledTimes(1);
    finish();
    await waitFor(() => expect(stop).not.toHaveAttribute("aria-disabled"));
    // Итог `cancelled` пришёл событием: кнопки нет, фокус — на «Показать журнал» того же запуска.
    rerender(<AgentRuns runs={[{ ...running, status: "cancelled", endedAtMs: 2 }, queued]} onShowLog={onShowLog} />);
    expect(screen.getByRole("button", { name: "Показать журнал запуска a0000a1" })).toHaveFocus();
    // У ждущего журнала нет — фокус на заголовок списка.
    vi.mocked(api.stopAgentRun).mockResolvedValue(agentRun({ id: "a0000a2", status: "cancelled" }));
    await user.click(screen.getByRole("button", { name: "Остановить запуск a0000a2" }));
    rerender(<AgentRuns runs={[{ ...running, status: "cancelled", endedAtMs: 2 }, { ...queued, status: "cancelled" }]} onShowLog={onShowLog} />);
    expect(screen.getByRole("heading", { level: 4, name: "Запуски" })).toHaveFocus();
  });

  it("список читается — «Загрузка…»; не прочитался — ошибка с «Повторить»; не «Запусков пока нет»", async () => {
    const onRetry = vi.fn();
    const user = userEvent.setup();
    const first = renderWithClient(<AgentRuns runs={[]} state="loading" onShowLog={onShowLog} />);
    expect(screen.getByText("Загрузка…")).toBeInTheDocument();
    expect(screen.queryByText("Запусков пока нет")).toBeNull();
    first.unmount();
    renderWithClient(<AgentRuns runs={[]} state="failed" error={{ code: "INTERNAL", message: "boom", retryable: false }} onRetry={onRetry} onShowLog={onShowLog} />);
    expect(screen.getByRole("alert")).toHaveTextContent("boom");
    expect(screen.queryByText("Запусков пока нет")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it(`показывает не больше ${RUNS_SHOWN} последних; без запусков — пояснение`, () => {
    const many = Array.from({ length: 25 }, (_, i) => agentRun({ id: `a${String(i).padStart(6, "0")}`, status: "done", startedAtMs: 0, endedAtMs: 1_000, queuedAtMs: 100 - i }));
    const first = renderWithClient(<AgentRuns runs={many} onShowLog={onShowLog} />);
    expect(within(screen.getByRole("table", { name: "Запуски" })).getAllByRole("row")).toHaveLength(RUNS_SHOWN + 1);
    first.unmount();
    renderWithClient(<AgentRuns runs={[]} onShowLog={onShowLog} />);
    expect(screen.getByText("Запусков пока нет")).toBeInTheDocument();
  });

  it("оценка по ценам Ollama Cloud — «≈ 0,01 $» с подсказкой о дне цен; точная стоимость Claude Code — без «≈» и подсказки", async () => {
    const runs = [
      agentRun({ id: "a0000e1", status: "done", startedAtMs: 0, endedAtMs: 1_000, costEstimate: { usd: 0.0148, pricesDate: "2026-11-07", offPeak: false } }),
      agentRun({ id: "a0000e2", preset: "claude", status: "done", startedAtMs: 0, endedAtMs: 1_000, costEstimate: { usd: 0.0061, pricesDate: "2026-12-01", offPeak: true } }),
      agentRun({ id: "a0000e3", preset: "claude", status: "done", startedAtMs: 0, endedAtMs: 1_000, costUsd: 0.42 }),
      agentRun({ id: "a0000e4", status: "done", startedAtMs: 0, endedAtMs: 1_000 }),
    ];
    const ru = renderWithClient(<AgentRuns runs={runs} onShowLog={onShowLog} />);
    const costCell = (i: number) => within(within(screen.getByRole("table")).getAllByRole("row")[i + 1]).getAllByRole("cell")[7];
    // Подсказка у «≈ …» — с клавиатуры и для скринридера (итоговое ревью): сам текст фокусируется, имя — текст и подсказка.
    const hintOf = (cell: HTMLElement) => within(cell).queryByRole("button");
    expect(costCell(0)).toHaveTextContent(/^≈ 0,01 \$$/);
    expect(hintOf(costCell(0))).toHaveAccessibleName(/^≈ 0,01\s\$: Оценка по ценам Ollama Cloud на 07\.11\.2026 \(ollama\.com\/pricing\)$/);
    act(() => hintOf(costCell(0))!.focus());
    expect(screen.getByRole("tooltip")).toHaveTextContent("Оценка по ценам Ollama Cloud на 07.11.2026 (ollama.com/pricing)");
    // Меньше цента — одна значащая цифра, а не «0,00 $»; вне пика — сказано в подсказке.
    expect(costCell(1)).toHaveTextContent(/^≈ 0,006 \$$/);
    expect(hintOf(costCell(1))).toHaveAccessibleName(/Оценка по ценам Ollama Cloud вне пика на 01\.12\.2026 \(ollama\.com\/pricing\)$/);
    expect(costCell(2)).toHaveTextContent(/^0,42 \$$/);
    expect(hintOf(costCell(2))).toBeNull();
    expect(costCell(3)).toHaveTextContent(/^$/);
    ru.unmount();
    try {
      await act(() => i18n.changeLanguage("en"));
      renderWithClient(<AgentRuns runs={runs} onShowLog={onShowLog} />);
      expect(costCell(0)).toHaveTextContent(/^≈ \$0\.01$/);
      expect(hintOf(costCell(0))).toHaveAccessibleName("≈ $0.01: Estimate based on Ollama Cloud prices as of 11/07/2026 (ollama.com/pricing)");
      expect(costCell(2)).toHaveTextContent(/^\$0\.42$/);
    } finally {
      await act(() => i18n.changeLanguage("ru"));
    }
  });

  it("стоимость — валютой по правилам языка: «0,42 $» в ru, «$0.42» в en", async () => {
    const runs = [agentRun({ id: "a0000a2", preset: "claude", status: "done", startedAtMs: 0, endedAtMs: 1_000, costUsd: 0.42 })];
    const ru = renderWithClient(<AgentRuns runs={runs} onShowLog={onShowLog} />);
    // Восьмой столбец — «Стоимость».
    const costCell = () => within(within(screen.getByRole("table")).getAllByRole("row")[1]).getAllByRole("cell")[7];
    // Между числом и знаком — неразрывный пробел (toHaveTextContent сводит его к обычному).
    expect(costCell()).toHaveTextContent(/^0,42 \$$/);
    expect(costCell().textContent).toBe("0,42\u00a0$");
    ru.unmount();
    try {
      await act(() => i18n.changeLanguage("en"));
      renderWithClient(<AgentRuns runs={runs} onShowLog={onShowLog} />);
      expect(costCell()).toHaveTextContent(/^\$0\.42$/);
    } finally {
      await act(() => i18n.changeLanguage("ru"));
    }
  });
});
