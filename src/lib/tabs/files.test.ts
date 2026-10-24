// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { ConnectionConfig, OpenedFile } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  runQuery: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
  listSavedQueries: vi.fn(),
  saveQuery: vi.fn(),
  pickSqlFiles: vi.fn(),
  allowDroppedPaths: vi.fn(),
  readSqlFile: vi.fn(),
  writeSqlFile: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { queries } from "../queries/queries.svelte";
import { tabs } from "./tabs.svelte";
import TabStrip from "./TabStrip.svelte";

const conn = { id: "c1", name: "Local", passwordSaved: true, environment: "local" } as ConnectionConfig;
const session = (database: string) => ({
  server: { version: "16", user: "sa", singleDatabase: false },
  databases: ["master", database],
  currentDatabase: database,
});
const flush = () => new Promise((r) => setTimeout(r, 0));

/** What `read_sql_file` returns for `path`: an SSMS file (UTF-16 LE, CRLF). */
const opened = (path: string, text = `SELECT '${path}';`): OpenedFile => ({
  path,
  name: path.split(/[\\/]/).pop()!,
  text,
  encoding: "utf16Le",
  lineEnding: "crlf",
});

let cmp: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.openSession).mockImplementation(async (_tab, _conn, database) => session(database ?? "master"));
  vi.mocked(ipc.listSavedQueries).mockResolvedValue([]);
  vi.mocked(ipc.saveQuery).mockResolvedValue();
  vi.mocked(ipc.allowDroppedPaths).mockResolvedValue();
  vi.mocked(ipc.writeSqlFile).mockResolvedValue();
  vi.mocked(ipc.readSqlFile).mockImplementation(async (path) => opened(path));
  connections.list = [conn];
  queries.reset();
  await tabs.reset();
  tabs.fileError = null;
});

afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

/** The active tab bound to c1 / Ventas, as the tab files open next to. */
async function boundTab() {
  tabs.open({ kind: "query", connectionId: "c1", database: "Ventas" });
  await flush();
}

test("openFiles opens one tab per file, bound to the active tab's connection and database, titled with the file name", async () => {
  await boundTab();
  vi.mocked(ipc.openSession).mockClear();
  await tabs.openFiles(["/home/u/sql/Reporte mensual.sql", "/home/u/sql/b.sql"]);
  await flush();

  const files = tabs.list.filter((x) => x.file);
  expect(files.map((x) => x.title)).toEqual(["Reporte mensual.sql", "b.sql"]);
  expect(files.map((x) => x.file)).toEqual([
    { path: "/home/u/sql/Reporte mensual.sql", encoding: "utf16Le", lineEnding: "crlf" },
    { path: "/home/u/sql/b.sql", encoding: "utf16Le", lineEnding: "crlf" },
  ]);
  expect(files.map((x) => x.pane!.text())).toEqual(["SELECT '/home/u/sql/Reporte mensual.sql';", "SELECT '/home/u/sql/b.sql';"]);
  expect(files.every((x) => !x.dirty && !x.savedQuery)).toBe(true);
  for (const f of files) expect(ipc.openSession).toHaveBeenCalledWith(f.id, "c1", "Ventas", undefined);
  expect(files.map((x) => tabs.binding(x))).toEqual([
    { connectionId: "c1", database: "Ventas" },
    { connectionId: "c1", database: "Ventas" },
  ]);
  expect(tabs.activeId).toBe(files[1].id);
  expect(tabs.fileError).toBeNull();
});

test("openFiles with no active tab opens unbound tabs", async () => {
  await tabs.openFiles(["/a.sql"]);
  expect(tabs.list).toHaveLength(1);
  expect(tabs.binding(tabs.list[0])).toEqual({ connectionId: null, database: null });
  expect(ipc.openSession).not.toHaveBeenCalled();
});

test("opening a file that is already open shows its tab instead of opening another", async () => {
  await tabs.openFiles(["/a.sql", "/b.sql"]);
  const a = tabs.list.find((x) => x.file?.path === "/a.sql")!;
  vi.mocked(ipc.readSqlFile).mockClear();
  await tabs.openFiles(["/a.sql"]);
  expect(tabs.list).toHaveLength(2);
  expect(tabs.activeId).toBe(a.id);
  expect(ipc.readSqlFile).not.toHaveBeenCalled();
  // The same path twice in one call opens it once.
  await tabs.openFiles(["/c.sql", "/c.sql"]);
  expect(tabs.list.filter((x) => x.file?.path === "/c.sql")).toHaveLength(1);
});

test("a file that cannot be read opens no tab and says why; the others still open", async () => {
  vi.mocked(ipc.readSqlFile).mockImplementation(async (path) => {
    if (path === "/bin.sql") throw "not_text";
    return opened(path);
  });
  await tabs.openFiles(["/bin.sql", "/ok.sql"]);
  expect(tabs.list.map((x) => x.title)).toEqual(["ok.sql"]);
  expect(tabs.fileError).toContain("bin.sql");
  expect(tabs.fileError).toContain("not a text file");
  // The next open that works clears it.
  await tabs.openFiles(["/ok2.sql"]);
  expect(tabs.fileError).toBeNull();
});

test("Ctrl+S on a file tab writes the file in its encoding and line endings without asking for a name", async () => {
  await tabs.openFiles(["/q.sql"]);
  const tab = tabs.active!;
  tab.pane!.liveDoc = () => "SELECT 2;\nGO\n";
  tab.dirty = true;
  await tabs.saveActive();
  expect(ipc.writeSqlFile).toHaveBeenCalledWith("/q.sql", "SELECT 2;\nGO\n", "utf16Le", "crlf");
  expect(tabs.nameRequest).toBeNull();
  expect(ipc.saveQuery).not.toHaveBeenCalled();
  expect(tab.dirty).toBe(false);
  expect(tab.title).toBe("q.sql");
});

test("typing while the file is written leaves the tab unsaved", async () => {
  await tabs.openFiles(["/q.sql"]);
  const tab = tabs.active!;
  tab.dirty = true;
  let release!: () => void;
  vi.mocked(ipc.writeSqlFile).mockReturnValue(new Promise<void>((r) => (release = r)));
  const saving = tabs.saveActive();
  tab.pane!.edited();
  release();
  await saving;
  expect(tab.dirty).toBe(true);
});

test("a failed write keeps the tab unsaved and says why above the editor", async () => {
  await tabs.openFiles(["/q.sql"]);
  const tab = tabs.active!;
  tab.dirty = true;
  vi.mocked(ipc.writeSqlFile).mockRejectedValueOnce("could not save /q.sql: it is read-only");
  await tabs.saveActive();
  expect(tab.dirty).toBe(true);
  expect(tab.pane!.notice).toContain("it is read-only");
  await tabs.saveActive();
  expect(tab.dirty).toBe(false);
  expect(tab.pane!.notice).toBeNull();
});

test("Ctrl+S on a file the backend does not allow yet allows it again and writes it", async () => {
  await tabs.openFiles(["/q.sql"]);
  const tab = tabs.active!;
  tab.dirty = true;
  vi.mocked(ipc.writeSqlFile).mockRejectedValueOnce("not_allowed");
  await tabs.saveActive();
  expect(ipc.allowDroppedPaths).toHaveBeenCalledWith(["/q.sql"]);
  expect(ipc.writeSqlFile).toHaveBeenCalledTimes(2);
  expect(tab.dirty).toBe(false);
  expect(tab.pane!.notice).toBeNull();
});

test("dropped files: only .sql paths are allowed, then opened", async () => {
  const order: string[] = [];
  vi.mocked(ipc.allowDroppedPaths).mockImplementation(async () => void order.push("allow"));
  vi.mocked(ipc.readSqlFile).mockImplementation(async (path) => {
    order.push("read");
    return opened(path);
  });
  await tabs.dropFiles(["/a.sql", "/notes.txt", "/B.SQL", "/dir"]);
  expect(ipc.allowDroppedPaths).toHaveBeenCalledWith(["/a.sql", "/B.SQL"]);
  expect(order).toEqual(["allow", "read", "read"]);
  expect(tabs.list.map((x) => x.title)).toEqual(["a.sql", "B.SQL"]);
  vi.mocked(ipc.allowDroppedPaths).mockClear();
  await tabs.dropFiles(["/notes.txt"]);
  expect(ipc.allowDroppedPaths).not.toHaveBeenCalled();
});

test("pickFiles opens the files picked in the dialog; a cancelled dialog opens nothing", async () => {
  vi.mocked(ipc.pickSqlFiles).mockResolvedValueOnce(["/a.sql", "/b.sql"]);
  await tabs.pickFiles();
  expect(ipc.pickSqlFiles).toHaveBeenCalledWith("SQL files");
  expect(tabs.list.map((x) => x.title)).toEqual(["a.sql", "b.sql"]);
  vi.mocked(ipc.pickSqlFiles).mockResolvedValueOnce([]);
  await tabs.pickFiles();
  expect(tabs.list).toHaveLength(2);
});

test("Save as saved query copies a file tab's text to the saved queries of its database; the tab stays on its file", async () => {
  await boundTab();
  await tabs.openFiles(["/sql/Reporte.sql"]);
  await flush();
  const tab = tabs.active!;
  tab.dirty = true;
  tabs.saveAsQuery(tab.id);
  const req = tabs.nameRequest!;
  expect(req.title).toBe("Save query in Ventas");
  expect(req.initial).toBe("Reporte");
  expect(await req.onsubmit("Reporte", false)).toBeNull();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Reporte", "SELECT '/sql/Reporte.sql';", false);
  expect(tabs.nameRequest).toBeNull();
  expect(tab.file?.path).toBe("/sql/Reporte.sql");
  expect(tab.savedQuery).toBeUndefined();
  expect(tab.title).toBe("Reporte.sql");
  // Still unsaved: the file itself was not written.
  expect(tab.dirty).toBe(true);
  vi.mocked(ipc.saveQuery).mockClear();
  await tabs.saveActive();
  expect(ipc.writeSqlFile).toHaveBeenCalledTimes(1);
  expect(ipc.saveQuery).not.toHaveBeenCalled();
});

test("Save as saved query on a tab that is not a file saves it like Ctrl+S, under a new name", async () => {
  await boundTab();
  const tab = tabs.active!;
  tab.pane!.liveDoc = () => "SELECT 1;";
  tabs.saveAsQuery(tab.id);
  expect(await tabs.nameRequest!.onsubmit("Uno", false)).toBeNull();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Ventas", "Uno", "SELECT 1;", false);
  expect(tab.savedQuery).toEqual({ database: "Ventas", name: "Uno" });
  expect(tab.title).toBe("Uno");
  tabs.saveAsQuery(tab.id);
  expect(tabs.nameRequest!.initial).toBe("Uno");
});

test("Save as saved query without a database says to pick one", async () => {
  await tabs.openFiles(["/q.sql"]);
  tabs.saveAsQuery(tabs.active!.id);
  expect(tabs.nameRequest).toBeNull();
  expect(tabs.active!.pane!.notice).toBe("Pick a database to save this query under.");
});

// ------------------------------------------------------------ the tab strip

function strip() {
  cmp = mount(TabStrip, { target: document.body, props: { groupId: tabs.activeGroupId } });
  flushSync();
}
const menuItems = () => [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')];

test("the + button offers New query and Open file…", async () => {
  vi.mocked(ipc.pickSqlFiles).mockResolvedValue(["/a.sql"]);
  strip();
  document.querySelector<HTMLButtonElement>('button[aria-label="New query or open a file"]')!.click();
  flushSync();
  expect(menuItems().map((b) => b.textContent!.replace(/\s+/g, " ").trim())).toEqual(["New query Ctrl T", "Open file… Ctrl O"]);
  menuItems()[0].click();
  flushSync();
  expect(tabs.list.map((x) => x.title)).toEqual(["untitled-1"]);
  expect(menuItems()).toEqual([]);

  document.querySelector<HTMLButtonElement>('button[aria-label="New query or open a file"]')!.click();
  flushSync();
  menuItems()[1].click();
  await flush();
  expect(ipc.pickSqlFiles).toHaveBeenCalled();
  expect(tabs.list.map((x) => x.title)).toEqual(["untitled-1", "a.sql"]);
});

test("a tab's menu saves it as a saved query", async () => {
  await boundTab();
  await tabs.openFiles(["/sql/Reporte.sql"]);
  await flush();
  strip();
  const tabEl = [...document.querySelectorAll<HTMLElement>(".tab")][1];
  tabEl.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: 10, clientY: 10 }));
  flushSync();
  expect(menuItems().map((b) => b.textContent!.trim())).toEqual(["Save as saved query", "Close"]);
  menuItems()[0].click();
  flushSync();
  expect(tabs.nameRequest?.initial).toBe("Reporte");
});
