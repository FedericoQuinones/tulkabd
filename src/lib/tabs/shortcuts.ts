export type ShortcutAction = "new" | "close" | "palette" | "save" | "splitRight" | "splitDown";

/**
 * The lowercase key of a shortcut. A non-ASCII key (Cyrillic, Greek…) falls back to the physical
 * letter key (`KeyT` -> "t"), so Ctrl+T works on those layouts; Latin layouts keep the typed letter.
 */
export function shortcutLetter(e: KeyboardEvent): string {
  const key = e.key.toLowerCase();
  if (!/[^\x00-\x7f]/.test(key)) return key;
  const m = /^Key([A-Z])$/.exec(e.code ?? "");
  return m ? m[1].toLowerCase() : key;
}

/**
 * Maps a keydown to an app shortcut (Ctrl+T / Ctrl+W / Ctrl+P / Ctrl+S / Ctrl+\\ / Ctrl+Shift+\\, Cmd on
 * macOS). Plain inputs keep their keys; the code editor is allowed, so Ctrl+W closes with the caret
 * in it. Ctrl+P and Ctrl+S work from anywhere: no input uses them.
 */
export function shortcutAction(e: KeyboardEvent, isMac: boolean): ShortcutAction | null {
  if (e.repeat || e.defaultPrevented) return null;
  const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return null;
  // The physical key too: on some layouts the key typing \ needs AltGr, or Shift turns it into |.
  const backslash = e.key === "\\" || e.code === "Backslash";
  if (e.shiftKey && !backslash) return null;
  const key = shortcutLetter(e);
  if (key === "p") return "palette";
  if (key === "s") return "save";
  const el = e.target as HTMLElement | null;
  if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) {
    if (!el.closest?.(".cm-editor")) return null;
  }
  if (backslash) return e.shiftKey ? "splitDown" : "splitRight";
  return key === "t" ? "new" : key === "w" ? "close" : null;
}
