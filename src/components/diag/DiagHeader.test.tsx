import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DiagTool } from "../../api/types";
import ru from "../../locales/ru.json";
import { renderWithClient } from "../../test/render";
import { DiagHeader } from "./DiagHeader";

const TOOLS: Record<DiagTool, true> = {
  pulse: true,
  sessions: true,
  idleTx: true,
  waits: true,
  locks: true,
  horizon: true,
  replication: true,
  slots: true,
  failover: true,
  health: true,
};

describe("DiagHeader", () => {
  it("у «Горизонта xmin» рядом с названием — подсказка «?» с пояснением", async () => {
    renderWithClient(<DiagHeader tool="horizon" />);
    expect(screen.getByRole("heading", { name: "Горизонт xmin" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Подсказка: Горизонт xmin" }));
    expect(await screen.findByRole("tooltip")).toHaveTextContent("VACUUM не убирает мёртвые строки");
  });

  it.each(Object.keys(TOOLS) as DiagTool[])("у инструмента %s есть пояснение «?» со своим текстом", async (tool) => {
    renderWithClient(<DiagHeader tool={tool} />);
    const title = ru.diag.tools[tool];
    await userEvent.click(screen.getByRole("button", { name: `Подсказка: ${title}` }));
    const text = ru.hints.diag[tool].replace(/<\/?code>/g, "");
    expect(await screen.findByRole("tooltip")).toHaveTextContent(text);
  });
});
