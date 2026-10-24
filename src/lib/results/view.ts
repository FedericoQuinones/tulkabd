import type { ColumnFilter, ViewSpec } from "../ipc";

export type SortSpec = NonNullable<ViewSpec["sort"]>;

/** Rows as received: no sort and no filter. */
export const EMPTY_VIEW: ViewSpec = { sort: null, text: null, columns: [] };

/** The sort after a click on `column`'s header: ascending, then descending, then none. */
export function nextSort(current: SortSpec | null, column: number): SortSpec | null {
  if (current?.column !== column) return { column, descending: false };
  return current.descending ? null : { column, descending: true };
}

/** The column filters with `column`'s filter replaced by `filter` (one per column), or removed for null. */
export function withColumnFilter(columns: ColumnFilter[], column: number, filter: ColumnFilter | null): ColumnFilter[] {
  const rest = columns.filter((f) => f.column !== column);
  return filter ? [...rest, filter] : rest;
}

/** True when the view hides rows (a sort alone shows them all). */
export function isFiltered(spec: ViewSpec): boolean {
  return !!spec.text || spec.columns.length > 0;
}

const isEmpty = (spec: ViewSpec) => spec.sort === null && !isFiltered(spec);

type Listener = (error: string | null) => void;

/**
 * The view of one received result. `set` sends the view to the backend one call at a time: a
 * call waits for the previous one, and only the newest view asked for meanwhile is sent next,
 * so an older view can never be applied last.
 */
export class ResultView {
  /** The view the user chose; ahead of the backend while `pending`. */
  spec: ViewSpec = EMPTY_VIEW;
  /** The view the backend applied last. */
  applied: ViewSpec = EMPTY_VIEW;
  /** Rows the applied view shows; null when it shows every row as received. */
  visible: number | null = null;
  /** True while a call runs or waits. */
  pending = false;
  /** Grows each time the backend's view changed: rows read before then are in the old order. */
  generation = 0;

  private waiting: ViewSpec | null = null;
  private running = false;
  private disposed = false;
  private listeners = new Set<Listener>();

  constructor(private send: (spec: ViewSpec) => Promise<number>) {}

  /** Calls `fn` after each change, with the error when a call failed. */
  subscribe(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  set(spec: ViewSpec): void {
    if (this.disposed) return;
    this.spec = spec;
    this.waiting = spec;
    this.pending = true;
    this.notify(null);
    void this.pump();
  }

  /** Drops what still waits; the call already sent (if any) finishes without a follow-up. */
  dispose(): void {
    this.disposed = true;
    this.waiting = null;
  }

  private async pump(): Promise<void> {
    if (this.running) return;
    this.running = true;
    let changed = false;
    let error: string | null = null;
    while (this.waiting) {
      const spec = this.waiting;
      this.waiting = null;
      try {
        const visible = await this.send(spec);
        this.applied = spec;
        this.visible = isEmpty(spec) ? null : visible;
        changed = true;
        error = null;
      } catch (e) {
        // The backend keeps its previous view when a call fails.
        error = String(e);
      }
    }
    this.running = false;
    this.pending = false;
    if (error !== null) this.spec = this.applied;
    if (changed) this.generation++;
    this.notify(error);
  }

  private notify(error: string | null): void {
    for (const fn of this.listeners) fn(error);
  }
}

const views = new WeakMap<object, { run: number; byIndex: Map<number, ResultView> }>();

/**
 * The view of result `index` of run `run` of `owner` (a pane). It outlives the grid, which is
 * mounted again on every tab switch, while the backend keeps the view until the next run.
 */
export function resultView(owner: object, run: number, index: number, send: (spec: ViewSpec) => Promise<number>): ResultView {
  let entry = views.get(owner);
  if (!entry || entry.run !== run) {
    for (const v of entry?.byIndex.values() ?? []) v.dispose();
    entry = { run, byIndex: new Map() };
    views.set(owner, entry);
  }
  let view = entry.byIndex.get(index);
  if (!view) {
    view = new ResultView(send);
    entry.byIndex.set(index, view);
  }
  return view;
}
