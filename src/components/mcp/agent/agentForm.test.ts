import { describe, expect, it } from "vitest";
import { agentSettings } from "../../../test/mcp";
import { fromDraft, hiddenErrorPreset, isLoopbackUrl, parseDecimal, toDraft, validateDraft } from "./agentForm";

const base = toDraft(agentSettings());

describe("agentForm", () => {
  it("черновик ↔ настройки: числа строками; пустой предел — null; запятая в пределе; пробелы по краям убираются", () => {
    expect(base).toMatchObject({ preset: "qwen", timeout: "15", keepWarm: "10", budget: "1", viaOllama: false });
    expect(fromDraft({ ...base, budget: "" }, agentSettings({ enabled: true }))).toEqual(
      agentSettings({ enabled: true, claude: { path: "", model: "", viaOllama: false, budgetUsd: null } }),
    );
    const d = fromDraft({ ...base, budget: "0,5", qwenPath: " /opt/homebrew/bin/qwen ", timeout: "20" }, agentSettings());
    expect(d.claude.budgetUsd).toBe(0.5);
    expect(d.qwen.path).toBe("/opt/homebrew/bin/qwen");
    expect(d.timeoutMin).toBe(20);
    // Выключатель в черновик не входит: он берётся из сохранённых настроек.
    expect(fromDraft(base, agentSettings({ enabled: true })).enabled).toBe(true);
    expect(fromDraft(toDraft(agentSettings({ claude: { path: "", model: "", viaOllama: false, budgetUsd: 2.5 } })), agentSettings()).claude.budgetUsd).toBe(2.5);
  });

  it("десятичное число: точка или запятая; экспонента, знак и пустое — не число", () => {
    expect(parseDecimal("2,5")).toBe(2.5);
    expect(parseDecimal(" 0.01 ")).toBe(0.01);
    for (const s of ["", "1e3", "-1", "1,2,3", "abc", "1."]) expect(parseDecimal(s)).toBeNaN();
  });

  it("сроки и предел — пределы Rust: целые 2–60 и 0–120; предел 0,01–100 или пусто", () => {
    expect(validateDraft({ ...base, timeout: "1" }).timeout).toEqual({ key: "settings.mcp.agent.errors.range", params: { min: 2, max: 60 } });
    expect(validateDraft({ ...base, timeout: "60" }).timeout).toBeUndefined();
    expect(validateDraft({ ...base, timeout: "61" }).timeout).toBeDefined();
    expect(validateDraft({ ...base, timeout: "15.5" }).timeout).toBeDefined();
    expect(validateDraft({ ...base, timeout: "" }).timeout).toBeDefined();
    expect(validateDraft({ ...base, keepWarm: "0" }).keepWarm).toBeUndefined();
    expect(validateDraft({ ...base, keepWarm: "121" }).keepWarm).toEqual({ key: "settings.mcp.agent.errors.range", params: { min: 0, max: 120 } });
    expect(validateDraft({ ...base, budget: "0" }).budget).toEqual({ key: "settings.mcp.agent.errors.budget", params: { min: 0.01, max: 100 } });
    expect(validateDraft({ ...base, budget: "0,01" }).budget).toBeUndefined();
    expect(validateDraft({ ...base, budget: "100" }).budget).toBeUndefined();
    expect(validateDraft({ ...base, budget: "100.01" }).budget).toBeDefined();
    expect(validateDraft({ ...base, budget: "" }).budget).toBeUndefined();
    expect(validateDraft({ ...base, budget: "1e3" }).budget).toBeDefined();
    expect(validateDraft(base)).toEqual({});
  });

  it("путь — пусто или абсолютный (Unix и Windows), без управляющих символов, до 1024; модель — пусто или допустимые символы до 128", () => {
    expect(validateDraft({ ...base, qwenPath: "qwen" }).qwenPath).toEqual({ key: "settings.mcp.agent.errors.path" });
    expect(validateDraft({ ...base, qwenPath: "/usr/local/bin/qwen" }).qwenPath).toBeUndefined();
    expect(validateDraft({ ...base, qwenPath: "  " }).qwenPath).toBeUndefined();
    expect(validateDraft({ ...base, claudePath: "C:\\Users\\me\\.local\\bin\\claude.exe" }).claudePath).toBeUndefined();
    expect(validateDraft({ ...base, claudePath: "\\\\server\\share\\claude.cmd" }).claudePath).toBeUndefined();
    expect(validateDraft({ ...base, qwenPath: "/usr/local/bin/qw\nen" }).qwenPath).toBeDefined();
    expect(validateDraft({ ...base, qwenPath: `/${"x".repeat(1024)}` }).qwenPath).toBeDefined();
    expect(validateDraft({ ...base, qwenModel: "qwen3.6-64k:35b-a3b" }).qwenModel).toBeUndefined();
    expect(validateDraft({ ...base, qwenModel: "bad model" }).qwenModel).toEqual({ key: "settings.mcp.agent.errors.model" });
    expect(validateDraft({ ...base, claudeModel: "x".repeat(128) }).claudeModel).toBeUndefined();
    expect(validateDraft({ ...base, claudeModel: "x".repeat(129) }).claudeModel).toBeDefined();
  });

  it("адрес Ollama: http(s) с хостом, до 256 символов; локальный — localhost, 127.0.0.1, ::1", () => {
    expect(validateDraft({ ...base, ollamaUrl: "127.0.0.1:11434" }).ollamaUrl).toEqual({ key: "settings.mcp.agent.errors.url" });
    expect(validateDraft({ ...base, ollamaUrl: "localhost:11434" }).ollamaUrl).toBeDefined();
    expect(validateDraft({ ...base, ollamaUrl: "ftp://127.0.0.1" }).ollamaUrl).toBeDefined();
    expect(validateDraft({ ...base, ollamaUrl: `http://${"a".repeat(250)}.com` }).ollamaUrl).toBeDefined();
    expect(validateDraft({ ...base, ollamaUrl: "http://10.0.0.5:11434" }).ollamaUrl).toBeUndefined();
    expect(validateDraft({ ...base, ollamaUrl: "https://ollama.example.com" }).ollamaUrl).toBeUndefined();
    expect(isLoopbackUrl("http://127.0.0.1:11434")).toBe(true);
    expect(isLoopbackUrl("http://localhost:11434")).toBe(true);
    expect(isLoopbackUrl("http://[::1]:11434")).toBe(true);
    expect(isLoopbackUrl("http://10.0.0.5:11434")).toBe(false);
    expect(isLoopbackUrl("not a url")).toBe(false);
  });

  it("ошибка в полях другого агента называет его; через Ollama предел не проверяется и не затирает сохранённый", () => {
    const d = { ...base, claudePath: "claude" };
    expect(hiddenErrorPreset(d, validateDraft(d))).toBe("claude");
    expect(hiddenErrorPreset({ ...d, preset: "claude" }, validateDraft(d))).toBeNull();
    expect(hiddenErrorPreset(base, validateDraft(base))).toBeNull();
    const v = { ...base, preset: "claude" as const, viaOllama: true, budget: "abc" };
    expect(validateDraft(v).budget).toBeUndefined();
    expect(fromDraft(v, agentSettings()).claude.budgetUsd).toBe(1);
  });
});
