import { describe, expect, it } from "vitest";
import { installPolyfills } from "./polyfills";

describe("installPolyfills", () => {
  it("adds Object.hasOwn when the engine lacks it", () => {
    const target = {} as { hasOwn?: (o: object, k: PropertyKey) => boolean };
    installPolyfills(target as ObjectConstructor);
    expect(typeof target.hasOwn).toBe("function");
    const proto = { inherited: 1 };
    const obj = Object.assign(Object.create(proto), { own: 1 });
    expect(target.hasOwn!(obj, "own")).toBe(true);
    expect(target.hasOwn!(obj, "inherited")).toBe(false);
    expect(target.hasOwn!(Object.create(null), "x")).toBe(false);
    expect(Object.keys(target)).toEqual([]);
  });

  it("keeps a native Object.hasOwn", () => {
    const hasOwn = () => (Object as { hasOwn?: unknown }).hasOwn;
    const native = hasOwn();
    expect(typeof native).toBe("function");
    installPolyfills();
    expect(hasOwn()).toBe(native);
  });

  it("is installed before the app in main.tsx", async () => {
    const main = (await import("./main.tsx?raw")).default as string;
    const firstImport = /^import\s+["']([^"']+)["']/m.exec(main)?.[1];
    expect(firstImport).toBe("./polyfills");
  });
});
