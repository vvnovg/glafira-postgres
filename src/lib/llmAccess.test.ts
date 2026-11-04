import { describe, expect, it } from "vitest";
import { mcpStatus } from "../test/mcp";
import { effectiveLlmAccess } from "./llmAccess";

describe("effectiveLlmAccess", () => {
  it("уровень профиля; без поля — off", () => {
    expect(effectiveLlmAccess({ llmAccess: "diagnostics" }, mcpStatus())).toBe("diagnostics");
    expect(effectiveLlmAccess({ llmAccess: "off" }, mcpStatus())).toBe("off");
    expect(effectiveLlmAccess({}, mcpStatus())).toBe("off");
  });

  it("политика запрещает MCP — off при любом уровне", () => {
    expect(effectiveLlmAccess({ llmAccess: "diagnostics" }, mcpStatus({ policyEnabled: false }))).toBe("off");
  });

  it("потолок diagnostics «Диагностику» не снижает; статус неизвестен — уровень профиля", () => {
    expect(effectiveLlmAccess({ llmAccess: "diagnostics" }, mcpStatus({ maxAccess: "diagnostics" }))).toBe("diagnostics");
    expect(effectiveLlmAccess({ llmAccess: "diagnostics" }, undefined)).toBe("diagnostics");
  });
});
