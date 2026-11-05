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
