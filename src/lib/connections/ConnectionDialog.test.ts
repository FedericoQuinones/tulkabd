// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({ listServerDatabases: vi.fn(), saveConnection: vi.fn(), testConnection: vi.fn() }));

import * as ipc from "../ipc";
import type { ConnectionConfig } from "../ipc";
import { explorer } from "../explorer/tree.svelte";
import { connections } from "./store.svelte";
import ConnectionDialog from "./ConnectionDialog.svelte";

let cmp: ReturnType<typeof mount> | undefined;
const settle = () => new Promise((r) => setTimeout(r, 0));

const base: ConnectionConfig = {
  id: "c1",
  name: "Local",
  driver: "mssql",
  server: "127.0.0.1",
  port: 1433,
  user: "sa",
  databases: { kind: "selected", names: ["Ventas", "Gone"] },
  environment: "local",
  color: "#3b82f6",
  guard: "off",
  encrypt: true,
  trustServerCertificate: true,
  connectTimeoutS: 15,
  passwordSaved: false,
  folderId: null,
};

function open(props: { initial?: ConnectionConfig; folderId?: string | null }) {
  cmp = mount(ConnectionDialog, { target: document.body, props: { onclose: () => {}, ...props } });
  flushSync();
}
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === text)!;

beforeEach(() => {
  vi.resetAllMocks();
  explorer.reset();
  connections.list = [];
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

test("Load databases lists the server's databases and labels vanished ones as missing", async () => {
  vi.mocked(ipc.listServerDatabases).mockResolvedValue(["Ventas", "Stock"]);
  open({ initial: base });
  button("Load databases").click();
  await settle();
  flushSync();
  const rows = [...document.querySelectorAll<HTMLLabelElement>(".list label")];
  expect(rows.map((r) => r.textContent?.replace(/\s+/g, " ").trim())).toEqual(["Ventas", "Stock", "Gone missing"]);
  expect(rows.map((r) => r.querySelector("input")!.checked)).toEqual([true, false, true]);
  expect(vi.mocked(ipc.listServerDatabases).mock.calls[0][0].databases).toEqual({
    kind: "selected",
    names: ["Ventas", "Gone"],
  });
});

test("a password_required answer asks for the password inline", async () => {
  vi.mocked(ipc.listServerDatabases).mockRejectedValue("password_required");
  open({ initial: base });
  button("Load databases").click();
  await settle();
  flushSync();
  expect(document.querySelector('[role="alert"]')?.textContent).toBe("Enter the password above to load the databases.");
});

test("Selected with nothing chosen blocks saving", async () => {
  open({ initial: { ...base, databases: { kind: "selected", names: [] } } });
  document.querySelector("form")!.requestSubmit();
  await settle();
  flushSync();
  expect(document.body.textContent).toContain("Choose at least one database, or use All.");
  expect(ipc.saveConnection).not.toHaveBeenCalled();
});

test("the folder selector shows when creating, prefilled, and is hidden when editing", () => {
  explorer.folders = [{ id: "f1", name: "Cliente A", parentId: null }, { id: "f2", name: "Proyecto X", parentId: "f1" }];
  open({ folderId: "f2" });
  const sel = document.querySelector<HTMLSelectElement>("#c-folder")!;
  expect(sel.value).toBe("f2");
  expect([...sel.options].map((o) => o.textContent)).toEqual(["No folder", "Cliente A", "Cliente A / Proyecto X"]);
  unmount(cmp!);
  document.body.innerHTML = "";
  connections.list = [base];
  open({ initial: base });
  expect(document.querySelector("#c-folder")).toBeNull();
});
