import { describe, expect, it } from "vitest";
import { mcpStatus } from "../test/mcp";
import { dataAllowed, effectiveLlmAccess } from "./llmAccess";

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

describe("уровень «Диагностика и данные» (P4b)", () => {
  it("строгий учёт статуса: пока статуса нет — ни «разрешено», ни «запрещено»", () => {
    expect(dataAllowed(undefined)).toBeUndefined();
    expect(dataAllowed(mcpStatus())).toBe(true);
    expect(dataAllowed(mcpStatus({ maxAccess: "diagnostics" }))).toBe(false);
  });

  it("data действует, только когда статус пришёл и потолок разрешает данные", () => {
    expect(effectiveLlmAccess({ llmAccess: "data" }, mcpStatus())).toBe("data");
    expect(effectiveLlmAccess({ llmAccess: "data" }, undefined)).toBe("diagnostics");
    expect(effectiveLlmAccess({ llmAccess: "data" }, mcpStatus({ maxAccess: "diagnostics" }))).toBe("diagnostics");
    expect(effectiveLlmAccess({ llmAccess: "data" }, mcpStatus({ policyEnabled: false }))).toBe("off");
  });
});
