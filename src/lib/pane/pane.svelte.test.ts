import { beforeEach, expect, test, vi } from "vitest";
import type { ConnectionConfig, RunSummary, UiEvent } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  useDatabase: vi.fn(),
  runQuery: vi.fn(),
  cancelQuery: vi.fn(),
  setConnectionGuard: vi.fn(),
  setGuardOffForSession: vi.fn(),
  logCancelledRun: vi.fn(),
}));

import * as ipc from "../ipc";
import { PaneState } from "./pane.svelte";
import { connections } from "../connections/store.svelte";

const pane = new PaneState("t1");

const conn = { id: "c1", name: "Local", passwordSaved: true } as ConnectionConfig;
const info = { server: { version: "2022", user: "sa", singleDatabase: false }, databases: ["master", "Sales"], currentDatabase: "master" };
const ok: RunSummary = { elapsedMs: 1, finishedAt: "", outcome: "ok", findings: [] };

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => ((resolve = res), (reject = rej)));
  return { promise, resolve, reject };
}

/** Starts a run whose events and completion the test controls. */
function startRun() {
  const d = deferred<RunSummary>();
  let emit!: (e: UiEvent) => void;
  vi.mocked(ipc.runQuery).mockImplementationOnce((_t, _s, _c, onEvent) => {
    emit = onEvent;
    return d.promise;
  });
  const done = pane.run("SELECT 1", 0);
  return { d, done, emit: (e: UiEvent) => emit(e) };
}

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.openSession).mockResolvedValue(info);
  pane.status = "idle";
  await pane.open(conn);
});

test("disconnected event then a cancelled rejection stays disconnected", async () => {
  const r = startRun();
  r.emit({ kind: "disconnected", reason: "x" });
  expect(pane.status).toBe("disconnected");
  r.d.reject({ kind: "cancelled" });
  await r.done;
  expect(pane.status).toBe("disconnected");
});

test("disconnected event then an other rejection stays disconnected", async () => {
  const r = startRun();
  r.emit({ kind: "disconnected", reason: "x" });
  r.d.reject({ kind: "other", value: "boom" });
  await r.done;
  expect(pane.status).toBe("disconnected");
});

test("a stale run does not change state after Reconnect starts", async () => {
  const r = startRun();
  r.emit({ kind: "disconnected", reason: "x" });
  const reopen = deferred<typeof info>();
  vi.mocked(ipc.openSession).mockReturnValueOnce(reopen.promise);
  const opening = pane.open(conn, undefined, "master");
  expect(pane.status).toBe("connecting");
  r.d.resolve({ ...ok, outcome: "disconnected" });
  await r.done;
  expect(pane.status).toBe("connecting");
  reopen.resolve(info);
  await opening;
  expect(pane.status).toBe("idle");
  expect(pane.summary).toBeNull();
});

test("cancelled rejection returns to idle with an info line", async () => {
  const r = startRun();
  r.d.reject({ kind: "cancelled" });
  await r.done;
  expect(pane.status).toBe("idle");
  expect(pane.messages).toEqual([{ kind: "info", text: "Query cancelled.", line: null }]);
});

test("failed reconnect keeps disconnected and reports the error", async () => {
  const r = startRun();
  r.emit({ kind: "disconnected", reason: "x" });
  r.d.resolve({ ...ok, outcome: "disconnected" });
  await r.done;
  vi.mocked(ipc.openSession).mockRejectedValueOnce("no route");
  await pane.open(conn);
  expect(pane.status).toBe("disconnected");
  expect(pane.messages.at(-1)?.text).toContain("no route");
});

test("failed database switch keeps the database and reports the error", async () => {
  vi.mocked(ipc.useDatabase).mockRejectedValueOnce("denied");
  await pane.changeDatabase("Sales");
  expect(pane.database).toBe("master");
  expect(pane.switching).toBe(false);
  expect(pane.messages.at(-1)?.kind).toBe("error");
});

test("an open that finishes after unbind leaves the pane clean", async () => {
  const d = deferred<typeof info>();
  vi.mocked(ipc.openSession).mockReturnValueOnce(d.promise);
  pane.status = "idle";
  const opening = pane.open(conn);
  await new Promise((r) => setTimeout(r, 0));
  pane.unbind();
  d.resolve(info);
  await opening;
  expect(pane.connectionId).toBeNull();
  expect(pane.session).toBeNull();
  expect(pane.status).toBe("idle");
});

test("a stale open never closes the session of a newer open", async () => {
  const a = deferred<typeof info>();
  vi.mocked(ipc.openSession).mockReturnValueOnce(a.promise);
  pane.status = "idle";
  const openingA = pane.open(conn);
  await new Promise((r) => setTimeout(r, 0));
  vi.mocked(ipc.closeSession).mockClear();
  pane.unbind();
  expect(ipc.closeSession).toHaveBeenCalledTimes(1);
  await pane.open(conn);
  expect(pane.connectionId).toBe("c1");
  a.resolve(info);
  await openingA;
  expect(ipc.closeSession).toHaveBeenCalledTimes(1);
  expect(pane.session).not.toBeNull();
  expect(pane.connectionId).toBe("c1");
});

// Guard

const finding = (line: number) => ({ kind: "deleteWithoutWhere" as const, object: "dbo.T", statement: "DELETE FROM dbo.T", line });
const needs: RunSummary = { elapsedMs: 0, finishedAt: "", outcome: "needsConfirmation", findings: [finding(2)] };
const blockedSummary: RunSummary = { elapsedMs: 0, finishedAt: "", outcome: "blocked", findings: [finding(2)] };

test("needsConfirmation sets guardPrompt and leaves status idle", async () => {
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs);
  await pane.run("DELETE FROM dbo.T", 4);
  expect(pane.status).toBe("idle");
  expect(pane.guardPrompt).toEqual({ sql: "DELETE FROM dbo.T", lineOffset: 4, findings: needs.findings });
  expect(pane.blocked).toBeNull();
});

test("confirmGuard runs again with confirmed=true and the exact text that was held", async () => {
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs).mockResolvedValueOnce(ok);
  await pane.run("DELETE FROM dbo.T", 0);
  // The editor may change while the dialog is open; the held text is what goes out.
  pane.liveDoc = () => "DROP TABLE other";
  await pane.confirmGuard(false);
  pane.liveDoc = null;
  expect(vi.mocked(ipc.runQuery).mock.calls[1].slice(0, 3)).toEqual(["t1", "DELETE FROM dbo.T", true]);
  expect(pane.guardPrompt).toBeNull();
  expect(pane.status).toBe("idle");
  expect(pane.summary?.outcome).toBe("ok");
});

test("confirmGuard(true) turns the connection off before running and offers undo", async () => {
  connections.list = [{ ...conn, guard: "confirm" } as ConnectionConfig];
  const order: string[] = [];
  vi.mocked(ipc.setConnectionGuard).mockImplementation(async () => (order.push("guard"), "confirm"));
  vi.mocked(ipc.runQuery)
    .mockResolvedValueOnce(needs)
    .mockImplementationOnce(async () => (order.push("run"), ok));
  await pane.run("DELETE FROM dbo.T", 0);
  await pane.confirmGuard(true);
  expect(order).toEqual(["guard", "run"]);
  expect(ipc.setConnectionGuard).toHaveBeenCalledWith("c1", "off");
  expect(connections.list[0].guard).toBe("off");
  expect(pane.guardToast?.text).toBe("Guard turned off for Local.");
  await pane.undoGuardOff();
  expect(ipc.setConnectionGuard).toHaveBeenLastCalledWith("c1", "confirm");
  expect(connections.list[0].guard).toBe("confirm");
  expect(pane.guardToast).toBeNull();
});

test("confirmGuard(true) does not run when the level could not be saved", async () => {
  connections.list = [{ ...conn, guard: "confirm" } as ConnectionConfig];
  vi.mocked(ipc.setConnectionGuard).mockRejectedValue("read-only");
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs);
  await pane.run("DELETE FROM dbo.T", 0);
  await pane.confirmGuard(true);
  expect(ipc.runQuery).toHaveBeenCalledTimes(1);
  expect(pane.messages.at(-1)?.text).toContain("read-only");
  expect(pane.status).toBe("idle");
});

test("cancelGuard logs a cancelled run", async () => {
  vi.mocked(ipc.logCancelledRun).mockResolvedValue();
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs);
  await pane.run("DELETE FROM dbo.T", 0);
  pane.cancelGuard();
  expect(ipc.logCancelledRun).toHaveBeenCalledWith("t1", "DELETE FROM dbo.T");
  expect(pane.guardPrompt).toBeNull();
  expect(pane.status).toBe("idle");
});

test("blocked sets blocked and a new run clears it", async () => {
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(blockedSummary).mockResolvedValueOnce(ok);
  await pane.run("DELETE FROM dbo.T", 0);
  expect(pane.blocked).toHaveLength(1);
  expect(pane.guardPrompt).toBeNull();
  expect(pane.status).toBe("idle");
  await pane.run("SELECT 1", 0);
  expect(pane.blocked).toBeNull();
});

test("finding lines include the selection offset", async () => {
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(blockedSummary);
  await pane.run("DELETE FROM dbo.T", 10);
  expect(pane.blocked![0].line).toBe(12);
});

test("a double click on Run anyway with Don't ask again saves the level once and undo restores the original", async () => {
  connections.list = [{ ...conn, guard: "confirm" } as ConnectionConfig];
  vi.mocked(ipc.setConnectionGuard).mockResolvedValueOnce("confirm");
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs).mockResolvedValue(ok);
  await pane.run("DELETE FROM dbo.T", 0);
  await Promise.all([pane.confirmGuard(true), pane.confirmGuard(true)]);
  expect(ipc.setConnectionGuard).toHaveBeenCalledTimes(1);
  expect(ipc.runQuery).toHaveBeenCalledTimes(2);
  vi.mocked(ipc.setConnectionGuard).mockResolvedValueOnce("off");
  await pane.undoGuardOff();
  expect(ipc.setConnectionGuard).toHaveBeenLastCalledWith("c1", "confirm");
});

test("the undo toast is dropped when the level changed some other way", async () => {
  connections.list = [{ ...conn, guard: "confirm" } as ConnectionConfig];
  vi.mocked(ipc.setConnectionGuard).mockResolvedValueOnce("confirm");
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs).mockResolvedValue(ok);
  await pane.run("DELETE FROM dbo.T", 0);
  await pane.confirmGuard(true);
  connections.list = [{ ...conn, guard: "block" } as ConnectionConfig];
  await pane.undoGuardOff();
  expect(ipc.setConnectionGuard).toHaveBeenCalledTimes(1);
  expect(pane.guardToast).toBeNull();
});

test("the editor is focused as soon as the dialog closes, before the query ends", async () => {
  vi.mocked(ipc.runQuery).mockResolvedValueOnce(needs);
  await pane.run("DELETE FROM dbo.T", 0);
  const d = deferred<RunSummary>();
  vi.mocked(ipc.runQuery).mockReturnValueOnce(d.promise);
  const seen: boolean[] = [];
  const done = pane.confirmGuard(false, () => seen.push(true));
  expect(seen).toEqual([true]);
  d.resolve(ok);
  await done;
});

test("unbind and close drop the guard prompt, the blocked list and the toast", async () => {
  for (const drop of [() => pane.unbind(), () => pane.close()]) {
    pane.guardPrompt = { sql: "x", lineOffset: 0, findings: [] };
    pane.blocked = [];
    pane.guardToast = { text: "x", connectionId: "c1", previous: "confirm" };
    await drop();
    expect(pane.guardPrompt).toBeNull();
    expect(pane.blocked).toBeNull();
    expect(pane.guardToast).toBeNull();
    await pane.open(conn);
  }
});
