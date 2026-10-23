import { useTranslation } from "react-i18next";
import type { ConnectionKind, ConnectionProfile, HostOverride, HostPort, PatroniSettings, SecretsSet, TargetServerType } from "../../api/types";
import { Hint } from "../common/Hint";
import { ClientCertRows, FieldGroup, PathRow, RowLabel, SecretRow } from "./fields";
import { emptyPatroni, isValidPort, type SecretField } from "./profile";

const TARGETS: TargetServerType[] = ["primary", "standby", "preferStandby", "any"];

export type PatroniSecrets = { password: SecretField; keyPassword: SecretField };

export function NodesSection(props: {
  /** Префикс id полей: у каждой формы свой (вкладки остаются смонтированными, одинаковые id склеили бы подписи). */
  idPrefix: string;
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  /** Смена вида (флажок «под управлением Patroni»); память формы между видами ведёт форма. */
  onKind: (kind: ConnectionKind) => void;
  secrets: PatroniSecrets;
  onSecrets: (s: PatroniSecrets) => void;
  /** Какие секреты уже сохранены в keychain (у нового подключения — никакие). */
  saved: SecretsSet;
}) {
  const { t } = useTranslation();
  const { profile: p, onChange, idPrefix: id } = props;
  const setHosts = (hosts: HostPort[]) => onChange({ ...p, hosts });
  const setPatroni = (patch: Partial<PatroniSettings>) => onChange({ ...p, patroni: { ...(p.patroni ?? emptyPatroni()), ...patch } });

  if (p.kind === "server") {
    const h = p.hosts[0] ?? { host: "", port: 5432 };
    return (
      <>
        <label htmlFor={`${id}-host`}>{t("connections.form.host")}</label>
        <span className="path-field">
          <input id={`${id}-host`} value={h.host} onChange={(e) => setHosts([{ ...h, host: e.target.value }])} />
          <input
            aria-label={t("connections.form.port")}
            type="number"
            min={1}
            max={65535}
            style={{ width: 90 }}
            aria-invalid={!isValidPort(h.port)}
            value={h.port}
            onChange={(e) => setHosts([{ ...h, port: Number(e.target.value) }])}
          />
        </span>
      </>
    );
  }

  if (p.kind === "hostList") {
    const move = (i: number, d: -1 | 1) => {
      const next = [...p.hosts];
      [next[i], next[i + d]] = [next[i + d], next[i]];
      setHosts(next);
    };
    return (
      <>
        <span className="field-label">{t("connections.form.hosts")}</span>
        <div className="headers-editor">
          {p.hosts.map((h, i) => (
            <span key={i} className="path-field">
              <input aria-label={t("connections.form.hostN", { n: i + 1 })} value={h.host} onChange={(e) => setHosts(p.hosts.map((x, j) => (j === i ? { ...x, host: e.target.value } : x)))} />
              <input
                aria-label={t("connections.form.portN", { n: i + 1 })}
                type="number"
                min={1}
                max={65535}
                style={{ width: 90 }}
                aria-invalid={!isValidPort(h.port)}
                value={h.port}
                onChange={(e) => setHosts(p.hosts.map((x, j) => (j === i ? { ...x, port: Number(e.target.value) } : x)))}
              />
              <button type="button" className="icon" disabled={i === 0} aria-label={t("connections.form.moveUp", { n: i + 1 })} onClick={() => move(i, -1)}>↑</button>
              <button type="button" className="icon" disabled={i === p.hosts.length - 1} aria-label={t("connections.form.moveDown", { n: i + 1 })} onClick={() => move(i, 1)}>↓</button>
              <button type="button" className="icon" disabled={p.hosts.length === 1} aria-label={t("connections.form.removeHost", { n: i + 1 })} onClick={() => setHosts(p.hosts.filter((_, j) => j !== i))}>✕</button>
            </span>
          ))}
          <button type="button" className="link" disabled={p.hosts.length >= 16} onClick={() => setHosts([...p.hosts, { host: "", port: 5432 }])}>
            {t("connections.form.addHost")}
          </button>
        </div>

        <RowLabel id={`${id}-target`} label={t("connections.form.target")} hint={<Hint k="hints.connections.target" label={t("connections.form.target")} />} />
        <select id={`${id}-target`} value={p.targetServerType} onChange={(e) => onChange({ ...p, targetServerType: e.target.value as TargetServerType })}>
          {TARGETS.map((x) => (
            <option key={x} value={x}>
              {t(`connections.form.targets.${x}`)}
            </option>
          ))}
        </select>

        <label htmlFor={`${id}-under-patroni`}>{t("connections.form.underPatroni")}</label>
        <span>
          <input id={`${id}-under-patroni`} type="checkbox" checked={false} onChange={() => props.onKind("patroni")} />
        </span>
      </>
    );
  }

  const pt = p.patroni ?? emptyPatroni();
  const tls = pt.tls;
  const apiScope = t("connections.form.apiTls");
  const setOverride = (i: number, patch: Partial<HostOverride>) =>
    setPatroni({ hostOverrides: pt.hostOverrides.map((o, j) => (j === i ? { ...o, ...patch } : o)) });
  return (
    <>
      <span className="field-label">{t("connections.form.patroniUrls")}</span>
      <div className="headers-editor">
        {pt.urls.map((u, i) => (
          <span key={i} className="path-field">
            <input aria-label={t("connections.form.urlN", { n: i + 1 })} placeholder="https://pg-1:8008" value={u} onChange={(e) => setPatroni({ urls: pt.urls.map((x, j) => (j === i ? e.target.value : x)) })} />
            <button type="button" className="icon" disabled={pt.urls.length === 1} aria-label={t("connections.form.removeUrl", { n: i + 1 })} onClick={() => setPatroni({ urls: pt.urls.filter((_, j) => j !== i) })}>✕</button>
          </span>
        ))}
        <button type="button" className="link" disabled={pt.urls.length >= 8} onClick={() => setPatroni({ urls: [...pt.urls, ""] })}>
          {t("connections.form.addUrl")}
        </button>
      </div>

      <label htmlFor={`${id}-api-user`}>{t("connections.form.patroniUser")}</label>
      <input id={`${id}-api-user`} value={pt.username ?? ""} onChange={(e) => setPatroni({ username: e.target.value || null })} />
      {pt.username && (
        <SecretRow id={`${id}-api-pass`} label={t("connections.form.patroniPassword")} saved={props.saved.patroniPassword} value={props.secrets.password} onChange={(password) => props.onSecrets({ ...props.secrets, password })} />
      )}

      <label htmlFor={`${id}-api-tls`}>{t("connections.form.apiTls")}</label>
      <span>
        <input id={`${id}-api-tls`} type="checkbox" checked={tls !== null} onChange={(e) => setPatroni({ tls: e.target.checked ? { caPath: null, verifyHostname: true, clientCert: null } : null })} />
      </span>
      {tls && (
        <FieldGroup legend={t("connections.form.apiTls")}>
          <PathRow id={`${id}-api-ca`} label={t("connections.form.caPath")} scope={apiScope} value={tls.caPath} onChange={(caPath) => setPatroni({ tls: { ...tls, caPath } })} />
          <label htmlFor={`${id}-api-verify`}>{t("connections.form.verifyHostname")}</label>
          <span>
            <input id={`${id}-api-verify`} type="checkbox" checked={tls.verifyHostname} onChange={(e) => setPatroni({ tls: { ...tls, verifyHostname: e.target.checked } })} />
          </span>
          <ClientCertRows
            idPrefix={`${id}-api`}
            scope={apiScope}
            cert={tls.clientCert}
            onChange={(clientCert) => setPatroni({ tls: { ...tls, clientCert } })}
            keyPassword={props.secrets.keyPassword}
            onKeyPassword={(keyPassword) => props.onSecrets({ ...props.secrets, keyPassword })}
            saved={props.saved.patroniKeyPassword}
          />
        </FieldGroup>
      )}

      <details className="full">
        <summary>
          {t("connections.form.overrides")} <Hint k="hints.connections.overrides" label={t("connections.form.overrides")} />
        </summary>
        <div className="headers-editor">
          {pt.hostOverrides.map((o, i) => (
            <span key={i} className="path-field">
              <input aria-label={t("connections.form.overrideMember", { n: i + 1 })} placeholder="pg-1" value={o.member} onChange={(e) => setOverride(i, { member: e.target.value })} />
              <input aria-label={t("connections.form.overrideHost", { n: i + 1 })} placeholder="10.0.0.1" value={o.host ?? ""} onChange={(e) => setOverride(i, { host: e.target.value || null })} />
              <input
                aria-label={t("connections.form.overridePort", { n: i + 1 })}
                type="number"
                min={1}
                max={65535}
                style={{ width: 90 }}
                aria-invalid={o.port !== null && !isValidPort(o.port)}
                value={o.port ?? ""}
                onChange={(e) => setOverride(i, { port: e.target.value ? Number(e.target.value) : null })}
              />
              <input aria-label={t("connections.form.overrideApi", { n: i + 1 })} placeholder="https://10.0.0.1:8008" value={o.apiUrl ?? ""} onChange={(e) => setOverride(i, { apiUrl: e.target.value || null })} />
              <button type="button" className="icon" aria-label={t("connections.form.removeOverride", { n: i + 1 })} onClick={() => setPatroni({ hostOverrides: pt.hostOverrides.filter((_, j) => j !== i) })}>✕</button>
            </span>
          ))}
          <button type="button" className="link" onClick={() => setPatroni({ hostOverrides: [...pt.hostOverrides, { member: "", host: null, port: null, apiUrl: null }] })}>
            {t("connections.form.addOverride")}
          </button>
        </div>
      </details>
    </>
  );
}
