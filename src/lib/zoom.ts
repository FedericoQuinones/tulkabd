export const ZOOM_MIN = 70;
export const ZOOM_MAX = 200;
export const ZOOM_DEFAULT = 100;
const STEP = 10;

export type ZoomAction = "zoomIn" | "zoomOut" | "zoomReset";

/** The zoom (percent) after `action`: 10 % steps, kept between 70 and 200. */
export function stepZoom(zoom: number, action: ZoomAction): number {
  if (action === "zoomReset") return ZOOM_DEFAULT;
  const next = zoom + (action === "zoomIn" ? STEP : -STEP);
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, next));
}
