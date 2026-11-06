// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import type { Folder } from "../ipc";

vi.mock("../ipc", () => ({
  connectExplorer: vi.fn(),
  explorerDatabases: vi.fn(),
  explorerObjects: vi.fn(),
  explorerColumns: vi.fn(),
  searchObjects: vi.fn(),
  setExpanded: vi.fn(),
  listFolders: vi.fn(),
  listConnections: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import ExplorerTree from "./ExplorerTree.svelte";
import { explorer } from "./tree.svelte";

const ROW = 28;
const VIEW = 280;
const TOTAL = 3000;

// jsdom has no layout: the tree reports a fixed height, and keeps the scrollTop it is given.
let clientHeight: PropertyDescriptor | undefined;
beforeEach(() => {
  clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get() {
      return (this as HTMLElement).getAttribute("role") === "tree" ? VIEW : 0;
    },
  });
});
afterEach(() => {
  if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
});

// Zero-padded so the sort by name is the index order.
const name = (i: number) => `F${String(i).padStart(4, "0")}`;
const many = (n: number): Folder[] => Array.from({ length: n }, (_, i) => ({ id: `f${i}`, name: name(i), parentId: null }));

let cmp: ReturnType<typeof mount> | undefined;
beforeEach(async () => {
  vi.resetAllMocks();
  vi.mocked(ipc.setExpanded).mockResolvedValue();
  connections.list = [];
  explorer.reset();
  explorer.folders = many(TOTAL);
  cmp = mount(ExplorerTree, { target: document.body, props: { onnewconnection: () => {}, onedit: () => {} } });
  flushSync();
  await tick();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const tree = () => document.querySelector<HTMLElement>('[role="tree"]')!;
const items = () => [...document.querySelectorAll<HTMLElement>('[role="treeitem"]')];
const indexes = () => items().map((el) => Number(el.dataset.index));
const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
const settle = async () => {
  await tick();
  flushSync();
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
};
async function scrollTo(top: number) {
  tree().scrollTop = top;
  tree().dispatchEvent(new Event("scroll"));
  await settle();
}

test("only the rows in view are in the DOM, in tree order", () => {
  expect(items().length).toBeLessThan(40);
  const idx = indexes();
  expect(idx[0]).toBe(0);
  expect(idx).toEqual([...idx].sort((a, b) => a - b));
  expect(tree().querySelector<HTMLElement>(".inner")!.style.height).toBe(`${TOTAL * ROW}px`);
});

test("rows carry level, set size and position because their siblings are not in the DOM", () => {
  const second = items()[1];
  expect(second.getAttribute("aria-level")).toBe("1");
  expect(second.getAttribute("aria-setsize")).toBe(String(TOTAL));
  expect(second.getAttribute("aria-posinset")).toBe("2");
});

test("scrolling swaps the rendered window", async () => {
  await scrollTo(1000 * ROW);
  const idx = indexes();
  expect(idx).toContain(1000);
  expect(idx).not.toContain(1);
  expect(idx.length).toBeLessThan(40);
  const row = items().find((el) => el.dataset.index === "1000")!;
  expect(row.style.top).toBe(`${1000 * ROW}px`);
  expect(row.getAttribute("aria-posinset")).toBe("1001");
  expect(row.textContent).toContain(name(1000));
});

test("End moves the focus to the last row and brings it into view", async () => {
  items()[0].focus();
  key(items()[0], "End");
  await settle();
  const last = items().find((el) => el.dataset.index === String(TOTAL - 1))!;
  expect(last).toBeDefined();
  expect(document.activeElement).toBe(last);
  expect(last.tabIndex).toBe(0);
  expect(tree().scrollTop).toBeGreaterThan((TOTAL - 20) * ROW);
});

test("PageDown and PageUp move by a screenful", async () => {
  items()[0].focus();
  key(items()[0], "PageDown");
  await settle();
  const pageRows = Math.floor(VIEW / ROW) - 1;
  expect(document.activeElement).toBe(items().find((el) => el.dataset.index === String(pageRows)));
  key(document.activeElement!, "PageUp");
  await settle();
  expect(document.activeElement).toBe(items().find((el) => el.dataset.index === "0"));
});

test("the focused row stays rendered when it is scrolled out of view", async () => {
  items()[0].focus();
  await scrollTo(2000 * ROW);
  const tab = items().filter((el) => el.tabIndex === 0);
  expect(tab).toHaveLength(1);
  expect(tab[0].dataset.index).toBe("0");
  expect(indexes()).toEqual([...indexes()].sort((a, b) => a - b));
});

test("a row focused from elsewhere is scrolled to", async () => {
  explorer.focusKey = "f2500";
  await settle();
  expect(indexes()).toContain(2500);
  expect(tree().scrollTop).toBeGreaterThan(2400 * ROW);
});

test("rows appearing above the view do not move what is being read", async () => {
  await scrollTo(1000 * ROW);
  const before = items().find((el) => el.dataset.index === "1000")!.textContent;
  explorer.folders = [{ id: "new", name: "A", parentId: null }, ...explorer.folders];
  await settle();
  expect(tree().scrollTop).toBe(1001 * ROW);
  const shown = items().find((el) => el.dataset.index === "1001")!;
  expect(shown.textContent).toBe(before);
});

test("an empty filter result shows the notice and no rows", async () => {
  vi.mocked(ipc.searchObjects).mockResolvedValue({ hits: [], totalMatches: 0, databasesMatched: 0, indexing: 0, queries: [] });
  await explorer.setFilter("zzzz-nothing");
  await settle();
  expect(items()).toHaveLength(0);
  expect(document.querySelector(".none")).not.toBeNull();
});
