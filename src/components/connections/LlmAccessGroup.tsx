import { useTranslation } from "react-i18next";
import type { ConnectionProfile, LlmAccess } from "../../api/types";
import { useMcpStatus } from "../../lib/useMcpStatus";
import { Hint } from "../common/Hint";

/** Уровни в порядке переключателей; «Диагностика и данные» (`data`) добавит P4b. */
const LEVELS: LlmAccess[] = ["off", "diagnostics"];

/**
 * Группа «Доступ для LLM» формы подключения (спец. P4 §4.1, §11): две ячейки сетки `.form` — подпись с подсказкой и
 * переключатели с пометкой. Уровень — локальное поле профиля: сохраняется вместе с формой, не уходит в экспорт и не
 * переоткрывает соединение. Статус MCP учитывается строго: пометки (запрет политикой, выключенный сервер) — только по
 * загруженному статусу; пока он грузится, группа ничего не утверждает, а если не загрузился — так и говорит.
 * Переключатели «Нет» и «Диагностика» от статуса не зависят. Пометка — живая область, которая есть всегда: текст,
 * вписанный в уже существующую область, экранный диктор объявит.
 */
export function LlmAccessGroup({ idPrefix, profile, onChange }: { idPrefix: string; profile: ConnectionProfile; onChange: (p: ConnectionProfile) => void }) {
  const { t } = useTranslation();
  const { status, loading } = useMcpStatus();
  const level = profile.llmAccess ?? "off";
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
  return (
    <>
      <span className="field-label">
        <span id={id}>{t("connections.form.llmAccess")}</span>
        <Hint k="hints.connections.llmAccess" label={t("connections.form.llmAccess")} />
      </span>
      <span className="llm-levels">
        <span role="radiogroup" aria-labelledby={id} aria-describedby={`${id}-note`}>
          {LEVELS.map((l) => (
            <label key={l} className="inline">
              <input type="radio" name={id} checked={level === l} onChange={() => onChange({ ...profile, llmAccess: l })} />
              {t(`connections.form.llmLevels.${l}`)}
            </label>
          ))}
        </span>
        <span id={`${id}-note`} className="muted" role="status">
          {note}
        </span>
      </span>
    </>
  );
}
