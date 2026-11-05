/** Narrowest a column can be dragged or fitted to, in px. */
export const MIN_COLUMN_WIDTH = 48;
/** Widest an auto-fit column gets, in px. */
export const MAX_FIT_WIDTH = 600;

/** Widths the user set, per tab and column name; they outlive a grid so a re-run keeps them. */
const remembered = new Map<string, Map<string, number>>();

export function rememberWidth(tabId: string, column: string, width: number): void {
  let m = remembered.get(tabId);
  if (!m) remembered.set(tabId, (m = new Map()));
  m.set(column, width);
}

/** The remembered widths of `names`, by position; a column never sized has none. */
export function recallWidths(tabId: string, names: string[]): number[] {
  const m = remembered.get(tabId);
  const out: number[] = [];
  names.forEach((n, i) => {
    const w = m?.get(n);
    if (w !== undefined) out[i] = w;
  });
  return out;
}

export function forgetWidths(tabId: string): void {
  remembered.delete(tabId);
}

/** Width that shows the header (`header` px of content) and every measured cell (`cells` px of content). */
export function fitWidth(header: number, cells: number[]): number {
  const need = Math.ceil(Math.max(header, ...cells));
  return Math.min(MAX_FIT_WIDTH, Math.max(MIN_COLUMN_WIDTH, need));
}
