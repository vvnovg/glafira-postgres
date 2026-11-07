import { describe, expect, it } from "vitest";
import { agentSettings } from "../../../test/mcp";
import { fromDraft, hiddenErrorPreset, isAbsolutePath, isLoopbackUrl, parseDecimal, toDraft, unloadOff, validateDraft } from "./agentForm";

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

  it("путь — пусто или абсолютный по правилам ОС Glafira (как Path::is_absolute), без управляющих символов, до 1024", () => {
    expect(validateDraft({ ...base, qwenPath: "qwen" }).qwenPath).toEqual({ key: "settings.mcp.agent.errors.path" });
    expect(validateDraft({ ...base, qwenPath: "/usr/local/bin/qwen" }).qwenPath).toBeUndefined();
    expect(validateDraft({ ...base, qwenPath: "  " }).qwenPath).toBeUndefined();
    // macOS и Linux (jsdom — не Windows): путь Windows не абсолютный.
    expect(validateDraft({ ...base, claudePath: "C:\\Users\\me\\.local\\bin\\claude.exe" }).claudePath).toBeDefined();
    // Windows: буква диска или UNC; путь с «/» — нет.
    expect(validateDraft({ ...base, claudePath: "C:\\Users\\me\\.local\\bin\\claude.exe" }, true).claudePath).toBeUndefined();
    expect(validateDraft({ ...base, claudePath: "\\\\server\\share\\claude.cmd" }, true).claudePath).toBeUndefined();
    expect(validateDraft({ ...base, qwenPath: "/usr/local/bin/qwen" }, true).qwenPath).toBeDefined();
    expect(isAbsolutePath("C:/tools/qwen.cmd", true)).toBe(true);
    expect(isAbsolutePath("C:qwen.cmd", true)).toBe(false);
    expect(isAbsolutePath("\\\\server\\share\\qwen.cmd", false)).toBe(false);
    expect(validateDraft({ ...base, qwenPath: "/usr/local/bin/qw\nen" }).qwenPath).toBeDefined();
    expect(validateDraft({ ...base, qwenPath: `/${"x".repeat(1024)}` }).qwenPath).toBeDefined();
    expect(validateDraft({ ...base, qwenModel: "qwen3.6-64k:35b-a3b" }).qwenModel).toBeUndefined();
    expect(validateDraft({ ...base, qwenModel: "bad model" }).qwenModel).toEqual({ key: "settings.mcp.agent.errors.model" });
    expect(validateDraft({ ...base, claudeModel: "x".repeat(128) }).claudeModel).toBeUndefined();
    expect(validateDraft({ ...base, claudeModel: "x".repeat(129) }).claudeModel).toBeDefined();
  });

  it("адрес Ollama — как settings::parse_ollama_url в Rust: только http(s)://хост[:порт], до 256 байт; локальный — localhost, 127.0.0.1, ::1", () => {
    expect(validateDraft({ ...base, ollamaUrl: "127.0.0.1:11434" }).ollamaUrl).toEqual({ key: "settings.mcp.agent.errors.url" });
    // Те же адреса, что в тесте Rust the_ollama_address_is_only_a_scheme_a_host_and_a_port.
    for (const ok of ["http://127.0.0.1:11434", "http://127.0.0.1:11434/", "HTTP://LocalHost:11434", "http://[::1]:11434", "http://10.0.0.5:11434", "https://ollama.example.com", "https://ollama.example.com:443/", " http://127.0.0.1:11434 "])
      expect(validateDraft({ ...base, ollamaUrl: ok }).ollamaUrl, ok).toBeUndefined();
    for (const bad of [
      "localhost:11434",
      "ftp://127.0.0.1",
      "file:///tmp/ollama",
      "http://127.0.0.1:11434/v1",
      "http://127.0.0.1:11434/api/",
      "http://127.0.0.1:11434?x=1",
      "http://127.0.0.1:11434/?",
      "http://127.0.0.1:11434#top",
      "http://user@127.0.0.1:11434",
      "http://user:secret@127.0.0.1:11434",
      "http://:secret@127.0.0.1:11434",
      "http://127.0.0.1:11\n434",
      "http://127.0.0.1:11434\t/",
      "http://127.0.0.1:99999",
      `http://${"a".repeat(250)}.example.com`,
    ])
      expect(validateDraft({ ...base, ollamaUrl: bad }).ollamaUrl, JSON.stringify(bad)).toEqual({ key: "settings.mcp.agent.errors.url" });
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

  it("«Выгрузить модель»: Glafira управляет моделью только у локальной Ollama, при «держать модель» > 0 и не у Claude Code без Ollama", () => {
    expect(unloadOff(agentSettings())).toBeNull();
    expect(unloadOff(agentSettings({ preset: "claude" }))).toBe("noLocalModel");
    expect(unloadOff(agentSettings({ preset: "claude", claude: { path: "", model: "m", viaOllama: true, budgetUsd: 1 } }))).toBeNull();
    expect(unloadOff(agentSettings({ keepWarmMin: 0 }))).toBe("keepWarmOff");
    expect(unloadOff(agentSettings({ ollamaUrl: "http://10.0.0.5:11434" }))).toBe("remote");
  });
});
