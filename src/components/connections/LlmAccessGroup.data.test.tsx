import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpStatus, ReadinessItem } from "../../api/types";
import { connection } from "../../test/fixtures";
import { mcpStatus } from "../../test/mcp";
import { LlmAccessGroup } from "./LlmAccessGroup";

const mcp = vi.hoisted(() => ({ status: undefined as McpStatus | undefined }));
vi.mock("../../lib/useMcpStatus", () => ({
  useMcpStatus: () => ({ status: mcp.status, loading: mcp.status === undefined, error: null, refetch: () => {} }),
}));

const dataRadio = () => screen.getByRole("radio", { name: /Диагностика и данные/ });

describe("LlmAccessGroup — уровень «Диагностика и данные»", () => {
  beforeEach(() => {
    mcp.status = undefined;
  });

  it("пока статус MCP не пришёл, третий уровень неактивен без пометки, а сохранённый data остаётся выбранным", () => {
    render(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "data" })} onChange={() => {}} />);
    expect(dataRadio()).toBeDisabled();
    expect(dataRadio()).toBeChecked();
    expect(screen.queryByText(/ограничен вашей организацией/)).toBeNull();
  });

  it("при потолке data уровень выбирается", async () => {
    mcp.status = mcpStatus();
    const onChange = vi.fn();
    render(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "diagnostics" })} onChange={onChange} />);
    expect(dataRadio()).toBeEnabled();
    await userEvent.click(dataRadio());
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ llmAccess: "data" }));
  });

  it("prod и data — предупреждение, что данные уйдут провайдеру LLM", () => {
    mcp.status = mcpStatus();
    render(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "data", environment: "prod" })} onChange={() => {}} />);
    expect(screen.getByText("Данные production-базы будут уходить провайдеру LLM")).toBeInTheDocument();
  });

  it("потолок diagnostics: третий уровень неактивен; у сохранённого data — «Действует как „Диагностика“»", () => {
    mcp.status = mcpStatus({ maxAccess: "diagnostics" });
    const { rerender } = render(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "data" })} onChange={() => {}} />);
    expect(dataRadio()).toBeDisabled();
    expect(screen.getByText("Действует как «Диагностика»: доступ к данным ограничен вашей организацией")).toBeInTheDocument();
    rerender(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "diagnostics" })} onChange={() => {}} />);
    expect(screen.getByText("Доступ к данным ограничен вашей организацией")).toBeInTheDocument();
  });

  it("предупреждение о роли из последней проверки готовности — только на уровне data", () => {
    mcp.status = mcpStatus();
    const item: ReadinessItem = {
      id: "llmRole",
      status: "warn",
      params: { role: "postgres", memberships: "superuser", reason: "superuser" },
      fix: "CREATE ROLE glafira_llm LOGIN PASSWORD '…';",
    };
    const text =
      "Роль postgres — суперпользователь: через run_select модель сможет завершать сессии, писать, читать файлы сервера и запускать программы, если обойдёт список запрещённых функций";
    const { rerender } = render(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "data" })} onChange={() => {}} roleItem={item} />);
    expect(screen.getByText(text)).toBeInTheDocument();
    rerender(<LlmAccessGroup idPrefix="f" profile={connection({ llmAccess: "diagnostics" })} onChange={() => {}} roleItem={item} />);
    expect(screen.queryByText(text)).toBeNull();
  });
});
