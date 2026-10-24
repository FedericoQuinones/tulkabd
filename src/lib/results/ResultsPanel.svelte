<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { ViewSpec } from "../ipc";
  import type { PaneState } from "../pane/pane.svelte";
  import CopyToast from "./CopyToast.svelte";
  import MessagesView from "./MessagesView.svelte";
  import ResultGrid from "./ResultGrid.svelte";
  import { toIpcSelection } from "./selection";
  import { isFiltered, resultView, type ResultView } from "./view";

  interface Props {
    pane: PaneState;
    onjump: (line: number) => void;
  }
  let { pane, onjump }: Props = $props();

  const MAX_ROWS = 100_000;
  const TOAST_MS = 2500;
  const ERROR_TOAST_MS = 5000;
  const FILTER_DEBOUNCE_MS = 150;

  let tab = $state<number | "messages">(0);
  let grid = $state<ResultGrid>();
  let toast = $state<{ text: string; error: boolean } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let copying = false;

  // A new run goes back to the first result set.
  $effect(() => {
    void pane.runCount;
    tab = 0;
    toast = null;
  });

  const active = $derived(tab !== "messages" && pane.results[tab] ? tab : "messages");
  const result = $derived(active === "messages" ? null : pane.results[active]);
  const count = (n: number) => n.toLocaleString("en-US");
  // Sorting and filtering wait for the run to end, when all the rows are in; the backend says "busy" before that.
  const ready = $derived(pane.status !== "running");

  /** Sends the view of result `index` of run `run`; a later run has new results without a view. */
  function sender(index: number, run: number) {
    return (spec: ViewSpec) =>
      run === pane.runCount ? ipc.setResultView(pane.tabId, index, spec) : Promise.reject(new Error("stale run"));
  }
  const view = $derived(active === "messages" ? null : resultView(pane, pane.runCount, active, sender(active, pane.runCount)));

  // `viewVersion` grows on each change of the active view, so the template reads it again.
  let viewVersion = $state(0);
  let filterText = $state("");
  let filterTimer: ReturnType<typeof setTimeout> | undefined;
  let pendingFilter: (() => void) | null = null;

  $effect(() => {
    const v = view;
    untrack(() => {
      // Text typed for the previous result still goes to it.
      flushFilter();
      filterText = v?.spec.text ?? "";
    });
    if (!v) return;
    return v.subscribe((error) => {
      viewVersion++;
      if (error === null) return;
      show(t("results.viewFailed", { error }), true);
      // The backend kept its previous view; so does the field, unless more is being typed.
      if (!pendingFilter) filterText = v.spec.text ?? "";
    });
  });

  function applyFilter(v: ResultView, text: string) {
    pendingFilter = null;
    clearTimeout(filterTimer);
    const next = text === "" ? null : text;
    if ((v.spec.text ?? null) !== next) v.set({ ...v.spec, text: next });
  }

  function flushFilter() {
    pendingFilter?.();
  }

  function onFilterInput(text: string) {
    filterText = text;
    const v = view;
    if (!v) return;
    clearTimeout(filterTimer);
    pendingFilter = () => applyFilter(v, text);
    filterTimer = setTimeout(() => applyFilter(v, text), FILTER_DEBOUNCE_MS);
  }

  function onFilterKey(e: KeyboardEvent) {
    if (e.key !== "Escape" || filterText === "") return;
    e.preventDefault();
    filterText = "";
    if (view) applyFilter(view, "");
  }

  const viewPending = $derived((void viewVersion, view?.pending ?? false));

  const shown = $derived.by(() => {
    void viewVersion;
    if (!view || !result || view.visible === null || !isFiltered(view.applied)) return null;
    return t("results.rowsShown", { shown: count(view.visible), total: count(result.total) });
  });

  const finishedAt = $derived.by(() => {
    const raw = pane.summary?.finishedAt;
    if (!raw) return "";
    const d = new Date(raw);
    return Number.isNaN(d.getTime()) ? raw : d.toLocaleTimeString("en-GB");
  });

  function show(text: string, error = false) {
    clearTimeout(toastTimer);
    toast = { text, error };
    toastTimer = setTimeout(() => (toast = null), error ? ERROR_TOAST_MS : TOAST_MS);
  }
  onDestroy(() => {
    clearTimeout(toastTimer);
    flushFilter();
  });

  async function copy(mode: "excel" | "ai") {
    // While a view is on its way, the selection's row indexes may belong to either order.
    if (active === "messages" || !result || copying || viewPending) return;
    const sel = grid?.getSelection() ?? null;
    const run = pane.runCount;
    copying = true;
    try {
      const out = await ipc.copyResults(pane.tabId, active, mode, toIpcSelection(sel));
      if (run !== pane.runCount) return;
      const rows = out.rows;
      const tokens = count(out.tokens);
      if (mode === "excel") show(rows === 1 ? t("results.toast.copiedOne") : t("results.toast.copied", { rows: count(rows) }));
      else
        show(
          rows === 1
            ? t("results.toast.copiedAiOne", { tokens })
            : t("results.toast.copiedAi", { rows: count(rows), tokens }),
        );
    } catch (e) {
      show(t("results.copyFailed", { error: String(e) }), true);
    } finally {
      copying = false;
    }
  }
</script>

<div class="panel">
  <div class="rhead">
    {#each pane.results as r, i (i)}
      <button type="button" class="rt" class:on={active === i} onclick={() => (tab = i)}>
        {i === 0 ? t("results.tab") : t("results.tabN", { n: i + 1 })}<span class="num">{count(r.total)}</span>
      </button>
    {/each}
    <button type="button" class="rt" class:on={active === "messages"} onclick={() => (tab = "messages")}>
      {t("pane.messages")}
    </button>
    <!-- Always mounted: a live region announces changes of its text, not its arrival. -->
    <span class="grow"><span class="meta num" aria-live="polite">{shown ?? ""}</span></span>
    {#if result}
      <input
        class="filter"
        type="search"
        spellcheck="false"
        autocomplete="off"
        placeholder={t("results.filterRows")}
        aria-label={t("results.filterRows")}
        title={ready ? undefined : t("results.viewWhenDone")}
        disabled={!ready}
        value={filterText}
        oninput={(e) => onFilterInput(e.currentTarget.value)}
        onkeydown={onFilterKey}
      />
    {/if}
    {#if result?.truncated}
      <span class="warn">{t("results.truncated", { max: count(MAX_ROWS) })}</span>
    {/if}
    {#if pane.summary}
      <span class="meta num">{t("results.elapsed", { ms: count(pane.summary.elapsedMs) })}{finishedAt ? ` · ${finishedAt}` : ""}</span>
    {/if}
    <button type="button" class="ghost" disabled={!result || viewPending} onclick={() => copy("excel")}>{t("results.copy")}</button>
    <button type="button" class="ghost" disabled={!result || viewPending} onclick={() => copy("ai")}>{t("results.copyAi")}</button>
  </div>
  <div class="body">
    {#if active === "messages" || !result}
      <MessagesView messages={pane.messages} {onjump} />
    {:else if view}
      {#key `${pane.runCount}:${active}`}
        <ResultGrid bind:this={grid} tabId={pane.tabId} resultIndex={active} {result} {view} {ready}
          final={pane.status !== "running" || pane.results.length > active + 1}
          oncopy={() => copy("excel")}
        />
      {/key}
    {/if}
    {#if toast}<CopyToast text={toast.text} error={toast.error} />{/if}
  </div>
</div>

<style>
  .panel {
    display: grid;
    grid-template-rows: 40px minmax(0, 1fr);
    min-height: 0;
    height: 100%;
  }
  .rhead {
    display: flex;
    align-items: center;
    gap: 18px;
    padding: 0 14px 0 16px;
    border-bottom: 1px solid var(--border);
  }
  .rt {
    position: relative;
    padding: 10px 0;
    border: 0;
    background: none;
    color: var(--muted);
    font: inherit;
    cursor: pointer;
  }
  .rt.on {
    color: var(--text);
  }
  .rt.on::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 1px;
    background: var(--accent);
  }
  .rt .num {
    margin-left: 5px;
    font-size: 12px;
    color: var(--muted);
  }
  .grow {
    flex: 1;
    min-width: 0;
    text-align: right;
  }
  .meta {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
  }
  .filter {
    width: 180px;
    min-width: 0;
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    outline: none;
  }
  .filter:focus {
    border-color: var(--accent);
  }
  .filter::placeholder {
    color: var(--muted);
  }
  .filter:disabled {
    opacity: 0.4;
  }
  .warn {
    color: var(--warning);
    font-size: 12px;
  }
  .ghost {
    height: 28px;
    padding: 0 11px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    font-size: 12.5px;
    cursor: pointer;
  }
  .ghost:hover:not(:disabled) {
    color: var(--text);
  }
  .ghost:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .body {
    position: relative;
    min-height: 0;
    overflow: hidden;
  }
</style>
