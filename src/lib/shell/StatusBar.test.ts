// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { SessionInfo } from "../ipc";
import StatusBar from "./StatusBar.svelte";

let cmp: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const session = { server: { version: "Microsoft SQL Server 2022 (RTM) - 16.0.1000.6", user: "sa" } } as unknown as SessionInfo;
function show(props: Partial<{ status: "idle" | "connecting" | "running" | "disconnected"; session: SessionInfo | null; error: string | null }>) {
  cmp = mount(StatusBar, { target: document.body, props: { status: "idle", session: null, cursor: { line: 1, col: 1 }, onreconnect: vi.fn(), ...props } });
  flushSync();
  return document.body.textContent ?? "";
}

test("no connection, connecting and connected each read differently", () => {
  expect(show({})).toContain("No connection");
  unmount(cmp!);
  document.body.innerHTML = "";
  expect(show({ status: "connecting" })).toContain("Connecting...");
  unmount(cmp!);
  document.body.innerHTML = "";
  expect(show({ session })).toContain("connected");
});

test("a failed connect leaves its reason in the status bar", () => {
  const text = show({ error: "Login failed" });
  expect(text).toContain("No connection");
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("Login failed");
});

test("a lost connection offers Reconnect", () => {
  const onreconnect = vi.fn();
  cmp = mount(StatusBar, { target: document.body, props: { status: "disconnected", session, cursor: { line: 1, col: 1 }, onreconnect } });
  flushSync();
  document.querySelector<HTMLButtonElement>("button")!.click();
  expect(onreconnect).toHaveBeenCalledOnce();
});
