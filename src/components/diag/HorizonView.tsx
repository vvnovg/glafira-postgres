import { useTranslation } from "react-i18next";
import type { DiagReport, Holder, HolderKind, HorizonData } from "../../api/types";
import { formatCount, formatDuration } from "../../lib/format";
import { quoteLiteral } from "../../lib/sql";
import { useWorkspace } from "../../state/workspace";
import { CopyButton } from "../common/CopyButton";
import { metaText, uniqueKeys } from "./viewText";

/** Подпись вида держателя: ключ выбирается по состоянию, поэтому — статическими литералами. */
const KIND_KEYS: Record<HolderKind, string> = {
  session: "diag.horizon.kinds.session",
  preparedXact: "diag.horizon.kinds.preparedXact",
  slot: "diag.horizon.kinds.slot",
  standbyFeedback: "diag.horizon.kinds.standbyFeedback",
};

/** Ключ строки: вид, имя и всё, что отличает одноимённых (pid переиспользуется, у реплик бывает один `application_name`). */
const baseKey = (h: Holder) => `${h.kind}:${h.name}:${h.pid ?? ""}:${h.backendStartUs ?? ""}:${h.clientAddr ?? ""}`;

function HolderRow({ h, connectionId, here }: { h: Holder; connectionId: string; here: string | null }) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  let main = h.name;
  let meta: string;
  switch (h.kind) {
    case "session":
      main = t("diag.horizon.pid", { pid: h.pid ?? h.name });
      meta = metaText([h.user, h.application, h.database, h.state, h.clientAddr]);
      break;
    case "preparedXact":
      main = h.gid ?? h.name;
      meta = metaText([h.user, h.database]);
      break;
    case "slot":
      meta = metaText([h.slotType, h.active === null ? null : t(h.active ? "diag.slots.active" : "diag.slots.inactive"), h.database, h.application, h.clientAddr]);
      break;
    case "standbyFeedback":
      meta = metaText([h.application !== h.name ? h.application : null, h.user, h.clientAddr]);
      break;
  }
  // Откатить prepared transaction — действие пользователя: приложение команду только показывает.
  const command = h.kind === "preparedXact" ? `ROLLBACK PREPARED ${quoteLiteral(h.gid ?? h.name)};` : null;
  // Горизонт держит реплика (feedback или физический слот с её отправителем): чинить нужно на ней. Ссылка на себя бессмысленна.
  const member = h.member !== null && h.member !== here ? h.member : null;
  return (
    <tr>
      <td>{h.kind === "slot" && h.catalog ? t("diag.horizon.kinds.slotCatalog") : t(KIND_KEYS[h.kind])}</td>
      <td>
        <div>
          <strong>{main}</strong>
        </div>
        {meta && <div className="muted">{meta}</div>}
        {command !== null && (
          <div className="diag-command">
            <code>{command}</code>
            <CopyButton text={command} label={t("diag.findings.copy")} />
          </div>
        )}
        {member !== null && (
          <button type="button" className="link" onClick={() => open({ kind: "diag", connectionId, tool: "horizon", member })}>
            {t("diag.horizon.openOn", { member })}
          </button>
        )}
      </td>
      <td className="num">{formatCount(h.xminAge)}</td>
      <td className="num">{formatDuration(h.durationS)}</td>
    </tr>
  );
}

function HorizonBody({ connectionId, data, member }: { connectionId: string; data: HorizonData; member: string | null }) {
  const { t } = useTranslation();
  const keys = uniqueKeys(data.holders.map(baseKey));
  return (
    <div className="diag-view">
      {data.hotStandbyFeedback !== null && <p className="muted">{t("diag.horizon.feedback", { value: data.hotStandbyFeedback ? "on" : "off" })}</p>}
      {data.holders.length === 0 ? (
        <p className="muted">{t("diag.horizon.none")}</p>
      ) : (
        <div className="table-scroll">
          <table className="diag-table" aria-label={t("diag.horizon.table")}>
            <thead>
              <tr>
                <th scope="col">{t("diag.horizon.columns.kind")}</th>
                <th scope="col">{t("diag.horizon.columns.object")}</th>
                <th scope="col">{t("diag.horizon.columns.xminAge")}</th>
                <th scope="col">{t("diag.horizon.columns.duration")}</th>
              </tr>
            </thead>
            <tbody>
              {data.holders.map((h, i) => (
                <HolderRow key={keys[i]} h={h} connectionId={connectionId} here={member} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

/**
 * Горизонт xmin (спец. P2 §5.5): держатели по убыванию возраста xmin — кто мешает VACUUM убирать мёртвые строки.
 * Реплика, держащая горизонт, открывается ссылкой («чинить на ней»); у prepared transaction — готовая команда отката.
 */
export function HorizonView({ connectionId, report }: { connectionId: string; report: DiagReport }) {
  if (report.data.kind !== "horizon") return null;
  return <HorizonBody connectionId={connectionId} data={report.data} member={report.member} />;
}
