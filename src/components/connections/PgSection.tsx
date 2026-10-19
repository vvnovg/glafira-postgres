import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ConnectionProfile, PgSettings, SslMode } from "../../api/types";
import { Hint } from "../common/Hint";
import { ClientCertRows, PathRow, RowLabel, SecretRow } from "./fields";
import { formatDatabases, parseDatabases, type SecretField } from "./profile";

const SSL_MODES: SslMode[] = ["disable", "prefer", "require", "verify-ca", "verify-full"];

export type PgSecrets = { password: SecretField; keyPassword: SecretField };

export function PgSection(props: {
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  secrets: PgSecrets;
  onSecrets: (s: PgSecrets) => void;
  isNew: boolean;
}) {
  const { t } = useTranslation();
  const pg = props.profile.pg;
  const set = (patch: Partial<PgSettings>) => props.onChange({ ...props.profile, pg: { ...pg, ...patch } });
  const [dbText, setDbText] = useState(formatDatabases(pg.databases));
  return (
    <>
      <label htmlFor="cf-pg-user">{t("connections.form.pgUser")}</label>
      <input id="cf-pg-user" value={pg.username} onChange={(e) => set({ username: e.target.value })} />
      <SecretRow
        id="cf-pg-pass"
        label={t("connections.form.pgPassword")}
        hint={<Hint k="hints.connections.pgpass" label={t("connections.form.pgPassword")} />}
        isNew={props.isNew}
        value={props.secrets.password}
        onChange={(password) => props.onSecrets({ ...props.secrets, password })}
      />
      <label htmlFor="cf-pg-db">{t("connections.form.pgDatabase")}</label>
      <input id="cf-pg-db" value={pg.database} onChange={(e) => set({ database: e.target.value })} />
      <RowLabel id="cf-ssl" label="sslmode" hint={<Hint k="hints.connections.sslMode" label="sslmode" />} />
      <select id="cf-ssl" value={pg.sslMode} onChange={(e) => set({ sslMode: e.target.value as SslMode })}>
        {SSL_MODES.map((m) => (
          <option key={m} value={m}>
            {m}
          </option>
        ))}
      </select>
      {pg.sslMode !== "disable" && (
        <>
          <PathRow id="cf-pg-ca" label={t("connections.form.caPath")} value={pg.tls.caPath} onChange={(caPath) => set({ tls: { ...pg.tls, caPath } })} />
          <ClientCertRows
            idPrefix="cf-pg"
            cert={pg.tls.clientCert}
            onChange={(clientCert) => set({ tls: { ...pg.tls, clientCert } })}
            keyPassword={props.secrets.keyPassword}
            onKeyPassword={(keyPassword) => props.onSecrets({ ...props.secrets, keyPassword })}
            isNew={props.isNew}
          />
        </>
      )}
      <label htmlFor="cf-timeout">{t("connections.form.timeout")}</label>
      <input id="cf-timeout" type="number" min={1} max={120} value={pg.connectTimeoutS} onChange={(e) => set({ connectTimeoutS: Number(e.target.value) })} />
      <label htmlFor="cf-dbs">{t("connections.form.databases")}</label>
      <input
        id="cf-dbs"
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
