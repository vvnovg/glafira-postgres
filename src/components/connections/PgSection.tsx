import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConnectionProfile, PgSettings, SecretsSet, SslMode } from "../../api/types";
import { Hint } from "../common/Hint";
import { ClientCertRows, FieldGroup, PathRow, RowLabel, SecretRow } from "./fields";
import { formatDatabases, parseDatabases, type SecretField } from "./profile";

const SSL_MODES: SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

export type PgSecrets = { password: SecretField; keyPassword: SecretField };

export function PgSection(props: {
  /** Префикс id полей: у каждой формы свой (см. NodesSection). */
  idPrefix: string;
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  secrets: PgSecrets;
  onSecrets: (s: PgSecrets) => void;
  /** Какие секреты уже сохранены в keychain (у нового подключения — никакие). */
  saved: SecretsSet;
}) {
  const { t } = useTranslation();
  const id = props.idPrefix;
  const pg = props.profile.pg;
  const tlsScope = t("connections.form.pgTls");
  const set = (patch: Partial<PgSettings>) => props.onChange({ ...props.profile, pg: { ...pg, ...patch } });
  const [dbText, setDbText] = useState(formatDatabases(pg.databases));
  return (
    <>
      <label htmlFor={`${id}-pg-user`}>{t("connections.form.pgUser")}</label>
      <input id={`${id}-pg-user`} value={pg.username} onChange={(e) => set({ username: e.target.value })} />
      <SecretRow
        id={`${id}-pg-pass`}
        label={t("connections.form.pgPassword")}
        hint={<Hint k="hints.connections.pgpass" label={t("connections.form.pgPassword")} />}
        saved={props.saved.pgPassword}
        value={props.secrets.password}
        onChange={(password) => props.onSecrets({ ...props.secrets, password })}
      />
      <label htmlFor={`${id}-pg-db`}>{t("connections.form.pgDatabase")}</label>
      <input id={`${id}-pg-db`} value={pg.database} onChange={(e) => set({ database: e.target.value })} />
      <RowLabel id={`${id}-ssl`} label="sslmode" hint={<Hint k="hints.connections.sslMode" label="sslmode" />} />
      <select id={`${id}-ssl`} value={pg.sslMode} onChange={(e) => set({ sslMode: e.target.value as SslMode })}>
        {SSL_MODES.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
      {pg.sslMode === "disable" && (pg.tls.caPath !== null || pg.tls.clientCert !== null) && (
        <p className="muted" role="note">
          {t("connections.form.tlsDroppedOnDisable")}
        </p>
      )}
      {pg.sslMode !== "disable" && (
        <FieldGroup legend={tlsScope}>
          <PathRow id={`${id}-pg-ca`} label={t("connections.form.caPath")} scope={tlsScope} value={pg.tls.caPath} onChange={(caPath) => set({ tls: { ...pg.tls, caPath } })} />
          <ClientCertRows
            idPrefix={`${id}-pg`}
            scope={tlsScope}
            cert={pg.tls.clientCert}
            onChange={(clientCert) => set({ tls: { ...pg.tls, clientCert } })}
            keyPassword={props.secrets.keyPassword}
            onKeyPassword={(keyPassword) => props.onSecrets({ ...props.secrets, keyPassword })}
            saved={props.saved.pgKeyPassword}
          />
        </FieldGroup>
      )}
      <label htmlFor={`${id}-timeout`}>{t("connections.form.timeout")}</label>
      <input id={`${id}-timeout`} type="number" min={1} max={120} value={pg.connectTimeoutS} onChange={(e) => set({ connectTimeoutS: Number(e.target.value) })} />
      <label htmlFor={`${id}-dbs`}>{t("connections.form.databases")}</label>
      <input
        id={`${id}-dbs`}
        placeholder={t("connections.form.databasesPlaceholder")}
        value={dbText}
        onChange={(e) => {
          setDbText(e.target.value);
          set({ databases: parseDatabases(e.target.value) });
        }}
      />
    </>
  );
}
