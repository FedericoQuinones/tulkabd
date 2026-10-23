// The workspace session saved between runs: groups, tabs, bindings and unsaved editor text.
// Never passwords, never results. The backend stores the JSON as is (see `saveSession`).
import { t } from "../i18n/t";
import { PaneState } from "../pane/pane.svelte";
import type { Tab, TabsStore } from "../tabs/tabs.svelte";
import { groups, MAX_GROUPS, type Layout } from "./layout";

type Encoding = "utf8" | "utf8Bom" | "utf16Le" | "utf16Be";
type LineEnding = "lf" | "crlf";

export interface SessionV1 {
  version: 1;
  layout: Layout;
  activeGroupId: string;
  groups: Record<string, { tabIds: string[]; activeId: string | null }>;
  tabs: SessionTab[];
}

export type SessionTab =
  | {
      id: string;
      kind: "query";
      title: string;
      connectionId: string | null;
      database: string | null;
      text: string;
      savedQuery?: { database: string; name: string };
      file?: { path: string; encoding: Encoding; lineEnding: LineEnding };
      dirty: boolean;
    }
  | { id: string; kind: "structure"; connectionId: string; database: string; schema: string; name: string }
  | { id: string; kind: "definition"; connectionId: string; database: string; schema: string; name: string };

function sessionTab(store: TabsStore, tab: Tab, text: (pane: PaneState) => string): SessionTab {
  const ref = { connectionId: tab.connectionId!, database: tab.database!, schema: tab.schema!, name: tab.name! };
  if (tab.kind === "structure") return { id: tab.id, kind: "structure", ...ref };
  // Definition tabs reload their text when connected, so it is not saved.
  if (tab.name !== undefined && tab.connectionId !== undefined) return { id: tab.id, kind: "definition", ...ref };
  const { connectionId, database } = store.binding(tab);
  const out: SessionTab = { id: tab.id, kind: "query", title: tab.title, connectionId, database, text: text(tab.pane!), dirty: tab.dirty };
  if (tab.savedQuery) out.savedQuery = { database: tab.savedQuery.database, name: tab.savedQuery.name };
  if (tab.file) out.file = { path: tab.file.path, encoding: tab.file.encoding, lineEnding: tab.file.lineEnding };
  return out;
}

function build(store: TabsStore, text: (pane: PaneState) => string): SessionV1 {
  const out: SessionV1["groups"] = {};
  for (const g of store.tabGroups) out[g.id] = { tabIds: [...g.tabIds], activeId: g.activeId };
  return {
    version: 1,
    layout: store.layout,
    activeGroupId: store.activeGroupId,
    groups: out,
    tabs: store.list.map((tab) => sessionTab(store, tab, text)),
  };
}

export function snapshot(store: TabsStore): SessionV1 {
  return build(store, (pane) => pane.text());
}

/**
 * Reads every reactive value the snapshot depends on, without building the editor text (which is
 * not reactive: the editor bumps `edits` instead). Called from an effect to know when to save.
 */
export function watchSession(store: TabsStore): void {
  build(store, (pane) => {
    void pane.edits;
    void pane.docVersion;
    return "";
  });
}

type Json = Record<string, unknown>;
const isObject = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v);
const isString = (v: unknown): v is string => typeof v === "string";
const isStringOrNull = (v: unknown) => v === null || isString(v);

/** Group ids of a well-formed layout, or null. */
function layoutGroups(l: unknown): string[] | null {
  if (!isObject(l)) return null;
  if (l.kind === "group") return isString(l.id) ? [l.id] : null;
  if (l.kind !== "split" || (l.dir !== "row" && l.dir !== "column")) return null;
  const { children, sizes } = l;
  if (!Array.isArray(children) || !Array.isArray(sizes) || children.length < 2 || sizes.length !== children.length) return null;
  if (sizes.some((s) => typeof s !== "number" || !Number.isFinite(s) || !(s > 0))) return null;
  const ids: string[] = [];
  for (const c of children) {
    const sub = layoutGroups(c);
    if (!sub) return null;
    ids.push(...sub);
  }
  return ids;
}

function validTab(v: unknown): v is SessionTab {
  if (!isObject(v) || !isString(v.id)) return false;
  if (v.kind === "structure" || v.kind === "definition") {
    return isString(v.connectionId) && isString(v.database) && isString(v.schema) && isString(v.name);
  }
  if (v.kind !== "query") return false;
  if (!isString(v.title) || !isStringOrNull(v.connectionId) || !isStringOrNull(v.database)) return false;
  if (!isString(v.text) || typeof v.dirty !== "boolean") return false;
  const q = v.savedQuery;
  if (q !== undefined && !(isObject(q) && isString(q.database) && isString(q.name))) return false;
  const f = v.file;
  if (f === undefined) return true;
  return (
    isObject(f) &&
    isString(f.path) &&
    ["utf8", "utf8Bom", "utf16Le", "utf16Be"].includes(f.encoding as string) &&
    (f.lineEnding === "lf" || f.lineEnding === "crlf")
  );
}

/** The saved session, or null (start empty) when there is none or it is not a valid version 1. */
export function parseSession(json: string | null): SessionV1 | null {
  if (json === null) return null;
  let v: unknown;
  try {
    v = JSON.parse(json);
  } catch {
    return null;
  }
  if (!isObject(v) || v.version !== 1 || !isObject(v.groups) || !Array.isArray(v.tabs)) return null;
  const ids = layoutGroups(v.layout);
  if (!ids || ids.length > MAX_GROUPS || new Set(ids).size !== ids.length) return null;
  if (!isString(v.activeGroupId) || !ids.includes(v.activeGroupId)) return null;
  if (Object.keys(v.groups).length !== ids.length) return null;
  if (!v.tabs.every(validTab)) return null;
  const tabIds = new Set(v.tabs.map((x) => x.id));
  if (tabIds.size !== v.tabs.length) return null;
  // Every tab sits in exactly one group, and every group shows one of its own tabs.
  const placed = new Set<string>();
  for (const id of ids) {
    const g = v.groups[id];
    if (!isObject(g) || !Array.isArray(g.tabIds) || !isStringOrNull(g.activeId)) return null;
    for (const t of g.tabIds) {
      if (!isString(t) || !tabIds.has(t) || placed.has(t)) return null;
      placed.add(t);
    }
    if (g.activeId === null ? g.tabIds.length > 0 : !g.tabIds.includes(g.activeId)) return null;
  }
  if (placed.size !== tabIds.size) return null;
  return v as unknown as SessionV1;
}

function restoredTab(s: SessionTab, known: { connectionIds: Set<string> }): Tab {
  if (s.kind === "structure") {
    const { id, connectionId, database, schema, name } = s;
    return { id, kind: "structure", title: t("tabs.structureTitle", { name }), connectionId, database, schema, name, connectError: null, dirty: false };
  }
  const pane = new PaneState(s.id);
  const bind = (connectionId: string | null, database: string | null) => {
    if (connectionId === null) return;
    if (!known.connectionIds.has(connectionId)) {
      pane.notice = t("tabs.restoredWithoutConnection");
      return;
    }
    pane.connectionId = connectionId;
    pane.database = database;
  };
  if (s.kind === "definition") {
    const { id, connectionId, database, schema, name } = s;
    bind(connectionId, database);
    return { id, kind: "query", title: name, pane, connectionId, database, schema, name, connectError: null, dirty: false };
  }
  pane.initialDoc = s.text;
  bind(s.connectionId, s.database);
  const tab: Tab = { id: s.id, kind: "query", title: s.title, pane, connectError: null, dirty: s.dirty };
  if (s.savedQuery) tab.savedQuery = s.savedQuery;
  if (s.file) tab.file = s.file;
  return tab;
}

/**
 * Replaces the store's workspace with a saved session. Connects nothing: a bound query tab opens
 * its session on its first Run. A tab whose connection is not in `known` keeps its text, unbound,
 * with a notice. Definition tabs try to load their text (it needs the Explorer connected).
 */
export function restore(store: TabsStore, s: SessionV1, known: { connectionIds: Set<string> }): void {
  const list = s.tabs.map((x) => restoredTab(x, known));
  const tabGroups = groups(s.layout).map((id) => ({ id, tabIds: [...s.groups[id].tabIds], activeId: s.groups[id].activeId }));
  store.replaceWorkspace({ list, tabGroups, layout: s.layout, activeGroupId: s.activeGroupId });
  for (const x of s.tabs) {
    if (x.kind === "definition" && known.connectionIds.has(x.connectionId)) void store.retry(x.id);
  }
}
