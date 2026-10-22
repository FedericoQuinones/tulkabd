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

const type = (sel: string, value: string) => {
  const el = document.querySelector<HTMLInputElement>(sel)!;
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const rowNames = () => [...document.querySelectorAll(".list .n")].map((n) => n.textContent);

test("editing the server resets a loaded list but keeps the chosen names", async () => {
  vi.mocked(ipc.listServerDatabases).mockResolvedValue(["Ventas", "Stock"]);
  open({ initial: base });
  button("Load databases").click();
  await settle();
  flushSync();
  expect(rowNames()).toEqual(["Ventas", "Stock", "Gone"]);
  type("#c-server", "other-host");
  expect(rowNames()).toEqual(["Ventas", "Gone"]);
  expect(document.body.textContent).not.toContain("missing");
});

test("a slow response for an older request is ignored", async () => {
  let first!: (v: string[]) => void;
  vi.mocked(ipc.listServerDatabases)
    .mockImplementationOnce(() => new Promise((r) => (first = r)))
    .mockResolvedValueOnce(["New1"]);
  open({ initial: base });
  button("Load databases").click();
  type("#c-server", "other-host");
  button("Load databases").click();
  await settle();
  flushSync();
  first(["Old1"]);
  await settle();
  flushSync();
  expect(rowNames()).toEqual(["New1", "Ventas", "Gone"]);
});

test("a response arriving after the inputs changed is dropped", async () => {
  let first!: (v: string[]) => void;
  vi.mocked(ipc.listServerDatabases).mockImplementationOnce(() => new Promise((r) => (first = r)));
  open({ initial: base });
  button("Load databases").click();
  type("#c-server", "other-host");
  first(["Old1"]);
  await settle();
  flushSync();
  expect(rowNames()).toEqual(["Ventas", "Gone"]);
});

test("the filter ignores case", async () => {
  vi.mocked(ipc.listServerDatabases).mockResolvedValue(["Ventas", "Stock"]);
  open({ initial: base });
  button("Load databases").click();
  await settle();
  flushSync();
  type(".filter", "VEN");
  expect(rowNames()).toEqual(["Ventas"]);
});

test("Selected, All, Selected keeps the chosen names", () => {
  open({ initial: base });
  button("All").click();
  flushSync();
  expect(document.querySelector(".list")).toBeNull();
  button("Selected").click();
  flushSync();
  expect(rowNames()).toEqual(["Ventas", "Gone"]);
});

test("a load error other than password_required is shown inline", async () => {
  vi.mocked(ipc.listServerDatabases).mockRejectedValue("login failed");
  open({ initial: base });
  button("Load databases").click();
  await settle();
  flushSync();
  expect(document.querySelector('[role="alert"]')?.textContent).toBe("Could not load databases: login failed");
});

test("duplicate saved names do not break the list", () => {
  open({ initial: { ...base, databases: { kind: "selected", names: ["Ventas", "Ventas"] } } });
  expect(rowNames()).toEqual(["Ventas"]);
});

test("Save sends the selected scope", async () => {
  vi.mocked(ipc.saveConnection).mockResolvedValue({ connection: base, keyringUnavailable: false });
  open({ initial: base });
  document.querySelector("form")!.requestSubmit();
  await settle();
  const conn = vi.mocked(ipc.saveConnection).mock.calls[0][0];
  expect(conn.databases).toEqual({ kind: "selected", names: ["Ventas", "Gone"] });
});
