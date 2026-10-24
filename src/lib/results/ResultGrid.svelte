<script lang="ts" module>
  let nextGridId = 0;
</script>

<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import ContextMenu from "../explorer/ContextMenu.svelte";
  import type { MenuItem } from "../explorer/tree";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { ColumnFilter, Value } from "../ipc";
  import type { PaneResult } from "../pane/reduce";
  import { cellText } from "./cellText";
  import ColumnMenu from "./ColumnMenu.svelte";
  import { gridKey, type Cell } from "./gridKeys";
  import { isSelected, type GridSelection } from "./selection";
  import { nextSort, withColumnFilter, type ResultView } from "./view";
  import { anchoredScrollTop, headerHeight, rowHeight, visibleRange } from "./virtual";
  import { zoom } from "../zoom.svelte";
  import { WindowCache } from "./windowCache";

  interface Props {
    tabId: string;
    resultIndex: number;
    result: PaneResult;
    /** False while rows may still be streaming in. */
    final: boolean;
    /** The sort and filters of this result; it outlives the grid. */
    view: ResultView;
    /** True once the run ended: only then can the view change (D4). */
    ready: boolean;
    /** Ctrl+C in the grid. */
    oncopy: () => void;
  }
  let { tabId, resultIndex, result, final, view, ready, oncopy }: Props = $props();

  const ID_WIDTH = 56;
  const NUMERIC = new Set(["int", "bigint", "smallint", "tinyint", "decimal", "numeric", "float", "real", "money", "smallmoney"]);
  const gridId = `result-grid-${++nextGridId}`;

  // The parent keys this component by run, so the cache and widths die with the run.
  let version = $state(0);
  const cache = new WindowCache(
    (offset, limit) => ipc.fetchRows(tabId, resultIndex, offset, limit),
    () => version++,
  );

  // The view is read through `viewVersion`, which grows on each of its changes.
  let viewVersion = $state(0);
  const v = untrack(() => view);
  let seenGeneration = v.generation;
  const unsubscribe = v.subscribe(() => {
    // The backend's rows are in a new order: start over at the top.
    if (v.generation !== seenGeneration) {
      seenGeneration = v.generation;
      cache.clear();
      selection = null;
      active = null;
      scrollTop = 0;
      if (gridEl) gridEl.scrollTop = 0;
    }
    viewVersion++;
  });

  onDestroy(() => {
    unsubscribe();
    cache.destroy();
    stopResize?.();
  });

  let selection = $state<GridSelection | null>(null);
  let active = $state<Cell | null>(null);
  let scrollTop = $state(0);
  let viewportHeight = $state(0);
  let gridEl = $state<HTMLDivElement>();
  let widths = $state<number[]>([]);
  let columnMenu = $state<{ column: number; x: number; y: number; trigger: HTMLElement } | null>(null);
  let cellMenu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const spec = $derived((void viewVersion, v.spec));
  const viewPending = $derived((void viewVersion, v.pending));
  const columns = $derived(result.columns);
  const total = $derived((void viewVersion, v.visible ?? result.total));
  const numeric = $derived(columns.map((c) => NUMERIC.has(c.typeName.toLowerCase())));
  const colWidth = (i: number) => widths[i] ?? (numeric[i] ? 120 : 180);
  const totalWidth = $derived(columns.reduce((sum, _, i) => sum + colWidth(i), ID_WIDTH));
  const rowH = $derived(rowHeight(zoom.level));
  const headH = $derived(headerHeight(zoom.level));
  const bodyHeight = $derived(Math.max(viewportHeight - headH, 0));
  const range = $derived(visibleRange(scrollTop, bodyHeight, rowH, total));
  const page = $derived(Math.max(1, Math.floor(bodyHeight / rowH)));

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
    // Re-runs on scroll and when `total` grows. While a view call runs, the backend may answer
    // in either order: wait, and keep showing the rows already here.
    if (!viewPending) cache.ensure(range.start, range.end, total);
  });

  function rowAt(i: number): Value[] | undefined {
    void version;
    return cache.get(i);
  }
  const fetchError = $derived((void version, cache.error));
  const cellId = (row: number, col: number) => `${gridId}-${row}-${col}`;
  const activeId = $derived(active && active.row >= range.start && active.row < range.end ? cellId(active.row, active.col) : undefined);

  function sortDirection(col: number): "ascending" | "descending" | undefined {
    if (spec.sort?.column !== col) return undefined;
    return spec.sort.descending ? "descending" : "ascending";
  }

  function sortBy(col: number) {
    if (!ready) return;
    v.set({ ...v.spec, sort: nextSort(v.spec.sort, col) });
  }

  function filterColumn(col: number, filter: ColumnFilter | null) {
    if (!ready) return;
    v.set({ ...v.spec, columns: withColumnFilter(v.spec.columns, col, filter) });
  }

  function openColumnMenu(e: MouseEvent, column: number) {
    const trigger = e.currentTarget as HTMLElement;
    const r = trigger.getBoundingClientRect();
    columnMenu = { column, x: r.left, y: r.bottom + 2, trigger };
  }

  function closeColumnMenu(refocus: boolean) {
    const trigger = columnMenu?.trigger;
    columnMenu = null;
    if (refocus) trigger?.focus();
  }

  /** "Filter by this value": the text the cell shows (the backend accepts its shortened form); NULL is Is NULL. */
  function filterByValue(at: Cell, value: Value) {
    filterColumn(
      at.col,
      value.kind === "null" ? { kind: "isNull", column: at.col } : { kind: "equals", column: at.col, text: cellText(value).text },
    );
  }

  function oncontextmenu(e: MouseEvent) {
    const target = e.target as HTMLElement;
    const el = target.closest<HTMLElement>("[data-col]");
    // The context-menu key on the grid opens the menu of the active cell.
    const at: Cell | null = el ? { row: Number(el.dataset.row), col: Number(el.dataset.col) } : target === gridEl ? active : null;
    if (!at) return;
    e.preventDefault();
    const value = cache.get(at.row)?.[at.col];
    if (!ready || !value) return;
    if (!isSelected(selection, at.row, at.col)) {
      active = at;
      selection = { kind: "cells", anchor: at, focus: at };
    }
    let x = e.clientX;
    let y = e.clientY;
    if (!el) {
      const r = (document.getElementById(cellId(at.row, at.col)) ?? gridEl!).getBoundingClientRect();
      x = r.left;
      y = r.bottom;
    }
    cellMenu = { x, y, items: [{ id: "filterValue", label: t("results.filter.byValue"), run: () => filterByValue(at, value) }] };
  }

  function closeCellMenu(refocus: boolean) {
    cellMenu = null;
    if (refocus) gridEl?.focus();
  }

  function onkeydown(e: KeyboardEvent) {
    // Header buttons keep their keys.
    if (e.target !== gridEl && (e.target as HTMLElement).closest("button")) return;
    const out = gridKey(e, { active, rows: total, cols: columns.length, selection, page });
    if (out === null) return;
    e.preventDefault();
    if (out === "copy") {
      oncopy();
      return;
    }
    active = out.active;
    selection = out.selection;
    if (active) reveal(active);
  }

  /** Scrolls so the cell is fully in view. */
  function reveal(cell: Cell) {
    if (!gridEl) return;
    const top = cell.row * rowH;
    if (top < scrollTop) scrollTop = top;
    else if (top + rowH > scrollTop + bodyHeight) scrollTop = Math.max(0, top + rowH - bodyHeight);
    gridEl.scrollTop = scrollTop;
    let left = ID_WIDTH;
    for (let i = 0; i < cell.col; i++) left += colWidth(i);
    const right = left + colWidth(cell.col);
    if (cell.col === 0) gridEl.scrollLeft = 0;
    else if (left < gridEl.scrollLeft) gridEl.scrollLeft = left;
    else if (right > gridEl.scrollLeft + gridEl.clientWidth) gridEl.scrollLeft = right - gridEl.clientWidth;
  }

  function pickCell(e: MouseEvent, row: number, col: number) {
    if (e.shiftKey && selection?.kind === "cells") selection = { ...selection, focus: { row, col } };
    else selection = { kind: "cells", anchor: { row, col }, focus: { row, col } };
    active = { row, col };
    // The grid, not the cell, holds the focus: a cell scrolled out of view is removed.
    gridEl?.focus({ preventScroll: true });
  }

  function pickRow(e: MouseEvent, row: number) {
    if (e.shiftKey && selection?.kind === "rows") selection = { ...selection, focus: row };
    else selection = { kind: "rows", anchor: row, focus: row };
    active = { row, col: active?.col ?? 0 };
    gridEl?.focus({ preventScroll: true });
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
{:else if total === 0 && final && !viewPending}
  <div class="notice muted" style:top="{headH}px">{t(spec.text || spec.columns.length ? "results.noMatches" : "results.empty")}</div>
{/if}
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="grid"
  class:busy={viewPending}
  role="grid"
  tabindex="0"
  aria-label={t("results.grid")}
  aria-rowcount={total + 1}
  aria-colcount={columns.length + 1}
  aria-multiselectable="true"
  aria-activedescendant={activeId}
  aria-busy={viewPending}
  bind:this={gridEl}
  bind:clientHeight={viewportHeight}
  onscroll={(e) => (scrollTop = e.currentTarget.scrollTop)}
  {onkeydown}
  {oncontextmenu}
>
  <div class="inner" style:width="{totalWidth}px" style:height="{headH + total * rowH}px">
    <div class="head" style:height="{headH}px" role="row" aria-rowindex={1}>
      <div class="id" style:width="{ID_WIDTH}px" role="columnheader" aria-colindex={1}>{t("results.rowNumber")}</div>
      {#each columns as c, i (i)}
        {@const dir = sortDirection(i)}
        {@const filtered = spec.columns.some((f) => f.column === i)}
        <div class="h" class:r={numeric[i]} style:width="{colWidth(i)}px" role="columnheader" aria-colindex={i + 2} aria-sort={dir}>
          <button
            type="button"
            class="sort"
            disabled={!ready}
            title={ready ? c.typeName : t("results.viewWhenDone")}
            onclick={() => sortBy(i)}
          >
            <span class="name">{c.name}</span>
            {#if dir}<span class="arrow" aria-hidden="true">{dir === "ascending" ? "↑" : "↓"}</span>{/if}
          </button>
          <button
            type="button"
            class="filter"
            class:on={filtered}
            disabled={!ready}
            aria-label={t("results.columnMenu", { name: c.name })}
            title={ready ? t("results.columnMenu", { name: c.name }) : t("results.viewWhenDone")}
            aria-haspopup="menu"
            aria-expanded={columnMenu?.column === i}
            onclick={(e) => openColumnMenu(e, i)}>▾</button
          >
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <span class="grip" aria-label={t("results.resize", { name: c.name })} onpointerdown={(e) => startResize(e, i)}></span>
        </div>
      {/each}
    </div>
    {#each indexes as i (i)}
      {@const row = rowAt(i)}
      <div class="row" style:top="{headH + i * rowH}px" style:height="{rowH}px" role="row" aria-rowindex={i + 2}>
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          class="id num"
          class:sel={selection?.kind === "all" || (selection?.kind === "rows" && isSelected(selection, i, -1))}
          style:width="{ID_WIDTH}px"
          role="rowheader"
          aria-colindex={1}
          tabindex="-1"
          onclick={(e) => pickRow(e, i)}
        >
          {i + 1}
        </div>
        {#each columns as _, j (j)}
          {@const cell = row ? cellText(row[j]) : null}
          {@const selected = isSelected(selection, i, j)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            id={cellId(i, j)}
            class="cell"
            class:r={numeric[j]}
            class:num={cell?.kind === "number" || cell?.kind === "binary"}
            class:null={cell?.kind === "null"}
            class:sel={selected}
            class:active={active?.row === i && active.col === j}
            class:pending={!cell}
            style:width="{colWidth(j)}px"
            role="gridcell"
            aria-colindex={j + 2}
            aria-selected={selected}
            tabindex="-1"
            data-row={i}
            data-col={j}
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
{#if columnMenu}
  {@const col = columnMenu.column}
  <ColumnMenu
    name={columns[col]?.name ?? ""}
    column={col}
    filter={spec.columns.find((f) => f.column === col)}
    x={columnMenu.x}
    y={columnMenu.y}
    onpick={filterColumn}
    onclose={closeColumnMenu}
  />
{/if}
{#if cellMenu}
  <ContextMenu items={cellMenu.items} x={cellMenu.x} y={cellMenu.y} label={t("results.cellMenu")} onclose={closeCellMenu} />
{/if}

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
    outline: none;
  }
  .grid.busy .row {
    opacity: 0.6;
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
    padding: 0 6px 0 0;
    color: var(--muted);
  }
  .sort {
    flex: 1;
    min-width: 0;
    height: 100%;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 0 0 12px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .h.r .sort {
    justify-content: flex-end;
  }
  .sort:disabled {
    cursor: default;
  }
  .sort:hover:not(:disabled),
  .sort:focus-visible {
    outline: none;
    color: var(--text);
  }
  .arrow {
    color: var(--text);
  }
  .filter {
    flex: none;
    width: 18px;
    height: 20px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: var(--radius-control);
    background: none;
    color: var(--muted);
    font: inherit;
    cursor: pointer;
    opacity: 0;
  }
  .h:hover .filter:not(:disabled),
  .filter:focus-visible,
  .filter.on,
  .filter[aria-expanded="true"] {
    opacity: 1;
  }
  .filter:focus-visible {
    outline: none;
    border-color: var(--accent);
  }
  .filter.on {
    color: var(--accent);
  }
  .filter:disabled {
    cursor: default;
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
  .cell.active {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .grid:not(:focus) .cell.active {
    box-shadow: none;
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
