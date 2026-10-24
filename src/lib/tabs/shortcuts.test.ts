import { expect, test } from "vitest";
import { shortcutAction, shortcutLetter } from "./shortcuts";

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
test("ctrl+p opens the palette, also from a plain input", () => {
  expect(shortcutAction(ev("p", { ctrlKey: true }), false)).toBe("palette");
  expect(shortcutAction(ev("p", { metaKey: true }), true)).toBe("palette");
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("p", { ctrlKey: true }, input), false)).toBe("palette");
  expect(shortcutAction(ev("p", { ctrlKey: true, repeat: true }), false)).toBeNull();
});
test("non-Latin layouts fall back to the physical key", () => {
  // Russian: T, W and P type е, ц and з; Greek: τ, ς and π.
  expect(shortcutAction(ev("е", { ctrlKey: true, code: "KeyT" }), false)).toBe("new");
  expect(shortcutAction(ev("ц", { ctrlKey: true, code: "KeyW" }), false)).toBe("close");
  expect(shortcutAction(ev("з", { ctrlKey: true, code: "KeyP" }), false)).toBe("palette");
  expect(shortcutAction(ev("τ", { metaKey: true, code: "KeyT" }), true)).toBe("new");
  expect(shortcutAction(ev("π", { ctrlKey: true, code: "KeyP" }), false)).toBe("palette");
});
test("Latin layouts keep the typed letter", () => {
  // Dvorak: the key in QWERTY's T position types "y"; the one typing "t" sits on KeyK.
  expect(shortcutAction(ev("y", { ctrlKey: true, code: "KeyT" }), false)).toBeNull();
  expect(shortcutAction(ev("t", { ctrlKey: true, code: "KeyK" }), false)).toBe("new");
  expect(shortcutLetter(ev("Enter", { code: "Enter" }))).toBe("enter");
  expect(shortcutLetter(ev("ж", { code: "Semicolon" }))).toBe("ж");
});
test("ctrl+\\ splits right and ctrl+shift+\\ splits down", () => {
  expect(shortcutAction(ev("\\", { ctrlKey: true, code: "Backslash" }), false)).toBe("splitRight");
  expect(shortcutAction(ev("|", { ctrlKey: true, shiftKey: true, code: "Backslash" }), false)).toBe("splitDown");
  expect(shortcutAction(ev("\\", { metaKey: true, code: "Backslash" }), true)).toBe("splitRight");
  expect(shortcutAction(ev("\\", { ctrlKey: true, altKey: true, code: "Backslash" }), false)).toBeNull();
  // A layout where the backslash key types something else still splits by the physical key...
  expect(shortcutAction(ev("ç", { ctrlKey: true, code: "Backslash" }), false)).toBe("splitRight");
  // ...and one where \ sits on another key (with AltGr) splits by the typed key.
  expect(shortcutAction(ev("\\", { ctrlKey: true, code: "Backquote" }), false)).toBe("splitRight");
});
test("split shortcuts do not fire from a plain input, but do from the code editor", () => {
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("\\", { ctrlKey: true, code: "Backslash" }, input), false)).toBeNull();
  expect(shortcutAction(ev("|", { ctrlKey: true, shiftKey: true, code: "Backslash" }, input), false)).toBeNull();
  const cm = { tagName: "DIV", isContentEditable: true, closest: (s: string) => (s === ".cm-editor" ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("|", { ctrlKey: true, shiftKey: true, code: "Backslash" }, cm), false)).toBe("splitDown");
});
test("ctrl+s saves from the code editor, but not from a plain input", () => {
  expect(shortcutAction(ev("s", { ctrlKey: true }), false)).toBe("save");
  expect(shortcutAction(ev("s", { metaKey: true }), true)).toBe("save");
  const cm = { tagName: "DIV", isContentEditable: true, closest: (s: string) => (s === ".cm-editor" ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("s", { ctrlKey: true }, cm), false)).toBe("save");
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("s", { ctrlKey: true }, input), false)).toBeNull();
  // The tree's inline rename input sits in a treeitem, not in the editor.
  const rename = { tagName: "INPUT", isContentEditable: false, closest: (s: string) => (s === '[role="treeitem"]' ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("s", { ctrlKey: true }, rename), false)).toBeNull();
  expect(shortcutAction(ev("s", { ctrlKey: true, shiftKey: true }), false)).toBeNull();
});

test("ctrl with plus, minus and zero zoom, including in the code editor", () => {
  const cm = { tagName: "DIV", isContentEditable: true, closest: (s: string) => (s === ".cm-editor" ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("=", { ctrlKey: true }), false)).toBe("zoomIn");
  expect(shortcutAction(ev("+", { ctrlKey: true }), false)).toBe("zoomIn");
  expect(shortcutAction(ev("+", { ctrlKey: true, shiftKey: true, code: "Equal" }), false)).toBe("zoomIn");
  expect(shortcutAction(ev("-", { ctrlKey: true }), false)).toBe("zoomOut");
  expect(shortcutAction(ev("0", { ctrlKey: true }), false)).toBe("zoomReset");
  expect(shortcutAction(ev("=", { metaKey: true }, cm), true)).toBe("zoomIn");
  expect(shortcutAction(ev("-", { ctrlKey: true }, cm), false)).toBe("zoomOut");
});
test("zoom falls back to the physical key and keeps plain inputs", () => {
  expect(shortcutAction(ev("¿", { ctrlKey: true, code: "Equal" }), false)).toBe("zoomIn");
  expect(shortcutAction(ev("x", { ctrlKey: true, code: "NumpadAdd" }), false)).toBe("zoomIn");
  expect(shortcutAction(ev("x", { ctrlKey: true, code: "NumpadSubtract" }), false)).toBe("zoomOut");
  expect(shortcutAction(ev("x", { ctrlKey: true, code: "Numpad0" }), false)).toBe("zoomReset");
  expect(shortcutAction(ev("x", { ctrlKey: true, code: "Digit0" }), false)).toBe("zoomReset");
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("=", { ctrlKey: true }, input), false)).toBeNull();
  expect(shortcutAction(ev("-", { ctrlKey: true, shiftKey: true }), false)).toBeNull();
});
test("ctrl+o opens files, from the code editor too but not from a plain input", () => {
  expect(shortcutAction(ev("o", { ctrlKey: true }), false)).toBe("open");
  expect(shortcutAction(ev("O", { metaKey: true }), true)).toBe("open");
  expect(shortcutAction(ev("щ", { ctrlKey: true, code: "KeyO" }), false)).toBe("open");
  const cm = { tagName: "DIV", isContentEditable: true, closest: (s: string) => (s === ".cm-editor" ? {} : null) } as unknown as HTMLElement;
  expect(shortcutAction(ev("o", { ctrlKey: true }, cm), false)).toBe("open");
  const input = { tagName: "INPUT", isContentEditable: false, closest: () => null } as unknown as HTMLElement;
  expect(shortcutAction(ev("o", { ctrlKey: true }, input), false)).toBeNull();
  expect(shortcutAction(ev("o", { ctrlKey: true, shiftKey: true }), false)).toBeNull();
});
