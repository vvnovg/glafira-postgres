import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AppError, ConnectionFolder, ConnectionProfile, SecretsSet, TestReport } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { topologyView } from "../../test/fixtures";
import { renderWithClient } from "../../test/render";
import { emptyPatroni, newProfile } from "./profile";

vi.mock("../../api/client", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../api/client")>();
  return {
    ...orig,
    api: {
      listFolders: vi.fn(),
      parseConnectionString: vi.fn(),
      saveConnection: vi.fn(),
      deleteConnection: vi.fn(),
      testConnection: vi.fn(),
      moveItems: vi.fn(),
      connectionSecrets: vi.fn(),
    },
  };
});

import { api } from "../../api/client";
import { ConnectionForm } from "./ConnectionForm";

const FOLDERS: ConnectionFolder[] = [
  { id: "f1", name: "Прод", parentId: null, order: 0 },
  { id: "f2", name: "Тест", parentId: null, order: 1 },
];
const EMPTY_REPORT: TestReport = { ok: true, topology: null, readiness: [] };
const appError = (code: string, message: string, key?: string): AppError => ({ code, key, message, retryable: false }) as AppError;
const saved = (id: string): ConnectionProfile => ({ ...newProfile(), id });
/** Существующее подключение (id задан), по умолчанию dev. */
const existing = (patch: Partial<ConnectionProfile> = {}): ConnectionProfile => ({ ...newProfile(), id: "c1", name: "dev-1", environment: "dev", ...patch });

const NO_SECRETS: SecretsSet = { pgPassword: false, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false };
const ALL_SECRETS: SecretsSet = { pgPassword: true, pgKeyPassword: true, patroniPassword: true, patroniKeyPassword: true };

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => (resolve = r));
  return { promise, resolve };
}

beforeEach(() => {
  vi.resetAllMocks(); // сбрасывает и реализации: ответы не перетекают между тестами
  vi.mocked(api.listFolders).mockResolvedValue([]);
  vi.mocked(api.connectionSecrets).mockResolvedValue(NO_SECRETS);
  useWorkspace.setState({ tabs: [], activeId: null });
});

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

  it("prod доступен новому подключению и подписан просто «prod»", async () => {
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    const prod = await screen.findByRole("option", { name: "prod" });
    expect(prod).toBeEnabled();
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

  it("существующее подключение: сохранённый пароль можно оставить или удалить", async () => {
    const user = userEvent.setup();
    vi.mocked(api.connectionSecrets).mockResolvedValue({ ...NO_SECRETS, pgPassword: true });
    vi.mocked(api.saveConnection).mockResolvedValue({ ...newProfile(), id: "c1" });
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), id: "c1", name: "dev-1", environment: "dev" }} tabId="t" />);
    await waitFor(() => expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "оставить сохранённый"));
    await user.click(screen.getByLabelText("Удалить сохранённый: Пароль"));
    expect(screen.getByLabelText("Пароль")).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(vi.mocked(api.saveConnection).mock.calls[0][1].pgPassword).toBe("");
  });

  it("у существующего prod-подключения «Только чтение» доступно и снимается", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), id: "c1", name: "p", environment: "prod", readOnly: true }} tabId="t" />);
    const ro = screen.getByLabelText("Только чтение");
    expect(ro).toBeChecked();
    expect(ro).toBeEnabled();
    await user.click(ro);
    expect(ro).not.toBeChecked();
  });

  it("после разбора строка подключения очищается (пароль не остаётся открытым), список пропущенного виден", async () => {
    const user = userEvent.setup();
    vi.mocked(api.parseConnectionString).mockResolvedValue({
      patch: { kind: "server", hosts: [{ host: "h1", port: 5432 }], targetServerType: null, username: "app", database: null, sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: "topsecret",
      ignored: ["ApplicationName"],
    });
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    const connstr = screen.getByLabelText("Строка подключения");
    await user.type(connstr, "postgresql://app:topsecret@h1/db");
    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByText(/ApplicationName/)).toBeInTheDocument();
    expect(connstr).toHaveValue("");
    // Пароль лежит только в поле пароля (type=password).
    expect(screen.getByLabelText("Пароль")).toHaveAttribute("type", "password");
    expect(screen.getByLabelText("Пароль")).toHaveValue("topsecret");
    expect(screen.queryByDisplayValue(/postgresql:\/\//)).not.toBeInTheDocument();
  });

  it("неудачный разбор оставляет строку для правки и убирает прежний список пропущенного", async () => {
    const user = userEvent.setup();
    vi.mocked(api.parseConnectionString).mockResolvedValueOnce({
      patch: { kind: "server", hosts: [{ host: "h1", port: 5432 }], targetServerType: null, username: null, database: null, sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: null,
      ignored: ["ApplicationName"],
    });
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.type(screen.getByLabelText("Строка подключения"), "postgresql://h1/db");
    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByText(/ApplicationName/)).toBeInTheDocument();

    vi.mocked(api.parseConnectionString).mockRejectedValueOnce(appError("INVALID_INPUT", "Cannot parse the connection string"));
    await user.type(screen.getByLabelText("Строка подключения"), "garbage");
    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Cannot parse the connection string");
    expect(screen.queryByText(/ApplicationName/)).not.toBeInTheDocument();
    expect(screen.getByLabelText("Строка подключения")).toHaveValue("garbage");
  });

  describe("несколько форм в одном окне", () => {
    const two = () =>
      renderWithClient(
        <>
          <ConnectionForm initial={{ ...newProfile(), name: "alpha" }} tabId="a" />
          <ConnectionForm initial={{ ...newProfile(), name: "beta" }} tabId="b" />
        </>,
      );

    it("подпись привязана к полю своей формы", async () => {
      const user = userEvent.setup();
      two();
      expect(screen.getAllByLabelText("Только чтение")).toHaveLength(2);
      const [roA, roB] = screen.getAllByLabelText("Только чтение");
      await user.click(screen.getAllByText("Только чтение", { selector: "label" })[1]);
      expect(roB).toBeChecked();
      expect(roA).not.toBeChecked();

      const [nameA, nameB] = screen.getAllByLabelText("Имя");
      expect(nameA).toHaveValue("alpha");
      expect(nameB).toHaveValue("beta");
      await user.type(nameB, "-2");
      expect(nameA).toHaveValue("alpha");
      expect(nameB).toHaveValue("beta-2");
    });

    it("вид подключения переключается независимо (у радиокнопок своё имя группы)", async () => {
      const user = userEvent.setup();
      two();
      await user.click(screen.getAllByRole("radio", { name: "Список узлов" })[1]);
      expect(screen.getAllByRole("radio", { name: "Список узлов" })[1]).toBeChecked();
      expect(screen.getAllByRole("radio", { name: "Сервер" })[0]).toBeChecked();
      expect(screen.getAllByRole("radio", { name: "Список узлов" })[0]).not.toBeChecked();
    });

    it("id полей не повторяются, даже когда раскрыты все блоки (Patroni + TLS API + TLS PostgreSQL)", () => {
      const cert = { format: "pem" as const, certificatePath: "/c.pem", keyPath: "/k.pem" };
      const full = (name: string): ConnectionProfile => ({
        ...existing({ name }),
        kind: "patroni",
        hosts: [],
        patroni: { ...emptyPatroni(), username: "api", tls: { caPath: "/ca.pem", verifyHostname: true, clientCert: cert } },
        pg: { ...newProfile().pg, tls: { caPath: "/ca.pem", clientCert: cert } },
      });
      renderWithClient(
        <>
          <ConnectionForm initial={full("a")} tabId="a" />
          <ConnectionForm initial={full("b")} tabId="b" />
        </>,
      );
      const ids = [...document.querySelectorAll("[id]")].map((e) => e.id);
      expect(ids.length).toBeGreaterThan(30);
      expect(new Set(ids).size).toBe(ids.length);
      // Каждая подпись указывает ровно на одно поле своей формы.
      for (const label of document.querySelectorAll("label[for]")) {
        expect(document.querySelectorAll(`[id="${label.getAttribute("for")}"]`)).toHaveLength(1);
      }
      expect(screen.getAllByRole("group", { name: "TLS для API" })).toHaveLength(2);
    });
  });

  it("доступные имена в одной форме различимы: группы TLS, выбор файла, удаление сохранённого", async () => {
    vi.mocked(api.connectionSecrets).mockResolvedValue(ALL_SECRETS);
    const cert = { format: "pem" as const, certificatePath: "/c.pem", keyPath: "/k.pem" };
    renderWithClient(
      <ConnectionForm
        initial={{
          ...existing(),
          kind: "patroni",
          hosts: [],
          patroni: { ...emptyPatroni(), username: "api", tls: { caPath: "/ca.pem", verifyHostname: true, clientCert: cert } },
          pg: { ...newProfile().pg, tls: { caPath: "/ca.pem", clientCert: cert } },
        }}
        tabId="t"
      />,
    );
    const api_ = screen.getByRole("group", { name: "TLS для API" });
    const pg = screen.getByRole("group", { name: "TLS PostgreSQL" });
    // Подписи в группах те же, но принадлежат разным группам.
    for (const group of [api_, pg]) {
      for (const label of ["Доверенный CA", "Клиентский сертификат", "Формат", "Сертификат", "Ключ", "Пароль ключа"]) {
        expect(within(group).getByLabelText(label)).toBeInTheDocument();
      }
    }
    // Кнопки выбора файла и флажки «Удалить сохранённый» различимы и без группы.
    const names = (re: RegExp, role: "button" | "checkbox") => screen.getAllByRole(role, { name: re }).map((e) => e.getAttribute("aria-label"));
    const picks = names(/^Выбрать файл/, "button");
    expect(picks).toHaveLength(6);
    expect(new Set(picks).size).toBe(6);
    expect(picks).toContain("Выбрать файл: Доверенный CA (TLS для API)");
    expect(picks).toContain("Выбрать файл: Ключ (TLS PostgreSQL)");
    // Флажки «Удалить сохранённый» появляются, когда ответ о сохранённых секретах получен.
    await waitFor(() => expect(screen.getAllByRole("checkbox", { name: /^Удалить сохранённый/ })).toHaveLength(4));
    const clears = names(/^Удалить сохранённый/, "checkbox");
    expect(clears).toHaveLength(4);
    expect(new Set(clears).size).toBe(4);
    expect(clears).toEqual(
      expect.arrayContaining([
        "Удалить сохранённый: Пароль",
        "Удалить сохранённый: Пароль API",
        "Удалить сохранённый: Пароль ключа (TLS для API)",
        "Удалить сохранённый: Пароль ключа (TLS PostgreSQL)",
      ]),
    );
  });

  it("смена вида не теряет список узлов и адреса API", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), kind: "hostList", hosts: [{ host: "a", port: 5432 }, { host: "b", port: 5433 }, { host: "c", port: 5434 }] }} tabId="t" />);
    await user.click(screen.getByRole("radio", { name: "Сервер" }));
    expect(screen.getByLabelText("Хост")).toHaveValue("a");
    await user.click(screen.getByRole("radio", { name: "Список узлов" }));
    expect(screen.getByLabelText("Узел 3")).toHaveValue("c");
    expect(screen.getByLabelText("Порт узла 2")).toHaveValue(5433);

    // Адреса API вычислены из узлов — пересчитываются; исправленные вручную остаются.
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    expect(screen.getByLabelText("Адрес API 3")).toHaveValue("http://c:8008");
    await user.click(screen.getByRole("radio", { name: "Список узлов" }));
    await user.click(screen.getByRole("button", { name: "Добавить узел" }));
    await user.type(screen.getByLabelText("Узел 4"), "d");
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    expect(screen.getByLabelText("Адрес API 4")).toHaveValue("http://d:8008");

    await user.clear(screen.getByLabelText("Адрес API 1"));
    await user.type(screen.getByLabelText("Адрес API 1"), "https://pg-a:8443");
    await user.click(screen.getByRole("radio", { name: "Список узлов" }));
    await user.type(screen.getByLabelText("Узел 4"), "e");
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    expect(screen.getByLabelText("Адрес API 1")).toHaveValue("https://pg-a:8443");
    expect(screen.queryByLabelText("Адрес API 5")).not.toBeInTheDocument();
  });

  it("sslmode=disable: скрытые поля TLS не отправляются и не требуют пароль ключа", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    const initial = existing({
      pg: { ...newProfile().pg, username: "u", tls: { caPath: "/ca.pem", clientCert: { format: "pem", certificatePath: "", keyPath: "" } } },
    });
    renderWithClient(<ConnectionForm initial={initial} tabId="t" />);
    await user.type(screen.getByLabelText("Пароль ключа"), "kp");
    await user.selectOptions(screen.getByLabelText("sslmode"), "disable");
    expect(screen.queryByRole("group", { name: "TLS PostgreSQL" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const [profile, secrets] = vi.mocked(api.saveConnection).mock.calls[0];
    expect(profile.pg.sslMode).toBe("disable");
    expect(profile.pg.tls).toEqual({ caPath: null, clientCert: null });
    // Значение поля уходит как есть: у профиля без сертификата Rust удаляет пароль ключа сам.
    expect(secrets.pgKeyPassword).toBe("kp");
  });

  it("sslmode=disable: введённое в TLS не пропадает при возврате на другой режим, а проверка получает тот же профиль, что и сохранение", async () => {
    const user = userEvent.setup();
    vi.mocked(api.testConnection).mockResolvedValue(EMPTY_REPORT);
    renderWithClient(<ConnectionForm initial={existing({ pg: { ...newProfile().pg, tls: { caPath: "/ca.pem", clientCert: null } } })} tabId="t" />);
    await user.selectOptions(screen.getByLabelText("sslmode"), "disable");
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    await waitFor(() => expect(api.testConnection).toHaveBeenCalled());
    expect(vi.mocked(api.testConnection).mock.calls[0][0].pg.tls.caPath).toBeNull();
    await user.selectOptions(screen.getByLabelText("sslmode"), "verify-full");
    expect(screen.getByLabelText("Доверенный CA")).toHaveValue("/ca.pem");
  });

  it("долгая проверка не блокирует «Заполнить» и «Сохранить», но второй запуск проверки невозможен", async () => {
    const user = userEvent.setup();
    const test = deferred<TestReport>();
    vi.mocked(api.testConnection).mockReturnValue(test.promise);
    vi.mocked(api.saveConnection).mockResolvedValue(saved("n"));
    vi.mocked(api.parseConnectionString).mockResolvedValue({
      patch: { kind: "server", hosts: [{ host: "h9", port: 5432 }], targetServerType: null, username: null, database: null, sslMode: null, caPath: null, clientCert: null, connectTimeoutS: null },
      password: null,
      ignored: [],
    });
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), name: "x" }} tabId="t" />);
    await user.type(screen.getByLabelText("Строка подключения"), "postgresql://h9/db");
    const testBtn = screen.getByRole("button", { name: "Проверить" });
    await user.click(testBtn);
    await waitFor(() => expect(testBtn).toBeDisabled());
    expect(api.testConnection).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Заполнить" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "Заполнить" }));
    expect(await screen.findByLabelText("Хост")).toHaveValue("h9");
    await user.click(testBtn); // отключена — запуск не происходит
    expect(api.testConnection).toHaveBeenCalledTimes(1);

    test.resolve(EMPTY_REPORT);
    await waitFor(() => expect(testBtn).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalledTimes(1));
  });

  it("ошибка проверки показывается, кнопка снова доступна", async () => {
    const user = userEvent.setup();
    vi.mocked(api.testConnection).mockRejectedValue(appError("AUTH_FAILED", "Authentication failed", "errors.pg.authFailed"));
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Аутентификация не прошла");
    expect(screen.getByRole("button", { name: "Проверить" })).toBeEnabled();
  });

  describe("папка и перенос", () => {
    const pick = async (user: ReturnType<typeof userEvent.setup>, value: string) => {
      await screen.findByRole("option", { name: "Тест" });
      await user.selectOptions(screen.getByLabelText("Папка"), value);
    };
    const saveClick = (user: ReturnType<typeof userEvent.setup>) => user.click(screen.getByRole("button", { name: "Сохранить" }));

    beforeEach(() => {
      vi.mocked(api.listFolders).mockResolvedValue(FOLDERS);
      vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
      vi.mocked(api.moveItems).mockResolvedValue(undefined);
    });

    it("папка изменена — подключение переносится", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ folderId: "f1" })} tabId="t" />);
      await pick(user, "f2");
      await saveClick(user);
      await waitFor(() => expect(api.moveItems).toHaveBeenCalledTimes(1));
      expect(api.moveItems).toHaveBeenCalledWith([{ kind: "connection", id: "c1" }], "f2", null);
      expect(vi.mocked(api.saveConnection).mock.calls[0][0].folderId).toBe("f2");
    });

    it("перенос в корень", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ folderId: "f1" })} tabId="t" />);
      await pick(user, "");
      await saveClick(user);
      await waitFor(() => expect(api.moveItems).toHaveBeenCalledWith([{ kind: "connection", id: "c1" }], null, null));
    });

    it("папку не трогали — перенос не вызывается", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ folderId: "f1" })} tabId="t" />);
      await screen.findByRole("option", { name: "Тест" });
      await saveClick(user);
      await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
      await waitFor(() => expect(useWorkspace.getState().tabs.map((x) => x.id)).toContain("overview:c1"));
      expect(api.moveItems).not.toHaveBeenCalled();
    });

    it("папку сменили и вернули обратно — переносить нечего", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ folderId: "f1" })} tabId="t" />);
      await pick(user, "f2");
      await pick(user, "f1");
      await saveClick(user);
      await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
      await waitFor(() => expect(useWorkspace.getState().tabs.map((x) => x.id)).toContain("overview:c1"));
      expect(api.moveItems).not.toHaveBeenCalled();
    });

    it("новое подключение попадает в папку самим сохранением, без переноса", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={{ ...newProfile(), name: "n", folderId: "f1" }} tabId="t" />);
      await screen.findByRole("option", { name: "Тест" });
      await saveClick(user);
      await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
      expect(vi.mocked(api.saveConnection).mock.calls[0][0].folderId).toBe("f1");
      expect(api.moveItems).not.toHaveBeenCalled();
    });

    it("перенос не удался после сохранения: кэш обновлён, ошибка показана, вкладка остаётся", async () => {
      const user = userEvent.setup();
      vi.mocked(api.moveItems).mockRejectedValue(appError("INTERNAL", "Cannot move the item"));
      const { client } = renderWithClient(<ConnectionForm initial={existing({ folderId: "f1" })} tabId="connection-form:c1" />);
      client.setQueryData(["connections"], []);
      client.setQueryData(["catalog", "c1", "databases"], []);
      client.setQueryData(["catalog", "other", "databases"], []);
      useWorkspace.getState().open({ kind: "connection-form", connectionId: "c1" });
      const invalidate = vi.spyOn(client, "invalidateQueries");
      await pick(user, "f2");
      await saveClick(user);
      expect(await screen.findByRole("alert")).toHaveTextContent("Cannot move the item");
      expect(invalidate).toHaveBeenCalledWith({ queryKey: ["connections"] });
      expect(invalidate).toHaveBeenCalledWith({ queryKey: ["topology", "c1"], exact: true });
      expect(client.getQueryData(["catalog", "other", "databases"])).toEqual([]);
      expect(useWorkspace.getState().tabs.map((x) => x.id)).toEqual(["connection-form:c1"]);
      // Кнопки снова доступны: повторное сохранение повторит перенос.
      expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
    });
  });

  it("успешное сохранение закрывает форму, открывает обзор и перечитывает вид подключения, не удаляя его кэш", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    const { client } = renderWithClient(<ConnectionForm initial={existing()} tabId="connection-form:c1" />);
    const live = topologyView({ state: "connected" });
    client.setQueryData(["topology", "c1"], live);
    client.setQueryData(["topologyEvents", "c1"], []);
    client.setQueryData(["catalog", "c1", "databases"], []);
    useWorkspace.getState().open({ kind: "connection-form", connectionId: "c1" });
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(useWorkspace.getState().tabs.map((x) => x.id)).toEqual(["overview:c1"]));
    // Правка оформления подключение не закрывает (Rust): вид и журнал помечены устаревшими, но остаются в кэше —
    // дерево и обзор не теряют подключение и раскрытый каталог.
    expect(client.getQueryData(["topology", "c1"])).toEqual(live);
    expect(client.getQueryState(["topology", "c1"])!.isInvalidated).toBe(true);
    expect(client.getQueryState(["topologyEvents", "c1"])!.isInvalidated).toBe(true);
    expect(client.getQueryData(["catalog", "c1", "databases"])).toEqual([]);
  });

  it.each(["70000", "0", ""])("порт «%s» не уходит в Rust: ни сохранения, ни проверки, понятная ошибка", async (port) => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    const input = screen.getByRole("spinbutton", { name: "Порт" });
    await user.clear(input);
    if (port) await user.type(input, port);
    expect(input).toHaveAttribute("aria-invalid", "true");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Порт — от 1 до 65535");
    await user.click(screen.getByRole("button", { name: "Проверить" }));
    expect(api.saveConnection).not.toHaveBeenCalled();
    expect(api.testConnection).not.toHaveBeenCalled();
  });

  it("порт узла списка и подмена порта Patroni проверяются так же; пустая подмена — «не менять»", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    const pt = { ...emptyPatroni(), hostOverrides: [{ member: "pg-1", host: null, port: 99999, apiUrl: null }] };
    renderWithClient(<ConnectionForm initial={existing({ kind: "patroni", hosts: [], patroni: pt })} tabId="t" />);
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Порт — от 1 до 65535");
    expect(api.saveConnection).not.toHaveBeenCalled();
    await user.clear(screen.getByRole("spinbutton", { name: "Порт PostgreSQL 1" }));
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
    expect(vi.mocked(api.saveConnection).mock.calls[0][0].patroni!.hostOverrides[0].port).toBeNull();
  });

  it("ошибка сохранения: баннер с причиной, форма остаётся открытой", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockRejectedValue(appError("INVALID_INPUT", "Connection name cannot be empty", "errors.connections.emptyName"));
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="connection-form:new" />);
    useWorkspace.getState().open({ kind: "connection-form", connectionId: null });
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Имя подключения не может быть пустым");
    expect(alert).toHaveTextContent("INVALID_INPUT");
    expect(useWorkspace.getState().tabs.map((x) => x.id)).toEqual(["connection-form:new"]);
    expect(screen.getByRole("button", { name: "Сохранить" })).toBeEnabled();
  });

  describe("удаление", () => {
    it("подтверждение → удаление, кэш подключения очищен, вкладки закрыты", async () => {
      const user = userEvent.setup();
      vi.mocked(api.deleteConnection).mockResolvedValue(undefined);
      const { client } = renderWithClient(<ConnectionForm initial={existing()} tabId="connection-form:c1" />);
      client.setQueryData(["catalog", "c1", "databases"], []);
      useWorkspace.getState().open({ kind: "connection-form", connectionId: "c1" });
      useWorkspace.getState().open({ kind: "overview", connectionId: "c1" });
      await user.click(screen.getByRole("button", { name: "Удалить" }));
      expect(api.deleteConnection).not.toHaveBeenCalled();
      const group = screen.getByRole("group", { name: "Удаление подключения" });
      expect(group).toHaveTextContent("Удалить подключение «dev-1»?");
      await user.click(within(group).getByRole("button", { name: "Удалить" }));
      await waitFor(() => expect(api.deleteConnection).toHaveBeenCalledWith("c1"));
      await waitFor(() => expect(useWorkspace.getState().tabs).toEqual([]));
      expect(client.getQueryData(["catalog", "c1", "databases"])).toBeUndefined();
    });

    it("«Отмена» возвращает обычную кнопку, ничего не удаляя", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
      await user.click(screen.getByRole("button", { name: "Удалить" }));
      await user.click(screen.getByRole("button", { name: "Отмена" }));
      expect(screen.queryByRole("group", { name: "Удаление подключения" })).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Удалить" })).toBeInTheDocument();
      expect(api.deleteConnection).not.toHaveBeenCalled();
    });

    it("ошибка удаления показана, вкладка остаётся", async () => {
      const user = userEvent.setup();
      vi.mocked(api.deleteConnection).mockRejectedValue(appError("INTERNAL", "Cannot delete the connection"));
      renderWithClient(<ConnectionForm initial={existing()} tabId="connection-form:c1" />);
      useWorkspace.getState().open({ kind: "connection-form", connectionId: "c1" });
      await user.click(screen.getByRole("button", { name: "Удалить" }));
      await user.click(within(screen.getByRole("group", { name: "Удаление подключения" })).getByRole("button", { name: "Удалить" }));
      expect(await screen.findByRole("alert")).toHaveTextContent("Cannot delete the connection");
      expect(useWorkspace.getState().tabs.map((x) => x.id)).toEqual(["connection-form:c1"]);
    });

    it("у нового подключения кнопки удаления нет", () => {
      renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
      expect(screen.queryByRole("button", { name: "Удалить" })).not.toBeInTheDocument();
    });
  });

  it("Patroni: пароль API и пароль ключа API попадают в секреты, PostgreSQL-секреты — отдельно", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    const cert = { format: "pem" as const, certificatePath: "/c.pem", keyPath: "/k.pem" };
    renderWithClient(
      <ConnectionForm
        initial={existing({
          kind: "patroni",
          hosts: [],
          patroni: { ...emptyPatroni(), username: "api", tls: { caPath: null, verifyHostname: true, clientCert: cert } },
        })}
        tabId="t"
      />,
    );
    await user.type(screen.getByLabelText("Пароль API"), "api-pw");
    await user.type(within(screen.getByRole("group", { name: "TLS для API" })).getByLabelText("Пароль ключа"), "api-key-pw");
    await user.type(screen.getByLabelText("Пароль"), "pg-pw");
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const [profile, secrets] = vi.mocked(api.saveConnection).mock.calls[0];
    expect(profile.kind).toBe("patroni");
    expect(secrets).toEqual({ pgPassword: "pg-pw", pgKeyPassword: null, patroniPassword: "api-pw", patroniKeyPassword: "api-key-pw" });
    // Секретов нет в профиле.
    expect(JSON.stringify(profile)).not.toMatch(/api-pw|api-key-pw|pg-pw/);
  });

  it("Patroni: «удалить сохранённый» пароль API и ключа API уходит как пустая строка", async () => {
    const user = userEvent.setup();
    vi.mocked(api.connectionSecrets).mockResolvedValue({ ...NO_SECRETS, patroniPassword: true, patroniKeyPassword: true });
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    const cert = { format: "pem" as const, certificatePath: "/c.pem", keyPath: "/k.pem" };
    renderWithClient(
      <ConnectionForm
        initial={existing({ kind: "patroni", hosts: [], patroni: { ...emptyPatroni(), username: "api", tls: { caPath: null, verifyHostname: true, clientCert: cert } } })}
        tabId="t"
      />,
    );
    await user.click(await screen.findByRole("checkbox", { name: "Удалить сохранённый: Пароль API" }));
    await user.click(screen.getByRole("checkbox", { name: "Удалить сохранённый: Пароль ключа (TLS для API)" }));
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    const secrets = vi.mocked(api.saveConnection).mock.calls[0][1];
    expect(secrets).toMatchObject({ patroniPassword: "", patroniKeyPassword: "", pgPassword: null });
  });

  it("«оставить сохранённый» и «Удалить сохранённый» показываются только у сохранённых секретов", async () => {
    vi.mocked(api.connectionSecrets).mockResolvedValue({ pgPassword: true, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false });
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "оставить сохранённый"));
    expect(screen.getByRole("checkbox", { name: /Удалить сохранённый.*Пароль/ })).toBeInTheDocument();
    expect(api.connectionSecrets).toHaveBeenCalledWith("c1");
  });

  it("у подключения без сохранённых секретов поле пароля — как у нового", async () => {
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledWith("c1"));
    expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "");
    expect(screen.queryByRole("checkbox", { name: /Удалить сохранённый/ })).not.toBeInTheDocument();
  });

  it("у нового подключения и копии сохранённые секреты не запрашиваются", () => {
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    renderWithClient(<ConnectionForm initial={{ ...existing(), id: "" }} copyOf="dev-1" tabId="t2" />);
    expect(api.connectionSecrets).not.toHaveBeenCalled();
  });

  it("keychain не прочитался: форма открывается как у подключения без секретов и сохраняется", async () => {
    const user = userEvent.setup();
    vi.mocked(api.connectionSecrets).mockRejectedValue(appError("INTERNAL", "keychain unavailable"));
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledWith("c1"));
    expect(screen.getByLabelText("Пароль")).toHaveAttribute("placeholder", "");
    expect(screen.queryByRole("checkbox", { name: /Удалить сохранённый/ })).not.toBeInTheDocument();
    const save = screen.getByRole("button", { name: "Сохранить" });
    expect(save).toBeEnabled();
    await user.click(save);
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
    expect(vi.mocked(api.saveConnection).mock.calls[0][1]).toEqual({ pgPassword: null, pgKeyPassword: null, patroniPassword: null, patroniKeyPassword: null });
  });

  it("после сохранения ответ о сохранённых секретах запрашивается заново", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledTimes(1));
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.connectionSecrets).toHaveBeenCalledTimes(2));
  });

  it("секрет перестал быть сохранённым после отметки «удалить»: флажок скрыт, а поле пароля снова доступно", async () => {
    const user = userEvent.setup();
    vi.mocked(api.connectionSecrets).mockResolvedValue({ ...NO_SECRETS, pgPassword: true });
    const { client } = renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
    await user.click(await screen.findByRole("checkbox", { name: "Удалить сохранённый: Пароль" }));
    expect(screen.getByLabelText("Пароль")).toBeDisabled();
    // Сохранение не закрыло форму (например, не удался перенос в папку), а секрет уже удалён.
    vi.mocked(api.connectionSecrets).mockResolvedValue(NO_SECRETS);
    await client.invalidateQueries({ queryKey: ["connectionSecrets", "c1"] });
    await waitFor(() => expect(screen.queryByRole("checkbox", { name: /Удалить сохранённый/ })).not.toBeInTheDocument());
    expect(screen.getByLabelText("Пароль")).toBeEnabled();
    await user.type(screen.getByLabelText("Пароль"), "new");
    expect(screen.getByLabelText("Пароль")).toHaveValue("new");
  });

  it("sslmode=disable при заданном CA предупреждает, что настройки TLS будут удалены", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), pg: { ...newProfile().pg, tls: { caPath: "/etc/ca.pem", clientCert: null } } }} tabId="t" />);
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText("sslmode"), "disable");
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent("Настройки TLS (CA, клиентский сертификат) при сохранении будут удалены — верните sslmode, чтобы их оставить");
    // Заметка на всю ширину сетки и привязана к полю sslmode для скринридера.
    expect(note).toHaveClass("full");
    expect(screen.getByLabelText("sslmode")).toHaveAccessibleDescription(note.textContent ?? "");
  });

  it("sslmode=disable без CA и сертификата ничего не удаляет — предупреждения нет", async () => {
    const user = userEvent.setup();
    renderWithClient(<ConnectionForm initial={newProfile()} tabId="t" />);
    await user.selectOptions(screen.getByLabelText("sslmode"), "disable");
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("Patroni: узлы прежнего вида в Rust не уходят, даже если порт из него некорректен", async () => {
    const user = userEvent.setup();
    vi.mocked(api.saveConnection).mockResolvedValue(saved("c1"));
    renderWithClient(<ConnectionForm initial={{ ...newProfile(), kind: "hostList", hosts: [{ host: "a", port: 70000 }, { host: "b", port: 5432 }] }} tabId="t" />);
    await user.click(screen.getByLabelText("Узлы под управлением Patroni"));
    await user.click(screen.getByRole("button", { name: "Сохранить" }));
    await waitFor(() => expect(api.saveConnection).toHaveBeenCalled());
    const profile = vi.mocked(api.saveConnection).mock.calls[0][0];
    expect(profile.kind).toBe("patroni");
    expect(profile.hosts).toEqual([]);
  });

  describe("prod", () => {
    it("при выборе prod для нового подключения «только чтение» включается, но его можно снять", async () => {
      const user = userEvent.setup();
      vi.mocked(api.saveConnection).mockResolvedValue(saved("n"));
      renderWithClient(<ConnectionForm initial={{ ...newProfile(), name: "n" }} tabId="t" />);
      expect(screen.getByRole("option", { name: "prod" })).toBeEnabled();
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      const ro = screen.getByLabelText("Только чтение");
      expect(ro).toBeChecked();
      expect(ro).toBeEnabled();
      await user.click(ro);
      expect(ro).not.toBeChecked();
      await user.click(screen.getByRole("button", { name: "Сохранить" }));
      const profile = vi.mocked(api.saveConnection).mock.calls[0][0];
      expect(profile).toMatchObject({ environment: "prod", readOnly: false });
    });

    it("у существующего подключения переход в prod не навязывает «только чтение»", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing()} tabId="t" />);
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      expect(screen.getByLabelText("Только чтение")).not.toBeChecked();
    });

    it("у существующего подключения возврат в prod не трогает снятый флажок", async () => {
      const user = userEvent.setup();
      renderWithClient(<ConnectionForm initial={existing({ environment: "prod", readOnly: true })} tabId="t" />);
      const ro = screen.getByLabelText("Только чтение");
      await user.selectOptions(screen.getByLabelText("Окружение"), "dev");
      await user.click(ro);
      expect(ro).not.toBeChecked();
      await user.selectOptions(screen.getByLabelText("Окружение"), "prod");
      expect(ro).not.toBeChecked();
      expect(ro).toBeEnabled();
    });
  });
});
