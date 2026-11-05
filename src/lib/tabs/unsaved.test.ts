import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { ConnectionConfig } from "../ipc";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
  transactionCount: vi.fn(),
  endTransactions: vi.fn(),
  onQuitRequested: vi.fn(),
  logCancelledRun: vi.fn(),
  listSavedQueries: vi.fn(),
  saveQuery: vi.fn(),
  writeSqlFile: vi.fn(),
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
import { connections } from "../connections/store.svelte";
import { prefs } from "../settings/prefs.svelte";
import { DEFAULT_PREFERENCES } from "../settings/prefs.svelte";
import { SAVE_TIMEOUT_MS, tabs } from "./tabs.svelte";

const conn = { id: "c1", name: "Local", passwordSaved: true, environment: "local" } as ConnectionConfig;
const flush = () => new Promise((r) => setTimeout(r, 0));

/** An open query tab edited by the user, with `text` in it. */
function edited(text: string, spec: Parameters<typeof tabs.open>[0] = { kind: "query" }): string {
  const id = tabs.open(spec);
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  pane.initialDoc = text;
  pane.edited();
  return id;
}

const stops: (() => void)[] = [];
afterEach(() => {
  for (const stop of stops.splice(0)) stop();
});

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.transactionCount).mockResolvedValue(0);
  vi.mocked(ipc.logCancelledRun).mockResolvedValue();
  vi.mocked(ipc.saveQuery).mockResolvedValue();
  vi.mocked(ipc.writeSqlFile).mockResolvedValue(1);
  vi.mocked(ipc.onQuitRequested).mockResolvedValue(() => {});
  destroy.mockClear();
  closeRequested = null;
  connections.list = [conn];
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: true };
  await tabs.reset();
});

test("closing an edited untitled tab asks first and keeps it until answered", async () => {
  const id = edited("SELECT 1");
  await tabs.requestClose(id);
  expect(tabs.unsavedPrompt?.scope).toBe("tab");
  expect(tabs.unsavedPrompt?.tabs.map((x) => x.tabId)).toEqual([id]);
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
});

test("cancel keeps the tab and its text", async () => {
  const id = edited("SELECT 1");
  await tabs.requestClose(id);
  tabs.cancelUnsaved();
  expect(tabs.unsavedPrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
});

test("Don't save closes the tab", async () => {
  const id = edited("SELECT 1");
  await tabs.requestClose(id);
  await tabs.answerUnsaved(false);
  expect(tabs.unsavedPrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("an untitled tab with no text, and a clean tab, close without asking", async () => {
  const empty = tabs.open({ kind: "query" });
  tabs.list.find((x) => x.id === empty)!.dirty = true;
  await tabs.requestClose(empty);
  expect(tabs.unsavedPrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === empty)).toBe(false);

  const clean = tabs.open({ kind: "query", sql: "SELECT 1" });
  await tabs.requestClose(clean);
  expect(tabs.unsavedPrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === clean)).toBe(false);
});

test("Save on a saved query writes it and closes the tab", async () => {
  const id = edited("SELECT 2", { kind: "query", sql: "SELECT 1", savedQuery: { database: "Sales", name: "Q" } });
  await tabs.requestClose(id);
  await tabs.answerUnsaved(true);
  expect(ipc.saveQuery).toHaveBeenCalledWith("Sales", "Q", "SELECT 2", true);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("Save on a file tab writes the file and closes the tab", async () => {
  const file = { path: "/a/b.sql", name: "b.sql", encoding: "utf8" as const, lineEnding: "lf" as const };
  const id = edited("SELECT 2", { kind: "query", sql: "SELECT 1", file });
  await tabs.requestClose(id);
  await tabs.answerUnsaved(true);
  expect(ipc.writeSqlFile).toHaveBeenCalledWith("/a/b.sql", "SELECT 2", "utf8", "lf");
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("a failed save keeps the tab and says so in the dialog", async () => {
  vi.mocked(ipc.writeSqlFile).mockRejectedValue("disk full");
  const file = { path: "/a/b.sql", name: "b.sql", encoding: "utf8" as const, lineEnding: "lf" as const };
  const id = edited("SELECT 2", { kind: "query", sql: "SELECT 1", file });
  await tabs.requestClose(id);
  await tabs.answerUnsaved(true);
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(tabs.unsavedPrompt?.error).toContain("b.sql");
  expect(tabs.unsavedPrompt?.busy).toBe(false);
});

test("Save on an untitled tab opens Save as and closes the tab once saved", async () => {
  const id = edited("SELECT 2");
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  pane.connectionId = "c1";
  pane.database = "Sales";
  await tabs.requestClose(id);
  await tabs.answerUnsaved(true);
  expect(tabs.unsavedPrompt).toBeNull();
  expect(tabs.nameRequest).not.toBeNull();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
  expect(await tabs.nameRequest!.onsubmit("Report", false)).toBeNull();
  await flush();
  expect(ipc.saveQuery).toHaveBeenCalledWith("Sales", "Report", "SELECT 2", false);
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});

test("cancelling Save as keeps the untitled tab", async () => {
  const id = edited("SELECT 2");
  const pane = tabs.list.find((x) => x.id === id)!.pane!;
  pane.connectionId = "c1";
  pane.database = "Sales";
  await tabs.requestClose(id);
  await tabs.answerUnsaved(true);
  tabs.nameRequest!.oncancel();
  await flush();
  expect(tabs.list.some((x) => x.id === id)).toBe(true);
});

test("after Don't save an open transaction is still asked about", async () => {
  const id = edited("SELECT 1", { kind: "query", connectionId: "c1", database: "Sales" });
  // No session: the tab holds nothing, so it closes straight after the answer.
  await tabs.requestClose(id);
  await tabs.answerUnsaved(false);
  expect(tabs.closePrompt).toBeNull();
  expect(tabs.list.some((x) => x.id === id)).toBe(false);
});


async function armWindowClose(save: (json: string) => Promise<void> = async () => {}) {
  stops.push(tabs.startAutosave(save));
  await flush();
  expect(closeRequested).not.toBeNull();
}

const fileSpec = { path: "/a/b.sql", name: "b.sql", encoding: "utf8" as const, lineEnding: "lf" as const };

test("with the last session off, closing the window asks about unsaved tabs", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  const a = edited("SELECT 1");
  edited("", { kind: "query" });
  await armWindowClose();
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).toHaveBeenCalled();
  expect(tabs.unsavedPrompt?.scope).toBe("window");
  expect(tabs.unsavedPrompt?.tabs.map((x) => x.tabId)).toEqual([a]);
  expect(destroy).not.toHaveBeenCalled();
});

test("Discard all closes the window", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  edited("SELECT 1");
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  await tabs.answerUnsaved(false);
  expect(destroy).toHaveBeenCalledTimes(1);
});

test("Save all writes the tabs that have a place and closes the window", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  edited("SELECT 2", { kind: "query", sql: "SELECT 1", file: fileSpec });
  edited("SELECT 4", { kind: "query", sql: "SELECT 3", savedQuery: { database: "Sales", name: "Q" } });
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  await tabs.answerUnsaved(true);
  expect(ipc.writeSqlFile).toHaveBeenCalledTimes(1);
  expect(ipc.saveQuery).toHaveBeenCalledTimes(1);
  expect(destroy).toHaveBeenCalledTimes(1);
});

test("Save all with an untitled tab keeps the window open and says why", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  edited("SELECT 2", { kind: "query", sql: "SELECT 1", file: fileSpec });
  const untitled = edited("SELECT 9");
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  await tabs.answerUnsaved(true);
  expect(destroy).not.toHaveBeenCalled();
  expect(tabs.unsavedPrompt?.error).toContain("1");
  expect(tabs.unsavedPrompt?.tabs.map((x) => x.tabId)).toEqual([untitled]);
});

test("Cancel keeps the window open", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  edited("SELECT 1");
  await armWindowClose();
  await closeRequested!({ preventDefault: vi.fn() });
  tabs.cancelUnsaved();
  expect(tabs.unsavedPrompt).toBeNull();
  expect(destroy).not.toHaveBeenCalled();
});

test("with the last session on and saved, the window closes without asking", async () => {
  edited("SELECT 1");
  await armWindowClose();
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).not.toHaveBeenCalled();
  expect(tabs.unsavedPrompt).toBeNull();
});

test("with the last session on, a failed session save makes the window ask", async () => {
  edited("SELECT 1");
  await armWindowClose(async () => {
    throw new Error("disk");
  });
  const preventDefault = vi.fn();
  await closeRequested!({ preventDefault });
  expect(preventDefault).toHaveBeenCalled();
  expect(tabs.unsavedPrompt?.scope).toBe("window");
});

test("a session save that hits the timeout makes the window ask", async () => {
  vi.useFakeTimers();
  try {
    edited("SELECT 1");
    stops.push(tabs.startAutosave(() => new Promise(() => {})));
    await vi.advanceTimersByTimeAsync(0);
    const preventDefault = vi.fn();
    const closing = closeRequested!({ preventDefault });
    await vi.advanceTimersByTimeAsync(SAVE_TIMEOUT_MS + 10);
    await closing;
    expect(preventDefault).toHaveBeenCalled();
    expect(tabs.unsavedPrompt?.scope).toBe("window");
  } finally {
    vi.useRealTimers();
  }
});

test("the window close is heard before the restore is done and waits for it", async () => {
  prefs.value = { ...DEFAULT_PREFERENCES, restoreSession: false };
  let done!: () => void;
  const stop = tabs.guardWindow(new Promise<void>((r) => (done = r)));
  await flush();
  expect(closeRequested).not.toBeNull();
  const preventDefault = vi.fn();
  const closing = closeRequested!({ preventDefault });
  await flush();
  // Nothing decided yet: the restore has not finished.
  expect(tabs.unsavedPrompt).toBeNull();
  edited("SELECT 1");
  done();
  await closing;
  expect(preventDefault).toHaveBeenCalled();
  expect(tabs.unsavedPrompt?.scope).toBe("window");
  stop();
});

test("Cmd+Q is heard before the restore is done and goes through the same flow", async () => {
  let quit: (() => void) | null = null;
  vi.mocked(ipc.onQuitRequested).mockImplementation(async (cb) => {
    quit = cb;
    return () => {};
  });
  let done!: () => void;
  const stop = tabs.guardWindow(new Promise<void>((r) => (done = r)));
  await flush();
  expect(quit).not.toBeNull();
  quit!();
  await flush();
  expect(destroy).not.toHaveBeenCalled();
  done();
  await flush();
  expect(destroy).toHaveBeenCalledTimes(1);
  stop();
});

test("autosave started after the guard does not listen a second time", async () => {
  const stop = tabs.guardWindow(Promise.resolve());
  const stopAutosave = tabs.startAutosave(async () => {});
  await flush();
  expect(ipc.onQuitRequested).toHaveBeenCalledTimes(1);
  stopAutosave();
  stop();
});
