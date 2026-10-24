// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { ConnectionConfig, RunSummary } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  useDatabase: vi.fn(),
  runQuery: vi.fn(),
  cancelQuery: vi.fn(),
  objectDefinition: vi.fn(),
  tableStructure: vi.fn(),
  connectExplorer: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
  completionMetadata: vi.fn(),
  onAppEvent: vi.fn(),
  allowDroppedPaths: vi.fn(),
  sqlFilesExist: vi.fn(),
}));

let closeRequested: (() => Promise<void>) | null = null;
const unlisten = vi.fn();
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: async (handler: () => Promise<void>) => {
      closeRequested = handler;
      return unlisten;
    },
  }),
}));

import * as ipc from "../ipc";
import { EditorView } from "@codemirror/view";
import { connections } from "../connections/store.svelte";
import { t } from "../i18n/t";
import SqlEditor from "../editor/SqlEditor.svelte";
import { tabs } from "../tabs/tabs.svelte";
import { groups } from "./layout";
import { parseSession, restore, snapshot, type SessionV1 } from "./session";
import Workspace from "./Workspace.svelte";

const conn = { id: "c1", name: "Local", passwordSaved: true, environment: "local", guard: "off" } as ConnectionConfig;
const nopw = { id: "c2", name: "Prod", passwordSaved: false, environment: "production", guard: "off" } as ConnectionConfig;
const info = { server: { version: "2022", user: "sa", singleDatabase: false }, databases: ["master", "Sales"], currentDatabase: "Sales" };
const ok: RunSummary = { elapsedMs: 1, finishedAt: "", outcome: "ok" };
const flush = () => new Promise((r) => setTimeout(r, 0));
const known = { connectionIds: new Set(["c1", "c2"]) };

let cmp: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.openSession).mockResolvedValue(info);
  vi.mocked(ipc.runQuery).mockResolvedValue(ok);
  vi.mocked(ipc.objectDefinition).mockRejectedValue("not_connected");
  vi.mocked(ipc.tableStructure).mockRejectedValue("not_connected");
  vi.mocked(ipc.completionMetadata).mockResolvedValue({ schemas: [], objects: [], columns: {} } as never);
  vi.mocked(ipc.onAppEvent).mockResolvedValue(() => {});
  vi.mocked(ipc.allowDroppedPaths).mockResolvedValue();
  vi.mocked(ipc.sqlFilesExist).mockImplementation(async (paths) => paths.map(() => true));
  connections.list = [conn, nopw];
  closeRequested = null;
  await tabs.reset();
});

afterEach(async () => {
  vi.useRealTimers();
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  await tabs.reset();
});

/** A saved session: two groups, a bound query tab with unsaved text, an unbound one, a structure and a definition tab. */
async function buildWorkspace() {
  const a = tabs.open({ kind: "query", connectionId: "c1", database: "Sales", sql: "SELECT 1" });
  const b = tabs.open({ kind: "query" });
  tabs.list.find((x) => x.id === b)!.pane!.editorState = { doc: "SELECT from editorState" };
  const s = tabs.open({ kind: "structure", connectionId: "c1", database: "Sales", schema: "dbo", name: "Orders" });
  tabs.activate(a);
  tabs.splitActive("row");
  const c = tabs.activeId!;
  tabs.list.find((x) => x.id === c)!.pane!.liveDoc = () => "SELECT live";
  const d = tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "vOrders" });
  tabs.activate(c);
  await flush();
  return { a, b, s, c, d };
}

test("snapshot then restore on an empty store reproduces groups, tabs, text and bindings", async () => {
  const ids = await buildWorkspace();
  const saved = snapshot(tabs);
  const json = JSON.stringify(saved);
  await tabs.reset();
  vi.mocked(ipc.openSession).mockClear();

  restore(tabs, parseSession(json)!, known);

  expect(tabs.layout).toEqual(saved.layout);
  expect(tabs.activeGroupId).toBe(saved.activeGroupId);
  const [g1, g2] = groups(tabs.layout);
  expect(tabs.tabsIn(g1).map((x) => x.title)).toEqual(["untitled-1", "untitled-2", t("tabs.structureTitle", { name: "Orders" })]);
  expect(tabs.tabsIn(g2).map((x) => x.id)).toEqual([ids.c, ids.d]);
  expect(tabs.activeIn(g1)?.id).toBe(ids.a);
  expect(tabs.activeIn(g2)?.id).toBe(ids.c);
  const pane = (id: string) => tabs.list.find((x) => x.id === id)!.pane!;
  expect(pane(ids.a).text()).toBe("SELECT 1");
  expect(pane(ids.b).text()).toBe("SELECT from editorState");
  expect(pane(ids.c).text()).toBe("SELECT live");
  expect(tabs.binding(tabs.list.find((x) => x.id === ids.a)!)).toEqual({ connectionId: "c1", database: "Sales" });
  expect(tabs.binding(tabs.list.find((x) => x.id === ids.b)!)).toEqual({ connectionId: null, database: null });
  const structure = tabs.list.find((x) => x.id === ids.s)!;
  expect([structure.kind, structure.connectionId, structure.database, structure.schema, structure.name]).toEqual(["structure", "c1", "Sales", "dbo", "Orders"]);
  expect(snapshot(tabs)).toEqual(saved);
});

test("restored tabs keep savedQuery, file and dirty", async () => {
  const id = tabs.open({ kind: "query" });
  const tab = tabs.list.find((x) => x.id === id)!;
  tab.savedQuery = { database: "Sales", name: "Top orders" };
  tab.file = { path: "/tmp/q.sql", encoding: "utf16Le", lineEnding: "crlf" };
  tab.dirty = true;
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  restore(tabs, parseSession(json)!, known);
  const back = tabs.list[0];
  expect(back.savedQuery).toEqual({ database: "Sales", name: "Top orders" });
  expect(back.file).toEqual({ path: "/tmp/q.sql", encoding: "utf16Le", lineEnding: "crlf" });
  expect(back.dirty).toBe(true);
});

test("restore lets the backend read and write the restored tabs' files", async () => {
  const a = tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  const b = tabs.open({ kind: "query" });
  tabs.list.find((x) => x.id === a)!.file = { path: "/home/u/a.sql", encoding: "utf8", lineEnding: "lf" };
  tabs.list.find((x) => x.id === b)!.file = { path: "C:\\sql\\b.sql", encoding: "utf16Le", lineEnding: "crlf" };
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  restore(tabs, parseSession(json)!, known);
  expect(ipc.allowDroppedPaths).toHaveBeenCalledTimes(1);
  expect(ipc.allowDroppedPaths).toHaveBeenCalledWith(["/home/u/a.sql", "C:\\sql\\b.sql"]);
});

/** A saved session with file tabs on `paths`, each with unsaved text and not dirty. */
async function sessionWithFiles(paths: string[]): Promise<string> {
  for (const path of paths) {
    const id = tabs.open({ kind: "query", sql: `SELECT '${path}'` });
    tabs.list.find((x) => x.id === id)!.file = { path, encoding: "utf16Le", lineEnding: "crlf" };
  }
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  return json;
}

test("a restored file tab whose file is gone keeps its text, loses the file and says so", async () => {
  const json = await sessionWithFiles(["/sql/here.sql", "/sql/gone.sql"]);
  const order: string[] = [];
  vi.mocked(ipc.allowDroppedPaths).mockImplementation(async () => void order.push("allow"));
  vi.mocked(ipc.sqlFilesExist).mockImplementation(async (paths) => {
    order.push("exist");
    return paths.map((p) => p !== "/sql/gone.sql");
  });
  restore(tabs, parseSession(json)!, known);
  await flush();
  expect(order).toEqual(["allow", "exist"]);
  expect(ipc.sqlFilesExist).toHaveBeenCalledWith(["/sql/here.sql", "/sql/gone.sql"]);
  const [here, gone] = tabs.list;
  expect(here.file?.path).toBe("/sql/here.sql");
  expect(here.pane!.notice).toBeNull();
  expect(gone.file).toBeUndefined();
  expect(gone.pane!.text()).toBe("SELECT '/sql/gone.sql'");
  expect(gone.pane!.notice).toBe(t("tabs.restoredWithoutFile", { path: "/sql/gone.sql" }));
  // Its text is only in the session now.
  expect(gone.dirty).toBe(true);
  expect(snapshot(tabs).tabs[1]).not.toHaveProperty("file");
});

test("a restored file tab that lost both its connection and its file keeps both notices", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c1", database: "Sales", sql: "SELECT 1" });
  await flush();
  tabs.list.find((x) => x.id === id)!.file = { path: "/gone.sql", encoding: "utf8", lineEnding: "lf" };
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  vi.mocked(ipc.sqlFilesExist).mockResolvedValue([false]);
  restore(tabs, parseSession(json)!, { connectionIds: new Set() });
  await flush();
  const notice = tabs.list[0].pane!.notice!;
  expect(notice).toContain(t("tabs.restoredWithoutConnection"));
  expect(notice).toContain(t("tabs.restoredWithoutFile", { path: "/gone.sql" }));
});

test("when the restored paths cannot be allowed, file tabs keep their file and nothing is checked", async () => {
  const json = await sessionWithFiles(["/sql/a.sql"]);
  vi.mocked(ipc.allowDroppedPaths).mockRejectedValue("internal error");
  restore(tabs, parseSession(json)!, known);
  await flush();
  expect(ipc.sqlFilesExist).not.toHaveBeenCalled();
  expect(tabs.list[0].file?.path).toBe("/sql/a.sql");
  expect(tabs.list[0].pane!.notice).toBeNull();
});

test("a restored file tab keeps its file when the check fails or cannot tell", async () => {
  const json = await sessionWithFiles(["/sql/locked.sql", "/sql/gone.sql"]);
  vi.mocked(ipc.sqlFilesExist).mockRejectedValue("internal error");
  restore(tabs, parseSession(json)!, known);
  await flush();
  expect(tabs.list.map((x) => x.file?.path)).toEqual(["/sql/locked.sql", "/sql/gone.sql"]);
  expect(tabs.list.map((x) => x.pane!.notice)).toEqual([null, null]);
  await tabs.reset();

  // `null`: that file could not be checked (no permission); only the missing one loses its file.
  vi.mocked(ipc.sqlFilesExist).mockResolvedValue([null, false]);
  restore(tabs, parseSession(json)!, known);
  await flush();
  const [locked, gone] = tabs.list;
  expect(locked.file?.path).toBe("/sql/locked.sql");
  expect(locked.pane!.notice).toBeNull();
  expect(locked.dirty).toBe(false);
  expect(gone.file).toBeUndefined();
});

test("the file check of a restore leaves alone a tab closed or replaced meanwhile", async () => {
  const json = await sessionWithFiles(["/sql/gone.sql"]);
  let answer!: (v: boolean[]) => void;
  vi.mocked(ipc.sqlFilesExist).mockReturnValue(new Promise((r) => (answer = r)));
  restore(tabs, parseSession(json)!, known);
  await flush();
  await tabs.reset();
  const id = tabs.open({ kind: "query" });
  const fresh = tabs.list.find((x) => x.id === id)!;
  fresh.file = { path: "/sql/gone.sql", encoding: "utf8", lineEnding: "lf" };
  answer([false]);
  await flush();
  expect(fresh.file?.path).toBe("/sql/gone.sql");
  expect(fresh.pane!.notice).toBeNull();
});

test("restore without file tabs allows nothing", async () => {
  tabs.open({ kind: "query" });
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  restore(tabs, parseSession(json)!, known);
  expect(ipc.allowDroppedPaths).not.toHaveBeenCalled();
});

test("parseSession returns null for garbage, another version, null or a broken structure", async () => {
  tabs.open({ kind: "query" });
  const good = snapshot(tabs);
  expect(parseSession(JSON.stringify(good))).toEqual(good);
  expect(parseSession(null)).toBeNull();
  expect(parseSession("not json {")).toBeNull();
  expect(parseSession("42")).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, version: 2 }))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, activeGroupId: "other" }))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, layout: { kind: "split" } }))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, groups: {} }))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, tabs: "x" }))).toBeNull();
});

/** A one-group session holding `tabs` in that order, showing `activeId`. */
function oneGroup(list: unknown[], activeId: string | null): string {
  const ids = list.map((x) => (x as { id: string }).id);
  return JSON.stringify({
    version: 1,
    layout: { kind: "group", id: "g1" },
    activeGroupId: "g1",
    groups: { g1: { tabIds: ids, activeId } },
    tabs: list,
  });
}
const queryTab = (id: string, text = `SELECT '${id}'`) => ({
  id,
  kind: "query",
  title: id,
  connectionId: null,
  database: null,
  text,
  dirty: false,
});

test("parseSession drops an invalid tab alone and keeps the others", async () => {
  const broken = { id: "t2", kind: "query" };
  const s = parseSession(oneGroup([queryTab("t1"), broken, queryTab("t3")], "t1"))!;
  expect(s).not.toBeNull();
  expect(s.tabs.map((x) => x.id)).toEqual(["t1", "t3"]);
  expect(s.groups.g1).toEqual({ tabIds: ["t1", "t3"], activeId: "t1" });

  // An unknown kind (from a newer version) is dropped too.
  const future = { id: "t4", kind: "chart", connectionId: "c1" };
  expect(parseSession(oneGroup([future, queryTab("t5")], "t5"))!.groups.g1).toEqual({ tabIds: ["t5"], activeId: "t5" });
});

test("parseSession shows a neighbour when the active tab was the invalid one", async () => {
  const broken = { id: "t2", kind: "query", text: 42 };
  const s = parseSession(oneGroup([queryTab("t1"), broken, queryTab("t3")], "t2"))!;
  expect(s.groups.g1).toEqual({ tabIds: ["t1", "t3"], activeId: "t1" });
  const first = parseSession(oneGroup([broken, queryTab("t3")], "t2"))!;
  expect(first.groups.g1).toEqual({ tabIds: ["t3"], activeId: "t3" });
  const none = parseSession(oneGroup([broken], "t2"))!;
  expect(none.tabs).toEqual([]);
  expect(none.groups.g1).toEqual({ tabIds: [], activeId: null });
});

test("parseSession tolerates group lists that do not match the tabs, never losing a valid tab", async () => {
  const json = JSON.stringify({
    version: 1,
    layout: { kind: "split", dir: "row", sizes: [1, 1], children: [{ kind: "group", id: "g1" }, { kind: "group", id: "g2" }] },
    activeGroupId: "g2",
    groups: {
      // "nope" is no tab; "t1" sits in two groups; "t3" in none; a duplicate tab id.
      g1: { tabIds: ["nope", "t1", 7], activeId: "nope" },
      g2: { tabIds: ["t1", "t2"], activeId: "t1" },
    },
    tabs: [queryTab("t1"), queryTab("t2"), queryTab("t3"), queryTab("t1", "SELECT twin")],
  });
  const s = parseSession(json)!;
  expect(s.tabs.map((x) => [x.id, (x as { text: string }).text])).toEqual([
    ["t1", "SELECT 't1'"],
    ["t2", "SELECT 't2'"],
    ["t3", "SELECT 't3'"],
  ]);
  expect(s.groups.g1).toEqual({ tabIds: ["t1"], activeId: "t1" });
  // An unplaced tab joins the active group.
  expect(s.groups.g2).toEqual({ tabIds: ["t2", "t3"], activeId: "t2" });

  restore(tabs, s, known);
  expect(tabs.list.map((x) => x.pane!.text())).toEqual(["SELECT 't1'", "SELECT 't2'", "SELECT 't3'"]);
});

test("an autosave held until an edit saves nothing, even on close, until the user types", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const id = tabs.open({ kind: "query" });
  const save = vi.fn(async (_json: string) => {});
  const stop = tabs.startAutosave(save, { holdUntilEdit: true });
  flushSync();
  await Promise.resolve();

  tabs.open({ kind: "query", sql: "SELECT opened" });
  tabs.splitActive("row");
  flushSync();
  vi.advanceTimersByTime(10_000);
  await closeRequested!();
  expect(save).not.toHaveBeenCalled();

  tabs.list.find((x) => x.id === id)!.pane!.edits++;
  flushSync();
  vi.advanceTimersByTime(1000);
  expect(save).toHaveBeenCalledTimes(1);
  expect(parseSession(save.mock.calls[0][0])!.tabs.map((x) => x.id)).toContain(id);
  // From then on it saves as usual.
  tabs.open({ kind: "query" });
  flushSync();
  await closeRequested!();
  expect(save).toHaveBeenCalledTimes(2);
  stop();
});

test("a tab whose connection no longer exists restores unbound, with its text and a notice", async () => {
  tabs.open({ kind: "query", connectionId: "c1", database: "Sales", sql: "SELECT keep" });
  tabs.open({ kind: "query", connectionId: "c1", sql: "SELECT other" });
  await flush();
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();

  restore(tabs, parseSession(json)!, { connectionIds: new Set(["c2"]) });

  expect(tabs.list).toHaveLength(2);
  const pane = tabs.list[0].pane!;
  expect(pane.text()).toBe("SELECT keep");
  expect(tabs.binding(tabs.list[0])).toEqual({ connectionId: null, database: null });
  expect(pane.notice).toBe(t("tabs.restoredWithoutConnection"));
  expect(tabs.list[1].pane!.text()).toBe("SELECT other");
});

test("restoring connects nothing", async () => {
  await buildWorkspace();
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  vi.mocked(ipc.openSession).mockClear();

  restore(tabs, parseSession(json)!, known);
  cmp = mount(Workspace, { target: document.body });
  flushSync();
  await flush();

  expect(ipc.openSession).not.toHaveBeenCalled();
  expect(ipc.connectExplorer).not.toHaveBeenCalled();
});

test("the next untitled tab does not reuse a restored title", async () => {
  tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  tabs.close(tabs.list[0].id);
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  restore(tabs, parseSession(json)!, known);
  tabs.open({ kind: "query" });
  expect(tabs.list.map((x) => x.title)).toEqual(["untitled-2", "untitled-3", "untitled-4"]);
});

test("typing in an editor saves once, 1 s after the last change", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const id = tabs.open({ kind: "query" });
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  cmp = mount(SqlEditor, { target: document.body, props: { pane, onrun: () => {}, oncursor: () => {} } });
  flushSync();
  const save = vi.fn(async (_json: string) => {});
  const stop = tabs.startAutosave(save);
  flushSync();
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;

  for (const ch of "SEL") {
    view.dispatch({ changes: { from: view.state.doc.length, insert: ch } });
    flushSync();
    vi.advanceTimersByTime(500);
  }
  expect(save).not.toHaveBeenCalled();
  vi.advanceTimersByTime(499);
  expect(save).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  expect(save).toHaveBeenCalledTimes(1);
  const saved = parseSession(save.mock.calls[0][0])!;
  expect(saved.tabs[0]).toMatchObject({ id, kind: "query", text: "SEL" });
  vi.advanceTimersByTime(5000);
  expect(save).toHaveBeenCalledTimes(1);
  stop();
});

test("closing the window saves right away; stopping the autosave cancels it", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const save = vi.fn(async (_json: string) => {});
  const stop = tabs.startAutosave(save);
  await Promise.resolve();
  tabs.open({ kind: "query", sql: "SELECT bye" });
  flushSync();
  await closeRequested!();
  expect(save).toHaveBeenCalledTimes(1);
  expect(parseSession(save.mock.calls[0][0])!.tabs[0]).toMatchObject({ text: "SELECT bye" });

  tabs.open({ kind: "query" });
  flushSync();
  stop();
  vi.advanceTimersByTime(2000);
  expect(save).toHaveBeenCalledTimes(1);
  expect(unlisten).toHaveBeenCalled();
});

test("Run on a restored tab opens its session and then runs", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c1", database: "Sales", sql: "SELECT 1" });
  await flush();
  const json = JSON.stringify(snapshot(tabs));
  await tabs.reset();
  vi.mocked(ipc.openSession).mockClear();
  restore(tabs, parseSession(json)!, known);
  cmp = mount(Workspace, { target: document.body });
  flushSync();

  const run = [...document.querySelectorAll<HTMLButtonElement>("button.run")][0];
  expect(run.disabled).toBe(false);
  run.click();
  await flush();

  expect(ipc.openSession).toHaveBeenCalledWith(id, "c1", "Sales", undefined);
  expect(ipc.runQuery).toHaveBeenCalledTimes(1);
  expect(vi.mocked(ipc.runQuery).mock.calls[0][1]).toBe("SELECT 1");
  expect(vi.mocked(ipc.openSession).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.runQuery).mock.invocationCallOrder[0]);
});

test("Run on a restored tab without a saved password asks for it, then runs", async () => {
  const id = tabs.open({ kind: "query", sql: "SELECT 2" });
  const json = JSON.stringify(snapshot(tabs)).replace('"connectionId":null', '"connectionId":"c2"');
  await tabs.reset();
  restore(tabs, parseSession(json)!, known);

  tabs.run(id, "SELECT 2", 0);
  await flush();
  expect(ipc.openSession).not.toHaveBeenCalled();
  expect(tabs.passwordRequest?.tabId).toBe(id);

  await tabs.submitPassword("pw");
  await flush();
  expect(ipc.openSession).toHaveBeenCalledWith(id, "c2", undefined, "pw");
  expect(vi.mocked(ipc.runQuery).mock.calls[0][1]).toBe("SELECT 2");
});

test("an edited definition keeps its text across a restore and the reload", async () => {
  vi.mocked(ipc.objectDefinition).mockResolvedValue("CREATE VIEW v AS SELECT 1");
  const id = tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "v" });
  await flush();
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  expect(pane.text()).toBe("CREATE VIEW v AS SELECT 1");
  pane.editorState = { doc: "CREATE VIEW v AS SELECT 2 -- mine" };
  const saved = snapshot(tabs);
  expect(saved.tabs[0]).toMatchObject({ kind: "definition", text: "CREATE VIEW v AS SELECT 2 -- mine" });
  await tabs.reset();

  vi.mocked(ipc.objectDefinition).mockResolvedValue("CREATE VIEW v AS SELECT 3");
  restore(tabs, parseSession(JSON.stringify(saved))!, known);
  await flush();

  expect(ipc.objectDefinition).toHaveBeenCalledTimes(2);
  expect(tabs.list[0].pane!.text()).toBe("CREATE VIEW v AS SELECT 2 -- mine");
});

test("an unedited definition saves no text and reloads fresh text", async () => {
  vi.mocked(ipc.objectDefinition).mockResolvedValue("CREATE VIEW v AS SELECT 1");
  tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "v" });
  await flush();
  const saved = snapshot(tabs);
  expect(saved.tabs[0]).not.toHaveProperty("text");
  await tabs.reset();

  vi.mocked(ipc.objectDefinition).mockResolvedValue("CREATE VIEW v AS SELECT 3");
  restore(tabs, parseSession(JSON.stringify(saved))!, known);
  await flush();

  expect(tabs.list[0].pane!.text()).toBe("CREATE VIEW v AS SELECT 3");
});

test("closing the window is never blocked by a snapshot that throws or a save that fails or hangs", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const id = tabs.open({ kind: "query" });
  const pane = tabs.list.find((x) => x.id === id)!.pane!;

  pane.liveDoc = () => {
    throw new Error("editor gone");
  };
  const save = vi.fn(async (_json: string) => {});
  let stop = tabs.startAutosave(save);
  await Promise.resolve();
  await expect(closeRequested!()).resolves.toBeUndefined();
  expect(save).not.toHaveBeenCalled();
  stop();

  pane.liveDoc = null;
  const failing = vi.fn(async (_json: string) => {
    throw "disk full";
  });
  stop = tabs.startAutosave(failing);
  await Promise.resolve();
  await expect(closeRequested!()).resolves.toBeUndefined();
  expect(failing).toHaveBeenCalledTimes(1);
  stop();

  const hanging = vi.fn((_json: string) => new Promise<void>(() => {}));
  stop = tabs.startAutosave(hanging);
  await Promise.resolve();
  let closed = false;
  void closeRequested!().then(() => (closed = true));
  await vi.advanceTimersByTimeAsync(1999);
  expect(closed).toBe(false);
  await vi.advanceTimersByTimeAsync(1);
  expect(closed).toBe(true);
  stop();
});

test("typing without a pause still saves every 5 s", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const id = tabs.open({ kind: "query" });
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  const save = vi.fn(async (_json: string) => {});
  const stop = tabs.startAutosave(save);
  flushSync();
  const type = async (times: number) => {
    for (let i = 0; i < times; i++) {
      pane.edits++;
      flushSync();
      await vi.advanceTimersByTimeAsync(500);
    }
  };
  await type(9); // t = 4.5 s
  expect(save).not.toHaveBeenCalled();
  await type(1); // t = 5 s
  expect(save).toHaveBeenCalledTimes(1);
  await type(9); // t = 9.5 s
  expect(save).toHaveBeenCalledTimes(1);
  await type(1); // t = 10 s: 5 s after the first unsaved change
  expect(save).toHaveBeenCalledTimes(2);
  // That save already holds the last change: nothing is pending.
  await vi.advanceTimersByTimeAsync(1000);
  expect(save).toHaveBeenCalledTimes(2);
  stop();
});

test("the close save waits for a save already in flight, so it lands last", async () => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const id = tabs.open({ kind: "query", sql: "SELECT old" });
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  const done: (() => void)[] = [];
  const save = vi.fn((_json: string) => new Promise<void>((r) => done.push(r)));
  const stop = tabs.startAutosave(save);
  await Promise.resolve();
  pane.edits++;
  flushSync();
  vi.advanceTimersByTime(1000);
  expect(save).toHaveBeenCalledTimes(1);

  pane.initialDoc = "SELECT new";
  let closed = false;
  void closeRequested!().then(() => (closed = true));
  await vi.advanceTimersByTimeAsync(0);
  expect(save).toHaveBeenCalledTimes(1);

  done[0]();
  await vi.advanceTimersByTimeAsync(0);
  expect(save).toHaveBeenCalledTimes(2);
  expect(parseSession(save.mock.calls[1][0])!.tabs[0]).toMatchObject({ text: "SELECT new" });
  expect(closed).toBe(false);
  done[1]();
  await vi.advanceTimersByTimeAsync(0);
  expect(closed).toBe(true);
  stop();
});

test("a restored definition tab offers Retry only while its connection exists", async () => {
  tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "v" });
  await flush();
  const json = JSON.stringify(snapshot(tabs));
  const retry = () => [...document.querySelectorAll(".notice button")].filter((b) => b.textContent === t("tabs.retry"));

  await tabs.reset();
  restore(tabs, parseSession(json)!, known);
  cmp = mount(Workspace, { target: document.body });
  flushSync();
  await flush();
  flushSync();
  expect(retry()).toHaveLength(1);
  unmount(cmp);

  await tabs.reset();
  connections.list = [nopw];
  restore(tabs, parseSession(json)!, { connectionIds: new Set(["c2"]) });
  cmp = mount(Workspace, { target: document.body });
  flushSync();
  expect(document.querySelector(".notice")?.textContent).toContain(t("tabs.restoredWithoutConnection"));
  expect(retry()).toHaveLength(0);
});
