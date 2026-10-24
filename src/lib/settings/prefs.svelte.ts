import { getPreferences, setPreferences, type Preferences } from "../ipc";

/** The preferences as the backend ships them when nothing is saved; shown until `load()` answers. */
export const DEFAULT_PREFERENCES: Preferences = {
  zoom: 100,
  fontSize: 13,
  restoreSession: true,
  maxRows: 100_000,
  queryTimeoutS: 0,
  copyNumbers: "system",
  runLogDays: 30,
  environments: {
    production: { color: "#e5566f", guard: "confirm" },
    staging: { color: "#d9a441", guard: "off" },
    test: { color: "#44a282", guard: "off" },
    local: { color: "#4a8ad6", guard: "off" },
  },
};

/** The saved preferences, shared by the whole interface. */
class PrefsStore {
  value = $state<Preferences>(structuredClone(DEFAULT_PREFERENCES));
  // Bumped by every update, so a slow `load()` never overwrites a newer change.
  #version = 0;
  // The read in flight, if any: an update waits for it so it never saves defaults over saved values.
  #loading: Promise<unknown> | null = null;

  get zoom() {
    return this.value.zoom;
  }
  get fontSize() {
    return this.value.fontSize;
  }
  get maxRows() {
    return this.value.maxRows;
  }
  get environments() {
    return this.value.environments;
  }

  /** Reads the saved preferences. Resolves to the current ones if that fails. */
  async load(): Promise<Preferences> {
    const version = this.#version;
    const read = getPreferences().then(
      (loaded) => {
        if (loaded && version === this.#version) this.value = loaded;
      },
      () => {
        // Keep the defaults; the app works without saved preferences.
      },
    );
    this.#loading = read;
    await read;
    if (this.#loading === read) this.#loading = null;
    return this.value;
  }

  /**
   * Applies `patch` right away and saves the whole object. The backend's answer (brought into
   * range) replaces the local value. A read-only config refuses to save: the change then holds
   * for this session only and the error is rethrown.
   */
  async update(patch: Partial<Preferences>): Promise<Preferences> {
    await this.#loading;
    this.#version++;
    const next = { ...this.value, ...patch };
    this.value = next;
    const version = this.#version;
    const kept = await setPreferences(next);
    if (kept && version === this.#version) this.value = kept;
    return kept;
  }
}

export const prefs = new PrefsStore();
