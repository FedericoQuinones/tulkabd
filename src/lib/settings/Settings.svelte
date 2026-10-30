<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { AiAccess, DataLocationKey, DataLocations, Environment, GuardLevel, NumberFormat, Preferences } from "../ipc";
  import { ENVIRONMENTS } from "../connections/env";
  import { zoom } from "../zoom.svelte";
  import { prefs } from "./prefs.svelte";
  import { runLog } from "../runlog/runlog.svelte";

  interface Props {
    /** Asks to leave the screen (Escape). */
    onclose: () => void;
  }
  let { onclose }: Props = $props();

  const DEBOUNCE_MS = 300;
  const SAVED_MS = 1500;

  type Section = "general" | "environments" | "execution" | "copy" | "runLog" | "ai" | "data";
  type NumKey = "fontSize" | "zoom" | "queryTimeoutS" | "maxRows" | "runLogDays" | "aiMaxRows" | "aiQueryTimeoutS";

  let root: HTMLElement;
  // What each section's "Saved" or status note says; empty most of the time.
  let notes = $state<Partial<Record<Section, string>>>({});
  const noteTimers: Partial<Record<Section, ReturnType<typeof setTimeout>>> = {};
  let error = $state<string | null>(null);

  function note(section: Section, text: string) {
    notes[section] = text;
    clearTimeout(noteTimers[section]);
    noteTimers[section] = setTimeout(() => (notes[section] = ""), SAVED_MS);
  }

  /** Saves `patch`; the backend's answer (brought into range) is what the fields show afterwards. */
  async function save(patch: Partial<Preferences>, section: Section) {
    try {
      await prefs.update(patch);
      error = null;
      note(section, t("settings.saved"));
    } catch (e) {
      error = t("settings.saveFailed", { error: String(e) });
    }
    // The editor and the grid follow the saved font size and zoom.
    if ("fontSize" in patch || "zoom" in patch) zoom.refresh();
  }

  // Numeric fields: the text being typed, until its save ends. Without a draft a field shows the saved value.
  let drafts = $state<Partial<Record<NumKey, string>>>({});
  const timers: Partial<Record<NumKey, ReturnType<typeof setTimeout>>> = {};
  // The text already sent (or being sent) for each field, so a blur during a save does not send it again.
  const sent: Partial<Record<NumKey, string>> = {};
  // The field being typed in: the backend's value waits for the blur instead of replacing the text.
  let focused: NumKey | null = null;
  const sections: Record<NumKey, Section> = {
    fontSize: "general",
    zoom: "general",
    queryTimeoutS: "execution",
    maxRows: "execution",
    runLogDays: "runLog",
    aiMaxRows: "ai",
    aiQueryTimeoutS: "ai",
  };
  const shown = (key: NumKey) => drafts[key] ?? String(prefs.value[key]);

  function typed(key: NumKey, text: string) {
    drafts[key] = text;
    clearTimeout(timers[key]);
    timers[key] = setTimeout(() => void commit(key), DEBOUNCE_MS);
  }

  async function commit(key: NumKey) {
    clearTimeout(timers[key]);
    delete timers[key];
    const text = drafts[key];
    if (text === undefined) return;
    const done = () => {
      if (focused !== key) delete drafts[key];
    };
    const n = Number(text);
    if (sent[key] === text) {
      // Already saved, or on its way: the draft only waits for the end of that save.
      if (!saving.has(key)) done();
      return;
    }
    if (text.trim() === "" || !Number.isFinite(n)) {
      done();
      return;
    }
    // The zoom moves in steps of 10; the backend brings everything else into its range.
    const value = key === "zoom" ? Math.round(n / 10) * 10 : Math.round(n);
    sent[key] = text;
    saving.add(key);
    try {
      await save({ [key]: value }, sections[key]);
    } finally {
      saving.delete(key);
    }
    if (drafts[key] === text) done();
  }
  const saving = new Set<NumKey>();

  /** Saves what is still being typed, so closing the screen never loses a change. */
  function flush() {
    for (const key of Object.keys(timers) as NumKey[]) void commit(key);
  }
  onDestroy(() => {
    flush();
    for (const timer of Object.values(noteTimers)) clearTimeout(timer);
  });

  const decimal = new Intl.NumberFormat().formatToParts(1.5).find((p) => p.type === "decimal")?.value ?? ".";
  const formats: { id: NumberFormat; label: string }[] = [
    { id: "system", label: t("settings.numbersSystem", { sep: decimal }) },
    { id: "invariant", label: t("settings.numbersInvariant") },
  ];
  const guards: GuardLevel[] = ["off", "confirm", "block"];
  const aiLevels: AiAccess[] = ["none", "readOnly"];

  function setEnvironment(id: Environment, change: Partial<Preferences["environments"][Environment]>) {
    void save({ environments: { ...prefs.environments, [id]: { ...prefs.environments[id], ...change } } }, "environments");
  }

  // Run log.
  let confirming = $state(false);
  let cancelButton = $state<HTMLButtonElement>();
  let clearButton: HTMLButtonElement;
  let dialog = $state<HTMLElement>();

  $effect(() => {
    if (confirming) cancelButton?.focus();
  });

  function closeConfirm() {
    confirming = false;
    // Focus goes back to the button that opened it.
    queueMicrotask(() => clearButton?.focus());
  }

  async function clearLog() {
    closeConfirm();
    try {
      await ipc.clearRunLog();
      runLog.reset();
      note("runLog", t("settings.runLogCleared"));
    } catch (e) {
      error = t("settings.clearFailed", { error: String(e) });
    }
  }

  // AI assistants.
  let setupCommand = $state("");
  async function copySetup(kind: ipc.McpSetupKind) {
    try {
      await ipc.copyMcpSetup(kind);
      error = null;
      note("ai", t("settings.aiCopied"));
    } catch (e) {
      error = t("settings.aiCopyFailed", { error: String(e) });
    }
  }

  // Data.
  let locations = $state<DataLocations | null>(null);
  const places: { id: string; label: string; key: DataLocationKey; path: (l: DataLocations) => string }[] = [
    { id: "config", label: "settings.dataConfig", key: "config", path: (l) => l.configFile },
    { id: "queries", label: "settings.dataQueries", key: "queries", path: (l) => l.queriesDir },
    { id: "cache", label: "settings.dataCache", key: "data", path: (l) => l.cacheFile },
    { id: "runlog", label: "settings.dataRunLog", key: "data", path: (l) => l.runLogFile },
  ];

  async function reveal(key: DataLocationKey) {
    try {
      await ipc.revealDataLocation(key);
      error = null;
    } catch (e) {
      error = t("settings.revealFailed", { error: String(e) });
    }
  }

  async function clearCache() {
    try {
      await ipc.clearMetadataCache();
      error = null;
      note("data", t("settings.cacheCleared"));
    } catch (e) {
      error = t("settings.clearFailed", { error: String(e) });
    }
  }

  onMount(() => {
    root.focus();
    ipc
      .mcpSetup()
      .then((m) => (setupCommand = m.claudeCode))
      .catch(() => {});
    ipc
      .dataLocations()
      .then((l) => (locations = l))
      .catch(() => {});
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && !e.defaultPrevented) {
      // A dialog or the palette over this screen takes the Escape itself.
      if (!confirming && document.querySelector('[aria-modal="true"]')) return;
      e.preventDefault();
      if (confirming) closeConfirm();
      else onclose();
      return;
    }
    if (e.key !== "Tab" || !confirming || !dialog) return;
    // Tab stays inside the confirmation.
    const items = [...dialog.querySelectorAll<HTMLElement>("button")];
    const active = document.activeElement as HTMLElement | null;
    if (e.shiftKey && (active === items[0] || !items.includes(active!))) {
      e.preventDefault();
      items.at(-1)!.focus();
    } else if (!e.shiftKey && (active === items.at(-1) || !items.includes(active!))) {
      e.preventDefault();
      items[0].focus();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="settings" bind:this={root} tabindex="-1">
  <div class="page">
    <h1>{t("settings.title")}</h1>
    {#if error}<p class="err" role="alert">{error}</p>{/if}

    {#snippet head(title: string, section: Section)}
      <div class="sh">
        <h2>{title}</h2>
        <span class="saved" aria-live="polite">{notes[section] ?? ""}</span>
      </div>
    {/snippet}
    {#snippet num(key: NumKey, id: string, label: string, min: number, max: number, step = 1)}
      <div class="row">
        <label for={id}>{label}</label>
        <input
          {id}
          class="m"
          type="number"
          inputmode="numeric"
          {min}
          {max}
          {step}
          value={shown(key)}
          oninput={(e) => typed(key, e.currentTarget.value)}
          onfocus={() => (focused = key)}
          onblur={() => {
            if (focused === key) focused = null;
            void commit(key);
          }}
        />
      </div>
    {/snippet}

    <section data-section="general">
      {@render head(t("settings.general"), "general")}
      {@render num("fontSize", "set-font", t("settings.fontSize"), 10, 24)}
      {@render num("zoom", "set-zoom", t("settings.zoom"), 70, 200, 10)}
      <div class="row">
        <label for="set-restore">{t("settings.restoreSession")}</label>
        <input
          id="set-restore"
          type="checkbox"
          checked={prefs.value.restoreSession}
          onchange={(e) => void save({ restoreSession: e.currentTarget.checked }, "general")}
        />
      </div>
    </section>

    <section data-section="environments">
      {@render head(t("settings.environments"), "environments")}
      {#each ENVIRONMENTS as env (env.id)}
        {@const style = prefs.environments[env.id]}
        <div class="row">
          <span class="lbl"><span class="dot" style:background={style.color}></span>{t(env.label)}</span>
          <div class="pair">
            <input
              type="color"
              aria-label={t("settings.envColor", { env: t(env.label) })}
              value={style.color}
              onchange={(e) => setEnvironment(env.id, { color: e.currentTarget.value })}
            />
            <select
              aria-label={t("settings.envGuard", { env: t(env.label) })}
              value={style.guard}
              onchange={(e) => setEnvironment(env.id, { guard: e.currentTarget.value as GuardLevel })}
            >
              {#each guards as g (g)}<option value={g} selected={style.guard === g}>{t(`guard.${g}`)}</option>{/each}
            </select>
            <select
              aria-label={t("settings.envAi", { env: t(env.label) })}
              value={style.aiAccess}
              onchange={(e) => setEnvironment(env.id, { aiAccess: e.currentTarget.value as AiAccess })}
            >
              {#each aiLevels as a (a)}<option value={a} selected={style.aiAccess === a}>{t(`ai.${a}`)}</option>{/each}
            </select>
          </div>
        </div>
      {/each}
      <p class="hint">{t("settings.envNote")}</p>
    </section>

    <section data-section="execution">
      {@render head(t("settings.execution"), "execution")}
      {@render num("queryTimeoutS", "set-timeout", t("settings.timeout"), 0, 86_400)}
      {@render num("maxRows", "set-rows", t("settings.maxRows"), 1_000, 1_000_000, 1_000)}
    </section>

    <section data-section="copy">
      {@render head(t("settings.copy"), "copy")}
      <div class="row">
        <label for="set-numbers">{t("settings.numbers")}</label>
        <select
          id="set-numbers"
          value={prefs.value.copyNumbers}
          onchange={(e) => void save({ copyNumbers: e.currentTarget.value as NumberFormat }, "copy")}
        >
          {#each formats as f (f.id)}<option value={f.id} selected={prefs.value.copyNumbers === f.id}>{f.label}</option>{/each}
        </select>
      </div>
    </section>

    <section data-section="runLog">
      {@render head(t("settings.runLog"), "runLog")}
      {@render num("runLogDays", "set-days", t("settings.runLogDays"), 1, 365)}
      <div class="row">
        <span class="lbl"></span>
        <button type="button" class="ghost" bind:this={clearButton} onclick={() => (confirming = true)}>{t("settings.clearRunLog")}</button>
      </div>
      <p class="hint">{t("settings.runLogNote")}</p>
      <p class="hint">{t("settings.runLogAiNote")}</p>
    </section>

    <section data-section="ai">
      {@render head(t("settings.ai"), "ai")}
      {@render num("aiMaxRows", "set-ai-rows", t("settings.aiMaxRows"), 1, 10_000)}
      {@render num("aiQueryTimeoutS", "set-ai-timeout", t("settings.aiTimeout"), 1, 3_600)}
      <div class="row place">
        <div class="where">
          <span class="lbl" id="ai-claude">{t("settings.aiSetup")} · {t("settings.aiClaude")}</span>
          <code class="path">{setupCommand}</code>
        </div>
        <button type="button" class="ghost" aria-describedby="ai-claude" onclick={() => void copySetup("claude")}>
          {t("settings.copy")}
        </button>
      </div>
      <div class="row">
        <span class="lbl"></span>
        <div class="pair">
          <button type="button" class="ghost" onclick={() => void copySetup("json")}>{t("settings.aiCopyJson")}</button>
          <button type="button" class="ghost" onclick={() => void copySetup("vscode")}>{t("settings.aiCopyVscode")}</button>
        </div>
      </div>
      <p class="hint">{t("settings.aiNote")}</p>
    </section>

    <section data-section="data">
      {@render head(t("settings.data"), "data")}
      {#each places as place (place.id)}
        <div class="row place">
          <div class="where">
            <span class="lbl" id="place-{place.id}">{t(place.label)}</span>
            <code class="path">{locations ? place.path(locations) : ""}</code>
          </div>
          <button type="button" class="ghost" aria-describedby="place-{place.id}" onclick={() => void reveal(place.key)}>
            {t("settings.reveal")}
          </button>
        </div>
      {/each}
      <div class="row">
        <span class="lbl"></span>
        <button type="button" class="ghost" onclick={() => void clearCache()}>{t("settings.clearCache")}</button>
      </div>
    </section>
  </div>
</div>

{#if confirming}
  <div class="backdrop">
    <div class="confirm" role="dialog" aria-modal="true" aria-labelledby="clear-title" aria-describedby="clear-body" bind:this={dialog}>
      <div class="ct" id="clear-title">{t("settings.clearRunLogTitle")}</div>
      <p id="clear-body">{t("settings.clearRunLogBody")}</p>
      <div class="foot">
        <button type="button" class="ghost" bind:this={cancelButton} onclick={closeConfirm}>{t("settings.cancel")}</button>
        <button type="button" class="danger" onclick={() => void clearLog()}>{t("settings.clearConfirm")}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .settings {
    height: 100%;
    overflow: auto;
    outline: none;
  }
  .page {
    max-width: 640px;
    padding: 24px 32px 48px;
  }
  h1 {
    margin: 0 0 20px;
    font-size: 18px;
    font-weight: 600;
  }
  section {
    padding: 4px 0 18px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 14px;
  }
  section:last-child {
    border-bottom: 0;
  }
  .sh {
    display: flex;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 10px;
  }
  h2 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--soft);
  }
  .saved {
    color: var(--positive);
    font-size: 12px;
    min-height: 1em;
  }
  .row {
    display: grid;
    grid-template-columns: 260px minmax(0, 1fr);
    align-items: center;
    gap: 12px;
    min-height: 40px;
  }
  .row.place {
    grid-template-columns: minmax(0, 1fr) auto;
  }
  label,
  .lbl {
    color: var(--soft);
    font-size: 12.5px;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .where {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .path {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--muted);
    word-break: break-all;
  }
  .pair {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  input:not([type="checkbox"]):not([type="color"]),
  select {
    width: 160px;
    height: 32px;
    padding: 0 12px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
  }
  select {
    width: auto;
    min-width: 120px;
  }
  input.m {
    font-family: var(--mono);
  }
  input[type="color"] {
    width: 28px;
    height: 28px;
    padding: 0;
    border: 1px solid var(--control);
    border-radius: 50%;
    background: none;
    cursor: pointer;
  }
  input[type="checkbox"] {
    justify-self: start;
    margin: 0;
  }
  input:focus-visible,
  select:focus-visible,
  button:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .hint {
    margin: 6px 0 0;
    color: var(--muted);
    font-size: 12px;
  }
  .err {
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .ghost,
  .danger {
    height: 32px;
    padding: 0 14px;
    justify-self: start;
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
    background: none;
  }
  .ghost {
    color: var(--soft);
    border: 1px solid var(--control);
  }
  .ghost:hover {
    color: var(--text);
  }
  .danger {
    color: var(--negative);
    border: 1px solid var(--negative);
    font-weight: 500;
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .confirm {
    width: 380px;
    max-width: calc(100vw - 32px);
    padding: 16px 18px;
    background: var(--surface);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
  }
  .ct {
    font-weight: 600;
    margin-bottom: 6px;
  }
  .confirm p {
    margin: 0;
    color: var(--soft);
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
</style>
