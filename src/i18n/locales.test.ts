import { afterAll, describe, expect, it } from "vitest";
import { DICTIONARIES, i18n, setLanguage } from "./index";
import { LANGUAGES } from "./languages";

type Dict = Record<string, unknown>;
function get(dict: Dict, key: string): unknown {
  return key.split(".").reduce<unknown>((o, k) => (o && typeof o === "object" ? (o as Dict)[k] : undefined), dict);
}
function flatKeys(dict: Dict, prefix = ""): string[] {
  return Object.entries(dict).flatMap(([k, v]) =>
    v && typeof v === "object" ? flatKeys(v as Dict, `${prefix}${k}.`) : [`${prefix}${k}`],
  );
}
const pluralStems = flatKeys(DICTIONARIES.en).filter((k) => k.endsWith("_one")).map((k) => k.slice(0, -4));

afterAll(() => i18n.changeLanguage("ru"));

describe("словари", () => {
  it("загружены en и ru, каждый словарь — язык из списка", () => {
    expect(Object.keys(DICTIONARIES)).toEqual(expect.arrayContaining(["en", "ru"]));
    for (const code of Object.keys(DICTIONARIES)) expect(LANGUAGES.map((l) => l.code)).toContain(code);
  });

  for (const code of Object.keys(DICTIONARIES)) {
    it(`${code}: переключение и плюральные формы`, async () => {
      await setLanguage(code);
      expect(i18n.language).toBe(code);
      expect(document.documentElement.lang).toBe(code);
      expect(i18n.t("common.save")).toBe(get(DICTIONARIES[code], "common.save"));
      const rules = new Intl.PluralRules(code);
      for (const stem of pluralStems) {
        for (const n of [0, 1, 2, 5, 21, 1_000_000]) {
          const form = `${stem}_${rules.select(n)}`;
          expect(get(DICTIONARIES[code], form), `${code}: ${form}`).toBeTypeOf("string");
        }
      }
    });
  }
});
