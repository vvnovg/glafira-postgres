import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { FunctionDetails } from "../../api/types";
import { topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getFunction: vi.fn(), getTopology: vi.fn() } };
});

import { api } from "../../api/client";
import { FunctionTab } from "./FunctionTab";

const fn: FunctionDetails = {
  oid: 16401, schema: "shop", name: "order_count", kind: "f", signature: "c bigint", returns: "bigint", language: "sql", volatility: "s",
  securityDefiner: true, owner: "postgres", comment: "Число заказов",
  definition: "CREATE OR REPLACE FUNCTION shop.order_count(c bigint)\n RETURNS bigint\n LANGUAGE sql\nAS $function$ SELECT count(*) FROM shop.orders $function$\n",
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getFunction).mockResolvedValue(fn);
  vi.mocked(api.getTopology).mockResolvedValue(topologyView());
});

afterEach(() => {
  Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
});

describe("FunctionTab", () => {
  it("сигнатура, результат, язык, изменчивость, SECURITY DEFINER и определение", async () => {
    renderWithClient(<FunctionTab connectionId="c" database="orders" oid={16401} name="order_count" />);
    expect(await screen.findByText("shop.order_count(c bigint)")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "order_count" })).toBeInTheDocument();
    expect(api.getFunction).toHaveBeenCalledWith("c", "orders", 16401);
    expect(screen.getByText("Функция")).toBeInTheDocument();
    expect(screen.getByText("sql")).toBeInTheDocument();
    expect(screen.getByText("stable")).toBeInTheDocument();
    expect(screen.getByText("SECURITY DEFINER")).toBeInTheDocument();
    expect(screen.getByText("Да")).toBeInTheDocument();
    expect(screen.getByText("postgres")).toBeInTheDocument();
    expect(screen.getByText("Число заказов")).toBeInTheDocument();
    expect(screen.getByText(/CREATE OR REPLACE FUNCTION shop\.order_count/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Копировать определение" })).toBeInTheDocument();
  });

  it("«Копировать определение» кладёт в буфер ровно текст определения", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    // user-event.setup() ставит свою заглушку буфера — подменять нужно после него.
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderWithClient(<FunctionTab connectionId="c" database="orders" oid={16401} name="order_count" />);
    await user.click(await screen.findByRole("button", { name: "Копировать определение" }));
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(writeText).toHaveBeenCalledWith(fn.definition);
  });

  it("агрегат: только сигнатура, определения нет", async () => {
    vi.mocked(api.getFunction).mockResolvedValue({ ...fn, kind: "a", returns: null, definition: null, securityDefiner: false, volatility: "i", language: "internal" });
    renderWithClient(<FunctionTab connectionId="c" database="orders" oid={16401} name="order_count" />);
    expect(await screen.findByText("Агрегатная функция")).toBeInTheDocument();
    expect(screen.getByText("Определение не строится: доступна только сигнатура.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Копировать определение" })).not.toBeInTheDocument();
    expect(screen.getByText("Нет")).toBeInTheDocument();
    expect(screen.getByText("immutable")).toBeInTheDocument();
  });

  it("процедура: результата нет — прочерк", async () => {
    vi.mocked(api.getFunction).mockResolvedValue({ ...fn, kind: "p", returns: null, securityDefiner: false });
    renderWithClient(<FunctionTab connectionId="c" database="orders" oid={16401} name="order_count" />);
    expect(await screen.findByText("Процедура")).toBeInTheDocument();
    expect(screen.getByText("—")).toBeInTheDocument();
  });

  it("ошибка загрузки показывается с повтором", async () => {
    vi.mocked(api.getFunction).mockRejectedValue({ code: "TIMEOUT", message: "slow", retryable: true });
    renderWithClient(<FunctionTab connectionId="c" database="orders" oid={16401} name="order_count" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("slow");
    expect(screen.getByRole("button", { name: "Повторить" })).toBeInTheDocument();
  });
});
