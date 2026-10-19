import { describe, expect, it } from "vitest";
import type { ConnectionFolder, ConnectionProfile } from "../../../api/types";
import { newProfile } from "../profile";
import { buildTree, canMoveFolder, filterTree, folderPaths, type TreeNode } from "./folderTree";

const folder = (id: string, name: string, parentId: string | null = null, order = 0): ConnectionFolder => ({ id, name, parentId, order });
const conn = (id: string, name: string, folderId: string | null = null, order = 0): ConnectionProfile => ({
  ...newProfile(),
  id,
  name,
  folderId,
  order,
});

/** Компактная запись дерева для сравнения: папка — "имя/[дети]", подключение — имя. */
function shape(nodes: TreeNode[]): unknown[] {
  return nodes.map((n) => (n.kind === "folder" ? { [n.folder.name]: shape(n.children) } : n.connection.name));
}

describe("buildTree", () => {
  it("папки первыми, внутри уровня по order, затем по имени", () => {
    const tree = buildTree(
      [folder("f2", "Бета", null, 1), folder("f1", "Альфа", null, 1), folder("f0", "Яблоко", null, 0)],
      [conn("c1", "zeta", null, 0), conn("c2", "beta", null, 2), conn("c3", "alpha", null, 2)],
    );
    expect(shape(tree)).toEqual([{ Яблоко: [] }, { Альфа: [] }, { Бета: [] }, "zeta", "alpha", "beta"]);
  });

  it("вложенность в три уровня", () => {
    const tree = buildTree(
      [folder("a", "A"), folder("b", "B", "a"), folder("c", "C", "b")],
      [conn("x", "x", "c"), conn("y", "y", "a"), conn("z", "z")],
    );
    expect(shape(tree)).toEqual([{ A: [{ B: [{ C: ["x"] }] }, "y"] }, "z"]);
  });

  it("подключение с несуществующей папкой и без order — в корне", () => {
    const orphan = { ...conn("o", "orphan", "missing"), order: undefined };
    const tree = buildTree([folder("a", "A", "missing")], [orphan, conn("n", "named")]);
    expect(shape(tree)).toEqual([{ A: [] }, "named", "orphan"]);
  });
});

describe("filterTree", () => {
  const tree = buildTree(
    [folder("a", "Проект А"), folder("b", "ИФТ", "a"), folder("c", "Прочее")],
    [conn("x", "kafka-ift", "b"), conn("y", "kafka-dev", "a"), conn("z", "local", "c")],
  );

  it("пустой запрос возвращает дерево как есть", () => {
    expect(filterTree(tree, "  ")).toBe(tree);
  });

  it("совпало подключение — остаются его предки", () => {
    expect(shape(filterTree(tree, "IFT"))).toEqual([{ "Проект А": [{ ИФТ: ["kafka-ift"] }] }]);
  });

  it("совпала папка — остаётся целиком", () => {
    expect(shape(filterTree(tree, "ифт"))).toEqual([{ "Проект А": [{ ИФТ: ["kafka-ift"] }] }]);
    expect(shape(filterTree(tree, "проект"))).toEqual([{ "Проект А": [{ ИФТ: ["kafka-ift"] }, "kafka-dev"] }]);
  });

  it("ничего не совпало — пусто", () => {
    expect(filterTree(tree, "nope")).toEqual([]);
  });
});

describe("folderPaths", () => {
  it("пути от корня, отсортированы по пути", () => {
    const paths = folderPaths([folder("b", "ИФТ", "a"), folder("c", "Б"), folder("a", "А"), folder("d", "ПРОМ", "a")]);
    expect(paths).toEqual([
      { id: "a", path: ["А"] },
      { id: "b", path: ["А", "ИФТ"] },
      { id: "d", path: ["А", "ПРОМ"] },
      { id: "c", path: ["Б"] },
    ]);
  });
});

describe("canMoveFolder", () => {
  const folders = [folder("a", "A"), folder("b", "B", "a"), folder("c", "C", "b"), folder("d", "D")];

  it("нельзя в себя и в потомка", () => {
    expect(canMoveFolder(folders, "a", "a")).toBe(false);
    expect(canMoveFolder(folders, "a", "b")).toBe(false);
    expect(canMoveFolder(folders, "a", "c")).toBe(false);
  });

  it("можно в корень, к соседу и к предку", () => {
    expect(canMoveFolder(folders, "c", null)).toBe(true);
    expect(canMoveFolder(folders, "a", "d")).toBe(true);
    expect(canMoveFolder(folders, "c", "a")).toBe(true);
  });

  it("нельзя в несуществующую папку", () => {
    expect(canMoveFolder(folders, "a", "missing")).toBe(false);
  });
});
