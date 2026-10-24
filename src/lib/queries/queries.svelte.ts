// Saved queries (spec §5.9): a cache of the `.sql` files under `queries/<database>/`, reloaded after
// every change made here. Opening, saving and renaming them in tabs lives in the tabs store, which
// keeps open tabs in step; the Explorer, the palette and the Rail panel read this cache.
import { t } from "../i18n/t";
import * as ipc from "../ipc";
import type { SavedQuery } from "../ipc";

/** What the name dialog (`NameDialog.svelte`) asks for and reports back. */
export interface NameRequest {
  title: string;
  initial: string;
  /** Resolves to null when done (the dialog closes), else an error code such as `"exists"`. */
  onsubmit: (name: string, overwrite: boolean) => Promise<string | null>;
  oncancel: () => void;
}

/** A saved query command error as user text. */
export function queryError(e: unknown): string {
  const s = String(e);
  if (s === "exists") return t("queries.errors.exists");
  if (s === "empty_name") return t("queries.errors.emptyName");
  if (s === "not_found") return t("queries.errors.notFound");
  if (s === "not_text") return t("queries.errors.notText");
  return t("queries.errors.action", { error: s });
}

export interface ConnectionDatabases {
  id: string;
  connected: boolean;
  /** Database names the app knows on this connection (cached or from a live session). */
  databases: string[];
}

/**
 * The connection a saved query of `database` opens on (plan D6): the active tab's connection when
 * it has a database with that name; else the first connected connection that has it; else none.
 * `candidates` are in the order "first" means.
 */
export function pickConnection(database: string, activeConnectionId: string | null, candidates: ConnectionDatabases[]): string | null {
  const has = (c: ConnectionDatabases) => c.databases.includes(database);
  const active = candidates.find((c) => c.id === activeConnectionId);
  if (active && has(active)) return active.id;
  return candidates.find((c) => c.connected && has(c))?.id ?? null;
}

class QueriesStore {
  /** Every saved query, sorted by database then name (the backend's order). */
  all = $state.raw<SavedQuery[]>([]);

  private readonly byDatabase = $derived.by(() => {
    const out = new Map<string, SavedQuery[]>();
    for (const q of this.all) {
      let list = out.get(q.database);
      if (!list) out.set(q.database, (list = []));
      list.push(q);
    }
    return out;
  });

  private token = 0;

  /** The saved queries of a database (exact name), from the cache. */
  list(database: string): SavedQuery[] {
    return this.byDatabase.get(database) ?? [];
  }

  /** Reads the list again; a failed read keeps what is shown. */
  async reload(): Promise<void> {
    const token = ++this.token;
    try {
      const list = await ipc.listSavedQueries();
      if (token === this.token) this.all = list;
    } catch {
      // Keep the cache.
    }
  }

  /** Rejects with the backend's error code (`exists`, `empty_name`…). */
  async save(database: string, name: string, text: string, overwrite: boolean): Promise<void> {
    await ipc.saveQuery(database, name, text, overwrite);
    await this.reload();
  }

  async rename(database: string, name: string, newName: string): Promise<void> {
    await ipc.renameSavedQuery(database, name, newName);
    await this.reload();
  }

  async remove(database: string, name: string): Promise<void> {
    await ipc.deleteSavedQuery(database, name);
    await this.reload();
  }

  /** Clears the cache (used by tests). */
  reset(): void {
    this.token++;
    this.all = [];
  }
}

export const queries = new QueriesStore();
