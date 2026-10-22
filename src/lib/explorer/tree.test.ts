import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { CachedDatabase, ConnectionConfig, Folder, PaletteHit, SearchResponse } from "../ipc";

vi.mock("../ipc", () => ({
  connectExplorer: vi.fn(),
  disconnectExplorer: vi.fn(),
  refreshMetadata: vi.fn(),
  explorerDatabases: vi.fn(),
  explorerObjects: vi.fn(),
  explorerColumns: vi.fn(),
  searchObjects: vi.fn(),
  listFolders: vi.fn(),
  listExpanded: vi.fn(),
  setExpanded: vi.fn(),
  moveFolder: vi.fn(),
  moveConnection: vi.fn(),
  createFolder: vi.fn(),
  renameFolder: vi.fn(),
  deleteFolder: vi.fn(),
  listConnections: vi.fn(),
  saveConnection: vi.fn(),
  deleteConnection: vi.fn(),
  onAppEvent: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  runQuery: vi.fn(),
  objectDefinition: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { tabs } from "../tabs/tabs.svelte";
import {
  buildMenu,
  buildTree,
  canMoveFolder,
  keyOf,
  navigate,
  quoteName,
  selectTop10,
  type MenuActions,
  type Row,
} from "./tree";
import { explorer } from "./tree.svelte";

const flush = () => new Promise((r) => setTimeout(r, 0));

const folders: Folder[] = [
  { id: "fx", name: "Proyecto X", parentId: "fa" },
  { id: "fa", name: "Cliente A", parentId: null },
  { id: "fo", name: "Otros", parentId: null },
];
const conn = (id: string, name: string, folderId: string | null, passwordSaved = true) =>
  ({ id, name, folderId, passwordSaved, environment: "local", color: "#4a8ad6" }) as ConnectionConfig;
const sales = conn("c1", "Sales", "fx");
const local = conn("c2", "Local", null);
const billing = conn("c3", "billing", null);
const db = (name: string, over: Partial<CachedDatabase> = {}): CachedDatabase => ({
  name,
  noAccess: false,
  indexedAt: 1,
  tables: 0,
  views: 0,
  procedures: 0,
  ...over,
});

const labels = () => explorer.rows.map(label);
function label(r: Row): string {
  switch (r.kind) {
    case "folder":
      return r.folder.name;
    case "connection":
      return r.conn.name;
    case "database":
      return `db:${r.db.name}`;
    case "group":
      return `group:${r.objectKind}`;
    case "object":
      return `${r.object.schema}.${r.object.name}`;
    case "column":
      return `col:${r.column.name}`;
    case "notice":
      return `notice:${r.text}`;
  }
}

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([]);
  vi.mocked(ipc.explorerObjects).mockResolvedValue([]);
  vi.mocked(ipc.explorerColumns).mockResolvedValue([]);
  vi.mocked(ipc.setExpanded).mockResolvedValue();
  vi.mocked(ipc.listFolders).mockResolvedValue(folders);
  vi.mocked(ipc.listExpanded).mockResolvedValue([]);
  vi.mocked(ipc.listConnections).mockResolvedValue([sales, local, billing]);
  vi.mocked(ipc.searchObjects).mockResolvedValue({ hits: [], totalMatches: 0, databasesMatched: 0, indexing: 0 });
  vi.mocked(ipc.onAppEvent).mockResolvedValue(() => {});
  vi.mocked(ipc.closeSession).mockResolvedValue();
  connections.list = [sales, local, billing];
  explorer.reset();
  explorer.folders = folders;
  explorer.persistDelayMs = 0;
  await tabs.reset();
});

afterEach(() => {
  vi.useRealTimers();
});

test("quoteName brackets both parts and doubles ]", () => {
  expect(quoteName("dbo", "Order Details")).toBe("[dbo].[Order Details]");
  expect(quoteName("we]ird", "a]b")).toBe("[we]]ird].[a]]b]");
  expect(quoteName("dbo", "Pedidos_Año")).toBe("[dbo].[Pedidos_Año]");
  expect(selectTop10("dbo", "Pedidos")).toBe("SELECT TOP 10 * FROM [dbo].[Pedidos];");
});

test("buildTree nests folders, sorts by name and puts root connections last", () => {
  const tree = buildTree(folders, [sales, local, billing], new Set(["fa"]));
  expect(tree.map((n) => n.name)).toEqual(["Cliente A", "Otros", "billing", "Local"]);
  const a = tree[0];
  expect(a.kind === "folder" && a.expanded).toBe(true);
  const x = a.kind === "folder" ? a.children[0] : undefined;
  expect(x?.name).toBe("Proyecto X");
  expect(x?.kind === "folder" && x.expanded).toBe(false);
  expect(x?.kind === "folder" && x.children.map((n) => n.name)).toEqual(["Sales"]);
});

test("buildTree survives unknown parents and parent loops", () => {
  const broken: Folder[] = [
    { id: "a", name: "A", parentId: "b" },
    { id: "b", name: "B", parentId: "a" },
    { id: "c", name: "C", parentId: "gone" },
  ];
  const tree = buildTree(broken, [conn("x", "X", "gone")], new Set());
  const names = JSON.stringify(tree);
  for (const n of ["A", "B", "C", "X"]) expect(names).toContain(`"name":"${n}"`);
});

test("rows show only what is expanded", () => {
  expect(labels()).toEqual(["Cliente A", "Otros", "billing", "Local"]);
  explorer.expanded.add("fa");
  explorer.expanded.add("fx");
  expect(labels()).toEqual(["Cliente A", "Proyecto X", "Sales", "Otros", "billing", "Local"]);
  expect(explorer.rows.map((r) => r.level)).toEqual([1, 2, 3, 1, 1, 1]);
});

test('the filter "proy" keeps Proyecto X and its ancestors only', async () => {
  await explorer.setFilter("proy");
  expect(labels()).toEqual(["Cliente A", "Proyecto X"]);
  // The forced expansion is not persisted.
  expect(explorer.expanded.has("fa")).toBe(false);
  await explorer.setFilter("");
  expect(labels()).toEqual(["Cliente A", "Otros", "billing", "Local"]);
});

test("the filter matches connections and loaded databases case-insensitively", async () => {
  explorer.state("c2").databases = [db("Ventas"), db("master")];
  await explorer.setFilter("VENT");
  expect(labels()).toEqual(["Local", "db:Ventas"]);
});

const hit = (object: PaletteHit["object"], database = "Ventas", connectionId = "c1"): PaletteHit => ({
  connectionId,
  connectionName: "Sales",
  folderPath: ["Cliente A", "Proyecto X"],
  connected: false,
  database,
  object,
  indices: [],
});
const response = (hits: PaletteHit[]): SearchResponse => ({ hits, totalMatches: hits.length, databasesMatched: 1, indexing: 0 });

test("with 2+ characters the filter shows cached objects under their database", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ schema: "dbo", name: "Pedidos", kind: "table" })]));
  await explorer.setFilter("ped");
  expect(ipc.searchObjects).toHaveBeenCalledWith("ped", 200);
  expect(labels()).toEqual(["Cliente A", "Proyecto X", "Sales", "db:Ventas", "group:table", "dbo.Pedidos"]);
});

test("one character does not search", async () => {
  await explorer.setFilter("p");
  expect(ipc.searchObjects).not.toHaveBeenCalled();
});

test("a stale search response is discarded", async () => {
  let resolveOld!: (r: SearchResponse) => void;
  vi.mocked(ipc.searchObjects)
    .mockImplementationOnce(() => new Promise((r) => (resolveOld = r)))
    .mockResolvedValueOnce(response([hit({ schema: "dbo", name: "Pedidos", kind: "table" })]));
  const old = explorer.setFilter("pe");
  await explorer.setFilter("ped");
  resolveOld(response([hit({ schema: "dbo", name: "Pendientes", kind: "view" })]));
  await old;
  expect(labels()).toContain("dbo.Pedidos");
  expect(labels()).not.toContain("dbo.Pendientes");
});

test("init shows expanded connections from the cache without connecting", async () => {
  vi.mocked(ipc.listExpanded).mockResolvedValue(["c2"]);
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([db("Ventas")]);
  await explorer.init();
  await flush();
  expect(ipc.connectExplorer).not.toHaveBeenCalled();
  expect(explorer.state("c2").status).toBe("offline");
  const rows = labels();
  expect(rows.slice(rows.indexOf("Local"))).toEqual(["Local", `notice:${"Offline, showing the cache"}`, "db:Ventas"]);
});

test("expanding a connection connects and loads its databases", async () => {
  vi.mocked(ipc.connectExplorer).mockResolvedValue({ server: { version: "16", user: "sa", singleDatabase: false }, databases: ["Ventas"] });
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([db("Ventas", { tables: 3 })]);
  await explorer.expand("c2");
  await flush();
  expect(ipc.connectExplorer).toHaveBeenCalledWith("c2", undefined);
  expect(explorer.state("c2").status).toBe("connected");
  expect(labels()).toContain("db:Ventas");
  await flush();
  expect(ipc.setExpanded).toHaveBeenCalledWith(["c2"]);
});

test("password_required asks for a password and retries with it", async () => {
  vi.mocked(ipc.connectExplorer)
    .mockRejectedValueOnce("password_required")
    .mockResolvedValueOnce({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  await explorer.expand("c2");
  expect(explorer.passwordRequest?.conn.id).toBe("c2");
  await explorer.submitPassword("secret");
  expect(explorer.passwordRequest).toBeNull();
  expect(ipc.connectExplorer).toHaveBeenLastCalledWith("c2", "secret");
  expect(explorer.state("c2").status).toBe("connected");
});

test("cancelling the password prompt leaves the connection offline", async () => {
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("password_required");
  await explorer.expand("c2");
  explorer.cancelPassword();
  expect(explorer.passwordRequest).toBeNull();
  expect(explorer.state("c2").status).toBe("offline");
});

test("closed is a silent no-op", async () => {
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("closed");
  explorer.handleEvent({ kind: "explorerStatus", connectionId: "c2", status: "disconnected", message: null });
  await explorer.connect("c2");
  expect(explorer.state("c2").status).not.toBe("error");
  expect(explorer.state("c2").message).toBeNull();
});

test("an error status shows the message in the node and Retry connects again", async () => {
  vi.mocked(ipc.connectExplorer).mockResolvedValue({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  explorer.expanded.add("c2");
  explorer.handleEvent({ kind: "explorerStatus", connectionId: "c2", status: "error", message: "Login failed for user 'sa'." });
  const notice = explorer.rows.find((r) => r.kind === "notice");
  expect(notice?.kind === "notice" && notice.text).toBe("Login failed for user 'sa'.");
  expect(notice?.kind === "notice" && notice.tone).toBe("error");
  expect(notice?.kind === "notice" && notice.action?.label).toBe("Retry");
  if (notice?.kind === "notice") await notice.action!.run();
  await flush();
  expect(ipc.connectExplorer).toHaveBeenCalledWith("c2", undefined);
});

test("a failed connect shows the error inline", async () => {
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("authentication failed: bad login");
  await explorer.expand("c2");
  expect(explorer.state("c2").status).toBe("error");
  expect(labels()).toContain("notice:authentication failed: bad login");
});

test("indexProgress marks indexing and refreshes the counters", async () => {
  explorer.expanded.add("c2");
  const st = explorer.state("c2");
  st.status = "connected";
  st.databases = [db("Ventas", { indexedAt: null })];
  explorer.handleEvent({ kind: "indexProgress", connectionId: "c2", database: "Ventas", done: false, noAccess: false });
  const row = () => explorer.rows.find((r) => r.kind === "database");
  expect(row()?.kind === "database" && row()!.indexing).toBe(true);

  vi.mocked(ipc.explorerDatabases).mockResolvedValue([db("Ventas", { indexedAt: 5, tables: 12, views: 3, procedures: 4 })]);
  explorer.handleEvent({ kind: "indexProgress", connectionId: "c2", database: "Ventas", done: true, noAccess: false });
  await flush();
  await flush();
  const r = row();
  expect(r?.kind === "database" && r.indexing).toBe(false);
  expect(r?.kind === "database" && r.db.tables).toBe(12);

  explorer.expanded.add(keyOf.db("c2", "Ventas"));
  const counts = explorer.rows.filter((x) => x.kind === "group").map((g) => (g.kind === "group" ? g.count : 0));
  expect(counts).toEqual([12, 3, 4]);
});

test("progress after an error does not bring back indexing", () => {
  explorer.expanded.add("c2");
  const st = explorer.state("c2");
  st.status = "connected";
  st.databases = [db("Ventas")];
  explorer.handleEvent({ kind: "indexProgress", connectionId: "c2", database: "Ventas", done: false, noAccess: false });
  explorer.handleEvent({ kind: "explorerStatus", connectionId: "c2", status: "error", message: "lost" });
  expect(st.indexing).toEqual([]);
  explorer.handleEvent({ kind: "indexProgress", connectionId: "c2", database: "Ventas", done: false, noAccess: false });
  expect(st.indexing).toEqual([]);
});

test("a database without access is dimmed and not expandable", () => {
  explorer.expanded.add("c2");
  explorer.state("c2").databases = [db("Secret", { noAccess: true })];
  const r = explorer.rows.find((x) => x.kind === "database");
  expect(r?.expandable).toBe(false);
  expect(r?.kind === "database" && r.db.noAccess).toBe(true);
});

test("tables load lazily and expand into columns", async () => {
  explorer.expanded.add("c2");
  explorer.state("c2").databases = [db("Ventas", { tables: 1 })];
  vi.mocked(ipc.explorerObjects).mockResolvedValue([{ schema: "dbo", name: "Pedidos", kind: "table" }]);
  vi.mocked(ipc.explorerColumns).mockResolvedValue([
    { name: "Id", typeName: "int", nullable: false, primaryKey: true, default: null },
    { name: "Nota", typeName: "nvarchar(50)", nullable: true, primaryKey: false, default: null },
  ]);
  await explorer.expand(keyOf.db("c2", "Ventas"));
  await explorer.expand(keyOf.group("c2", "Ventas", "table"));
  expect(ipc.explorerObjects).toHaveBeenCalledWith("c2", "Ventas", "table");
  const obj = { schema: "dbo", name: "Pedidos", kind: "table" as const };
  await explorer.expand(keyOf.object("c2", "Ventas", obj));
  expect(ipc.explorerColumns).toHaveBeenCalledWith("c2", "Ventas", "dbo", "Pedidos");
  expect(labels().slice(-5)).toEqual(["dbo.Pedidos", "col:Id", "col:Nota", "group:view", "group:procedure"]);
});

test("moving a folder into its own child does not call moveFolder", async () => {
  expect(canMoveFolder(folders, "fa", "fx")).toBe(false);
  expect(canMoveFolder(folders, "fa", "fa")).toBe(false);
  expect(canMoveFolder(folders, "fx", "fo")).toBe(true);
  await explorer.move({ kind: "folder", id: "fa" }, "fx");
  expect(ipc.moveFolder).not.toHaveBeenCalled();
  vi.mocked(ipc.moveFolder).mockResolvedValue();
  await explorer.move({ kind: "folder", id: "fx" }, "fo");
  expect(ipc.moveFolder).toHaveBeenCalledWith("fx", "fo");
});

test("moving a connection to the folder it is in does nothing", async () => {
  await explorer.move({ kind: "connection", id: "c1" }, "fx");
  expect(ipc.moveConnection).not.toHaveBeenCalled();
  vi.mocked(ipc.moveConnection).mockResolvedValue();
  await explorer.move({ kind: "connection", id: "c1" }, null);
  expect(ipc.moveConnection).toHaveBeenCalledWith("c1", null);
});

test("a cycle error from the backend shows inline", async () => {
  vi.mocked(ipc.moveFolder).mockRejectedValue("cycle");
  await explorer.move({ kind: "folder", id: "fx" }, "fo");
  expect(explorer.error).toBe("A folder can't go inside itself.");
});

test("keyboard navigation moves, expands, collapses and goes to the parent", () => {
  explorer.expanded.add("fa");
  const rows = explorer.rows; // Cliente A, Proyecto X, Otros, billing, Local
  expect(navigate(rows, "fa", "ArrowDown")).toEqual({ focus: "fx" });
  expect(navigate(rows, "fx", "ArrowUp")).toEqual({ focus: "fa" });
  expect(navigate(rows, "fx", "ArrowRight")).toEqual({ expand: "fx" });
  expect(navigate(rows, "fa", "ArrowRight")).toEqual({ focus: "fx" });
  expect(navigate(rows, "fx", "ArrowLeft")).toEqual({ focus: "fa" });
  expect(navigate(rows, "fa", "ArrowLeft")).toEqual({ collapse: "fa" });
  expect(navigate(rows, "fx", "Enter")).toEqual({ expand: "fx" });
  expect(navigate(rows, "fa", "Enter")).toEqual({ collapse: "fa" });
  expect(navigate(rows, "c2", "ArrowDown")).toEqual({ focus: "c2" });
  expect(navigate(rows, "fa", "End")).toEqual({ focus: "c2" });
  expect(navigate(rows, "c2", "Home")).toEqual({ focus: "fa" });
  expect(navigate(rows, null, "ArrowDown")).toEqual({ focus: "fa" });
  expect(navigate(rows, "fa", "x")).toBeNull();
});

test("collapsing an ancestor moves the focus to it", () => {
  explorer.expanded.add("fa");
  explorer.expanded.add("fx");
  explorer.focusKey = "c1";
  explorer.collapse("fa");
  expect(explorer.focusKey).toBe("fa");
});

const actions = (): MenuActions => ({
  rename: vi.fn(),
  edit: vi.fn(),
  moveTo: vi.fn(),
  remove: vi.fn(),
  newSubfolder: vi.fn(),
});

test("object menus follow the kind", () => {
  explorer.expanded.add("c2");
  const base = { connectionId: "c2", database: "Ventas", level: 4, parent: null, expandable: true, expanded: false };
  const mk = (kind: "table" | "view" | "procedure"): Row => ({
    ...base,
    key: kind,
    kind: "object",
    object: { schema: "dbo", name: "Order Details", kind },
  });
  const ids = (r: Row) => buildMenu(r, actions(), explorer).map((m) => m.id);
  expect(ids(mk("table"))).toEqual(["selectTop", "structure", "copyName"]);
  expect(ids(mk("view"))).toEqual(["selectTop", "structure", "definition", "copyName"]);
  expect(ids(mk("procedure"))).toEqual(["definition", "copyName"]);
  expect(buildMenu(mk("table"), actions(), explorer).find((m) => m.id === "copyName")?.hint).toBe("[dbo].[Order Details]");
});

test("Select top 10 opens a query tab that runs the quoted select", () => {
  const spy = vi.spyOn(tabs, "open").mockReturnValue("t");
  const row: Row = {
    key: "k",
    kind: "object",
    level: 4,
    parent: null,
    expandable: true,
    expanded: false,
    connectionId: "c2",
    database: "Ventas",
    object: { schema: "dbo", name: "Order Details", kind: "table" },
  };
  buildMenu(row, actions(), explorer).find((m) => m.id === "selectTop")!.run();
  expect(spy).toHaveBeenCalledWith({
    kind: "query",
    connectionId: "c2",
    database: "Ventas",
    sql: "SELECT TOP 10 * FROM [dbo].[Order Details];",
    run: true,
  });
  buildMenu(row, actions(), explorer).find((m) => m.id === "structure")!.run();
  expect(spy).toHaveBeenLastCalledWith({ kind: "structure", connectionId: "c2", database: "Ventas", schema: "dbo", name: "Order Details" });
  spy.mockRestore();
});

test("connection menu toggles Connect and Disconnect", () => {
  explorer.expanded.add("c2");
  const row = () => explorer.rows.find((r) => r.kind === "connection" && r.conn.id === "c2")!;
  expect(buildMenu(row(), actions(), explorer).map((m) => m.id)).toEqual([
    "connect",
    "refresh",
    "newTab",
    "rename",
    "edit",
    "moveTo",
    "delete",
  ]);
  explorer.state("c2").status = "connected";
  expect(buildMenu(row(), actions(), explorer)[0].id).toBe("disconnect");
});

test("Copy name writes the quoted name to the clipboard", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal("navigator", { clipboard: { writeText } });
  await explorer.copyName({ schema: "dbo", name: "Pedidos", kind: "table" });
  expect(writeText).toHaveBeenCalledWith("[dbo].[Pedidos]");
  expect(explorer.toast).toBe("Copied [dbo].[Pedidos]");
  vi.unstubAllGlobals();
});

test("a failed column load shows the error with Retry", async () => {
  explorer.expanded.add("c2");
  explorer.state("c2").databases = [db("Ventas", { tables: 1 })];
  const obj = { schema: "dbo", name: "Pedidos", kind: "table" as const };
  vi.mocked(ipc.explorerObjects).mockResolvedValue([obj]);
  vi.mocked(ipc.explorerColumns)
    .mockRejectedValueOnce("cache locked")
    .mockResolvedValueOnce([{ name: "Id", typeName: "int", nullable: false, primaryKey: true, default: null }]);
  await explorer.expand(keyOf.db("c2", "Ventas"));
  await explorer.expand(keyOf.group("c2", "Ventas", "table"));
  await explorer.expand(keyOf.object("c2", "Ventas", obj));
  const notice = explorer.rows.find((r) => r.kind === "notice" && r.tone === "error");
  expect(notice?.kind === "notice" && notice.text).toBe("cache locked");
  expect(notice?.kind === "notice" && notice.action?.label).toBe("Retry");
  if (notice?.kind === "notice") await notice.action!.run();
  expect(ipc.explorerColumns).toHaveBeenCalledTimes(2);
  expect(labels()).toContain("col:Id");
});

test("while a search is pending, old hits that no longer match are hidden", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValueOnce(
    response([hit({ schema: "dbo", name: "Pedidos", kind: "table" }), hit({ schema: "dbo", name: "Personas", kind: "view" })]),
  );
  await explorer.setFilter("pe");
  expect(labels()).toContain("dbo.Personas");
  vi.mocked(ipc.searchObjects).mockImplementation(() => new Promise(() => {}));
  void explorer.setFilter("ped");
  expect(labels()).toContain("dbo.Pedidos");
  expect(labels()).not.toContain("dbo.Personas");
  // A query that does not extend the previous one drops the old hits at once.
  void explorer.setFilter("xy");
  expect(labels()).not.toContain("dbo.Pedidos");
});
