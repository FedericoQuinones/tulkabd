import * as ipc from "../ipc";
import type { AiVisible, ChatEvent } from "../ipc";
import { t } from "../i18n/t";

export type ChatItem =
  | { kind: "user"; text: string }
  | { kind: "assistant"; text: string; streaming: boolean }
  | { kind: "tool"; id: string; tool: string; input: Record<string, unknown>; ok?: boolean; preview?: string }
  | { kind: "error"; text: string };

export interface ChatContext {
  connectionLabel: string;
  database: string | null;
}

/** The connection and database the user is looking at, when the assistant may see that connection. */
export function contextFor(connectionId: string | null, database: string | null, visible: AiVisible[]): ChatContext | null {
  if (!connectionId) return null;
  const found = visible.find((v) => v.connectionId === connectionId);
  return found ? { connectionLabel: found.label, database } : null;
}

function prefix(ctx: ChatContext): string {
  const db = ctx.database === null ? "" : `, database "${ctx.database}"`;
  return `[Context: the user is looking at connection "${ctx.connectionLabel}"${db}.]\n\n`;
}

function failure(message: string): string {
  if (message === "stopped") return t("ai.chat.stopped");
  if (message === "claude_not_found") return t("ai.chat.notInstalled");
  return message;
}

/** The conversation with Claude Code shown in the AI panel. */
export class AiChat {
  items = $state<ChatItem[]>([]);
  running = $state(false);
  sessionId = $state<string | null>(null);
  useContext = $state(true);
  private chatId = crypto.randomUUID();

  apply(ev: ChatEvent): void {
    const last = this.items[this.items.length - 1];
    switch (ev.kind) {
      case "started":
        this.sessionId = ev.sessionId;
        break;
      case "textDelta":
        if (last?.kind === "assistant" && last.streaming) last.text += ev.text;
        else this.items.push({ kind: "assistant", text: ev.text, streaming: true });
        break;
      case "text":
        if (last?.kind === "assistant" && last.streaming) {
          last.text = ev.text;
          last.streaming = false;
        } else this.items.push({ kind: "assistant", text: ev.text, streaming: false });
        break;
      case "toolCall": {
        const input = ev.input && typeof ev.input === "object" ? (ev.input as Record<string, unknown>) : {};
        this.items.push({ kind: "tool", id: ev.id, tool: ev.tool, input });
        break;
      }
      case "toolResult": {
        const item = this.items.find((i) => i.kind === "tool" && i.id === ev.id);
        if (item && item.kind === "tool") {
          item.ok = ev.ok;
          item.preview = ev.preview;
        }
        break;
      }
      case "done":
        this.running = false;
        if (ev.isError) this.items.push({ kind: "error", text: ev.result });
        break;
      case "failed":
        this.running = false;
        this.items.push({ kind: "error", text: failure(ev.message) });
        break;
    }
  }

  async send(text: string, context: ChatContext | null): Promise<void> {
    const chatId = this.chatId;
    this.items.push({ kind: "user", text });
    this.running = true;
    const body = this.useContext && context ? prefix(context) + text : text;
    try {
      await ipc.aiChatSend(chatId, this.sessionId, body, (ev) => {
        if (chatId === this.chatId) this.apply(ev);
      });
    } catch (e) {
      if (chatId === this.chatId) this.apply({ kind: "failed", message: String(e) });
    } finally {
      if (chatId === this.chatId) {
        this.running = false;
        const last = this.items[this.items.length - 1];
        if (last?.kind === "assistant") last.streaming = false;
      }
    }
  }

  async stop(): Promise<void> {
    if (this.running) await ipc.aiChatStop(this.chatId).catch(() => {});
  }

  /** Stops a run in progress, clears the conversation and starts a new one. */
  async reset(): Promise<void> {
    const old = this.chatId;
    const wasRunning = this.running;
    this.chatId = crypto.randomUUID();
    this.items = [];
    this.running = false;
    this.sessionId = null;
    if (wasRunning) await ipc.aiChatStop(old).catch(() => {});
  }
}

export const aiChat = new AiChat();
