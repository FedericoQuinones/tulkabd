// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";

vi.mock("../ipc", async (original) => ({
  ...(await original<typeof import("../ipc")>()),
  listRunLog: vi.fn(),
  openSession: vi.fn(),
  closeSession: vi.fn(),
  fetchRows: vi.fn(),
  setResultView: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }),
}));

import * as ipc from "../ipc";
import type { ConnectionConfig, RunLogEntry } from "../ipc";
import { connections } from "../connections/store.svelte";
import { PaneState } from "../pane/pane.svelte";
import ResultsPanel from "../results/ResultsPanel.svelte";
import Rail from "../shell/Rail.svelte";
import { tabs } from "../tabs/tabs.svelte";
import { runLog } from "./runlog.svelte";
import { showRunLog } from "./reveal";

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const noon = new Date();
noon.setHours(12, 0, 0, 0);
const DAY = 24 * 3600 * 1000;

function entry(id: number, over: Partial<RunLogEntry> = {}): RunLogEntry {
  return {
    id,
    atMs: noon.getTime() + id * 1000,
    connectionId: "c1",
    connectionName: "Sales",
    color: "var(--env-prod)",
    database: "Orders",
    sql: `SELECT ${id}`,
    sqlTruncated: false,
    outcome: "ok",
    rows: 20,
    elapsedMs: 84,
    source: "app",
    client: null,
    ...over,
  };
}
const page = (from: number, n: number) => Array.from({ length: n }, (_, i) => entry(from - i));

const conn = { id: "c1", name: "Sales", passwordSaved: true, environment: "production" } as ConnectionConfig;
let cmp: ReturnType<typeof mount> | undefined;
let pane: PaneState;

function show() {
  cmp = mount(ResultsPanel, { target: document.body, props: { pane, onjump: () => {} } });
  flushSync();
}

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.listRunLog).mockResolvedValue([]);
  vi.mocked(ipc.closeSession).mockResolvedValue();
  runLog.reset();
  connections.list = [conn];
  await tabs.reset();
  pane = new PaneState("tab-x");
});

afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

test("loads the first page only when the tab opens", async () => {
  vi.mocked(ipc.listRunLog).mockResolvedValue(page(3, 3));
  show();
  await tick();
  expect(ipc.listRunLog).not.toHaveBeenCalled();
  pane.bottomTab = "runlog";
  flushSync();
  await vi.waitFor(() => expect(runLog.entries).toHaveLength(3));
  expect(ipc.listRunLog).toHaveBeenCalledTimes(1);
  expect(ipc.listRunLog).toHaveBeenCalledWith(null, 100);
  // Leaving and coming back does not ask again.
  pane.bottomTab = "results";
  flushSync();
  pane.bottomTab = "runlog";
  flushSync();
  await tick();
  expect(ipc.listRunLog).toHaveBeenCalledTimes(1);
});

test("runLogged prepends and bumps the today counter", () => {
  runLog.prepend(entry(1, { atMs: noon.getTime() - DAY }));
  expect(runLog.todayCount).toBe(0);
  runLog.prepend(entry(2));
  runLog.prepend(entry(3));
  expect(runLog.entries.map((e) => e.id)).toEqual([3, 2, 1]);
  expect(runLog.todayCount).toBe(2);
  show();
  expect(document.body.textContent).toContain("Run log2");
});

test("runLogged from the poller prepends like any other", () => {
  runLog.prepend(entry(5));
  runLog.prepend(entry(7, { source: "ai", client: "claude" }));
  // Noticed late: lower id than an app run held already.
  runLog.prepend(entry(6, { source: "ai", client: "claude" }));
  runLog.prepend(entry(1));
  runLog.prepend(entry(6, { source: "ai", client: "claude" }));
  expect(runLog.entries.map((e) => e.id)).toEqual([7, 6, 5, 1]);
});

test("ai entries show the AI tag with the client in the title", () => {
  runLog.prepend(entry(1));
  runLog.prepend(entry(2, { source: "ai", client: "claude-code" }));
  runLog.prepend(entry(3, { source: "ai", client: null }));
  pane.bottomTab = "runlog";
  show();
  const tag = (id: number) => document.querySelector<HTMLElement>(`tr[data-id="${id}"] .ai`);
  expect(tag(1)).toBeNull();
  expect(tag(2)!.textContent).toBe("AI");
  expect(tag(2)!.title).toBe("Run by an AI assistant (claude-code)");
  expect(tag(3)!.title).toBe("Run by an AI assistant");
  tag(2)!.closest("tr")!.click();
  expect(tabs.active!.pane!.connectionId).toBe("c1");
});

test("an entry that arrives before the first load is not duplicated by it", async () => {
  runLog.prepend(entry(5));
  vi.mocked(ipc.listRunLog).mockResolvedValue(page(5, 3));
  await runLog.ensureLoaded();
  expect(runLog.entries.map((e) => e.id)).toEqual([5, 4, 3]);
});

test("blocked and cancelled show their outcome, not rows", async () => {
  runLog.prepend(entry(1, { outcome: "blocked", rows: null, elapsedMs: null }));
  runLog.prepend(entry(2, { outcome: "cancelled", rows: null, elapsedMs: null }));
  runLog.prepend(entry(3, { rows: 20, elapsedMs: 1240 }));
  runLog.prepend(entry(4, { rows: null, elapsedMs: 12 }));
  pane.bottomTab = "runlog";
  show();
  const rows = [...document.querySelectorAll<HTMLElement>("tr")].slice(1);
  const cells = rows.map((r) => [...r.querySelectorAll('[role="gridcell"]')].map((c) => c.textContent?.trim()));
  expect(cells[0].slice(3)).toEqual(["—", "12 ms"]);
  expect(cells[1].slice(3)).toEqual(["20", "1.24 s"]);
  expect(cells[2].slice(3)).toEqual(["Cancelled", "—"]);
  expect(cells[3].slice(3)).toEqual(["Blocked", "—"]);
  expect(rows[3].querySelector(".no")).not.toBeNull();
  expect(cells[3][1]).toBe("Sales · Orders");
});

test("click opens a new tab bound to the entry's connection and database without connecting", () => {
  runLog.prepend(entry(1, { sql: "DELETE FROM dbo.Pedidos", database: "Billing" }));
  pane.bottomTab = "runlog";
  show();
  document.querySelector<HTMLElement>('tr[data-id="1"]')!.click();
  const tab = tabs.active!;
  expect(tab.pane!.connectionId).toBe("c1");
  expect(tab.pane!.database).toBe("Billing");
  expect(tab.pane!.initialDoc).toBe("DELETE FROM dbo.Pedidos");
  expect(tab.pane!.notice).toBeNull();
  expect(ipc.openSession).not.toHaveBeenCalled();
});

test("Enter on a focused row opens it and the arrows move the focus", () => {
  runLog.prepend(entry(1));
  runLog.prepend(entry(2));
  pane.bottomTab = "runlog";
  show();
  const r = [...document.querySelectorAll<HTMLElement>("tr[data-id]")];
  r[0].focus();
  r[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  expect(document.activeElement).toBe(r[1]);
  r[1].dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  expect(tabs.active!.pane!.initialDoc).toBe("SELECT 1");
});

test("deleted connection opens unbound with a notice", () => {
  runLog.prepend(entry(1, { connectionId: "gone" }));
  pane.bottomTab = "runlog";
  show();
  document.querySelector<HTMLElement>('tr[data-id="1"]')!.click();
  const p = tabs.active!.pane!;
  expect(p.connectionId).toBeNull();
  expect(p.notice).toBe("The connection of this run no longer exists.");
  expect(p.initialDoc).toBe("SELECT 1");
});

test("rail clock switches the active pane to the run log", async () => {
  const onselect = vi.fn();
  const rail = mount(Rail, { target: document.body, props: { active: null, onselect } });
  flushSync();
  document.querySelector<HTMLElement>('nav button[aria-label="Run log"]')!.click();
  expect(onselect).toHaveBeenCalledWith("rail.log");
  unmount(rail);

  const id = tabs.open({ kind: "query" });
  const active = tabs.activePane!;
  expect(active.tabId).toBe(id);
  active.bottomTab = "results";
  runLog.prepend(entry(1));
  cmp = mount(ResultsPanel, { target: document.body, props: { pane: active, onjump: () => {} } });
  void showRunLog();
  expect(active.bottomTab).toBe("runlog");
  await vi.waitFor(() => expect(document.activeElement?.tagName).toBe("TR"));
  expect(document.activeElement?.tagName).toBe("TR");
});

test("scrolling to the end loads more with before = last id", async () => {
  vi.mocked(ipc.listRunLog).mockResolvedValueOnce(page(200, 100)).mockResolvedValueOnce(page(100, 40));
  pane.bottomTab = "runlog";
  show();
  await vi.waitFor(() => expect(runLog.entries).toHaveLength(100));
  document.querySelector<HTMLElement>(".scroller")!.dispatchEvent(new Event("scroll"));
  await vi.waitFor(() => expect(runLog.entries).toHaveLength(140));
  expect(ipc.listRunLog).toHaveBeenLastCalledWith(101, 100);
  // A short page was the last one: nothing more is asked.
  document.querySelector<HTMLElement>(".scroller")!.dispatchEvent(new Event("scroll"));
  await tick();
  expect(ipc.listRunLog).toHaveBeenCalledTimes(2);
});

test("an empty log says so", async () => {
  pane.bottomTab = "runlog";
  show();
  await vi.waitFor(() => expect(document.body.textContent).toContain("Nothing run yet."));
});

test("the first open from the rail loads the log and then focuses the table", async () => {
  vi.mocked(ipc.listRunLog).mockResolvedValue(page(3, 3));
  tabs.open({ kind: "query" });
  const active = tabs.activePane!;
  cmp = mount(ResultsPanel, { target: document.body, props: { pane: active, onjump: () => {} } });
  void showRunLog();
  await vi.waitFor(() => expect(document.activeElement?.tagName).toBe("TR"));
});

test("a truncated run opens with a notice", () => {
  runLog.prepend(entry(1, { sqlTruncated: true }));
  pane.bottomTab = "runlog";
  show();
  document.querySelector<HTMLElement>('tr[data-id="1"]')!.click();
  expect(tabs.active!.pane!.notice).toBe("This run was longer than 100 KB; only the beginning was kept.");
});

test("a new run goes back from the run log to the results", () => {
  pane.bottomTab = "runlog";
  show();
  pane.runCount++;
  flushSync();
  expect(pane.bottomTab).toBe("results");
});

test("Enter with a modifier is left to the shortcuts", () => {
  runLog.prepend(entry(1));
  pane.bottomTab = "runlog";
  show();
  const row = document.querySelector<HTMLElement>('tr[data-id="1"]')!;
  row.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true }));
  expect(tabs.list).toHaveLength(0);
});

test("a failed first load shows an error and Retry reads again", async () => {
  vi.mocked(ipc.listRunLog).mockRejectedValueOnce("boom").mockResolvedValueOnce(page(2, 2));
  pane.bottomTab = "runlog";
  show();
  await vi.waitFor(() => expect(document.body.textContent).toContain("Could not read the run log."));
  [...document.querySelectorAll("button")].find((b) => b.textContent === "Retry")!.click();
  await vi.waitFor(() => expect(runLog.entries).toHaveLength(2));
  expect(document.body.textContent).not.toContain("Could not read the run log.");
});

test("error and disconnected show their outcome", () => {
  runLog.prepend(entry(1, { outcome: "error", rows: null }));
  runLog.prepend(entry(2, { outcome: "disconnected", rows: null }));
  pane.bottomTab = "runlog";
  show();
  const out = [...document.querySelectorAll("tr[data-id] td.out")].map((c) => c.textContent);
  expect(out).toEqual(["Disconnected", "Error"]);
});

test("ArrowDown on the last row loads more", async () => {
  vi.mocked(ipc.listRunLog).mockResolvedValueOnce(page(200, 100)).mockResolvedValueOnce([]);
  pane.bottomTab = "runlog";
  show();
  await vi.waitFor(() => expect(runLog.entries).toHaveLength(100));
  const rows = document.querySelectorAll<HTMLElement>("tr[data-id]");
  rows[99].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
  await vi.waitFor(() => expect(ipc.listRunLog).toHaveBeenLastCalledWith(101, 100));
});
