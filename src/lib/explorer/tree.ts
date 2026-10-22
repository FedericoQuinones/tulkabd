// Pure parts of the Explorer tree: names, the folder hierarchy, row types,
// keyboard navigation and context menus. The stateful store is tree.svelte.ts.
import { t } from "../i18n/t";
import type { CachedDatabase, ColumnInfo, ConnectionConfig, DbObject, Folder, ObjectKind } from "../ipc";
import { tabs } from "../tabs/tabs.svelte";

/** `[schema].[name]` with `]` doubled; mirrors `qualified` in tulka-core. */
export function quoteName(schema: string, name: string): string {
  const q = (s: string) => `[${s.replaceAll("]", "]]")}]`;
  return `${q(schema)}.${q(name)}`;
}

export const selectTop10 = (schema: string, name: string) => `SELECT TOP 10 * FROM ${quoteName(schema, name)};`;

export const OBJECT_KINDS: ObjectKind[] = ["table", "view", "procedure"];

/**
 * Row keys. Folders and connections use their own id (that is what `set_expanded`
 * stores); deeper nodes use a JSON tuple, which never collides with an id.
 */
export const keyOf = {
  db: (c: string, d: string) => JSON.stringify(["d", c, d]),
  group: (c: string, d: string, k: ObjectKind) => JSON.stringify(["g", c, d, k]),
  object: (c: string, d: string, o: DbObject) => JSON.stringify(["o", c, d, o.kind, o.schema, o.name]),
  column: (objectKey: string, name: string) => JSON.stringify(["col", objectKey, name]),
  notice: (parent: string, tag: string) => JSON.stringify(["n", parent, tag]),
};

export interface FolderNode {
  kind: "folder";
  id: string;
  name: string;
  folder: Folder;
  expanded: boolean;
  children: TreeNode[];
}
export interface ConnectionNode {
  kind: "connection";
  id: string;
  name: string;
  connection: ConnectionConfig;
  expanded: boolean;
}
export type TreeNode = FolderNode | ConnectionNode;

const byName = (a: { name: string }, b: { name: string }) =>
  a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });

/**
 * Folders (nested, sorted by name) with their connections after their subfolders;
 * connections without a folder go last at the root. A folder whose parent is
 * unknown, or that sits in a parent loop, is shown at the root.
 */
export function buildTree(
  folders: Folder[],
  conns: ConnectionConfig[],
  expanded: { has(id: string): boolean },
): TreeNode[] {
  const byId = new Map(folders.map((f) => [f.id, f]));
  const parentOf = (f: Folder) => (f.parentId && byId.has(f.parentId) ? f.parentId : null);
  const effective = new Map<string, string | null>();
  for (const f of folders) {
    let parent = parentOf(f);
    let cur = parent;
    for (let steps = 0; cur !== null; steps++) {
      if (cur === f.id || steps > folders.length) {
        parent = null;
        break;
      }
      cur = parentOf(byId.get(cur)!);
    }
    effective.set(f.id, parent);
  }
  const kids = new Map<string | null, { folders: Folder[]; conns: ConnectionConfig[] }>();
  const slot = (id: string | null) => {
    let s = kids.get(id);
    if (!s) kids.set(id, (s = { folders: [], conns: [] }));
    return s;
  };
  for (const f of folders) slot(effective.get(f.id) ?? null).folders.push(f);
  for (const c of conns) slot(c.folderId && byId.has(c.folderId) ? c.folderId : null).conns.push(c);

  const build = (id: string | null): TreeNode[] => {
    const s = kids.get(id);
    if (!s) return [];
    return [
      ...[...s.folders].sort(byName).map(
        (f): FolderNode => ({
          kind: "folder",
          id: f.id,
          name: f.name,
          folder: f,
          expanded: expanded.has(f.id),
          children: build(f.id),
        }),
      ),
      ...[...s.conns].sort(byName).map(
        (c): ConnectionNode => ({ kind: "connection", id: c.id, name: c.name, connection: c, expanded: expanded.has(c.id) }),
      ),
    ];
  };
  return build(null);
}

/** False when `target` is the folder itself or one of its descendants. */
export function canMoveFolder(folders: Folder[], id: string, target: string | null): boolean {
  const byId = new Map(folders.map((f) => [f.id, f]));
  let cur = target;
  for (let steps = 0; cur !== null && steps <= folders.length; steps++) {
    if (cur === id) return false;
    cur = byId.get(cur)?.parentId ?? null;
  }
  return true;
}

export type ConnStatus = "offline" | "connecting" | "connected" | "error";


interface RowBase {
  key: string;
  /** 1-based, as `aria-level`. */
  level: number;
  parent: string | null;
  expandable: boolean;
  expanded: boolean;
}
export interface NoticeAction {
  label: string;
  run: () => unknown;
}
export type Row =
  | (RowBase & { kind: "folder"; folder: Folder })
  | (RowBase & { kind: "connection"; conn: ConnectionConfig; status: ConnStatus })
  | (RowBase & { kind: "database"; connectionId: string; db: CachedDatabase; indexing: boolean })
  | (RowBase & { kind: "group"; connectionId: string; database: string; objectKind: ObjectKind; count: number | null })
  | (RowBase & { kind: "object"; connectionId: string; database: string; object: DbObject })
  | (RowBase & { kind: "column"; column: ColumnInfo })
  | (RowBase & { kind: "notice"; tone: "error" | "muted"; text: string; action: NoticeAction | null });

export type NavResult = { focus: string } | { expand: string } | { collapse: string } | { activate: string };

/** Maps a key press on the focused row to what the tree should do. */
export function navigate(rows: Row[], focusKey: string | null, key: string): NavResult | null {
  if (rows.length === 0) return null;
  const i = rows.findIndex((r) => r.key === focusKey);
  if (i < 0) return ["ArrowDown", "ArrowUp", "Home", "End"].includes(key) ? { focus: rows[0].key } : null;
  const r = rows[i];
  switch (key) {
    case "ArrowDown":
      return { focus: rows[Math.min(i + 1, rows.length - 1)].key };
    case "ArrowUp":
      return { focus: rows[Math.max(i - 1, 0)].key };
    case "Home":
      return { focus: rows[0].key };
    case "End":
      return { focus: rows[rows.length - 1].key };
    case "ArrowRight":
      if (r.expandable && !r.expanded) return { expand: r.key };
      if (r.expanded && rows[i + 1] && rows[i + 1].level > r.level) return { focus: rows[i + 1].key };
      return null;
    case "ArrowLeft":
      if (r.expandable && r.expanded) return { collapse: r.key };
      if (r.parent !== null && rows.some((x) => x.key === r.parent)) return { focus: r.parent };
      return null;
    case "Enter":
      if (r.kind === "notice" && r.action) return { activate: r.key };
      if (r.expandable) return r.expanded ? { collapse: r.key } : { expand: r.key };
      return null;
  }
  return null;
}

export interface MenuItem {
  id: string;
  label: string;
  run: () => unknown;
  danger?: boolean;
  /** Draws a rule above the item. */
  separator?: boolean;
  /** Shown muted on the right, e.g. the name Copy name copies. */
  hint?: string;
}

/** Explorer store actions the menus call; the store passes itself. */
export interface MenuStore {
  connect(id: string): unknown;
  disconnect(id: string): unknown;
  refresh(id: string): unknown;
  refreshDatabase(id: string, database: string): unknown;
  copyName(object: DbObject): unknown;
}

/** Menu actions that need a dialog or an inline editor, provided by the tree component. */
export interface MenuActions {
  rename(row: Row): void;
  edit(conn: ConnectionConfig): void;
  moveTo(row: Row): void;
  remove(row: Row): void;
  newSubfolder(folder: Folder): void;
}

/**
 * Context menu of a row, in display order. To add an item, push it in the
 * branch of its row kind; `ui` carries the actions that open dialogs.
 */
export function buildMenu(row: Row, ui: MenuActions, store: MenuStore): MenuItem[] {
  switch (row.kind) {
    case "folder":
      return [
        { id: "rename", label: t("explorer.menu.rename"), run: () => ui.rename(row) },
        { id: "newSubfolder", label: t("explorer.menu.newSubfolder"), run: () => ui.newSubfolder(row.folder) },
        { id: "delete", label: t("explorer.menu.delete"), run: () => ui.remove(row), danger: true, separator: true },
      ];
    case "connection": {
      const id = row.conn.id;
      const live = row.status === "connected" || row.status === "connecting";
      return [
        live
          ? { id: "disconnect", label: t("explorer.menu.disconnect"), run: () => store.disconnect(id) }
          : { id: "connect", label: t("explorer.menu.connect"), run: () => store.connect(id) },
        { id: "refresh", label: t("explorer.menu.refresh"), run: () => store.refresh(id) },
        { id: "newTab", label: t("explorer.menu.newTab"), run: () => tabs.open({ kind: "query", connectionId: id }) },
        { id: "rename", label: t("explorer.menu.rename"), run: () => ui.rename(row), separator: true },
        { id: "edit", label: t("explorer.menu.edit"), run: () => ui.edit(row.conn) },
        { id: "moveTo", label: t("explorer.menu.moveTo"), run: () => ui.moveTo(row) },
        { id: "delete", label: t("explorer.menu.delete"), run: () => ui.remove(row), danger: true, separator: true },
      ];
    }
    case "database": {
      const { connectionId } = row;
      const database = row.db.name;
      return [
        {
          id: "newTabHere",
          label: t("explorer.menu.newTabHere"),
          run: () => tabs.open({ kind: "query", connectionId, database }),
        },
        { id: "refresh", label: t("explorer.menu.refresh"), run: () => store.refreshDatabase(connectionId, database) },
      ];
    }
    case "object": {
      const { connectionId, database, object } = row;
      const { schema, name, kind } = object;
      const target = { connectionId, database, schema, name };
      const items: MenuItem[] = [];
      if (kind !== "procedure") {
        items.push(
          {
            id: "selectTop",
            label: t("explorer.menu.selectTop"),
            run: () => tabs.open({ kind: "query", connectionId, database, sql: selectTop10(schema, name), run: true }),
          },
          { id: "structure", label: t("explorer.menu.structure"), run: () => tabs.open({ kind: "structure", ...target }) },
        );
      }
      if (kind !== "table") {
        items.push({ id: "definition", label: t("explorer.menu.definition"), run: () => tabs.open({ kind: "definition", ...target }) });
      }
      items.push({
        id: "copyName",
        label: t("explorer.menu.copyName"),
        run: () => store.copyName(object),
        hint: quoteName(schema, name),
        separator: true,
      });
      return items;
    }
    default:
      return [];
  }
}

export type DragItem = { kind: "folder" | "connection"; id: string };

/** Command errors as user text; folder sentinels get their own message. */
export function actionError(e: unknown): string {
  const s = String(e);
  if (s === "cycle") return t("explorer.errors.cycle");
  if (s === "not_found") return t("explorer.errors.notFound");
  if (s === "empty_name") return t("explorer.errors.emptyName");
  return t("explorer.errors.action", { error: s });
}

/** Parses a JSON row key; `null` for folder and connection ids. */
export function parseKey(key: string): string[] | null {
  if (!key.startsWith("[")) return null;
  try {
    return JSON.parse(key) as string[];
  } catch {
    return null;
  }
}
