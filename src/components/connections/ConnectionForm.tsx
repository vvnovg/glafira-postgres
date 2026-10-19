import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../api/client";
import type { AppError, ConnectionKind, ConnectionProfile, Environment, SecretsInput, TestEvent, TestReport } from "../../api/types";
import { proHint, useLicense } from "../../lib/useLicense";
import { useWorkspace } from "../../state/workspace";
import { ErrorBanner } from "../common/ErrorBanner";
import { Hint } from "../common/Hint";
import { ProLock } from "../common/ProLock";
import { NodesSection, type PatroniSecrets } from "./NodesSection";
import { PgSection, type PgSecrets } from "./PgSection";
import { applyParsed, emptySecret, secretValue, withKind } from "./profile";
import { TestPanel } from "./TestPanel";
import { folderPaths } from "./tree/folderTree";

const ENVIRONMENTS: Environment[] = ["local", "dev", "test", "stage", "prod"];
const KINDS: ConnectionKind[] = ["server", "hostList", "patroni"];

export function ConnectionForm({ initial, tabId }: { initial: ConnectionProfile; tabId: string }) {
  const { t } = useTranslation();
  const [profile, setProfile] = useState(initial);
  const [pgSecrets, setPgSecrets] = useState<PgSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [apiSecrets, setApiSecrets] = useState<PatroniSecrets>({ password: emptySecret, keyPassword: emptySecret });
  const [connstr, setConnstr] = useState("");
  const [ignored, setIgnored] = useState<string[]>([]);
  const [events, setEvents] = useState<TestEvent[]>([]);
  const [report, setReport] = useState<TestReport | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [busy, setBusy] = useState(false);
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
  const { locked } = useLicense();
  const readOnlyLocked = locked && initial.environment === "prod" && initial.readOnly && profile.environment === "prod";

  const current = (): ConnectionProfile => ({ ...profile, folderId });
  const secrets = (): SecretsInput => ({
    pgPassword: secretValue(pgSecrets.password),
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
      const parsed = await api.parseConnectionString(connstr);
      setProfile((p) => applyParsed(p, parsed));
      if (parsed.password) setPgSecrets((s) => ({ ...s, password: { text: parsed.password!, clear: false } }));
      setIgnored(parsed.ignored);
    });

  const onTest = () =>
    run(async () => {
      setEvents([]);
      setReport(null);
      setReport(await api.testConnection(current(), secrets(), (e) => setEvents((prev) => [...prev, e])));
    });

  const onSave = () =>
    run(async () => {
      const saved = await api.saveConnection(current(), secrets());
      try {
        if (!isNew && folderTouched && folderId !== knownFolder(loadedFolderId)) {
          await api.moveItems([{ kind: "connection", id: saved.id }], folderId, null);
        }
      } finally {
        await queryClient.invalidateQueries({ queryKey: ["connections"] });
      }
      // Сохранение закрывает открытое подключение (Rust): кэш его каталога больше не нужен.
      queryClient.removeQueries({ predicate: (q) => q.queryKey[1] === saved.id });
      close(tabId);
      open({ kind: "overview", connectionId: saved.id });
    });

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
        <h2>{isNew ? t("connections.form.titleNew") : t("connections.form.titleEdit", { name: initial.name })}</h2>
      </div>
      <div className="form">
        <span className="field-label">
          <label htmlFor="cf-connstr">{t("connections.form.connstr")}</label>
          <Hint k="hints.connections.connstr" label={t("connections.form.connstr")} />
        </span>
        <span className="path-field">
          <input id="cf-connstr" placeholder="jdbc:postgresql://h1,h2/db?targetServerType=primary" value={connstr} onChange={(e) => setConnstr(e.target.value)} />
          <button type="button" disabled={busy || !connstr.trim()} onClick={onParse}>
            {t("connections.form.connstrApply")}
          </button>
        </span>
        {ignored.length > 0 && <p className="full muted">{t("connections.form.connstrIgnored", { list: ignored.join(", ") })}</p>}

        <label htmlFor="cf-name">{t("connections.form.name")}</label>
        <input id="cf-name" value={profile.name} onChange={(e) => setProfile({ ...profile, name: e.target.value })} />

        <label htmlFor="cf-folder">{t("connections.form.folder")}</label>
        <select
          id="cf-folder"
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
          <label htmlFor="cf-env">{t("connections.form.environment")}</label>
          <Hint k="hints.connections.environment" label={t("connections.form.environment")} />
        </span>
        <select
          id="cf-env"
          value={profile.environment}
          onChange={(e) => {
            const environment = e.target.value as Environment;
            const forceReadOnly = environment === "prod" && (isNew || (locked && initial.environment === "prod" && initial.readOnly));
            setProfile({ ...profile, environment, readOnly: forceReadOnly ? true : profile.readOnly });
          }}
        >
          {ENVIRONMENTS.map((env) => {
            const proOnly = env === "prod" && locked && initial.environment !== "prod";
            return (
              <option key={env} value={env} disabled={proOnly}>
                {proOnly ? t("connections.form.prodPro") : env}
              </option>
            );
          })}
        </select>

        <span className="field-label">
          <label htmlFor="cf-ro">{t("common.readOnly")}</label>
          <Hint k="hints.connections.readOnly" label={t("common.readOnly")} />
        </span>
        <span>
          <input
            id="cf-ro"
            type="checkbox"
            checked={profile.readOnly}
            disabled={readOnlyLocked}
            title={readOnlyLocked ? proHint() : undefined}
            onChange={(e) => setProfile({ ...profile, readOnly: e.target.checked })}
          />
          {readOnlyLocked && (
            <>
              {" "}
              <ProLock />
            </>
          )}
        </span>

        <label htmlFor="cf-color">{t("connections.form.color")}</label>
        <input id="cf-color" type="color" value={profile.color} onChange={(e) => setProfile({ ...profile, color: e.target.value })} />

        <span className="field-label">
          {t("connections.form.kind")} <Hint k="hints.connections.kind" label={t("connections.form.kind")} />
        </span>
        <span role="radiogroup" aria-label={t("connections.form.kind")}>
          {KINDS.map((k) => (
            <label key={k} className="inline">
              <input type="radio" name="cf-kind" checked={profile.kind === k} onChange={() => setProfile(withKind(profile, k))} />
              {t(`connections.form.kinds.${k}`)}
            </label>
          ))}
        </span>

        <NodesSection profile={profile} onChange={setProfile} secrets={apiSecrets} onSecrets={setApiSecrets} isNew={isNew} />
        <h3 className="full">PostgreSQL</h3>
        <PgSection profile={profile} onChange={setProfile} secrets={pgSecrets} onSecrets={setPgSecrets} isNew={isNew} />
      </div>

      <div className="toolbar">
        <button type="button" disabled={busy} onClick={onTest}>
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
        {busy && <span className="muted">{t("connections.form.busy")}</span>}
      </div>

      {error && <ErrorBanner error={error} />}
      {(events.length > 0 || report) && <TestPanel events={events} report={report} />}
    </div>
  );
}
