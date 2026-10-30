import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { connection, member, topology, topologyView } from "../test/fixtures";
import { renderWithClient } from "../test/render";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { getTopology: vi.fn(), listConnections: vi.fn(), listDatabases: vi.fn() } };
});

import { api } from "../api/client";
import { TabContent } from "./TabContent";

describe("TabContent explain", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(api.getTopology).mockResolvedValue(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2", { role: "replica" })] }) }));
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "c" })]);
    vi.mocked(api.listDatabases).mockResolvedValue([{ name: "postgres", sizeBytes: 1 }, { name: "perf", sizeBytes: 1 }]);
  });

  it("вкладка консоли получает узел, базу и SQL из спеки", async () => {
    renderWithClient(<TabContent tab={{ kind: "explain", connectionId: "c", member: "pg-2", database: "perf", sql: "SELECT 1", id: "explain:c" }} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Консоль EXPLAIN" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "Узел" })).toHaveValue("pg-2");
    expect(screen.getByRole("combobox", { name: "База" })).toHaveValue("perf");
    expect(screen.getByRole("textbox", { name: "SQL" })).toHaveValue("SELECT 1");
    expect(api.listDatabases).toHaveBeenCalledWith("c");
  });

  it("скрытая вкладка список баз не читает", async () => {
    renderWithClient(<TabContent tab={{ kind: "explain", connectionId: "c", id: "explain:c" }} active={false} />);
    await vi.waitFor(() => expect(api.getTopology).toHaveBeenCalled());
    await Promise.resolve();
    expect(api.listDatabases).not.toHaveBeenCalled();
  });
});
