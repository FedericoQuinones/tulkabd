import type { Value } from "../ipc";
import { WINDOW_SIZE, windowOffsets } from "./virtual";

type Fetch = (offset: number, limit: number) => Promise<Value[][]>;

/**
 * Row cache for one result of one run, filled in aligned windows.
 * Dedupes in-flight windows, refetches windows that were short because rows were still
 * streaming, drops responses after `destroy()` (stale run) and keeps the last failure.
 */
export class WindowCache {
  private rows = new Map<number, Value[]>();
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
  ) {}

  get(row: number): Value[] | undefined {
    return this.rows.get(row);
  }

  /** Requests the windows covering `[start, end)`; `total` is the current row count. */
  ensure(start: number, end: number, total: number): void {
    this.range = { start, end };
    this.total = total;
    for (const o of windowOffsets(start, end, this.size)) {
      // Windows that failed are retried only through `retry()`, never in a loop.
      if (!this.failed.has(o)) void this.load(o);
    }
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

  private incomplete(offset: number): boolean {
    const limit = Math.min(this.size, this.total - offset);
    if (limit <= 0) return false;
    for (let i = offset; i < offset + limit; i++) if (!this.rows.has(i)) return true;
    return false;
  }

  private async load(offset: number): Promise<void> {
    if (this.destroyed || this.inflight.has(offset) || !this.incomplete(offset)) return;
    this.inflight.add(offset);
    const totalAtRequest = this.total;
    try {
      const rows = await this.fetch(offset, this.size);
      if (this.destroyed) return;
      rows.forEach((r, i) => this.rows.set(offset + i, r));
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
