import type { Value } from "../ipc";
import { WINDOW_SIZE, windowOffsets } from "./virtual";

type Fetch = (offset: number, limit: number) => Promise<Value[][]>;

/** Most windows kept (10,000 rows at the default window size); the backend owns the rows. */
export const MAX_WINDOWS = 50;

/**
 * Row cache for one result of one run, filled in aligned windows.
 * Dedupes in-flight windows, refetches windows that were short because rows were still
 * streaming, drops responses after `destroy()` (stale run) and keeps the last failure.
 * Keeps at most `maxWindows` windows: the least recently ensured ones outside the current
 * range are evicted, and fetched again when scrolled back to.
 */
export class WindowCache {
  /** Rows per window offset, in least to most recently ensured order. */
  private windows = new Map<number, Value[][]>();
  private inflight = new Set<number>();
  private failed = new Set<number>();
  private destroyed = false;
  private total = 0;
  private range = { start: 0, end: 0 };
  /** Message of the last failed fetch; cleared by a successful fetch or `retry()`. */
  error: string | null = null;

  constructor(
    private fetch: Fetch,
    private onChange: () => void = () => {},
    private size = WINDOW_SIZE,
    private maxWindows = MAX_WINDOWS,
  ) {}

  get(row: number): Value[] | undefined {
    const offset = Math.floor(row / this.size) * this.size;
    return this.windows.get(offset)?.[row - offset];
  }

  /** Requests the windows covering `[start, end)`; `total` is the current row count. */
  ensure(start: number, end: number, total: number): void {
    this.range = { start, end };
    this.total = total;
    for (const o of windowOffsets(start, end, this.size)) {
      this.touch(o);
      // Windows that failed are retried only through `retry()`, never in a loop.
      if (!this.failed.has(o)) void this.load(o);
    }
    this.evict();
  }

  /** Clears the error and requests the failed windows again. */
  retry(): void {
    const again = [...this.failed];
    this.failed.clear();
    this.error = null;
    this.onChange();
    for (const o of again) void this.load(o);
    this.ensure(this.range.start, this.range.end, this.total);
  }

  destroy(): void {
    this.destroyed = true;
  }

  /** Marks a cached window as the most recently ensured one. */
  private touch(offset: number): void {
    const rows = this.windows.get(offset);
    if (rows === undefined) return;
    this.windows.delete(offset);
    this.windows.set(offset, rows);
  }

  /** Drops the least recently ensured windows outside the current range, down to `maxWindows`. */
  private evict(): void {
    if (this.windows.size <= this.maxWindows) return;
    const first = Math.floor(this.range.start / this.size) * this.size;
    for (const offset of this.windows.keys()) {
      if (this.windows.size <= this.maxWindows) break;
      if (offset >= first && offset < this.range.end) continue;
      this.windows.delete(offset);
    }
  }

  private incomplete(offset: number): boolean {
    const limit = Math.min(this.size, this.total - offset);
    if (limit <= 0) return false;
    return (this.windows.get(offset)?.length ?? 0) < limit;
  }

  private async load(offset: number): Promise<void> {
    if (this.destroyed || this.inflight.has(offset) || !this.incomplete(offset)) return;
    this.inflight.add(offset);
    const totalAtRequest = this.total;
    try {
      const rows = await this.fetch(offset, this.size);
      if (this.destroyed) return;
      // A response never shrinks a window (rows only grow while streaming).
      if (rows.length >= (this.windows.get(offset)?.length ?? 0)) {
        this.windows.delete(offset);
        this.windows.set(offset, rows);
        this.evict();
      }
      this.failed.delete(offset);
      // The notice stays while any other window is still failed.
      if (this.failed.size === 0) this.error = null;
      this.onChange();
    } catch (e) {
      if (this.destroyed) return;
      this.failed.add(offset);
      this.error = String(e);
      this.onChange();
      return;
    } finally {
      this.inflight.delete(offset);
    }
    // Only a `total` that grew during the request can make another fetch useful;
    // otherwise a short or empty answer would loop forever.
    if (this.total !== totalAtRequest) void this.load(offset);
  }
}
