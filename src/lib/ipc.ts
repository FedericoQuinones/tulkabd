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
export type GuardKind = "updateWithoutWhere" | "deleteWithoutWhere" | "drop" | "truncate" | "unparsed";
export interface GuardFinding {
  kind: GuardKind;
  /** The target as written, e.g. `dbo.Pedidos`; `null` when unknown. */
  object: string | null;
  /** The statement text, trimmed, at most 400 chars plus an ellipsis. */
  statement: string;
  /** 0-based line of the statement start, relative to the text sent (the selection, if one was run). */
  line: number;
}
export interface RunSummary {
  elapsedMs: number;
  finishedAt: string;
  outcome: "ok" | "error" | "cancelled" | "disconnected" | "blocked" | "needsConfirmation";
  /** The dangerous statements found; empty unless the outcome is `blocked` or `needsConfirmation`. */
  findings: GuardFinding[];
}
export type Selection =
  | { kind: "all" }
  | { kind: "rows"; from: number; to: number }
  | { kind: "cells"; rows: [number, number]; cols: [number, number] };
export interface CopyOutcome {
  rows: number;
  tokens: number;
}
/** A filter on one column of a result; filters combine with AND. */
export type ColumnFilter =
  | { kind: "isNull"; column: number }
  | { kind: "notNull"; column: number }
  /**
   * Exact text (case-sensitive); numbers also match with thousands separators, long text and
   * binary also as the grid shortens them (ending in "…"); NULL never.
   */
  | { kind: "equals"; column: number; text: string };
/** How a received result is shown: filtered, then sorted (stable). All empty = rows as received. */
export interface ViewSpec {
  sort: { column: number; descending: boolean } | null;
  /** Substring of the cell text as the grid shows it, in any column, ignoring case. */
  text: string | null;
  columns: ColumnFilter[];
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
  /** Saved queries whose name matches, best first, at most 20. Empty for a folder-scoped search. */
  queries: SavedQueryHit[];
}
export interface SavedQueryHit {
  database: string;
  name: string;
  /** Matched positions in `name` as Unicode code points, not UTF-16 offsets: index `Array.from(name)`. */
  indices: number[];
}
/** A query saved as `<data dir>/queries/<database>/<name>.sql`. */
export interface SavedQuery {
  /** The database, compared exactly; on a case-insensitive file system names differing only in case share one folder. */
  database: string;
  name: string;
  /** Last change of the file, Unix milliseconds (0 if unknown). */
  modifiedMs: number;
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
  /** Well-known settings tulkabd does not use, by canonical name (`Application Name`, `Pooling`, ...). */
  ignoredKeys: string[];
  /** Other keys not understood; only counted, since their names come from the paste. */
  unknownKeys: number;
}
export type ExplorerStatus = "connecting" | "connected" | "disconnected" | "error";
/** Pushed on the `app-event` event, outside of any command. No `indexIdle` follows an `error` or `disconnected` status. */
export type AppEvent =
  | { kind: "explorerStatus"; connectionId: string; status: ExplorerStatus; message: string | null }
  | { kind: "indexProgress"; connectionId: string; database: string; done: boolean; noAccess: boolean }
  | { kind: "indexIdle"; connectionId: string }
  | { kind: "runLogged"; entry: RunLogEntry };

export type RunLogOutcome = "ok" | "error" | "cancelled" | "blocked" | "disconnected";
/** One statement run, with where it ran when it ran. Text only: never passwords or rows. */
export interface RunLogEntry {
  id: number;
  /** Unix milliseconds, UTC. */
  atMs: number;
  connectionId: string;
  connectionName: string;
  color: string;
  database: string;
  /** At most 100 KB. */
  sql: string;
  sqlTruncated: boolean;
  outcome: RunLogOutcome;
  /** Rows received, else rows affected; null when neither applies. */
  rows: number | null;
  /** Null for a blocked statement and for a cancelled confirmation. */
  elapsedMs: number | null;
}

/**
 * Command errors arrive as plain strings. Sentinels: `"password_required"`, `"disconnected"`,
 * `"busy"` (the tab's session is in use), `"closed"` (tab closed, or explorer disconnected or reconnected, while connecting),
 * `"not_connected"` (no explorer session), `"not_found"` (an unknown folder, connection or database, a saved query or
 * file that does not exist), `"cycle"` (folders), `"empty_name"` (folders and saved queries), `"exists"` (saved
 * queries), and for `.sql` files `"not_allowed"` (a path never allowed in this run), `"not_text"` (also saved
 * queries) and `"too_large"` (over 20 MB).
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

/**
 * Runs a script. The backend applies the connection's guard before anything is sent:
 * a `blocked` or `needsConfirmation` outcome still resolves with a `RunSummary`
 * (its `findings` list the flagged statements, `elapsedMs` is 0) and nothing ran.
 * Send the same script again with `confirmed = true` to go ahead after
 * `needsConfirmation`; `confirmed` never overrides `blocked`.
 */
export const runQuery = (tabId: string, sql: string, confirmed: boolean, onEvent: (e: UiEvent) => void) => {
  const channel = new Channel<UiEvent>();
  channel.onmessage = onEvent;
  return invoke<RunSummary>("run_query", { tabId, sql, confirmed, onEvent: channel });
};

/** Switches a connection's guard off (or back on) until the app closes. */
export const setGuardOffForSession = (connectionId: string, off: boolean) =>
  invoke<void>("set_guard_off_for_session", { connectionId, off });

/**
 * Saves a connection's guard level; resolves with the previous level (for undo).
 * Rejects with `not_found` for an unknown connection, or with the read-only config
 * error when the connections file could not be read at startup.
 */
export const setConnectionGuard = (connectionId: string, level: GuardLevel) =>
  invoke<GuardLevel>("set_connection_guard", { connectionId, level });

/** Ids of the connections whose guard is off until the app closes. */
export const guardOffForSession = () => invoke<string[]>("guard_off_for_session");

/** Run log entries, newest first. `before` is an entry id to page backwards; `limit` is capped at 500. */
export const listRunLog = (before: number | null, limit: number) =>
  invoke<RunLogEntry[]>("list_run_log", { before, limit });

export const clearRunLog = () => invoke<void>("clear_run_log");

/** Records that the user cancelled the confirmation of `sql` on the tab. */
export const logCancelledRun = (tabId: string, sql: string) =>
  invoke<void>("log_cancelled_run", { tabId, sql });

export const cancelQuery = (tabId: string) => invoke<void>("cancel_query", { tabId });

/**
 * Rows in display form: text over 1000 characters arrives as its first 1000 plus "…",
 * binary over 32 bytes as its first 33 bytes. Copying uses the full stored values.
 */
export const fetchRows = (tabId: string, resultIndex: number, offset: number, limit: number) =>
  invoke<Value[][]>("fetch_rows", { tabId, resultIndex, offset, limit });

/**
 * Sorts and filters a received result without rerunning it; returns the visible row count.
 * `fetchRows` and `copyResults` then use visible-row indexes. Fails with "busy" while the tab runs.
 * Calls on one result are not ordered by the backend: the UI must wait for one before sending
 * the next (or drop stale replies), or an older view may be applied last.
 */
export const setResultView = (tabId: string, resultIndex: number, view: ViewSpec) =>
  invoke<number>("set_result_view", { tabId, resultIndex, view });

export const copyResults = (
  tabId: string,
  resultIndex: number,
  mode: "excel" | "ai",
  selection: Selection,
) => invoke<CopyOutcome>("copy_results", { tabId, resultIndex, mode, selection });

export const closeSession = (tabId: string) => invoke<void>("close_session", { tabId });

/** Open transactions on the tab's session; 0 without one. Rejects with "busy" while it runs, "disconnected" when lost. */
export const transactionCount = (tabId: string) => invoke<number>("transaction_count", { tabId });

/** Commits or rolls back every open transaction of the tab's session. */
export const endTransactions = (tabId: string, commit: boolean) => invoke<void>("end_transactions", { tabId, commit });

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
/** How "Copy" writes numbers: with the system's decimal separator or always with a dot. */
export type NumberFormat = "system" | "invariant";
/** What a new connection of an environment starts with. */
export interface EnvironmentStyle {
  /** `#rrggbb`. */
  color: string;
  guard: GuardLevel;
}
/** Preferences kept in `config.json`; the backend brings every value into its range. */
export interface Preferences {
  /** Zoom of the editor and the result grid, in percent (70 to 200). */
  zoom: number;
  /** Editor and grid font size at 100 % zoom, in pixels (10 to 24). */
  fontSize: number;
  /** Whether the tabs of the last session are reopened at startup. */
  restoreSession: boolean;
  /** Rows kept per result set (1,000 to 1,000,000). */
  maxRows: number;
  /** Seconds after which a running query is cancelled; 0 is no limit (max 86,400). */
  queryTimeoutS: number;
  copyNumbers: NumberFormat;
  /** Days the run log keeps its entries (1 to 365). */
  runLogDays: number;
  /** Color and guard level new connections start with. */
  environments: Record<Environment, EnvironmentStyle>;
}
export const getPreferences = () => invoke<Preferences>("get_preferences");
/** Stores all preferences; resolves to what was kept, brought into range. */
export const setPreferences = (prefs: Preferences) => invoke<Preferences>("set_preferences", { prefs });
/** Stores the zoom (percent); resolves to the value kept, brought into 70 to 200. */
export const setZoom = (zoom: number) => invoke<number>("set_zoom", { zoom });
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

/** Saved queries of `database`, or of every database, sorted by database and name. */
export const listSavedQueries = (database?: string) =>
  invoke<SavedQuery[]>("list_saved_queries", { database: database ?? null });
/** With `\n` line breaks. Rejects with `"not_found"` when the query does not exist, `"not_text"` when its file is not text. */
export const readSavedQuery = (database: string, name: string) =>
  invoke<string>("read_saved_query", { database, name });
/** Rejects with `"exists"` when the name is taken and `overwrite` is false, `"empty_name"` on a blank name. */
export const saveQuery = (database: string, name: string, text: string, overwrite: boolean) =>
  invoke<void>("save_query", { database, name, text, overwrite });
/** Rejects with `"not_found"`, `"exists"` (the new name is taken) or `"empty_name"`. */
export const renameSavedQuery = (database: string, name: string, newName: string) =>
  invoke<void>("rename_saved_query", { database, name, newName });
/** Rejects with `"not_found"` when the query does not exist. */
export const deleteSavedQuery = (database: string, name: string) =>
  invoke<void>("delete_saved_query", { database, name });
/** Shows the query's `.sql` file in the system file manager. */
export const revealSavedQuery = (database: string, name: string) =>
  invoke<void>("reveal_saved_query", { database, name });

/** How a `.sql` file from disk is encoded; Ctrl+S writes it back the same way. */
export type FileEncoding = "utf8" | "utf8Bom" | "utf16Le" | "utf16Be";
/** The line ending a `.sql` file is written back with (its more frequent one when read). */
export type LineEnding = "lf" | "crlf";
export interface OpenedFile {
  path: string;
  /** The file name, the tab's title. */
  name: string;
  /** With `\n` line breaks. */
  text: string;
  encoding: FileEncoding;
  lineEnding: LineEnding;
}

/**
 * Shows the native open dialog for `.sql` files (several at once); the backend may then read and
 * write the picked paths, which it returns. `filterName` labels the `*.sql` filter. Empty when cancelled.
 */
export const pickSqlFiles = (filterName: string) => invoke<string[]>("pick_sql_files", { filterName });
/**
 * Lets the backend read and write `.sql` paths dropped on the window or restored with the session.
 * It accepts any absolute `.sql` path: the allowed list is advisory, not a security boundary.
 */
export const allowDroppedPaths = (paths: string[]) => invoke<void>("allow_dropped_paths", { paths });
/**
 * For each path, whether the backend allows it and its file still exists: `false` when it is not
 * allowed or not there, `null` when it could not be checked (no permission, an I/O error). Rejects
 * only when the check could not run at all.
 */
export const sqlFilesExist = (paths: string[]) => invoke<(boolean | null)[]>("sql_files_exist", { paths });
/**
 * Rejects with `"not_allowed"`, `"not_found"`, `"too_large"` (over 20 MB), `"not_text"` or why it
 * could not be read.
 */
export const readSqlFile = (path: string) => invoke<OpenedFile>("read_sql_file", { path });
/** Writes atomically, recreating a deleted file. Rejects with `"not_allowed"` or why it could not be written. */
export const writeSqlFile = (path: string, text: string, encoding: FileEncoding, lineEnding: LineEnding) =>
  invoke<void>("write_sql_file", { path, text, encoding, lineEnding });

/** Subscribes to the explorer and indexing events. Call the returned function to stop. */
export const onAppEvent = (cb: (e: AppEvent) => void): Promise<UnlistenFn> =>
  listen<AppEvent>("app-event", (ev) => cb(ev.payload));

/** The saved workspace session (opaque JSON owned by the UI), or `null` when there is none. */
export const loadSession = () => invoke<string | null>("load_session");

/** Replaces the saved workspace session. Works even when the connections file is read-only. */
export const saveSession = (json: string) => invoke<void>("save_session", { json });

/** Where tulkabd keeps its files (full paths; empty when the platform has no such folder). */
export interface DataLocations {
  configFile: string;
  dataDir: string;
  queriesDir: string;
  cacheFile: string;
  runLogFile: string;
  sessionFile: string;
}
export const dataLocations = () => invoke<DataLocations>("data_locations");
/** Shows the config file, the saved queries folder or the data folder in the system file manager. */
export type DataLocationKey = "config" | "queries" | "data";
export const revealDataLocation = (which: DataLocationKey) => invoke<void>("reveal_data_location", { which });
/** Empties the metadata cache and the search index; connected servers are indexed again. */
export const clearMetadataCache = () => invoke<void>("clear_metadata_cache");
