import { describe, expect, it } from "vitest";
import { agentSettings } from "../../../test/mcp";
import { type AgentDraft, fromDraft, hiddenErrorPreset, isAbsolutePath, isLoopbackUrl, parseDecimal, type PathRules, pathRules, toDraft, unloadOff, validateDraft } from "./agentForm";

const base = toDraft(agentSettings());
/** Проверка черновика; ОС Glafira по умолчанию — macOS и Linux. */
const check = (d: AgentDraft, rules: PathRules = "unix") => validateDraft(d, rules);

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
    expect(check({ ...base, timeout: "1" }).timeout).toEqual({ key: "settings.mcp.agent.errors.range", params: { min: 2, max: 60 } });
    expect(check({ ...base, timeout: "60" }).timeout).toBeUndefined();
    expect(check({ ...base, timeout: "61" }).timeout).toBeDefined();
    expect(check({ ...base, timeout: "15.5" }).timeout).toBeDefined();
    expect(check({ ...base, timeout: "" }).timeout).toBeDefined();
    expect(check({ ...base, keepWarm: "0" }).keepWarm).toBeUndefined();
    expect(check({ ...base, keepWarm: "121" }).keepWarm).toEqual({ key: "settings.mcp.agent.errors.range", params: { min: 0, max: 120 } });
    expect(check({ ...base, budget: "0" }).budget).toEqual({ key: "settings.mcp.agent.errors.budget", params: { min: 0.01, max: 100 } });
    expect(check({ ...base, budget: "0,01" }).budget).toBeUndefined();
    expect(check({ ...base, budget: "100" }).budget).toBeUndefined();
    expect(check({ ...base, budget: "100.01" }).budget).toBeDefined();
    expect(check({ ...base, budget: "" }).budget).toBeUndefined();
    expect(check({ ...base, budget: "1e3" }).budget).toBeDefined();
    expect(check(base)).toEqual({});
  });

  it("путь — пусто или абсолютный по правилам ОС Glafira (как Path::is_absolute), без управляющих символов, до 1024", () => {
    expect(check({ ...base, qwenPath: "qwen" }).qwenPath).toEqual({ key: "settings.mcp.agent.errors.path" });
    expect(check({ ...base, qwenPath: "/usr/local/bin/qwen" }).qwenPath).toBeUndefined();
    expect(check({ ...base, qwenPath: "  " }).qwenPath).toBeUndefined();
    // macOS и Linux: путь Windows не абсолютный.
    expect(check({ ...base, claudePath: "C:\\Users\\me\\.local\\bin\\claude.exe" }).claudePath).toBeDefined();
    // Windows: буква диска или UNC с сервером и ресурсом; путь с «/» — нет.
    expect(check({ ...base, claudePath: "C:\\Users\\me\\.local\\bin\\claude.exe" }, "windows").claudePath).toBeUndefined();
    expect(check({ ...base, claudePath: "\\\\server\\share\\claude.cmd" }, "windows").claudePath).toBeUndefined();
    expect(check({ ...base, qwenPath: "/usr/local/bin/qwen" }, "windows").qwenPath).toBeDefined();
    // ОС ещё не прочитана — годится любой вид: решит Rust.
    expect(check({ ...base, claudePath: "C:\\Tools\\claude.exe", qwenPath: "/usr/local/bin/qwen" }, null)).toEqual({});
    expect(check({ ...base, qwenPath: "qwen" }, null).qwenPath).toBeDefined();
  });

  it("абсолютный путь Windows — как Path::is_absolute: диск, UNC с сервером и ресурсом (оба разделителя), \\\\?\\", () => {
    for (const p of ["C:\\tools\\qwen.cmd", "C:/tools/qwen.cmd", "\\\\server\\share\\qwen.cmd", "//server/share/qwen.cmd", "\\\\?\\C:\\tools\\qwen.cmd"])
      expect(isAbsolutePath(p, "windows"), p).toBe(true);
    for (const p of ["C:qwen.cmd", "\\\\server", "\\\\server\\", "\\tools\\qwen.cmd", "/usr/local/bin/qwen", "qwen"]) expect(isAbsolutePath(p, "windows"), p).toBe(false);
    expect(isAbsolutePath("\\\\server\\share\\qwen.cmd", "unix")).toBe(false);
    expect(isAbsolutePath("//server/share/qwen", "unix")).toBe(true);
    expect([pathRules("windows"), pathRules("macos"), pathRules("linux"), pathRules(undefined)]).toEqual(["windows", "unix", "unix", null]);
  });

  it("путь без управляющих символов, до 1024; модель — пусто или допустимые символы до 128", () => {
    expect(check({ ...base, qwenPath: "/usr/local/bin/qw\nen" }).qwenPath).toBeDefined();
    expect(check({ ...base, qwenPath: `/${"x".repeat(1024)}` }).qwenPath).toBeDefined();
    expect(check({ ...base, qwenModel: "qwen3.6-64k:35b-a3b" }).qwenModel).toBeUndefined();
    expect(check({ ...base, qwenModel: "bad model" }).qwenModel).toEqual({ key: "settings.mcp.agent.errors.model" });
    expect(check({ ...base, claudeModel: "x".repeat(128) }).claudeModel).toBeUndefined();
    expect(check({ ...base, claudeModel: "x".repeat(129) }).claudeModel).toBeDefined();
  });

  it("адрес Ollama — как settings::parse_ollama_url в Rust: только http(s)://хост[:порт], до 256 байт; локальный — localhost, 127.0.0.1, ::1", () => {
    expect(check({ ...base, ollamaUrl: "127.0.0.1:11434" }).ollamaUrl).toEqual({ key: "settings.mcp.agent.errors.url" });
    // Те же адреса, что в тесте Rust the_ollama_address_is_only_a_scheme_a_host_and_a_port.
    for (const ok of ["http://127.0.0.1:11434", "http://127.0.0.1:11434/", "HTTP://LocalHost:11434", "http://[::1]:11434", "http://10.0.0.5:11434", "https://ollama.example.com", "https://ollama.example.com:443/", " http://127.0.0.1:11434 "])
      expect(check({ ...base, ollamaUrl: ok }).ollamaUrl, ok).toBeUndefined();
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
      expect(check({ ...base, ollamaUrl: bad }).ollamaUrl, JSON.stringify(bad)).toEqual({ key: "settings.mcp.agent.errors.url" });
    expect(isLoopbackUrl("http://127.0.0.1:11434")).toBe(true);
    expect(isLoopbackUrl("http://localhost:11434")).toBe(true);
    expect(isLoopbackUrl("http://[::1]:11434")).toBe(true);
    expect(isLoopbackUrl("http://10.0.0.5:11434")).toBe(false);
    expect(isLoopbackUrl("not a url")).toBe(false);
  });

  it("ошибка в полях другого агента называет его; через Ollama предел не проверяется и не затирает сохранённый", () => {
    const d = { ...base, claudePath: "claude" };
    expect(hiddenErrorPreset(d, check(d))).toBe("claude");
    expect(hiddenErrorPreset({ ...d, preset: "claude" }, check(d))).toBeNull();
    expect(hiddenErrorPreset(base, check(base))).toBeNull();
    const v = { ...base, preset: "claude" as const, viaOllama: true, budget: "abc" };
    expect(check(v).budget).toBeUndefined();
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
