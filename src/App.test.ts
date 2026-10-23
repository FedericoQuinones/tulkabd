// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => (cmd.startsWith("list_") ? [] : null)),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: vi.fn(async () => () => {}) }),
}));

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

test("the saved session is restored before the workspace renders, without connecting", async () => {
  unmount(cmp!);
  await tabs.reset();
  const session = {
    version: 1,
    layout: { kind: "group", id: "g1" },
    activeGroupId: "g1",
    groups: { g1: { tabIds: ["t1", "t2"], activeId: "t2" } },
    tabs: [
      { id: "t1", kind: "query", title: "untitled-1", connectionId: null, database: null, text: "SELECT 1", dirty: false },
      { id: "t2", kind: "query", title: "untitled-7", connectionId: "gone", database: "Sales", text: "SELECT kept", dirty: false },
    ],
  };
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) =>
    cmd === "load_session" ? JSON.stringify(session) : defaultInvoke(cmd, args as never),
  );
  try {
    cmp = mount(App, { target: document.body });
    flushSync();
    expect(document.querySelector(".group")).toBeNull();
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    const titles = [...document.querySelectorAll('.group [role="tab"]')].map((x) => x.textContent!.trim());
    expect(titles).toEqual(["untitled-1", "untitled-7"]);
    expect(document.querySelector(".cm-content")?.textContent).toBe("SELECT kept");
    expect(document.querySelector('main.workspace [role="alert"]')?.textContent).toContain("no longer exists");
    expect(vi.mocked(invoke).mock.calls.some(([cmd]) => cmd === "open_session" || cmd === "connect_explorer")).toBe(false);
  } finally {
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});
