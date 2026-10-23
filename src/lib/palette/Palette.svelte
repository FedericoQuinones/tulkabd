<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { onMount, tick } from "svelte";
  import { t } from "../i18n/t";
  import { highlight, paletteKey } from "./hits";
  import { palette } from "./palette.svelte";

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

  let input: HTMLInputElement;
  let list = $state<HTMLDivElement>();
  // Focus goes back where it was when the palette closes without opening anything.
  const before = typeof document !== "undefined" ? (document.activeElement as HTMLElement | null) : null;

  onMount(() => input.focus());

  const res = $derived(palette.response);
  const count = (n: number, one: string, many: string) => (n === 1 ? t(one) : t(many, { count: n }));

  // Keystroke -> rendered results, logged by the backend only under TULKA_STARTUP_PROBE=1.
  let probe = true;
  async function search(value: string) {
    const t0 = performance.now();
    await palette.search(value);
    await tick();
    requestAnimationFrame(() => {
      if (!probe) return;
      const ms = (performance.now() - t0).toFixed(1);
      invoke<boolean>("probe_log", { msg: `TULKA_PALETTE_MS=${ms} q=${JSON.stringify(value)}` })
        .then((on) => (probe = on))
        .catch(() => (probe = false));
    });
  }

  function close() {
    palette.hide();
    before?.focus?.();
  }

  function onkeydown(e: KeyboardEvent) {
    // Enter, Esc and the arrows belong to the IME while it composes.
    if (e.isComposing) return;
    // Ctrl+P again keeps the palette (and stops the webview from printing).
    if ((isMac ? e.metaKey : e.ctrlKey) && e.key.toLowerCase() === "p") {
      e.preventDefault();
      return;
    }
    const atEnd = input.selectionStart === input.value.length && input.selectionEnd === input.value.length;
    const key = paletteKey(e, isMac, atEnd);
    if (!key) return;
    const hit = palette.flat[palette.selected];
    // ArrowRight on a table, or with nothing selected, keeps moving the caret.
    if (key === "definition" && (!hit || hit.object.kind === "table")) return;
    e.preventDefault();
    e.stopPropagation();
    if (key === "close") close();
    else if (key === "up" || key === "down") {
      palette.move(key === "up" ? -1 : 1);
      void tick().then(() => list?.querySelector('[aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" }));
    } else void palette.choose(key);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(e) => e.target === e.currentTarget && close()}
  onkeydown={(e) => {
    // The input handles its own keys; this catches Esc when focus left it.
    if (e.key === "Escape" && e.target !== input) {
      e.stopPropagation();
      close();
    }
  }}
>
  <div class="panel" role="dialog" aria-modal="true" aria-label={t("palette.label")}>
    <input
      bind:this={input}
      type="text"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-list"
      aria-activedescendant={palette.flat.length > 0 ? `palette-${palette.selected}` : undefined}
      aria-autocomplete="list"
      placeholder={t("palette.placeholder")}
      spellcheck="false"
      autocomplete="off"
      value={palette.query}
      oninput={(e) => void search(e.currentTarget.value)}
      {onkeydown}
    />
    {#if palette.error}<p class="error" role="alert">{palette.error}</p>{/if}
    <div class="list" id="palette-list" role="listbox" aria-label={t("palette.results")} bind:this={list}>
      {#each palette.groups as group (group.key)}
        <div class="group" role="presentation" class:off={!group.connected}>
          <span class="path">{group.path.join(" › ")}</span>
          {#if !group.connected}<span class="state">{t("palette.notConnected")}</span>{/if}
        </div>
        {#each group.hits as hit (hit)}
          {@const i = palette.flat.indexOf(hit)}
          <div
            id="palette-{i}"
            class="row"
            class:on={i === palette.selected}
            class:off={!group.connected}
            role="option"
            tabindex="-1"
            aria-selected={i === palette.selected}
            onmousemove={() => (palette.selected = i)}
            onclick={() => void palette.choose("open", i)}
          >
            <span class="kind">{t(`palette.kind.${hit.object.kind}`)}</span>
            <span class="name mono">
              {#each highlight(`${hit.object.schema}.${hit.object.name}`, hit.indices) as seg, j (j)}
                {#if seg.hit}<mark>{seg.text}</mark>{:else}{seg.text}{/if}
              {/each}
            </span>
          </div>
        {/each}
      {/each}
      {#if res && palette.query.trim() !== "" && res.hits.length === 0}
        <p class="none">{t("palette.none")}</p>
      {/if}
    </div>
    <div class="foot">
      {#if res && palette.query.trim() !== ""}
        <span>
          {t("palette.footer", {
            matches: count(res.totalMatches, "palette.matchOne", "palette.matchMany"),
            databases: count(res.databasesMatched, "palette.databaseOne", "palette.databaseMany"),
          })}
        </span>
      {/if}
      {#if res && res.indexing > 0}
        <span class="indexing">{count(res.indexing, "palette.indexingOne", "palette.indexingMany")}</span>
      {/if}
      <span class="keys">{t(isMac ? "palette.keys.mac" : "palette.keys.other")}</span>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 12vh;
    z-index: 15;
  }
  .panel {
    width: 600px;
    max-width: calc(100vw - 32px);
    max-height: 70vh;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  input {
    height: 44px;
    padding: 0 16px;
    background: transparent;
    color: var(--text);
    border: 0;
    border-bottom: 1px solid var(--border);
    font: inherit;
    font-size: 14px;
    outline: none;
  }
  .list {
    overflow-y: auto;
    min-height: 0;
    flex: 1;
    padding: 4px 0;
  }
  .group {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 8px 16px 2px;
    color: var(--muted);
    font-size: 11px;
  }
  .state {
    font-style: italic;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 4px 16px;
    cursor: pointer;
    color: var(--soft);
  }
  .row.on {
    background: var(--raised);
    color: var(--text);
  }
  .row.off {
    color: var(--muted);
  }
  .kind {
    width: 64px;
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  mark {
    background: none;
    color: var(--accent);
    font-weight: 500;
  }
  .error {
    margin: 8px 16px 0;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .none {
    margin: 12px 16px;
    color: var(--muted);
  }
  .foot {
    display: flex;
    gap: 12px;
    padding: 6px 16px;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 11px;
  }
  .keys {
    margin-left: auto;
  }
</style>
