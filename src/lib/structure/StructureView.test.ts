// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", async (original) => ({
  ...(await original<typeof import("../ipc")>()),
  tableStructure: vi.fn(),
  saveConnection: vi.fn(),
}));

import * as ipc from "../ipc";
import type { TableStructure } from "../ipc";
import { explorer } from "../explorer/tree.svelte";
import StructureView from "./StructureView.svelte";
import { clearStructureCache } from "./cache";
import { connections } from "../connections/store.svelte";

const structure: TableStructure = {
  columns: [{ name: "id", typeName: "int", nullable: false, primaryKey: true, default: null }],
  foreignKeys: [],
  indexes: [],
} as unknown as TableStructure;

let cmp: ReturnType<typeof mount> | undefined;
const props = { connectionId: "c1", database: "Sales", schema: "dbo", name: "Orders" };
const settle = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
  flushSync();
};
async function show() {
  cmp = mount(StructureView, { target: document.body, props });
  await settle();
}
function hide() {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.tableStructure).mockResolvedValue(structure);
  clearStructureCache();
});
afterEach(() => {
  hide();
  delete explorer.conns["c1"];
});

test("shows the columns of the table", async () => {
  await show();
  expect(ipc.tableStructure).toHaveBeenCalledWith("c1", "Sales", "dbo", "Orders");
  expect(document.body.textContent).toContain("int");
  expect(document.body.textContent).toContain("PK");
});

test("activating the tab again does not query again; Refresh does", async () => {
  await show();
  hide();
  await show();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(1);
  expect(document.body.textContent).toContain("Columns");
  [...document.querySelectorAll("button")].find((b) => b.textContent === "Refresh")!.click();
  await settle();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(2);
});

test("reloads when the connection comes back", async () => {
  explorer.conns["c1"] = { status: "offline", message: null, databases: null, indexing: [], failed: [] };
  await show();
  explorer.conns["c1"].status = "connected";
  await settle();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(2);
});

test("a failed load shows the error with Retry, and is not cached", async () => {
  vi.mocked(ipc.tableStructure).mockRejectedValueOnce("boom");
  await show();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("boom");
  document.querySelector<HTMLButtonElement>('[role="alert"] button')!.click();
  await settle();
  expect(document.querySelector('[role="alert"]')).toBeNull();
  expect(document.body.textContent).toContain("PK");
});

test("a view showing not connected loads when the connection comes up", async () => {
  explorer.conns["c1"] = { status: "offline", message: null, databases: null, indexing: [], failed: [] };
  vi.mocked(ipc.tableStructure).mockRejectedValueOnce("not_connected");
  await show();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(1);
  explorer.conns["c1"].status = "connected";
  await settle();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(2);
  expect(document.body.textContent).toContain("PK");
});

test("a reconnect or an edit of the connection drops the cached structures of its other tables", async () => {
  explorer.conns["c1"] = { status: "offline", message: null, databases: null, indexing: [], failed: [] };
  await show();
  hide();
  const other = { ...props, name: "Customers" };
  cmp = mount(StructureView, { target: document.body, props: other });
  await settle();
  hide();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(2);
  // Orders is not on screen while the connection goes down and up.
  explorer.conns["c1"].status = "connected";
  await settle();
  await show();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(3);
  hide();
  // Saving the connection (same id, maybe a new server) forgets them too.
  vi.mocked(ipc.saveConnection).mockResolvedValue({} as never);
  vi.spyOn(connections, "refresh").mockResolvedValue();
  vi.spyOn(connections, "loadGuardOff").mockResolvedValue();
  await connections.save({ id: "c1" } as never);
  await show();
  expect(ipc.tableStructure).toHaveBeenCalledTimes(4);
});
