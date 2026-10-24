// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { startDrag } from "./pointerDrag";

let target: HTMLElement;
let inner: HTMLElement;
let under: Element | null;
beforeEach(() => {
  document.body.innerHTML = '<div id="src">tab</div><div id="dst" data-drop="here"><span id="inner">x</span></div>';
  target = document.getElementById("dst")!;
  inner = document.getElementById("inner")!;
  under = null;
  // jsdom has no layout: the element under the pointer is whatever the test says.
  document.elementFromPoint = vi.fn(() => under);
});
afterEach(() => {
  // Ends any drag a test left running, so its window listeners go away.
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  document.body.innerHTML = "";
});

const pointer = (type: string, x: number, y: number, init: PointerEventInit = {}) =>
  new PointerEvent(type, { clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, cancelable: true, ...init });
const fire = (type: string, x: number, y: number) => document.body.dispatchEvent(pointer(type, x, y));

function begin(threshold?: number) {
  const opts = { payload: { kind: "tab" as const, tabId: "t1" }, threshold, onover: vi.fn(), ondrop: vi.fn(), oncancel: vi.fn() };
  startDrag(pointer("pointerdown", 10, 10), opts);
  return opts;
}

test("does not start until the pointer moves 4 px by default", () => {
  under = inner;
  const d = begin();
  fire("pointermove", 13, 10);
  expect(d.onover).not.toHaveBeenCalled();
  fire("pointermove", 14, 10);
  expect(d.onover).toHaveBeenCalledWith(target, 14, 10);
});

test("the page is marked as dragging while the drag lasts", () => {
  begin();
  fire("pointermove", 12, 10);
  expect(document.documentElement.dataset.dragging).toBeUndefined();
  fire("pointermove", 30, 10);
  expect(document.documentElement.dataset.dragging).toBe("tab");
  fire("pointerup", 30, 10);
  expect(document.documentElement.dataset.dragging).toBeUndefined();
});

test("a press released before the threshold is a click, not a drop", () => {
  under = inner;
  const d = begin();
  fire("pointermove", 12, 11);
  fire("pointerup", 12, 11);
  expect(d.onover).not.toHaveBeenCalled();
  expect(d.ondrop).not.toHaveBeenCalled();
  expect(d.oncancel).not.toHaveBeenCalled();
  // The listeners are gone: a later move does nothing.
  fire("pointermove", 40, 40);
  expect(d.onover).not.toHaveBeenCalled();
});

test("the threshold can be changed", () => {
  const d = begin(10);
  fire("pointermove", 18, 10);
  expect(d.onover).not.toHaveBeenCalled();
  fire("pointermove", 20, 10);
  expect(d.onover).toHaveBeenCalledWith(null, 20, 10);
});

test("Escape cancels the drag and calls oncancel", () => {
  under = inner;
  const d = begin();
  fire("pointermove", 30, 10);
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(d.oncancel).toHaveBeenCalledTimes(1);
  fire("pointermove", 40, 10);
  fire("pointerup", 40, 10);
  expect(d.onover).toHaveBeenCalledTimes(1);
  expect(d.ondrop).not.toHaveBeenCalled();
});

test("a cancelled pointer cancels the drag", () => {
  const d = begin();
  fire("pointermove", 30, 10);
  fire("pointercancel", 30, 10);
  expect(d.oncancel).toHaveBeenCalledTimes(1);
  expect(d.ondrop).not.toHaveBeenCalled();
});

test("dropping on an element marked data-drop calls ondrop with it", () => {
  const d = begin();
  fire("pointermove", 30, 10);
  under = inner;
  fire("pointerup", 31, 12);
  expect(document.elementFromPoint).toHaveBeenLastCalledWith(31, 12);
  expect(d.ondrop).toHaveBeenCalledWith(target);
  expect(d.oncancel).not.toHaveBeenCalled();
});

test("dropping where nothing is marked data-drop calls ondrop with null", () => {
  under = document.getElementById("src");
  const d = begin();
  fire("pointermove", 30, 10);
  fire("pointerup", 30, 10);
  expect(d.ondrop).toHaveBeenCalledWith(null);
});

test("a press with another button never starts a drag", () => {
  const opts = { payload: { kind: "tree" as const, key: "k" }, onover: vi.fn(), ondrop: vi.fn(), oncancel: vi.fn() };
  startDrag(pointer("pointerdown", 10, 10, { button: 2 }), opts);
  fire("pointermove", 40, 10);
  fire("pointerup", 40, 10);
  expect(opts.onover).not.toHaveBeenCalled();
  expect(opts.ondrop).not.toHaveBeenCalled();
});

test("the click that ends a drag does not reach the page", () => {
  const onclick = vi.fn();
  document.body.addEventListener("click", onclick);
  const d = begin();
  fire("pointermove", 30, 10);
  fire("pointerup", 30, 10);
  expect(d.ondrop).toHaveBeenCalled();
  document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  expect(onclick).not.toHaveBeenCalled();
  document.body.removeEventListener("click", onclick);
});

test("the click after a plain press still goes through", () => {
  const onclick = vi.fn();
  document.body.addEventListener("click", onclick);
  begin();
  fire("pointerup", 10, 10);
  document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  expect(onclick).toHaveBeenCalledTimes(1);
  document.body.removeEventListener("click", onclick);
});
