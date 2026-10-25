import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { useWorkspace } from "../../../state/workspace";
import { renderWithClient } from "../../../test/render";
import { DIAG_TOOLS_FOR, DiagnosticsNodes } from "./DiagnosticsNodes";

describe("DiagnosticsNodes", () => {
  it("у сервера нет инструментов Patroni; клик открывает вкладку инструмента «за лидером»", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="server" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    // Стрелка раскрытия группы — декоративный символ (aria-hidden): в имя кнопки она не входит.
    const names = screen.getAllByRole("button").map((b) => b.textContent?.replace(/^[▸▾]/, ""));
    expect(names).toEqual(["Диагностика", "Пульс", "Сессии", "Idle in transaction", "Профиль ожиданий", "Блокировки", "Горизонт xmin", "Реплики", "Слоты репликации", "Журнал действий"]);
    await user.click(screen.getByRole("button", { name: "Блокировки" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", connectionId: "c", tool: "locks", id: "diag:c:locks" });
    expect(useWorkspace.getState().tabs[0]).not.toHaveProperty("member", "pg-1");
  });

  it("последний узел группы — «Журнал действий»; клик открывает вкладку журнала подключения", async () => {
    const user = userEvent.setup();
    useWorkspace.setState({ tabs: [], activeId: null });
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="server" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    const buttons = screen.getAllByRole("button");
    expect(buttons[buttons.length - 1]).toHaveTextContent("Журнал действий");
    await user.click(screen.getByRole("button", { name: "Журнал действий" }));
    expect(useWorkspace.getState().tabs).toEqual([{ kind: "actions", connectionId: "c", id: "actions:c" }]);
    expect(useWorkspace.getState().activeId).toBe("actions:c");
  });

  it("у кластера Patroni — ещё готовность к failover и матрица health", async () => {
    const user = userEvent.setup();
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="patroni" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    expect(screen.getByRole("button", { name: "Готовность к failover" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Матрица health" })).toBeInTheDocument();
    // Журнал — после инструментов и у кластера.
    const buttons = screen.getAllByRole("button");
    expect(buttons[buttons.length - 1]).toHaveTextContent("Журнал действий");
  });

  it("список инструментов по виду подключения; у списка узлов нет инструментов Patroni", () => {
    expect(DIAG_TOOLS_FOR("hostList")).toEqual(DIAG_TOOLS_FOR("server"));
    expect(DIAG_TOOLS_FOR("patroni").slice(-2)).toEqual(["failover", "health"]);
    expect(DIAG_TOOLS_FOR("server")).not.toContain("failover");
  });
});
