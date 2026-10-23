import { getPreferences, setZoom } from "./ipc";
import { stepZoom, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN, type ZoomAction } from "./zoom";

/** Zoom of the editor and the result grid, in percent. Mirrored in `--code-zoom` on `:root`. */
class Zoom {
  level = $state(ZOOM_DEFAULT);
  // Set once the user zooms, so a slow startup load never overwrites that choice.
  #touched = false;

  #show(level: number) {
    this.level = level;
    document.documentElement.style.setProperty("--code-zoom", String(level));
  }

  /** Paints at 100 % right away, then applies the saved level once it arrives. */
  init() {
    this.#show(ZOOM_DEFAULT);
    getPreferences()
      .then((p) => {
        if (!this.#touched) this.#show(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, p.zoom)));
      })
      .catch(() => {});
  }

  apply(action: ZoomAction) {
    this.#touched = true;
    this.#show(stepZoom(this.level, action));
    // A read-only config refuses to save; the zoom still holds for this session.
    setZoom(this.level).catch(() => {});
  }
}

export const zoom = new Zoom();
