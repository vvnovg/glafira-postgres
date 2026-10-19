import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderWithClient } from "../../test/render";
import { newProfile } from "./profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listFolders: vi.fn(() => Promise.resolve([])),
      getLicenseStatus: vi.fn(() => Promise.resolve({ tier: "free", state: "none" })),
      parseConnectionString: vi.fn(),
      saveConnection: vi.fn(),
      testConnection: vi.fn(),
      moveItems: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { ConnectionForm } from "./ConnectionForm";

beforeEach(() => vi.clearAllMocks());

describe("ConnectionForm", () => {
  it("вставленная строка заполняет вид, узлы и пароль", async () => {
    const user = userEvent.setup();
    vi.mocked(api.parseConnectionString).mockResolvedValue({
      patch: { kind: "hostList", hosts: [{ host: "h1", port: 5432 }, { host: "h2", port: 5433 }], targetServerType: "primary", username: "app", database: "orders", sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: "pw",
      ignored: ["ApplicationName"],
    });
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.type(screen.getByLabelText("Строка подключения"), "jdbc:postgresql://h1,h2:5433/orders");
    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByLabelText("Узел 2")).toHaveValue("h2");
    expect(screen.getByLabelText("Порт узла 2")).toHaveValue(5433);
    expect(screen.getByLabelText("Пользователь")).toHaveValue("app");
    expect(screen.getByText(/ApplicationName/)).toBeInTheDocument();

    vi.mocked(api.saveConnection).mockResolvedValue({ ...newProfile(), id: "n" });
    await user.type(screen.getByLabelText("Имя"), "prod-orders");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const [profile, secrets] = vi.mocked(api.saveConnection).mock.calls[0];
    expect(profile.kind).toBe("hostList");
    expect(secrets.pgPassword).toBe("pw");
  });

  it("узлы списка переводятся под Patroni", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), kind: "hostList", hosts: [{ host: "db1", port: 5432 }] }} tabId="t" />);
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    expect(screen.getByLabelText("Адрес API 1")).toHaveValue("http://db1:8008");
    expect(screen.getByRole("radio", { name: "Кластер Patroni" })).toBeChecked();
  });

  it("prod недоступен во Free", async () => {
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    const prod = await screen.findByRole("option", { name: /prod/ });
    await waitFor(() => expect(prod).toBeDisabled());
  });

  it("проверка: шаги и чек-лист приходят по мере выполнения, секреты уходят из формы", async () => {
    const user = userEvent.setup();
    vi.mocked(api.testConnection).mockImplementation(async (_profile, _secrets, onEvent) => {
      onEvent({ step: "patroni", member: null, status: "skipped", message: null, error: null });
      onEvent({
        step: "node",
        member: "localhost:5432",
        status: "ok",
        message: { key: "test.nodeOk", params: { role: "primary", version: "17.2" }, text: "primary, PostgreSQL 17.2" },
        error: null,
      });
      return {
        ok: true,
        topology: null,
        readiness: [{ id: "monitorRole", status: "missing", params: {}, fix: 'GRANT pg_monitor TO "glafira_plain";' }],
      };
    });
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.type(screen.getByLabelText("Пароль"), "s3cret");
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    expect(await screen.findByText("primary, PostgreSQL 17.2")).toBeInTheDocument();
    expect(screen.getByText('GRANT pg_monitor TO "glafira_plain";')).toBeInTheDocument();
    const [, secrets] = vi.mocked(api.testConnection).mock.calls[0];
    expect(secrets).toEqual({ pgPassword: "s3cret", pgKeyPassword: null, patroniPassword: null, patroniKeyPassword: null });
  });

  it("существующее подключение: пароль можно оставить или удалить", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue({ ...newProfile(), id: "c1" });
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), id: "c1", name: "dev-1", environment: "dev" }} tabId="t" />);
    expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "оставить сохранённый");
    await user.click(screen.getByLabelText("Удалить сохранённый"));
    expect(screen.getByLabelText("Пароль")).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(vi.mocked(api.saveConnection).mock.calls[0][1].pgPassword).toBe("");
  });

  it("prod, только чтение и Free: флаг заблокирован, пока окружение остаётся prod", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), id: "c1", name: "p", environment: "prod", readOnly: true }} tabId="t" />);
    const ro = screen.getByLabelText("Только чтение");
    await waitFor(() => expect(ro).toBeDisabled());
    expect(ro).toBeChecked();
    await user.selectOptions(screen.getByLabelText("Окружение"), "dev");
    expect(ro).toBeEnabled();
  });
});
