<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { Value } from "../ipc";
  import type { PaneResult } from "../pane/reduce";
  import { cellText } from "./cellText";
  import { isSelected, type GridSelection } from "./selection";
  import { anchoredScrollTop, headerHeight, rowHeight, visibleRange } from "./virtual";
  import { zoom } from "../zoom.svelte";
  import { WindowCache } from "./windowCache";

  interface Props {
    tabId: string;
    resultIndex: number;
    result: PaneResult;
    /** False while rows may still be streaming in. */
    final: boolean;
  }
  let { tabId, resultIndex, result, final }: Props = $props();

  const ID_WIDTH = 56;
  const NUMERIC = new Set(["int", "bigint", "smallint", "tinyint", "decimal", "numeric", "float", "real", "money", "smallmoney"]);

  // The parent keys this component by run, so the cache and widths die with the run.
  let version = $state(0);
  const cache = new WindowCache(
    (offset, limit) => ipc.fetchRows(tabId, resultIndex, offset, limit),
    () => version++,
  );
  onDestroy(() => {
    cache.destroy();
    stopResize?.();
  });

  let selection = $state<GridSelection | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let gridEl = $state<HTMLDivElement>();
  let widths = $state<number[]>([]);

  const columns = $derived(result.columns);
  const total = $derived(result.total);
  const numeric = $derived(columns.map((c) => NUMERIC.has(c.typeName.toLowerCase())));
  const colWidth = (i: number) => widths[i] ?? (numeric[i] ? 120 : 180);
  const totalWidth = $derived(columns.reduce((sum, _, i) => sum + colWidth(i), ID_WIDTH));
  const rowH = $derived(rowHeight(zoom.level));
  const headH = $derived(headerHeight(zoom.level));
  const range = $derived(visibleRange(scrollTop, Math.max(viewportHeight - headH, 0), rowH, total));

  // A zoom change keeps the first visible row at the top. `scrollTop` still holds the position
  // for the old row height here: the browser's own scroll event (if any) comes later.
  let shownRowH = untrack(() => rowH);
  $effect(() => {
    const h = rowH;
    untrack(() => {
      if (h !== shownRowH && gridEl) {
        scrollTop = anchoredScrollTop(scrollTop, shownRowH, h);
        gridEl.scrollTop = scrollTop;
      }
      shownRowH = h;
    });
  });
  const indexes = $derived(Array.from({ length: range.end - range.start }, (_, i) => range.start + i));

  export function getSelection(): GridSelection | null {
    return selection;
  }

  $effect(() => {
    // Re-runs on scroll and when `total` grows.
    cache.ensure(range.start, range.end, total);
  });

  function rowAt(i: number): Value[] | undefined {
    void version;
    return cache.get(i);
  }
  const fetchError = $derived((void version, cache.error));

  function pickCell(e: MouseEvent, row: number, col: number) {
    if (e.shiftKey && selection?.kind === "cells") selection = { ...selection, focus: { row, col } };
    else selection = { kind: "cells", anchor: { row, col }, focus: { row, col } };
  }

  function pickRow(e: MouseEvent, row: number) {
    if (e.shiftKey && selection?.kind === "rows") selection = { ...selection, focus: row };
    else selection = { kind: "rows", anchor: row, focus: row };
  }

  let stopResize: (() => void) | undefined;
  function startResize(e: PointerEvent, col: number) {
    e.preventDefault();
    stopResize?.();
    const startX = e.clientX;
    const startW = colWidth(col);
    const move = (ev: PointerEvent) => {
      widths[col] = Math.max(48, startW + ev.clientX - startX);
    };
    const stop = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", stop);
      window.removeEventListener("pointercancel", stop);
      stopResize = undefined;
    };
    stopResize = stop;
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", stop);
    window.addEventListener("pointercancel", stop);
  }
</script>

{#if fetchError}
  <div class="notice" role="alert">
    <span>{t("results.fetchFailed", { error: fetchError })}</span>
    <button type="button" onclick={() => cache.retry()}>{t("results.retry")}</button>
  </div>
{:else if total === 0 && final}
  <div class="notice muted" style:top="{headH}px">{t("results.empty")}</div>
{/if}
<div
  class="grid"
  role="grid"
  aria-label={t("results.grid")}
  aria-rowcount={total}
  bind:this={gridEl}
  bind:clientHeight={viewportHeight}
  onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
>
  <div class="inner" style:width="{totalWidth}px" style:height="{headH + total * rowH}px">
    <div class="head" style:height="{headH}px" role="row">
      <div class="id" style:width="{ID_WIDTH}px" role="columnheader">{t("results.rowNumber")}</div>
      {#each columns as c, i (i)}
        <div class="h" class:r={numeric[i]} style:width="{colWidth(i)}px" role="columnheader" title={c.typeName}>
          <span class="name">{c.name}</span>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <span class="grip" aria-label={t("results.resize", { name: c.name })} onpointerdown={(e) => startResize(e, i)}></span>
        </div>
      {/each}
    </div>
    {#each indexes as i (i)}
      {@const row = rowAt(i)}
      <div class="row" style:top="{headH + i * rowH}px" style:height="{rowH}px" role="row">
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="id num"
          class:sel={isSelected(selection, i, -1) && selection?.kind === "rows"}
          style:width="{ID_WIDTH}px"
          role="rowheader"
          tabindex="-1"
          onclick={(e) => pickRow(e, i)}
        >
          {i + 1}
        </div>
        {#each columns as _, j (j)}
          {@const cell = row ? cellText(row[j]) : null}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="cell"
            class:r={numeric[j]}
            class:num={cell?.kind === "number" || cell?.kind === "binary"}
            class:null={cell?.kind === "null"}
            class:sel={isSelected(selection, i, j)}
            class:pending={!cell}
            style:width="{colWidth(j)}px"
            role="gridcell"
            tabindex="-1"
            title={cell && cell.kind !== "null" ? cell.text : undefined}
            onclick={(e) => pickCell(e, i, j)}
          >
            {cell ? cell.text : ""}
          </div>
        {/each}
      </div>
    {/each}
  </div>
</div>

<style>
  .notice {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 3;
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 8px 16px;
    background: var(--raised);
    border-top: 1px solid var(--control);
    color: var(--negative);
    font-size: 12px;
  }
  .notice.muted {
    bottom: auto;
    border: 0;
    background: none;
    color: var(--muted);
  }
  .notice button {
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    padding: 2px 10px;
    cursor: pointer;
  }
  .grid {
    height: 100%;
    overflow: auto;
    position: relative;
    user-select: none;
  }
  .inner {
    position: relative;
    min-width: 100%;
  }
  .head {
    font-size: calc(12px * var(--code-zoom) / 100);
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .h,
  .id,
  .cell {
    flex: none;
    padding: 0 12px;
    display: flex;
    align-items: center;
    overflow: hidden;
    white-space: nowrap;
  }
  .h {
    position: relative;
    color: var(--muted);
  }
  .h.r {
    justify-content: flex-end;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .grip {
    position: absolute;
    right: 0;
    top: 0;
    bottom: 0;
    width: 6px;
    cursor: col-resize;
  }
  .grip:hover {
    background: var(--control);
  }
  .id {
    justify-content: flex-end;
    padding: 0 6px 0 16px;
    color: #55565b;
    font-size: 12px;
    cursor: pointer;
  }
  .row .id,
  .head .id {
    font-size: calc(12px * var(--code-zoom) / 100);
  }
  .head .id {
    cursor: default;
  }
  .row {
    font-size: calc(var(--code-size) * var(--code-zoom) / 100);
    position: absolute;
    left: 0;
    display: flex;
    border-top: 1px solid var(--rule);
    color: var(--soft);
  }
  .cell {
    cursor: default;
  }
  .cell.r {
    justify-content: flex-end;
    color: var(--text);
  }
  .cell.num {
    font-family: var(--mono);
    font-variant-numeric: tabular-nums;
  }
  .cell.null {
    color: var(--muted);
    font-style: italic;
  }
  .cell.sel,
  .id.sel {
    background: #2f4a6e;
    color: var(--text);
  }
  .cell.pending {
    position: relative;
  }
  .cell.pending::after {
    content: "";
    width: 55%;
    height: 8px;
    border-radius: 4px;
    background: var(--rule);
  }
</style>
