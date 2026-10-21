// Typed wrappers for the Tauri commands. Mirrors the serde shapes in crates/tulka-core.
import { Channel, invoke } from "@tauri-apps/api/core";

export type Value =
  | { kind: "null" }
  | { kind: "bool"; value: boolean }
  | { kind: "int"; value: string }
  | { kind: "float"; value: number }
  | { kind: "decimal"; value: string }
  | { kind: "text"; value: string }
  | { kind: "bytes"; value: number[] }
  | { kind: "date"; value: string }
  | { kind: "time"; value: string }
  | { kind: "dateTime"; value: string }
  | { kind: "dateTimeOffset"; value: string }
  | { kind: "uuid"; value: string };

export type DriverError =
  | { kind: "unreachable"; value: string }
  | { kind: "authFailed"; value: string }
  | { kind: "tls"; value: string }
  | { kind: "query"; value: { message: string; line: number | null } }
  | { kind: "cancelled" }
  | { kind: "disconnected"; value: string }
  | { kind: "other"; value: string };

export type SecretError = { kind: string; value?: string };

export type Environment = "production" | "staging" | "test" | "local";
export type GuardLevel = "off" | "confirm" | "block";
export type DatabaseScope = { kind: "all" } | { kind: "selected"; names: string[] };

export interface ConnectionConfig {
  id: string;
  name: string;
  driver: string;
  server: string;
  port: number;
  user: string;
  databases: DatabaseScope;
  environment: Environment;
  color: string;
  guard: GuardLevel;
  encrypt: boolean;
  trustServerCertificate: boolean;
  connectTimeoutS: number;
  passwordSaved: boolean;
}

export interface Column {
  name: string;
  typeName: string;
}
export type MessageKind = "info" | "warning" | "error";

/** Events forwarded by `run_query`; `rows` is replaced by `rowCount` (rows stay in the Rust buffer). */
export type UiEvent =
  | { kind: "resultStart"; index: number; columns: Column[] }
  | { kind: "rowCount"; index: number; total: number; truncated: boolean }
  | { kind: "message"; messageKind: MessageKind; text: string; line: number | null }
  | { kind: "rowsAffected"; count: number }
  | { kind: "batchDone"; batch: number }
  | { kind: "disconnected"; reason: string };

export interface ServerInfo {
  version: string;
  user: string;
}
export interface SaveConnectionResult {
  connection: ConnectionConfig;
  keyringUnavailable: boolean;
}
export interface SessionInfo {
  server: ServerInfo;
  databases: string[];
  currentDatabase: string;
}
export interface RunSummary {
  elapsedMs: number;
  finishedAt: string;
  outcome: "ok" | "error" | "cancelled" | "disconnected";
}
export type Selection =
  | { kind: "all" }
  | { kind: "rows"; from: number; to: number }
  | { kind: "cells"; rows: [number, number]; cols: [number, number] };
export interface CopyOutcome {
  rows: number;
  tokens: number;
}

/** Command errors arrive as plain strings; `"password_required"`, `"disconnected"`, `"busy"` (the tab's session is in use) and `"closed"` (tab closed while `open_session` connected) are sentinels. */
/** Why the connections file could not be read at startup (the app is then read-only for connections), if it could not. */
export const startupError = () => invoke<string | null>("startup_error");

export const listConnections = () => invoke<ConnectionConfig[]>("list_connections");

export const saveConnection = (conn: ConnectionConfig, password?: string) =>
  invoke<SaveConnectionResult>("save_connection", { conn, password: password ?? null });

export const deleteConnection = (id: string) => invoke<void>("delete_connection", { id });

export const testConnection = (conn: ConnectionConfig, password?: string) =>
  invoke<ServerInfo>("test_connection", { conn, password: password ?? null });

export const openSession = (tabId: string, connectionId: string, database?: string, password?: string) =>
  invoke<SessionInfo>("open_session", {
    tabId,
    connectionId,
    database: database ?? null,
    password: password ?? null,
  });

export const useDatabase = (tabId: string, database: string) =>
  invoke<void>("use_database", { tabId, database });

export const runQuery = (tabId: string, sql: string, onEvent: (e: UiEvent) => void) => {
  const channel = new Channel<UiEvent>();
  channel.onmessage = onEvent;
  return invoke<RunSummary>("run_query", { tabId, sql, onEvent: channel });
};

export const cancelQuery = (tabId: string) => invoke<void>("cancel_query", { tabId });

export const fetchRows = (tabId: string, resultIndex: number, offset: number, limit: number) =>
  invoke<Value[][]>("fetch_rows", { tabId, resultIndex, offset, limit });

export const copyResults = (
  tabId: string,
  resultIndex: number,
  mode: "excel" | "ai",
  selection: Selection,
) => invoke<CopyOutcome>("copy_results", { tabId, resultIndex, mode, selection });

export const closeSession = (tabId: string) => invoke<void>("close_session", { tabId });
