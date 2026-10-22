import { openUrl } from "@tauri-apps/plugin-opener";

/** Страница поддержки проекта — один адрес на все входы: низ левой панели, «О программе», меню Help. */
export const SUPPORT_URL = "https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md";
/** Текст лицензии Apache 2.0 в репозитории исходного кода. */
export const LICENSE_URL = "https://github.com/vvnovg/glafira-postgres/blob/main/LICENSE";

/** Открывает страницу поддержки в системном браузере. */
export function openSupport(): void {
  void openUrl(SUPPORT_URL);
}

/** Открывает текст лицензии в системном браузере. */
export function openLicense(): void {
  void openUrl(LICENSE_URL);
}
