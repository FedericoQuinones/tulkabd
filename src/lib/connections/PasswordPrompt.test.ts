// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import type { ConnectionConfig } from "../ipc";
import PasswordPrompt from "./PasswordPrompt.svelte";

let cmp: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const conn = { id: "c1", name: "Local", color: "#3b82f6" } as ConnectionConfig;

function open() {
  const onsubmit = vi.fn();
  cmp = mount(PasswordPrompt, { target: document.body, props: { connection: conn, onsubmit, oncancel: vi.fn() } });
  flushSync();
  const box = document.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
  const field = document.querySelector<HTMLInputElement>("#pw-input")!;
  field.value = "secret";
  field.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
  return { onsubmit, box };
}

test("offers to save the password, checked by default, and passes the choice", () => {
  const { onsubmit, box } = open();
  expect(box.checked).toBe(true);
  document.querySelector<HTMLButtonElement>("button.run")!.click();
  expect(onsubmit).toHaveBeenCalledWith("secret", true);
});

test("unchecking the box connects without saving", () => {
  const { onsubmit, box } = open();
  box.click();
  flushSync();
  document.querySelector<HTMLButtonElement>("button.run")!.click();
  expect(onsubmit).toHaveBeenCalledWith("secret", false);
});
