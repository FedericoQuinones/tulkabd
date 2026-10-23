// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
  Channel: class {},
}));

import type { ConnectionConfig } from "../ipc";
import { connections } from "../connections/store.svelte";
import { tabs } from "../tabs/tabs.svelte";
import { groups } from "./layout";
import Workspace from "./Workspace.svelte";

let cmp: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  connections.list = [];
  cmp = mount(Workspace, { target: document.body });
  flushSync();
});
afterEach(async () => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  connections.list = [];
  await tabs.reset();
});

const groupEls = () => [...document.querySelectorAll<HTMLElement>(".group")];
const titles = (g: HTMLElement) => [...g.querySelectorAll('[role="tab"]')].map((x) => x.textContent!.trim());

test("renders two groups side by side, each with its tabs and its active editor", () => {
  tabs.open({ kind: "query" });
  tabs.open({ kind: "query" });
  tabs.splitActive("row");
  flushSync();

  const split = document.querySelector<HTMLElement>(".split")!;
  expect(split.classList.contains("row")).toBe(true);
  expect(groupEls()).toHaveLength(2);
  expect(groupEls().map(titles)).toEqual([["untitled-1", "untitled-2"], ["untitled-3"]]);
  for (const g of groupEls()) expect(g.querySelectorAll(".cm-editor")).toHaveLength(1);
  expect(groupEls().map((g) => g.classList.contains("active"))).toEqual([false, true]);
  expect(split.querySelectorAll('[role="separator"]')).toHaveLength(1);
});

test("clicking a tab in another group makes that group active", () => {
  tabs.open({ kind: "query" });
  const first = tabs.activeGroupId;
  tabs.splitActive("row");
  flushSync();
  groupEls()[0].querySelector<HTMLButtonElement>('[role="tab"]')!.click();
  flushSync();
  expect(tabs.activeGroupId).toBe(first);
  expect(groupEls().map((g) => g.classList.contains("active"))).toEqual([true, false]);
});

test("the split buttons split their own group and are disabled at four groups", () => {
  tabs.open({ kind: "query" });
  flushSync();
  const button = (g: HTMLElement, label: string) => g.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
  button(groupEls()[0], "Split down").click();
  flushSync();
  expect(tabs.layout).toMatchObject({ kind: "split", dir: "column" });
  button(groupEls()[0], "Split right").click();
  flushSync();
  button(groupEls()[0], "Split right").click();
  flushSync();
  expect(groups(tabs.layout)).toHaveLength(4);
  for (const g of groupEls()) {
    expect(button(g, "Split right").disabled).toBe(true);
    expect(button(g, "Split down").disabled).toBe(true);
  }
});

test("each group's left stripe takes the environment color of its active tab", () => {
  connections.list = [{ id: "c1", name: "Prod", passwordSaved: false, environment: "production" } as ConnectionConfig];
  tabs.open({ kind: "query" });
  tabs.splitActive("row");
  tabs.open({ kind: "structure", connectionId: "c1", database: "Sales", schema: "dbo", name: "T" });
  flushSync();
  const [left, right] = groupEls();
  expect(right.style.getPropertyValue("--env")).toBe("#e5566f");
  expect(left.style.getPropertyValue("--env")).toBe("var(--control)");
});

test("the separator resizes the groups from the keyboard, keeping 15 % per group", () => {
  tabs.open({ kind: "query" });
  tabs.splitActive("row");
  flushSync();
  const sep = document.querySelector<HTMLElement>('[role="separator"]')!;
  const key = (k: string) => sep.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true }));
  key("ArrowRight");
  flushSync();
  expect(tabs.layout.kind === "split" && tabs.layout.sizes[0]).toBeCloseTo(0.55);
  for (let i = 0; i < 20; i++) key("ArrowRight");
  flushSync();
  expect(tabs.layout.kind === "split" && tabs.layout.sizes[0]).toBeCloseTo(0.85);
  key("ArrowDown");
  flushSync();
  expect(tabs.layout.kind === "split" && tabs.layout.sizes[0]).toBeCloseTo(0.85);
});

test("after a group closes, the group the store picks stays active even though remounted editors focus", () => {
  // [[A / B] column, C]: A and B stacked on the left, C on the right.
  tabs.open({ kind: "query" });
  const a = tabs.activeGroupId;
  tabs.splitActive("row");
  const c = tabs.activeGroupId;
  tabs.focusGroup(a);
  tabs.splitActive("column");
  const b = tabs.activeGroupId;
  flushSync();
  expect(tabs.layout).toMatchObject({ kind: "split", dir: "row", children: [{ kind: "split", dir: "column" }, { kind: "group", id: c }] });

  tabs.close(tabs.activeIn(b)!.id);
  flushSync();
  expect(groups(tabs.layout)).toEqual([a, c]);
  expect(tabs.activeGroupId).toBe(c);
  expect(groupEls().map((g) => g.classList.contains("active"))).toEqual([false, true]);
});
