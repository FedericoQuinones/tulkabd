// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({ createFolder: vi.fn(), listFolders: vi.fn() }));

import * as ipc from "../ipc";
import FolderDialog from "./FolderDialog.svelte";
import { explorer } from "./tree.svelte";

let cmp: ReturnType<typeof mount> | undefined;
const onclose = vi.fn();
beforeEach(() => {
  vi.resetAllMocks();
  explorer.reset();
  cmp = mount(FolderDialog, { target: document.body, props: { mode: { kind: "create", parent: null }, onclose } });
  flushSync();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

const settle = () => new Promise((r) => setTimeout(r, 0));

test("a failed action shows its error in the dialog only", async () => {
  vi.mocked(ipc.createFolder).mockRejectedValue("boom");
  const input = document.querySelector<HTMLInputElement>("#fd-name")!;
  input.value = "Proyecto X";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  document.querySelector("form")!.requestSubmit();
  await settle();
  flushSync();
  expect(document.querySelector('[role="alert"]')?.textContent).toBe("Could not do that: boom");
  // The tree behind the dialog must not repeat it, nor keep it after Cancel.
  expect(explorer.error).toBeNull();
  expect(onclose).not.toHaveBeenCalled();
});
