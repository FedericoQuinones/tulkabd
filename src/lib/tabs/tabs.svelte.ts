import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { ConnectionConfig } from "../ipc";
import { connections } from "../connections/store.svelte";
import { ENVIRONMENTS } from "../connections/env";
import { PaneState } from "../pane/pane.svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { groups, removeGroup, split, type Layout } from "../workspace/layout";
import { snapshot, watchSession } from "../workspace/session";

export type TabSpec =
  | { kind: "query"; connectionId?: string; database?: string; sql?: string; run?: boolean }
  | { kind: "structure"; connectionId: string; database: string; schema: string; name: string }
  | { kind: "definition"; connectionId: string; database: string; schema: string; name: string };

export interface Tab {
  id: string;
  /** Definition tabs are query tabs too: their `kind` is "query". */
  kind: "query" | "structure";
  title: string;
  /** Query tabs own a pane (editor text, results and a server session). */
  pane?: PaneState;
  /** Structure tabs read these; query tabs use the pane's live connection and database. */
  connectionId?: string;
  database?: string;
  schema?: string;
  name?: string;
  /** Why the last connect of this tab failed; shown in the tab's own toolbar. */
  connectError: string | null;
  /** Query tabs: the text differs from the saved query or file it was opened from. */
  dirty: boolean;
  /** Query tabs opened from (or saved as) a saved query. */
  savedQuery?: { database: string; name: string };
  /** Query tabs opened from (or saved to) a `.sql` file, with the format to write it back in. */
  file?: { path: string; encoding: "utf8" | "utf8Bom" | "utf16Le" | "utf16Be"; lineEnding: "lf" | "crlf" };
}

export interface PasswordRequest {
  tabId: string;
  conn: ConnectionConfig;
  database?: string;
}

/** A tab group of the workspace: its tabs in strip order and the one it shows. */
export interface TabGroup {
  id: string;
  tabIds: string[];
  activeId: string | null;
}

const newGroup = (): TabGroup => ({ id: `group-${crypto.randomUUID()}`, tabIds: [], activeId: null });

interface Pending {
  sql?: string;
  run: boolean;
  /** Editor line of the first line of `sql` (0 when it is the whole document). */
  lineOffset?: number;
}

/** Wait after the last change before the session is saved. */
export const AUTOSAVE_MS = 1000;
/** Longest a change waits to be saved while changes keep coming. */
export const AUTOSAVE_MAX_WAIT_MS = 5000;
/** Longest a save may hold up the next one, or the window close. */
export const SAVE_TIMEOUT_MS = 2000;

/** Resolves when `p` settles or after `ms`, whichever comes first; never rejects. */
function settleWithin(p: Promise<unknown>, ms: number): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    p.catch(() => {}).finally(() => {
      clearTimeout(t);
      resolve();
    });
  });
}

/** True when a command rejected because the Explorer has no session for the connection. */
export function isNotConnected(e: unknown): boolean {
  if (typeof e === "string") return e === "not_connected";
  if (e && typeof e === "object" && "value" in e) return (e as { value?: unknown }).value === "not_connected";
  return false;
}

/** Environment color of the connection a tab is bound to; the neutral control color when unbound. */
export function envColor(tab: Tab | null): string {
  const id = tabs.binding(tab).connectionId;
  const conn = connections.list.find((c) => c.id === id);
  return ENVIRONMENTS.find((e) => e.id === conn?.environment)?.color ?? "var(--control)";
}

export class TabsStore {
  /** Every open tab, in opening order; `tabGroups` holds which group shows each one. */
  list = $state<Tab[]>([]);
  tabGroups = $state<TabGroup[]>([newGroup()]);
  /** Replaced, never mutated: the layout functions return new trees. */
  layout = $state.raw<Layout>({ kind: "group", id: this.tabGroups[0].id });
  activeGroupId = $state(this.tabGroups[0].id);
  /** Tabs waiting for a password, oldest first; App shows the prompt for the first. */
  passwordRequests = $state<PasswordRequest[]>([]);

  private counter = 0;
  private pending = new Map<string, Pending>();

  get passwordRequest(): PasswordRequest | null {
    return this.passwordRequests[0] ?? null;
  }

  private request(req: PasswordRequest) {
    this.passwordRequests = [...this.passwordRequests.filter((r) => r.tabId !== req.tabId), req];
  }

  private dropRequest(tabId: string) {
    this.passwordRequests = this.passwordRequests.filter((r) => r.tabId !== tabId);
  }

  private group(groupId: string): TabGroup | undefined {
    return this.tabGroups.find((g) => g.id === groupId);
  }

  /** The group showing the tab; "" when the tab is not open. */
  groupOf(tabId: string): string {
    return this.tabGroups.find((g) => g.tabIds.includes(tabId))?.id ?? "";
  }

  tabsIn(groupId: string): Tab[] {
    const ids = this.group(groupId)?.tabIds ?? [];
    return ids.map((id) => this.list.find((x) => x.id === id)).filter((x): x is Tab => !!x);
  }

  activeIn(groupId: string): Tab | null {
    const id = this.group(groupId)?.activeId;
    return this.list.find((x) => x.id === id) ?? null;
  }

  /** The active tab of the active group. */
  get activeId(): string | null {
    return this.group(this.activeGroupId)?.activeId ?? null;
  }

  get active(): Tab | null {
    return this.activeIn(this.activeGroupId);
  }

  get activePane(): PaneState | null {
    return this.active?.pane ?? null;
  }

  /** Connection and database a tab is bound to right now. */
  binding(tab: Tab | null): { connectionId: string | null; database: string | null } {
    if (!tab) return { connectionId: null, database: null };
    if (tab.pane) return { connectionId: tab.pane.connectionId, database: tab.pane.database };
    return { connectionId: tab.connectionId ?? null, database: tab.database ?? null };
  }

  /** Adds the tab to the active group and shows it there. */
  private add(tab: Tab) {
    this.list.push(tab);
    const g = this.group(this.activeGroupId)!;
    g.tabIds.push(tab.id);
    g.activeId = tab.id;
  }

  open(spec: TabSpec): string {
    const id = `tab-${crypto.randomUUID()}`;
    if (spec.kind === "structure") {
      this.add({
        id,
        kind: "structure",
        title: t("tabs.structureTitle", { name: spec.name }),
        connectionId: spec.connectionId,
        database: spec.database,
        schema: spec.schema,
        name: spec.name,
        connectError: null,
        dirty: false,
      });
      return id;
    }
    const pane = new PaneState(id);
    const title = spec.kind === "definition" ? spec.name : `untitled-${++this.counter}`;
    if (spec.kind === "query" && spec.sql) pane.initialDoc = spec.sql;
    const tab: Tab = { id, kind: "query", title, pane, connectError: null, dirty: false };
    if (spec.kind === "definition") {
      tab.connectionId = spec.connectionId;
      tab.database = spec.database;
      tab.schema = spec.schema;
      tab.name = spec.name;
    }
    this.add(tab);
    this.pending.set(id, { sql: spec.kind === "query" ? spec.sql : undefined, run: spec.kind === "query" && !!spec.run });
    if (spec.kind === "definition") void this.retry(id);
    if (spec.connectionId) {
      const conn = connections.list.find((c) => c.id === spec.connectionId);
      if (conn) void this.connectTab(id, conn, undefined, spec.database);
    }
    return id;
  }

  /** Ctrl+T: a new query tab bound to the active tab's connection and database. */
  newTab(): string | null {
    const { connectionId, database } = this.binding(this.active);
    return this.open({ kind: "query", connectionId: connectionId ?? undefined, database: database ?? undefined });
  }

  /** Shows the tab in its group and makes that group the active one. */
  activate(id: string) {
    const g = this.group(this.groupOf(id));
    if (!g) return;
    g.activeId = id;
    this.activeGroupId = g.id;
  }

  focusGroup(groupId: string) {
    if (this.group(groupId)) this.activeGroupId = groupId;
  }

  /**
   * Splits the active group: a new group beside it (`row`) or below it (`column`) becomes the active
   * one, with a new query tab bound to the active tab's connection and database. False at MAX_GROUPS.
   * A group with no tabs is not split (it would stay empty beside the new one): the tab opens in it.
   */
  splitActive(dir: "row" | "column"): boolean {
    if (this.group(this.activeGroupId)?.tabIds.length === 0) {
      this.open({ kind: "query" });
      return true;
    }
    const g = newGroup();
    const next = split(this.layout, this.activeGroupId, dir, g.id);
    if (!next) return false;
    const { connectionId, database } = this.binding(this.active);
    this.tabGroups.push(g);
    this.layout = next;
    this.activeGroupId = g.id;
    this.open({ kind: "query", connectionId: connectionId ?? undefined, database: database ?? undefined });
    return true;
  }

  /** Moves a tab to `index` (its position once moved) in a group, and shows it there. */
  moveTab(tabId: string, groupId: string, index: number) {
    const from = this.group(this.groupOf(tabId));
    const to = this.group(groupId);
    if (!from || !to) return;
    this.detach(from, tabId);
    to.tabIds.splice(Math.max(0, Math.min(index, to.tabIds.length)), 0, tabId);
    to.activeId = tabId;
    this.activeGroupId = to.id;
    if (from.id !== to.id) this.dropIfEmpty(from);
  }

  /** Takes a tab out of its group; the group then shows the tab on its right, or on its left if last. */
  private detach(g: TabGroup, tabId: string) {
    const i = g.tabIds.indexOf(tabId);
    if (i < 0) return;
    g.tabIds.splice(i, 1);
    if (g.activeId === tabId) g.activeId = g.tabIds[i] ?? g.tabIds[i - 1] ?? null;
  }

  /** An empty group goes away unless it is the only one; its neighbour takes over if it was active. */
  private dropIfEmpty(g: TabGroup) {
    if (g.tabIds.length > 0 || this.tabGroups.length <= 1) return;
    const order = groups(this.layout);
    const i = order.indexOf(g.id);
    this.layout = removeGroup(this.layout, g.id);
    this.tabGroups = this.tabGroups.filter((x) => x.id !== g.id);
    if (this.activeGroupId === g.id) this.activeGroupId = order[i + 1] ?? order[i - 1];
  }

  close(id: string) {
    const i = this.list.findIndex((x) => x.id === id);
    if (i < 0) return;
    const g = this.group(this.groupOf(id));
    const [tab] = this.list.splice(i, 1);
    this.pending.delete(id);
    this.dropRequest(id);
    if (g) {
      this.detach(g, id);
      this.dropIfEmpty(g);
    }
    void tab.pane?.close();
  }

  /** Opens (or reopens) the tab's session. The password prompt is raised through `passwordRequest`. */
  async connectTab(id: string, conn: ConnectionConfig, password?: string, database?: string): Promise<void> {
    const tab = this.list.find((x) => x.id === id);
    const pane = tab?.pane;
    if (!pane || pane.status === "connecting" || pane.status === "running") return;
    tab.connectError = null;
    if (!conn.passwordSaved && password === undefined) {
      this.request({ tabId: id, conn, database });
      return;
    }
    const result = await pane.open(conn, password, database);
    if (!this.list.some((x) => x.id === id)) {
      // Closed while connecting: do not leave the session behind.
      await pane.close();
      return;
    }
    if (result.kind === "passwordRequired") {
      this.request({ tabId: id, conn, database });
    } else if (result.kind === "error") {
      tab.connectError = t("explorer.connectFailed", { error: result.error });
    } else if (result.kind === "ok") {
      const p = this.pending.get(id);
      this.pending.delete(id);
      if (p?.run && p.sql) void pane.run(p.sql, p.lineOffset ?? 0);
    }
  }

  async submitPassword(password: string): Promise<void> {
    const req = this.passwordRequest;
    if (req) this.dropRequest(req.tabId);
    if (req) await this.connectTab(req.tabId, req.conn, password, req.database);
  }

  cancelPassword() {
    // A later reconnect must not fire the run that was waiting for this password.
    const req = this.passwordRequest;
    if (!req) return;
    this.pending.delete(req.tabId);
    this.dropRequest(req.tabId);
  }

  /** A connection was deleted: its tabs keep their text but lose the session and the binding. */
  unbindConnection(connectionId: string): void {
    for (const tab of this.list) {
      if (tab.pane?.connectionId !== connectionId) continue;
      this.dropRequest(tab.id);
      this.pending.delete(tab.id);
      tab.connectError = null;
      tab.pane.unbind();
      tab.pane.notice = t("tabs.connectionDeleted");
    }
  }

  /** Binds a connection picked in the Explorer: reuses the active tab when it is an unbound query tab. */
  connect(conn: ConnectionConfig): void {
    const tab = this.active;
    if (tab?.pane && !tab.pane.session && tab.pane.status === "idle") void this.connectTab(tab.id, conn);
    else this.open({ kind: "query", connectionId: conn.id });
  }

  reconnectActive(): void {
    if (this.activeId) this.reconnect(this.activeId);
  }

  /**
   * Runs sql in a query tab. A tab bound to a connection but without a session (restored at
   * startup) connects first, asking for the password if needed, and runs once connected.
   */
  run(id: string, sql: string, lineOffset: number): void {
    const pane = this.list.find((x) => x.id === id)?.pane;
    if (!pane || pane.status !== "idle") return;
    if (pane.session) {
      void pane.run(sql, lineOffset);
      return;
    }
    const conn = connections.list.find((c) => c.id === pane.connectionId);
    if (!conn) return;
    this.pending.set(id, { sql, run: true, lineOffset });
    void this.connectTab(id, conn, undefined, pane.database ?? undefined);
  }

  /** Reopens a query tab's session on the connection and database it was bound to. */
  reconnect(id: string): void {
    const tab = this.list.find((x) => x.id === id);
    const pane = tab?.pane;
    if (!tab || !pane) return;
    const conn = connections.list.find((c) => c.id === pane.connectionId);
    if (conn) void this.connectTab(tab.id, conn, undefined, pane.database ?? undefined);
  }

  /** Loads (or reloads) a definition tab's text; needs the Explorer session. */
  async retry(id: string): Promise<void> {
    const tab = this.list.find((x) => x.id === id);
    if (!tab?.pane || !tab.connectionId || tab.name === undefined) return;
    const pane = tab.pane;
    try {
      const text = await ipc.objectDefinition(tab.connectionId, tab.database!, tab.schema!, tab.name);
      pane.notice = null;
      pane.loadText(text);
    } catch (e) {
      pane.notice = isNotConnected(e) ? t("tabs.definitionNotConnected") : t("tabs.loadFailed", { error: String(e) });
    }
  }

  /**
   * Replaces the whole workspace with a restored one (see `restore` in workspace/session). Nothing
   * connects: a bound tab opens its session on its first Run. Untitled numbering continues after
   * the highest restored `untitled-N`.
   */
  replaceWorkspace(w: { list: Tab[]; tabGroups: TabGroup[]; layout: Layout; activeGroupId: string }): void {
    const old = this.list.map((x) => x.pane);
    this.list = w.list;
    this.tabGroups = w.tabGroups;
    this.layout = w.layout;
    this.activeGroupId = w.activeGroupId;
    this.passwordRequests = [];
    this.pending.clear();
    this.counter = Math.max(0, ...w.list.map((x) => Number(/^untitled-(\d+)$/.exec(x.title)?.[1] ?? 0)));
    for (const p of old) void p?.close();
  }

  /**
   * Saves the session AUTOSAVE_MS after the last change (tabs, groups, layout, bindings or editor
   * text), and right away when the window is asked to close. Returns a function that stops it.
   */
  startAutosave(save: (json: string) => Promise<void>): () => void {
    let timer: ReturnType<typeof setTimeout> | undefined;
    // Started by the first unsaved change, so continuous typing still saves.
    let maxTimer: ReturnType<typeof setTimeout> | undefined;
    let stopped = false;
    let unlisten: (() => void) | null = null;
    // Never rejects and never hangs: the window close waits for this promise.
    const flush = (): Promise<void> => {
      clearTimeout(timer);
      clearTimeout(maxTimer);
      timer = maxTimer = undefined;
      let saving: Promise<unknown>;
      try {
        saving = save(JSON.stringify(snapshot(this)));
      } catch {
        return Promise.resolve();
      }
      return settleWithin(saving, SAVE_TIMEOUT_MS);
    };
    let first = true;
    const stopEffect = $effect.root(() => {
      $effect(() => {
        watchSession(this);
        if (first) {
          first = false;
          return;
        }
        clearTimeout(timer);
        timer = setTimeout(() => void flush(), AUTOSAVE_MS);
        maxTimer ??= setTimeout(() => void flush(), AUTOSAVE_MAX_WAIT_MS);
      });
    });
    getCurrentWindow()
      .onCloseRequested(async () => {
        if (!stopped) await flush();
      })
      .then((fn) => {
        if (stopped) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      stopped = true;
      clearTimeout(timer);
      clearTimeout(maxTimer);
      stopEffect();
      unlisten?.();
    };
  }

  /** Closes every tab (used by tests). */
  async reset(): Promise<void> {
    const panes = this.list.map((x) => x.pane);
    const g = newGroup();
    this.list = [];
    this.tabGroups = [g];
    this.layout = { kind: "group", id: g.id };
    this.activeGroupId = g.id;
    this.passwordRequests = [];
    this.pending.clear();
    this.counter = 0;
    await Promise.all(panes.map((p) => p?.close()));
  }
}

export const tabs = new TabsStore();
