// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({ openSession: vi.fn(), closeSession: vi.fn() }));

import { PaneState } from "../pane/pane.svelte";
import SqlEditor from "./SqlEditor.svelte";

let cmp: ReturnType<typeof mount> | undefined;
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
});

function show(pane: PaneState) {
  cmp = mount(SqlEditor, { target: document.body, props: { pane, onrun: () => {}, oncursor: () => {} } });
  flushSync();
}
const text = () => document.querySelector(".cm-content")!.textContent;

test("loadText on a mounted empty editor shows the new text", () => {
  const pane = new PaneState("t");
  show(pane);
  pane.loadText("CREATE VIEW v AS SELECT 1");
  flushSync();
  expect(text()).toBe("CREATE VIEW v AS SELECT 1");
  pane.loadText("CREATE VIEW v AS SELECT 2");
  flushSync();
  expect(text()).toBe("CREATE VIEW v AS SELECT 2");
});

test("loadText does not overwrite text the user typed", () => {
  const pane = new PaneState("t");
  pane.initialDoc = "SELECT mine";
  show(pane);
  pane.loadText("CREATE VIEW v");
  flushSync();
  expect(text()).toBe("SELECT mine");
});

test("loadText while the tab is unmounted is used on the next mount", () => {
  const pane = new PaneState("t");
  show(pane);
  unmount(cmp!);
  cmp = undefined;
  document.body.innerHTML = "";
  pane.loadText("CREATE VIEW v");
  show(pane);
  expect(text()).toBe("CREATE VIEW v");
});
