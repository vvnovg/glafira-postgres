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
    expect(names).toEqual(["Диагностика", "Пульс", "Сессии", "Idle in transaction", "Профиль ожиданий", "Блокировки", "Горизонт xmin", "Реплики", "Слоты репликации"]);
    await user.click(screen.getByRole("button", { name: "Блокировки" }));
    expect(useWorkspace.getState().tabs[0]).toMatchObject({ kind: "diag", connectionId: "c", tool: "locks", id: "diag:c:locks" });
    expect(useWorkspace.getState().tabs[0]).not.toHaveProperty("member", "pg-1");
  });

  it("у кластера Patroni — ещё готовность к failover и матрица health", async () => {
    const user = userEvent.setup();
    renderWithClient(<ul><DiagnosticsNodes connectionId="c" kind="patroni" /></ul>);
    await user.click(screen.getByRole("button", { name: "Диагностика" }));
    expect(screen.getByRole("button", { name: "Готовность к failover" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Матрица health" })).toBeInTheDocument();
  });

  it("список инструментов по виду подключения; у списка узлов нет инструментов Patroni", () => {
    expect(DIAG_TOOLS_FOR("hostList")).toEqual(DIAG_TOOLS_FOR("server"));
    expect(DIAG_TOOLS_FOR("patroni").slice(-2)).toEqual(["failover", "health"]);
    expect(DIAG_TOOLS_FOR("server")).not.toContain("failover");
  });
});
