import * as ipc from "../ipc";
import { forgetStructures } from "../structure/cache";
import type { ConnectionConfig, GuardLevel, SaveConnectionResult } from "../ipc";

class ConnectionStore {
  list = $state<ConnectionConfig[]>([]);
  /** Last load/save/delete failure, if any (never contains secrets). */
  error = $state<string | null>(null);

  /** Ids of the connections whose guard is off until the app closes (mirrors the backend). */
  guardOff = $state<string[]>([]);

  /** Called with a connection id when what was cached for it no longer applies (its server or scope changed, or it was deleted). */
  onForget: ((id: string) => void) | null = null;

  /** Set when the connections file could not be read at startup. */
  startupError = $state<string | null>(null);

  async loadStartupError(): Promise<void> {
    try {
      this.startupError = await ipc.startupError();
    } catch {
      // An IPC failure hides the banner on purpose: it is only an advisory.
      this.startupError = null;
    }
  }

  async refresh(): Promise<void> {
    try {
      this.list = await ipc.listConnections();
      this.error = null;
    } catch (e) {
      this.error = String(e);
    }
  }

  /** Reads the "off until the app closes" list from the backend. */
  async loadGuardOff(): Promise<void> {
    try {
      this.guardOff = await ipc.guardOffForSession();
    } catch {
      // Keep what was known: dropping it would show a guard as on when it is off.
    }
  }

  /** Saves a connection; the password goes to the backend only and is not retained here. */
  async save(conn: ConnectionConfig, password?: string): Promise<SaveConnectionResult> {
    const old = this.list.find((c) => c.id === conn.id);
    const result = await ipc.saveConnection(conn, password);
    if (
      old &&
      (old.server !== conn.server || old.port !== conn.port || JSON.stringify(old.databases) !== JSON.stringify(conn.databases))
    ) {
      this.onForget?.(conn.id);
    }
    forgetStructures(conn.id);
    // Saving with another guard level ends the until-restart override, so both lists are re-read.
    await Promise.all([this.refresh(), this.loadGuardOff()]);
    return result;
  }

  /**
   * Saves a guard level for good and mirrors it here (the saved level also ends the
   * until-restart override). Resolves with the previous level, for undo.
   */
  async setGuard(id: string, level: GuardLevel): Promise<GuardLevel> {
    const previous = await ipc.setConnectionGuard(id, level);
    this.list = this.list.map((c) => (c.id === id ? { ...c, guard: level } : c));
    this.guardOff = this.guardOff.filter((x) => x !== id);
    return previous;
  }

  /** Turns the guard of a connection off (or back on) until the app closes. */
  async setGuardOffForSession(id: string, off: boolean): Promise<void> {
    await ipc.setGuardOffForSession(id, off);
    this.guardOff = off ? [...new Set([...this.guardOff, id])] : this.guardOff.filter((x) => x !== id);
  }

  async remove(id: string): Promise<void> {
    await ipc.deleteConnection(id);
    forgetStructures(id);
    await this.refresh();
    this.onForget?.(id);
  }
}

export const connections = new ConnectionStore();
