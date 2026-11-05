// @vitest-environment jsdom
import { afterEach, expect, test } from "vitest";
import { modal } from "./dialogFocus";

afterEach(() => {
  document.body.innerHTML = "";
});

test("Tab from a details summary moves to the next field, not back to the first", () => {
  document.body.innerHTML =
    '<div id="p"><input id="a"><details open><summary id="s">Advanced</summary><input id="b"></details><button id="z">ok</button></div>';
  const node = document.getElementById("p")!;
  const action = modal(node);
  document.getElementById("s")!.focus();
  const e = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  node.dispatchEvent(e);
  // The browser moves on by itself: the trap must not hijack a Tab in the middle.
  expect(e.defaultPrevented).toBe(false);
  document.getElementById("z")!.focus();
  const wrap = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
  node.dispatchEvent(wrap);
  expect(wrap.defaultPrevented).toBe(true);
  expect(document.activeElement?.id).toBe("a");
  action.destroy();
});
