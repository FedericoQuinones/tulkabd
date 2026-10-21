<script lang="ts">
  import { onDestroy } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import { pane } from "../pane/pane.svelte";
  import CopyToast from "./CopyToast.svelte";
  import MessagesView from "./MessagesView.svelte";
  import ResultGrid from "./ResultGrid.svelte";
  import { toIpcSelection } from "./selection";

  interface Props {
    onjump: (line: number) => void;
  }
  let { onjump }: Props = $props();

  const MAX_ROWS = 100_000;
  const TOAST_MS = 2500;

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

  const finishedAt = $derived.by(() => {
    const raw = pane.summary?.finishedAt;
    if (!raw) return "";
    const d = new Date(raw);
    return Number.isNaN(d.getTime()) ? raw : d.toLocaleTimeString("en-GB");
  });

  function show(text: string, error = false) {
    clearTimeout(toastTimer);
    toast = { text, error };
    toastTimer = setTimeout(() => (toast = null), TOAST_MS);
  }
  onDestroy(() => clearTimeout(toastTimer));

  async function copy(mode: "excel" | "ai") {
    if (active === "messages" || !result || copying) return;
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
    <span class="grow"></span>
    {#if result?.truncated}
      <span class="warn">{t("results.truncated", { max: count(MAX_ROWS) })}</span>
    {/if}
    {#if pane.summary}
      <span class="meta num">{t("results.elapsed", { ms: count(pane.summary.elapsedMs) })}{finishedAt ? ` · ${finishedAt}` : ""}</span>
    {/if}
    <button type="button" class="ghost" disabled={!result} onclick={() => copy("excel")}>{t("results.copy")}</button>
    <button type="button" class="ghost" disabled={!result} onclick={() => copy("ai")}>{t("results.copyAi")}</button>
  </div>
  <div class="body">
    {#if active === "messages" || !result}
      <MessagesView messages={pane.messages} {onjump} />
    {:else}
      {#key `${pane.runCount}:${active}`}
        <ResultGrid bind:this={grid} tabId={pane.tabId} resultIndex={active} {result} />
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
  }
  .meta {
    color: var(--muted);
    font-size: 12px;
    white-space: nowrap;
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
