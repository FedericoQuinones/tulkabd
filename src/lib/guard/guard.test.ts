// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  setConnectionGuard: vi.fn(),
  setGuardOffForSession: vi.fn(),
  guardOffForSession: vi.fn(),
}));

import * as ipc from "../ipc";
import type { ConnectionConfig, GuardFinding } from "../ipc";
import { connections } from "../connections/store.svelte";
import PaneToolbar from "../pane/PaneToolbar.svelte";
import BlockedBar from "./BlockedBar.svelte";
import GuardDialog from "./GuardDialog.svelte";
import GuardMenu from "./GuardMenu.svelte";

const conn = { id: "c1", name: "Prod DB", environment: "production", color: "#e5566f", guard: "confirm" } as ConnectionConfig;
const findings: GuardFinding[] = [
  { kind: "deleteWithoutWhere", object: "dbo.Orders", statement: "DELETE FROM dbo.Orders", line: 0 },
  { kind: "drop", object: null, statement: "DROP TABLE x", line: 3 },
];

let cmp: ReturnType<typeof mount> | undefined;
const settle = () => new Promise((r) => setTimeout(r, 0));
const button = (text: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === text)!;

beforeEach(() => {
  vi.resetAllMocks();
  connections.list = [conn];
  connections.guardOff = [];
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

function openDialog(props: Partial<{ onconfirm: (d: boolean) => void; oncancel: () => void }> = {}) {
  cmp = mount(GuardDialog, {
    target: document.body,
    props: {
      connection: conn,
      prompt: { sql: "x", lineOffset: 10, findings },
      onconfirm: () => {},
      oncancel: () => {},
      ...props,
    },
  });
  flushSync();
}

test("GuardDialog names the connection, shows each statement and its consequence", () => {
  openDialog();
  const text = document.body.textContent!;
  expect(text).toContain("Run on Prod DB?");
  expect(text).toContain("DELETE FROM dbo.Orders");
  expect(text).toContain("This deletes every row in dbo.Orders because it has no WHERE.");
  expect(text).toContain("DROP TABLE x");
  expect(text).toContain("This drops the target.");
  // 0-based line + selection offset, shown 1-based.
  expect(text).toContain("Line 11");
  expect(text).toContain("Line 14");
  expect(document.querySelector<HTMLElement>('[role="dialog"]')!.getAttribute("aria-modal")).toBe("true");
});

test("Run anyway is not the default focus; Escape cancels", () => {
  const oncancel = vi.fn();
  const onconfirm = vi.fn();
  openDialog({ oncancel, onconfirm });
  expect(document.activeElement).toBe(button("Cancel"));
  document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(oncancel).toHaveBeenCalledOnce();
  expect(onconfirm).not.toHaveBeenCalled();
});

test("Run anyway confirms, and the checkbox asks not to be asked again", () => {
  const onconfirm = vi.fn();
  openDialog({ onconfirm });
  button("Run anyway").click();
  expect(onconfirm).toHaveBeenLastCalledWith(false);
  document.querySelector<HTMLInputElement>('input[type="checkbox"]')!.click();
  button("Run anyway").click();
  expect(onconfirm).toHaveBeenLastCalledWith(true);
});

test("Tab stays inside the dialog and focus returns to where it was", () => {
  const outside = document.createElement("button");
  document.body.append(outside);
  outside.focus();
  openDialog();
  const run = button("Run anyway");
  run.focus();
  const tab = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  run.dispatchEvent(tab);
  expect(tab.defaultPrevented).toBe(true);
  expect(document.activeElement).toBe(document.querySelector('input[type="checkbox"]'));
  const back = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
  document.activeElement!.dispatchEvent(back);
  expect(document.activeElement).toBe(run);
  unmount(cmp!);
  cmp = undefined;
  expect(document.activeElement).toBe(outside);
});

test("BlockedBar toggle calls setGuardOffForSession and the toolbar shows Off until restart", async () => {
  vi.mocked(ipc.setGuardOffForSession).mockResolvedValue();
  cmp = mount(BlockedBar, { target: document.body, props: { connection: conn, findings } });
  flushSync();
  expect(document.body.textContent).toContain("Blocked: 2 dangerous statement(s). Nothing was run.");
  button("Turn off until I close the app").click();
  await settle();
  flushSync();
  expect(ipc.setGuardOffForSession).toHaveBeenCalledWith("c1", true);
  expect(connections.guardOff).toEqual(["c1"]);
  unmount(cmp);
  cmp = mount(PaneToolbar, {
    target: document.body,
    props: {
      connection: conn,
      database: "master",
      databases: ["master"],
      status: "idle",
      onrun: () => {},
      onstop: () => {},
      onreconnect: () => {},
      ondatabase: async () => {},
    },
  });
  flushSync();
  expect(document.body.textContent).toContain("Off until restart");
});

test("GuardMenu changes the level permanently with setConnectionGuard", async () => {
  vi.mocked(ipc.setConnectionGuard).mockResolvedValue("confirm");
  cmp = mount(GuardMenu, { target: document.body, props: { connection: conn, label: "Confirm" } });
  flushSync();
  button("Confirm").click();
  flushSync();
  const block = [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find((b) => b.textContent?.trim() === "Block")!;
  block.click();
  await settle();
  flushSync();
  expect(ipc.setConnectionGuard).toHaveBeenCalledWith("c1", "block");
  expect(connections.list[0].guard).toBe("block");
  expect(document.querySelector('[role="menu"]')).toBeNull();
});
