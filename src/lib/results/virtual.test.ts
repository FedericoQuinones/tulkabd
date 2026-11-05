import { expect, test } from "vitest";
import {
  anchoredScrollTop,
  headerHeight,
  idColumnWidth,
  isScaled,
  MAX_SPACER,
  maxRealTop,
  maxVirtualTop,
  resyncedTop,
  revealRowTop,
  rowHeight,
  spacerHeight,
  toRealTop,
  toVirtualTop,
  visibleColumns,
  visibleRange,
  windowOffsets,
} from "./virtual";

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

const small = { total: 1000, rowH: 34, bodyH: 600 };
const huge = { total: 1_000_000, rowH: 68, bodyH: 800 };

test("below the cap the scroll mapping is the identity", () => {
  expect(isScaled(small)).toBe(false);
  expect(spacerHeight(small)).toBe(34000);
  expect(toVirtualTop(1234, small)).toBe(1234);
  expect(toRealTop(1234, small)).toBe(1234);
});

test("above the cap the spacer is capped and the ends still map to the ends", () => {
  expect(isScaled(huge)).toBe(true);
  expect(spacerHeight(huge)).toBe(MAX_SPACER);
  expect(toVirtualTop(0, huge)).toBe(0);
  expect(toVirtualTop(maxRealTop(huge), huge)).toBe(maxVirtualTop(huge));
  // At the very end the last row is the last one shown.
  const r = visibleRange(maxVirtualTop(huge), huge.bodyH, huge.rowH, huge.total, 0);
  expect(r.end).toBe(huge.total);
  expect(toRealTop(maxVirtualTop(huge), huge)).toBeCloseTo(maxRealTop(huge), 3);
});

test("scaled mapping round-trips", () => {
  for (const v of [0, 1e6, 3.3e7, 6.7e7]) expect(toVirtualTop(toRealTop(v, huge), huge)).toBeCloseTo(v, 3);
});

test("revealRowTop scrolls the least needed", () => {
  expect(revealRowTop(5, 0, small)).toBe(0);
  expect(revealRowTop(100, 0, small)).toBe(100 * 34 + 34 - 600);
  expect(revealRowTop(2, 340, small)).toBe(68);
  expect(revealRowTop(999_999, 0, huge)).toBe(maxVirtualTop(huge));
});

test("the row-number column grows with the digits and the zoom", () => {
  expect(idColumnWidth(999, 100)).toBe(56);
  expect(idColumnWidth(1_000_000, 100)).toBeGreaterThanOrEqual(22 + 7 * 7.2);
  expect(idColumnWidth(1_000_000, 200)).toBeGreaterThan(idColumnWidth(1_000_000, 100));
  expect(idColumnWidth(100_000, 100)).toBeGreaterThan(idColumnWidth(1000, 100));
  expect(idColumnWidth(0, 100)).toBe(56);
});

test("only the columns near the viewport are rendered", () => {
  const w = () => 100;
  expect(visibleColumns(1000, w, 56, 0, 500, 0)).toEqual({ start: 0, end: 5, offset: 0 });
  // 5000 px in: columns whose right edge is past 5000 start at index 49 (left 4956).
  expect(visibleColumns(1000, w, 56, 5000, 500, 0)).toEqual({ start: 49, end: 55, offset: 4900 });
  expect(visibleColumns(1000, w, 56, 5000, 500, 300)).toEqual({ start: 46, end: 58, offset: 4600 });
  expect(visibleColumns(3, w, 56, 0, 0)).toEqual({ start: 0, end: 3, offset: 0 });
  expect(visibleColumns(3, w, 56, 10_000, 500, 0)).toEqual({ start: 3, end: 3, offset: 300 });
});

test("streaming more rows keeps the first row, or the end if the grid was at the end", () => {
  const a = { total: 1_000_000, rowH: 34, bodyH: 800 };
  const b = { total: 2_000_000, rowH: 34, bodyH: 800 };
  expect(resyncedTop(5e6, a, b)).toBe(5e6);
  expect(resyncedTop(maxVirtualTop(a), a, b)).toBe(maxVirtualTop(b));
  // The same row sits at a different real scrollTop once the factor changed.
  expect(toRealTop(5e6, b)).not.toBeCloseTo(toRealTop(5e6, a), 0);
  expect(resyncedTop(9e9, a, a)).toBe(maxVirtualTop(a));
});
