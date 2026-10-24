import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import type { Finding, PulseNode } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { diagReport, pulseFacts, pulseRates } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { PULSE_COLUMNS, PulseView } from "./PulseView";

const finding = (id: string, severity: Finding["severity"], member = "pg-1"): Finding => ({ id, severity, member, target: null, params: {}, links: [] });
const node = (patch: Partial<PulseNode> = {}): PulseNode => ({ member: "pg-1", role: "primary", reachable: true, error: null, facts: pulseFacts(), rates: pulseRates(), ...patch });

describe("PulseView", () => {
  beforeEach(() => useWorkspace.setState({ tabs: [], activeId: null }));

  it("строка на узел, клетка окрашена находкой, клик открывает инструмент на узле", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    const report = diagReport(
      { kind: "pulse", nodes: [
        { member: "pg-1", role: "primary", reachable: true, error: null, facts: pulseFacts({ used: 96, available: 100, longestIdleInTransactionS: 400 }), rates: pulseRates({ tps: 1234.5 }) },
        { member: "pg-2", role: "replica", reachable: false, error: { code: "NODE_UNAVAILABLE", key: "errors.topology.noNode", message: "x", retryable: true }, facts: null, rates: null },
      ] },
      { member: null, followsLeader: false, findings: [{ id: "connectionsSaturated", severity: "critical", member: "pg-1", target: null, params: { usedPct: 96, used: 96, available: 100 }, links: [] }] },
    );
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-1/ });
    const cell = within(row).getByRole("button", { name: /96 \/ 100/ });
    expect(cell.closest("td")).toHaveClass("sev-critical");
    expect(within(row).getByText(/1\s234,5/)).toBeInTheDocument(); // \s — и неразрывный пробел разрядов
    await user.click(cell);
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", tool: "sessions", member: "pg-1" });
    expect(within(screen.getByRole("row", { name: /pg-2/ })).getByText("Узел недоступен")).toBeInTheDocument();
  });

  it("клетка берёт худшую строгость находок своей колонки у своего узла; чужой узел и чужая колонка её не красят", () => {
    const report = diagReport(
      {
        kind: "pulse",
        nodes: [
          node({ facts: pulseFacts({ used: 10, available: 100, longestQueryS: 700, longestIdleInTransactionS: 400 }) }),
          node({ member: "pg-2", role: "replica", facts: pulseFacts({ used: 10, available: 100 }) }),
        ],
      },
      { findings: [finding("longQuery", "warning"), finding("longQuery", "critical"), finding("connectionsSaturated", "critical", "pg-2"), finding("tempFiles", "info")] },
    );
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const first = screen.getByRole("row", { name: /pg-1/ });
    const cellOf = (row: HTMLElement, name: RegExp) => within(row).getByRole("button", { name }).closest("td");
    expect(cellOf(first, /11 мин 40 с/)).toHaveClass("sev-critical"); // longQuery: из двух находок худшая
    expect(cellOf(first, /10 \/ 100/)).not.toHaveClass("sev-critical"); // connectionsSaturated — у pg-2
    expect(cellOf(first, /6 мин 40 с/)).not.toHaveClass("sev-critical"); // idleInTransaction без находки
    // Цвет не единственный признак: слово строгости доступно читалке экрана.
    expect(within(first).getByRole("button", { name: /Критично/ })).toBeInTheDocument();
    const second = screen.getByRole("row", { name: /pg-2/ });
    expect(cellOf(second, /10 \/ 100/)).toHaveClass("sev-critical");
    expect(cellOf(second, /10 \/ 100/)).not.toHaveClass("sev-warning");
  });

  it("находка tempFiles красит колонку pg_wal, deadlocks — колонку TPS", () => {
    const report = diagReport(
      { kind: "pulse", nodes: [node({ facts: pulseFacts({ walDirBytes: 3 * 1024 ** 3 }), rates: pulseRates({ tps: 10, commitsPerS: 9, rollbacksPerS: 1 }) })] },
      { findings: [finding("tempFiles", "info"), finding("deadlocks", "warning")] },
    );
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-1/ });
    expect(within(row).getByRole("button", { name: /3,0 GB/ }).closest("td")).toHaveClass("sev-info");
    const tps = within(row).getAllByRole("button")[PULSE_COLUMNS.findIndex((c) => c.key === "tps")];
    expect(tps.closest("td")).toHaveClass("sev-warning");
    // Состав TPS — в подсказке.
    expect(tps).toHaveAttribute("title", "коммиты: 9,0/с, откаты: 1,0/с");
  });

  it("первый вызов: скорости ещё не известны — «…», а не нули; без находок клетка не окрашена", () => {
    const report = diagReport({ kind: "pulse", nodes: [node({ facts: pulseFacts({ used: 3, available: 100 }), rates: pulseRates() })] });
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-1/ });
    // TPS, попадания в кэш и WAL в секунду.
    expect(within(row).getAllByText("…")).toHaveLength(3);
    for (const td of within(row).getAllByRole("cell")) expect(td.className).not.toMatch(/sev-/);
  });

  it("известные скорости: WAL в секунду, кэш; пустая доля кэша без чтений — «—»", () => {
    const report = diagReport({
      kind: "pulse",
      nodes: [node({ rates: pulseRates({ tps: 5, commitsPerS: 5, rollbacksPerS: 0, walBytesPerS: 2 * 1024 ** 2, cacheHitPct: 99.5 }) }), node({ member: "pg-2", role: "replica", rates: pulseRates({ tps: 0, commitsPerS: 0, rollbacksPerS: 0 }) })],
    });
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const first = screen.getByRole("row", { name: /pg-1/ });
    expect(within(first).getByText("2,0 MB/с")).toBeInTheDocument();
    expect(within(first).getByText("99,5 %")).toBeInTheDocument();
    const second = screen.getByRole("row", { name: /pg-2/ });
    expect(within(second).queryByText("…")).not.toBeInTheDocument();
  });

  it("долгие значения, блокировки, возраст XID, pg_wal и лаг реплики", () => {
    const report = diagReport({
      kind: "pulse",
      nodes: [
        node({
          role: "replica",
          facts: pulseFacts({
            longestTransactionS: 3700,
            lockWaiting: 2,
            longestLockWaitS: 40,
            maxXidAge: 1_500_000,
            xidAgePct: 0.07,
            walDirBytes: 512 * 1024 ** 2,
            maxWalSizeBytes: 1024 ** 3,
            lagBytes: 2 * 1024 ** 2,
            replayDelayMs: 2500,
          }),
        }),
      ],
    });
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-1/ });
    expect(within(row).getByText("1 ч 1 мин")).toBeInTheDocument();
    expect(within(row).getByText("2 · 40 с")).toBeInTheDocument();
    expect(within(row).getByText(/^1\s500\s000 \(0,1 %\)$/)).toBeInTheDocument();
    expect(within(row).getByText("512,0 MB")).toBeInTheDocument();
    expect(within(row).getByText("2,0 MB · 2 с")).toBeInTheDocument();
  });

  it("каждая колонка ведёт в свой инструмент (клик по клетке открывает его на узле)", async () => {
    const user = userEvent.setup();
    const report = diagReport({ kind: "pulse", nodes: [node({ member: "pg-7", facts: pulseFacts({ used: 1, available: 10 }) })] });
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-7/ });
    const buttons = within(row).getAllByRole("button");
    expect(buttons).toHaveLength(PULSE_COLUMNS.length);
    for (const [i, col] of PULSE_COLUMNS.entries()) {
      await user.click(buttons[i]);
      expect(useWorkspace.getState().tabs.find((t) => t.kind === "diag" && t.tool === col.tool)).toMatchObject({ kind: "diag", connectionId: "c", member: "pg-7" });
    }
    expect(PULSE_COLUMNS.map((c) => [c.key, c.tool])).toEqual([
      ["connections", "sessions"],
      ["tps", "locks"],
      ["cache", "sessions"],
      ["wal", "replication"],
      ["xact", "sessions"],
      ["query", "sessions"],
      ["idle", "idleTx"],
      ["locks", "locks"],
      ["xid", "horizon"],
      ["walDir", "slots"],
      ["lag", "replication"],
    ]);
  });

  it("недоступный узел: причина — баннером в строке; доступный узел без находок не помечен", () => {
    const report = diagReport({
      kind: "pulse",
      nodes: [node({ member: "pg-2", role: "replica", reachable: false, facts: null, rates: null, error: { code: "NODE_UNAVAILABLE", key: "errors.topology.noNode", message: "x", retryable: true } })],
    });
    renderWithClient(<PulseView connectionId="c" report={report} />);
    const row = screen.getByRole("row", { name: /pg-2/ });
    expect(within(row).getByText("Узел недоступен")).toBeInTheDocument();
    expect(within(row).getByRole("alert")).toHaveTextContent("Нет подходящего доступного узла");
    expect(within(row).queryByRole("button")).not.toBeInTheDocument();
  });
});
