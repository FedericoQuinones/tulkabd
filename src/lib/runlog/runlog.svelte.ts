import * as ipc from "../ipc";
import type { RunLogEntry } from "../ipc";

const PAGE = 100;

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

/**
 * Every run of every tab, newest first. The first page is read the first time the Run log tab is
 * shown, so the app does not pay for it at startup; runs that finish later arrive as events.
 */
export class RunLogStore {
  entries = $state<RunLogEntry[]>([]);
  /** True once the first page has been read. */
  loaded = $state(false);
  /** True when the first page could not be read; `ensureLoaded` tries again. */
  failed = $state(false);
  private loading = false;
  private exhausted = false;

  /**
   * Runs of today (local time) among the entries held. Before the tab has been opened that is
   * only what this session has run so far.
   */
  get todayCount(): number {
    const now = new Date();
    return this.entries.filter((e) => sameDay(new Date(e.atMs), now)).length;
  }

  /**
   * A run that just finished, placed by id. Usually it is the newest, but an AI run is noticed up
   * to a couple of seconds after it happened, so it can belong below a run held already.
   */
  prepend(entry: RunLogEntry): void {
    if (this.entries.some((e) => e.id === entry.id)) return;
    const at = this.entries.findIndex((e) => e.id < entry.id);
    this.entries.splice(at === -1 ? this.entries.length : at, 0, entry);
  }

  async ensureLoaded(): Promise<void> {
    if (this.loaded || this.loading) return;
    this.loading = true;
    this.failed = false;
    try {
      const rows = await ipc.listRunLog(null, PAGE);
      // Runs that arrived while the page was on its way are in the page too.
      const held = new Set(rows.map((e) => e.id));
      const fresh = this.entries.filter((e) => !held.has(e.id));
      this.entries = [...fresh, ...rows].sort((a, b) => b.id - a.id);
      this.exhausted = rows.length < PAGE;
      this.loaded = true;
    } catch {
      this.failed = true;
    } finally {
      this.loading = false;
    }
  }

  /** Reads the page of runs older than the oldest one held. */
  async loadMore(): Promise<void> {
    if (!this.loaded) return void (await this.ensureLoaded());
    if (this.loading || this.exhausted || this.entries.length === 0) return;
    this.loading = true;
    try {
      const before = this.entries[this.entries.length - 1].id;
      const rows = await ipc.listRunLog(before, PAGE);
      const held = new Set(this.entries.map((e) => e.id));
      this.entries.push(...rows.filter((e) => !held.has(e.id)));
      this.exhausted = rows.length < PAGE;
    } catch {
      // The next scroll to the end tries again.
    } finally {
      this.loading = false;
    }
  }

  /** Back to the state before the first load. */
  reset(): void {
    this.entries = [];
    this.loaded = false;
    this.failed = false;
    this.loading = false;
    this.exhausted = false;
  }
}

export const runLog = new RunLogStore();
