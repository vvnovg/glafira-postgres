import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook, waitFor } from "@testing-library/react";
import { createElement, type ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionProfile } from "../api/types";
import { connection } from "../test/fixtures";

vi.mock("../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../api/client")>();
  return { ...orig, api: { listConnections: vi.fn() } };
});

import { api } from "../api/client";
import { canAct, useConnectionProfile } from "./useConnectionProfile";

/** Один клиент на тест: перерисовка хука сохраняет кэш запросов, а не заводит новый. */
function setup(id: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => createElement(QueryClientProvider, { client }, children);
  return { client, ...renderHook((p: { id: string }) => useConnectionProfile(p.id), { wrapper, initialProps: { id } }) };
}

describe("useConnectionProfile", () => {
  beforeEach(() => {
    vi.mocked(api.listConnections).mockReset();
  });

  it("возвращает профиль по id из списка подключений", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "a", name: "A" }), connection({ id: "b", name: "B", environment: "prod" })]);
    const { result } = setup("b");
    expect(result.current).toBeUndefined();
    await waitFor(() => expect(result.current?.name).toBe("B"));
    expect(result.current?.environment).toBe("prod");
  });

  it("неизвестный id — undefined, когда запрос уже завершён", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "a" })]);
    const { result, client } = setup("zzz");
    await waitFor(() => expect(client.getQueryState(["connections"])?.status).toBe("success"));
    expect(client.getQueryData<ConnectionProfile[]>(["connections"])).toHaveLength(1);
    expect(result.current).toBeUndefined();
  });

  it("перерисовка с другим id берёт профиль из того же кэша без нового запроса", async () => {
    vi.mocked(api.listConnections).mockResolvedValue([connection({ id: "a", name: "A" }), connection({ id: "b", name: "B" })]);
    const { result, rerender } = setup("a");
    await waitFor(() => expect(result.current?.name).toBe("A"));
    rerender({ id: "b" });
    expect(result.current?.name).toBe("B");
    expect(api.listConnections).toHaveBeenCalledTimes(1);
  });
});

describe("canAct", () => {
  it("нужен профиль без read-only", () => {
    expect(canAct(undefined)).toBe(false);
    expect(canAct(connection({ readOnly: true }))).toBe(false);
    expect(canAct(connection({ readOnly: false }))).toBe(true);
  });

  it("в ветке «нельзя» имя и окружение профиля доступны (проверяется tsc: предикат типа сузил бы профиль до never)", () => {
    const profile: ConnectionProfile | undefined = connection({ name: "billing-prod", environment: "prod", readOnly: true });
    const hint = canAct(profile) ? "" : `${profile?.name} (${profile?.environment})`;
    expect(hint).toBe("billing-prod (prod)");
    if (!canAct(profile)) {
      // До `profile?.` в этой ветке профиль не `never`: поля читаются как у ConnectionProfile | undefined.
      const name: string | undefined = profile?.name;
      expect(name).toBe("billing-prod");
    }
  });
});
