// @vitest-environment jsdom
import { afterEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  openSession: vi.fn(),
  closeSession: vi.fn(),
  completionMetadata: vi.fn(async () => ({ schemas: [], objects: [], columns: {} })),
  onAppEvent: vi.fn(async () => () => {}),
}));

import * as ipc from "../ipc";
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";

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

test("undo does not take back the text loadText put in", () => {
  const pane = new PaneState("t");
  show(pane);
  pane.loadText("CREATE VIEW v AS SELECT 1");
  flushSync();
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  expect(undo(view)).toBe(false);
  expect(text()).toBe("CREATE VIEW v AS SELECT 1");
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

test("completion metadata is fetched per connection and database, not per mount", () => {
  const pane = new PaneState("t");
  pane.connectionId = "c-meta";
  pane.database = "Ventas";
  show(pane);
  unmount(cmp!);
  show(pane);
  expect(ipc.completionMetadata).toHaveBeenCalledTimes(1);
  pane.database = "Hist";
  flushSync();
  expect(ipc.completionMetadata).toHaveBeenLastCalledWith("c-meta", "Hist");
  expect(ipc.completionMetadata).toHaveBeenCalledTimes(2);
});

test("ctrl+shift+\\ next to a bracket is left to the app (split down)", () => {
  const pane = new PaneState("t");
  pane.initialDoc = "(1)";
  show(pane);
  const content = document.querySelector<HTMLElement>(".cm-content")!;
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  view.dispatch({ selection: { anchor: 0 } });
  const e = new KeyboardEvent("keydown", { key: "|", code: "Backslash", keyCode: 220, ctrlKey: true, shiftKey: true, bubbles: true, cancelable: true });
  content.dispatchEvent(e);
  expect(e.defaultPrevented).toBe(false);
  expect(view.state.selection.main.head).toBe(0);
});

test("a loaded definition is not an edit; typing is", () => {
  const pane = new PaneState("t");
  const onedit = vi.fn();
  pane.onedit = onedit;
  show(pane);
  pane.loadText("CREATE VIEW v AS SELECT 1");
  flushSync();
  expect(text()).toBe("CREATE VIEW v AS SELECT 1");
  expect(onedit).not.toHaveBeenCalled();
  const view = EditorView.findFromDOM(document.querySelector<HTMLElement>(".cm-editor")!)!;
  view.dispatch({ changes: { from: 0, insert: "-- mine\n" }, userEvent: "input.type" });
  expect(onedit).toHaveBeenCalledTimes(1);
  expect(pane.edits).toBe(1);
});
