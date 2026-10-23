import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { ConnectionConfig, RunSummary, SessionInfo } from "../ipc";
import { pushDisconnectedMessage, reduceEvent, type PaneBody, type PaneMessage, type PaneResult } from "./reduce";

export type PaneStatus = "idle" | "connecting" | "running" | "disconnected";

export type OpenResult = { kind: "ok" } | { kind: "passwordRequired" } | { kind: "busy" } | { kind: "error"; error: string };

/** What a failed command rejection means for the pane. */
function classify(e: unknown): { kind: "disconnected" | "cancelled" | "query" | "other"; text: string; line: number | null } {
  if (typeof e === "string") {
    if (e === "disconnected") return { kind: "disconnected", text: e, line: null };
    if (e === "cancelled") return { kind: "cancelled", text: e, line: null };
    return { kind: "other", text: e, line: null };
  }
  if (e && typeof e === "object" && "kind" in e) {
    const err = e as ipc.DriverError;
    switch (err.kind) {
      case "disconnected":
        return { kind: "disconnected", text: err.value, line: null };
      case "cancelled":
        return { kind: "cancelled", text: "cancelled", line: null };
      case "query":
        return { kind: "query", text: err.value.message, line: err.value.line };
      default:
        return { kind: "other", text: err.value, line: null };
    }
  }
  return { kind: "other", text: String(e), line: null };
}

/**
 * State of one query tab and the session it owns (one `PaneState` per tab).
 * Task 10 consumes `results`, `messages` and `summary`. Message `line` values are
 * already 1-based editor lines (backend line + lineOffset); see `PaneMessage`.
 */
export class PaneState {
  readonly tabId: string;
  connectionId = $state<string | null>(null);
  database = $state<string | null>(null);
  status = $state<PaneStatus>("idle");
  results = $state<PaneResult[]>([]);
  messages = $state<PaneMessage[]>([]);
  summary = $state<RunSummary | null>(null);
  /** Increments on every run start; the results view uses it to drop its row cache. */
  runCount = $state(0);
  session = $state<SessionInfo | null>(null);
  /** Editor cursor, shown in the status bar. */
  cursor = $state({ line: 1, col: 1 });

  /** Text the editor starts with when it has no saved state yet. */
  initialDoc = $state("");
  /** Bumped when `loadText` replaces the document; a mounted editor swaps its content. */
  docVersion = $state(0);
  /** Bumped by the mounted editor on every document change; the session autosave watches it. */
  edits = $state(0);
  /** Inline notice shown above the editor (e.g. a definition that could not load). */
  notice = $state<string | null>(null);
  /** CodeMirror state saved when the editor unmounts, so text, cursor and undo survive tab switches. */
  editorState: unknown = null;

  /** True while a use_database call is in flight (Run and the database select are disabled). */
  switching = $state(false);

  constructor(tabId: string) {
    this.tabId = tabId;
  }

  /** Set by a mounted editor so `loadText` can see what the user has right now. */
  liveDoc: (() => string) | null = null;
  private loadedText: string | null = null;

  /** The editor text right now: the mounted editor's, else the state saved on unmount, else `initialDoc`. */
  text(): string {
    if (this.liveDoc) return this.liveDoc();
    const saved = this.editorState as { doc?: string } | null;
    return saved ? (saved.doc ?? "") : this.initialDoc;
  }

  /**
   * Puts loaded text (a definition) into the editor. It only replaces the content while the user
   * has not edited the tab: the document is empty or still the previously loaded text.
   */
  loadText(text: string) {
    const current = this.text();
    if (current !== "" && current !== this.loadedText) return;
    this.loadedText = text;
    this.initialDoc = text;
    this.editorState = null;
    this.docVersion++;
  }

  /**
   * Closes the backend session, if one is open or being opened. Closing while an open is in
   * flight makes the backend reject that open, so it never lands after a newer one.
   */
  async close(): Promise<void> {
    this.runId++;
    if (!this.hasSession && this.status !== "connecting") return;
    this.hasSession = false;
    await ipc.closeSession(this.tabId).catch(() => {});
  }

  /** Drops the session and the connection binding but keeps the editor text. */
  unbind(): void {
    void this.close();
    this.session = null;
    this.connectionId = null;
    this.database = null;
    this.status = "idle";
  }

  private hasSession = false;
  // Run token: every run() takes a new id and open() bumps it, so a run that is still pending
  // (e.g. after a mid-run "disconnected") is stale once a reconnect starts and must not touch state.
  private runId = 0;

  /**
   * Binds the tab to a connection: closes the previous session (if any) and opens a new one.
   * `database` is only set when reconnecting to the same database.
   */
  async open(conn: ConnectionConfig, password?: string, database?: string): Promise<OpenResult> {
    if (this.status === "connecting" || this.status === "running") return { kind: "busy" };
    const before = this.status;
    const token = ++this.runId;
    this.status = "connecting";
    try {
      if (this.hasSession) {
        this.hasSession = false;
        await ipc.closeSession(this.tabId).catch(() => {});
      }
      const info = await ipc.openSession(this.tabId, conn.id, database, password);
      if (token !== this.runId) {
        // Unbound or closed while connecting: close() already told the backend, which rejects it.
        return { kind: "busy" };
      }
      this.hasSession = true;
      this.session = info;
      this.connectionId = conn.id;
      this.database = info.currentDatabase;
      this.results = [];
      this.messages = [];
      this.summary = null;
      this.status = "idle";
      return { kind: "ok" };
    } catch (e) {
      if (token !== this.runId) return { kind: "busy" };
      if (before === "disconnected" && this.connectionId === conn.id) {
        this.status = "disconnected";
      } else {
        this.session = null;
        this.connectionId = null;
        this.database = null;
        this.status = "idle";
      }
      if (before === "disconnected" && String(e) !== "password_required") {
        this.messages.push({ kind: "error", text: t("pane.reconnectFailed", { error: String(e) }), line: null });
      }
      return String(e) === "password_required" ? { kind: "passwordRequired" } : { kind: "error", error: String(e) };
    }
  }

  // A method call, so TS does not narrow `status` across awaits (events can change it mid-run).
  private isDisconnected(): boolean {
    return this.status === "disconnected";
  }

  private markDisconnected() {
    this.status = "disconnected";
    pushDisconnectedMessage(this);
  }

  async run(sql: string, lineOffset: number): Promise<void> {
    if (this.status !== "idle" || !this.session) return;
    this.status = "running";
    const id = ++this.runId;
    this.runCount++;
    this.results = [];
    this.messages = [];
    this.summary = null;
    // `body` is a view over this pane so the pure reducer can fill it.
    const self = this;
    const body: PaneBody = {
      get results() {
        return self.results;
      },
      get messages() {
        return self.messages;
      },
      get disconnected() {
        return self.status === "disconnected";
      },
      set disconnected(v: boolean) {
        if (v) self.status = "disconnected";
      },
    };
    const cancelledLine = () => this.messages.push({ kind: "info", text: t("pane.cancelled"), line: null });
    try {
      const summary = await ipc.runQuery(this.tabId, sql, (e) => {
        if (id === this.runId) reduceEvent(body, e, lineOffset);
      });
      if (id !== this.runId) return;
      this.summary = summary;
      if (this.isDisconnected()) return;
      if (summary.outcome === "disconnected") this.markDisconnected();
      else {
        if (summary.outcome === "cancelled") cancelledLine();
        this.status = "idle";
      }
    } catch (e) {
      if (id !== this.runId) return;
      const err = classify(e);
      if (err.kind === "disconnected") {
        this.markDisconnected();
        return;
      }
      // Never downgrade a status already set to "disconnected" by an event.
      const alive = !this.isDisconnected();
      if (err.kind === "cancelled") {
        if (alive) cancelledLine();
      } else {
        this.messages.push({ kind: "error", text: err.text, line: err.line === null ? null : err.line + lineOffset });
      }
      if (alive) this.status = "idle";
    }
  }

  /** Only effective while a query runs: earlier cancels would be dropped by the driver. */
  async stop(): Promise<void> {
    if (this.status !== "running") return;
    await ipc.cancelQuery(this.tabId).catch(() => {});
  }

  /** Returns when done; on failure the error is in `messages` and `database` is unchanged. */
  async changeDatabase(database: string): Promise<void> {
    if (this.status !== "idle" || this.switching || !this.session || database === this.database) return;
    this.switching = true;
    try {
      await ipc.useDatabase(this.tabId, database);
      this.database = database;
    } catch (e) {
      const err = classify(e);
      if (err.kind === "disconnected") this.markDisconnected();
      else this.messages.push({ kind: "error", text: t("pane.useDatabaseFailed", { error: err.text }), line: null });
    } finally {
      this.switching = false;
    }
  }
}
