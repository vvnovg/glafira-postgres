import { useTranslation } from "react-i18next";
import type { ConnectionProfile, SecretsSet, SshAuth, SshSettings } from "../../api/types";
import { Hint } from "../common/Hint";
import { FieldGroup, PathRow, RowLabel, SecretRow } from "./fields";
import { emptySsh, isValidPort, type SecretField } from "./profile";

/** Секреты туннеля, которые вводятся в форме: пароль входа и пассфраза ключа. */
export type SshSecrets = { password: SecretField; passphrase: SecretField };
const AUTHS: SshAuth[] = ["agent", "key", "password"];

/**
 * Раздел «SSH» формы (спец. P1 §12): выключатель, цель, порт, пользователь, способ входа, ключ и секреты.
 * Порт и пользователь можно оставить пустыми — их тогда берёт сам ssh из ~/.ssh/config. Поле секрета показывается
 * только для способа, которому он нужен: пассфраза — у «ключа», пароль — у «пароля».
 */
export function SshSection(props: {
  /** Префикс id полей: у каждой формы свой (вкладки остаются смонтированными, одинаковые id склеили бы подписи). */
  idPrefix: string;
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  secrets: SshSecrets;
  onSecrets: (s: SshSecrets) => void;
  /** Какие секреты уже сохранены в keychain (у нового подключения — никакие). */
  saved: SecretsSet;
}) {
  const { t } = useTranslation();
  const { profile: p, onChange, idPrefix: id } = props;
  const ssh = p.ssh;
  const set = (patch: Partial<SshSettings>) => onChange({ ...p, ssh: { ...(ssh ?? emptySsh()), ...patch } });
  return (
    <FieldGroup legend={t("connections.form.ssh.section")}>
      <label htmlFor={`${id}-ssh-on`}>{t("connections.form.ssh.enabled")}</label>
      <span>
        <input id={`${id}-ssh-on`} type="checkbox" checked={ssh !== null} onChange={(e) => onChange({ ...p, ssh: e.target.checked ? emptySsh() : null })} />
      </span>
      {ssh && (
        <>
          <RowLabel id={`${id}-ssh-target`} label={t("connections.form.ssh.target")} hint={<Hint k="hints.connections.sshTarget" label={t("connections.form.ssh.target")} />} />
          <input id={`${id}-ssh-target`} value={ssh.target} onChange={(e) => set({ target: e.target.value })} />
          <label htmlFor={`${id}-ssh-port`}>{t("connections.form.ssh.port")}</label>
          <input
            id={`${id}-ssh-port`}
            type="number"
            min={1}
            max={65535}
            placeholder={t("connections.form.ssh.fromConfig")}
            aria-invalid={ssh.port !== null && !isValidPort(ssh.port)}
            value={ssh.port ?? ""}
            onChange={(e) => set({ port: e.target.value === "" ? null : Number(e.target.value) })}
          />
          <label htmlFor={`${id}-ssh-user`}>{t("connections.form.ssh.user")}</label>
          <input id={`${id}-ssh-user`} value={ssh.user ?? ""} placeholder={t("connections.form.ssh.fromConfig")} onChange={(e) => set({ user: e.target.value || null })} />
          <label htmlFor={`${id}-ssh-auth`}>{t("connections.form.ssh.auth")}</label>
          <select id={`${id}-ssh-auth`} value={ssh.auth} onChange={(e) => set({ auth: e.target.value as SshAuth })}>
            {AUTHS.map((a) => (
              <option key={a} value={a}>
                {t(`connections.form.ssh.auths.${a}`)}
              </option>
            ))}
          </select>
          {ssh.auth === "key" && (
            <>
              <PathRow id={`${id}-ssh-key`} label={t("connections.form.ssh.keyPath")} value={ssh.keyPath} onChange={(keyPath) => set({ keyPath })} />
              <SecretRow
                id={`${id}-ssh-passphrase`}
                label={t("connections.form.ssh.passphrase")}
                saved={props.saved.sshPassphrase}
                value={props.secrets.passphrase}
                onChange={(passphrase) => props.onSecrets({ ...props.secrets, passphrase })}
              />
            </>
          )}
          {ssh.auth === "password" && (
            <SecretRow
              id={`${id}-ssh-password`}
              label={t("connections.form.ssh.password")}
              saved={props.saved.sshPassword}
              value={props.secrets.password}
              onChange={(password) => props.onSecrets({ ...props.secrets, password })}
            />
          )}
        </>
      )}
    </FieldGroup>
  );
}
