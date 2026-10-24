import type { GridSelection } from "./selection";

export interface Cell {
  row: number;
  col: number;
}

export interface GridKeyState {
  active: Cell | null;
  /** Visible rows. */
  rows: number;
  cols: number;
  selection: GridSelection | null;
  /** Rows that fit in the viewport, for PageUp and PageDown. */
  page?: number;
}

export type GridKeyResult = { active: Cell | null; selection: GridSelection | null } | "copy" | null;

const clamp = (n: number, max: number) => Math.max(0, Math.min(n, max));

/**
 * What a keydown does in the result grid: arrows move the active cell (Shift extends the
 * selection), Home/End go to the ends of the row and with Ctrl to the ends of the grid,
 * PageUp/PageDown move by `page` rows, Ctrl+A selects everything and Ctrl+C copies (Cmd on
 * macOS too). Null for keys the grid leaves alone.
 */
export function gridKey(e: KeyboardEvent, state: GridKeyState): GridKeyResult {
  if (e.defaultPrevented || e.altKey) return null;
  const mod = e.ctrlKey || e.metaKey;
  const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (mod && !e.shiftKey && key === "a") return { active: state.active, selection: { kind: "all" } };
  if (mod && !e.shiftKey && key === "c") return "copy";

  // Ctrl only goes with Home and End: Ctrl+arrows and Ctrl+PageUp/PageDown belong to others.
  if (mod && key !== "Home" && key !== "End") return null;
  const { rows, cols } = state;
  if (rows <= 0 || cols <= 0) return null;
  const from = state.active ?? { row: 0, col: 0 };
  const page = Math.max(1, state.page ?? 10);
  let to: Cell;
  // The first arrow press only shows where the active cell starts.
  if (state.active === null && !e.shiftKey && key.startsWith("Arrow")) to = from;
  else
    switch (key) {
      case "ArrowUp":
        to = { row: from.row - 1, col: from.col };
        break;
      case "ArrowDown":
        to = { row: from.row + 1, col: from.col };
        break;
      case "ArrowLeft":
        to = { row: from.row, col: from.col - 1 };
        break;
      case "ArrowRight":
        to = { row: from.row, col: from.col + 1 };
        break;
      case "Home":
        to = mod ? { row: 0, col: 0 } : { row: from.row, col: 0 };
        break;
      case "End":
        to = mod ? { row: rows - 1, col: cols - 1 } : { row: from.row, col: cols - 1 };
        break;
      case "PageUp":
        to = { row: from.row - page, col: from.col };
        break;
      case "PageDown":
        to = { row: from.row + page, col: from.col };
        break;
      default:
        return null;
    }
  const active = { row: clamp(to.row, rows - 1), col: clamp(to.col, cols - 1) };
  return { active, selection: e.shiftKey ? extend(state.selection, from, active) : { kind: "cells", anchor: active, focus: active } };
}

/** The selection after Shift moved the active cell from `from` to `to`. */
function extend(sel: GridSelection | null, from: Cell, to: Cell): GridSelection {
  if (sel?.kind === "rows" && to.col === from.col) return { kind: "rows", anchor: sel.anchor, focus: to.row };
  const anchor = sel?.kind === "cells" ? sel.anchor : from;
  return { kind: "cells", anchor, focus: to };
}
