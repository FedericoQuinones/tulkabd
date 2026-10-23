export type ShortcutAction = "new" | "close" | "palette" | "save" | "open" | "splitRight" | "splitDown" | "zoomIn" | "zoomOut" | "zoomReset";

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

/** Ctrl and plus (also `=` and numpad +), minus or zero; the physical key covers non-US layouts. */
function zoomKey(e: KeyboardEvent): ShortcutAction | null {
  const code = e.code ?? "";
  if (e.key === "=" || e.key === "+" || code === "Equal" || code === "NumpadAdd") return "zoomIn";
  if (e.key === "-" || code === "Minus" || code === "NumpadSubtract") return "zoomOut";
  if (e.key === "0" || code === "Digit0" || code === "Numpad0") return "zoomReset";
  return null;
}

/**
 * Maps a keydown to an app shortcut (Ctrl+T / Ctrl+W / Ctrl+P / Ctrl+S / Ctrl+O / Ctrl+\\ / Ctrl+Shift+\\ / Ctrl+plus, minus, 0, Cmd on
 * macOS). Plain inputs keep their keys; the code editor is allowed, so Ctrl+W closes and Ctrl+S saves
 * with the caret in it. Ctrl+P works from anywhere: no input uses it.
 */
export function shortcutAction(e: KeyboardEvent, isMac: boolean): ShortcutAction | null {
  if (e.repeat || e.defaultPrevented) return null;
  const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return null;
  // The physical key too: on some layouts the key typing \ needs AltGr, or Shift turns it into |.
  const backslash = e.key === "\\" || e.code === "Backslash";
  const zoom = zoomKey(e);
  // Shift only matters to the splits and to the zoom-in that types + with it.
  if (e.shiftKey && !backslash && zoom !== "zoomIn") return null;
  const key = shortcutLetter(e);
  if (key === "p") return "palette";
  const el = e.target as HTMLElement | null;
  if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) {
    if (!el.closest?.(".cm-editor")) return null;
  }
  if (zoom) return zoom;
  if (backslash) return e.shiftKey ? "splitDown" : "splitRight";
  return key === "t" ? "new" : key === "w" ? "close" : key === "s" ? "save" : key === "o" ? "open" : null;
}
