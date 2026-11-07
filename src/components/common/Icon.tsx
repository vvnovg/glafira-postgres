import type { ReactNode, SVGAttributes } from "react";

/**
 * Набор значков интерфейса (спец. U1 §5): SVG 16×16, обводка 1.5, цвет `currentColor`. Значок без подписи
 * всегда лежит внутри кнопки с `aria-label`; сам он скрыт от экранного диктора.
 */
export type IconName =
  | "plus"
  | "folderPlus"
  | "folder"
  | "more"
  | "pencil"
  | "close"
  | "chevron"
  | "warning"
  | "lock"
  | "help"
  | "refresh"
  | "send"
  | "users"
  | "play"
  | "stop"
  | "copy"
  | "search"
  | "settings"
  | "heart"
  | "book"
  | "info"
  | "grid"
  | "list"
  | "flag"
  | "dot"
  // Значки Kubernetes-версии (5bfad06) — те же имена и рисунки: узлы, состояния, подвкладки.
  | "server"
  | "star"
  | "clock"
  | "code"
  | "logs"
  | "check"
  | "error"
  | "hourglass"
  // Значки PostgreSQL-версии (спец. переноса U1 §5; рисунки каталога — с доски «Единое дерево» прототипа).
  | "databases"
  | "database"
  | "schema"
  | "table"
  | "partition"
  | "view"
  | "matview"
  | "function"
  | "sequence"
  | "role"
  | "extension"
  | "replica"
  | "gauge"
  | "pulse"
  | "sessions"
  | "horizon"
  | "replication"
  | "slot"
  | "failover"
  | "health"
  | "plan"
  | "arrowUp"
  | "arrowDown"
  | "chevrons"
  | "skip";

const PATHS: Record<IconName, ReactNode> = {
  plus: <path d="M8 3v10M3 8h10" />,
  folderPlus: (
    <>
      <path d="M2 4.5h4l1.5 1.5H14v7.5H2z" />
      <path d="M8 8v4M6 10h4" />
    </>
  ),
  folder: <path d="M2 4.5h4l1.5 1.5H14v7.5H2z" />,
  more: (
    <>
      <circle cx="3.5" cy="8" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="12.5" cy="8" r="1.4" fill="currentColor" stroke="none" />
    </>
  ),
  pencil: <path d="M11 2.5l2.5 2.5L5.5 13H3v-2.5z" />,
  close: <path d="M4 4l8 8M12 4l-8 8" />,
  chevron: <path d="M6 4l4 4-4 4" />,
  warning: (
    <>
      <path d="M8 2.5l6 11H2z" />
      <path d="M8 6.5v3.2" />
      <circle cx="8" cy="11.6" r=".6" fill="currentColor" stroke="none" />
    </>
  ),
  lock: (
    <>
      <rect x="3" y="7" width="10" height="7" rx="1.5" />
      <path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
    </>
  ),
  help: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M6.3 6.3a1.7 1.7 0 1 1 2.6 1.4c-.6.4-.9.8-.9 1.3" />
      <circle cx="8" cy="11.3" r=".6" fill="currentColor" stroke="none" />
    </>
  ),
  refresh: (
    <>
      <path d="M13 8a5 5 0 1 1-1.6-3.7" />
      <path d="M13 2.5v3h-3" />
    </>
  ),
  send: <path d="M2.5 8L14 2.5 10 13.5 8 9z" />,
  users: (
    <>
      <circle cx="6" cy="5.5" r="2.3" />
      <path d="M1.8 13c.4-2.3 2.1-3.5 4.2-3.5s3.8 1.2 4.2 3.5" />
      <circle cx="11.5" cy="5.5" r="1.8" />
      <path d="M11 9.6c1.9 0 3 1.1 3.3 3" />
    </>
  ),
  play: <path d="M4 2.5v11l9-5.5z" fill="currentColor" stroke="none" />,
  stop: <rect x="3" y="3" width="10" height="10" rx="1.5" fill="currentColor" stroke="none" />,
  copy: (
    <>
      <rect x="5" y="5" width="8" height="8" rx="1.5" />
      <path d="M3 11V4a1 1 0 0 1 1-1h7" />
    </>
  ),
  search: (
    <>
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.5 10.5L14 14" />
    </>
  ),
  settings: (
    <>
      <path d="M2 4.5h12M2 8h12M2 11.5h12" />
      <circle cx="6" cy="4.5" r="1.5" fill="var(--sidebar, var(--bg))" />
      <circle cx="11" cy="8" r="1.5" fill="var(--sidebar, var(--bg))" />
      <circle cx="5" cy="11.5" r="1.5" fill="var(--sidebar, var(--bg))" />
    </>
  ),
  heart: <path d="M8 13.5S2.5 10 2.5 6.2A2.9 2.9 0 0 1 8 4.6a2.9 2.9 0 0 1 5.5 1.6C13.5 10 8 13.5 8 13.5z" />,
  book: <path d="M2.5 3h4a1.5 1.5 0 0 1 1.5 1.5V13a1.5 1.5 0 0 0-1.5-1.5h-4zM13.5 3h-4A1.5 1.5 0 0 0 8 4.5V13a1.5 1.5 0 0 1 1.5-1.5h4z" />,
  info: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 7v4" />
      <circle cx="8" cy="5" r=".6" fill="currentColor" stroke="none" />
    </>
  ),
  grid: (
    <>
      <rect x="2.5" y="2.5" width="5" height="5" rx="1" />
      <rect x="8.5" y="2.5" width="5" height="5" rx="1" />
      <rect x="2.5" y="8.5" width="5" height="5" rx="1" />
      <rect x="8.5" y="8.5" width="5" height="5" rx="1" />
    </>
  ),
  list: <path d="M2.5 4h11M2.5 8h11M2.5 12h7" />,
  flag: (
    <>
      <path d="M4 2.5v11" />
      <path d="M4 3h8l-2 3 2 3H4" />
    </>
  ),
  dot: <circle cx="8" cy="8" r="4" fill="currentColor" stroke="none" />,
  server: (
    <>
      <rect x="2.5" y="2.5" width="11" height="4.5" rx="1" />
      <rect x="2.5" y="9" width="11" height="4.5" rx="1" />
      <circle cx="5" cy="4.75" r=".7" fill="currentColor" stroke="none" />
      <circle cx="5" cy="11.25" r=".7" fill="currentColor" stroke="none" />
    </>
  ),
  star: <path d="M8 2.2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4l-3.6 1.9.7-4L2.2 6.5l4-.6z" fill="currentColor" stroke="none" />,
  clock: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5v3.2l2.2 1.3" />
    </>
  ),
  code: <path d="M5.5 4.5L2 8l3.5 3.5M10.5 4.5L14 8l-3.5 3.5" />,
  logs: (
    <>
      <path d="M2.5 4.5l3 2.5-3 2.5" />
      <path d="M7.5 9.5h6M2.5 13h11" />
    </>
  ),
  check: <path d="M3 8.5l3.2 3L13 4.5" />,
  error: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M5.8 5.8l4.4 4.4M10.2 5.8l-4.4 4.4" />
    </>
  ),
  hourglass: <path d="M4 2.5h8M4 13.5h8M5 2.5v2.3c0 1.2.7 2 1.8 2.6L8 8l1.2-.6C10.3 6.8 11 6 11 4.8V2.5M5 13.5v-2.3c0-1.2.7-2 1.8-2.6L8 8l1.2.6c1.1.6 1.8 1.4 1.8 2.6v2.3" />,
  databases: (
    <>
      <ellipse cx="8" cy="4" rx="5" ry="2" />
      <path d="M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4" />
      <path d="M3 8c0 1.1 2.2 2 5 2s5-.9 5-2" />
    </>
  ),
  database: (
    <>
      <ellipse cx="8" cy="4" rx="5" ry="2" />
      <path d="M3 4v8c0 1.1 2.2 2 5 2s5-.9 5-2V4" />
    </>
  ),
  schema: (
    <>
      <rect x="2.5" y="2.5" width="11" height="11" rx="1.5" />
      <path d="M2.5 6.5h11M6.5 6.5v7" />
    </>
  ),
  table: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="1.5" />
      <path d="M2.5 6.5h11M6.5 6.5V13M10 6.5V13" />
    </>
  ),
  partition: (
    <>
      <rect x="2.5" y="3" width="11" height="10" rx="1.5" />
      <path d="M2.5 6.5h11" />
    </>
  ),
  view: (
    <>
      <path d="M1.5 8s2.5-4 6.5-4 6.5 4 6.5 4-2.5 4-6.5 4S1.5 8 1.5 8z" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  matview: (
    <>
      <path d="M1.5 7s2.5-4 6.5-4 6.5 4 6.5 4-2.5 4-6.5 4S1.5 7 1.5 7z" />
      <circle cx="8" cy="7" r="2" />
      <path d="M3 13.5h10" />
    </>
  ),
  function: <path d="M4.5 13c2.2 0 2.3-2.5 2.8-5.5S8.3 3 10.5 3M5 8h5" />,
  sequence: <path d="M2.5 10h3v3.5h-3zM6.5 7h3v6.5h-3zM10.5 3.5h3v10h-3z" />,
  role: (
    <>
      <circle cx="8" cy="5.5" r="2.5" />
      <path d="M3 13.5c.5-3 2.5-4.5 5-4.5s4.5 1.5 5 4.5" />
    </>
  ),
  extension: (
    <>
      <rect x="3" y="3" width="10" height="10" rx="2" />
      <path d="M8 3v3M3 8h3" />
    </>
  ),
  replica: <circle cx="8" cy="8" r="4.5" />,
  gauge: (
    <>
      <path d="M2.5 11a5.5 5.5 0 1 1 11 0" />
      <path d="M8 11l2.5-3.5" />
      <circle cx="8" cy="11" r=".9" fill="currentColor" stroke="none" />
    </>
  ),
  pulse: <path d="M1.5 8h3l2-4 3 8 2-4h3" />,
  sessions: <path d="M3 4h10M3 8h10M3 12h6" />,
  horizon: <path d="M1.5 12h13M4.5 12a3.5 3.5 0 0 1 7 0M8 4.5V6M3.8 6.8l1 1M12.2 6.8l-1 1" />,
  replication: <path d="M3 7.5a5 5 0 0 1 8.6-3.3M11.5 1.8v2.8h-2.8M13 8.5a5 5 0 0 1-8.6 3.3M4.5 14.2v-2.8h2.8" />,
  slot: (
    <>
      <rect x="2.5" y="5.5" width="11" height="5" rx="1.5" />
      <path d="M5.5 8h5" />
    </>
  ),
  failover: <path d="M3 5.5h9.5M10 3l2.5 2.5L10 8M13 10.5H3.5M6 8l-2.5 2.5L6 13" />,
  health: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 5.5v5M5.5 8h5" />
    </>
  ),
  plan: (
    <>
      <circle cx="4" cy="3.5" r="1.5" />
      <circle cx="11" cy="8" r="1.5" />
      <circle cx="11" cy="12.5" r="1.5" />
      <path d="M4 5v7.5h5.5M4 8h5.5" />
    </>
  ),
  arrowUp: <path d="M8 13V3M4 7l4-4 4 4" />,
  arrowDown: <path d="M8 3v10M4 9l4 4 4-4" />,
  chevrons: <path d="M4 4l4 4-4 4M8.5 4l4 4-4 4" />,
  skip: (
    <>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M5.5 8h5" />
    </>
  ),
};

export const ICON_NAMES = Object.keys(PATHS) as IconName[];

interface Props extends Omit<SVGAttributes<SVGSVGElement>, "children"> {
  name: IconName;
  size?: number;
}

export function Icon({ name, size = 16, className, ...rest }: Props) {
  return (
    <svg
      className={className ? `icon-svg ${className}` : "icon-svg"}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      data-icon={name}
      {...rest}
    >
      {PATHS[name]}
    </svg>
  );
}
