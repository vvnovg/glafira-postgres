import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { connection, diagReport, member, topology, topologyView } from "../test/fixtures";
import { renderWithClient } from "../test/render";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(() => Promise.resolve([connection({ id: "c" })])),
      getTopology: vi.fn(() => Promise.resolve(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2")] }) }))),
      runDiagnostic: vi.fn(() => Promise.resolve(diagReport({ kind: "locks", roots: [], cycles: [], ddlQueue: [] }, { member: "pg-2", followsLeader: false }))),
    },
  };
});

import { api } from "../api/client";
import { TabContent } from "./TabContent";

describe("TabContent diag", () => {
  it("вкладка инструмента передаёт подключение, инструмент и узел вкладки", async () => {
    renderWithClient(<TabContent tab={{ kind: "diag", connectionId: "c", tool: "locks", member: "pg-2", id: "diag:c:locks" }} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Блокировки" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "locks", member: "pg-2" });
  });

  it("без узла в спеке запрос идёт «за лидером»; скрытая вкладка тоже читает первый отчёт", async () => {
    vi.mocked(api.runDiagnostic).mockClear();
    renderWithClient(<TabContent tab={{ kind: "diag", connectionId: "c", tool: "locks", id: "diag:c:locks" }} active={false} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Блокировки" })).toBeInTheDocument();
    expect(api.runDiagnostic).toHaveBeenCalledWith("c", { tool: "locks", member: null });
  });
});
