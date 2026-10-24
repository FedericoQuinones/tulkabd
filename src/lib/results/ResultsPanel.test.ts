// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", async (original) => ({
  ...(await original<typeof import("../ipc")>()),
  fetchRows: vi.fn(),
  setResultView: vi.fn(),
  copyResults: vi.fn(),
}));

import * as ipc from "../ipc";
import type { Value } from "../ipc";
import { PaneState } from "../pane/pane.svelte";
import ResultsPanel from "./ResultsPanel.svelte";

const int = (n: number): Value => ({ kind: "int", value: String(n) });
const rows: Value[][] = [
  [int(1234), { kind: "text", value: "pedido" }],
  [int(7), { kind: "null" }],
];

// jsdom has no layout: the grid's size binding needs an observer that never reports.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let cmp: ReturnType<typeof mount> | undefined;
let pane: PaneState;

function open(status: PaneState["status"] = "idle") {
  pane.status = status;
  cmp = mount(ResultsPanel, { target: document.body, props: { pane, onjump: () => {} } });
  flushSync();
}

function close() {
  if (cmp) unmount(cmp);
  cmp = undefined;
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(ipc.fetchRows).mockResolvedValue(rows);
  vi.mocked(ipc.setResultView).mockResolvedValue(3);
  vi.mocked(ipc.copyResults).mockResolvedValue({ rows: 2, tokens: 0 });
  pane = new PaneState(`tab-${Math.random()}`);
  pane.runCount = 1;
  pane.results = [
    {
      columns: [
        { name: "id", typeName: "int" },
        { name: "estado", typeName: "nvarchar" },
      ],
      total: 20,
      truncated: false,
    },
  ];
});
afterEach(() => {
  close();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

const settle = async () => {
  for (let i = 0; i < 5; i++) await Promise.resolve();
  flushSync();
};
const filterField = () => document.querySelector<HTMLInputElement>('input[aria-label="Filter rows"]')!;
const header = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="columnheader"]')].find((h) => h.querySelector(".name")?.textContent === name)!;
const sortButton = (name: string) => header(name).querySelector<HTMLButtonElement>("button.sort")!;
const grid = () => document.querySelector<HTMLElement>('[role="grid"]')!;
const type = (text: string) => {
  const input = filterField();
  input.value = text;
  input.dispatchEvent(new Event("input", { bubbles: true }));
};

test("typing in Filter rows sends one view after the debounce and shows N of M rows", async () => {
  vi.useFakeTimers();
  open();
  type("p");
  type("pe");
  type("ped");
  vi.advanceTimersByTime(149);
  expect(ipc.setResultView).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  expect(ipc.setResultView).toHaveBeenCalledTimes(1);
  expect(ipc.setResultView).toHaveBeenCalledWith(pane.tabId, 0, { sort: null, text: "ped", columns: [] });
  await settle();
  expect(document.body.textContent).toContain("3 of 20 rows");
  expect(grid().getAttribute("aria-rowcount")).toBe("4");
});

test("clicking a header sorts ascending, descending, then back, with aria-sort", async () => {
  open();
  sortButton("id").click();
  await settle();
  expect(ipc.setResultView).toHaveBeenLastCalledWith(pane.tabId, 0, {
    sort: { column: 0, descending: false },
    text: null,
    columns: [],
  });
  expect(header("id").getAttribute("aria-sort")).toBe("ascending");
  expect(header("id").textContent).toContain("↑");
  // A sort alone hides no rows.
  expect(document.body.textContent).not.toContain("of 20 rows");
  sortButton("id").click();
  await settle();
  expect(header("id").getAttribute("aria-sort")).toBe("descending");
  sortButton("id").click();
  await settle();
  expect(header("id").hasAttribute("aria-sort")).toBe(false);
  expect(ipc.setResultView).toHaveBeenLastCalledWith(pane.tabId, 0, { sort: null, text: null, columns: [] });
});

test("while the run goes on the headers do not sort and the filter is off", async () => {
  open("running");
  expect(sortButton("id").disabled).toBe(true);
  sortButton("id").click();
  sortButton("id").dispatchEvent(new MouseEvent("click", { bubbles: true }));
  await settle();
  expect(ipc.setResultView).not.toHaveBeenCalled();
  expect(header("id").querySelector<HTMLButtonElement>("button.filter")!.disabled).toBe(true);
  expect(filterField().disabled).toBe(true);
});

test("a second click waits for the first view before it is sent", async () => {
  let release!: (n: number) => void;
  vi.mocked(ipc.setResultView).mockImplementationOnce(() => new Promise((r) => (release = r)));
  open();
  sortButton("id").click();
  sortButton("id").click();
  await settle();
  expect(ipc.setResultView).toHaveBeenCalledTimes(1);
  release(20);
  await settle();
  expect(ipc.setResultView).toHaveBeenCalledTimes(2);
  expect(vi.mocked(ipc.setResultView).mock.calls[1][2].sort).toEqual({ column: 0, descending: true });
});

test("the view comes back when the results are shown again", async () => {
  open();
  type("ped");
  // Leaving flushes the text that waits for the debounce.
  close();
  expect(ipc.setResultView).toHaveBeenCalledTimes(1);
  await settle();
  open();
  expect(filterField().value).toBe("ped");
  expect(document.body.textContent).toContain("3 of 20 rows");
});

test("a new run starts without the view", async () => {
  open();
  sortButton("id").click();
  await settle();
  close();
  pane.runCount = 2;
  open();
  expect(header("id").hasAttribute("aria-sort")).toBe(false);
});

test("Filter by this value filters on the text the cell shows, NULL with Is NULL", async () => {
  open();
  await settle();
  const cell = (row: number, col: number) => document.querySelector<HTMLElement>(`[role="gridcell"][data-row="${row}"][data-col="${col}"]`)!;
  expect(cell(0, 0).textContent?.trim()).toBe("1,234");
  cell(0, 0).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  flushSync();
  const item = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find((b) => b.textContent?.trim() === "Filter by this value")!;
  item.click();
  await settle();
  expect(ipc.setResultView).toHaveBeenLastCalledWith(pane.tabId, 0, {
    sort: null,
    text: null,
    columns: [{ kind: "equals", column: 0, text: "1,234" }],
  });
  expect(document.body.textContent).toContain("3 of 20 rows");

  cell(1, 1).dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  flushSync();
  [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find((b) => b.textContent?.trim() === "Filter by this value")!.click();
  await settle();
  expect(vi.mocked(ipc.setResultView).mock.calls.at(-1)![2].columns).toEqual([
    { kind: "equals", column: 0, text: "1,234" },
    { kind: "isNull", column: 1 },
  ]);
});

test("the column menu filters a column and clears it", async () => {
  open();
  header("estado").querySelector<HTMLButtonElement>("button.filter")!.click();
  flushSync();
  const item = (label: string) =>
    [...document.querySelectorAll<HTMLButtonElement>('[role="menu"] button')].find((b) => b.textContent?.trim().startsWith(label))!;
  item("Is not NULL").click();
  await settle();
  expect(vi.mocked(ipc.setResultView).mock.calls.at(-1)![2].columns).toEqual([{ kind: "notNull", column: 1 }]);

  header("estado").querySelector<HTMLButtonElement>("button.filter")!.click();
  flushSync();
  expect(item("Is not NULL").getAttribute("aria-checked")).toBe("true");
  item("Equals").click();
  await settle();
  const input = document.querySelector<HTMLInputElement>("#column-equals")!;
  input.value = "pedido";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  input.form!.requestSubmit();
  await settle();
  expect(vi.mocked(ipc.setResultView).mock.calls.at(-1)![2].columns).toEqual([{ kind: "equals", column: 1, text: "pedido" }]);

  header("estado").querySelector<HTMLButtonElement>("button.filter")!.click();
  flushSync();
  item("Clear filter").click();
  await settle();
  expect(vi.mocked(ipc.setResultView).mock.calls.at(-1)![2]).toEqual({ sort: null, text: null, columns: [] });
  expect(document.body.textContent).not.toContain("of 20 rows");
});

test("keys move the active cell, Ctrl+A selects all and Ctrl+C copies it", async () => {
  open();
  await settle();
  const key = (k: string, mods: KeyboardEventInit = {}) =>
    grid().dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...mods }));
  key("ArrowDown");
  flushSync();
  const activeId = grid().getAttribute("aria-activedescendant")!;
  expect(document.getElementById(activeId)?.dataset.row).toBe("0");
  key("ArrowRight", { shiftKey: true });
  flushSync();
  expect(document.querySelectorAll('[role="gridcell"][aria-selected="true"]').length).toBe(2);
  key("a", { ctrlKey: true });
  key("c", { ctrlKey: true });
  await settle();
  expect(ipc.copyResults).toHaveBeenCalledWith(pane.tabId, 0, "excel", { kind: "all" });
});

test("a failed view shows the error and the field goes back", async () => {
  vi.useFakeTimers();
  vi.mocked(ipc.setResultView).mockRejectedValue("busy");
  open();
  type("x");
  vi.advanceTimersByTime(150);
  await settle();
  expect(document.body.textContent).toContain("Could not sort or filter: busy");
  expect(filterField().value).toBe("");
});

test("Filter by this value from the keyboard gives the focus back to the grid", async () => {
  open();
  await settle();
  grid().focus();
  grid().dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
  flushSync();
  // The context-menu key fires `contextmenu` on the focused grid.
  grid().dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  flushSync();
  const item = document.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
  expect(document.activeElement).toBe(item);
  item.click();
  flushSync();
  await settle();
  expect(ipc.setResultView).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(grid());
});

test("nothing is copied while a view is on its way", async () => {
  let release!: (n: number) => void;
  vi.mocked(ipc.setResultView).mockImplementationOnce(() => new Promise((r) => (release = r)));
  open();
  await settle();
  sortButton("id").click();
  await settle();
  const copyButton = [...document.querySelectorAll<HTMLButtonElement>("button.ghost")].find((b) => b.textContent === "Copy")!;
  expect(copyButton.disabled).toBe(true);
  grid().dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true }));
  copyButton.click();
  await settle();
  expect(ipc.copyResults).not.toHaveBeenCalled();
  release(20);
  await settle();
  expect(copyButton.disabled).toBe(false);
  grid().dispatchEvent(new KeyboardEvent("keydown", { key: "c", ctrlKey: true, bubbles: true, cancelable: true }));
  await settle();
  expect(ipc.copyResults).toHaveBeenCalledTimes(1);
});

test("the row counter is one live region that stays and only changes its text", async () => {
  vi.useFakeTimers();
  open();
  const live = document.querySelector('[aria-live="polite"]')!;
  expect(live).not.toBeNull();
  expect(live.textContent).toBe("");
  type("ped");
  vi.advanceTimersByTime(150);
  await settle();
  expect(document.querySelector('[aria-live="polite"]')).toBe(live);
  expect(live.textContent).toBe("3 of 20 rows");
  type("");
  vi.advanceTimersByTime(150);
  await settle();
  expect(document.querySelector('[aria-live="polite"]')).toBe(live);
  expect(live.textContent).toBe("");
});
