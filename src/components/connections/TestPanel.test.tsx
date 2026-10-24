import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TestEvent, TestReport } from "../../api/types";
import { DICTIONARIES } from "../../i18n";
import { TestPanel } from "./TestPanel";

const ev = (patch: Partial<TestEvent>): TestEvent => ({ step: "node", member: null, status: "running", message: null, error: null, ...patch });

const FIXES: TestReport = {
  ok: true,
  topology: null,
  readiness: [
    { id: "pgssInstalled", status: "missing", params: { database: "postgres" }, fix: "CREATE EXTENSION IF NOT EXISTS pg_stat_statements;" },
    { id: "extension", status: "warn", params: { name: "hypopg" }, fix: "CREATE EXTENSION IF NOT EXISTS hypopg;" },
  ],
};

/** Подменяет буфер обмена (user-event.setup() ставит свою заглушку — подменять нужно после него). */
function stubClipboard(clipboard: { writeText: (t: string) => Promise<void> } | undefined) {
  Object.defineProperty(navigator, "clipboard", { value: clipboard, configurable: true });
}

describe("TestPanel", () => {
  afterEach(() => stubClipboard(undefined));

  it("показывает последнее состояние каждого шага", () => {
    render(
      <TestPanel
        events={[
          ev({ step: "patroni", status: "skipped" }),
          ev({ member: "db1:5432", status: "running" }),
          ev({ member: "db1:5432", status: "ok", message: { key: "test.nodeOk", params: { role: "primary", version: "16.4" }, text: "primary, PostgreSQL 16.4" } }),
          ev({ member: "db2:5432", status: "failed", error: { code: "AUTH_FAILED", key: "errors.pg.authFailed", message: "Authentication failed", retryable: false } }),
        ]}
        report={null}
      />,
    );
    const items = screen.getAllByRole("listitem");
    expect(items).toHaveLength(3);
    expect(items[1]).toHaveTextContent("primary, PostgreSQL 16.4");
    expect(items[2]).toHaveTextContent("Аутентификация не прошла");
  });

  it("шаг SSH идёт первым: версия OpenSSH, ошибка туннеля, «не используется» при прямом подключении", () => {
    const { unmount } = render(
      <TestPanel events={[ev({ step: "ssh", status: "ok", message: { key: "test.sshOk", params: { version: "9.6" }, text: "OpenSSH 9.6, tunnel is up" } })]} report={null} />,
    );
    expect(screen.getByRole("listitem")).toHaveTextContent("SSH: OpenSSH 9.6, туннель поднят");
    unmount();

    const noVersion = render(<TestPanel events={[ev({ step: "ssh", status: "ok", message: { key: "test.sshOkNoVersion", params: {}, text: "Tunnel is up" } })]} report={null} />);
    expect(screen.getByRole("listitem")).toHaveTextContent("SSH: Туннель поднят");
    noVersion.unmount();

    const failed = render(
      <TestPanel
        events={[
          ev({ step: "ssh", status: "running" }),
          ev({ step: "ssh", status: "failed", error: { code: "SSH_ERROR", key: "errors.ssh.authFailed", message: "SSH login refused", retryable: false, params: { kind: "authFailed", target: "bastion" } } }),
        ]}
        report={null}
      />,
    );
    expect(screen.getByRole("img", { name: "Ошибка" })).toBeInTheDocument();
    expect(screen.getByRole("listitem")).toHaveTextContent("SSH: Вход по SSH на bastion отклонён");
    failed.unmount();

    render(<TestPanel events={[ev({ step: "ssh", status: "skipped", message: { key: "test.sshNotUsed", params: {}, text: "Not used: direct connection" } })]} report={null} />);
    expect(screen.getByRole("listitem")).toHaveTextContent("SSH: Не используется: прямое подключение");
  });

  it("шаги с числом узлов склоняются: 1 узел, 5 узлов; участников 3, лидер назван", () => {
    const one = ev({ step: "discovery", status: "ok", message: { key: "test.discoveryOk", params: { count: "1" }, text: "1 nodes found" } });
    const five = ev({ step: "discovery", status: "ok", message: { key: "test.discoveryOk", params: { count: "5" }, text: "5 nodes found" } });
    const members = ev({ step: "patroni", status: "ok", message: { key: "test.patroniOk", params: { count: "3", leader: "pg-1" }, text: "3 members, leader pg-1" } });
    const { unmount } = render(<TestPanel events={[one]} report={null} />);
    expect(screen.getByRole("listitem")).toHaveTextContent("Найден 1 узел");
    unmount();
    const second = render(<TestPanel events={[five]} report={null} />);
    expect(screen.getByRole("listitem")).toHaveTextContent("Найдено 5 узлов");
    second.unmount();
    render(<TestPanel events={[members]} report={null} />);
    expect(screen.getByRole("listitem")).toHaveTextContent("3 участника, лидер pg-1");
  });

  it("чек-лист с командами для исправления", () => {
    const report: TestReport = {
      ok: true,
      topology: null,
      readiness: [
        { id: "monitorRole", status: "ok", params: {}, fix: null },
        { id: "pgssInstalled", status: "missing", params: { database: "postgres" }, fix: "CREATE EXTENSION IF NOT EXISTS pg_stat_statements;" },
        { id: "extension", status: "warn", params: { name: "hypopg" }, fix: "CREATE EXTENSION IF NOT EXISTS hypopg;" },
      ],
    };
    render(<TestPanel events={[]} report={report} />);
    expect(screen.getByText("CREATE EXTENSION IF NOT EXISTS pg_stat_statements;")).toBeInTheDocument();
    expect(screen.getByText(/hypopg/, { selector: "strong" })).toBeInTheDocument();
    expect(screen.getByText(/postgres/)).toBeInTheDocument();
  });

  it("роль без прав: команда GRANT с именем в кавычках и скрипт роли", () => {
    const report: TestReport = {
      ok: true,
      topology: null,
      readiness: [{ id: "monitorRole", status: "missing", params: {}, fix: 'GRANT pg_monitor TO "glafira_plain";' }],
    };
    render(<TestPanel events={[]} report={report} />);
    expect(screen.getByText('GRANT pg_monitor TO "glafira_plain";')).toBeInTheDocument();
    expect(screen.getByText("Скрипт роли для диагностики")).toBeInTheDocument();
  });

  it("pgssPreloaded: «неизвестно» объясняет обе причины", () => {
    const report: TestReport = {
      ok: true,
      topology: null,
      readiness: [{ id: "pgssPreloaded", status: "warn", params: { reason: "unknown" }, fix: null }],
    };
    render(<TestPanel events={[]} report={report} />);
    const text = screen.getByText(/Не удалось проверить/).textContent ?? "";
    expect(text).toContain("нет доступных узлов");
    expect(text).toContain("pg_monitor");
    expect(text).toContain("pg_read_all_settings");
  });

  it("предупреждения топологии", () => {
    const report: TestReport = {
      ok: true,
      topology: {
        connectionId: "c",
        kind: "hostList",
        observedAtMs: 0,
        scope: null,
        paused: null,
        members: [],
        warnings: [{ kind: "multiplePrimaries", members: ["a", "b"] }, { kind: "memberUnreachable", name: "c" }],
      },
      readiness: [],
    };
    render(<TestPanel events={[]} report={report} />);
    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Несколько primary (a, b)");
    expect(alert).toHaveTextContent("Узел c недоступен");
  });

  it("в обоих словарях есть тексты для всех пунктов чек-листа", () => {
    // Сочетания (id, вариант), которые выдаёт pg_advisor::readiness::evaluate: вариант — params.reason, без него — статус.
    const variants: Record<string, string[]> = {
      monitorRole: ["ok", "missing"],
      signalRole: ["ok", "missing"],
      maintainRole: ["ok", "grant", "ownerOnly"],
      pgssPreloaded: ["ok", "missing", "warn", "unknown"],
      pgssInstalled: ["ok", "missing"],
      computeQueryId: ["ok", "warn", "missing", "version"],
      trackIoTiming: ["ok", "warn"],
      trackActivityQuerySize: ["ok", "warn"],
      extension: ["ok", "warn", "missing"],
    };
    const missing: string[] = [];
    for (const lng of ["en", "ru"])
      for (const [id, list] of Object.entries(variants))
        for (const k of ["title", "affects", ...list]) {
          const key = `readiness.${id}.${k}`;
          // Ищем в словаре языка напрямую: i18n.exists засчитал бы запасной язык (en) за перевод.
          const value = key.split(".").reduce<unknown>((o, part) => (o as Record<string, unknown> | undefined)?.[part], DICTIONARIES[lng]);
          if (typeof value !== "string") missing.push(`${lng}: ${key}`);
        }
    expect(missing).toEqual([]);
  });

  it("значки статусов читаются как текст", () => {
    render(
      <TestPanel
        events={[
          ev({ step: "patroni", status: "skipped" }),
          ev({ member: "db1:5432", status: "ok" }),
          ev({ member: "db2:5432", status: "failed", error: { code: "AUTH_FAILED", key: "errors.pg.authFailed", message: "x", retryable: false } }),
          ev({ member: "db3:5432", status: "running" }),
        ]}
        report={{
          ok: true,
          topology: null,
          readiness: [
            { id: "monitorRole", status: "ok", params: {}, fix: null },
            { id: "trackIoTiming", status: "warn", params: {}, fix: null },
            { id: "pgssInstalled", status: "missing", params: { database: "postgres" }, fix: null },
          ],
        }}
      />,
    );
    for (const name of ["Пропущено", "Успешно", "Ошибка", "Выполняется", "Готово", "Предупреждение", "Не хватает"]) {
      expect(screen.getByRole("img", { name })).toBeInTheDocument();
    }
  });

  it("кнопки «Копировать» различимы по имени", () => {
    render(<TestPanel events={[]} report={FIXES} />);
    const names = screen.getAllByRole("button", { name: /^Копировать/ }).map((b) => b.getAttribute("aria-label"));
    expect(new Set(names).size).toBe(2);
    expect(names[0]).toContain("Расширение pg_stat_statements");
    expect(names[1]).toContain("hypopg");
  });

  it("копирование: команда уходит в буфер, итог сообщается", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    stubClipboard({ writeText });
    render(<TestPanel events={[]} report={FIXES} />);
    await user.click(screen.getAllByRole("button", { name: /^Копировать/ })[1]);
    expect(writeText).toHaveBeenCalledWith("CREATE EXTENSION IF NOT EXISTS hypopg;");
    expect(await screen.findByText("Скопировано")).toBeInTheDocument();
    // Сообщение — в живой области (скринридер объявит), и только у нажатой кнопки.
    expect(screen.getByText("Скопировано")).toHaveAttribute("role", "status");
  });

  it("копирование: отказ буфера обмена не остаётся необработанным, пользователь видит сообщение", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockRejectedValue(new DOMException("denied", "NotAllowedError"));
    stubClipboard({ writeText });
    render(<TestPanel events={[]} report={FIXES} />);
    await user.click(screen.getAllByRole("button", { name: /^Копировать/ })[0]);
    expect(await screen.findByText("Не удалось скопировать")).toBeInTheDocument();
    expect(screen.queryByText("Скопировано")).not.toBeInTheDocument();
  });

  it("копирование: буфера обмена нет (небезопасный контекст) — то же сообщение", async () => {
    const user = userEvent.setup();
    stubClipboard(undefined);
    render(<TestPanel events={[]} report={FIXES} />);
    await user.click(screen.getAllByRole("button", { name: /^Копировать/ })[0]);
    expect(await screen.findByText("Не удалось скопировать")).toBeInTheDocument();
  });
});
