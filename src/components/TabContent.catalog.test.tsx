import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { topologyView } from "../test/fixtures";
import { renderWithClient } from "../test/render";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { getTable: vi.fn(), getView: vi.fn(), getFunction: vi.fn(), getSequence: vi.fn(), getTopology: vi.fn() } };
});

import { api } from "../api/client";
import { TabContent } from "./TabContent";

// Вкладки объектов открываются по своему виду и с теми же аргументами, что в TabSpec; тела — в тестах компонентов.
beforeEach(() => {
  vi.resetAllMocks();
  // Загрузка не заканчивается: проверяется только, какая вкладка открыта и с какими аргументами.
  vi.mocked(api.getTable).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.getView).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.getFunction).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.getSequence).mockReturnValue(new Promise(() => {}));
  vi.mocked(api.getTopology).mockResolvedValue(topologyView());
});

describe("TabContent: объекты каталога", () => {
  it("таблица", () => {
    renderWithClient(<TabContent tab={{ kind: "table", connectionId: "c", database: "orders", schema: "shop", name: "events", id: "t" }} />);
    expect(screen.getByRole("heading", { name: "shop.events" })).toBeInTheDocument();
    expect(api.getTable).toHaveBeenCalledWith("c", "orders", "shop", "events");
  });

  it("представление и мат. представление", () => {
    const first = renderWithClient(<TabContent tab={{ kind: "view", connectionId: "c", database: "orders", schema: "shop", name: "v", matview: false, id: "v" }} />);
    expect(screen.getByText("Представление")).toBeInTheDocument();
    expect(api.getView).toHaveBeenCalledWith("c", "orders", "shop", "v");
    first.unmount();
    renderWithClient(<TabContent tab={{ kind: "view", connectionId: "c", database: "orders", schema: "shop", name: "m", matview: true, id: "m" }} />);
    expect(screen.getByText("Материализованное представление")).toBeInTheDocument();
  });

  it("функция — по oid", () => {
    renderWithClient(<TabContent tab={{ kind: "function", connectionId: "c", database: "orders", oid: 16401, name: "order_count", id: "f" }} />);
    expect(screen.getByRole("heading", { name: "order_count" })).toBeInTheDocument();
    expect(api.getFunction).toHaveBeenCalledWith("c", "orders", 16401);
  });

  it("последовательность", () => {
    renderWithClient(<TabContent tab={{ kind: "sequence", connectionId: "c", database: "orders", schema: "shop", name: "s", id: "s" }} />);
    expect(screen.getByRole("heading", { name: "shop.s" })).toBeInTheDocument();
    expect(api.getSequence).toHaveBeenCalledWith("c", "orders", "shop", "s");
  });
});
