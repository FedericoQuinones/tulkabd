import { expect, test } from "vitest";
import { shortcutAction } from "./shortcuts";

const ev = (key: string, mods: Partial<KeyboardEvent> = {}, target: Partial<HTMLElement> | null = null) =>
  ({ key, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, target, ...mods }) as unknown as KeyboardEvent;

test("ctrl+t and ctrl+w map to new and close", () => {
  expect(shortcutAction(ev("t", { ctrlKey: true }), false)).toBe("new");
  expect(shortcutAction(ev("W", { ctrlKey: true }), false)).toBe("close");
});
test("macOS uses cmd, not ctrl", () => {
  expect(shortcutAction(ev("t", { metaKey: true }), true)).toBe("new");
  expect(shortcutAction(ev("t", { ctrlKey: true }), true)).toBeNull();
});
test("plain keys and extra modifiers are ignored", () => {
  expect(shortcutAction(ev("t"), false)).toBeNull();
  expect(shortcutAction(ev("t", { ctrlKey: true, shiftKey: true }), false)).toBeNull();
});
test("plain inputs keep their keys, the code editor does not", () => {
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("w", { ctrlKey: true }, input), false)).toBeNull();
  const cm = { tagName: "DIV", isContentEditable: true, closest: (s: string) => (s === ".cm-editor" ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("w", { ctrlKey: true }, cm), false)).toBe("close");
});
test("held keys and handled events are ignored", () => {
  expect(shortcutAction(ev("t", { ctrlKey: true, repeat: true }), false)).toBeNull();
  expect(shortcutAction(ev("t", { ctrlKey: true, defaultPrevented: true }), false)).toBeNull();
});
