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
    production: { color: "#e5566f", guard: "confirm", aiAccess: "none" },
    staging: { color: "#d9a441", guard: "off", aiAccess: "readOnly" },
    test: { color: "#44a282", guard: "off", aiAccess: "readOnly" },
    local: { color: "#4a8ad6", guard: "off", aiAccess: "readOnly" },
  },
  aiMaxRows: 500,
  aiQueryTimeoutS: 30,
};

/** The saved preferences, shared by the whole interface. */
class PrefsStore {
  #value = $state<Preferences>(structuredClone(DEFAULT_PREFERENCES));
  // The last preferences the backend answered with (or defaults before the first read).
  #confirmed: Preferences = structuredClone(DEFAULT_PREFERENCES);
  // Bumped by every update, so a slow `load()` never overwrites a newer change.
  #version = 0;
  // The read in flight, if any: an update waits for it so it never saves defaults over saved values.
  #loading: Promise<unknown> | null = null;
  // The last read failed or answered nothing, so `value` may be the defaults, not what is saved.
  #loadFailed = false;
  // Writes run one after the other, so they reach the backend in the order they were requested.
  #queue: Promise<unknown> = Promise.resolve();

  /** What the interface shows: includes a change whose save is still in flight. */
  get value(): Preferences {
    return this.#value;
  }
  /** Replaces the preferences as if the backend had confirmed them (loading, tests). */
  set value(p: Preferences) {
    this.#value = p;
    this.#confirmed = structuredClone($state.snapshot(p) as Preferences);
  }
  /** The last values the backend confirmed; use these where a failed save must not leak in. */
  get confirmed(): Preferences {
    return this.#confirmed;
  }

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
        this.#loadFailed = !loaded;
        if (!loaded) return;
        if (version === this.#version) this.value = loaded;
        else this.#confirmed = structuredClone($state.snapshot(loaded) as Preferences);
      },
      () => {
        // Keep the defaults; the app works without saved preferences.
        this.#loadFailed = true;
      },
    );
    this.#loading = read;
    await read;
    if (this.#loading === read) this.#loading = null;
    return this.#value;
  }

  /**
   * Applies `patch` and saves the whole object. The backend's answer (brought into
   * range) replaces the local value. A read-only config refuses to save: the change then holds
   * for this session only and the error is rethrown.
   */
  update(patch: Partial<Preferences>): Promise<Preferences> {
    const run = this.#queue.then(() => this.#write(patch));
    this.#queue = run.catch(() => {});
    return run;
  }

  async #write(patch: Partial<Preferences>): Promise<Preferences> {
    await this.#loading;
    if (this.#loadFailed) {
      // Never save over values that were not read: try once more, and give up if that fails too.
      await this.load();
      if (this.#loadFailed) throw new Error("preferences could not be read");
    }
    this.#version++;
    const next = { ...this.#value, ...patch };
    this.#value = next;
    const version = this.#version;
    let kept: Preferences;
    try {
      kept = await setPreferences(next);
    } catch (e) {
      // The change did not reach the backend: show what the backend has.
      if (version === this.#version) this.#value = structuredClone(this.#confirmed);
      throw e;
    }
    if (kept) {
      this.#confirmed = structuredClone($state.snapshot(kept) as Preferences);
      if (version === this.#version) this.#value = kept;
    }
    return kept;
  }
}

export const prefs = new PrefsStore();
