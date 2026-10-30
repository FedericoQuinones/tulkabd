import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { ConnectionConfig, GuardFinding, GuardLevel, RunSummary, SessionInfo } from "../ipc";
import { connections } from "../connections/store.svelte";
import { prefs } from "../settings/prefs.svelte";
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
  /** Which tab of the panel under the editor is showing. */
  bottomTab = $state<"results" | "messages" | "runlog">("results");
  /** Increments on every run start; the results view uses it to drop its row cache. */
  runCount = $state(0);
  /** The confirmed row limit in force when the last run started; later preference changes do not apply to it. */
  runMaxRows = $state(prefs.confirmed.maxRows);
  session = $state<SessionInfo | null>(null);
  /** Editor cursor, shown in the status bar. */
  cursor = $state({ line: 1, col: 1 });

  /** Text the editor starts with when it has no saved state yet. */
  initialDoc = $state("");
  /** Bumped when `loadText` replaces the document; a mounted editor swaps its content. */
  docVersion = $state(0);
  /** Bumped on every edit by the user (see `edited`); the session autosave watches it. */
  edits = $state(0);
  /** Set by the tabs store: hears every edit by the user, to mark the tab as unsaved. */
  onedit: (() => void) | null = null;
  /** Inline notice shown above the editor (e.g. a definition that could not load). */
  notice = $state<string | null>(null);
  /** CodeMirror state saved when the editor unmounts, so text, cursor and undo survive tab switches. */
  editorState: unknown = null;

  /**
   * A script the connection's guard held for confirmation: the exact text that was sent,
   * where it started in the editor, and the flagged statements (lines relative to `sql`).
   */
  guardPrompt = $state<{ sql: string; lineOffset: number; findings: GuardFinding[] } | null>(null);
  /** The statements that made the guard refuse the last run, with editor-relative 0-based lines. */
  blocked = $state<GuardFinding[] | null>(null);
  /** Shown after "don't ask again" turns the guard off; `undoGuardOff` restores `previous`. */
  guardToast = $state<{ text: string; connectionId: string; previous: GuardLevel } | null>(null);

  /** True while a use_database call is in flight (Run and the database select are disabled). */
  switching = $state(false);

  constructor(tabId: string) {
    this.tabId = tabId;
  }

  /** Set by a mounted editor so `loadText` can see what the user has right now. */
  liveDoc: (() => string) | null = null;
  private loadedText: string | null = null;

  /** The last text `loadText` put in (a definition), or null; differing text means the user edited it. */
  get lastLoaded(): string | null {
    return this.loadedText;
  }

  /** The mounted editor reports an edit by the user (not text put in by `loadText`). */
  edited(): void {
    this.edits++;
    this.onedit?.();
  }

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
    const token = ++this.runId;
    // A confirmation that is still open is discarded with the tab: record it like Cancel does,
    // while the session the log entry is read from still exists.
    const held = this.guardPrompt;
    this.clearGuard();
    const owned = this.hasSession || this.status === "connecting";
    this.hasSession = false;
    if (held) await ipc.logCancelledRun(this.tabId, held.sql).catch(() => {});
    // A newer open began while logging: it replaces this session, and a close sent now would
    // hit the new one.
    if (!owned || token !== this.runId) return;
    await ipc.closeSession(this.tabId).catch(() => {});
  }

  /** Drops the session and the connection binding but keeps the editor text. */
  unbind(): void {
    void this.close();
    this.clearGuard();
    this.session = null;
    this.connectionId = null;
    this.database = null;
    this.status = "idle";
  }

  private clearGuard() {
    this.guardPrompt = null;
    this.blocked = null;
    this.guardToast = null;
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
      this.guardPrompt = null;
      this.blocked = null;
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
    if (this.status === "idle" && this.session) this.guardToast = null;
    await this.send(sql, lineOffset, false);
  }

  /**
   * Sends the script held by the guard once more, now confirmed. It is the text the guard
   * analyzed, not whatever the editor holds now. With `dontAskAgain` the connection's guard is
   * saved as Off first; if that fails nothing runs.
   */
  async confirmGuard(dontAskAgain: boolean, beforeRun?: () => void): Promise<void> {
    const prompt = this.guardPrompt;
    if (!prompt || this.status !== "idle") return;
    // Taken before any await, so a second click on the button finds nothing to confirm.
    this.guardPrompt = null;
    if (dontAskAgain && this.connectionId) {
      const id = this.connectionId;
      const name = connections.list.find((c) => c.id === id)?.name ?? "";
      try {
        const previous = await connections.setGuard(id, "off");
        this.guardToast = { text: t("guard.turnedOff", { connection: name }), connectionId: id, previous };
      } catch (e) {
        this.messages = [{ kind: "error", text: t("guard.changeFailed", { error: String(e) }), line: null }];
        return;
      }
    }
    beforeRun?.();
    await this.send(prompt.sql, prompt.lineOffset, true);
  }

  /** Drops the held script and records that the user backed out. */
  cancelGuard(): void {
    const prompt = this.guardPrompt;
    if (!prompt) return;
    this.guardPrompt = null;
    void ipc.logCancelledRun(this.tabId, prompt.sql).catch(() => {});
  }

  /** Puts back the guard level that "don't ask again" replaced. */
  async undoGuardOff(): Promise<void> {
    const toast = this.guardToast;
    if (!toast) return;
    this.guardToast = null;
    // Another change since (the menu, the connection dialog) wins over this undo.
    if (connections.list.find((c) => c.id === toast.connectionId)?.guard !== "off") return;
    try {
      await connections.setGuard(toast.connectionId, toast.previous);
    } catch (e) {
      this.messages.push({ kind: "error", text: t("guard.changeFailed", { error: String(e) }), line: null });
    }
  }

  private async send(sql: string, lineOffset: number, confirmed: boolean): Promise<void> {
    if (this.status !== "idle" || !this.session) return;
    this.guardPrompt = null;
    this.blocked = null;
    this.status = "running";
    const id = ++this.runId;
    this.runCount++;
    this.runMaxRows = prefs.confirmed.maxRows;
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
      const summary = await ipc.runQuery(this.tabId, sql, confirmed, (e) => {
        if (id === this.runId) reduceEvent(body, e, lineOffset);
      });
      if (id !== this.runId) return;
      this.summary = summary;
      if (this.isDisconnected()) return;
      if (summary.outcome === "disconnected") this.markDisconnected();
      else {
        if (summary.outcome === "needsConfirmation") {
          this.guardPrompt = { sql, lineOffset, findings: summary.findings };
        } else if (summary.outcome === "blocked") {
          this.blocked = summary.findings.map((f) => ({ ...f, line: f.line + lineOffset }));
        }
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
