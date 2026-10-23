import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { CertFormat, ClientCert } from "../../api/types";
import { formatFromPath, type SecretField } from "./profile";

/** Подпись строки сетки; значок подсказки — рядом с `<label>`, а не внутри (иначе попадёт в доступное имя). */
export function RowLabel({ id, label, hint }: { id: string; label: string; hint?: ReactNode }) {
  if (!hint) return <label htmlFor={id}>{label}</label>;
  return (
    <span className="field-label">
      <label htmlFor={id}>{label}</label>
      {hint}
    </span>
  );
}

/** Название поля с указанием раздела, если одинаковые подписи есть в нескольких (CA в «TLS для API» и в «TLS PostgreSQL»). */
function useScopedLabel(label: string, scope?: string): string {
  const { t } = useTranslation();
  return scope ? t("connections.form.scoped", { label, scope }) : label;
}

/** Рамка с заголовком для набора полей: у доступной группы своё имя, а подписи внутри остаются короткими. */
export function FieldGroup({ legend, children }: { legend: string; children: ReactNode }) {
  return (
    <fieldset className="field-group">
      <legend>{legend}</legend>
      <div className="form">{children}</div>
    </fieldset>
  );
}

async function pickFile(onPath: (p: string) => void) {
  const path = await openDialog({ multiple: false, directory: false });
  if (typeof path === "string") onPath(path);
}

export function PathRow(props: { id: string; label: string; scope?: string; hint?: ReactNode; value: string | null; onChange: (v: string | null) => void }) {
  const { t } = useTranslation();
  const scoped = useScopedLabel(props.label, props.scope);
  return (
    <>
      <RowLabel id={props.id} label={props.label} hint={props.hint} />
      <div className="path-field">
        <input id={props.id} value={props.value ?? ""} onChange={(e) => props.onChange(e.target.value || null)} />
        <button type="button" aria-label={t("connections.form.pickAria", { label: scoped })} onClick={() => void pickFile(props.onChange)}>
          {t("connections.form.pick")}
        </button>
      </div>
    </>
  );
}

export function SecretRow(props: { id: string; label: string; scope?: string; hint?: ReactNode; saved: boolean; value: SecretField; onChange: (v: SecretField) => void }) {
  const { t } = useTranslation();
  const scoped = useScopedLabel(props.label, props.scope);
  return (
    <>
      <RowLabel id={props.id} label={props.label} hint={props.hint} />
      <span className="path-field">
        <input
          id={props.id}
          type="password"
          autoComplete="off"
          // Отметка «удалить» значима, только пока секрет сохранён: если его уже нет (удалён при сохранении, сброшен
          // импортом), поле остаётся доступным, хотя флажок скрыт.
          disabled={props.saved && props.value.clear}
          placeholder={props.saved ? t("connections.form.passwordKeep") : ""}
          value={props.value.text}
          onChange={(e) => props.onChange({ text: e.target.value, clear: false })}
        />
        {props.saved && (
          <label className="inline">
            <input type="checkbox" aria-label={t("connections.form.passwordClearAria", { label: scoped })} checked={props.value.clear} onChange={(e) => props.onChange({ text: "", clear: e.target.checked })} />
            {t("connections.form.passwordClear")}
          </label>
        )}
      </span>
    </>
  );
}

/** Клиентский сертификат: формат, файлы, пароль ключа. `null` — без сертификата. */
export function ClientCertRows(props: {
  idPrefix: string;
  /** Раздел, в котором стоят поля, — для различимых имён кнопок и флажков. */
  scope?: string;
  cert: ClientCert | null;
  onChange: (c: ClientCert | null) => void;
  keyPassword: SecretField;
  onKeyPassword: (v: SecretField) => void;
  /** Сохранён ли в keychain пароль ключа. */
  saved: boolean;
}) {
  const { t } = useTranslation();
  const { cert, idPrefix: p } = props;
  return (
    <>
      <label htmlFor={`${p}-cc`}>{t("connections.form.clientCert")}</label>
      <span>
        <input
          id={`${p}-cc`}
          type="checkbox"
          checked={cert !== null}
          onChange={(e) => props.onChange(e.target.checked ? { format: "pem", certificatePath: "", keyPath: "" } : null)}
        />
      </span>
      {cert && (
        <>
          <label htmlFor={`${p}-fmt`}>{t("connections.form.certFormat")}</label>
          <select id={`${p}-fmt`} value={cert.format} onChange={(e) => props.onChange({ ...cert, format: e.target.value as CertFormat, keyPath: e.target.value === "pem" ? cert.keyPath ?? "" : null })}>
            <option value="pem">PEM</option>
            <option value="pkcs12">PKCS#12 (.p12, .pfx)</option>
          </select>
          <PathRow
            id={`${p}-cert`}
            label={t("connections.form.certPath")}
            scope={props.scope}
            value={cert.certificatePath}
            onChange={(v) => {
              const guessed = v ? formatFromPath(v) : null;
              const format = guessed ?? cert.format;
              props.onChange({ ...cert, certificatePath: v ?? "", format, keyPath: format === "pem" ? cert.keyPath ?? "" : null });
            }}
          />
          {cert.format === "pem" && (
            <PathRow id={`${p}-key`} label={t("connections.form.keyPath")} scope={props.scope} value={cert.keyPath} onChange={(v) => props.onChange({ ...cert, keyPath: v })} />
          )}
          <SecretRow id={`${p}-kpw`} label={t("connections.form.keyPassword")} scope={props.scope} saved={props.saved} value={props.keyPassword} onChange={props.onKeyPassword} />
        </>
      )}
    </>
  );
}
