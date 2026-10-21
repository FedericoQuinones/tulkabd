import * as ipc from "../ipc";
import type { ConnectionConfig, SaveConnectionResult } from "../ipc";

class ConnectionStore {
  list = $state<ConnectionConfig[]>([]);
  /** Last load/save/delete failure, if any (never contains secrets). */
  error = $state<string | null>(null);

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

  /** Saves a connection; the password goes to the backend only and is not retained here. */
  async save(conn: ConnectionConfig, password?: string): Promise<SaveConnectionResult> {
    const result = await ipc.saveConnection(conn, password);
    await this.refresh();
    return result;
  }

  async remove(id: string): Promise<void> {
    await ipc.deleteConnection(id);
    await this.refresh();
  }
}

export const connections = new ConnectionStore();
