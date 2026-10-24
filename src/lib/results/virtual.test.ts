import { expect, test } from "vitest";
import { anchoredScrollTop, headerHeight, rowHeight, visibleRange, windowOffsets } from "./virtual";

test("visible range with overscan", () => expect(visibleRange(340, 680, 34, 1000, 10)).toEqual({ start: 0, end: 40 }));
test("range clamps at end", () =>
  expect(visibleRange(34 * 995, 680, 34, 1000, 10)).toEqual({ start: 985, end: 1000 }));
test("empty result gives an empty range", () => expect(visibleRange(0, 680, 34, 0)).toEqual({ start: 0, end: 0 }));
test("windows are aligned to multiples of 200", () => {
  expect(windowOffsets(0, 40)).toEqual([0]);
  expect(windowOffsets(150, 420)).toEqual([0, 200, 400]);
  expect(windowOffsets(200, 400)).toEqual([200]);
  expect(windowOffsets(5, 5)).toEqual([]);
});

test("rowHeight scales with the zoom and the window follows it", () => {
  expect(rowHeight(100)).toBe(34);
  expect(rowHeight(150)).toBe(Math.round(rowHeight(100) * 1.5));
  expect(rowHeight(70)).toBe(24);
  // At 150 % a row is 51 px: scrolled 510 px, row 10 is the first one shown.
  expect(visibleRange(510, 102, rowHeight(150), 1000, 0)).toEqual({ start: 10, end: 12 });
  // At 70 % a row is 24 px.
  expect(visibleRange(240, 96, rowHeight(70), 1000, 0)).toEqual({ start: 10, end: 14 });
});

test("a zoom change keeps the first visible row where it was", () => {
  // Row 10 and a half is at the top at 100 %; at 150 % it is still at the top.
  expect(anchoredScrollTop(34 * 10.5, 34, 51)).toBe(51 * 10.5);
  expect(anchoredScrollTop(0, 34, 24)).toBe(0);
  expect(anchoredScrollTop(340, 34, 24)).toBe(240);
});

test("the grid header grows with the zoom", () => {
  expect(headerHeight(100)).toBe(36);
  expect(headerHeight(150)).toBe(54);
  expect(headerHeight(70)).toBe(25);
});
