import type { HTMLAttributes, ReactNode, Ref } from "react";
import { Icon, type IconName } from "../../common/Icon";

/** Кнопка-шеврон раскрытия: состояние и подпись для экранного диктора. */
export interface TreeToggle {
  expanded: boolean;
  onToggle: () => void;
  label: string;
  /** id кнопки раскрытия: по нему вложенный список называется (`aria-labelledby`). */
  id?: string;
  /** Кнопка раскрытия — для возврата фокуса из контекстного меню строки. */
  buttonRef?: Ref<HTMLButtonElement>;
}

interface Props {
  /** Шеврон раскрытия; `"leaf"` — пустая колонка той же ширины (имена одного уровня выровнены); `"none"` — без колонки. */
  chevron?: TreeToggle | "leaf" | "none";
  /** Значок типа узла или свой элемент (точка состояния подключения). */
  icon?: IconName | ReactNode;
  name: ReactNode;
  /** Полное имя для подсказки, когда оно обрезано «…». */
  title?: string;
  /** Дополнительный текст справа: счётчик, размер. */
  extra?: ReactNode;
  /** Узел активной вкладки: заливка акцентом и `aria-current`. */
  current?: boolean;
  bold?: boolean;
  /**
   * Вся строка — кнопка (лист дерева, пункт нижней панели). Вместе с шевроном-кнопкой — раздельный режим (PostgreSQL-версия,
   * спец. переноса U1 §3.1): шеврон — своя кнопка, значок, имя и дополнительный текст — вторая, с `aria-current`.
   */
  onActivate?: () => void;
  /** Подпись кнопки строки, когда видимого имени мало («Открыть …»). */
  activateLabel?: string;
  /** id описания кнопки строки (`aria-describedby`): состояние подключения в значке. */
  activateDescribedBy?: string;
  className?: string;
  /** Атрибуты строки-контейнера: перетаскивание, контекстное меню. */
  rowProps?: HTMLAttributes<HTMLDivElement>;
  /** Хвост строки: бейджи, кнопки меню и правки. */
  children?: ReactNode;
}

function cls(...parts: (string | false | undefined | null)[]): string {
  return parts.filter(Boolean).join(" ");
}

/**
 * Строка единого дерева (спец. U1 §6): 24 px, колонка шеврона 16 px, значок типа, имя с обрезкой, дополнительный
 * текст справа. Отступ уровня задаёт вложенный `ul` (`padding-left: 14px`), а не сама строка.
 */
export function TreeRow({ chevron = "leaf", icon, name, title, extra, current, bold, onActivate, activateLabel, activateDescribedBy, className, rowProps, children }: Props) {
  const iconEl = icon === undefined ? null : typeof icon === "string" ? <Icon name={icon as IconName} className="tree-icon" /> : <span className="tree-icon">{icon}</span>;
  const nameEl = (
    <span className="tree-name" title={title}>
      {name}
    </span>
  );
  const extraEl = extra !== undefined && extra !== null ? <span className="tree-extra">{extra}</span> : null;
  const rowClass = cls("tree-row", bold && "bold", current && "current", className, rowProps?.className);

  if (typeof chevron === "object" && onActivate) {
    return (
      <div {...rowProps} className={cls(rowClass, "split")}>
        <button
          type="button"
          id={chevron.id}
          ref={chevron.buttonRef}
          className="tree-chevron-button"
          aria-expanded={chevron.expanded}
          aria-label={chevron.label}
          title={chevron.label}
          onClick={chevron.onToggle}
        >
          <span className={chevron.expanded ? "tree-chevron open" : "tree-chevron"} aria-hidden="true">
            <Icon name="chevron" />
          </span>
        </button>
        <button
          type="button"
          className="tree-main"
          aria-current={current ? "page" : undefined}
          aria-label={activateLabel}
          aria-describedby={activateDescribedBy}
          title={title}
          onClick={onActivate}
        >
          {iconEl}
          {nameEl}
          {extraEl}
        </button>
        {children}
      </div>
    );
  }

  if (typeof chevron === "object") {
    return (
      <div {...rowProps} className={rowClass}>
        <button
          type="button"
          id={chevron.id}
          ref={chevron.buttonRef}
          className="tree-toggle"
          aria-expanded={chevron.expanded}
          aria-label={chevron.label}
          onClick={chevron.onToggle}
        >
          <span className={chevron.expanded ? "tree-chevron open" : "tree-chevron"} aria-hidden="true">
            <Icon name="chevron" />
          </span>
          {iconEl}
          {nameEl}
          {extraEl}
        </button>
        {children}
      </div>
    );
  }

  const placeholder = chevron === "leaf" ? <span className="tree-chevron" aria-hidden="true" /> : null;
  if (onActivate) {
    return (
      <button
        type="button"
        className={rowClass}
        aria-current={current ? "page" : undefined}
        aria-label={activateLabel}
        aria-describedby={activateDescribedBy}
        title={title}
        onClick={onActivate}
      >
        {placeholder}
        {iconEl}
        {nameEl}
        {extraEl}
        {children}
      </button>
    );
  }
  return (
    <div {...rowProps} className={rowClass}>
      {placeholder}
      {iconEl}
      {nameEl}
      {extraEl}
      {children}
    </div>
  );
}
