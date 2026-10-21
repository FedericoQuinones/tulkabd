import { expect, test } from "vitest";
import { visibleRange, windowOffsets } from "./virtual";

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
