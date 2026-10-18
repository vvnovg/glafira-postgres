import { describe, expect, it } from "vitest";
import { i18n } from "./index";
import { tr } from "./text";

describe("tr", () => {
  it("ключ из словаря, иначе запасной текст", async () => {
    i18n.addResource("ru", "translation", "errors.test.x", "Подключение «{{name}}»");
    expect(tr({ key: "errors.test.x", params: { name: "a" }, message: "Connection a" })).toBe("Подключение «a»");
    expect(tr({ key: "errors.unknown", message: "Fallback text" })).toBe("Fallback text");
    expect(tr({ key: "errors.generic", params: { message: "как есть" }, message: "как есть" })).toBe("как есть");
  });

  it("плюральный ключ: count из строки выбирает форму", () => {
    i18n.addResource("ru", "translation", "warnings.test.items_one", "{{count}} подключение");
    i18n.addResource("ru", "translation", "warnings.test.items_few", "{{count}} подключения");
    i18n.addResource("ru", "translation", "warnings.test.items_many", "{{count}} подключений");
    i18n.addResource("ru", "translation", "warnings.test.items_other", "{{count}} подключения");
    expect(tr({ key: "warnings.test.items", params: { count: "1" }, text: "1 connection" })).toBe("1 подключение");
    expect(tr({ key: "warnings.test.items", params: { count: "3" }, text: "3 connections" })).toBe("3 подключения");
    expect(tr({ key: "warnings.test.items", params: { count: "5" }, text: "5 connections" })).toBe("5 подключений");
  });

  it("без ключа — text, затем message", () => {
    expect(tr({ text: "Plain", message: "other" })).toBe("Plain");
    expect(tr({ message: "only message" })).toBe("only message");
    expect(tr({})).toBe("");
  });
});
