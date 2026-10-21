export type ShortcutAction = "new" | "close";

/**
 * Maps a keydown to a tab shortcut (Ctrl+T / Ctrl+W, Cmd on macOS).
 * Plain inputs keep their keys; the code editor is allowed, so Ctrl+W closes with the caret in it.
 */
export function shortcutAction(e: KeyboardEvent, isMac: boolean): ShortcutAction | null {
  const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey || e.shiftKey) return null;
  const el = e.target as HTMLElement | null;
  if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) {
    if (!el.closest?.(".cm-editor")) return null;
  }
  const key = e.key.toLowerCase();
  return key === "t" ? "new" : key === "w" ? "close" : null;
}
