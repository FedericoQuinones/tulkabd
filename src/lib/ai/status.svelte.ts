import * as ipc from "../ipc";
import type { AiVisible, ClaudeStatus } from "../ipc";

/** What the AI panel knows about Claude Code and the shared connections, for the connect section and the chat. */
class AiStatus {
  status = $state<ClaudeStatus | null>(null);
  visible = $state<AiVisible[]>([]);
  loaded = $state(false);
  /** The text of the last failed refresh; null when the last one worked. */
  error = $state<string | null>(null);
  private inflight: Promise<void> | null = null;

  /** Reads both again. A refresh already running is reused, and a failure keeps what was known. */
  refresh(): Promise<void> {
    this.inflight ??= (async () => {
      try {
        const [status, visible] = await Promise.all([ipc.claudeStatus(), ipc.aiVisibleConnections()]);
        this.status = status;
        this.visible = visible;
        this.error = null;
      } catch (e) {
        this.error = e instanceof Error ? e.message : typeof e === "string" ? e : JSON.stringify(e);
      } finally {
        this.loaded = true;
        this.inflight = null;
      }
    })();
    return this.inflight;
  }

  /** Reads only the shared connections, keeping the previous list on failure. */
  async refreshVisible(): Promise<void> {
    try {
      this.visible = await ipc.aiVisibleConnections();
    } catch {
      /* keep the previous list */
    }
  }

  /** Back to the state before the first refresh. */
  reset(): void {
    this.status = null;
    this.visible = [];
    this.loaded = false;
    this.error = null;
    this.inflight = null;
  }
}

export const aiStatus = new AiStatus();
