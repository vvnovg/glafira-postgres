import type { ConnectionFolder, ConnectionProfile } from "../../../api/types";

export type TreeNode =
  | { kind: "folder"; folder: ConnectionFolder; children: TreeNode[] }
  | { kind: "connection"; connection: ConnectionProfile };

const byName = (a: string, b: string) => a.localeCompare(b, undefined, { sensitivity: "base", numeric: true });

/** Порядок внутри уровня: по order (без order — в конце), затем по имени. */
function byOrderThenName(a: { order?: number; name: string }, b: { order?: number; name: string }): number {
  const oa = a.order ?? Number.POSITIVE_INFINITY;
  const ob = b.order ?? Number.POSITIVE_INFINITY;
  if (oa !== ob) return oa < ob ? -1 : 1;
  return byName(a.name, b.name);
}

/**
 * Дерево из плоских списков: папки первыми, внутри уровня — по order, затем по имени.
 * Ссылка на несуществующую папку (у папки или подключения) означает корень.
 */
export function buildTree(folders: ConnectionFolder[], connections: ConnectionProfile[]): TreeNode[] {
  const known = new Set(folders.map((f) => f.id));
  const parentOf = (id: string | null | undefined) => (id && known.has(id) ? id : null);

  const foldersByParent = new Map<string | null, ConnectionFolder[]>();
  for (const f of folders) {
    const key = parentOf(f.parentId);
    foldersByParent.set(key, [...(foldersByParent.get(key) ?? []), f]);
  }
  const connsByFolder = new Map<string | null, ConnectionProfile[]>();
  for (const c of connections) {
    const key = parentOf(c.folderId);
    connsByFolder.set(key, [...(connsByFolder.get(key) ?? []), c]);
  }

  // visited защищает от циклов в повреждённых данных: такие папки просто не попадут в дерево.
  const visited = new Set<string>();
  const level = (parentId: string | null): TreeNode[] => {
    const subfolders = [...(foldersByParent.get(parentId) ?? [])].sort(byOrderThenName);
    const conns = [...(connsByFolder.get(parentId) ?? [])].sort(byOrderThenName);
    const nodes: TreeNode[] = [];
    for (const folder of subfolders) {
      if (visited.has(folder.id)) continue;
      visited.add(folder.id);
      nodes.push({ kind: "folder", folder, children: level(folder.id) });
    }
    for (const connection of conns) nodes.push({ kind: "connection", connection });
    return nodes;
  };
  return level(null);
}

/**
 * Фильтр по имени без учёта регистра: совпавшая папка остаётся целиком,
 * у совпавшего подключения остаются предки. Пустой запрос возвращает исходный массив.
 */
export function filterTree(nodes: TreeNode[], query: string): TreeNode[] {
  const q = query.trim().toLocaleLowerCase();
  if (!q) return nodes;
  const walk = (list: TreeNode[]): TreeNode[] =>
    list.flatMap((node): TreeNode[] => {
      if (node.kind === "connection") return node.connection.name.toLocaleLowerCase().includes(q) ? [node] : [];
      if (node.folder.name.toLocaleLowerCase().includes(q)) return [node];
      const children = walk(node.children);
      return children.length > 0 ? [{ ...node, children }] : [];
    });
  return walk(nodes);
}

/** Пути папок от корня (для выпадающих списков), отсортированы по пути. */
export function folderPaths(folders: ConnectionFolder[]): { id: string; path: string[] }[] {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const pathOf = (folder: ConnectionFolder): string[] => {
    const path: string[] = [];
    const seen = new Set<string>();
    let cur: ConnectionFolder | undefined = folder;
    while (cur && !seen.has(cur.id)) {
      seen.add(cur.id);
      path.unshift(cur.name);
      cur = cur.parentId ? byId.get(cur.parentId) : undefined;
    }
    return path;
  };
  const comparePaths = (a: string[], b: string[]) => {
    for (let i = 0; i < Math.min(a.length, b.length); i++) {
      const c = byName(a[i], b[i]);
      if (c !== 0) return c;
    }
    return a.length - b.length;
  };
  return folders.map((f) => ({ id: f.id, path: pathOf(f) })).sort((a, b) => comparePaths(a.path, b.path));
}

/** Можно ли перенести папку под targetParentId (null — корень): не в себя, не в потомка, не в несуществующую. */
export function canMoveFolder(folders: ConnectionFolder[], folderId: string, targetParentId: string | null): boolean {
  if (targetParentId === null) return true;
  const byId = new Map(folders.map((f) => [f.id, f]));
  if (!byId.has(targetParentId)) return false;
  const seen = new Set<string>();
  let cur: string | null = targetParentId;
  while (cur !== null && !seen.has(cur)) {
    if (cur === folderId) return false;
    seen.add(cur);
    cur = byId.get(cur)?.parentId ?? null;
  }
  return true;
}
