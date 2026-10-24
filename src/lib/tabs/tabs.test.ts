import { beforeEach, expect, test, vi } from "vitest";
import type { ConnectionConfig, RunSummary } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  useDatabase: vi.fn(),
  runQuery: vi.fn(),
  cancelQuery: vi.fn(),
  objectDefinition: vi.fn(),
  tableStructure: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import { groups } from "../workspace/layout";
import { tabs } from "./tabs.svelte";

const conn = { id: "c1", name: "Local", passwordSaved: true, environment: "local" } as ConnectionConfig;
const nopw = { id: "c2", name: "Prod", passwordSaved: false, environment: "production" } as ConnectionConfig;
const info = { server: { version: "2022", user: "sa", singleDatabase: false }, databases: ["master", "Sales"], currentDatabase: "Sales" };
const ok: RunSummary = { elapsedMs: 1, finishedAt: "", outcome: "ok", findings: [] };
const flush = () => new Promise((r) => setTimeout(r, 0));

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.openSession).mockResolvedValue(info);
  vi.mocked(ipc.runQuery).mockResolvedValue(ok);
  connections.list = [conn, nopw];
  await tabs.reset();
});

test("open numbers untitled tabs and activates the new one", () => {
  const a = tabs.open({ kind: "query" });
  const b = tabs.open({ kind: "query" });
  expect(tabs.list.map((x) => x.title)).toEqual(["untitled-1", "untitled-2"]);
  expect(tabs.activeId).toBe(b);
  expect(a).not.toBe(b);
});

test("closing the active tab activates the one on its right, or the left if last", () => {
  const [a, b, c] = [tabs.open({ kind: "query" }), tabs.open({ kind: "query" }), tabs.open({ kind: "query" })];
  tabs.activate(b);
  tabs.close(b);
  expect(tabs.activeId).toBe(c);
  tabs.close(c);
  expect(tabs.activeId).toBe(a);
  tabs.close(a);
  expect(tabs.activeId).toBeNull();
  expect(tabs.list).toEqual([]);
});

test("closing an inactive tab keeps the active one", () => {
  const a = tabs.open({ kind: "query" });
  const b = tabs.open({ kind: "query" });
  tabs.close(a);
  expect(tabs.activeId).toBe(b);
});

test("close calls closeSession with the tab id", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c1" });
  await flush();
  tabs.close(id);
  await flush();
  expect(ipc.closeSession).toHaveBeenCalledWith(id);
});

test("run:true runs the sql once after the session opens", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c1", database: "Sales", sql: "SELECT TOP 10 * FROM t", run: true });
  await flush();
  expect(ipc.openSession).toHaveBeenCalledWith(id, "c1", "Sales", undefined);
  expect(ipc.runQuery).toHaveBeenCalledTimes(1);
  expect(vi.mocked(ipc.runQuery).mock.calls[0][1]).toBe("SELECT TOP 10 * FROM t");
  expect(vi.mocked(ipc.openSession).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.runQuery).mock.invocationCallOrder[0]);
});

test("a query without run does not execute", async () => {
  tabs.open({ kind: "query", connectionId: "c1", sql: "SELECT 1" });
  await flush();
  expect(ipc.runQuery).not.toHaveBeenCalled();
});

test("structure tabs open no session", async () => {
  tabs.open({ kind: "structure", connectionId: "c1", database: "Sales", schema: "dbo", name: "Pedidos" });
  await flush();
  expect(ipc.openSession).not.toHaveBeenCalled();
  expect(tabs.list[0].title).toBe("Structure · Pedidos");
  expect(tabs.list[0].kind).toBe("structure");
});

test("definition opens a query tab with the CREATE text and does not run it", async () => {
  vi.mocked(ipc.objectDefinition).mockResolvedValue("CREATE VIEW v AS SELECT 1");
  tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "v" });
  await flush();
  const tab = tabs.list[0];
  expect(tab.kind).toBe("query");
  expect(tab.title).toBe("v");
  expect(tab.pane!.initialDoc).toBe("CREATE VIEW v AS SELECT 1");
  expect(ipc.runQuery).not.toHaveBeenCalled();
});

test("a definition that needs the Explorer shows an inline notice and retries", async () => {
  vi.mocked(ipc.objectDefinition).mockRejectedValueOnce("not_connected").mockResolvedValueOnce("CREATE VIEW v");
  const id = tabs.open({ kind: "definition", connectionId: "c1", database: "Sales", schema: "dbo", name: "v" });
  await flush();
  expect(tabs.list[0].pane!.notice).toBe("Connect this server in the Explorer to load its definition.");
  await tabs.retry(id);
  expect(tabs.list[0].pane!.notice).toBeNull();
  expect(tabs.list[0].pane!.initialDoc).toBe("CREATE VIEW v");
});

test("newTab binds to the active tab's connection and database", async () => {
  tabs.open({ kind: "query", connectionId: "c1", database: "Sales" });
  await flush();
  const id = tabs.newTab()!;
  await flush();
  expect(ipc.openSession).toHaveBeenLastCalledWith(id, "c1", "Sales", undefined);
});

test("newTab with no tabs opens an unbound tab", () => {
  expect(tabs.newTab()).not.toBeNull();
  expect(ipc.openSession).not.toHaveBeenCalled();
});

test("a connection without a saved password asks for it before opening", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c2" });
  await flush();
  expect(ipc.openSession).not.toHaveBeenCalled();
  expect(tabs.passwordRequest?.tabId).toBe(id);
  await tabs.submitPassword("pw");
  expect(ipc.openSession).toHaveBeenCalledWith(id, "c2", undefined, "pw");
  expect(tabs.passwordRequest).toBeNull();
});

test("password_required from the backend raises the prompt", async () => {
  vi.mocked(ipc.openSession).mockRejectedValueOnce("password_required");
  tabs.open({ kind: "query", connectionId: "c1" });
  await flush();
  expect(tabs.passwordRequest?.conn.id).toBe("c1");
});

test("closing a tab while it connects does not leak the session", async () => {
  let resolve!: (v: typeof info) => void;
  vi.mocked(ipc.openSession).mockReturnValueOnce(new Promise((r) => (resolve = r)));
  const id = tabs.open({ kind: "query", connectionId: "c1" });
  tabs.close(id);
  resolve(info);
  await flush();
  expect(ipc.closeSession).toHaveBeenCalledWith(id);
});

test("cancelling the password prompt drops the pending run", async () => {
  const id = tabs.open({ kind: "query", connectionId: "c2", sql: "SELECT 1", run: true });
  await flush();
  tabs.cancelPassword();
  await tabs.connectTab(id, nopw, "pw");
  await flush();
  expect(ipc.runQuery).not.toHaveBeenCalled();
});

test("a failed connect leaves its error on that tab only, and closing the tab drops it", async () => {
  vi.mocked(ipc.openSession).mockRejectedValueOnce("boom");
  const a = tabs.open({ kind: "query", connectionId: "c1" });
  await flush();
  const b = tabs.open({ kind: "query" });
  expect(tabs.list.find((x) => x.id === a)!.connectError).toContain("boom");
  expect(tabs.list.find((x) => x.id === b)!.connectError).toBeNull();
  tabs.close(a);
  expect(tabs.list.some((x) => x.connectError)).toBe(false);
});

test("two tabs asking for a password queue in order and cancelling the first shows the second", async () => {
  const a = tabs.open({ kind: "query", connectionId: "c2" });
  const b = tabs.open({ kind: "query", connectionId: "c2" });
  await flush();
  expect(tabs.passwordRequests.map((r) => r.tabId)).toEqual([a, b]);
  expect(tabs.passwordRequest?.tabId).toBe(a);
  tabs.cancelPassword();
  expect(tabs.passwordRequest?.tabId).toBe(b);
  await tabs.submitPassword("pw");
  expect(ipc.openSession).toHaveBeenCalledWith(b, "c2", undefined, "pw");
  expect(tabs.passwordRequest).toBeNull();
});

test("unbindConnection leaves the tabs of that connection without a session and with a notice", async () => {
  const a = tabs.open({ kind: "query", connectionId: "c1" });
  const b = tabs.open({ kind: "query" });
  await flush();
  tabs.unbindConnection("c1");
  const pa = tabs.list.find((x) => x.id === a)!.pane!;
  expect(pa.connectionId).toBeNull();
  expect(pa.session).toBeNull();
  expect(pa.notice).toBe("This connection was deleted.");
  expect(tabs.list.find((x) => x.id === b)!.pane!.notice).toBeNull();
  expect(ipc.closeSession).toHaveBeenCalledWith(a);
});

test("splitActive opens a tab bound like the active one in a new active group beside it", async () => {
  const a = tabs.open({ kind: "query", connectionId: "c1", database: "Sales" });
  await flush();
  const first = tabs.activeGroupId;
  expect(tabs.splitActive("row")).toBe(true);
  await flush();
  const second = tabs.activeGroupId;
  expect(second).not.toBe(first);
  expect(tabs.layout).toMatchObject({ kind: "split", dir: "row" });
  expect(groups(tabs.layout)).toEqual([first, second]);
  const b = tabs.active!;
  expect(b.id).not.toBe(a);
  expect(tabs.groupOf(b.id)).toBe(second);
  expect(tabs.tabsIn(second).map((x) => x.id)).toEqual([b.id]);
  expect(tabs.tabsIn(first).map((x) => x.id)).toEqual([a]);
  expect(tabs.activeIn(first)?.id).toBe(a);
  expect(ipc.openSession).toHaveBeenLastCalledWith(b.id, "c1", "Sales", undefined);
});

test("splitActive with four groups returns false and changes nothing", () => {
  tabs.open({ kind: "query" });
  expect(tabs.splitActive("row")).toBe(true);
  expect(tabs.splitActive("column")).toBe(true);
  expect(tabs.splitActive("row")).toBe(true);
  const layout = tabs.layout;
  const active = tabs.activeGroupId;
  const count = tabs.list.length;
  expect(groups(layout)).toHaveLength(4);
  expect(tabs.splitActive("column")).toBe(false);
  expect(tabs.layout).toBe(layout);
  expect(tabs.activeGroupId).toBe(active);
  expect(tabs.list).toHaveLength(count);
});

test("splitActive on a group with no tabs opens a tab in it instead of leaving it empty beside a new one", () => {
  const first = tabs.activeGroupId;
  expect(tabs.active).toBeNull();
  expect(tabs.splitActive("row")).toBe(true);
  expect(tabs.layout).toEqual({ kind: "group", id: first });
  expect(tabs.tabGroups.map((g) => g.id)).toEqual([first]);
  expect(tabs.activeGroupId).toBe(first);
  expect(tabs.tabsIn(first).map((x) => x.id)).toEqual([tabs.activeId]);
});

test("closing the last tab of a group removes the group and activates its neighbour", () => {
  const a = tabs.open({ kind: "query" });
  const first = tabs.activeGroupId;
  tabs.splitActive("row");
  tabs.close(tabs.activeId!);
  expect(tabs.layout).toEqual({ kind: "group", id: first });
  expect(tabs.activeGroupId).toBe(first);
  expect(tabs.activeId).toBe(a);
});

test("closing the last tab of the only group keeps the group", () => {
  const first = tabs.activeGroupId;
  tabs.close(tabs.open({ kind: "query" }));
  expect(tabs.layout).toEqual({ kind: "group", id: first });
  expect(tabs.active).toBeNull();
});

test("closing a tab in another group keeps the active group", () => {
  const a = tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  const first = tabs.activeGroupId;
  tabs.splitActive("column");
  const second = tabs.activeGroupId;
  tabs.close(a);
  expect(tabs.activeGroupId).toBe(second);
  expect(groups(tabs.layout)).toEqual([first, second]);
});

test("moveTab activates the tab in its new group and removes the group it leaves empty", () => {
  const a = tabs.open({ kind: "query" });
  const b = tabs.open({ kind: "query" });
  const first = tabs.activeGroupId;
  tabs.splitActive("row");
  const second = tabs.activeGroupId;
  const c = tabs.activeId!;

  tabs.moveTab(a, second, 0);
  expect(tabs.tabsIn(second).map((x) => x.id)).toEqual([a, c]);
  expect(tabs.activeIn(second)?.id).toBe(a);
  expect(tabs.activeGroupId).toBe(second);
  expect(tabs.groupOf(a)).toBe(second);
  expect(tabs.tabsIn(first).map((x) => x.id)).toEqual([b]);
  expect(tabs.activeIn(first)?.id).toBe(b);

  tabs.moveTab(b, second, 1);
  expect(tabs.layout).toEqual({ kind: "group", id: second });
  expect(tabs.tabsIn(second).map((x) => x.id)).toEqual([a, b, c]);
  expect(tabs.activeId).toBe(b);
});

test("moveTab within a group reorders it", () => {
  const a = tabs.open({ kind: "query" });
  const b = tabs.open({ kind: "query" });
  const c = tabs.open({ kind: "query" });
  tabs.moveTab(a, tabs.activeGroupId, 2);
  expect(tabs.tabsIn(tabs.activeGroupId).map((x) => x.id)).toEqual([b, c, a]);
  expect(tabs.activeId).toBe(a);
});

test("focusGroup and activate pick the group that active, open and newTab use", () => {
  const a = tabs.open({ kind: "query" });
  const first = tabs.activeGroupId;
  tabs.splitActive("row");
  const second = tabs.activeGroupId;
  const b = tabs.activeId!;

  tabs.focusGroup(first);
  expect(tabs.active?.id).toBe(a);
  const c = tabs.open({ kind: "query" });
  expect(tabs.groupOf(c)).toBe(first);

  tabs.activate(b);
  expect(tabs.activeGroupId).toBe(second);
  expect(tabs.activePane).toBe(tabs.list.find((x) => x.id === b)!.pane);
  expect(tabs.groupOf(tabs.newTab()!)).toBe(second);

  tabs.focusGroup("nope");
  expect(tabs.activeGroupId).toBe(second);
});
