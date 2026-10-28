import { useQuery } from "@tanstack/react-query";
import { act, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getTopology: vi.fn() } };
});

import { api } from "../../api/client";
import { ObjectTab } from "./ObjectTab";

/** Вкладка с запросом, чтением которого управляет тест: каждое чтение ждёт своего `resolve`. */
function Harness({ read }: { read: () => Promise<string> }) {
  const query = useQuery({ queryKey: ["object-tab-test"], queryFn: read });
  return (
    <ObjectTab connectionId="c" title="shop.orders" kind="Таблица" query={query}>
      {(data) => <p>{data}</p>}
    </ObjectTab>
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getTopology).mockResolvedValue(topologyView());
});

describe("ObjectTab", () => {
  it("«Обновить» читает данные заново; пока чтение идёт, кнопка не disabled (фокус остаётся), а клик повторного чтения не запускает", async () => {
    const user = userEvent.setup();
    const pending: Array<(v: string) => void> = [];
    const read = vi.fn(() => new Promise<string>((resolve) => pending.push(resolve)));
    renderWithClient(<Harness read={read} />);
    const refresh = screen.getByRole("button", { name: "Обновить" });
    // Первое чтение ещё идёт.
    expect(refresh).toHaveAttribute("aria-busy", "true");
    expect(refresh).toHaveAttribute("aria-disabled", "true");
    expect(refresh).not.toHaveAttribute("disabled");
    await user.click(refresh);
    expect(read).toHaveBeenCalledTimes(1);
    await act(async () => pending[0]("v1"));
    expect(await screen.findByText("v1")).toBeInTheDocument();
    expect(refresh).toHaveAttribute("aria-busy", "false");
    expect(refresh).toHaveAttribute("aria-disabled", "false");

    // Обычное нажатие читает заново; фокус остаётся на кнопке, пока идёт загрузка.
    await user.click(refresh);
    expect(read).toHaveBeenCalledTimes(2);
    expect(refresh).toHaveFocus();
    expect(refresh).toHaveAttribute("aria-busy", "true");
    expect(refresh).toHaveAttribute("aria-disabled", "true");
    expect(refresh).not.toHaveAttribute("disabled");
    // Ещё нажатия (мышью и с клавиатуры) во время чтения его не перезапускают.
    await user.click(refresh);
    await user.keyboard("{Enter}");
    expect(read).toHaveBeenCalledTimes(2);
    expect(refresh).toHaveFocus();

    await act(async () => pending[1]("v2"));
    expect(await screen.findByText("v2")).toBeInTheDocument();
    expect(refresh).toHaveAttribute("aria-disabled", "false");
    await user.click(refresh);
    expect(read).toHaveBeenCalledTimes(3);
  });
});
