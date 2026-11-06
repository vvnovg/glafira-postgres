import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { McpStatus } from "../../../api/types";
import { mcpStatus } from "../../../test/mcp";
import { LlmBadge } from "./LlmBadge";

const mcp = vi.hoisted(() => ({ status: undefined as McpStatus | undefined }));
vi.mock("../../../lib/useMcpStatus", () => ({
  useMcpStatus: () => ({ status: mcp.status, loading: mcp.status === undefined, error: null, refetch: () => {} }),
}));

describe("LlmBadge", () => {
  beforeEach(() => {
    mcp.status = mcpStatus();
  });

  it("off — значка нет; diagnostics — контурный", () => {
    const { container, rerender } = render(<LlmBadge connection={{ llmAccess: "off" }} />);
    expect(container).toBeEmptyDOMElement();
    rerender(<LlmBadge connection={{ llmAccess: "diagnostics" }} />);
    const badge = screen.getByRole("img", { name: "Доступ для LLM: диагностика" });
    expect(badge).toHaveClass("llm-badge");
    expect(badge).not.toHaveClass("llm-badge--data");
  });

  it("data — заполненный с подсказкой; без статуса или под потолком — как диагностика", () => {
    const { rerender } = render(<LlmBadge connection={{ llmAccess: "data" }} />);
    expect(screen.getByRole("img", { name: "Доступ для LLM: диагностика и данные" })).toHaveClass("llm-badge--data");
    mcp.status = undefined;
    rerender(<LlmBadge connection={{ llmAccess: "data" }} />);
    expect(screen.getByRole("img", { name: "Доступ для LLM: диагностика" })).not.toHaveClass("llm-badge--data");
    mcp.status = mcpStatus({ maxAccess: "diagnostics" });
    rerender(<LlmBadge connection={{ llmAccess: "data" }} />);
    expect(screen.getByRole("img", { name: "Доступ для LLM: диагностика" })).not.toHaveClass("llm-badge--data");
  });
});
