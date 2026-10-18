import { useTranslation } from "react-i18next";
import { proHint } from "../../lib/useLicense";

export function ProLock() {
  useTranslation(); // перерисовка подсказки при смене языка
  return (
    <span className="pro-lock" title={proHint()}>
      🔒 Pro
    </span>
  );
}
