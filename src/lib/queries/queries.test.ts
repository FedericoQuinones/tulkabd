// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { CachedDatabase, ConnectionConfig, SavedQuery, SearchResponse } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  runQuery: vi.fn(),
  objectDefinition: vi.fn(),
  connectExplorer: vi.fn(),
  explorerDatabases: vi.fn(),
  searchObjects: vi.fn(),
  onAppEvent: vi.fn(),
  listSavedQueries: vi.fn(),
  readSavedQuery: vi.fn(),
  saveQuery: vi.fn(),
  renameSavedQuery: vi.fn(),
  deleteSavedQuery: vi.fn(),
  revealSavedQuery: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { buildMenu, keyOf, type MenuActions, type MenuStore, type Row } from "../explorer/tree";
import { explorer } from "../explorer/tree.svelte";
import { palette } from "../palette/palette.svelte";
import { tabs } from "../tabs/tabs.svelte";
import NameDialog from "./NameDialog.svelte";
import { pickConnection, queries, queryError } from "./queries.svelte";
import { t } from "../i18n/t";

const flush = () => new Promise((r) => setTimeout(r, 0));

const conn = (id: string, name: string) =>
  ({ id, name, folderId: null, passwordSaved: true, environment: "local", color: "#4a8ad6" }) as ConnectionConfig;
const db = (name: string): CachedDatabase => ({ name, noAccess: false, indexedAt: 1, tables: 0, views: 0, procedures: 0 });
const session = (database: string, databases = [database]) => ({
  server: { version: "16", user: "sa", singleDatabase: false },
  databases,
  currentDatabase: database,
});
const saved = (database: string, name: string): SavedQuery => ({ database, name, modifiedMs: 0 });
const response = (queries: SearchResponse["queries"]): SearchResponse => ({
  hits: [],
  totalMatches: 0,
  databasesMatched: 0,
  indexing: 0,
  queries,
});

let cmp: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.openSession).mockImplementation(async (_tab, _conn, database) => session(database ?? "master"));
  vi.mocked(ipc.onAppEvent).mockResolvedValue(() => {});
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([]);
  vi.mocked(ipc.listSavedQueries).mockResolvedValue([]);
  vi.mocked(ipc.saveQuery).mockResolvedValue();
  vi.mocked(ipc.readSavedQuery).mockResolvedValue("SELECT 1;");
  connections.list = [conn("c1", "Sales"), conn("c2", "Local"), conn("c3", "Backup")];
  explorer.reset();
  palette.hide();
  queries.reset();
  await tabs.reset();
});

afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

/** A query tab bound to `database` on c1, with `text` in its editor. */
async function tabOn(database: string | null, text = "SELECT * FROM Pedidos;") {
  const id = database
    ? tabs.open({ kind: "query", connectionId: "c1", database, sql: text })
    : tabs.open({ kind: "query", sql: text });
  await flush();
  return tabs.list.find((x) => x.id === id)!;
}

test("Ctrl+S on a new tab asks for a name and saves it under the tab's database", async () => {
  const tab = await tabOn("Ventas");
  await tabs.saveActive();
  expect(ipc.saveQuery).not.toHaveBeenCalled();
  const req = tabs.nameRequest!;
  expect(req).not.toBeNull();
  expect(await req.onsubmit("Reporte", false)).toBeNull();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Reporte", "SELECT * FROM Pedidos;", false);
  expect(tabs.nameRequest).toBeNull();
  expect(tab.title).toBe("Reporte");
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Reporte" });
  expect(tab.dirty).toBe(false);
  expect(ipc.listSavedQueries).toHaveBeenCalled();
});

test("a second Ctrl+S overwrites the saved query without asking", async () => {
  await tabOn("Ventas");
  await tabs.saveActive();
  await tabs.nameRequest!.onsubmit("Reporte", false);
  vi.mocked(ipc.saveQuery).mockClear();
  await tabs.saveActive();
  expect(tabs.nameRequest).toBeNull();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Reporte", "SELECT * FROM Pedidos;", true);
});

test("a taken name keeps the dialog open with the error; Replace overwrites it", async () => {
  const tab = await tabOn("Ventas");
  vi.mocked(ipc.saveQuery).mockRejectedValueOnce("exists");
  await tabs.saveActive();
  const req = tabs.nameRequest!;
  expect(await req.onsubmit("Reporte", false)).toBe("exists");
  expect(tabs.nameRequest).toBe(req);
  expect(tab.savedQuery).toBeUndefined();
  expect(await req.onsubmit("Reporte", true)).toBeNull();
  expect(ipc.saveQuery).toHaveBeenLastCalledWith("Ventas", "Reporte", "SELECT * FROM Pedidos;", true);
  expect(tab.title).toBe("Reporte");
});

test("Ctrl+S without a database shows the notice and calls nothing", async () => {
  const tab = await tabOn(null);
  await tabs.saveActive();
  expect(tabs.nameRequest).toBeNull();
  expect(ipc.saveQuery).not.toHaveBeenCalled();
  expect(tab.pane!.notice).toBe("Pick a database to save this query under.");
});

test("editing after saving marks the tab dirty again", async () => {
  const tab = await tabOn("Ventas");
  tab.pane!.edited();
  expect(tab.dirty).toBe(true);
  await tabs.saveActive();
  await tabs.nameRequest!.onsubmit("Reporte", false);
  expect(tab.dirty).toBe(false);
  tab.pane!.edited();
  expect(tab.dirty).toBe(true);
  await tabs.saveActive();
  expect(tab.dirty).toBe(false);
});

test("a saved query's menu has Open, Rename, Delete and Reveal in file manager", () => {
  const row: Row = {
    key: keyOf.savedQuery("c1", "Ventas", "Reporte"),
    level: 4,
    parent: keyOf.saved("c1", "Ventas"),
    expandable: false,
    expanded: false,
    kind: "savedQuery",
    connectionId: "c1",
    database: "Ventas",
    name: "Reporte",
  };
  const ui = { rename: vi.fn(), remove: vi.fn() } as unknown as MenuActions;
  const store = { openSavedQuery: vi.fn(), revealSavedQuery: vi.fn() } as unknown as MenuStore;
  const items = buildMenu(row, ui, store);
  expect(items.map((m) => m.label)).toEqual(["Open", "Rename", "Delete", "Reveal in file manager"]);
  items[0].run();
  expect(store.openSavedQuery).toHaveBeenCalledWith("c1", "Ventas", "Reporte");
  items[1].run();
  expect(ui.rename).toHaveBeenCalledWith(row);
  items[2].run();
  expect(ui.remove).toHaveBeenCalledWith(row);
  items[3].run();
  expect(store.revealSavedQuery).toHaveBeenCalledWith("Ventas", "Reporte");
});

test("the tree shows Saved queries first under each database, with the count and the queries", async () => {
  vi.mocked(ipc.listSavedQueries).mockResolvedValue([saved("Ventas", "Reporte"), saved("Ventas", "Top clientes"), saved("Hist", "x")]);
  await queries.reload();
  explorer.expanded.add("c2");
  explorer.state("c2").databases = [db("Ventas")];
  explorer.expanded.add(keyOf.db("c2", "Ventas"));
  const under = () => explorer.rows.filter((r) => r.parent === keyOf.db("c2", "Ventas"));
  const first = under()[0];
  expect(first.kind).toBe("saved");
  expect(first.kind === "saved" && first.count).toBe(2);
  expect(under().map((r) => r.kind)).toEqual(["saved", "group", "group", "group"]);
  explorer.expanded.add(keyOf.saved("c2", "Ventas"));
  const names = explorer.rows.flatMap((r) => (r.kind === "savedQuery" ? [r.name] : []));
  expect(names).toEqual(["Reporte", "Top clientes"]);
});

test("D6: a saved query opens on the active tab's connection when it has that database", () => {
  const active = { id: "c1", connected: false, databases: ["Ventas"] };
  const other = { id: "c2", connected: true, databases: ["Ventas"] };
  expect(pickConnection("Ventas", "c1", [other, active])).toBe("c1");
  expect(pickConnection("Ventas", "c1", [other, { ...active, databases: ["Hist"] }])).toBe("c2");
  expect(pickConnection("Ventas", null, [{ ...other, connected: false }])).toBeNull();
});

async function chooseQueryHit(database: string, name: string) {
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([{ database, name, indices: [0] }]));
  palette.show();
  await palette.search("rep");
  expect(palette.items.map((i) => i.kind)).toEqual(["query"]);
  await palette.choose("open");
  await flush();
  return tabs.active!;
}

test("Enter on a saved query hit binds the tab to the active tab's connection when it has the database", async () => {
  vi.mocked(ipc.openSession).mockImplementation(async (_t, _c, database) => session(database ?? "master", ["master", "Ventas"]));
  await tabOn("master");
  // c2 is connected and has Ventas too, but the active tab's connection wins.
  connections.list = [conn("c2", "Local"), conn("c1", "Sales")];
  explorer.state("c2").status = "connected";
  explorer.state("c2").databases = [db("Ventas")];
  const tab = await chooseQueryHit("Ventas", "Reporte");
  expect(ipc.readSavedQuery).toHaveBeenCalledWith("Ventas", "Reporte");
  expect(tab.title).toBe("Reporte");
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Reporte" });
  expect(tab.pane!.connectionId).toBe("c1");
  expect(tab.pane!.database).toBe("Ventas");
  expect(tab.pane!.text()).toBe("SELECT 1;");
});

test("Enter on a saved query hit falls back to the first connected connection that has the database", async () => {
  await tabOn("master");
  explorer.state("c1").databases = [db("master")];
  // c2 has Ventas cached but is offline; c3 is connected and has it.
  explorer.state("c2").databases = [db("Ventas")];
  explorer.state("c3").status = "connected";
  explorer.state("c3").databases = [db("Ventas")];
  const tab = await chooseQueryHit("Ventas", "Reporte");
  expect(tab.pane!.connectionId).toBe("c3");
  expect(tab.pane!.database).toBe("Ventas");
});

test("Enter on a saved query hit opens an unbound tab when no connection has the database", async () => {
  explorer.state("c2").status = "connected";
  explorer.state("c2").databases = [db("Hist")];
  const tab = await chooseQueryHit("Ventas", "Reporte");
  expect(ipc.openSession).not.toHaveBeenCalled();
  expect(tab.pane!.connectionId).toBeNull();
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Reporte" });
  // Ctrl+S still knows where it belongs.
  await tabs.saveActive();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Reporte", "SELECT 1;", true);
});

test("opening a saved query that is already open activates its tab", async () => {
  const first = await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, null);
  tabs.open({ kind: "query" });
  const again = await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, null);
  expect(again).toBe(first);
  expect(tabs.activeId).toBe(first);
  expect(tabs.list.filter((x) => x.savedQuery?.name === "Reporte")).toHaveLength(1);
  expect(ipc.readSavedQuery).toHaveBeenCalledTimes(1);
});

test("renaming a saved query renames its open tab; deleting it marks the tab unsaved", async () => {
  vi.mocked(ipc.renameSavedQuery).mockResolvedValue();
  vi.mocked(ipc.deleteSavedQuery).mockResolvedValue();
  const id = await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, null);
  const tab = tabs.list.find((x) => x.id === id)!;
  await tabs.renameSavedQuery("Ventas", "Reporte", "Reporte 2024");
  expect(ipc.renameSavedQuery).toHaveBeenCalledWith("Ventas", "Reporte", "Reporte 2024");
  expect(tab.title).toBe("Reporte 2024");
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Reporte 2024" });
  expect(tab.dirty).toBe(false);
  await tabs.deleteSavedQuery("Ventas", "Reporte 2024");
  expect(ipc.deleteSavedQuery).toHaveBeenCalledWith("Ventas", "Reporte 2024");
  expect(tab.dirty).toBe(true);
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Reporte 2024" });
  expect(tab.pane!.notice).toBe("This saved query was deleted. Press Ctrl S to save it again.");
  await tabs.saveActive();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Reporte 2024", "SELECT 1;", true);
  expect(tab.pane!.notice).toBeNull();
});

test("queries.list reads the cache by database", async () => {
  vi.mocked(ipc.listSavedQueries).mockResolvedValue([saved("Hist", "a"), saved("Ventas", "b")]);
  expect(queries.list("Ventas")).toEqual([]);
  await queries.reload();
  expect(queries.list("Ventas").map((q) => q.name)).toEqual(["b"]);
  expect(queries.list("ventas")).toEqual([]);
});

test("NameDialog shows a taken name inline and offers Replace", async () => {
  const onsubmit = vi.fn(async (_name: string, overwrite: boolean) => (overwrite ? null : "exists"));
  const oncancel = vi.fn();
  cmp = mount(NameDialog, { target: document.body, props: { title: "Save query", initial: "", onsubmit, oncancel } });
  flushSync();
  const input = document.querySelector("input")!;
  input.value = "Reporte";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await flush();
  flushSync();
  expect(onsubmit).toHaveBeenCalledWith("Reporte", false);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("already exists");
  const replace = [...document.querySelectorAll("button")].find((b) => b.textContent === "Replace")!;
  replace.click();
  await flush();
  expect(onsubmit).toHaveBeenLastCalledWith("Reporte", true);
});

test("NameDialog does not submit a blank name", async () => {
  const onsubmit = vi.fn(async () => null);
  cmp = mount(NameDialog, { target: document.body, props: { title: "Save query", initial: "  ", onsubmit, oncancel: vi.fn() } });
  flushSync();
  document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  await flush();
  flushSync();
  expect(onsubmit).not.toHaveBeenCalled();
  expect(document.querySelector('[role="alert"]')?.textContent).toBe("Enter a name.");
});

test("the notice of a failed Ctrl+S goes away once a save succeeds", async () => {
  const tab = await tabOn(null);
  await tabs.saveActive();
  expect(tab.pane!.notice).toBe("Pick a database to save this query under.");
  tab.pane!.database = "Ventas";
  await tabs.saveActive();
  await tabs.nameRequest!.onsubmit("Reporte", false);
  expect(tab.pane!.notice).toBeNull();
  vi.mocked(ipc.saveQuery).mockRejectedValueOnce("disk full");
  await tabs.saveActive();
  expect(tab.pane!.notice).toBe("Could not save: disk full");
  await tabs.saveActive();
  expect(tab.pane!.notice).toBeNull();
});

test("a tree click reuses only a tab bound to the clicked connection and database", async () => {
  const prod = await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, "c1");
  // Clicked again while its session is still opening: the same tab.
  expect(await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, "c1")).toBe(prod);
  await flush();
  expect(await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, "c1")).toBe(prod);
  const test = await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, "c2");
  expect(test).not.toBe(prod);
  await flush();
  const tab = tabs.list.find((x) => x.id === test)!;
  expect(tab.pane!.connectionId).toBe("c2");
  expect(tab.pane!.database).toBe("Ventas");
  expect(tabs.activeId).toBe(test);
  // Without a connection (palette, Rail) any open tab of the query is reused: the first one.
  expect(await tabs.openSavedQuery({ database: "Ventas", name: "Reporte" }, null)).toBe(prod);
  expect(ipc.readSavedQuery).toHaveBeenCalledTimes(2);
});

test("Ctrl+S on a definition tab waits for the definition and offers the object's name", async () => {
  let load!: (text: string) => void;
  vi.mocked(ipc.objectDefinition).mockReturnValueOnce(new Promise((r) => (load = r)));
  const id = tabs.open({ kind: "definition", connectionId: "c1", database: "Ventas", schema: "dbo", name: "CerrarMes" });
  await flush();
  const tab = tabs.list.find((x) => x.id === id)!;
  await tabs.saveActive();
  expect(tabs.nameRequest).toBeNull();
  load("CREATE PROCEDURE A");
  await flush();
  await tabs.saveActive();
  expect(tabs.nameRequest?.initial).toBe("CerrarMes");
  // A Retry still in flight when the tab becomes a saved query must not replace its text.
  let reload!: (text: string) => void;
  vi.mocked(ipc.objectDefinition).mockReturnValueOnce(new Promise((r) => (reload = r)));
  const retry = tabs.retry(id);
  await tabs.nameRequest!.onsubmit("CerrarMes", false);
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "CerrarMes", "CREATE PROCEDURE A", false);
  reload("CREATE PROCEDURE B");
  await retry;
  expect(tab.pane!.text()).toBe("CREATE PROCEDURE A");
});

test("NameDialog gives the focus back when it closes", () => {
  const before = document.createElement("button");
  document.body.append(before);
  before.focus();
  const dialog = mount(NameDialog, { target: document.body, props: { title: "Save query", initial: "", onsubmit: vi.fn(), oncancel: vi.fn() } });
  flushSync();
  expect(document.activeElement?.tagName).toBe("INPUT");
  unmount(dialog);
  expect(document.activeElement).toBe(before);
});

test("a saved query that is not text says so", () => {
  expect(queryError("not_text")).toBe(t("queries.errors.notText"));
  expect(t("queries.errors.notText")).not.toBe("queries.errors.notText");
});
