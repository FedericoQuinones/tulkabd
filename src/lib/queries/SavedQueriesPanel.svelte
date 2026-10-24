<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import type { SavedQuery } from "../ipc";
  import { explorer } from "../explorer/tree.svelte";
  import { tabs } from "../tabs/tabs.svelte";
  import { queries, queryError } from "./queries.svelte";

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

  let filter = $state("");
  let error = $state<string | null>(null);
  let filterEl = $state<HTMLInputElement>();

  // The files may have changed outside the app since the last read.
  onMount(() => void queries.reload());

  /** Every saved query whose name or database contains the filter, grouped by database. */
  const groups = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const out: { database: string; queries: SavedQuery[] }[] = [];
    for (const item of queries.all) {
      if (q !== "" && !item.name.toLowerCase().includes(q) && !item.database.toLowerCase().includes(q)) continue;
      let g = out[out.length - 1];
      if (g?.database !== item.database) out.push((g = { database: item.database, queries: [] }));
      g.queries.push(item);
    }
    return out;
  });

  /** Opens on a connection that has the query's database, as from the palette (see `pickConnection`). */
  async function open(q: SavedQuery) {
    error = null;
    try {
      await tabs.openSavedQuery(q, explorer.savedQueryConnection(q.database));
    } catch (e) {
      error = t("palette.openFailed", { name: q.name, error: queryError(e) });
    }
  }
</script>

<div class="saved">
  <div class="filter">
    <svg class="i" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
    <input
      bind:this={filterEl}
      type="search"
      bind:value={filter}
      placeholder={t("queries.panel.filter")}
      aria-label={t("queries.panel.filter")}
      autocomplete="off"
      spellcheck="false"
      onkeydown={(e) => {
        if (e.key === "Escape" && filter !== "") {
          e.preventDefault();
          e.stopPropagation();
          filter = "";
        }
      }}
    />
    {#if filter !== ""}
      <button
        type="button"
        class="clear"
        aria-label={t("queries.panel.filterClear")}
        onclick={() => {
          filter = "";
          filterEl?.focus();
        }}
      >
        <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"><path d="M3 3l10 10M13 3L3 13" /></svg>
      </button>
    {/if}
  </div>

  <div class="sect">{t("queries.panel.label")}</div>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if queries.all.length === 0}
    <p class="none">{t("queries.panel.empty", { key: t(isMac ? "queries.saveKey.mac" : "queries.saveKey.other") })}</p>
  {:else if groups.length === 0}
    <p class="none">{t("queries.panel.none")}</p>
  {:else}
    <div class="list">
      {#each groups as group (group.database)}
        <section aria-label={group.database}>
          <h3 class="db">
            <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true"
              ><ellipse cx="8" cy="3.5" rx="5.5" ry="2" /><path d="M2.5 3.5v9c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2v-9M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2" /></svg
            >
            <span class="lab">{group.database}</span>
          </h3>
          {#each group.queries as q (q.name)}
            <button type="button" class="q" title={q.name} onclick={() => void open(q)}>
              <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true"><path d="M4 1.5h5.5L13 5v9.5H4z" /><path d="M9.5 1.5V5H13M6.5 8.5h4M6.5 11h4" /></svg>
              <span class="lab">{q.name}</span>
            </button>
          {/each}
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .saved {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .filter {
    margin: 14px 14px 6px;
    height: 34px;
    flex: none;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    color: var(--muted);
  }
  .filter:focus-within {
    border-color: var(--accent);
  }
  .filter input {
    flex: 1;
    min-width: 0;
    height: 100%;
    background: none;
    border: 0;
    outline: none;
    color: var(--text);
    font: inherit;
  }
  .filter input::placeholder {
    color: var(--muted);
  }
  .filter input::-webkit-search-cancel-button {
    display: none;
  }
  .clear {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    padding: 0;
    background: none;
    border: 0;
    color: var(--muted);
    cursor: pointer;
  }
  .clear svg {
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .clear:hover {
    color: var(--text);
  }
  .sect {
    padding: 8px 10px 4px 16px;
    color: var(--muted);
    font-size: 12px;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 6px 10px;
  }
  .db,
  .q {
    height: 28px;
    display: flex;
    align-items: center;
    gap: 7px;
    margin: 0;
    padding-right: 10px;
    white-space: nowrap;
    border-radius: 4px;
  }
  .db {
    padding-left: 6px;
    color: var(--soft);
    font: inherit;
    font-weight: 500;
  }
  .q {
    width: 100%;
    padding-left: 20px;
    background: none;
    border: 0;
    color: var(--soft);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .q:hover {
    background: var(--raised);
    color: var(--text);
  }
  .q:focus-visible,
  .clear:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .ic {
    flex: none;
    color: var(--muted);
  }
  .lab {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .none {
    margin: 8px 16px;
    color: var(--muted);
    font-size: 12px;
  }
  .error {
    margin: 4px 16px;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
</style>
