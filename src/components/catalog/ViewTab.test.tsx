import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Column, ViewDetails } from "../../api/types";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getView: vi.fn() } };
});

import { api } from "../../api/client";
import { ViewTab } from "./ViewTab";

const column = (name: string, dataType: string): Column => ({
  name, dataType, notNull: false, defaultExpr: null, identity: null, generated: null, collation: null, statTarget: null, storage: "p", comment: null,
});

const view: ViewDetails = {
  oid: 5, schema: "shop", name: "v_orders", matview: false, owner: "postgres", comment: "Заказы без черновиков",
  definition: " SELECT id, total\n   FROM shop.orders\n  WHERE NOT draft;",
  columns: [column("id", "bigint"), column("total", "numeric(12,2)")],
  populated: null, sizeBytes: null, indexes: [],
};

const matview: ViewDetails = {
  ...view, oid: 6, name: "order_totals", matview: true, populated: false, sizeBytes: 8192,
  indexes: [{ name: "order_totals_idx", definition: "CREATE INDEX order_totals_idx ON shop.order_totals USING btree (total)", primary: false, unique: false, valid: true, sizeBytes: 8192, idxScan: 4, idxTupRead: 9, lastIdxScanMs: null }],
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getView).mockResolvedValue(view);
});

describe("ViewTab", () => {
  it("представление: определение по умолчанию, колонки; индексов и заполненности нет", async () => {
    const user = userEvent.setup();
    renderWithClient(<ViewTab connectionId="c" database="orders" schema="shop" name="v_orders" matview={false} />);
    expect(await screen.findByText(/FROM shop\.orders/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "shop.v_orders" })).toBeInTheDocument();
    expect(api.getView).toHaveBeenCalledWith("c", "orders", "shop", "v_orders");
    expect(screen.getByText("Представление")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать определение" })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Индексы" })).not.toBeInTheDocument();
    expect(screen.queryByText("Заполнено")).not.toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Колонки" }));
    expect(within(screen.getByRole("table", { name: "Колонки" })).getByText("numeric(12,2)")).toBeInTheDocument();
  });

  it("мат. представление: не заполнено, размер, индексы", async () => {
    vi.mocked(api.getView).mockResolvedValue(matview);
    const user = userEvent.setup();
    renderWithClient(<ViewTab connectionId="c" database="orders" schema="shop" name="order_totals" matview />);
    expect(await screen.findByText("Заполнено")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "shop.order_totals" })).toBeInTheDocument();
    expect(screen.getByText("Материализованное представление")).toBeInTheDocument();
    expect(screen.getByText("Нет")).toBeInTheDocument();
    expect(screen.getByText("8,0 KB")).toBeInTheDocument();
    await user.click(screen.getByRole("tab", { name: "Индексы" }));
    expect(within(screen.getByRole("table", { name: "Индексы" })).getByText("order_totals_idx")).toBeInTheDocument();
  });

  it("ошибка загрузки показывается с повтором", async () => {
    vi.mocked(api.getView).mockRejectedValueOnce({ code: "TIMEOUT", message: "slow", retryable: true }).mockResolvedValueOnce(view);
    const user = userEvent.setup();
    renderWithClient(<ViewTab connectionId="c" database="orders" schema="shop" name="v_orders" matview={false} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("slow");
    await user.click(screen.getByRole("button", { name: "Повторить" }));
    expect(await screen.findByText(/FROM shop\.orders/)).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
