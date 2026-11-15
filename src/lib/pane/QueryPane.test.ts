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
import { PaneState } from "./pane.svelte";
import QueryPane from "./QueryPane.svelte";

let cmp: ReturnType<typeof mount> | undefined;
const runButton = () => document.querySelector<HTMLButtonElement>("button.run")!;
const picker = () => document.querySelector<HTMLSelectElement>('select[aria-label="Connection"]')!;

function render(onrun: (sql: string, lineOffset: number) => boolean | void) {
  const pane = new PaneState("t1");
  pane.initialDoc = "select 1";
  cmp = mount(QueryPane, { target: document.body, props: { pane, onreconnect: () => {}, autofocus: false, onrun } });
  flushSync();
}

beforeEach(() => {
  connections.list = [{ id: "c1", name: "Local", passwordSaved: true, environment: "local" } as ConnectionConfig];
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

test("Run in an unbound tab hands the sql to onrun", () => {
  const onrun = vi.fn(() => true);
  render(onrun);
  runButton().click();
  expect(onrun).toHaveBeenCalledWith("select 1", 0);
  expect(document.activeElement).not.toBe(picker());
});

test("Run in an unbound tab that cannot bind focuses the connection picker", () => {
  render(() => false);
  runButton().click();
  expect(document.activeElement).toBe(picker());
});
