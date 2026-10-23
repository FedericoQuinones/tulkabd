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

// ---- dragging tabs (pointer events) ----

const pointer = (type: string, x: number, y: number) =>
  new PointerEvent(type, { clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, cancelable: true });
const tabEl = (title: string) => [...document.querySelectorAll<HTMLElement>(".tab")].find((el) => el.querySelector(".lab")!.textContent === title)!;
const strip = (g: HTMLElement) => g.querySelector<HTMLElement>('[role="tablist"]')!;

/** jsdom has no layout: lays each group's tabs out 100 px wide from x = 0. */
function layOut() {
  for (const g of groupEls()) {
    g.querySelectorAll<HTMLElement>(".tab").forEach((el, i) => {
      el.getBoundingClientRect = () => ({ left: i * 100, width: 100, right: i * 100 + 100, top: 0, bottom: 36, height: 36, x: i * 100, y: 0, toJSON: () => ({}) });
    });
  }
}

/** Drags the tab titled `title` to x on `over` (the element under the pointer) and releases it. */
function dragTab(title: string, over: Element, x: number, release = true) {
  layOut();
  tabEl(title).querySelector('[role="tab"]')!.dispatchEvent(pointer("pointerdown", 20, 10));
  document.elementFromPoint = () => over;
  document.body.dispatchEvent(pointer("pointermove", x, 10));
  flushSync();
  if (release) {
    document.body.dispatchEvent(pointer("pointerup", x, 10));
    flushSync();
  }
}

/** Opens `n` query tabs in the active group and returns their titles (the untitled counter is never reset). */
function openTabs(n: number): string[] {
  return Array.from({ length: n }, () => {
    const id = tabs.open({ kind: "query" });
    return tabs.list.find((x) => x.id === id)!.title;
  });
}

/** Two groups side by side: [a, b] | [c, d]. */
function twoGroups(): string[] {
  const [a, b] = openTabs(2);
  tabs.splitActive("row");
  const c = tabs.active!.title;
  const [d] = openTabs(1);
  flushSync();
  return [a, b, c, d];
}

test("a tab dropped on another group's strip lands at the index of the x position", () => {
  const [a, b, c, d] = twoGroups();
  dragTab(a, strip(groupEls()[1]), 150);
  expect(groupEls().map(titles)).toEqual([[b], [c, a, d]]);
  expect(tabs.active?.title).toBe(a);

  dragTab(b, tabEl(c), 10);
  // Its group was left empty, so it went away.
  expect(groupEls().map(titles)).toEqual([[b, c, a, d]]);
});

test("a tab dragged along its own strip moves to where it is dropped", () => {
  const [a, b, c] = openTabs(3);
  flushSync();
  dragTab(a, strip(groupEls()[0]), 280);
  expect(groupEls().map(titles)).toEqual([[b, c, a]]);
  dragTab(a, strip(groupEls()[0]), 10);
  expect(groupEls().map(titles)).toEqual([[a, b, c]]);
  dragTab(a, strip(groupEls()[0]), 160);
  expect(groupEls().map(titles)).toEqual([[b, a, c]]);
});

test("the strip marks where the dragged tab would land", () => {
  const [a, b, c, d] = twoGroups();
  dragTab(a, strip(groupEls()[1]), 150, false);
  expect(tabEl(d).classList.contains("mark-before")).toBe(true);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  flushSync();
  expect(document.querySelectorAll(".mark-before, .mark-after")).toHaveLength(0);
  expect(groupEls().map(titles)).toEqual([[a, b], [c, d]]);
});

test("a tab dropped on another group's editor area goes last in that group", () => {
  const [a, b, c, d] = twoGroups();
  dragTab(a, groupEls()[1].querySelector(".cm-content")!, 300);
  expect(groupEls().map(titles)).toEqual([[b], [c, d, a]]);
});

test("a tab dropped outside any strip or group stays where it was", () => {
  const [a, b, c, d] = twoGroups();
  dragTab(a, document.body, 300);
  expect(groupEls().map(titles)).toEqual([[a, b], [c, d]]);
});
