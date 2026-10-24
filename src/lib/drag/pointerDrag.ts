// Dragging with pointer events, shared by tabs and Explorer rows. HTML5 drag and drop would need
// Tauri's native file drop turned off (Windows), and the file drop is what opens `.sql` files.

/** What is being dragged. */
export type DragPayload = { kind: "tab"; tabId: string } | { kind: "tree"; key: string };

export interface DragOptions {
  payload: DragPayload;
  /** How far (px) the pointer moves before the press becomes a drag; 4 by default. */
  threshold?: number;
  /** The pointer moved during the drag; `target` is the `[data-drop]` element under it, if any. */
  onover(target: Element | null, x: number, y: number): void;
  /** Released during the drag on `target` (the `[data-drop]` element under the pointer, if any). */
  ondrop(target: Element | null): void;
  /** Escape, a cancelled pointer or a lost window focus ended the drag. */
  oncancel(): void;
}

/** The drop target under a point: the closest element marked `data-drop`. */
function dropTargetAt(x: number, y: number): Element | null {
  return document.elementFromPoint(x, y)?.closest("[data-drop]") ?? null;
}

/** The click a browser fires after the release that ends a drag would activate what is under it. */
function swallowNextClick() {
  const swallow = (e: Event) => {
    e.preventDefault();
    e.stopPropagation();
  };
  window.addEventListener("click", swallow, { capture: true, once: true });
  // No click follows when the release lands outside the window.
  setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
}

/**
 * Called from a `pointerdown`: follows the pointer until it is released. Nothing happens until it
 * moves `threshold` px, so a press that stays put is still a click.
 */
export function startDrag(e: PointerEvent, opts: DragOptions): void {
  if (e.button !== 0) return;
  const threshold = opts.threshold ?? 4;
  const x0 = e.clientX;
  const y0 = e.clientY;
  let dragging = false;

  const end = () => {
    window.removeEventListener("pointermove", onmove, true);
    window.removeEventListener("pointerup", onup, true);
    window.removeEventListener("pointercancel", cancel, true);
    window.removeEventListener("keydown", onkey, true);
    window.removeEventListener("blur", cancel);
    window.removeEventListener("selectstart", noselect, true);
    delete document.documentElement.dataset.dragging;
  };

  function onmove(ev: PointerEvent) {
    if (ev.pointerId !== e.pointerId) return;
    if (!dragging) {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < threshold) return;
      dragging = true;
      document.documentElement.dataset.dragging = opts.payload.kind;
      window.getSelection()?.removeAllRanges();
    }
    opts.onover(dropTargetAt(ev.clientX, ev.clientY), ev.clientX, ev.clientY);
  }

  function onup(ev: PointerEvent) {
    if (ev.pointerId !== e.pointerId) return;
    end();
    if (!dragging) return;
    swallowNextClick();
    opts.ondrop(dropTargetAt(ev.clientX, ev.clientY));
  }

  function cancel() {
    end();
    if (dragging) opts.oncancel();
  }

  function onkey(ev: KeyboardEvent) {
    if (ev.key !== "Escape") return;
    if (dragging) {
      ev.preventDefault();
      ev.stopPropagation();
    }
    cancel();
  }

  function noselect(ev: Event) {
    if (dragging) ev.preventDefault();
  }

  window.addEventListener("pointermove", onmove, true);
  window.addEventListener("pointerup", onup, true);
  window.addEventListener("pointercancel", cancel, true);
  window.addEventListener("keydown", onkey, true);
  window.addEventListener("blur", cancel);
  window.addEventListener("selectstart", noselect, true);
}
