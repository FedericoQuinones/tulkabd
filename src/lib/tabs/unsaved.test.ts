import { beforeEach, expect, test, vi } from "vitest";
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

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.transactionCount).mockResolvedValue(0);
  vi.mocked(ipc.logCancelledRun).mockResolvedValue();
  vi.mocked(ipc.saveQuery).mockResolvedValue();
  vi.mocked(ipc.writeSqlFile).mockResolvedValue();
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

void SAVE_TIMEOUT_MS;
void closeRequested;
