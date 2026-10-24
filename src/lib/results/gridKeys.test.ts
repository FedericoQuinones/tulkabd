// @vitest-environment jsdom
import { expect, test } from "vitest";
import { gridKey, type GridKeyState } from "./gridKeys";

const key = (k: string, mods: Partial<KeyboardEventInit> = {}) => new KeyboardEvent("keydown", { key: k, ...mods });
const at = (row: number, col: number) => ({ row, col });
const cell = (row: number, col: number) => ({ kind: "cells" as const, anchor: at(row, col), focus: at(row, col) });
const state = (s: Partial<GridKeyState> = {}): GridKeyState => ({
  active: at(0, 0),
  rows: 100,
  cols: 5,
  selection: cell(0, 0),
  page: 10,
  ...s,
});

test("arrows move the active cell and select it", () => {
  expect(gridKey(key("ArrowDown"), state())).toEqual({ active: at(1, 0), selection: cell(1, 0) });
  expect(gridKey(key("ArrowRight"), state())).toEqual({ active: at(0, 1), selection: cell(0, 1) });
  expect(gridKey(key("ArrowUp"), state({ active: at(3, 2) }))).toEqual({ active: at(2, 2), selection: cell(2, 2) });
  expect(gridKey(key("ArrowLeft"), state({ active: at(3, 2) }))).toEqual({ active: at(3, 1), selection: cell(3, 1) });
});

test("arrow down on the last row stays in range", () => {
  expect(gridKey(key("ArrowDown"), state({ active: at(99, 4), selection: cell(99, 4) }))).toEqual({
    active: at(99, 4),
    selection: cell(99, 4),
  });
  expect(gridKey(key("ArrowUp"), state())).toEqual({ active: at(0, 0), selection: cell(0, 0) });
  expect(gridKey(key("ArrowRight"), state({ active: at(0, 4) }))).toEqual({ active: at(0, 4), selection: cell(0, 4) });
});

test("shift and an arrow extend the selection from one cell to two", () => {
  expect(gridKey(key("ArrowRight", { shiftKey: true }), state())).toEqual({
    active: at(0, 1),
    selection: { kind: "cells", anchor: at(0, 0), focus: at(0, 1) },
  });
  // Again: the anchor stays where the selection started.
  expect(
    gridKey(
      key("ArrowDown", { shiftKey: true }),
      state({ active: at(0, 1), selection: { kind: "cells", anchor: at(0, 0), focus: at(0, 1) } }),
    ),
  ).toEqual({ active: at(1, 1), selection: { kind: "cells", anchor: at(0, 0), focus: at(1, 1) } });
});

test("shift and up or down extend a row selection", () => {
  expect(
    gridKey(key("ArrowDown", { shiftKey: true }), state({ active: at(4, 0), selection: { kind: "rows", anchor: 4, focus: 4 } })),
  ).toEqual({ active: at(5, 0), selection: { kind: "rows", anchor: 4, focus: 5 } });
});

test("without an active cell the first key starts at the first cell", () => {
  expect(gridKey(key("ArrowDown"), state({ active: null, selection: null }))).toEqual({
    active: at(0, 0),
    selection: cell(0, 0),
  });
});

test("Home and End go to the ends of the row, with Ctrl to the ends of the grid", () => {
  const s = state({ active: at(7, 2), selection: cell(7, 2) });
  expect(gridKey(key("Home"), s)).toEqual({ active: at(7, 0), selection: cell(7, 0) });
  expect(gridKey(key("End"), s)).toEqual({ active: at(7, 4), selection: cell(7, 4) });
  expect(gridKey(key("Home", { ctrlKey: true }), s)).toEqual({ active: at(0, 0), selection: cell(0, 0) });
  expect(gridKey(key("End", { ctrlKey: true }), s)).toEqual({ active: at(99, 4), selection: cell(99, 4) });
  expect(gridKey(key("End", { metaKey: true, shiftKey: true }), s)).toEqual({
    active: at(99, 4),
    selection: { kind: "cells", anchor: at(7, 2), focus: at(99, 4) },
  });
});

test("PageUp and PageDown move by the visible page", () => {
  const s = state({ active: at(15, 1), selection: cell(15, 1), page: 12 });
  expect(gridKey(key("PageDown"), s)).toEqual({ active: at(27, 1), selection: cell(27, 1) });
  expect(gridKey(key("PageUp"), s)).toEqual({ active: at(3, 1), selection: cell(3, 1) });
  expect(gridKey(key("PageUp"), state({ active: at(5, 1), page: 12 }))).toEqual({ active: at(0, 1), selection: cell(0, 1) });
  expect(gridKey(key("PageDown"), state({ active: at(95, 1), page: 12 }))).toEqual({ active: at(99, 1), selection: cell(99, 1) });
});

test("Ctrl+A selects everything and keeps the active cell", () => {
  expect(gridKey(key("a", { ctrlKey: true }), state({ active: at(3, 3) }))).toEqual({
    active: at(3, 3),
    selection: { kind: "all" },
  });
  expect(gridKey(key("A", { metaKey: true }), state())).toEqual({ active: at(0, 0), selection: { kind: "all" } });
});

test("Ctrl+C copies", () => {
  expect(gridKey(key("c", { ctrlKey: true }), state())).toBe("copy");
  expect(gridKey(key("c", { metaKey: true }), state({ active: null, selection: null }))).toBe("copy");
});

test("other keys, Alt chords and handled events are left alone", () => {
  expect(gridKey(key("x"), state())).toBeNull();
  expect(gridKey(key("a"), state())).toBeNull();
  expect(gridKey(key("ArrowDown", { altKey: true }), state())).toBeNull();
  expect(gridKey(key("c", { ctrlKey: true, altKey: true }), state())).toBeNull();
  const handled = new KeyboardEvent("keydown", { key: "ArrowDown", cancelable: true });
  handled.preventDefault();
  expect(gridKey(handled, state())).toBeNull();
});

test("an empty grid does not move", () => {
  expect(gridKey(key("ArrowDown"), state({ active: null, selection: null, rows: 0 }))).toBeNull();
  expect(gridKey(key("ArrowDown"), state({ active: null, selection: null, cols: 0 }))).toBeNull();
});
