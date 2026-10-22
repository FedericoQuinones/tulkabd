export type ShortcutAction = "new" | "close" | "palette";

/**
 * Maps a keydown to an app shortcut (Ctrl+T / Ctrl+W / Ctrl+P, Cmd on macOS).
 * Plain inputs keep their keys; the code editor is allowed, so Ctrl+W closes with the caret in it.
 * Ctrl+P works from anywhere: no input uses it.
 */
export function shortcutAction(e: KeyboardEvent, isMac: boolean): ShortcutAction | null {
  if (e.repeat || e.defaultPrevented) return null;
  const mod = isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey || e.shiftKey) return null;
  const key = e.key.toLowerCase();
  if (key === "p") return "palette";
  const el = e.target as HTMLElement | null;
  if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable)) {
    if (!el.closest?.(".cm-editor")) return null;
  }
  return key === "t" ? "new" : key === "w" ? "close" : null;
}
