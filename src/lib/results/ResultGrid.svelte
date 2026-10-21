<script lang="ts">
  import { onDestroy } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { Value } from "../ipc";
  import type { PaneResult } from "../pane/reduce";
  import { cellText } from "./cellText";
  import { isSelected, type GridSelection } from "./selection";
  import { WINDOW_SIZE, visibleRange, windowOffsets } from "./virtual";

  interface Props {
    tabId: string;
    resultIndex: number;
    result: PaneResult;
  }
  let { tabId, resultIndex, result }: Props = $props();

  const ROW_HEIGHT = 34;
  const HEADER_HEIGHT = 36;
  const ID_WIDTH = 56;
  const NUMERIC = new Set(["int", "bigint", "smallint", "tinyint", "decimal", "numeric", "float", "real", "money", "smallmoney"]);

  // The parent keys this component by run, so the cache and widths die with the run.
  const rowCache = new Map<number, Value[]>();
  const inflight = new Set<number>();
  let alive = true;
  onDestroy(() => (alive = false));
  let version = $state(0);

  let selection = $state<GridSelection | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let widths = $state<number[]>([]);

  const columns = $derived(result.columns);
  const total = $derived(result.total);
  const numeric = $derived(columns.map((c) => NUMERIC.has(c.typeName.toLowerCase())));
  const colWidth = (i: number) => widths[i] ?? (numeric[i] ? 120 : 180);
  const totalWidth = $derived(columns.reduce((sum, _, i) => sum + colWidth(i), ID_WIDTH));
  const range = $derived(visibleRange(scrollTop, Math.max(viewportHeight - HEADER_HEIGHT, 0), ROW_HEIGHT, total));
  const indexes = $derived(Array.from({ length: range.end - range.start }, (_, i) => range.start + i));

  export function getSelection(): GridSelection | null {
    return selection;
  }

  function loadedIn(offset: number, size: number): number {
    let n = 0;
    for (let i = offset; i < offset + size; i++) if (rowCache.has(i)) n++;
    return n;
  }

  async function load(offset: number) {
    // Rows still streaming in: ask for what exists now and refetch when `total` grows.
    const limit = Math.min(WINDOW_SIZE, total - offset);
    if (limit <= 0 || inflight.has(offset) || loadedIn(offset, limit) === limit) return;
    inflight.add(offset);
    try {
      const rows = await ipc.fetchRows(tabId, resultIndex, offset, WINDOW_SIZE);
      if (!alive) return;
      rows.forEach((r, i) => rowCache.set(offset + i, r));
      version++;
    } catch {
      // Left as placeholders; scrolling again retries.
    } finally {
      inflight.delete(offset);
    }
  }

  $effect(() => {
    // Re-runs on scroll and when `total` grows.
    void total;
    for (const o of windowOffsets(range.start, range.end)) void load(o);
  });

  function rowAt(i: number): Value[] | undefined {
    void version;
    return rowCache.get(i);
  }

  function pickCell(e: MouseEvent, row: number, col: number) {
    if (e.shiftKey && selection?.kind === "cells") selection = { ...selection, focus: { row, col } };
    else selection = { kind: "cells", anchor: { row, col }, focus: { row, col } };
  }

  function pickRow(e: MouseEvent, row: number) {
    if (e.shiftKey && selection?.kind === "rows") selection = { ...selection, focus: row };
    else selection = { kind: "rows", anchor: row, focus: row };
  }

  function startResize(e: PointerEvent, col: number) {
    e.preventDefault();
    const startX = e.clientX;
    const startW = colWidth(col);
    const move = (ev: PointerEvent) => {
      widths[col] = Math.max(48, startW + ev.clientX - startX);
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
</script>

<div
  class="grid"
  role="grid"
  aria-label={t("results.grid")}
  aria-rowcount={total}
  bind:clientHeight={viewportHeight}
  onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
>
  <div class="inner" style:width="{totalWidth}px" style:height="{HEADER_HEIGHT + total * ROW_HEIGHT}px">
    <div class="head" style:height="{HEADER_HEIGHT}px" role="row">
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
      <div class="row" style:top="{HEADER_HEIGHT + i * ROW_HEIGHT}px" style:height="{ROW_HEIGHT}px" role="row">
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
    font-size: 12px;
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
  .head .id {
    cursor: default;
  }
  .row {
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
