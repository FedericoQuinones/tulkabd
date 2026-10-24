import { beforeEach, expect, test, vi } from "vitest";
import type { ConnectionConfig, RunSummary, UiEvent } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  useDatabase: vi.fn(),
  runQuery: vi.fn(),
  cancelQuery: vi.fn(),
}));

import * as ipc from "../ipc";
import { PaneState } from "./pane.svelte";

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
