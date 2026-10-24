import type { Selection } from "../ipc";

/** Grid selection: a cell rectangle (anchor to focus), whole rows or everything. Indexes are 0-based visible rows. */
export type GridSelection =
  | { kind: "cells"; anchor: { row: number; col: number }; focus: { row: number; col: number } }
  | { kind: "rows"; anchor: number; focus: number }
  | { kind: "all" };

const ordered = (a: number, b: number): [number, number] => (a <= b ? [a, b] : [b, a]);

/** Maps the grid selection to the `copy_results` selection (inclusive ranges); none means everything. */
export function toIpcSelection(sel: GridSelection | null): Selection {
  if (!sel || sel.kind === "all") return { kind: "all" };
  if (sel.kind === "rows") {
    const [from, to] = ordered(sel.anchor, sel.focus);
    return { kind: "rows", from, to };
  }
  return {
    kind: "cells",
    rows: ordered(sel.anchor.row, sel.focus.row),
    cols: ordered(sel.anchor.col, sel.focus.col),
  };
}

/** True when the cell is inside the selection. */
export function isSelected(sel: GridSelection | null, row: number, col: number): boolean {
  if (!sel) return false;
  if (sel.kind === "all") return true;
  const s = toIpcSelection(sel);
  if (s.kind === "rows") return row >= s.from && row <= s.to;
  if (s.kind === "cells") return row >= s.rows[0] && row <= s.rows[1] && col >= s.cols[0] && col <= s.cols[1];
  return false;
}
