import { describe, expect, it } from "vitest";
import { i18n } from "../../../i18n";
import { agentRun } from "../../../test/llm";
import { agentLabel, failReasonText, warningText } from "./agentText";

const t = i18n.t.bind(i18n);

describe("agentText", () => {
  it("агент и модель запуска; без модели — только агент", () => {
    expect(agentLabel(agentRun())).toBe("Qwen Code, qwen3.6-64k:35b-a3b");
    expect(agentLabel(agentRun({ preset: "claude", model: null }))).toBe("Claude Code");
  });

  it("причины ошибки: срок в минутах от старта до конца, путь программы, код выхода; без данных — общий текст", () => {
    expect(failReasonText(agentRun({ reason: "timeout", startedAtMs: 0, endedAtMs: 2 * 60_000 + 5_000 }), t)).toBe("превышен срок 2 мин");
    expect(failReasonText(agentRun({ reason: "timeout" }), t)).toBe("превышен срок запуска");
    expect(failReasonText(agentRun({ reason: "cliNotFound" }), t)).toBe("программа не найдена: /opt/homebrew/bin/qwen");
    expect(failReasonText(agentRun({ reason: "cliNotFound", path: null }), t)).toBe("программа не найдена");
    expect(failReasonText(agentRun({ reason: "exit", exitCode: 55 }), t)).toBe("код выхода 55");
    expect(failReasonText(agentRun({ reason: "exit" }), t)).toBe("программа завершилась с ошибкой");
    expect(failReasonText(agentRun({ reason: "spawnFailed" }), t)).toBe("не удалось запустить программу");
    expect(failReasonText(agentRun({ reason: "limit" }), t)).toBe("достигнут предел вызовов или расходов");
    expect(failReasonText(agentRun({ reason: "notConnected" }), t)).toBe("агент не подключился к Glafira");
    expect(failReasonText(agentRun({ reason: "mcpOff" }), t)).toBe("MCP-сервер выключили во время запуска");
    expect(failReasonText(agentRun({ reason: "accessOff" }), t)).toBe("подключение закрыто для LLM-ассистентов");
    expect(failReasonText(agentRun({ reason: "policy" }), t)).toBe("запрещено политикой вашей организации");
    expect(failReasonText(agentRun({ reason: "notConfigured" }), t)).toBe("фоновый ассистент не настроен");
    expect(failReasonText(agentRun({ reason: null }), t)).toBe("неизвестная причина");
  });

  it("лишние инструменты агента (unsafeTools): имена из предупреждения «unsafe …»; без события init — свой текст", () => {
    expect(failReasonText(agentRun({ reason: "unsafeTools", warnings: ["unsafe run_shell_command,read_file"] }), t)).toBe(
      "агенту доступны не только инструменты Glafira: run_shell_command, read_file; запуск остановлен до первого хода",
    );
    expect(failReasonText(agentRun({ reason: "unsafeTools", warnings: ["unsafe no-init"] }), t)).toBe("агент не сообщил список своих инструментов; запуск остановлен");
    // Предупреждения нет (старая запись) — без имён.
    expect(failReasonText(agentRun({ reason: "unsafeTools" }), t)).toBe("агенту доступны не только инструменты Glafira; запуск остановлен до первого хода");
  });

  it("предупреждения: вытеснение модели и лишние инструменты — текстом словаря; незнакомое — как прислал Rust", () => {
    expect(warningText("evict qwen3.8:27b,llama3.3:70b qwen3.6-64k:35b-a3b", t)).toBe(
      "Ollama может выгрузить qwen3.8:27b, llama3.3:70b, чтобы загрузить qwen3.6-64k:35b-a3b",
    );
    expect(warningText("unsafe Bash", t)).toBe("агенту доступны не только инструменты Glafira: Bash; запуск остановлен до первого хода");
    expect(warningText("unsafe no-init", t)).toBe("агент не сообщил список своих инструментов; запуск остановлен");
    expect(warningText("something else", t)).toBe("something else");
  });

  it("английские тексты причин и предупреждений", () => {
    const en = i18n.getFixedT("en");
    expect(failReasonText(agentRun({ reason: "timeout", startedAtMs: 0, endedAtMs: 15 * 60_000 + 4_000 }), en)).toBe("the 15 min time limit was exceeded");
    expect(failReasonText(agentRun({ reason: "unsafeTools", warnings: ["unsafe run_shell_command"] }), en)).toBe(
      "the agent can use more than Glafira's tools: run_shell_command; the run was stopped before its first step",
    );
    expect(warningText("unsafe no-init", en)).toBe("the agent did not report its tools; the run was stopped");
    expect(warningText("evict a b", en)).toBe("Ollama may unload a to load b");
  });
});
