import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Finding } from "../../api/types";
import { renderWithClient } from "../../test/render";
import { useWorkspace } from "../../state/workspace";
import { FindingsPanel } from "./FindingsPanel";

describe("FindingsPanel", () => {
  it("находки по строгости с текстом, «что сделать», командой и переходом в инструмент", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    renderWithClient(
      <FindingsPanel
        connectionId="c"
        findings={[
          { id: "noIdleTimeout", severity: "info", member: "pg-1", target: null, params: { command: "ALTER ROLE \"app\" SET idle_in_transaction_session_timeout = '10min';" }, links: [] },
          { id: "idleInTransaction", severity: "critical", member: "pg-1", target: { kind: "session", pid: 42, backendStartUs: 1 }, params: { maxS: 2000, count: 3 }, links: [{ kind: "openTool", tool: "idleTx", member: "pg-1" }] },
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    // Сначала более строгие, независимо от порядка во входных данных.
    expect(items[0]).toHaveTextContent("Критично");
    expect(items[0]).toHaveTextContent("Простой в транзакции 33 мин 20 с");
    await user.click(within(items[0]).getByRole("button", { name: "Открыть «Idle in transaction»" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", tool: "idleTx", member: "pg-1" });
    expect(within(items[1]).getByRole("button", { name: "Скопировать команду" })).toBeInTheDocument();
  });

  it("внутри одной строгости порядок сервера сохраняется; сворачивание скрывает список", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    const base = { member: "pg-1", target: null, params: {}, links: [] };
    renderWithClient(
      <FindingsPanel
        connectionId="c"
        findings={[
          { ...base, id: "longQuery", severity: "warning", params: { maxS: 1000, count: 1 } },
          { ...base, id: "deadlocks", severity: "warning", params: { count: 2 } },
          { ...base, id: "cacheHitLow", severity: "info", params: { hitPct: 85 } },
        ]}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items.map((i) => i.textContent)).toEqual([
      expect.stringContaining("Запрос выполняется 16 мин 40 с"),
      expect.stringContaining("Взаимных блокировок за интервал: 2"),
      expect.stringContaining("Cache hit за интервал: 85 %"),
    ]);
    expect(items[0]).toHaveTextContent("Внимание");
    expect(items[2]).toHaveTextContent("Сведения");
    await user.click(screen.getByRole("button", { name: "Свернуть находки" }));
    expect(screen.queryAllByRole("listitem")).toHaveLength(0);
    // Имя кнопки следует за состоянием.
    const expand = screen.getByRole("button", { name: "Развернуть находки" });
    expect(expand).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: "Свернуть находки" })).not.toBeInTheDocument();
    await user.click(expand);
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
    expect(screen.getByRole("button", { name: "Свернуть находки" })).toHaveAttribute("aria-expanded", "true");
  });

  describe("устойчивые ключи находок", () => {
    const base = { severity: "warning" as const, links: [] };
    const query1: Finding = { ...base, id: "longQuery", member: "pg-1", target: { kind: "session", pid: 11, backendStartUs: 1 }, params: { maxS: 1000, count: 1 } };
    const query2: Finding = { ...base, id: "longQuery", member: "pg-1", target: { kind: "session", pid: 22, backendStartUs: 2 }, params: { maxS: 2000, count: 1 } };
    const other: Finding = { ...base, id: "longQuery", member: "pg-2", target: { kind: "session", pid: 11, backendStartUs: 1 }, params: { maxS: 3000, count: 1 } };
    const items = () => screen.getAllByRole("listitem");
    const item = (text: string) => items().find((li) => li.textContent?.includes(text))!;
    const detailsOf = (li: HTMLElement) => li.querySelector("details")!;

    it("раскрытое «Что сделать» остаётся раскрытым, когда те же находки приходят в другом порядке", async () => {
      const user = userEvent.setup();
      const { rerender } = renderWithClient(<FindingsPanel connectionId="c" findings={[query1, query2, other]} showMember />);
      await user.click(within(item("16 мин 40 с")).getByText("Что сделать"));
      expect(detailsOf(item("16 мин 40 с"))).toHaveAttribute("open");
      rerender(<FindingsPanel connectionId="c" findings={[other, query2, query1]} showMember />);
      expect(items().map((li) => li.textContent)).toEqual([
        expect.stringContaining("50 мин"),
        expect.stringContaining("33 мин 20 с"),
        expect.stringContaining("16 мин 40 с"),
      ]);
      expect(detailsOf(item("16 мин 40 с"))).toHaveAttribute("open");
      expect(detailsOf(item("33 мин 20 с"))).not.toHaveAttribute("open");
      expect(detailsOf(item("50 мин"))).not.toHaveAttribute("open");
    });

    it("одинаковые находки не дают предупреждения о повторе ключа и показываются обе", () => {
      const error = vi.spyOn(console, "error").mockImplementation(() => {});
      try {
        renderWithClient(<FindingsPanel connectionId="c" findings={[query1, { ...query1 }]} />);
        expect(items()).toHaveLength(2);
        expect(error.mock.calls.filter((c) => String(c[0]).includes("same key"))).toEqual([]);
      } finally {
        error.mockRestore();
      }
    });
  });

  it("узел показывается отдельной меткой только у находок отчёта кластера", () => {
    const findings = [{ id: "longQuery", severity: "warning" as const, member: "pg-2", target: null, params: { maxS: 1000, count: 1 }, links: [] }];
    const { rerender } = renderWithClient(<FindingsPanel connectionId="c" findings={findings} />);
    expect(screen.queryByText("pg-2")).not.toBeInTheDocument();
    rerender(<FindingsPanel connectionId="c" findings={findings} showMember />);
    expect(screen.getByText("pg-2")).toBeInTheDocument();
  });

  it("без находок — «Проблем не найдено»", () => {
    renderWithClient(<FindingsPanel connectionId="c" findings={[]} />);
    expect(screen.getByText("Проблем не найдено")).toBeInTheDocument();
  });
});
