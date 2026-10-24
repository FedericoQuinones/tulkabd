import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { ConnectionConfig, FileEncoding, LineEnding } from "../ipc";
import { connections } from "../connections/store.svelte";
import { queries, type NameRequest } from "../queries/queries.svelte";
import { ENVIRONMENTS } from "../connections/env";
import { PaneState } from "../pane/pane.svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { groups, removeGroup, split, type Layout } from "../workspace/layout";
import { snapshot, watchSession } from "../workspace/session";

export type TabSpec =
  | {
      kind: "query";
      connectionId?: string;
      database?: string;
      sql?: string;
      run?: boolean;
      /** The saved query `sql` was read from: the tab takes its name. */
      savedQuery?: { database: string; name: string };
      /** The `.sql` file `sql` was read from: the tab takes its file name. */
      file?: TabFile & { name: string };
    }
  | { kind: "structure"; connectionId: string; database: string; schema: string; name: string }
  | { kind: "definition"; connectionId: string; database: string; schema: string; name: string };

/** A `.sql` file from disk and the format to write it back in. */
export interface TabFile {
  path: string;
  encoding: FileEncoding;
  lineEnding: LineEnding;
}

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
  /** Query tabs: edited since opened, or since last saved (as a saved query or file). */
  dirty: boolean;
  /** Query tabs opened from (or saved as) a saved query. */
  savedQuery?: { database: string; name: string };
  /** Query tabs opened from (or saved to) a `.sql` file, with the format to write it back in. */
  file?: TabFile;
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

/** A file command error as user text. */
export function fileErrorText(e: unknown): string {
  const s = String(e);
  if (s === "not_text") return t("files.errors.notText");
  if (s === "not_found") return t("files.errors.notFound");
  if (s === "not_allowed") return t("files.errors.notAllowed");
  return s;
}

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

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
  /** The name dialog App shows (Ctrl+S on a tab not saved yet, Save as saved query). */
  nameRequest = $state.raw<NameRequest | null>(null);
  /** Why the last files opened could not all be opened; App shows it until dismissed or the next open. */
  fileError = $state<string | null>(null);

  private counter = 0;
  private pending = new Map<string, Pending>();
  private saveNotices = new WeakMap<PaneState, string>();
  /** Binding a saved query tab was opened for, while its pane has none yet (see `tabOf`). */
  private wanted = new Map<string, { connectionId: string; database: string }>();

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

  /** An edit by the user in a query tab marks it unsaved. */
  private track(tab: Tab) {
    const id = tab.id;
    if (tab.pane) {
      tab.pane.onedit = () => {
        const x = this.list.find((y) => y.id === id);
        if (x && !x.dirty) x.dirty = true;
      };
    }
  }

  /** Adds the tab to the active group and shows it there. */
  private add(tab: Tab) {
    this.track(tab);
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
    const saved = spec.kind === "query" ? spec.savedQuery : undefined;
    const file = spec.kind === "query" ? spec.file : undefined;
    const title =
      spec.kind === "definition" ? spec.name : saved ? saved.name : file ? file.name : `untitled-${++this.counter}`;
    if (spec.kind === "query" && spec.sql) pane.initialDoc = spec.sql;
    const tab: Tab = { id, kind: "query", title, pane, connectError: null, dirty: false };
    if (saved) tab.savedQuery = { database: saved.database, name: saved.name };
    if (file) tab.file = { path: file.path, encoding: file.encoding, lineEnding: file.lineEnding };
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
    this.wanted.delete(id);
    this.dropRequest(id);
    if (tab.pane) tab.pane.onedit = null;
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
      // Bound to a database now: "pick a database" no longer holds.
      if (pane.notice === t("queries.pickDatabase")) this.clearSaveNotice(pane);
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
      this.wanted.delete(tab.id);
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
      // Saved as a query meanwhile: it is no longer a definition to reload.
      if (tab.name === undefined) return;
      pane.notice = null;
      pane.loadText(text);
    } catch (e) {
      if (tab.name === undefined) return;
      pane.notice = isNotConnected(e) ? t("tabs.definitionNotConnected") : t("tabs.loadFailed", { error: String(e) });
    }
  }

  /**
   * Ctrl+S on the active query tab. A saved query is overwritten in place. A tab not saved yet asks
   * for a name (through `nameRequest`) and is saved under its database; without one it says so.
   */
  async saveActive(): Promise<void> {
    const tab = this.active;
    const pane = tab?.pane;
    if (!tab || !pane) return;
    if (tab.savedQuery) {
      const { database, name } = tab.savedQuery;
      const edits = pane.edits;
      try {
        await queries.save(database, name, pane.text(), true);
      } catch (e) {
        this.saveNotice(pane, t("queries.saveFailed", { error: String(e) }));
        return;
      }
      // Typing while the save was in flight leaves the tab unsaved.
      if (pane.edits === edits) tab.dirty = false;
      this.clearSaveNotice(pane);
      return;
    }
    if (tab.file) {
      // Written back in place, in its own encoding and line endings; never asks for a name.
      const { path, encoding, lineEnding } = tab.file;
      const edits = pane.edits;
      const write = () => ipc.writeSqlFile(path, pane.text(), encoding, lineEnding);
      try {
        try {
          await write();
        } catch (e) {
          if (String(e) !== "not_allowed") throw e;
          // A restored tab whose path the backend could not take at startup: allow it again.
          await ipc.allowDroppedPaths([path]);
          await write();
        }
      } catch (e) {
        this.saveNotice(pane, t("files.saveFailed", { error: fileErrorText(e) }));
        return;
      }
      if (pane.edits === edits) tab.dirty = false;
      this.clearSaveNotice(pane);
      return;
    }
    this.askName(tab, tab.name ?? "");
  }

  /**
   * "Save as saved query" in a tab's menu: asks for a name and saves the text as a query of the
   * tab's database. A file tab stays on its file (the saved query is a copy, and the file's unsaved
   * state does not change); any other tab becomes that saved query.
   */
  saveAsQuery(id: string): void {
    const tab = this.list.find((x) => x.id === id);
    if (!tab?.pane) return;
    const initial = tab.file ? fileName(tab.file.path).replace(/\.sql$/i, "") : (tab.savedQuery?.name ?? tab.name ?? "");
    this.askName(tab, initial, !!tab.file);
  }

  /**
   * Asks for a name (through `nameRequest`) and saves the tab's text under its database; without a
   * database it says so. With `copy` the tab is left as it is; otherwise it becomes the saved query.
   */
  private askName(tab: Tab, initial: string, copy = false): void {
    const pane = tab.pane!;
    // A definition still loading (or that could not load) has nothing to save yet.
    if (tab.name !== undefined && pane.text() === "") return;
    const database = this.binding(tab).database;
    if (!database) {
      this.saveNotice(pane, t("queries.pickDatabase"));
      return;
    }
    const id = tab.id;
    const request: NameRequest = {
      title: t("queries.saveTitle", { database }),
      initial,
      onsubmit: async (name, overwrite) => {
        const x = this.list.find((y) => y.id === id);
        if (!x?.pane) {
          this.nameRequest = null;
          return null;
        }
        const edits = x.pane.edits;
        try {
          await queries.save(database, name, x.pane.text(), overwrite);
        } catch (e) {
          return String(e);
        }
        if (this.nameRequest === request) this.nameRequest = null;
        if (copy) {
          // "Pick a database" no longer holds.
          this.clearSaveNotice(x.pane);
          return null;
        }
        this.markSaved(x, { database, name });
        if (x.pane.edits === edits) x.dirty = false;
        return null;
      },
      oncancel: () => {
        if (this.nameRequest === request) this.nameRequest = null;
      },
    };
    this.nameRequest = request;
  }

  /**
   * Opens `.sql` files the backend may read (picked, dropped or restored) in new query tabs bound to
   * the active tab's connection and database, titled with the file name; a file already open shows
   * its tab instead. A file that cannot be read opens nothing and is reported in `fileError`.
   */
  async openFiles(paths: string[]): Promise<void> {
    const { connectionId, database } = this.binding(this.active);
    const unique = [...new Set(paths)];
    const read = await Promise.all(
      unique.map((path) =>
        this.tabOfFile(path)
          ? null
          : ipc.readSqlFile(path).then(
              (file) => ({ file }),
              (error: unknown) => ({ error }),
            ),
      ),
    );
    const errors: string[] = [];
    unique.forEach((path, i) => {
      const open = this.tabOfFile(path);
      const r = read[i];
      if (open) this.activate(open.id);
      else if (!r) return;
      else if ("error" in r) errors.push(t("files.openFailed", { name: fileName(path), error: fileErrorText(r.error) }));
      else {
        const { file } = r;
        this.open({
          kind: "query",
          connectionId: connectionId ?? undefined,
          database: connectionId ? (database ?? undefined) : undefined,
          sql: file.text,
          file: { path: file.path, name: file.name, encoding: file.encoding, lineEnding: file.lineEnding },
        });
      }
    });
    this.fileError = errors.length ? errors.join("\n") : null;
  }

  /** Ctrl+O and "Open file…": the native open dialog, then `openFiles` with the picked files. */
  async pickFiles(): Promise<void> {
    let paths: string[];
    try {
      paths = await ipc.pickSqlFiles(t("files.filter"));
    } catch (e) {
      this.fileError = t("files.pickFailed", { error: String(e) });
      return;
    }
    if (paths.length) await this.openFiles(paths);
  }

  /** Paths dropped on the window: the `.sql` ones are allowed in the backend, then opened. */
  async dropFiles(paths: string[]): Promise<void> {
    const sql = paths.filter((p) => /\.sql$/i.test(p));
    if (!sql.length) return;
    try {
      await ipc.allowDroppedPaths(sql);
    } catch {
      // Reading them says why.
    }
    await this.openFiles(sql);
  }

  private tabOfFile(path: string): Tab | undefined {
    return this.list.find((x) => x.file?.path === path);
  }

  /** The tab now holds a saved query: it takes its name, and a definition tab stops being one. */
  private markSaved(tab: Tab, q: { database: string; name: string }) {
    tab.savedQuery = { database: q.database, name: q.name };
    tab.title = q.name;
    if (tab.name !== undefined) {
      tab.connectionId = tab.database = tab.schema = tab.name = undefined;
    }
    if (tab.pane) this.clearSaveNotice(tab.pane);
  }

  /** Why the tab is not saved, shown above the editor until it is saved. */
  private saveNotice(pane: PaneState, text: string) {
    pane.notice = text;
    this.saveNotices.set(pane, text);
  }

  /** Clears the pane's notice only when it is still the one `saveNotice` put there. */
  private clearSaveNotice(pane: PaneState) {
    if (pane.notice !== null && pane.notice === this.saveNotices.get(pane)) pane.notice = null;
    this.saveNotices.delete(pane);
  }

  /**
   * The open tab of a saved query. With `connectionId` (a tree click), only one bound to that
   * connection and the query's database, or still connecting to them; any one otherwise.
   */
  private tabOf(q: { database: string; name: string }, connectionId: string | null = null): Tab | undefined {
    return this.list.find((x) => {
      if (x.savedQuery?.database !== q.database || x.savedQuery.name !== q.name) return false;
      if (connectionId === null) return true;
      const bound = this.binding(x);
      const b = bound.connectionId !== null ? bound : (this.wanted.get(x.id) ?? bound);
      return b.connectionId === connectionId && b.database === q.database;
    });
  }

  /**
   * Opens a saved query in a new tab bound to `connectionId` and the query's database (unbound when
   * null); when it is already open, shows that tab instead. Rejects when the file cannot be read.
   */
  async openSavedQuery(q: { database: string; name: string }, connectionId: string | null): Promise<string> {
    let open = this.tabOf(q, connectionId);
    if (!open) {
      const text = await ipc.readSavedQuery(q.database, q.name);
      // Opened by another click while reading.
      open = this.tabOf(q, connectionId);
      if (!open) {
        const id = this.open({
          kind: "query",
          connectionId: connectionId ?? undefined,
          database: connectionId ? q.database : undefined,
          sql: text,
          savedQuery: { database: q.database, name: q.name },
        });
        // Its session opens in the background; until then the binding it asked for counts.
        if (connectionId) this.wanted.set(id, { connectionId, database: q.database });
        return id;
      }
    }
    this.activate(open.id);
    return open.id;
  }

  /** Renames a saved query and the tabs showing it. Rejects with the backend's error code. */
  async renameSavedQuery(database: string, name: string, newName: string): Promise<void> {
    await queries.rename(database, name, newName);
    for (const tab of this.list) {
      if (tab.savedQuery?.database === database && tab.savedQuery.name === name) {
        tab.savedQuery = { database, name: newName };
        tab.title = newName;
      }
    }
  }

  /** Deletes a saved query. A tab showing it keeps its text, now unsaved, and says Ctrl+S writes it again. */
  async deleteSavedQuery(database: string, name: string): Promise<void> {
    await queries.remove(database, name);
    const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
    const notice = t("queries.deleted", { key: t(isMac ? "queries.saveKey.mac" : "queries.saveKey.other") });
    for (const tab of this.list) {
      if (tab.savedQuery?.database !== database || tab.savedQuery.name !== name) continue;
      tab.dirty = true;
      if (tab.pane) this.saveNotice(tab.pane, notice);
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
    this.nameRequest = null;
    this.pending.clear();
    this.wanted.clear();
    for (const tab of w.list) this.track(tab);
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
    const write = (json: string): Promise<void> => {
      let saving: Promise<unknown>;
      try {
        saving = save(json);
      } catch {
        return Promise.resolve();
      }
      return settleWithin(saving, SAVE_TIMEOUT_MS);
    };
    // Saves run one after another, so an older one never lands after a newer one.
    let queue: Promise<void> | null = null;
    // Never rejects and never hangs: the window close waits for this promise.
    const flush = (): Promise<void> => {
      clearTimeout(timer);
      clearTimeout(maxTimer);
      timer = maxTimer = undefined;
      let json: string;
      try {
        json = JSON.stringify(snapshot(this));
      } catch {
        return queue ?? Promise.resolve();
      }
      const next: Promise<void> = queue ? queue.then(() => write(json)) : write(json);
      queue = next;
      void next.then(() => {
        if (queue === next) queue = null;
      });
      return next;
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
    this.nameRequest = null;
    this.pending.clear();
    this.wanted.clear();
    this.counter = 0;
    await Promise.all(panes.map((p) => p?.close()));
  }
}

export const tabs = new TabsStore();
