import { useTranslation } from "react-i18next";
import { CopyButton } from "../common/CopyButton";

/**
 * Только для чтения: текст DDL или определения в `<pre>` и кнопка копирования.
 * title — имя прокручиваемой области (скринридер), copyLabel — имя кнопки; по умолчанию — «DDL» и «Копировать DDL».
 */
export function DdlView({ ddl, title, copyLabel }: { ddl: string; title?: string; copyLabel?: string }) {
  const { t } = useTranslation();
  return (
    <div className="ddl">
      <div className="toolbar">
        <CopyButton text={ddl} label={copyLabel ?? t("object.copyDdl")} />
      </div>
      {/* Область с прокруткой достижима с клавиатуры (Tab, затем стрелки), поэтому ей нужны роль и имя. */}
      <pre role="group" tabIndex={0} aria-label={title ?? t("object.ddl")}>
        <code>{ddl}</code>
      </pre>
    </div>
  );
}
