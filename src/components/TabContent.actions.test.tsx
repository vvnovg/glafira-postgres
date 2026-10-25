import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../test/render";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { listActions: vi.fn(() => Promise.resolve([])) } };
});

import { api } from "../api/client";
import { TabContent } from "./TabContent";

describe("TabContent actions", () => {
  it("вкладка журнала читает записи своего подключения", async () => {
    renderWithClient(<TabContent tab={{ kind: "actions", connectionId: "c", id: "actions:c" }} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Журнал действий" })).toBeInTheDocument();
    expect(api.listActions).toHaveBeenCalledWith("c", 1000);
  });

  it("скрытая вкладка журнал не читает", async () => {
    vi.mocked(api.listActions).mockClear();
    renderWithClient(<TabContent tab={{ kind: "actions", connectionId: "c", id: "actions:c" }} active={false} />);
    expect(await screen.findByRole("heading", { level: 2, name: "Журнал действий" })).toBeInTheDocument();
    expect(api.listActions).not.toHaveBeenCalled();
  });
});
