// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
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
  parseConnectionString: vi.fn(),
}));

import * as ipc from "../ipc";
import { connections } from "../connections/store.svelte";
import ExplorerTree from "./ExplorerTree.svelte";
import { explorer } from "./tree.svelte";

const folders: Folder[] = [{ id: "fa", name: "Cliente A", parentId: null }];
const conn = { id: "c1", name: "Sales", folderId: "fa", color: "#e5566f", passwordSaved: true } as ConnectionConfig;
const LONG = "TLS handshake failed. The certificate chain is not trusted. Enable trust server certificate or install the CA.";

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
  cmp = mount(ExplorerTree, { target: document.body, props: { onnewconnection: () => {}, onedit: () => {}, onnewfromstring: () => {} } });
  flushSync();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  vi.unstubAllGlobals();
});

const settle = () => new Promise((r) => setTimeout(r, 0));
const key = (el: Element, k: string) => el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
const notice = () => document.querySelector<HTMLElement>('[role="treeitem"].notice')!;
const text = () => notice().querySelector<HTMLElement>(".note")!;
const dlg = () => document.querySelector<HTMLElement>('[role="dialog"]');
const button = (name: string) => [...dlg()!.querySelectorAll("button")].find((b) => b.textContent!.trim() === name)!;

/** The connection is expanded and its connect attempt failed with `message`. */
function failed(message: string) {
  explorer.expanded.add("fa");
  explorer.expanded.add("c1");
  explorer.handleEvent({ kind: "explorerStatus", connectionId: "c1", status: "error", message });
  flushSync();
}
function offline() {
  explorer.expanded.add("fa");
  explorer.expanded.add("c1");
  flushSync();
}

test("clicking an error notice opens a dialog with the full message", () => {
  failed(LONG);
  text().click();
  flushSync();
  expect(dlg()!.getAttribute("aria-modal")).toBe("true");
  expect(dlg()!.textContent).toContain("Could not connect to Sales");
  expect(dlg()!.querySelector(".detail")!.textContent).toBe(LONG);
});

test("clicking a muted notice opens nothing", () => {
  offline();
  expect(notice().textContent).toContain("Not connected");
  text().click();
  flushSync();
  expect(dlg()).toBeNull();
});

test("Copy writes the full message to the clipboard", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  vi.stubGlobal("navigator", { clipboard: { writeText } });
  failed(LONG);
  text().click();
  flushSync();
  button("Copy").click();
  await settle();
  flushSync();
  expect(writeText).toHaveBeenCalledWith(LONG);
  expect(dlg()!.querySelector('[role="status"]')!.textContent).toBe("Copied");
});

test("a failed copy says so in the dialog", async () => {
  vi.stubGlobal("navigator", { clipboard: { writeText: vi.fn().mockRejectedValue("denied") } });
  failed(LONG);
  text().click();
  flushSync();
  button("Copy").click();
  await settle();
  flushSync();
  expect(dlg()!.querySelector('[role="alert"]')!.textContent).toBe("Could not copy: denied");
});

test("Retry in the dialog connects again and closes it", async () => {
  failed(LONG);
  text().click();
  flushSync();
  button("Retry").click();
  await settle();
  flushSync();
  expect(ipc.connectExplorer).toHaveBeenCalledWith("c1", undefined);
  expect(dlg()).toBeNull();
});

test("Escape and Close dismiss the dialog and return the focus to the notice", () => {
  failed(LONG);
  for (const how of ["escape", "close"]) {
    notice().focus();
    text().click();
    flushSync();
    expect(dlg()).not.toBeNull();
    if (how === "escape") key(document.activeElement!, "Escape");
    else button("Close").click();
    flushSync();
    expect(dlg()).toBeNull();
    expect(document.activeElement).toBe(notice());
  }
});

test("Enter on a focused error notice opens the details instead of retrying", () => {
  failed(LONG);
  explorer.focusKey = explorer.rows.find((r) => r.kind === "notice")!.key;
  flushSync();
  notice().focus();
  key(notice(), "Enter");
  flushSync();
  expect(dlg()).not.toBeNull();
  expect(ipc.connectExplorer).not.toHaveBeenCalled();
});

test("Enter on a muted notice still runs its action", () => {
  offline();
  vi.mocked(ipc.connectExplorer).mockResolvedValue({ server: { version: "16", user: "sa", singleDatabase: false }, databases: [] });
  explorer.focusKey = explorer.rows.find((r) => r.kind === "notice")!.key;
  flushSync();
  notice().focus();
  key(notice(), "Enter");
  flushSync();
  expect(dlg()).toBeNull();
  expect(ipc.connectExplorer).toHaveBeenCalledWith("c1", undefined);
});
