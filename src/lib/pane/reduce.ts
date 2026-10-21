import { t } from "../i18n/t";
import type { Column, MessageKind, UiEvent } from "../ipc";

export interface PaneResult {
  columns: Column[];
  total: number;
  truncated: boolean;
}

export interface PaneMessage {
  kind: MessageKind;
  text: string;
  /**
   * 1-based line in the editor document (backend line + the run's lineOffset),
   * or null. Task 10 can jump straight to it without further adjustment.
   */
  line: number | null;
}

/** The part of the pane state that query events build up. */
export interface PaneBody {
  results: PaneResult[];
  messages: PaneMessage[];
  /** Set when a `disconnected` event arrives; the store turns it into the pane status. */
  disconnected: boolean;
}

/** Adds the "connection lost" message unless it is already the last one. */
export function pushDisconnectedMessage(body: Pick<PaneBody, "messages">): void {
  const text = t("pane.connectionLost");
  if (body.messages.at(-1)?.text !== text) body.messages.push({ kind: "error", text, line: null });
}

/** Applies one backend event to the body (mutates it, so it works on `$state` proxies too). */
export function reduceEvent(body: PaneBody, e: UiEvent, lineOffset: number): void {
  switch (e.kind) {
    case "resultStart":
      body.results[e.index] = { columns: e.columns, total: 0, truncated: false };
      break;
    case "rowCount": {
      const r = body.results[e.index];
      if (r) {
        r.total = e.total;
        r.truncated = e.truncated;
      }
      break;
    }
    case "message":
      body.messages.push({
        kind: e.messageKind,
        text: e.text,
        line: e.line === null ? null : e.line + lineOffset,
      });
      break;
    case "rowsAffected":
      body.messages.push({ kind: "info", text: t("pane.rowsAffected", { count: e.count }), line: null });
      break;
    case "batchDone":
      break;
    case "disconnected":
      body.disconnected = true;
      pushDisconnectedMessage(body);
      break;
  }
}

/** "Microsoft SQL Server 2022 (RTM-CU13) ..." becomes "2022"; anything unrecognized falls back to its first line. */
export function shortVersion(version: string): string {
  const m = /SQL Server\s+(\d{4})/i.exec(version);
  return m ? m[1] : (version.split("\n")[0] ?? version).trim();
}
