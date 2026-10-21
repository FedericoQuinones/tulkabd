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
 * State of the single query tab (`untitled-1`) and the session it owns.
 * Task 10 consumes `results`, `messages` and `summary`. Message `line` values are
 * already 1-based editor lines (backend line + lineOffset); see `PaneMessage`.
 */
class PaneState {
  readonly tabId = "untitled-1";
  connectionId = $state<string | null>(null);
  database = $state<string | null>(null);
  status = $state<PaneStatus>("idle");
  results = $state<PaneResult[]>([]);
  messages = $state<PaneMessage[]>([]);
  summary = $state<RunSummary | null>(null);
  session = $state<SessionInfo | null>(null);
  /** Editor cursor, shown in the status bar. */
  cursor = $state({ line: 1, col: 1 });

  private hasSession = false;

  /**
   * Binds the tab to a connection: closes the previous session (if any) and opens a new one.
   * `database` is only set when reconnecting to the same database.
   */
  async open(conn: ConnectionConfig, password?: string, database?: string): Promise<OpenResult> {
    if (this.status === "connecting" || this.status === "running") return { kind: "busy" };
    const before = this.status;
    this.status = "connecting";
    try {
      if (this.hasSession) {
        this.hasSession = false;
        await ipc.closeSession(this.tabId).catch(() => {});
      }
      const info = await ipc.openSession(this.tabId, conn.id, database, password);
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
      if (before === "disconnected" && this.connectionId === conn.id) {
        this.status = "disconnected";
      } else {
        this.session = null;
        this.connectionId = null;
        this.database = null;
        this.status = "idle";
      }
      return String(e) === "password_required" ? { kind: "passwordRequired" } : { kind: "error", error: String(e) };
    }
  }

  private markDisconnected() {
    this.status = "disconnected";
    pushDisconnectedMessage(this);
  }

  async run(sql: string, lineOffset: number): Promise<void> {
    if (this.status !== "idle" || !this.session) return;
    this.status = "running";
    this.results = [];
    this.messages = [];
    this.summary = null;
    // `body` is a view over this pane so the pure reducer can fill it.
    const body: PaneBody = {
      get results() {
        return pane.results;
      },
      get messages() {
        return pane.messages;
      },
      get disconnected() {
        return pane.status === "disconnected";
      },
      set disconnected(v: boolean) {
        if (v) pane.status = "disconnected";
      },
    };
    try {
      this.summary = await ipc.runQuery(this.tabId, sql, (e) => reduceEvent(body, e, lineOffset));
      if ((this.status as PaneStatus) === "disconnected") return;
      if (this.summary.outcome === "disconnected") this.markDisconnected();
      else {
        if (this.summary.outcome === "cancelled") this.messages.push({ kind: "info", text: t("pane.cancelled"), line: null });
        this.status = "idle";
      }
    } catch (e) {
      const err = classify(e);
      if (err.kind === "disconnected") {
        this.markDisconnected();
      } else if (err.kind === "cancelled") {
        this.messages.push({ kind: "info", text: t("pane.cancelled"), line: null });
        this.status = "idle";
      } else {
        this.messages.push({ kind: "error", text: err.text, line: err.line === null ? null : err.line + lineOffset });
        this.status = "idle";
      }
    }
  }

  /** Only effective while a query runs: earlier cancels would be dropped by the driver. */
  async stop(): Promise<void> {
    if (this.status !== "running") return;
    await ipc.cancelQuery(this.tabId).catch(() => {});
  }

  async changeDatabase(database: string): Promise<void> {
    if (this.status !== "idle" || !this.session || database === this.database) return;
    try {
      await ipc.useDatabase(this.tabId, database);
      this.database = database;
    } catch (e) {
      const err = classify(e);
      if (err.kind === "disconnected") this.markDisconnected();
      else this.messages.push({ kind: "error", text: t("pane.useDatabaseFailed", { error: err.text }), line: null });
    }
  }
}

export const pane = new PaneState();
