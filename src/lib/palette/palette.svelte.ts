// Object palette store (Ctrl+P): the query, the latest search answer and the
// selection. Palette.svelte only renders it; grouping and keys live in hits.ts.
import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { PaletteHit, SavedQueryHit, SearchResponse } from "../ipc";
import { explorer } from "../explorer/tree.svelte";
import { objectTabs, type ObjectTarget } from "../explorer/tree";
import { queryError } from "../queries/queries.svelte";
import { tabs } from "../tabs/tabs.svelte";
import { canRun, groupHits, groupQueries, type PaletteAction, type PaletteGroup, type PaletteItem, type QueryGroup } from "./hits";

/** Rows shown; the footer still counts every match. */
const LIMIT = 50;

class PaletteStore {
  isOpen = $state(false);
  query = $state("");
  response = $state.raw<SearchResponse | null>(null);
  selected = $state(0);
  /** Why the last chosen result could not open: its connection failed. */
  error = $state<string | null>(null);

  readonly groups: PaletteGroup[] = $derived(groupHits(this.response?.hits ?? []));
  /** Object hits in display order. */
  readonly flat: PaletteHit[] = $derived(this.groups.flatMap((g) => g.hits));
  /** Saved query hits by database; shown below the objects. */
  readonly queryGroups: QueryGroup[] = $derived(groupQueries(this.response?.queries ?? []));
  /** Saved query hits in display order. */
  readonly queryHits: SavedQueryHit[] = $derived(this.queryGroups.flatMap((g) => g.queries));
  /** Every row in display order (objects, then saved queries), which the selection indexes. */
  readonly items: PaletteItem[] = $derived([
    ...this.flat.map((hit): PaletteItem => ({ kind: "object", hit })),
    ...this.queryHits.map((query): PaletteItem => ({ kind: "query", query })),
  ]);
  /**
   * The footer's counts: the backend's object matches and databases, plus the saved query hits and
   * their databases that no shown object already counts (saved queries belong to a database name).
   */
  readonly counts = $derived.by(() => {
    const res = this.response;
    if (!res) return { matches: 0, databases: 0 };
    const counted = new Set(res.hits.map((h) => h.database));
    const extra = new Set(res.queries.map((q) => q.database).filter((d) => !counted.has(d)));
    return { matches: res.totalMatches + res.queries.length, databases: res.databasesMatched + extra.size };
  });

  private token = 0;
  /** Bumped on every show and hide, so a late subscription from a closed palette is dropped. */
  private session = 0;
  private unlisten: (() => void) | null = null;

  show(): void {
    if (this.isOpen) return;
    this.query = "";
    this.response = null;
    this.selected = 0;
    this.error = null;
    this.reopen();
    // The empty query answers no hits, but says how many databases are indexing.
    void this.search("");
  }

  /** Opens with the last query, results and selection kept. */
  private reopen(): void {
    this.isOpen = true;
    // While databases are indexing, new rows can match: search again as each one finishes.
    const session = ++this.session;
    ipc
      .onAppEvent((e) => {
        if ((e.kind === "indexProgress" && e.done) || e.kind === "indexIdle") {
          if ((this.response?.indexing ?? 0) > 0) void this.search(this.query, true);
        }
      })
      .then((stop) => {
        if (session === this.session) this.unlisten = stop;
        else stop();
      })
      .catch(() => {});
  }

  hide(): void {
    this.isOpen = false;
    this.session++;
    this.token++;
    this.unlisten?.();
    this.unlisten = null;
  }

  /** Searches on every keystroke; an answer to an older keystroke is dropped. */
  async search(query: string, refresh = false): Promise<void> {
    this.query = query;
    if (!refresh) this.error = null;
    const token = ++this.token;
    let res: SearchResponse;
    try {
      res = await ipc.searchObjects(query, LIMIT);
    } catch {
      res = { hits: [], totalMatches: 0, databasesMatched: 0, indexing: 0, queries: [] };
    }
    if (token !== this.token || !this.isOpen) return;
    this.response = res;
    this.selected = refresh ? Math.min(this.selected, Math.max(res.hits.length + res.queries.length - 1, 0)) : 0;
  }

  move(delta: number): void {
    const n = this.items.length;
    if (n > 0) this.selected = (this.selected + delta + n) % n;
  }

  /**
   * Runs an action on the selected hit and closes the palette. Structure and definitions are read
   * through the Explorer session, so a hit that is not connected connects first (with the
   * password prompt if needed); if that connect fails, the palette comes back showing why.
   * Select top 10 opens a query tab, which connects on its own.
   */
  async choose(action: PaletteAction, index = this.selected): Promise<void> {
    const item = this.items[index];
    if (!item) return;
    if (item.kind === "query") {
      if (action === "open") await this.openQuery(item.query.database, item.query.name);
      return;
    }
    const hit = item.hit;
    const kind = hit.object.kind;
    if (!canRun(action, kind)) return;
    this.hide();
    const target: ObjectTarget = {
      connectionId: hit.connectionId,
      database: hit.database,
      schema: hit.object.schema,
      name: hit.object.name,
    };
    if (action === "selectTop") {
      objectTabs.selectTop(target);
      return;
    }
    const open = () => (action === "definition" || kind === "procedure" ? objectTabs.definition(target) : objectTabs.structure(target));
    const status = explorer.conns[hit.connectionId]?.status;
    // The store knows a connection only once the Explorer touched it; until then trust the hit.
    if (status === "connected" || (status === undefined && hit.connected)) {
      open();
      return;
    }
    // A failed connect brings the palette back with the error, also after a password prompt.
    await explorer.connect(hit.connectionId, undefined, (error) => {
      if (error === null) open();
      else this.fail(t("palette.connectFailed", { name: hit.connectionName, error }));
    });
  }

  /** Opens a saved query in a tab bound as plan D6 says (see `explorer.savedQueryConnection`). */
  private async openQuery(database: string, name: string): Promise<void> {
    this.hide();
    try {
      await tabs.openSavedQuery({ database, name }, explorer.savedQueryConnection(database));
    } catch (e) {
      this.fail(t("palette.openFailed", { name, error: queryError(e) }));
    }
  }

  private fail(message: string): void {
    if (!this.isOpen) this.reopen();
    this.error = message;
  }
}

export const palette = new PaletteStore();
