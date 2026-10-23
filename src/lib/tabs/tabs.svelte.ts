import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { ConnectionConfig } from "../ipc";
import { connections } from "../connections/store.svelte";
import { PaneState } from "../pane/pane.svelte";

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
}

export interface PasswordRequest {
  tabId: string;
  conn: ConnectionConfig;
  database?: string;
}

interface Pending {
  sql?: string;
  run: boolean;
}

/** True when a command rejected because the Explorer has no session for the connection. */
export function isNotConnected(e: unknown): boolean {
  if (typeof e === "string") return e === "not_connected";
  if (e && typeof e === "object" && "value" in e) return (e as { value?: unknown }).value === "not_connected";
  return false;
}

class TabsStore {
  list = $state<Tab[]>([]);
  activeId = $state<string | null>(null);
  /** Set while a tab waits for a password; App shows the prompt. */
  passwordRequest = $state<PasswordRequest | null>(null);
  connectError = $state<string | null>(null);

  private counter = 0;
  private pending = new Map<string, Pending>();

  get active(): Tab | null {
    return this.list.find((x) => x.id === this.activeId) ?? null;
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

  open(spec: TabSpec): string {
    const id = `tab-${crypto.randomUUID()}`;
    if (spec.kind === "structure") {
      this.list.push({
        id,
        kind: "structure",
        title: t("tabs.structureTitle", { name: spec.name }),
        connectionId: spec.connectionId,
        database: spec.database,
        schema: spec.schema,
        name: spec.name,
      });
      this.activeId = id;
      return id;
    }
    const pane = new PaneState(id);
    const title = spec.kind === "definition" ? spec.name : `untitled-${++this.counter}`;
    if (spec.kind === "query" && spec.sql) pane.initialDoc = spec.sql;
    const tab: Tab = { id, kind: "query", title, pane };
    if (spec.kind === "definition") {
      tab.connectionId = spec.connectionId;
      tab.database = spec.database;
      tab.schema = spec.schema;
      tab.name = spec.name;
    }
    this.list.push(tab);
    this.activeId = id;
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

  activate(id: string) {
    if (this.list.some((x) => x.id === id)) this.activeId = id;
  }

  close(id: string) {
    const i = this.list.findIndex((x) => x.id === id);
    if (i < 0) return;
    const [tab] = this.list.splice(i, 1);
    this.pending.delete(id);
    if (this.passwordRequest?.tabId === id) this.passwordRequest = null;
    if (this.activeId === id) this.activeId = (this.list[i] ?? this.list[i - 1])?.id ?? null;
    void tab.pane?.close();
  }

  /** Opens (or reopens) the tab's session. The password prompt is raised through `passwordRequest`. */
  async connectTab(id: string, conn: ConnectionConfig, password?: string, database?: string): Promise<void> {
    const tab = this.list.find((x) => x.id === id);
    const pane = tab?.pane;
    if (!pane || pane.status === "connecting" || pane.status === "running") return;
    this.connectError = null;
    if (!conn.passwordSaved && password === undefined) {
      this.passwordRequest = { tabId: id, conn, database };
      return;
    }
    const result = await pane.open(conn, password, database);
    if (!this.list.some((x) => x.id === id)) {
      // Closed while connecting: do not leave the session behind.
      await pane.close();
      return;
    }
    if (result.kind === "passwordRequired") {
      this.passwordRequest = { tabId: id, conn, database };
    } else if (result.kind === "error") {
      this.connectError = t("explorer.connectFailed", { error: result.error });
    } else if (result.kind === "ok") {
      const p = this.pending.get(id);
      this.pending.delete(id);
      if (p?.run && p.sql) void pane.run(p.sql, 0);
    }
  }

  async submitPassword(password: string): Promise<void> {
    const req = this.passwordRequest;
    this.passwordRequest = null;
    if (req) await this.connectTab(req.tabId, req.conn, password, req.database);
  }

  cancelPassword() {
    // A later reconnect must not fire the run that was waiting for this password.
    if (this.passwordRequest) this.pending.delete(this.passwordRequest.tabId);
    this.passwordRequest = null;
  }

  /** Binds a connection picked in the Explorer: reuses the active tab when it is an unbound query tab. */
  connect(conn: ConnectionConfig): void {
    const tab = this.active;
    if (tab?.pane && !tab.pane.session && tab.pane.status === "idle") void this.connectTab(tab.id, conn);
    else this.open({ kind: "query", connectionId: conn.id });
  }

  reconnectActive(): void {
    const tab = this.active;
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

  /** Closes every tab (used by tests). */
  async reset(): Promise<void> {
    const panes = this.list.map((x) => x.pane);
    this.list = [];
    this.activeId = null;
    this.passwordRequest = null;
    this.connectError = null;
    this.pending.clear();
    await Promise.all(panes.map((p) => p?.close()));
  }
}

export const tabs = new TabsStore();
