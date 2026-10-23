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

test("parseSession returns null for garbage, another version, null or a broken structure", async () => {
  tabs.open({ kind: "query" });
  const good = snapshot(tabs);
  expect(parseSession(JSON.stringify(good))).toEqual(good);
  expect(parseSession(null)).toBeNull();
  expect(parseSession("not json {")).toBeNull();
  expect(parseSession("42")).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, version: 2 }))).toBeNull();
  const gid = good.activeGroupId;
  const missingTab: SessionV1 = { ...good, groups: { [gid]: { tabIds: ["nope"], activeId: "nope" } } };
  expect(parseSession(JSON.stringify(missingTab))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, activeGroupId: "other" }))).toBeNull();
  expect(parseSession(JSON.stringify({ ...good, tabs: [{ id: good.tabs[0].id, kind: "query" }] }))).toBeNull();
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
