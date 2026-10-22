// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string) => (cmd.startsWith("list_") ? [] : null)),
  Channel: class {},
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));

import App from "./App.svelte";

let cmp: ReturnType<typeof mount> | undefined;
beforeEach(() => {
  localStorage.clear();
  cmp = mount(App, { target: document.body });
  flushSync();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const rail = () => document.querySelector<HTMLButtonElement>('nav button[aria-label="Explorer"]')!;

test("the rail button hides the explorer and the workspace takes its column", () => {
  const cols = document.querySelector(".cols")!;
  expect(document.querySelector("aside.explorer")).not.toBeNull();
  expect(rail().getAttribute("aria-current")).toBe("true");
  expect(cols.classList.contains("no-explorer")).toBe(false);

  rail().click();
  flushSync();
  expect(document.querySelector("aside.explorer")).toBeNull();
  expect(rail().hasAttribute("aria-current")).toBe(false);
  // With no aside, a 3-track template would leave the workspace in the `auto` track.
  expect(cols.classList.contains("no-explorer")).toBe(true);
  expect(cols.children).toHaveLength(2);

  rail().click();
  flushSync();
  expect(document.querySelector("aside.explorer")).not.toBeNull();
  expect(cols.classList.contains("no-explorer")).toBe(false);
});
