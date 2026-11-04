import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ConnectionProfile } from "../../api/types";
import { mcpStatus } from "../../test/mcp";
import { renderWithClient } from "../../test/render";
import { newProfile } from "./profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return { ...orig, api: { getMcpStatus: vi.fn() } };
});

import { api } from "../../api/client";
import { LlmAccessGroup } from "./LlmAccessGroup";

/** Группа в сетке формы с состоянием профиля, как в ConnectionForm; onProfile — каждый новый профиль. */
function Harness({ initial, onProfile }: { initial: ConnectionProfile; onProfile?: (p: ConnectionProfile) => void }) {
  const [profile, setProfile] = useState(initial);
  return (
    <div className="form">
      <LlmAccessGroup
        idPrefix="f"
        profile={profile}
        onChange={(p) => {
          setProfile(p);
          onProfile?.(p);
        }}
      />
    </div>
  );
}

const note = () => screen.getByRole("status");

beforeEach(() => vi.clearAllMocks());

describe("LlmAccessGroup", () => {
  it("уровни «Нет» и «Диагностика»; без поля в профиле — «Нет»; выбор уходит в профиль", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus());
    const onProfile = vi.fn();
    const user = userEvent.setup();
    renderWithClient(<Harness initial={newProfile()} onProfile={onProfile} />);
    expect(screen.getByRole("radiogroup", { name: "Доступ для LLM" })).toBeInTheDocument();
    expect(screen.getAllByRole("radio")).toHaveLength(2);
    expect(screen.getByRole("radio", { name: "Нет" })).toBeChecked();
    await user.click(screen.getByRole("radio", { name: "Диагностика" }));
    expect(onProfile).toHaveBeenLastCalledWith(expect.objectContaining({ llmAccess: "diagnostics" }));
    expect(screen.getByRole("radio", { name: "Диагностика" })).toBeChecked();
  });

  it("строгий учёт: пока статус MCP не пришёл — без пометок; не загрузился — «Статус MCP недоступен»", async () => {
    vi.mocked(api.getMcpStatus).mockReturnValue(new Promise(() => {}));
    const first = renderWithClient(<Harness initial={{ ...newProfile(), llmAccess: "diagnostics" }} />);
    expect(note()).toBeEmptyDOMElement();
    first.unmount();
    vi.mocked(api.getMcpStatus).mockRejectedValue({ code: "INTERNAL", message: "boom", retryable: false });
    renderWithClient(<Harness initial={{ ...newProfile(), llmAccess: "diagnostics" }} />);
    await waitFor(() => expect(note()).toHaveTextContent("Статус MCP недоступен"));
    // Переключатели «Нет» и «Диагностика» от статуса не зависят.
    expect(screen.getByRole("radio", { name: "Нет" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "Диагностика" })).toBeEnabled();
  });

  it("политика запрещает MCP — пометка; сервер выключен при открытом подключении — подсказка про «Настройки»", async () => {
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ policyEnabled: false, server: "stopped" }));
    const first = renderWithClient(<Harness initial={newProfile()} />);
    await waitFor(() => expect(note()).toHaveTextContent("MCP отключён вашей организацией: уровень не действует"));
    first.unmount();
    vi.mocked(api.getMcpStatus).mockResolvedValue(mcpStatus({ server: "stopped" }));
    const user = userEvent.setup();
    const { client } = renderWithClient(<Harness initial={newProfile()} />);
    // Уровень «Нет»: выключенный сервер ничего не меняет — пометки нет.
    await waitFor(() => expect(client.getQueryData(["mcpStatus"])).toBeDefined());
    expect(note()).toBeEmptyDOMElement();
    await user.click(screen.getByRole("radio", { name: "Диагностика" }));
    expect(note()).toHaveTextContent("MCP-сервер выключен: включите его в «Настройках» → «LLM-ассистенты (MCP)»");
  });
});
