// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => (cmd.startsWith("list_") ? [] : null)),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ onCloseRequested: vi.fn(async () => () => {}) }),
}));
type DropHandler = (e: { payload: string[] }) => void;
const drop = vi.hoisted(() => ({ handler: null as DropHandler | null }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, h: DropHandler) => {
    if (name === "sql-files-dropped") drop.handler = h;
    return () => {
      if (name === "sql-files-dropped") drop.handler = null;
    };
  }),
}));

import { invoke } from "@tauri-apps/api/core";
import App from "./App.svelte";
import type { ConnectionConfig } from "./lib/ipc";
import { connections } from "./lib/connections/store.svelte";
import { AUTOSAVE_MS, tabs } from "./lib/tabs/tabs.svelte";
import { palette } from "./lib/palette/palette.svelte";
import { DEFAULT_PREFERENCES, prefs } from "./lib/settings/prefs.svelte";

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

const gear = () => document.querySelector<HTMLButtonElement>('nav button[aria-label="Settings"]')!;

test("the gear opens Settings in place of the workspace, and Escape returns with the focus on the gear", async () => {
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
  expect(gear().getAttribute("aria-pressed")).toBe("false");
  expect(document.querySelector("h1")).toBeNull();

  gear().click();
  flushSync();
  expect(document.querySelector("h1")?.textContent).toBe("Settings");
  expect(gear().getAttribute("aria-pressed")).toBe("true");
  // The rail stays, the tabs are kept (hidden), and nothing else changes.
  expect(document.querySelector("nav.rail")).not.toBeNull();
  expect(document.querySelector<HTMLElement>(".ws")!.hidden).toBe(true);

  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  flushSync();
  await new Promise((r) => setTimeout(r, 0));
  expect(document.querySelector("h1")).toBeNull();
  expect(document.querySelector<HTMLElement>(".ws")!.hidden).toBe(false);
  expect(gear().getAttribute("aria-pressed")).toBe("false");
  expect(document.activeElement).toBe(gear());

  // The gear also closes it.
  gear().click();
  flushSync();
  gear().click();
  flushSync();
  expect(document.querySelector("h1")).toBeNull();
});

test("with Settings open, the search icon closes it and opens the palette", async () => {
  await new Promise((r) => setTimeout(r, 0));
  gear().click();
  flushSync();
  document.querySelector<HTMLButtonElement>('nav button[aria-label="Search objects"]')!.click();
  flushSync();
  await new Promise((r) => setTimeout(r, 0));
  expect(document.querySelector("h1")).toBeNull();
  expect(document.querySelector<HTMLElement>(".ws")!.hidden).toBe(false);
  expect(palette.isOpen).toBe(true);
  palette.hide();
});

test("with Settings open, opening a tab from anywhere closes it", async () => {
  await new Promise((r) => setTimeout(r, 0));
  gear().click();
  flushSync();
  expect(document.querySelector("h1")).not.toBeNull();
  tabs.open({ kind: "structure", connectionId: "c1", database: "Sales", schema: "dbo", name: "Orders" });
  flushSync();
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
  expect(document.querySelector("h1")).toBeNull();
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

test("a saved session that cannot be restored is kept until the user edits something", async () => {
  unmount(cmp!);
  await tabs.reset();
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  const autosave = vi.spyOn(tabs, "startAutosave");
  const start = async (load: () => Promise<unknown>) => {
    autosave.mockClear();
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) =>
      cmd === "load_session" ? load() : defaultInvoke(cmd, args as never),
    );
    cmp = mount(App, { target: document.body });
    flushSync();
    await new Promise((r) => setTimeout(r, 0));
    const opts = autosave.mock.calls[0][1];
    unmount(cmp);
    cmp = undefined;
    await tabs.reset();
    return opts?.holdUntilEdit === true;
  };
  try {
    expect(await start(async () => JSON.stringify({ version: 99, tabs: [] }))).toBe(true);
    expect(await start(async () => "{ broken")).toBe(true);
    expect(await start(async () => Promise.reject("internal error"))).toBe(true);
    expect(await start(async () => null)).toBe(false);
    const ok = { version: 1, layout: { kind: "group", id: "g1" }, activeGroupId: "g1", groups: { g1: { tabIds: [], activeId: null } }, tabs: [] };
    expect(await start(async () => JSON.stringify(ok))).toBe(false);
  } finally {
    autosave.mockRestore();
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});

test("with restore off the saved session is neither restored nor replaced until the user edits", async () => {
  unmount(cmp!);
  await tabs.reset();
  const session = {
    version: 1,
    layout: { kind: "group", id: "g1" },
    activeGroupId: "g1",
    groups: { g1: { tabIds: ["t1"], activeId: "t1" } },
    tabs: [{ id: "t1", kind: "query", title: "untitled-9", connectionId: null, database: null, text: "SELECT kept", dirty: false }],
  };
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  const autosave = vi.spyOn(tabs, "startAutosave");
  const start = async (restoreSession: boolean) => {
    autosave.mockClear();
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === "get_preferences") return { ...DEFAULT_PREFERENCES, restoreSession };
      if (cmd === "load_session") return JSON.stringify(session);
      return defaultInvoke(cmd, args as never);
    });
    cmp = mount(App, { target: document.body });
    flushSync();
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    const titles = [...document.querySelectorAll('.group [role="tab"]')].map((x) => x.textContent!.trim());
    const hold = autosave.mock.calls[0][1]?.holdUntilEdit === true;
    unmount(cmp);
    cmp = undefined;
    await tabs.reset();
    return { titles, hold };
  };
  try {
    expect(await start(false)).toEqual({ titles: [], hold: true });
    expect(await start(true)).toEqual({ titles: ["untitled-9"], hold: false });
  } finally {
    autosave.mockRestore();
    prefs.value = structuredClone(DEFAULT_PREFERENCES);
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});

test("with restore off, only a real edit lets the autosave replace the saved session", async () => {
  unmount(cmp!);
  await tabs.reset();
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "get_preferences") return { ...DEFAULT_PREFERENCES, restoreSession: false };
    if (cmd === "load_session") return JSON.stringify({ version: 1, layout: { kind: "group", id: "g1" }, activeGroupId: "g1", groups: { g1: { tabIds: [], activeId: null } }, tabs: [] });
    return defaultInvoke(cmd, args as never);
  });
  const saves = () => vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "save_session").length;
  try {
    cmp = mount(App, { target: document.body });
    flushSync();
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    vi.mocked(invoke).mockClear();
    // Opening an empty tab is not an edit.
    tabs.newTab();
    flushSync();
    await new Promise((r) => setTimeout(r, AUTOSAVE_MS + 200));
    expect(saves()).toBe(0);
    tabs.active!.pane!.edited();
    flushSync();
    await new Promise((r) => setTimeout(r, AUTOSAVE_MS + 200));
    expect(saves()).toBe(1);
  } finally {
    prefs.value = structuredClone(DEFAULT_PREFERENCES);
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});

test("the startup probe fires only once the restored workspace is painted", async () => {
  unmount(cmp!);
  await tabs.reset();
  let resolveSession: (json: string | null) => void = () => {};
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) =>
    cmd === "load_session" ? new Promise((r) => (resolveSession = r)) : defaultInvoke(cmd, args as never),
  );
  vi.mocked(invoke).mockClear();
  const probed = () => vi.mocked(invoke).mock.calls.some(([cmd]) => cmd === "startup_probe");
  try {
    cmp = mount(App, { target: document.body });
    flushSync();
    await new Promise((r) => setTimeout(r, 50));
    expect(probed()).toBe(false);
    resolveSession(null);
    await new Promise((r) => setTimeout(r, 50));
    expect(document.querySelector(".group")).not.toBeNull();
    expect(probed()).toBe(true);
  } finally {
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});

test("drop event opens the accepted files", async () => {
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "read_sql_file") {
      const path = (args as { path: string }).path;
      return { path, name: path.slice(1), text: "SELECT 1", encoding: "utf8Bom", lineEnding: "crlf" };
    }
    return defaultInvoke(cmd, args as never);
  });
  try {
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    vi.mocked(invoke).mockClear();
    // The backend already allowed these paths; the UI only opens them.
    drop.handler!({ payload: ["/a.sql"] });
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    const calls = vi.mocked(invoke).mock.calls.map(([cmd, args]) => [cmd, args]);
    expect(calls).toEqual([["read_sql_file", { path: "/a.sql" }]]);
    const titles = [...document.querySelectorAll('.group [role="tab"]')].map((x) => x.textContent!.trim());
    expect(titles).toEqual(["a.sql"]);
  } finally {
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});

test("Ctrl+O shows the open dialog; a file that cannot be read is reported until dismissed", async () => {
  const defaultInvoke = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "pick_sql_files") return ["/bin.sql"];
    if (cmd === "read_sql_file") throw "not_text";
    return defaultInvoke(cmd, args as never);
  });
  try {
    await new Promise((r) => setTimeout(r, 0));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "o", code: "KeyO", ctrlKey: true, bubbles: true }));
    await new Promise((r) => setTimeout(r, 0));
    flushSync();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith("pick_sql_files", { filterName: "SQL files" });
    const alert = document.querySelector<HTMLElement>(".file-error")!;
    expect(alert.textContent).toContain("Could not open bin.sql: it is not a text file");
    alert.querySelector<HTMLButtonElement>('button[aria-label="Dismiss"]')!.click();
    flushSync();
    expect(document.querySelector(".file-error")).toBeNull();
  } finally {
    vi.mocked(invoke).mockImplementation(defaultInvoke);
  }
});
