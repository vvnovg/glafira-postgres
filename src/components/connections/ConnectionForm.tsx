import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, ConnectionProfile, Environment, SecretsInput, SecretsSet, TestEvent, TestReport } from "../../api/types";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { Hint } from "../common/Hint";
import { NodesSection, type PatroniSecrets } from "./NodesSection";
import { PgSection, type PgSecrets } from "./PgSection";
import { applyParsed, emptyKindMemo, emptySecret, type KindMemo, portProblem, secretValue, switchKind, visibleProfile } from "./profile";
import { TestPanel } from "./TestPanel";
import { folderPaths } from "./tree/folderTree";

const ENVIRONMENTS: Environment[] = ["local", "dev", "test", "stage", "prod"];
const KINDS: ConnectionKind[] = ["server", "hostList", "patroni"];
/** Ничего не сохранено: у нового подключения и копии, пока ответ не пришёл и если keychain не прочитался. */
const NO_SECRETS: SecretsSet = { pgPassword: false, pgKeyPassword: false, patroniPassword: false, patroniKeyPassword: false, sshPassword: false, sshPassphrase: false };

/** copyOf — имя подключения, копия которого создаётся: заголовок и напоминание, что пароли не скопированы. */
export function ConnectionForm({ initial, tabId, copyOf }: { initial: ConnectionProfile; tabId: string; copyOf?: string }) {
  const { t } = useTranslation();
  // Вкладки формы остаются смонтированными (скрытые), поэтому id полей и имя группы радио у каждой формы свои.
  const uid = useId();
  const [profile, setProfile] = useState(initial);
  const [kindMemo, setKindMemo] = useState<KindMemo>(emptyKindMemo);
  const [pgSecrets, setPgSecrets] = useState<PgSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [apiSecrets, setApiSecrets] = useState<PatroniSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [connstr, setConnstr] = useState("");
  const [ignored, setIgnored] = useState<string[]>([]);
  const [events, setEvents] = useState<TestEvent[]>([]);
  const [report, setReport] = useState<TestReport | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  // busy — разбор строки, сохранение, удаление; testing — проверка (долгая, не должна блокировать остальное).
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const testingRef = useRef(false);
  const [confirmingDelete, setConfirmingDelete] = useState(false);
  const queryClient = useQueryClient();
  const foldersQ = useQuery({ queryKey: ["folders"], queryFn: api.listFolders });
  const paths = useMemo(() => folderPaths(foldersQ.data ?? []), [foldersQ.data]);
  // Папка, которой уже нет (удалена в дереве), считается корнем — иначе select покажет «Без папки», а сохранится висячий id.
  const knownFolder = (id: string | null | undefined): string | null =>
    !id || (foldersQ.isSuccess && !paths.some((p) => p.id === id)) ? null : id;
  const [loadedFolderId] = useState(initial.folderId);
  const [folderTouched, setFolderTouched] = useState(false);
  const folderId = knownFolder(folderTouched ? profile.folderId : initial.folderId);
  const open = useWorkspace((s) => s.open);
  const close = useWorkspace((s) => s.close);
  const closeForConnection = useWorkspace((s) => s.closeForConnection);
  const isNew = initial.id === "";
  // Какие секреты сохранены: у нового и копии — никаких (запрос не нужен). Если keychain не прочитался, форма остаётся
  // пригодной: поля как у подключения без секретов, сохранение не блокируется (пустое поле — «не менять»).
  const savedSecrets = useQuery({
    queryKey: ["connectionSecrets", initial.id],
    queryFn: () => api.connectionSecrets(initial.id),
    enabled: !isNew,
  });
  const saved = savedSecrets.data ?? NO_SECRETS;

  const current = (): ConnectionProfile => visibleProfile({ ...profile, folderId });
  const secrets = (): SecretsInput => ({
    pgPassword: secretValue(pgSecrets.password),
    // При sslmode=disable сертификат из профиля убирается (visibleProfile), и Rust удаляет пароль ключа вместе
    // с ним: значение поля здесь роли не играет, оно уходит как есть.
    pgKeyPassword: secretValue(pgSecrets.keyPassword),
    patroniPassword: secretValue(apiSecrets.password),
    patroniKeyPassword: secretValue(apiSecrets.keyPassword),
  });

  async function run(fn: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  }

  const onParse = () =>
    run(async () => {
      try {
        const parsed = await api.parseConnectionString(connstr);
        setProfile((p) => applyParsed(p, parsed));
        setKindMemo(emptyKindMemo); // разобранная строка заменила узлы — прежняя память о них устарела
        if (parsed.password) setPgSecrets((s) => ({ ...s, password: { text: parsed.password!, clear: false } }));
        setIgnored(parsed.ignored);
        // Пароль из строки перенесён в поле пароля (скрытое); в открытом виде строка не остаётся.
        setConnstr("");
      } catch (e) {
        setIgnored([]); // список «пропущено» относился к прошлому разбору
        throw e;
      }
    });

  const onTest = async () => {
    if (testingRef.current) return; // повторный запуск, пока идёт проверка, не нужен
    const bad = portProblem(current());
    if (bad) {
      setError(bad);
      return;
    }
    testingRef.current = true;
    setTesting(true);
    setError(null);
    setEvents([]);
    setReport(null);
    try {
      setReport(await api.testConnection(current(), secrets(), (e) => setEvents((prev) => [...prev, e])));
    } catch (e) {
      setError(toAppError(e));
    } finally {
      testingRef.current = false;
      setTesting(false);
    }
  };

  const onSave = () =>
    run(async () => {
      const bad = portProblem(current());
      if (bad) throw bad;
      const stored = await api.saveConnection(current(), secrets());
      // Смена папки — отдельная команда; её сбой не должен оставить кэш устаревшим: профиль уже сохранён.
      let moveError: unknown = null;
      try {
        if (!isNew && folderTouched && folderId !== knownFolder(loadedFolderId)) {
          await api.moveItems([{ kind: "connection", id: stored.id }], folderId, null);
        }
      } catch (e) {
        moveError = e;
      }
      await queryClient.invalidateQueries({ queryKey: ["connections"] });
      // Секреты могли измениться (новый пароль, удаление, сброс при смене настроек): вид «сохранён ли» перечитывается.
      void queryClient.invalidateQueries({ queryKey: ["connectionSecrets", stored.id], exact: true });
      // Правка оформления (имя, цвет, папка) открытое подключение не закрывает: Rust только обновляет в нём
      // профиль, поэтому вид подключения и журнал перечитываются, а не удаляются (удаление оторвало бы
      // наблюдателей дерева и обзора от кэша и свернуло бы раскрытый каталог). Если изменились настройки
      // соединения или секреты, Rust закрывает подключение, и событие Disconnected само чистит каталог и факты.
      void queryClient.invalidateQueries({ queryKey: ["topology", stored.id], exact: true });
      void queryClient.invalidateQueries({ queryKey: ["topologyEvents", stored.id], exact: true });
      // Папка не сменилась: остаёмся на форме и показываем ошибку (повторное сохранение повторит перенос).
      if (moveError !== null) throw moveError;
      close(tabId);
      open({ kind: "overview", connectionId: stored.id });
    });

  const changeKind = (kind: ConnectionKind) => {
    const next = switchKind(profile, kind, kindMemo);
    setProfile(next.profile);
    setKindMemo(next.memo);
  };

  const onDelete = () =>
    run(async () => {
      setConfirmingDelete(false);
      await api.deleteConnection(profile.id);
      await queryClient.invalidateQueries({ queryKey: ["connections"] });
      queryClient.removeQueries({ predicate: (q) => q.queryKey[1] === profile.id });
      closeForConnection(profile.id);
      close(tabId);
    });

  return (
    <div className="page">
      <div className="toolbar">
        <h2>
          {copyOf !== undefined
            ? t("connections.form.titleCopy", { name: copyOf })
            : isNew
              ? t("connections.form.titleNew")
              : t("connections.form.titleEdit", { name: initial.name })}
        </h2>
      </div>
      {copyOf !== undefined && <p className="muted">{t("connections.form.copyNotice")}</p>}
      <div className="form">
        <span className="field-label">
          <label htmlFor={`${uid}-connstr`}>{t("connections.form.connstr")}</label>
          <Hint k="hints.connections.connstr" label={t("connections.form.connstr")} />
        </span>
        <span className="path-field">
          <input id={`${uid}-connstr`} placeholder="jdbc:postgresql://h1,h2/db?targetServerType=primary" value={connstr} onChange={(e) => setConnstr(e.target.value)} />
          <button type="button" disabled={busy || !connstr.trim()} onClick={onParse}>
            {t("connections.form.connstrApply")}
          </button>
        </span>
        {ignored.length > 0 && <p className="full muted">{t("connections.form.connstrIgnored", { list: ignored.join(", ") })}</p>}

        <label htmlFor={`${uid}-name`}>{t("connections.form.name")}</label>
        <input id={`${uid}-name`} value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} />

        <label htmlFor={`${uid}-folder`}>{t("connections.form.folder")}</label>
        <select
          id={`${uid}-folder`}
          value={folderId ?? ""}
          onChange={(e) => {
            setFolderTouched(true);
            setProfile({ ...profile, folderId: e.target.value || null });
          }}
        >
          <option value="">{t("connections.form.folderRoot")}</option>
          {paths.map((p) => (
            <option key={p.id} value={p.id}>
              {p.path.join(" / ")}
            </option>
          ))}
        </select>

        <span className="field-label">
          <label htmlFor={`${uid}-env`}>{t("connections.form.environment")}</label>
          <Hint k="hints.connections.environment" label={t("connections.form.environment")} />
        </span>
        <select
          id={`${uid}-env`}
          value={profile.environment}
          onChange={(e) => {
            const environment = e.target.value as Environment;
            // Новое подключение (и копия) в prod по умолчанию только для чтения; флажок можно снять до сохранения.
            setProfile({ ...profile, environment, readOnly: environment === "prod" && isNew ? true : profile.readOnly });
          }}
        >
          {ENVIRONMENTS.map((env) => (
            <option key={env} value={env}>
              {env}
            </option>
          ))}
        </select>

        <span className="field-label">
          <label htmlFor={`${uid}-ro`}>{t("common.readOnly")}</label>
          <Hint k="hints.connections.readOnly" label={t("common.readOnly")} />
        </span>
        <span>
          <input
            id={`${uid}-ro`}
            type="checkbox"
            checked={profile.readOnly}
            onChange={(e) => setProfile({ ...profile, readOnly: e.target.checked })}
          />
        </span>

        <label htmlFor={`${uid}-color`}>{t("connections.form.color")}</label>
        <input id={`${uid}-color`} type="color" value={profile.color} onChange={(e) => setProfile({ ...profile, color: e.target.value })} />

        <span className="field-label">
          {t("connections.form.kind")} <Hint k="hints.connections.kind" label={t("connections.form.kind")} />
        </span>
        <span role="radiogroup" aria-label={t("connections.form.kind")}>
          {KINDS.map((k) => (
            <label key={k} className="inline">
              <input type="radio" name={`${uid}-kind`} checked={profile.kind === k} onChange={() => changeKind(k)} />
              {t(`connections.form.kinds.${k}`)}
            </label>
          ))}
        </span>

        <NodesSection idPrefix={uid} profile={profile} onChange={setProfile} onKind={changeKind} secrets={apiSecrets} onSecrets={setApiSecrets} saved={saved} />
        <h3 className="full">PostgreSQL</h3>
        <PgSection idPrefix={uid} profile={profile} onChange={setProfile} secrets={pgSecrets} onSecrets={setPgSecrets} saved={saved} />
      </div>

      <div className="toolbar">
        <button type="button" disabled={busy || testing} onClick={() => void onTest()}>
          {t("connections.form.test")}
        </button>
        <button type="button" className="primary" disabled={busy} onClick={onSave}>
          {t("common.save")}
        </button>
        {!isNew &&
          (confirmingDelete ? (
            <span role="group" aria-label={t("connections.form.deleteGroup")}>
              <strong>{t("connections.form.deleteConfirm", { name: profile.name })}</strong>{" "}
              <button type="button" className="danger" disabled={busy} onClick={onDelete}>
                {t("connections.form.deleteYes")}
              </button>{" "}
              <button type="button" disabled={busy} onClick={() => setConfirmingDelete(false)}>
                {t("common.cancel")}
              </button>
            </span>
          ) : (
            <button type="button" className="danger" disabled={busy} onClick={() => setConfirmingDelete(true)}>
              {t("common.delete")}
            </button>
          ))}
        {(busy || testing) && <span className="muted">{t("connections.form.busy")}</span>}
      </div>

      {error && <ErrorBanner error={error} />}
      {(events.length > 0 || report) && <TestPanel events={events} report={report} />}
    </div>
  );
}
