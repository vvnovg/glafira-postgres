import { isMacOS } from "../../lib/platform";

/**
 * Подсказка клавиши у кнопки (спец. U1 §8): `Mod` — ⌘ на macOS и Ctrl на других ОС, `Enter` — ⏎, `Shift` — ⇧.
 * Скрыта от экранного диктора: имя кнопки остаётся её текстом.
 */
export function Kbd({ keys }: { keys: string }) {
  const mac = isMacOS();
  const text = keys
    .split("+")
    .map((k) => (k === "Mod" ? (mac ? "⌘" : "Ctrl") : k === "Enter" ? "⏎" : k === "Shift" ? (mac ? "⇧" : "Shift") : k))
    .join(mac ? "" : "+");
  return (
    <kbd className="kbd" aria-hidden="true">
      {text}
    </kbd>
  );
}
