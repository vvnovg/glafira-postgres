import { act } from "@testing-library/react";
import { vi } from "vitest";

/**
 * Шина событий Tauri для тестов: подписчики по имени события; отписка убирает своего подписчика.
 * Подключение в тесте (фабрика `vi.mock` поднимается выше импортов, поэтому модуль берётся динамически):
 * `vi.mock("@tauri-apps/api/event", async () => (await import("<путь>/test/events")).tauriEventMock());`
 */
export const eventBus = { handlers: [] as { name: string; fn: (e: { payload: unknown }) => void }[] };

export function tauriEventMock() {
  return {
    listen: vi.fn(async (name: string, fn: (e: { payload: unknown }) => void) => {
      const h = { name, fn };
      eventBus.handlers.push(h);
      return () => {
        eventBus.handlers = eventBus.handlers.filter((x) => x !== h);
      };
    }),
  };
}

/** Подписчики события `name` сейчас. */
export const listeners = (name: string) => eventBus.handlers.filter((h) => h.name === name);

/** Доставляет событие всем его подписчикам (внутри `act`). */
export function emitEvent(name: string, payload: unknown): void {
  act(() => {
    for (const h of listeners(name)) h.fn({ payload });
  });
}

/** Забывает подписчиков: вызывать в `beforeEach`. */
export function resetEventBus(): void {
  eventBus.handlers = [];
}
