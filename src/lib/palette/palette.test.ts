// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { AppEvent, ConnectionConfig, ObjectKind, PaletteHit, SearchResponse } from "../ipc";

vi.mock("../ipc", () => ({
  connectExplorer: vi.fn(),
  explorerDatabases: vi.fn(),
  searchObjects: vi.fn(),
  onAppEvent: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  objectDefinition: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { explorer } from "../explorer/tree.svelte";
import { tabs } from "../tabs/tabs.svelte";
import { canRun, groupHits, highlight, MAX_HITS_PER_GROUP, paletteKey } from "./hits";
import { palette } from "./paletteState.svelte";
import Palette from "./Palette.svelte";

const flush = () => new Promise((r) => setTimeout(r, 0));

const conn = (id: string, name: string, passwordSaved = true) =>
  ({ id, name, folderId: null, passwordSaved, environment: "local", color: "#4a8ad6" }) as ConnectionConfig;

function hit(over: Partial<PaletteHit> & { name: string; schema?: string; kind?: PaletteHit["object"]["kind"] }): PaletteHit {
  const { name, schema = "dbo", kind = "table", ...rest } = over;
  return {
    connectionId: "c1",
    connectionName: "Sales",
    folderPath: [],
    connected: true,
    database: "Ventas",
    object: { schema, name, kind },
    indices: [],
    ...rest,
  };
}
const response = (hits: PaletteHit[], indexing = 0): SearchResponse => ({
  hits,
  totalMatches: hits.length,
  databasesMatched: new Set(hits.map((h) => `${h.connectionId}/${h.database}`)).size,
  indexing,
  queries: [],
});

let cmp: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.onAppEvent).mockResolvedValue(() => {});
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([]);
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([]));
  connections.list = [conn("c1", "Sales"), conn("c2", "Local", false)];
  explorer.reset();
  palette.hide();
  await tabs.reset();
});

afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

test("a response that arrives after a newer keystroke is dropped", async () => {
  palette.show();
  let resolveOld!: (r: SearchResponse) => void;
  vi.mocked(ipc.searchObjects)
    .mockReturnValueOnce(new Promise((r) => (resolveOld = r)))
    .mockResolvedValueOnce(response([hit({ name: "Pedidos" })]));
  const old = palette.search("p");
  await palette.search("pe");
  resolveOld(response([hit({ name: "Productos" })]));
  await old;
  expect(palette.query).toBe("pe");
  expect(palette.flat.map((h) => h.object.name)).toEqual(["Pedidos"]);
});

test("the palette searches again when the startup index finishes loading", async () => {
  let emit!: (e: AppEvent) => void;
  vi.mocked(ipc.onAppEvent).mockImplementation(async (cb) => {
    emit = cb;
    return () => {};
  });
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([], 1));
  palette.show();
  await flush();
  await palette.search("ped");
  expect(palette.response?.hits).toEqual([]);
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos" })]));
  emit({ kind: "indexLoaded" });
  await flush();
  expect(palette.response?.hits.map((h) => h.object.name)).toEqual(["Pedidos"]);
});

test("groups hits by folder › connection › database, keeping the ranking order", () => {
  const a = hit({ name: "Pedidos", folderPath: ["Cliente A", "Proyecto X"] });
  const b = hit({ name: "Pedidos", connectionId: "c2", connectionName: "Local", database: "master", connected: false });
  const c = hit({ name: "PedidoItems", folderPath: ["Cliente A", "Proyecto X"] });
  const groups = groupHits([a, b, c]);
  expect(groups.map((g) => g.path.join(" › "))).toEqual(["Cliente A › Proyecto X › Sales › Ventas", "Local › master"]);
  expect(groups[0].hits).toEqual([a, c]);
  expect(groups[1].connected).toBe(false);
});

test("a group with many hits is capped so the next group's best hit stays near the top", () => {
  const many = Array.from({ length: 30 }, (_, i) => hit({ name: `Sales${i}` }));
  const other = hit({ name: "Sales", connectionId: "c2", connectionName: "Local", database: "master" });
  const groups = groupHits([...many, other]);
  expect(groups.map((g) => g.hits.length)).toEqual([MAX_HITS_PER_GROUP, 1]);
  expect(groups[0].hidden).toBe(30 - MAX_HITS_PER_GROUP);
  expect(groups[1].hidden).toBe(0);
  // A single database keeps every hit.
  const alone = groupHits(many);
  expect(alone).toHaveLength(1);
  expect(alone[0].hits).toHaveLength(30);
  expect(alone[0].hidden).toBe(0);
  // Groups stay ordered by their best hit.
  const first = hit({ name: "Pedidos", connectionId: "c2", connectionName: "Local", database: "master" });
  expect(groupHits([first, ...many]).map((g) => g.path.at(-1))).toEqual(["master", "Ventas"]);
});

test("highlight marks code points, not UTF-16 units", () => {
  // "𝔸" is two UTF-16 units but one code point, so "ñ" sits at code point 4.
  expect(highlight("𝔸bcñ.x", [3, 5])).toEqual([
    { text: "𝔸bc", hit: false },
    { text: "ñ", hit: true },
    { text: ".", hit: false },
    { text: "x", hit: true },
  ]);
  expect(highlight("dbo.Año", [5])).toEqual([
    { text: "dbo.A", hit: false },
    { text: "ñ", hit: true },
    { text: "o", hit: false },
  ]);
});

test("palette keys", () => {
  const key = (key: string, mods: Partial<KeyboardEvent> = {}, caretAtEnd = true) =>
    paletteKey({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...mods } as KeyboardEvent, false, caretAtEnd);
  expect(key("ArrowDown")).toBe("down");
  expect(key("ArrowUp")).toBe("up");
  expect(key("Enter")).toBe("open");
  expect(key("Enter", { ctrlKey: true })).toBe("selectTop");
  expect(key("ArrowRight")).toBe("definition");
  expect(key("ArrowRight", {}, false)).toBe(null);
  expect(key("Escape")).toBe("close");
  expect(key("a")).toBe(null);
});

test("Ctrl+Enter opens a Select top 10 tab that runs", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos" })]));
  palette.show();
  cmp = mount(Palette, { target: document.body });
  await palette.search("ped");
  flushSync();
  const input = document.querySelector("input")!;
  input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true }));
  await flush();
  expect(open).toHaveBeenCalledWith({
    kind: "query",
    connectionId: "c1",
    database: "Ventas",
    sql: "SELECT TOP 10 * FROM [dbo].[Pedidos];",
    run: true,
  });
  expect(palette.isOpen).toBe(false);
});

test("Ctrl+Enter on a not connected hit opens the tab directly; the tab connects itself", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Order Details", connected: false })]));
  palette.show();
  await palette.search("ord");
  await palette.choose("selectTop");
  expect(ipc.connectExplorer).not.toHaveBeenCalled();
  expect(open).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT TOP 10 * FROM [dbo].[Order Details];", run: true }));
});

test("Enter on a not connected table connects the explorer, then opens its structure", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer).mockResolvedValue({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos", connected: false })]));
  palette.show();
  await palette.search("ped");
  const done = palette.choose("open");
  expect(open).not.toHaveBeenCalled();
  await done;
  await flush();
  expect(ipc.connectExplorer).toHaveBeenCalledWith("c1", undefined);
  expect(open).toHaveBeenCalledWith({ kind: "structure", connectionId: "c1", database: "Ventas", schema: "dbo", name: "Pedidos" });
});

test("a not connected procedure waits for the password prompt before opening its definition", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer)
    .mockRejectedValueOnce("password_required")
    .mockResolvedValueOnce({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  const proc = hit({ name: "CerrarMes", kind: "procedure", connectionId: "c2", connectionName: "Local", connected: false });
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([proc]));
  palette.show();
  await palette.search("cerrar");
  await palette.choose("open");
  expect(explorer.passwordRequest?.conn.id).toBe("c2");
  expect(open).not.toHaveBeenCalled();
  await explorer.submitPassword("pw");
  await flush();
  expect(ipc.connectExplorer).toHaveBeenLastCalledWith("c2", "pw");
  expect(open).toHaveBeenCalledWith({ kind: "definition", connectionId: "c2", database: "Ventas", schema: "dbo", name: "CerrarMes" });
});

test("functions and synonyms open their definition, and Select top 10 suits what a synonym points to", () => {
  const of = (kind: ObjectKind, target?: ObjectKind) => ({ schema: "dbo", name: "x", kind, target });
  expect(canRun("selectTop", of("function"))).toBe(false);
  expect(canRun("selectTop", of("tableFunction"))).toBe(false);
  expect(canRun("selectTop", of("synonym", "table"))).toBe(true);
  expect(canRun("selectTop", of("synonym", "view"))).toBe(true);
  for (const target of ["procedure", "function", "tableFunction"] as const) {
    expect(canRun("selectTop", of("synonym", target))).toBe(false);
  }
  // An unknown target (linked server, unreadable database) is left to the server.
  expect(canRun("selectTop", of("synonym"))).toBe(true);
  expect(canRun("definition", of("function"))).toBe(true);
  expect(canRun("definition", of("synonym"))).toBe(true);
});

test("a cancelled password prompt opens nothing", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("password_required");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos", connectionId: "c2", connected: false })]));
  palette.show();
  await palette.search("ped");
  await palette.choose("open");
  explorer.cancelPassword();
  vi.mocked(ipc.connectExplorer).mockResolvedValueOnce({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  await explorer.connect("c2", "pw");
  expect(open).not.toHaveBeenCalled();
});

test("keys during IME composition do nothing", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos" })]));
  palette.show();
  cmp = mount(Palette, { target: document.body });
  await palette.search("ped");
  flushSync();
  const input = document.querySelector("input")!;
  for (const key of ["Enter", "Escape", "ArrowDown"]) {
    const e = new KeyboardEvent("keydown", { key, isComposing: true, bubbles: true, cancelable: true });
    input.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
  }
  await flush();
  expect(open).not.toHaveBeenCalled();
  expect(palette.isOpen).toBe(true);
  expect(palette.selected).toBe(0);
});

test("Ctrl+P inside the palette is swallowed on a Cyrillic layout too", () => {
  palette.show();
  cmp = mount(Palette, { target: document.body });
  flushSync();
  const input = document.querySelector("input")!;
  const e = new KeyboardEvent("keydown", { key: "з", code: "KeyP", ctrlKey: true, bubbles: true, cancelable: true });
  input.dispatchEvent(e);
  expect(e.defaultPrevented).toBe(true);
  expect(palette.isOpen).toBe(true);
});

test("the footer counts matches and databases, and says when databases are indexing", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos" }), hit({ name: "Pagos", database: "Hist" })], 3));
  palette.show();
  cmp = mount(Palette, { target: document.body });
  await palette.search("p");
  flushSync();
  const foot = document.querySelector(".foot")!.textContent!;
  expect(foot).toContain("2 matches in 2 databases");
  expect(foot).toContain("indexing 3 databases…");
});

test("a failed connect shows its error in the palette and opens nothing", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("Login failed for user 'sa'.");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos", connected: false })]));
  palette.show();
  await palette.search("ped");
  await palette.choose("open");
  expect(open).not.toHaveBeenCalled();
  expect(palette.isOpen).toBe(true);
  expect(palette.query).toBe("ped");
  expect(palette.error).toBe("Could not connect to Sales: Login failed for user 'sa'.");
  cmp = mount(Palette, { target: document.body });
  flushSync();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("Login failed");
  await palette.search("pedi");
  expect(palette.error).toBeNull();
});

test("a connect that fails after the password prompt reopens the palette with the error", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("password_required").mockRejectedValueOnce("Login failed");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos", connectionId: "c2", connectionName: "Local", connected: false })]));
  palette.show();
  await palette.search("ped");
  await palette.choose("open");
  expect(palette.isOpen).toBe(false);
  await explorer.submitPassword("bad");
  expect(open).not.toHaveBeenCalled();
  expect(palette.isOpen).toBe(true);
  expect(palette.query).toBe("ped");
  expect(palette.error).toBe("Could not connect to Local: Login failed");
});

test("a connection deleted meanwhile shows an error in the palette", async () => {
  const open = vi.spyOn(tabs, "open").mockReturnValue("t");
  vi.mocked(ipc.connectExplorer).mockRejectedValueOnce("not_found");
  vi.mocked(ipc.searchObjects).mockResolvedValue(response([hit({ name: "Pedidos", connectionId: "c2", connectionName: "Local", connected: false })]));
  palette.show();
  await palette.search("ped");
  await palette.choose("open");
  await flush();
  expect(open).not.toHaveBeenCalled();
  expect(palette.isOpen).toBe(true);
  expect(palette.error).toBe("Could not connect to Local: That connection no longer exists.");
});

test("saved queries come after the objects, so Enter on the first row opens the best object", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValue({
    ...response([hit({ name: "Reportes" })]),
    queries: [{ database: "Ventas", name: "Reporte", indices: [0] }],
  });
  palette.show();
  cmp = mount(Palette, { target: document.body });
  await palette.search("rep");
  flushSync();
  expect(palette.items.map((i) => i.kind)).toEqual(["object", "query"]);
  const rows = [...document.querySelectorAll('[role="option"]')];
  expect(rows.map((r) => r.id)).toEqual(["palette-0", "palette-1"]);
  expect(rows[1].textContent).toContain("Reporte");
  expect(document.querySelectorAll(".group .path")[1].textContent).toBe("Saved queries › Ventas");
});

test("the footer counts saved query matches too", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValue({
    ...response([]),
    queries: [
      { database: "Ventas", name: "Reporte", indices: [] },
      { database: "Ventas", name: "Reporte mensual", indices: [] },
    ],
  });
  palette.show();
  cmp = mount(Palette, { target: document.body });
  await palette.search("rep");
  flushSync();
  expect(document.querySelector(".foot")!.textContent).toContain("2 matches in 1 database");
  expect(document.querySelector(".none")).toBeNull();

  vi.mocked(ipc.searchObjects).mockResolvedValue({
    ...response([hit({ name: "Reportes" }), hit({ name: "Pagos", database: "Hist" })]),
    queries: [
      { database: "Ventas", name: "Reporte", indices: [] },
      { database: "Otra", name: "x", indices: [] },
    ],
  });
  await palette.search("repo");
  flushSync();
  // Ventas already counts through its objects; Otra is one more database.
  expect(document.querySelector(".foot")!.textContent).toContain("4 matches in 3 databases");
});
