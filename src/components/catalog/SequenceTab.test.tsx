import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SequenceDetails } from "../../api/types";
import { topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getSequence: vi.fn(), getTopology: vi.fn() } };
});

import { api } from "../../api/client";
import { SequenceTab } from "./SequenceTab";

const seq: SequenceDetails = {
  oid: 9, schema: "shop", name: "orders_id_seq", dataType: "integer", start: 1, increment: 1, min: 1, max: 100, cycle: false, cache: 1,
  lastValue: 50, readable: true, percentUsed: 49.494949494949495, ownedBy: "shop.orders.id",
};

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getSequence).mockResolvedValue(seq);
  vi.mocked(api.getTopology).mockResolvedValue(topologyView());
});

describe("SequenceTab", () => {
  it("тип, последнее значение, границы, шаг, процент использования и владеющая колонка", async () => {
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    expect(await screen.findByText("49,5 %")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "shop.orders_id_seq" })).toBeInTheDocument();
    expect(api.getSequence).toHaveBeenCalledWith("c", "orders", "shop", "orders_id_seq");
    expect(screen.getByText("Последовательность")).toBeInTheDocument();
    expect(screen.getByText("integer")).toBeInTheDocument();
    expect(screen.getByText("50")).toBeInTheDocument();
    expect(screen.getByText("100")).toBeInTheDocument();
    expect(screen.getByText("shop.orders.id")).toBeInTheDocument();
  });

  it("ещё не использована: last_value и процент — прочерк", async () => {
    vi.mocked(api.getSequence).mockResolvedValue({ ...seq, lastValue: null, percentUsed: null, ownedBy: null });
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    await screen.findByText("Последнее значение");
    // Последнее значение, процент и владеющая колонка.
    expect(screen.getAllByText("—")).toHaveLength(3);
    expect(screen.queryByText("Нет права на чтение")).not.toBeInTheDocument();
  });

  it("у роли нет права на последовательность: об этом сказано словами, а не прочерком", async () => {
    vi.mocked(api.getSequence).mockResolvedValue({ ...seq, lastValue: null, percentUsed: null, readable: false });
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    await screen.findByText("Последнее значение");
    // Два поля — последнее значение и процент использования.
    expect(screen.getAllByText("Нет права на чтение")).toHaveLength(2);
  });

  it("границы bigint показываются точно, а не округлёнными вверх (getByText сворачивает неразрывные пробелы)", async () => {
    vi.mocked(api.getSequence).mockResolvedValue({ ...seq, dataType: "bigint", max: 2 ** 63, lastValue: 1, percentUsed: 0 });
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    expect(await screen.findByText("9 223 372 036 854 775 807")).toBeInTheDocument();
  });

  it("цикличная последовательность и убывающий шаг", async () => {
    vi.mocked(api.getSequence).mockResolvedValue({ ...seq, cycle: true, increment: -5, cache: 20 });
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    await screen.findByText("Последнее значение");
    expect(screen.getByText("Да")).toBeInTheDocument();
    expect(screen.getByText(/^[-−]5$/)).toBeInTheDocument();
    expect(screen.getByText("20")).toBeInTheDocument();
  });

  it("ошибка загрузки показывается с повтором", async () => {
    vi.mocked(api.getSequence).mockRejectedValue({ code: "TIMEOUT", message: "slow", retryable: true });
    renderWithClient(<SequenceTab connectionId="c" database="orders" schema="shop" name="orders_id_seq" />);
    expect(await screen.findByRole("alert")).toHaveTextContent("slow");
  });
});
