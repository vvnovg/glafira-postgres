import { useTranslation } from "react-i18next";
import type { ConnectionProfile, LlmAccess, ReadinessItem } from "../../api/types";
import { dataAllowed } from "../../lib/llmAccess";
import { llmRoleParams } from "../../lib/llmRole";
import { useMcpStatus } from "../../lib/useMcpStatus";
import { Hint } from "../common/Hint";

/** Уровни в порядке переключателей (спец. P4 §11). */
const LEVELS: LlmAccess[] = ["off", "diagnostics", "data"];

/**
 * Группа «Доступ для LLM» формы подключения (спец. P4 §4.1, §4.2, §4.5, §11): две ячейки сетки `.form` — подпись с
 * подсказкой и переключатели с пометками. Статус MCP учитывается строго: третий уровень доступен, только когда статус
 * пришёл и потолок политики разрешает данные. Пока статус не загружен или не загрузился, третий уровень неактивен без
 * пометки, а сохранённый `data` остаётся отмеченным (неактивная радиокнопка) — сохранение формы его не меняет.
 * `roleItem` — пункт `llmRole` последней проверки готовности в этой форме.
 */
export function LlmAccessGroup({
  idPrefix,
  profile,
  onChange,
  roleItem,
}: {
  idPrefix: string;
  profile: ConnectionProfile;
  onChange: (p: ConnectionProfile) => void;
  roleItem?: ReadinessItem | null;
}) {
  const { t } = useTranslation();
  const { status, loading } = useMcpStatus();
  const level = profile.llmAccess ?? "off";
  const allowed = dataAllowed(status);
  const id = `${idPrefix}-llm`;
  const note = !status
    ? loading
      ? ""
      : t("connections.form.llmStatusUnavailable")
    : !status.policyEnabled
      ? t("connections.form.llmPolicyOff")
      : status.server === "stopped" && level !== "off"
        ? t("connections.form.llmServerOff")
        : "";
  const dataNote =
    allowed === false ? (level === "data" ? t("connections.form.llmDataAsDiagnostics") : t("connections.form.llmDataPolicy")) : "";
  const prodWarning = allowed === true && level === "data" && profile.environment === "prod" ? t("connections.form.llmProdWarning") : "";
  // Живые области `aria-live` есть всегда: текст, вписанный в существующую область, экранный диктор объявит (предупреждение о
  // роли приходит позже, после проверки). Не `role="status"`: статус в группе один — пометка P4a.
  const roleWarning =
    level === "data" && roleItem?.status === "warn" ? t(`readiness.llmRole.${roleItem.params.reason}`, llmRoleParams(roleItem, t)) : "";
  return (
    <>
      <span className="field-label">
        <span id={id}>{t("connections.form.llmAccess")}</span>
        <Hint k="hints.connections.llmAccess" label={t("connections.form.llmAccess")} />
      </span>
      <span className="llm-levels">
        <span role="radiogroup" aria-labelledby={id} aria-describedby={`${id}-note ${id}-data`}>
          {LEVELS.map((l) => (
            <label key={l} className="inline">
              <input
                type="radio"
                name={id}
                checked={level === l}
                disabled={l === "data" && allowed !== true}
                onChange={() => onChange({ ...profile, llmAccess: l })}
              />
              {t(`connections.form.llmLevels.${l}`)}
            </label>
          ))}
        </span>
        <span id={`${id}-note`} className="muted" role="status">
          {note}
        </span>
        <span id={`${id}-data`} className="llm-notes">
          {dataNote && <span className="muted">{dataNote}</span>}
          <span className="warn" aria-live="polite">
            {prodWarning}
          </span>
          <span className="warn" aria-live="polite">
            {roleWarning}
          </span>
        </span>
      </span>
    </>
  );
}
