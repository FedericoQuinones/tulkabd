import { beforeEach, expect, test, vi } from "vitest";

vi.mock("../ipc", async (original) => ({
  ...(await original<typeof import("../ipc")>()),
  aiChatSend: vi.fn(),
  aiChatStop: vi.fn(),
}));

import * as ipc from "../ipc";
import type { ChatEvent } from "../ipc";
import { AiChat, contextFor } from "./chat.svelte";

const send = vi.mocked(ipc.aiChatSend);
const stop = vi.mocked(ipc.aiChatStop);

beforeEach(() => {
  vi.resetAllMocks();
  send.mockResolvedValue(undefined);
  stop.mockResolvedValue(undefined);
});

test("deltas_then_full_text_replace", () => {
  const c = new AiChat();
  c.apply({ kind: "textDelta", text: "Hel" });
  c.apply({ kind: "textDelta", text: "lo" });
  expect(c.items).toEqual([{ kind: "assistant", text: "Hello", streaming: true }]);
  c.apply({ kind: "text", text: "Hello there" });
  expect(c.items).toEqual([{ kind: "assistant", text: "Hello there", streaming: false }]);
  c.apply({ kind: "textDelta", text: "More" });
  expect(c.items).toHaveLength(2);
});

test("tool_call_then_result_fill", () => {
  const c = new AiChat();
  c.apply({ kind: "toolCall", id: "t1", tool: "run_query", input: { sql: "select 1" } });
  c.apply({ kind: "toolResult", id: "t1", ok: true, preview: "1 row" });
  expect(c.items).toEqual([{ kind: "tool", id: "t1", tool: "run_query", input: { sql: "select 1" }, ok: true, preview: "1 row" }]);
});

test("started_sets_session_and_next_send_resumes", async () => {
  send.mockImplementation(async (_c, _s, _t, onEvent) => {
    onEvent({ kind: "started", sessionId: "S1" });
    onEvent({ kind: "done", durationMs: 1, isError: false, result: "" });
  });
  const c = new AiChat();
  await c.send("hi", null);
  expect(send.mock.calls[0][1]).toBeNull();
  expect(c.sessionId).toBe("S1");
  expect(c.running).toBe(false);
  await c.send("again", null);
  expect(send.mock.calls[1][1]).toBe("S1");
  expect(send.mock.calls[1][0]).toBe(send.mock.calls[0][0]);
});

test("failed_stopped_message", () => {
  const c = new AiChat();
  c.running = true;
  c.apply({ kind: "failed", message: "stopped" });
  expect(c.running).toBe(false);
  expect(c.items).toEqual([{ kind: "error", text: "Stopped." }]);
  c.apply({ kind: "failed", message: "boom" });
  expect(c.items[1]).toEqual({ kind: "error", text: "boom" });
  c.apply({ kind: "failed", message: "claude_not_found" });
  expect((c.items[2] as { text: string }).text).toMatch(/not installed/);
});

test("done_with_error_pushes_result", () => {
  const c = new AiChat();
  c.apply({ kind: "done", durationMs: 1, isError: true, result: "bad" });
  expect(c.items).toEqual([{ kind: "error", text: "bad" }]);
});

test("new_chat_stops_running", async () => {
  let release!: () => void;
  send.mockImplementationOnce((_c, _s, _t, onEvent) => {
    onEvent({ kind: "started", sessionId: "S1" });
    return new Promise<void>((r) => (release = r));
  });
  const c = new AiChat();
  const p = c.send("hi", null);
  expect(c.running).toBe(true);
  const oldId = send.mock.calls[0][0];
  await c.reset();
  expect(stop).toHaveBeenCalledWith(oldId);
  expect(c.items).toEqual([]);
  expect(c.sessionId).toBeNull();
  release();
  await p;
  await c.send("next", null);
  expect(send.mock.calls[1][0]).not.toBe(oldId);
  expect(send.mock.calls[1][1]).toBeNull();
});

test("busy_rejection_shows_error", async () => {
  send.mockRejectedValue("busy");
  const c = new AiChat();
  await c.send("hi", null);
  expect(c.running).toBe(false);
  expect(c.items.at(-1)?.kind).toBe("error");
});

test("context_prefix_only_for_visible_connection", async () => {
  const visible = [{ connectionId: "a", label: "Prod", passwordSaved: true }];
  expect(contextFor("a", "Sales", visible)).toEqual({ connectionLabel: "Prod", database: "Sales" });
  expect(contextFor("b", "Sales", visible)).toBeNull();
  expect(contextFor(null, null, visible)).toBeNull();

  const c = new AiChat();
  await c.send("q", { connectionLabel: "Prod", database: "Sales" });
  expect(send.mock.calls[0][2]).toBe('[Context: the user is looking at connection "Prod", database "Sales".]\n\nq');
  await c.send("q", { connectionLabel: "Prod", database: null });
  expect(send.mock.calls[1][2]).toBe('[Context: the user is looking at connection "Prod".]\n\nq');
  c.useContext = false;
  await c.send("q", { connectionLabel: "Prod", database: null });
  expect(send.mock.calls[2][2]).toBe("q");
  await c.send("q", null);
  expect(send.mock.calls[3][2]).toBe("q");
});

test("second_send_while_running_is_ignored", async () => {
  let release!: () => void;
  send.mockImplementationOnce(() => new Promise<void>((r) => (release = r)));
  const c = new AiChat();
  const p = c.send("one", null);
  await c.send("two", null);
  await c.send("   ", null);
  expect(send).toHaveBeenCalledTimes(1);
  expect(c.items).toHaveLength(1);
  expect(c.running).toBe(true);
  release();
  await p;
});

test("streaming_cleared_on_every_assistant_item", () => {
  const c = new AiChat();
  c.items.push({ kind: "assistant", text: "a", streaming: true }, { kind: "tool", id: "t", tool: "x", input: {} });
  c.apply({ kind: "text", text: "b" });
  expect(c.items.filter((i) => i.kind === "assistant" && i.streaming)).toEqual([]);
  c.items.push({ kind: "assistant", text: "c", streaming: true });
  c.apply({ kind: "done", durationMs: 1, isError: false, result: "" });
  expect(c.items.filter((i) => i.kind === "assistant" && i.streaming)).toEqual([]);
});

test("rejection_text_and_empty_error", async () => {
  send.mockRejectedValueOnce("busy");
  const c = new AiChat();
  await c.send("hi", null);
  expect((c.items.at(-1) as { text: string }).text).toMatch(/still answering/);
  send.mockRejectedValueOnce(new Error("boom"));
  await c.send("hi", null);
  expect((c.items.at(-1) as { text: string }).text).toBe("boom");
  c.apply({ kind: "done", durationMs: 1, isError: true, result: "" });
  expect((c.items.at(-1) as { text: string }).text).toBe("Claude Code ended with an error.");
});

test("prefix_quotes_awkward_names", async () => {
  const c = new AiChat();
  await c.send("q", { connectionLabel: 'a"b\nc', database: null });
  expect(send.mock.calls[0][2]).toBe('[Context: the user is looking at connection "a\\"b\\nc".]\n\nq');
});
