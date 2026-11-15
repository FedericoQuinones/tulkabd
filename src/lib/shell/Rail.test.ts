// @vitest-environment jsdom
import { expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import Rail from "./Rail.svelte";

it("ai item follows run log", () => {
  const onselect = vi.fn();
  const cmp = mount(Rail, { target: document.body, props: { active: null, onselect } });
  flushSync();
  const labels = [...document.querySelectorAll("nav button")].map((b) => b.getAttribute("aria-label"));
  expect(labels.indexOf("AI assistant")).toBe(labels.indexOf("Run log") + 1);
  document.querySelector<HTMLElement>('nav button[aria-label="AI assistant"]')!.click();
  expect(onselect).toHaveBeenCalledWith("rail.ai");
  unmount(cmp);
});
