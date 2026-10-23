<script lang="ts">
  import { t } from "../i18n/t";
  import { tabs } from "../tabs/tabs.svelte";
  import GroupView from "./GroupView.svelte";
  import { groups, resize, type Layout } from "./layout";

  type SplitNode = Extract<Layout, { kind: "split" }>;

  /** Smallest share of its split a group can be dragged down to. */
  const MIN = 0.15;
  const STEP = 0.05;

  /**
   * Moves the border between children `i` and `i + 1` by `delta` (a share of the split). Each side keeps
   * MIN, or its current size if already below it, so a border never squeezes a child further.
   */
  function moved(sizes: number[], i: number, delta: number): number[] {
    const pair = sizes[i] + sizes[i + 1];
    const lo = Math.min(MIN, sizes[i]);
    const hi = pair - Math.min(MIN, sizes[i + 1]);
    const next = [...sizes];
    next[i] = Math.min(Math.max(sizes[i] + delta, lo), hi);
    next[i + 1] = pair - next[i];
    return next;
  }

  function setSizes(path: number[], sizes: number[]) {
    tabs.layout = resize(tabs.layout, path, sizes);
  }

  let drag: { path: number[]; i: number; row: boolean; start: number; extent: number; sizes: number[] } | null = null;

  function dragStart(e: PointerEvent, node: SplitNode, path: number[], i: number) {
    if (e.button !== 0) return;
    e.preventDefault();
    const sep = e.currentTarget as HTMLElement;
    const box = sep.parentElement!.getBoundingClientRect();
    const row = node.dir === "row";
    drag = { path, i, row, start: row ? e.clientX : e.clientY, extent: row ? box.width : box.height, sizes: [...node.sizes] };
    sep.setPointerCapture(e.pointerId);
  }

  function dragMove(e: PointerEvent) {
    if (!drag || !(drag.extent > 0)) return;
    const delta = ((drag.row ? e.clientX : e.clientY) - drag.start) / drag.extent;
    setSizes(drag.path, moved(drag.sizes, drag.i, delta));
  }

  function dragEnd() {
    drag = null;
  }

  function sepKey(e: KeyboardEvent, node: SplitNode, path: number[], i: number) {
    const row = node.dir === "row";
    const step = e.key === (row ? "ArrowLeft" : "ArrowUp") ? -STEP : e.key === (row ? "ArrowRight" : "ArrowDown") ? STEP : 0;
    if (step === 0) return;
    e.preventDefault();
    setSizes(path, moved(node.sizes, i, step));
  }
</script>

{#snippet view(l: Layout, path: number[])}
  {#if l.kind === "group"}
    <GroupView groupId={l.id} />
  {:else}
    <div class="split" class:row={l.dir === "row"} class:column={l.dir === "column"}>
      {#each l.children as child, i (groups(child).join(" "))}
        {#if i > 0}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
          <div
            class="sep"
            role="separator"
            aria-orientation={l.dir === "row" ? "vertical" : "horizontal"}
            aria-label={t("tabs.resizeGroups")}
            aria-valuenow={Math.round(l.sizes[i - 1] * 100)}
            aria-valuemin={0}
            aria-valuemax={100}
            tabindex="0"
            onpointerdown={(e) => dragStart(e, l, path, i - 1)}
            onpointermove={dragMove}
            onpointerup={dragEnd}
            onpointercancel={dragEnd}
            onkeydown={(e) => sepKey(e, l, path, i - 1)}
          ></div>
        {/if}
        <div class="cell" style:flex-grow={l.sizes[i]}>
          {@render view(child, [...path, i])}
        </div>
      {/each}
    </div>
  {/if}
{/snippet}

<div class="root">
  {@render view(tabs.layout, [])}
</div>

<style>
  .root,
  .split {
    height: 100%;
    min-width: 0;
    min-height: 0;
  }
  .split {
    display: flex;
  }
  .split.column {
    flex-direction: column;
  }
  .cell {
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .sep {
    position: relative;
    flex: none;
    background: var(--border);
    outline: none;
  }
  .row > .sep {
    width: 1px;
  }
  .column > .sep {
    height: 1px;
  }
  /* A wider hit area than the 1 px line. */
  .sep::after {
    content: "";
    position: absolute;
    z-index: 2;
  }
  .row > .sep::after {
    top: 0;
    bottom: 0;
    left: -3px;
    right: -3px;
    cursor: col-resize;
  }
  .column > .sep::after {
    left: 0;
    right: 0;
    top: -3px;
    bottom: -3px;
    cursor: row-resize;
  }
  .sep:hover,
  .sep:focus-visible {
    background: var(--accent);
  }
</style>
