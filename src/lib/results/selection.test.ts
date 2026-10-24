import { expect, test } from "vitest";
import { isSelected, toIpcSelection } from "./selection";

test("no selection copies everything", () => expect(toIpcSelection(null)).toEqual({ kind: "all" }));
test("rows are normalized", () =>
  expect(toIpcSelection({ kind: "rows", anchor: 9, focus: 3 })).toEqual({ kind: "rows", from: 3, to: 9 }));
test("single cell", () =>
  expect(toIpcSelection({ kind: "cells", anchor: { row: 4, col: 2 }, focus: { row: 4, col: 2 } })).toEqual({
    kind: "cells",
    rows: [4, 4],
    cols: [2, 2],
  }));
test("cell range in any direction", () =>
  expect(toIpcSelection({ kind: "cells", anchor: { row: 8, col: 1 }, focus: { row: 2, col: 3 } })).toEqual({
    kind: "cells",
    rows: [2, 8],
    cols: [1, 3],
  }));
test("isSelected", () => {
  const s = { kind: "cells", anchor: { row: 1, col: 1 }, focus: { row: 2, col: 2 } } as const;
  expect(isSelected(s, 2, 1)).toBe(true);
  expect(isSelected(s, 3, 1)).toBe(false);
  expect(isSelected({ kind: "rows", anchor: 1, focus: 1 }, 1, 9)).toBe(true);
  expect(isSelected(null, 0, 0)).toBe(false);
});
test("select all copies everything and covers every cell", () => {
  expect(toIpcSelection({ kind: "all" })).toEqual({ kind: "all" });
  expect(isSelected({ kind: "all" }, 70, 3)).toBe(true);
});
