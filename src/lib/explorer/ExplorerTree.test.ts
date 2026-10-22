// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import type { ConnectionConfig, Folder, ParsedConnection } from "../ipc";

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
  parseConnectionString: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import ExplorerTree from "./ExplorerTree.svelte";
import { explorer } from "./tree.svelte";

const folders: Folder[] = [{ id: "fa", name: "Cliente A", parentId: null }];
const conn = { id: "c1", name: "Sales", folderId: "fa", color: "#e5566f", passwordSaved: true } as ConnectionConfig;

let cmp: ReturnType<typeof mount> | undefined;
const onnewfromstring = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.explorerDatabases).mockResolvedValue([]);
  vi.mocked(ipc.setExpanded).mockResolvedValue();
  vi.mocked(ipc.connectExplorer).mockRejectedValue("closed");
  connections.list = [conn];
  explorer.reset();
  explorer.folders = folders;
  explorer.persistDelayMs = 0;
  cmp = mount(ExplorerTree, { target: document.body, props: { onnewconnection: () => {}, onedit: () => {}, onnewfromstring } });
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
    "New from connection string…",
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

const paste = (el: Element, text: string) => {
  const ev = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "clipboardData", { value: { getData: (type: string) => (type === "text/plain" ? text : "") } });
  el.dispatchEvent(ev);
};
const settle = () => new Promise((r) => setTimeout(r, 0));

const parsed: ParsedConnection = {
  server: "db",
  port: null,
  user: "sa",
  password: "pw",
  database: null,
  encrypt: null,
  trustServerCertificate: null,
  connectTimeoutS: null,
  ignoredKeys: [],
  unknownKeys: 0,
};

test("pasting text that is not a connection string on the tree does nothing", async () => {
  vi.mocked(ipc.parseConnectionString).mockRejectedValue("unrecognized");
  const [folder] = items();
  folder.focus();
  paste(folder, "hola mundo");
  await settle();
  expect(ipc.parseConnectionString).toHaveBeenCalledWith("hola mundo");
  expect(onnewfromstring).not.toHaveBeenCalled();
  expect(explorer.error).toBeNull();
});

test("pasting a connection string on a folder opens a new connection in it", async () => {
  vi.mocked(ipc.parseConnectionString).mockResolvedValue(parsed);
  const [folder] = items();
  folder.focus();
  paste(folder, "Server=db;UID=sa;PWD=pw");
  await settle();
  expect(onnewfromstring).toHaveBeenCalledWith("fa", parsed);
});

test("pasting into an input inside the tree is left alone", async () => {
  vi.mocked(ipc.parseConnectionString).mockResolvedValue(parsed);
  const [folder] = items();
  folder.focus();
  key(folder, "F2");
  flushSync();
  const input = folder.querySelector("input")!;
  paste(input, "Server=db");
  await settle();
  expect(ipc.parseConnectionString).not.toHaveBeenCalled();
  expect(onnewfromstring).not.toHaveBeenCalled();
});

test("the + menu offers New from connection string", () => {
  document.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]')!.click();
  flushSync();
  const item = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (b) => b.textContent!.trim() === "New from connection string…",
  )!;
  item.click();
  expect(onnewfromstring).toHaveBeenCalledWith(null, null);
});

test("beforepaste is cancelled on the tree so WebKit fires paste there, but not in an input", () => {
  const before = (el: Element) => {
    const ev = new Event("beforepaste", { bubbles: true, cancelable: true });
    el.dispatchEvent(ev);
    return ev.defaultPrevented;
  };
  const [folder] = items();
  folder.focus();
  expect(before(folder)).toBe(true);
  key(folder, "F2");
  flushSync();
  expect(before(folder.querySelector("input")!)).toBe(false);
});
