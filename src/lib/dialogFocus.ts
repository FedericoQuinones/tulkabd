const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';

/**
 * Svelte action for a modal panel: Tab and Shift+Tab cycle inside it, and when it goes away the
 * keyboard focus returns to what had it before the dialog opened.
 */
export function modal(node: HTMLElement) {
  const active = document.activeElement as HTMLElement | null;
  const before = active && !node.contains(active) ? active : null;

  function keydown(e: KeyboardEvent) {
    if (e.key !== "Tab" || e.defaultPrevented) return;
    const items = [...node.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const current = document.activeElement as HTMLElement | null;
    const inside = current !== null && items.includes(current);
    if (e.shiftKey && (current === first || !inside)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (current === last || !inside)) {
      e.preventDefault();
      first.focus();
    }
  }
  node.addEventListener("keydown", keydown);
  return {
    destroy() {
      node.removeEventListener("keydown", keydown);
      if (before?.isConnected) before.focus?.();
    },
  };
}
