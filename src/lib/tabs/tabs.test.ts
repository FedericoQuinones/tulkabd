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
  transactionCount: vi.fn(),
  endTransactions: vi.fn(),
}));

let closeRequested: ((event: { preventDefault: () => void }) => Promise<void>) | null = null;
const destroy = vi.fn(async () => {});
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: async (handler: typeof closeRequested) => {
      closeRequested = handler;
      return () => {};
    },
    destroy: () => destroy(),
  }),
}));

import * as ipc from "../ipc";
import { CHECK_TIMEOUT_MS, END_RETRIES, END_RETRY_MS } from "./tabs.svelte";
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
  vi.mocked(ipc.transactionCount).mockResolvedValue(0);
  vi.mocked(ipc.endTransactions).mockResolvedValue();
  vi.mocked(ipc.cancelQuery).mockResolvedValue();
  destroy.mockClear();
  closeRequested = null;
  connections.list = [conn, nopw];
  await tabs.reset();
  vi.mocked(ipc.closeSession).mockClear();
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

// ------------------------------------------------------------------ closing with a transaction

/** A tab with a live session on `conn`. */
async function liveTab(): Promise<string> {
  const id = tabs.open({ kind: "query", connectionId: "c1", database: "Sales" });
  await flush();
  return id;
}

test("requestClose without a transaction closes at once", async () => {
  const id = await liveTab();
  await tabs.requestClose(id);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
  expect(tabs.closePrompt).toBeNull();
});

test("requestClose with an open transaction waits for the answer", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(2);
  await tabs.requestClose(id);
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(tabs.closePrompt?.scope).toBe("tab");
  expect(tabs.closePrompt?.items).toEqual([{ tabId: id, connection: "Local", database: "Sales", state: "open", count: 2 }]);
  expect(ipc.endTransactions).not.toHaveBeenCalled();
  expect(ipc.closeSession).not.toHaveBeenCalled();
});

test("rollback ends then closes", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await tabs.requestClose(id);
  await tabs.answerClose(false);
  expect(ipc.endTransactions).toHaveBeenCalledWith(id, false);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
  expect(tabs.closePrompt).toBeNull();
  expect(vi.mocked(ipc.endTransactions).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.closeSession).mock.invocationCallOrder[0]);
});

test("a failed commit keeps the tab open with tx.failed", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  vi.mocked(ipc.endTransactions).mockRejectedValue("deadlock");
  await tabs.requestClose(id);
  await tabs.answerClose(true);
  expect(ipc.endTransactions).toHaveBeenCalledWith(id, true);
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(tabs.closePrompt?.error).toBe("Could not end the transaction: deadlock. The tab stays open.");
  expect(tabs.closePrompt?.busy).toBe(false);
  expect(ipc.closeSession).not.toHaveBeenCalled();
});

test("cancelling the prompt keeps the tab and its transaction", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await tabs.requestClose(id);
  tabs.cancelClose();
  expect(tabs.closePrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(ipc.endTransactions).not.toHaveBeenCalled();
});

test("a lost connection or a tab without a session closes without asking", async () => {
  const lost = await liveTab();
  vi.mocked(ipc.transactionCount).mockRejectedValue("disconnected");
  await tabs.requestClose(lost);
  expect(tabs.list.some((x) => x.id === lost)).toBe(false);
  const bare = tabs.open({ kind: "query" });
  vi.mocked(ipc.transactionCount).mockResolvedValue(0);
  await tabs.requestClose(bare);
  expect(tabs.list.some((x) => x.id === bare)).toBe(false);
  expect(tabs.closePrompt).toBeNull();
});

test("a running tab is listed as unknown and its query is cancelled before ending", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockRejectedValue("busy");
  await tabs.requestClose(id);
  expect(tabs.closePrompt?.items[0]).toMatchObject({ tabId: id, state: "unknown" });
  // The session is still held by the run for a moment after the cancel.
  vi.mocked(ipc.endTransactions).mockRejectedValueOnce("busy").mockResolvedValue();
  await tabs.answerClose(false);
  expect(ipc.cancelQuery).toHaveBeenCalledWith(id);
  expect(vi.mocked(ipc.cancelQuery).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(ipc.endTransactions).mock.invocationCallOrder[0]);
  expect(ipc.endTransactions).toHaveBeenCalledTimes(2);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

async function armWindowClose() {
  tabs.startAutosave(async () => {});
  await flush();
  expect(closeRequested).not.toBeNull();
}

test("window close with two open tabs asks once", async () => {
  const a = await liveTab();
  const b = await liveTab();
  const quiet = await liveTab();
  vi.mocked(ipc.transactionCount).mockImplementation(async (id) => (id === quiet ? 0 : 1));
  await armWindowClose();
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).toHaveBeenCalledTimes(1);
  expect(tabs.closePrompt?.scope).toBe("window");
  expect(tabs.closePrompt?.items.map((x) => x.tabId)).toEqual([a, b]);
  expect(destroy).not.toHaveBeenCalled();

  await tabs.answerClose(true);
  expect(ipc.endTransactions).toHaveBeenCalledWith(a, true);
  expect(ipc.endTransactions).toHaveBeenCalledWith(b, true);
  expect(ipc.endTransactions).toHaveBeenCalledTimes(2);
  expect(destroy).toHaveBeenCalledTimes(1);
});

test("Cancel keeps the window open", async () => {
  await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  tabs.cancelClose();
  expect(tabs.closePrompt).toBeNull();
  expect(destroy).not.toHaveBeenCalled();
  expect(ipc.endTransactions).not.toHaveBeenCalled();
});

test("a failed end keeps the window open and lists what is left", async () => {
  const a = await liveTab();
  const b = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  vi.mocked(ipc.endTransactions).mockImplementation(async (id) => {
    if (id === b) throw "locked";
  });
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  await tabs.answerClose(false);
  expect(destroy).not.toHaveBeenCalled();
  expect(tabs.closePrompt?.error).toContain("locked");
  expect(tabs.closePrompt?.items.map((x) => x.tabId)).toEqual([b]);
  expect(a).not.toBe(b);
});

test("window close with nothing open does not ask or prevent the close", async () => {
  await liveTab();
  await armWindowClose();
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).not.toHaveBeenCalled();
  expect(tabs.closePrompt).toBeNull();
});

test("the session is saved before the window close asks", async () => {
  await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  const save = vi.fn(async () => {});
  tabs.startAutosave(save);
  await flush();
  await closeRequested!({ preventDefault: vi.fn() });
  expect(save).toHaveBeenCalledTimes(1);
});

// ------------------------------------------------------------------ dead connections and running tabs

test("a count that never answers is unknown after the timeout", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockReturnValue(new Promise(() => {}));
  vi.useFakeTimers();
  try {
    const asking = tabs.requestClose(id);
    await vi.advanceTimersByTimeAsync(CHECK_TIMEOUT_MS + 1);
    await asking;
  } finally {
    vi.useRealTimers();
  }
  expect(tabs.closePrompt?.items[0]).toMatchObject({ tabId: id, state: "unknown" });
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
});

test("a rollback that finds the session busy closes the session instead", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await tabs.requestClose(id);
  vi.mocked(ipc.endTransactions).mockRejectedValue("busy");
  await tabs.answerClose(false);
  expect(ipc.closeSession).toHaveBeenCalledWith(id);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
  expect(tabs.closePrompt).toBeNull();
});

test("a rollback on a lost connection counts as done", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await tabs.requestClose(id);
  vi.mocked(ipc.endTransactions).mockRejectedValue("disconnected");
  await tabs.answerClose(false);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("a failed commit never closes the session, and the error has no raw sentinel", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await tabs.requestClose(id);
  vi.mocked(ipc.endTransactions).mockRejectedValue("busy");
  await tabs.answerClose(true);
  expect(ipc.closeSession).not.toHaveBeenCalled();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(tabs.closePrompt?.error).toBe("Could not end the transaction: the query is still running. The tab stays open.");
  vi.mocked(ipc.endTransactions).mockRejectedValue("disconnected");
  await tabs.answerClose(true);
  expect(tabs.closePrompt?.error).toBe("Could not end the transaction: the connection was lost. The tab stays open.");
});

test("a window rollback closes the session of a tab that cannot answer", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  vi.mocked(ipc.endTransactions).mockRejectedValue("busy");
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  await tabs.answerClose(false);
  expect(ipc.closeSession).toHaveBeenCalledWith(id);
  expect(destroy).toHaveBeenCalledTimes(1);
});

test("a running tab is retried END_RETRIES times, then rolled back by closing; a commit gives up", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockRejectedValue("busy");
  await tabs.requestClose(id);
  vi.mocked(ipc.endTransactions).mockRejectedValue("busy");
  vi.useFakeTimers();
  try {
    const answering = tabs.answerClose(false);
    await vi.advanceTimersByTimeAsync(END_RETRY_MS * (END_RETRIES + 1));
    await answering;
  } finally {
    vi.useRealTimers();
  }
  expect(ipc.endTransactions).toHaveBeenCalledTimes(END_RETRIES);
  expect(ipc.closeSession).toHaveBeenCalledWith(id);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("a running tab cannot be committed", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockRejectedValue("busy");
  await tabs.requestClose(id);
  await tabs.answerClose(true);
  expect(ipc.endTransactions).not.toHaveBeenCalled();
  expect(ipc.cancelQuery).not.toHaveBeenCalled();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
});

test("a window close while a tab prompt is open keeps the window", async () => {
  const id = await liveTab();
  vi.mocked(ipc.transactionCount).mockResolvedValue(1);
  await armWindowClose();
  await tabs.requestClose(id);
  expect(tabs.closePrompt?.scope).toBe("tab");
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).toHaveBeenCalledTimes(1);
  expect(destroy).not.toHaveBeenCalled();
});
