import { prefs } from "./settings/prefs.svelte";
import { stepZoom, ZOOM_DEFAULT, ZOOM_MAX, ZOOM_MIN, type ZoomAction } from "./zoom";

/**
 * Zoom of the editor and the result grid, in percent, and the base font size they scale.
 * Mirrored in `--code-zoom` and `--code-size` on `:root`. Both are kept in the preferences.
 */
class Zoom {
  level = $state(ZOOM_DEFAULT);
  // Set once the user zooms, so a slow startup load never overwrites that choice.
  #touched = false;

  #show(level: number, fontSize = prefs.fontSize) {
    this.level = level;
    const root = document.documentElement.style;
    root.setProperty("--code-zoom", String(level));
    root.setProperty("--code-size", `${fontSize}px`);
  }

  /** Paints at 100 % right away, then applies the saved level and font size once they arrive. */
  init() {
    this.#show(ZOOM_DEFAULT);
    prefs
      .load()
      .then((p) => {
        this.#show(this.#touched ? this.level : Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, p.zoom)), p.fontSize);
      })
      .catch(() => {});
  }

  /** Repaints the zoom and the font size after the Settings screen changed the preferences. */
  refresh() {
    this.#touched = true;
    this.#show(Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, prefs.zoom)));
  }

  apply(action: ZoomAction) {
    this.#touched = true;
    this.#show(stepZoom(this.level, action));
    // A read-only config refuses to save; the zoom still holds for this session.
    prefs.update({ zoom: this.level }).catch(() => {});
  }
}

export const zoom = new Zoom();
