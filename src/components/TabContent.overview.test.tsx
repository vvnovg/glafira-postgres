import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { member, topology, topologyView } from "../test/fixtures";
import { renderWithClient } from "../test/render";
import { newProfile } from "./connections/profile";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return {
    ...orig,
    api: {
      listConnections: vi.fn(() => Promise.resolve([{ ...newProfile(), id: "c", name: "billing", kind: "patroni" }])),
      getTopology: vi.fn(() => Promise.resolve(topologyView({ topology: topology({ members: [member("pg-1", { role: "primary" }), member("pg-2")] }) }))),
      listTopologyEvents: vi.fn(() => Promise.resolve([])),
      getMemberFacts: vi.fn(() => Promise.resolve({ startedAtMs: Date.now(), connections: 1, maxConnections: 100, databases: 2 })),
    },
  };
});

import { TabContent } from "./TabContent";

describe("TabContent overview", () => {
  it("вкладка обзора показывает подключение и выделяет выбранный узел", async () => {
    renderWithClient(<TabContent tab={{ kind: "overview", connectionId: "c", member: "pg-2", id: "overview:c" }} />);
    expect(await screen.findByRole("heading", { level: 2, name: "billing" })).toBeInTheDocument();
    expect(await screen.findByRole("row", { name: /^pg-2 / })).toHaveAttribute("aria-current", "true");
  });
});
