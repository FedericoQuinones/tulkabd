// Typed wrappers for the Tauri commands. Mirrors the serde shapes in crates/tulka-core.
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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
  | { kind: "noAccess"; value: string }
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
  /** The containing folder; `null` is the root. */
  folderId: string | null;
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
  /** True on Azure SQL Database, where a connection is bound to one database. */
  singleDatabase: boolean;
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

export type ObjectKind = "table" | "view" | "procedure";
export interface DbObject {
  schema: string;
  name: string;
  kind: ObjectKind;
}
export interface ColumnInfo {
  name: string;
  /** Already formatted for display: `nvarchar(50)`, `decimal(18,2)`. */
  typeName: string;
  nullable: boolean;
  primaryKey: boolean;
  default: string | null;
}
export interface ForeignKey {
  name: string;
  columns: string[];
  refSchema: string;
  refTable: string;
  refColumns: string[];
}
export interface IndexInfo {
  name: string;
  columns: string[];
  unique: boolean;
  primaryKey: boolean;
  clustered: boolean;
}
export interface TableStructure {
  columns: ColumnInfo[];
  foreignKeys: ForeignKey[];
  indexes: IndexInfo[];
}
export interface CachedDatabase {
  name: string;
  noAccess: boolean;
  /** Unix milliseconds, `null` when never indexed. */
  indexedAt: number | null;
  tables: number;
  views: number;
  procedures: number;
}
export interface ExplorerInfo {
  server: ServerInfo;
  databases: string[];
}
export interface PaletteHit {
  connectionId: string;
  connectionName: string;
  /** Folder names from the root down; `[]` at the root. */
  folderPath: string[];
  connected: boolean;
  database: string;
  object: DbObject;
  /** Matched positions in `"schema.name"` as Unicode code points, not UTF-16 offsets: index `Array.from(text)`. */
  indices: number[];
}
export interface SearchResponse {
  hits: PaletteHit[];
  totalMatches: number;
  databasesMatched: number;
  /** Databases still queued or being indexed (at least 1 while the startup index loads). */
  indexing: number;
}
export interface CompletionMetadata {
  schemas: string[];
  objects: DbObject[];
  /** Columns of tables and views, keyed by the unquoted `schema.name`. */
  columns: Record<string, string[]>;
}
export interface Folder {
  id: string;
  name: string;
  /** The containing folder; `null` is the root. */
  parentId: string | null;
}
/** What a pasted connection string provides; `null` where it did not say. */
export interface ParsedConnection {
  /** Host, with `\instance` when one is named. */
  server: string;
  port: number | null;
  user: string | null;
  password: string | null;
  database: string | null;
  encrypt: boolean | null;
  trustServerCertificate: boolean | null;
  connectTimeoutS: number | null;
  /** Keys that were not understood, as written. */
  ignoredKeys: string[];
}
export type ExplorerStatus = "connecting" | "connected" | "disconnected" | "error";
/** Pushed on the `app-event` event, outside of any command. No `indexIdle` follows an `error` or `disconnected` status. */
export type AppEvent =
  | { kind: "explorerStatus"; connectionId: string; status: ExplorerStatus; message: string | null }
  | { kind: "indexProgress"; connectionId: string; database: string; done: boolean; noAccess: boolean }
  | { kind: "indexIdle"; connectionId: string };

/**
 * Command errors arrive as plain strings. Sentinels: `"password_required"`, `"disconnected"`,
 * `"busy"` (the tab's session is in use), `"closed"` (tab closed, or explorer disconnected or reconnected, while connecting),
 * `"not_connected"` (no explorer session), `"not_found"`, `"cycle"` and `"empty_name"` (folders).
 */
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

/**
 * Rows in display form: text over 1000 characters arrives as its first 1000 plus "…",
 * binary over 32 bytes as its first 33 bytes. Copying uses the full stored values.
 */
export const fetchRows = (tabId: string, resultIndex: number, offset: number, limit: number) =>
  invoke<Value[][]>("fetch_rows", { tabId, resultIndex, offset, limit });

export const copyResults = (
  tabId: string,
  resultIndex: number,
  mode: "excel" | "ai",
  selection: Selection,
) => invoke<CopyOutcome>("copy_results", { tabId, resultIndex, mode, selection });

export const closeSession = (tabId: string) => invoke<void>("close_session", { tabId });

/** Why the connections file could not be read at startup (the app is then read-only for connections), if it could not. */
export const startupError = () => invoke<string | null>("startup_error");

export const listFolders = () => invoke<Folder[]>("list_folders");
export const createFolder = (name: string, parentId: string | null) =>
  invoke<Folder>("create_folder", { name, parentId });
export const renameFolder = (id: string, name: string) => invoke<void>("rename_folder", { id, name });
export const deleteFolder = (id: string) => invoke<void>("delete_folder", { id });
export const moveConnection = (connectionId: string, folderId: string | null) =>
  invoke<void>("move_connection", { connectionId, folderId });
export const moveFolder = (id: string, parentId: string | null) =>
  invoke<void>("move_folder", { id, parentId });
/** Ids of the tree nodes left open (folders and connections). */
export const listExpanded = () => invoke<string[]>("list_expanded");
/** Replaces the set of expanded folder and connection ids. */
export const setExpanded = (ids: string[]) => invoke<void>("set_expanded", { ids });
/**
 * Parses an ADO.NET, ODBC, JDBC or `sqlserver://` connection string. Errors are codes that never
 * contain the text: `"empty"`, `"no_server"`, `"bad_port"`, `"bad_value:<key>"`, `"unsupported_auth"`,
 * `"unrecognized"`.
 */
export const parseConnectionString = (text: string) =>
  invoke<ParsedConnection>("parse_connection_string", { text });
/** Saves a copy named "<name> (copy)" in the same folder, with its own copy of the saved password. */
export const duplicateConnection = (id: string) => invoke<ConnectionConfig>("duplicate_connection", { id });
/**
 * Copies the connection's connection string to the clipboard (from Rust). Resolves to whether the
 * password was included: only with `includePassword` and a saved password.
 */
export const copyConnectionString = (id: string, includePassword: boolean) =>
  invoke<boolean>("copy_connection_string", { id, includePassword });
export const listServerDatabases = (conn: ConnectionConfig, password?: string) =>
  invoke<string[]>("list_server_databases", { conn, password: password ?? null });

export const connectExplorer = (connectionId: string, password?: string) =>
  invoke<ExplorerInfo>("connect_explorer", { connectionId, password: password ?? null });
export const disconnectExplorer = (connectionId: string) =>
  invoke<void>("disconnect_explorer", { connectionId });
export const refreshMetadata = (connectionId: string, database?: string) =>
  invoke<void>("refresh_metadata", { connectionId, database: database ?? null });
export const explorerDatabases = (connectionId: string) =>
  invoke<CachedDatabase[]>("explorer_databases", { connectionId });
export const explorerObjects = (connectionId: string, database: string, kind: ObjectKind) =>
  invoke<DbObject[]>("explorer_objects", { connectionId, database, kind });
export const explorerColumns = (connectionId: string, database: string, schema: string, name: string) =>
  invoke<ColumnInfo[]>("explorer_columns", { connectionId, database, schema, name });
export const tableStructure = (connectionId: string, database: string, schema: string, name: string) =>
  invoke<TableStructure>("table_structure", { connectionId, database, schema, name });
export const objectDefinition = (connectionId: string, database: string, schema: string, name: string) =>
  invoke<string>("object_definition", { connectionId, database, schema, name });
export const searchObjects = (query: string, limit: number) =>
  invoke<SearchResponse>("search_objects", { query, limit });
export const completionMetadata = (connectionId: string, database: string) =>
  invoke<CompletionMetadata>("completion_metadata", { connectionId, database });

/** Subscribes to the explorer and indexing events. Call the returned function to stop. */
export const onAppEvent = (cb: (e: AppEvent) => void): Promise<UnlistenFn> =>
  listen<AppEvent>("app-event", (ev) => cb(ev.payload));
