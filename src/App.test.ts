// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => (cmd.startsWith("list_") ? [] : null)),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import { invoke } from "@tauri-apps/api/core";
import App from "./App.svelte";
import type { ConnectionConfig } from "./lib/ipc";
import { connections } from "./lib/connections/store.svelte";
import { tabs } from "./lib/tabs/tabs.svelte";

let cmp: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  localStorage.clear();
  cmp = mount(App, { target: document.body });
  flushSync();
});
afterEach(async () => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  await tabs.reset();
});

const rail = () => document.querySelector<HTMLButtonElement>('nav button[aria-label="Explorer"]')!;

test("the rail button hides the explorer and the workspace takes its column", () => {
  const cols = document.querySelector(".cols")!;
  expect(document.querySelector("aside.explorer")).not.toBeNull();
  expect(rail().getAttribute("aria-current")).toBe("true");
  expect(cols.classList.contains("no-explorer")).toBe(false);

  rail().click();
  flushSync();
  expect(document.querySelector("aside.explorer")).toBeNull();
  expect(rail().hasAttribute("aria-current")).toBe(false);
  // With no aside, a 3-track template would leave the workspace in the `auto` track.
  expect(cols.classList.contains("no-explorer")).toBe(true);
  expect(cols.children).toHaveLength(2);

  rail().click();
  flushSync();
  expect(document.querySelector("aside.explorer")).not.toBeNull();
  expect(cols.classList.contains("no-explorer")).toBe(false);
});

test("a failed tab connect is shown in the workspace while the explorer is hidden", async () => {
  rail().click();
  flushSync();
  expect(document.querySelector("aside.explorer")).toBeNull();

  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "open_session") throw "login failed";
    return defaultInvoke(cmd, args as never);
  });
  connections.list = [{ id: "c1", name: "Local", passwordSaved: true, environment: "local" } as ConnectionConfig];
  try {
    tabs.open({ kind: "query", connectionId: "c1" });
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    const alert = document.querySelector('main.workspace [role="alert"]');
    expect(alert?.textContent).toContain("login failed");
  } finally {
    vi.mocked(invoke).mockImplementation(defaultInvoke);
    connections.list = [];
  }
});

test("the second queued password prompt starts with an empty field", async () => {
  connections.list = [
    { id: "p1", name: "One", passwordSaved: false, environment: "local" } as ConnectionConfig,
    { id: "p2", name: "Two", passwordSaved: false, environment: "local" } as ConnectionConfig,
  ];
  try {
    tabs.open({ kind: "query", connectionId: "p1" });
    tabs.open({ kind: "query", connectionId: "p2" });
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    const field = () => document.querySelector<HTMLInputElement>("#pw-input")!;
    field().value = "secret-for-one";
    field().dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    document.querySelector<HTMLButtonElement>('[role="dialog"] button.ghost')!.click();
    flushSync();
    expect(document.querySelector("#pw-title")?.textContent).toContain("Two");
    expect(field().value).toBe("");
  } finally {
    connections.list = [];
  }
});
