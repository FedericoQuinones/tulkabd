// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  claudeStatus: vi.fn(),
  connectClaude: vi.fn(),
  openClaudeTerminal: vi.fn(),
  openClaudeInstallDocs: vi.fn(),
  aiVisibleConnections: vi.fn(),
  copyMcpSetup: vi.fn(),
}));

import * as ipc from "../ipc";
import type { ClaudeStatus } from "../ipc";
import AiConnect from "./AiConnect.svelte";

const status = (over: Partial<ClaudeStatus> = {}): ClaudeStatus => ({
  installed: true,
  version: "2.1.0",
  connected: false,
  connectedElsewhere: false,
  path: "/usr/bin/claude",
  ...over,
});

let cmp: ReturnType<typeof mount> | null = null;

async function show() {
  cmp = mount(AiConnect, { target: document.body });
  flushSync();
  await vi.waitFor(() => expect(ipc.claudeStatus).toHaveBeenCalled());
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
}
const button = (name: string) =>
  [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === name);

beforeEach(() => {
  for (const f of Object.values(vi.mocked(ipc))) (f as ReturnType<typeof vi.fn>).mockReset();
  vi.mocked(ipc.aiVisibleConnections).mockResolvedValue([]);
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = null;
  document.body.innerHTML = "";
});

it("shows_install_hint_when_missing", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status({ installed: false, version: null, path: null }));
  await show();
  expect(document.body.textContent).toContain("Claude Code is not installed");
  expect(button("Connect")).toBeUndefined();
  button("How to install Claude Code")!.click();
  expect(ipc.openClaudeInstallDocs).toHaveBeenCalledOnce();
  expect(button("Other agents")).toBeDefined();
});

it("connect_calls_ipc_and_shows_connected", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  vi.mocked(ipc.connectClaude).mockResolvedValue(status({ connected: true }));
  await show();
  expect(document.body.textContent).toContain("2.1.0");
  button("Connect")!.click();
  await vi.waitFor(() => expect(document.body.textContent).toContain("Connected to Claude Code"));
  expect(ipc.connectClaude).toHaveBeenCalledOnce();
  // Connected: the section folds to its summary line, which keeps Open in terminal.
  expect(button("Connect")).toBeUndefined();
  expect(button("Open in terminal")).toBeDefined();
});

it("reconnect_when_connected_elsewhere", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status({ connectedElsewhere: true }));
  await show();
  expect(button("Connect")).toBeUndefined();
  expect(button("Reconnect")).toBeDefined();
});

it("lists_visible_connections_with_password_hint", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  vi.mocked(ipc.aiVisibleConnections).mockResolvedValue([
    { connectionId: "a", label: "Prod", passwordSaved: true },
    { connectionId: "b", label: "Staging", passwordSaved: false },
  ]);
  await show();
  const rows = document.querySelectorAll("li");
  expect(rows).toHaveLength(2);
  expect(rows[0].textContent).toContain("Prod");
  expect(rows[0].textContent).not.toContain("password not saved");
  expect(rows[1].textContent).toContain("Staging");
  expect(rows[1].textContent).toContain("password not saved");
});

it("no_connections_hint", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  await show();
  expect(document.body.textContent).toContain("No connection is shared with AI.");
});

it("maps claude_not_found to a friendly alert", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  vi.mocked(ipc.connectClaude).mockRejectedValue("claude_not_found");
  await show();
  button("Connect")!.click();
  await vi.waitFor(() => expect(document.querySelector('[role="alert"]')).not.toBeNull());
  expect(document.querySelector('[role="alert"]')!.textContent).not.toContain("claude_not_found");
});

it("collapses to one line once connected and expands again", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status({ connected: true }));
  await show();
  expect(document.body.textContent).not.toContain("Claude Code 2.1.0");
  const toggle = document.querySelector<HTMLButtonElement>('button[aria-label="Connection details"]')!;
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  toggle.click();
  flushSync();
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(document.body.textContent).toContain("Claude Code 2.1.0");
});

it("keeps other agents and the shared list visible while connected and collapsed", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status({ connected: true }));
  vi.mocked(ipc.aiVisibleConnections).mockResolvedValue([{ connectionId: "b", label: "Staging", passwordSaved: false }]);
  await show();
  expect(button("Other agents")).toBeDefined();
  expect(document.querySelector("li")!.textContent).toContain("password not saved");
});

it("open in terminal calls ipc and shows its error", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  vi.mocked(ipc.openClaudeTerminal).mockResolvedValueOnce(undefined).mockRejectedValueOnce("run: claude --mcp-config x");
  await show();
  button("Open in terminal")!.click();
  await vi.waitFor(() => expect(ipc.openClaudeTerminal).toHaveBeenCalledOnce());
  await new Promise((r) => setTimeout(r, 0));
  flushSync();
  expect(document.querySelector('[role="alert"]')).toBeNull();
  button("Open in terminal")!.click();
  await vi.waitFor(() => expect(document.querySelector('[role="alert"]')).not.toBeNull());
  expect(document.querySelector('[role="alert"]')!.textContent).toContain("claude --mcp-config x");
});

it("disables the buttons while connect is in flight", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  let finish!: (s: ClaudeStatus) => void;
  vi.mocked(ipc.connectClaude).mockReturnValue(new Promise((r) => (finish = r)));
  await show();
  button("Connect")!.click();
  flushSync();
  expect(button("Refresh")!.disabled).toBe(true);
  expect(button("Open in terminal")!.disabled).toBe(true);
  finish(status({ connected: true }));
  await vi.waitFor(() => expect(button("Refresh")!.disabled).toBe(false));
});

it("shows other agents even when the first status load fails", async () => {
  vi.mocked(ipc.claudeStatus).mockRejectedValue("boom");
  await show();
  expect(document.querySelector('[role="alert"]')).not.toBeNull();
  expect(button("Other agents")).toBeDefined();
});

it("other agents copies the JSON and the VS Code setup", async () => {
  vi.mocked(ipc.claudeStatus).mockResolvedValue(status());
  vi.mocked(ipc.copyMcpSetup).mockResolvedValue(undefined);
  await show();
  button("Other agents")!.click();
  flushSync();
  button("Copy JSON (Cursor, Claude Desktop)")!.click();
  expect(ipc.copyMcpSetup).toHaveBeenCalledWith("json");
  await vi.waitFor(() => expect(document.querySelector('[role="status"]')!.textContent).toBe("Copied"));
  button("Other agents")!.click();
  flushSync();
  button("Copy for VS Code")!.click();
  expect(ipc.copyMcpSetup).toHaveBeenCalledWith("vscode");
});
