// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  onAppEvent: vi.fn(),
  listConnections: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: async () => () => {}, destroy: async () => {} }),
}));

import type { ConnectionConfig } from "../ipc";
import { connections } from "../connections/store.svelte";
import { explorer } from "../explorer/tree.svelte";
import PaneToolbar from "./PaneToolbar.svelte";

const conn = (id: string, name: string) => ({ id, name, passwordSaved: true, environment: "local" }) as ConnectionConfig;

let cmp: ReturnType<typeof mount> | undefined;
const picker = () => document.querySelector<HTMLSelectElement>('select[aria-label="Connection"]');

function render(props: Record<string, unknown> = {}) {
  cmp = mount(PaneToolbar, {
    target: document.body,
    props: {
      connection: null,
      database: null,
      databases: [],
      status: "idle",
      onrun: () => {},
      onstop: () => {},
      onreconnect: () => {},
      ondatabase: async () => {},
      ...props,
    },
  });
  flushSync();
}

beforeEach(() => {
  explorer.reset();
  connections.list = [conn("c1", "Zeta"), conn("c2", "Alpha"), conn("c3", "Mid")];
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

test("an unbound toolbar lists the connections, connected ones first then by name", () => {
  explorer.state("c1").status = "connected";
  render();
  const names = [...picker()!.querySelectorAll("option")].slice(1).map((o) => o.textContent?.split(" · ")[0]);
  expect(names).toEqual(["Zeta", "Alpha", "Mid"]);
});

test("choosing a connection in the picker hands it to onconnect", () => {
  const onconnect = vi.fn();
  render({ onconnect });
  const select = picker()!;
  select.value = "c3";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  expect(onconnect).toHaveBeenCalledWith(expect.objectContaining({ id: "c3" }));
});

test("a bound toolbar has no connection picker", () => {
  render({ connection: connections.list[0], database: "master", databases: ["master"] });
  expect(picker()).toBeNull();
});

test("focusPicker moves the keyboard to the picker", () => {
  render();
  (cmp as unknown as { focusPicker: () => void }).focusPicker();
  expect(document.activeElement).toBe(picker());
});
