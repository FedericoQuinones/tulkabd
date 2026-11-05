// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import ContextMenu from "./ContextMenu.svelte";

let cmp: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

test("a menu opened near the bottom-right corner is moved back inside the window", () => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 200, height: 100 } as DOMRect);
  cmp = mount(ContextMenu, {
    target: document.body,
    props: { items: [{ id: "a", label: "A", run: () => {} }], x: window.innerWidth - 10, y: window.innerHeight - 10, label: "m", onclose: () => {} },
  });
  flushSync();
  const menu = document.querySelector<HTMLElement>('[role="menu"]')!;
  expect(menu.style.left).toBe(`${window.innerWidth - 204}px`);
  expect(menu.style.top).toBe(`${window.innerHeight - 104}px`);
});
