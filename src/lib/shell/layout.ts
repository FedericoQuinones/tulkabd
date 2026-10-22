// Explorer panel size and visibility, remembered per machine in localStorage.
export const MIN_WIDTH = 180;
export const MAX_WIDTH = 480;
export const DEFAULT_WIDTH = 272;
const KEY = "tulkabd.explorer";

export interface Layout {
  width: number;
  hidden: boolean;
}

export function clampWidth(px: number): number {
  if (!Number.isFinite(px)) return DEFAULT_WIDTH;
  return Math.round(Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, px)));
}

/** Storage can be missing, full or blocked; every failure falls back to the defaults. */
export function loadLayout(): Layout {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Layout>;
    return {
      width: typeof raw.width === "number" ? clampWidth(raw.width) : DEFAULT_WIDTH,
      hidden: raw.hidden === true,
    };
  } catch {
    return { width: DEFAULT_WIDTH, hidden: false };
  }
}

export function saveLayout(layout: Layout): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ width: clampWidth(layout.width), hidden: layout.hidden }));
  } catch {
    // Not remembered this time; nothing else depends on it.
  }
}
