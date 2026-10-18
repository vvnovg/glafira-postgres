// Полифилы для WebKit на macOS 11–12 (Safari 14–15.3). Vite понижает синтаксис (build.target), но не API.

type HasOwn = (o: object, k: PropertyKey) => boolean;

/** Ставит недостающие API на target (по умолчанию Object). Встроенную реализацию не трогает. */
export function installPolyfills(target: ObjectConstructor = Object): void {
  // react-markdown вызывает Object.hasOwn без проверки (Safari 15.4+). lib в tsconfig — ES2020, поэтому через приведение.
  if (typeof (target as { hasOwn?: HasOwn }).hasOwn !== "function") {
    Object.defineProperty(target, "hasOwn", {
      value: ((o, k) => Object.prototype.hasOwnProperty.call(o, k)) as HasOwn,
      configurable: true,
      writable: true,
    });
  }
}

installPolyfills();
