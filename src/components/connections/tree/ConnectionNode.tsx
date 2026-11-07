import { useQueryClient } from "@tanstack/react-query";
import { type HTMLAttributes, useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { api, toAppError } from "../../../api/client";
import type { AppError, ConnectionProfile, TopologyView } from "../../../api/types";
import { tr } from "../../../i18n/text";
import { roleLabel } from "../../../lib/topology";
import { disconnectedView, ofConnection, refreshAfterConnect, useTopology } from "../../../lib/useConnectionEvents";
import { useWorkspace } from "../../../state/workspace";
import { EnvBadge } from "../../common/EnvBadge";
import { Glyph } from "../../common/Glyph";
import { Icon } from "../../common/Icon";
import { hostKeyUnknown } from "../profile";
import { TrustHostKeyDialog } from "../TrustHostKeyDialog";
import { CatalogNodes } from "./CatalogNodes";
import { FolderMenu, type FolderMenuItem } from "./FolderMenu";
import { LlmBadge } from "./LlmBadge";
import { TreeRow } from "./TreeRow";
import { inConnection, useActiveTab } from "./treeActive";
import { useFocusMenuItem } from "./treeFocus";

/**
 * Узел подключения: раскрытие открывает подключение (Rust поднимает наблюдение), дочерние узлы —
 * участники, базы, роли. rowProps — атрибуты строки (перетаскивание), как в Glafira. `defaultExpanded` — узел раскрыт
 * сразу (корень ветки в режиме фокуса): закрытое подключение при этом открывается, как при раскрытии щелчком.
 */
export function ConnectionNode({
  connection: c,
  rowProps,
  defaultExpanded = false,
}: {
  connection: ConnectionProfile;
  rowProps?: HTMLAttributes<HTMLDivElement>;
  defaultExpanded?: boolean;
}) {
  const { t } = useTranslation();
  const open = useWorkspace((s) => s.open);
  const qc = useQueryClient();
  const [expanded, setExpanded] = useState(defaultExpanded);
  const [menuOpen, setMenuOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [reopening, setReopening] = useState(false);
  const [error, setError] = useState<AppError | null>(null);
  // Ошибка ключа хоста, от диалога доверия по которой пользователь отказался: текст ошибки остаётся, диалог — нет.
  const [trustDismissed, setTrustDismissed] = useState<AppError | null>(null);
  const topologyQ = useTopology(c.id);
  const { data: view = disconnectedView } = topologyQ;
  const live = view.state === "connected" || view.state === "degraded";
  const focusItem = useFocusMenuItem({ kind: "connection", connectionId: c.id });
  const stateId = useId();
  const kindId = useId();
  const active = useActiveTab();

  // Локальная ошибка — про прошлую попытку или действие. Когда подключение снова здорово («connected»),
  // она устарела и не должна висеть под живым узлом.
  useEffect(() => {
    if (view.state === "connected") setError(null);
  }, [view.state]);

  const setView = (v: TopologyView) => qc.setQueryData(["topology", c.id], v);

  // Раскрыт сразу (фокус на подключении): как и раскрытие щелчком, открывает закрытое подключение — один раз, когда
  // состояние известно (до ответа топологии вид «закрыто» — заглушка, а не факт).
  const autoConnect = useRef(defaultExpanded);
  useEffect(() => {
    if (!autoConnect.current || !topologyQ.isSuccess) return;
    autoConnect.current = false;
    if (view.state === "disconnected") void connect();
  }, [topologyQ.isSuccess]);

  const connect = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setTrustDismissed(null);
    try {
      setView(await api.connect(c.id));
      // После отключения запросы каталога удалены, а открытые вкладки объектов могли получить отказ «нет подключения».
      refreshAfterConnect(qc, c.id);
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setBusy(false);
    }
  };

  /**
   * Переподключение живого подключения на месте. Пока оно идёт, пункт меню не предлагается (`reopening`), и повторные
   * нажатия не ставят переподключения в очередь. Флаг свой, не `busy`: иначе «Подключиться» после «Отключиться»
   * во время долгого переподключения молча пропадало бы.
   */
  const reconnect = async () => {
    if (reopening) return;
    setReopening(true);
    setError(null);
    setTrustDismissed(null);
    try {
      const v = await api.reconnect(c.id);
      setView(v);
      // Туннель, который не поднялся, оставляет подключение открытым (degraded) с ошибкой в виде, а не отказом команды.
      // Ошибку SSH поднимаем в локальную: текст виден под узлом (иначе при degraded он не показывался бы совсем),
      // а для неизвестного ключа хоста по ней же предлагается доверие — по действию пользователя.
      if (v.error?.code === "SSH_ERROR") setError(v.error);
      // Подключение осталось тем же (журнал и узел каталога сохранены), события Disconnected не было:
      // списки каталога и факты узлов, упавшие на отказе во входе, перечитываются сами.
      refreshAfterConnect(qc, c.id);
    } catch (e) {
      setError(toAppError(e));
    } finally {
      setReopening(false);
    }
  };

  /**
   * Повтор после доверия ключу хоста: сам Rust ничего не повторяет (туннель на стоянке ждёт «Переподключить»).
   * Открытое подключение переподключается на месте, закрытое или упавшее — подключается заново.
   */
  const retryAfterTrust = () => {
    setError(null);
    void (live ? reconnect() : connect());
  };

  /** Смена узла, с которого читается каталог: новый вид в кэш, каталог — перечитать заново. */
  const pickCatalogMember = (member: string | null) => {
    setError(null);
    void api
      .setCatalogMember(c.id, member)
      .then((v) => {
        setView(v);
        void qc.invalidateQueries({ predicate: ofConnection(["catalog"], c.id) });
      })
      .catch((e: unknown) => setError(toAppError(e)));
  };

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    if (next && !live && view.state !== "connecting") void connect();
  };

  const members = view.topology?.members ?? [];
  const menu: FolderMenuItem[] = [
    { label: t("sidebar.overview"), onSelect: () => open({ kind: "overview", connectionId: c.id }) },
    ...focusItem,
    { label: t("sidebar.pulse"), onSelect: () => open({ kind: "diag", connectionId: c.id, tool: "pulse" }) },
    { label: t("sidebar.editConnection"), onSelect: () => open({ kind: "connection-form", connectionId: c.id }) },
    {
      label: t("sidebar.duplicateConnection"),
      onSelect: () => open({ kind: "connection-form", connectionId: null, copyOf: c.id, folderId: c.folderId ?? null }),
    },
    ...(live && c.kind !== "server"
      ? [
          { label: t("sidebar.catalogDefault"), onSelect: () => pickCatalogMember(null) },
          ...members.map((m) => ({
            label: t("sidebar.catalogFrom", { member: m.name }),
            onSelect: () => pickCatalogMember(m.name),
          })),
        ]
      : []),
    // Упавшее подключение можно и повторить, и закрыть (иначе повтор — только свернуть и раскрыть узел).
    ...(view.state === "failed"
      ? [
          {
            label: t("sidebar.reconnect"),
            onSelect: () => {
              setExpanded(true);
              void connect();
            },
          },
        ]
      : []),
    // Живое подключение можно переподключить на месте: после смены пароля на сервере или сбоя LDAP отказ во входе
    // запоминается до переподключения (errors.pg.loginNotRetried); журнал и узел каталога при этом остаются. Пока идёт
    // подключение (connecting), пункта нет: переподключать ещё нечего; пока идёт само переподключение — тоже.
    ...(live && !reopening ? [{ label: t("sidebar.reopen"), onSelect: () => void reconnect() }] : []),
    ...(live || view.state === "failed"
      ? [
          {
            label: t("sidebar.disconnect"),
            onSelect: () =>
              void api
                .disconnect(c.id)
                .then(() => {
                  setError(null);
                  setExpanded(false);
                })
                .catch((e: unknown) => setError(toAppError(e))),
          },
        ]
      : [
          {
            label: t("sidebar.connect"),
            onSelect: () => {
              setExpanded(true);
              void connect();
            },
          },
        ]),
  ];

  // Ошибка последней попытки или причина, по которой Rust пометил подключение как недоступное.
  const shownError = error ?? (view.state === "failed" ? view.error : null);
  // У одиночного сервера группы «Узлы» нет, и роль (primary или standby) показывается в заголовке (спека P1 §8).
  const serverRole = c.kind === "server" ? view.topology?.members[0]?.role : undefined;
  const kindText = c.kind === "patroni" ? "Patroni" : serverRole !== undefined && serverRole !== "unknown" ? roleLabel(serverRole, "server") : null;
  // Диалог доверия — только для ошибки, которой можно доверять, и по собственному действию (не по фоновому событию).
  const trustError = hostKeyUnknown(error) && error !== trustDismissed ? error : null;

  // Подсвечено имя подключения: вкладка «Обзор» — его узел; вкладка внутри подключения, пока ветка не видна (свёрнута или
  // подключение не открыто) — тоже (спец. переноса U1 §3.3).
  const current = (active?.kind === "overview" && active.connectionId === c.id) || (inConnection(active, c.id) && (!expanded || !live));

  // Раздельный режим TreeRow: шеврон подключает и раскрывает, имя открывает «Обзор». Значок — точка цвета подключения
  // с отметкой состояния; состояние описывает кнопку имени (aria-describedby).
  return (
    <li>
      <TreeRow
        chevron={{ expanded, onToggle: toggle, label: t("sidebar.connectionToggle", { name: c.name }) }}
        icon={
          <span className="conn-icon">
            <span className="dot" style={{ background: c.color }} />
            <Glyph id={stateId} className={`conn-state conn-state-${view.state}`} symbol="" label={t(`sidebar.state.${view.state}`)} />
          </span>
        }
        name={c.name}
        title={c.name}
        // Вид (Patroni) и роль одиночного сервера — справа в строке: в узкой панели они уступают место имени первыми.
        extra={kindText ? <span id={kindId} className="kind-badge">{kindText}</span> : undefined}
        current={current}
        onActivate={() => open({ kind: "overview", connectionId: c.id })}
        activateLabel={c.name}
        activateDescribedBy={kindText ? `${stateId} ${kindId}` : stateId}
        rowProps={{
          ...rowProps,
          className: rowProps?.className ? `tree-conn ${rowProps.className}` : "tree-conn",
          onContextMenu: (e) => {
            e.preventDefault();
            setMenuOpen(true);
          },
          // Пока открыто меню, строку не перетаскивают (выделение текста пунктов не начинает перенос).
          draggable: rowProps?.draggable && !menuOpen,
        }}
      >
        <EnvBadge environment={c.environment} readOnly={c.readOnly} />
        <LlmBadge connection={c} />
        <span className="tree-menu-anchor">
          <button
            type="button"
            className="icon"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            aria-label={t("sidebar.connectionMenu", { name: c.name })}
            onClick={() => setMenuOpen(!menuOpen)}
          >
            <Icon name="more" />
          </button>
          {menuOpen && <FolderMenu label={t("sidebar.connectionMenu", { name: c.name })} items={menu} onClose={() => setMenuOpen(false)} />}
        </span>
      </TreeRow>
      {trustError && <TrustHostKeyDialog error={trustError} onTrusted={retryAfterTrust} onCancel={() => setTrustDismissed(trustError)} />}
      {expanded && (
        <>
          {(view.state === "connecting" || busy) && (
            <p className="tree-notice" role="status">
              {t("sidebar.state.connecting")}
            </p>
          )}
          {shownError && (
            <p className="tree-error" role="alert">
              {tr(shownError)}
            </p>
          )}
          {live && <CatalogNodes connection={c} view={view} />}
        </>
      )}
    </li>
  );
}
