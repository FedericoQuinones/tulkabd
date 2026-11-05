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

/** Tallest scroll spacer the grid uses, in px: browsers cap an element's height near 33.5M px. */
export const MAX_SPACER = 10_000_000;

/** Geometry of the scrolling body: rows of `rowH` px, `bodyH` px of them visible at once. */
export interface ScrollGeometry {
  total: number;
  rowH: number;
  bodyH: number;
}

/** Height of all the rows laid end to end, in px. */
export const bodyExtent = (g: ScrollGeometry) => g.total * g.rowH;

/** Height of the scroll spacer under the header: the rows' height, capped at `MAX_SPACER`. */
export const spacerHeight = (g: ScrollGeometry) => Math.min(bodyExtent(g), MAX_SPACER);

/** True when the rows are taller than the spacer, so scroll positions map to rows by a scale factor. */
export const isScaled = (g: ScrollGeometry) => bodyExtent(g) > MAX_SPACER;

/** Largest scroll position of the rows, in px of row space (not of the spacer). */
export const maxVirtualTop = (g: ScrollGeometry) => Math.max(0, bodyExtent(g) - g.bodyH);

/** Largest `scrollTop` the spacer allows. */
export const maxRealTop = (g: ScrollGeometry) => Math.max(0, spacerHeight(g) - g.bodyH);

/** The row-space position (what the rows are laid out in) at a real `scrollTop`; identical below the cap. */
export function toVirtualTop(real: number, g: ScrollGeometry): number {
  if (!isScaled(g)) return real;
  const maxReal = maxRealTop(g);
  if (maxReal <= 0) return 0;
  return Math.min(Math.max(real, 0) / maxReal, 1) * maxVirtualTop(g);
}

/** The real `scrollTop` that shows the row-space position `virtual`; the inverse of `toVirtualTop`. */
export function toRealTop(virtual: number, g: ScrollGeometry): number {
  if (!isScaled(g)) return virtual;
  const maxVirtual = maxVirtualTop(g);
  if (maxVirtual <= 0) return 0;
  return Math.min(Math.max(virtual, 0) / maxVirtual, 1) * maxRealTop(g);
}

/**
 * Smallest row-space scroll position that shows the whole row `row` from `virtual`; unchanged when
 * it is already in view.
 */
export function revealRowTop(row: number, virtual: number, g: ScrollGeometry): number {
  const top = row * g.rowH;
  if (top < virtual) return top;
  if (top + g.rowH > virtual + g.bodyH) return Math.max(0, top + g.rowH - g.bodyH);
  return virtual;
}

/** Horizontal padding of the row-number column (6px right, 16px left), in px; it does not scale with the zoom. */
const ID_PADDING = 22;
/** Narrowest row-number column, in px. */
const ID_MIN_WIDTH = 56;
/** Widest digit as a share of the font size; generous so no UI font clips. */
const DIGIT_EM = 0.62;

/** Width of the row-number column for `total` rows at `zoom` percent, so its widest number fits whole. */
export function idColumnWidth(total: number, zoom: number): number {
  const digits = String(Math.max(total, 1)).length;
  const font = (12 * zoom) / 100;
  return Math.max(ID_MIN_WIDTH, Math.ceil(ID_PADDING + digits * DIGIT_EM * font));
}

/**
 * Columns `[start, end)` to render for a horizontal scroll position, and the width of the columns
 * before `start` (the spacer that stands in for them). The row-number column of width `idWidth`
 * comes first; `overscan` px are kept on both sides. An unmeasured viewport renders everything.
 */
export function visibleColumns(
  count: number,
  width: (i: number) => number,
  idWidth: number,
  scrollLeft: number,
  viewportWidth: number,
  overscan = 300,
): { start: number; end: number; offset: number } {
  if (viewportWidth <= 0) return { start: 0, end: count, offset: 0 };
  const from = scrollLeft - overscan;
  const to = scrollLeft + viewportWidth + overscan;
  let left = idWidth;
  let start = 0;
  let offset = 0;
  let i = 0;
  while (i < count && left + width(i) <= from) {
    left += width(i);
    i++;
  }
  start = i;
  offset = left - idWidth;
  while (i < count && left < to) {
    left += width(i);
    i++;
  }
  return { start, end: i, offset };
}

/**
 * Row-space scroll position to keep after the geometry went from `prev` to `next`: the same first
 * row, or the end when the grid was scrolled to the end (rows streaming in keep it there).
 */
export function resyncedTop(virtual: number, prev: ScrollGeometry, next: ScrollGeometry): number {
  const max = maxVirtualTop(next);
  if (prev.total > 0 && virtual >= maxVirtualTop(prev) - 1) return max;
  return Math.min(Math.max(virtual, 0), max);
}
