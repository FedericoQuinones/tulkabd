// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import TransactionDialog from "./TransactionDialog.svelte";
import type { ClosePrompt } from "./tabs.svelte";

let cmp: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const item = { tabId: "a", connection: "Prod", database: "Sales", state: "open" as const, count: 2 };
const prompt = (over: Partial<ClosePrompt> = {}): ClosePrompt => ({ scope: "tab", items: [item], busy: false, error: null, ...over });
const button = (text: string) => [...document.querySelectorAll<HTMLButtonElement>("button")].find((b) => b.textContent?.trim() === text);

function open(p: ClosePrompt, props: { onanswer?: (c: boolean) => void; oncancel?: () => void } = {}) {
  cmp = mount(TransactionDialog, { target: document.body, props: { prompt: p, onanswer: () => {}, oncancel: () => {}, ...props } });
  flushSync();
}
const key = (k: string, shiftKey = false) =>
  document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key: k, shiftKey, bubbles: true, cancelable: true }));

test("the initial focus is Cancel, never Commit or Roll back", () => {
  open(prompt());
  expect(document.activeElement).toBe(button("Cancel"));
});

test("Escape cancels, unless an answer is being carried out", () => {
  const oncancel = vi.fn();
  open(prompt(), { oncancel });
  key("Escape");
  expect(oncancel).toHaveBeenCalledOnce();
  unmount(cmp!);
  document.body.innerHTML = "";
  const busy = vi.fn();
  open(prompt({ busy: true }), { oncancel: busy });
  key("Escape");
  expect(busy).not.toHaveBeenCalled();
});

test("Tab and Shift+Tab stay inside the dialog", () => {
  open(prompt());
  const cancel = button("Cancel")!;
  const commit = button("Commit")!;
  commit.focus();
  const forward = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  commit.dispatchEvent(forward);
  expect(forward.defaultPrevented).toBe(true);
  expect(document.activeElement).toBe(cancel);
  const back = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
  cancel.dispatchEvent(back);
  expect(back.defaultPrevented).toBe(true);
  expect(document.activeElement).toBe(commit);
});

test("the window prompt says all, and the buttons answer", () => {
  const onanswer = vi.fn();
  open(prompt({ scope: "window" }), { onanswer });
  button("Roll back all")!.click();
  button("Commit all")!.click();
  expect(onanswer.mock.calls).toEqual([[false], [true]]);
});

test("a running tab cannot be committed, only rolled back", () => {
  open(prompt({ items: [{ ...item, state: "unknown", count: 0 }] }));
  expect(button("Commit")).toBeUndefined();
  expect(button("Roll back")).toBeDefined();
  expect(document.body.textContent).toContain("Stopping it rolls back its open transaction; statements already committed stay.");
});

test("an error is shown", () => {
  open(prompt({ error: "Could not end the transaction: x. The tab stays open." }));
  expect(document.querySelector('[role="alert"]')!.textContent).toContain("Could not end");
});
