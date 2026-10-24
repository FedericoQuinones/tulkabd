/** Row height of the result grid at 100 % zoom, in px. */
export const BASE_ROW_HEIGHT = 34;

/** Row height of the result grid at `zoom` percent, rounded to whole pixels. */
export function rowHeight(zoom: number): number {
  return Math.round((BASE_ROW_HEIGHT * zoom) / 100);
}

/** Height of the grid's header row at 100 % zoom, in px. */
export const BASE_HEADER_HEIGHT = 36;

/** Height of the grid's header row at `zoom` percent, rounded to whole pixels. */
export function headerHeight(zoom: number): number {
  return Math.round((BASE_HEADER_HEIGHT * zoom) / 100);
}

/**
 * Scroll position that keeps the same row (and the same part of it) at the top after the row
 * height changed from `oldRowHeight` to `newRowHeight`.
 */
export function anchoredScrollTop(scrollTop: number, oldRowHeight: number, newRowHeight: number): number {
  return (scrollTop / oldRowHeight) * newRowHeight;
}

/** Rows of the window requested from `fetch_rows`. */
export const WINDOW_SIZE = 200;

/** Rows `[start, end)` to render for a scroll position; `end` is exclusive and clamped to `total`. */
export function visibleRange(
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  total: number,
  overscan = 10,
): { start: number; end: number } {
  const first = Math.floor(scrollTop / rowHeight);
  const last = Math.ceil((scrollTop + viewportHeight) / rowHeight);
  return {
    start: Math.max(0, Math.min(first - overscan, total)),
    end: Math.max(0, Math.min(last + overscan, total)),
  };
}

/** Offsets of the aligned windows (multiples of `size`) that cover `[start, end)`. */
export function windowOffsets(start: number, end: number, size = WINDOW_SIZE): number[] {
  const out: number[] = [];
  if (end <= start) return out;
  for (let o = Math.floor(start / size) * size; o < end; o += size) out.push(o);
  return out;
}
