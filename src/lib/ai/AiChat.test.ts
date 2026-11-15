// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  claudeStatus: vi.fn(),
  openClaudeInstallDocs: vi.fn(),
  aiVisibleConnections: vi.fn(),
  aiChatSend: vi.fn(),
  aiChatStop: vi.fn(),
}));
const pane = vi.hoisted(() => ({ connectionId: "c1" as string | null, database: "practice" as string | null }));
const open = vi.hoisted(() => vi.fn());
vi.mock("../tabs/tabs.svelte", () => ({
  tabs: {
    get activePane() {
      return pane;
    },
    open,
  },
}));

import * as ipc from "../ipc";
import { aiChat } from "./chat.svelte";
import AiChat from "./AiChat.svelte";

let cmp: ReturnType<typeof mount> | null = null;

async function show() {
  cmp = mount(AiChat, { target: document.body });
  flushSync();
  await vi.waitFor(() => expect(ipc.aiVisibleConnections).toHaveBeenCalled());
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
}
const button = (name: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === name || b.getAttribute("aria-label") === name);
const box = () => document.querySelector<HTMLTextAreaElement>("textarea")!;
function type(text: string) {
  box().value = text;
  box().dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}
function key(k: string, shiftKey = false) {
  const ev = new KeyboardEvent("keydown", { key: k, shiftKey, bubbles: true, cancelable: true });
  box().dispatchEvent(ev);
  flushSync();
  return ev;
}

beforeEach(async () => {
  for (const f of Object.values(vi.mocked(ipc))) (f as ReturnType<typeof vi.fn>).mockReset();
  open.mockReset();
  pane.connectionId = "c1";
  pane.database = "practice";
  vi.mocked(ipc.claudeStatus).mockResolvedValue({ installed: true, version: "2", connected: true, connectedElsewhere: false, path: "x", error: null });
  vi.mocked(ipc.aiVisibleConnections).mockResolvedValue([{ connectionId: "c1", label: "localtest", passwordSaved: true }]);
  vi.mocked(ipc.aiChatSend).mockResolvedValue(undefined as never);
  vi.mocked(ipc.aiChatStop).mockResolvedValue(undefined as never);
  await aiChat.reset();
  aiChat.useContext = true;
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = null;
  document.body.innerHTML = "";
});

it("enter_sends_shift_enter_newlines", async () => {
  await show();
  type("hello");
  expect(key("Enter", true).defaultPrevented).toBe(false);
  expect(ipc.aiChatSend).not.toHaveBeenCalled();
  expect(key("Enter").defaultPrevented).toBe(true);
  await vi.waitFor(() => expect(ipc.aiChatSend).toHaveBeenCalledOnce());
  expect(vi.mocked(ipc.aiChatSend).mock.calls[0][2]).toContain("hello");
  expect(box().value).toBe("");
});

it("stop_button_while_running", async () => {
  vi.mocked(ipc.aiChatSend).mockReturnValue(new Promise(() => {}));
  await show();
  expect(button("Send")!.disabled).toBe(true);
  type("hi");
  expect(button("Send")!.disabled).toBe(false);
  button("Send")!.click();
  flushSync();
  expect(button("Send")).toBeUndefined();
  button("Stop")!.click();
  expect(ipc.aiChatStop).toHaveBeenCalled();
});

it("tool_row_expands_and_opens_tab", async () => {
  aiChat.items.push({ kind: "tool", id: "t1", tool: "run_query", input: { connection: "localtest", database: "practice", sql: "SELECT 1" }, ok: true, preview: "1 row" });
  await show();
  expect(document.body.textContent).toContain("run_query · localtest/practice");
  expect(document.body.textContent).not.toContain("SELECT 1");
  const row = button("run_query · localtest/practice")!;
  expect(row.getAttribute("aria-expanded")).toBe("false");
  row.click();
  flushSync();
  expect(document.body.textContent).toContain("SELECT 1");
  expect(document.body.textContent).toContain("1 row");
  button("Open in tab")!.click();
  expect(open).toHaveBeenCalledWith({ kind: "query", connectionId: "c1", database: "practice", sql: "SELECT 1" });
});

it("open_in_tab_disabled_for_unknown_connection", async () => {
  aiChat.items.push({ kind: "tool", id: "t1", tool: "run_query", input: { connection: "nope", sql: "SELECT 1" } });
  await show();
  button("run_query · nope")!.click();
  flushSync();
  expect(button("Open in tab")!.disabled).toBe(true);
});

it("context_chip_hidden_for_invisible_connection", async () => {
  await show();
  const chip = button("localtest / practice")!;
  expect(chip).toBeDefined();
  expect(chip.getAttribute("aria-pressed")).toBe("true");
  chip.click();
  flushSync();
  expect(aiChat.useContext).toBe(false);
  unmount(cmp!);
  document.body.innerHTML = "";
  pane.connectionId = "other";
  await show();
  expect(button("localtest / practice")).toBeUndefined();
});

it("renders_code_block_with_copy", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  aiChat.items.push({ kind: "assistant", text: "Try:\n\n```sql\nSELECT <b>1</b>\n```", streaming: false });
  await show();
  const code = document.querySelector("pre code")!;
  expect(code.textContent).toBe("SELECT <b>1</b>");
  expect(document.querySelector("pre b")).toBeNull();
  button("Copy")!.click();
  expect(writeText).toHaveBeenCalledWith("SELECT <b>1</b>");
});

it("shows_install_hint_instead_of_composer", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue({ installed: false, version: null, connected: false, connectedElsewhere: false, path: null, error: null });
  await show();
  await vi.waitFor(() => expect(document.querySelector("textarea")).toBeNull());
  expect(document.body.textContent).toContain("not installed");
});

it("new_chat_clears", async () => {
  aiChat.items.push({ kind: "user", text: "old" });
  await show();
  expect(document.body.textContent).toContain("old");
  button("New chat")!.click();
  await vi.waitFor(() => expect(document.body.textContent).not.toContain("old"));
  expect(document.body.textContent).toContain("Ask something like");
});
