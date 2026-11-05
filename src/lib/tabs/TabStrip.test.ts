// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  transactionCount: vi.fn(),
  endTransactions: vi.fn(),
  onQuitRequested: vi.fn(),
  logCancelledRun: vi.fn(),
  listConnections: vi.fn(),
  startupError: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }),
}));

import * as ipc from "../ipc";
import TabStrip from "./TabStrip.svelte";
import { tabs } from "./tabs.svelte";

let cmp: ReturnType<typeof mount> | undefined;
const flush = () => new Promise((r) => setTimeout(r, 0));
const labels = () => [...document.querySelectorAll('[role="tab"] .lab')].map((e) => e.textContent);
const tabButton = (title: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="tab"]')].find((b) => b.textContent?.includes(title))!;

beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.closeSession).mockResolvedValue();
  vi.mocked(ipc.logCancelledRun).mockResolvedValue();
  vi.mocked(ipc.transactionCount).mockResolvedValue(0);
  vi.mocked(ipc.onQuitRequested).mockResolvedValue(() => {});
  await tabs.reset();
  tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  cmp = mount(TabStrip, { target: document.body, props: { groupId: tabs.activeGroupId } });
  flushSync();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

test("lists the tabs of the group and marks the active one", () => {
  expect(labels()).toEqual(["untitled-1", "untitled-2"]);
  expect(tabButton("untitled-2").getAttribute("aria-selected")).toBe("true");
  expect(tabButton("untitled-1").getAttribute("aria-selected")).toBe("false");
});

test("clicking a tab activates it", () => {
  tabButton("untitled-1").click();
  flushSync();
  expect(tabs.active?.title).toBe("untitled-1");
  expect(tabButton("untitled-1").getAttribute("aria-selected")).toBe("true");
});

test("the close button closes that tab", async () => {
  document.querySelector<HTMLButtonElement>('button[aria-label="Close untitled-1"]')!.click();
  await flush();
  flushSync();
  expect(labels()).toEqual(["untitled-2"]);
});

test("the context menu of a tab offers Close", async () => {
  tabButton("untitled-1").dispatchEvent(new KeyboardEvent("keydown", { key: "ContextMenu", bubbles: true, cancelable: true }));
  flushSync();
  const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find((b) => b.textContent?.includes("Close"));
  expect(item).toBeTruthy();
  item!.click();
  await flush();
  flushSync();
  expect(labels()).toEqual(["untitled-2"]);
});
