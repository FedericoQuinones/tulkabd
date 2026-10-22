// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import type { ConnectionConfig, Folder } from "../ipc";

vi.mock("../ipc", () => ({
  connectExplorer: vi.fn(),
  explorerDatabases: vi.fn(),
  explorerObjects: vi.fn(),
  explorerColumns: vi.fn(),
  searchObjects: vi.fn(),
  setExpanded: vi.fn(),
  moveFolder: vi.fn(),
  moveConnection: vi.fn(),
  listFolders: vi.fn(),
  listConnections: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import ExplorerTree from "./ExplorerTree.svelte";
import { explorer } from "./tree.svelte";

const folders: Folder[] = [{ id: "fa", name: "Cliente A", parentId: null }];
const conn = { id: "c1", name: "Sales", folderId: "fa", color: "#e5566f", passwordSaved: true } as ConnectionConfig;

let cmp: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([]);
  vi.mocked(ipc.setExpanded).mockResolvedValue();
  vi.mocked(ipc.connectExplorer).mockRejectedValue("closed");
  connections.list = [conn];
  explorer.reset();
  explorer.folders = folders;
  explorer.persistDelayMs = 0;
  cmp = mount(ExplorerTree, { target: document.body, props: { onnewconnection: () => {}, onedit: () => {} } });
  flushSync();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const items = () => [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')];
const key = (el: Element, k: string, init: KeyboardEventInit = {}) =>
  el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...init }));

test("rows are an ARIA tree with one tab stop", () => {
  expect(document.querySelector('[role="tree"]')).not.toBeNull();
  const [folder] = items();
  expect(folder.getAttribute("aria-level")).toBe("1");
  expect(folder.getAttribute("aria-expanded")).toBe("false");
  expect(items().filter((x) => x.tabIndex === 0)).toHaveLength(1);
});

test("arrow keys expand and move the focus", async () => {
  const [folder] = items();
  folder.focus();
  key(folder, "ArrowRight");
  flushSync();
  expect(items()).toHaveLength(2);
  expect(items()[0].getAttribute("aria-expanded")).toBe("true");
  key(items()[0], "ArrowDown");
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
  expect(document.activeElement).toBe(items()[1]);
  expect(items()[1].tabIndex).toBe(0);
  expect(items()[1].getAttribute("aria-level")).toBe("2");
});

test("Shift+F10 opens the row menu and Escape closes it", async () => {
  const [folder] = items();
  folder.focus();
  key(folder, "F10", { shiftKey: true });
  flushSync();
  const menu = document.querySelector('[role="menu"]')!;
  expect([...menu.querySelectorAll('[role="menuitem"]')].map((b) => b.textContent!.trim())).toEqual([
    "Rename",
    "New subfolder",
    "New connection here",
    "Delete",
  ]);
  key(document.activeElement!, "Escape");
  flushSync();
  expect(document.querySelector('[role="menu"]')).toBeNull();
});

test("Escape in the filter clears it", async () => {
  const input = document.querySelector<HTMLInputElement>('input[type="search"]')!;
  await explorer.setFilter("zz");
  flushSync();
  expect(items()).toHaveLength(0);
  key(input, "Escape");
  await tick();
  flushSync();
  expect(explorer.filter).toBe("");
  expect(items()).toHaveLength(1);
});
