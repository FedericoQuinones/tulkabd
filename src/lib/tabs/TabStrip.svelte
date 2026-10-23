<script lang="ts" module>
  /**
   * Where the tab being dragged would land: a slot of a group's strip (0 = before its first tab), or
   * `slot: null` for the group's editor area, which puts it last. Each strip and group shows its own.
   */
  export const tabDrop = $state<{ over: { groupId: string; slot: number | null } | null }>({ over: null });
</script>

<script lang="ts">
  import { startDrag } from "../drag/pointerDrag";
  import { t } from "../i18n/t";
  import { groups, MAX_GROUPS } from "../workspace/layout";
  import { envColor, tabs } from "./tabs.svelte";

  let { groupId }: { groupId: string } = $props();

  const full = $derived(groups(tabs.layout).length >= MAX_GROUPS);
  const list = $derived(tabs.tabsIn(groupId));
  const mark = $derived(tabDrop.over?.groupId === groupId ? tabDrop.over.slot : null);

  function splitFrom(dir: "row" | "column") {
    tabs.focusGroup(groupId);
    tabs.splitActive(dir);
  }

  /** The slot of a strip a tab dropped at `x` takes: how many of its tabs have their middle left of `x`. */
  function slotAt(strip: Element, x: number): number {
    let slot = 0;
    for (const el of strip.querySelectorAll(".tab")) {
      const r = el.getBoundingClientRect();
      if (r.left + r.width / 2 < x) slot++;
    }
    return slot;
  }

  /**
   * Where a tab dropped on `el` goes: a strip (`tabs:<group>`) at the slot under `x`, or a group's
   * editor area (`group:<group>`), except its own group's. Null for anything else.
   */
  function dropAt(el: Element | null, x: number, tabId: string): { groupId: string; slot: number | null } | null {
    const v = el?.getAttribute("data-drop") ?? "";
    if (v.startsWith("tabs:")) return { groupId: v.slice("tabs:".length), slot: slotAt(el!, x) };
    if (v.startsWith("group:")) {
      const id = v.slice("group:".length);
      return id === tabs.groupOf(tabId) ? null : { groupId: id, slot: null };
    }
    return null;
  }

  function ondragpress(tabId: string, e: PointerEvent) {
    let x = e.clientX;
    startDrag(e, {
      payload: { kind: "tab", tabId },
      onover: (el, px) => {
        x = px;
        tabDrop.over = dropAt(el, px, tabId);
      },
      ondrop: (el) => {
        tabDrop.over = null;
        const to = dropAt(el, x, tabId);
        if (!to) return;
        if (to.slot === null) {
          tabs.moveTab(tabId, to.groupId, tabs.tabsIn(to.groupId).length);
          return;
        }
        // The slot counts the dragged tab itself when it is in that strip; moveTab wants its final index.
        const from = tabs.tabsIn(to.groupId).findIndex((tab) => tab.id === tabId);
        tabs.moveTab(tabId, to.groupId, from >= 0 && from < to.slot ? to.slot - 1 : to.slot);
      },
      oncancel: () => (tabDrop.over = null),
    });
  }
</script>

<div class="bar">
  <div class="tabs" role="tablist" aria-label={t("tabs.label")} data-drop="tabs:{groupId}">
    {#each list as tab, i (tab.id)}
      {@const on = tab.id === tabs.activeIn(groupId)?.id}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="tab"
        class:on
        class:mark-before={mark === i}
        class:mark-after={mark === list.length && i === list.length - 1}
        style:--env={envColor(tab)}
        onpointerdown={(e) => ondragpress(tab.id, e)}
      >
        <button type="button" role="tab" aria-selected={on} class="main" onclick={() => tabs.activate(tab.id)}>
          <span class="dot" style:background={envColor(tab)}></span>
          <span class="lab">{tab.title}</span>
          {#if tab.dirty}<span class="mod" title={t("tabs.unsaved")}><span class="sr">{t("tabs.unsaved")}</span></span>{/if}
        </button>
        <button type="button" class="x" aria-label={t("tabs.close", { title: tab.title })} onclick={() => tabs.close(tab.id)}>
          <svg viewBox="0 0 16 16" width="10" height="10"><path d="M3 3l10 10M13 3L3 13" /></svg>
        </button>
      </div>
    {/each}
    <button
      type="button"
      class="add"
      aria-label={t("tabs.new")}
      onclick={() => {
        tabs.focusGroup(groupId);
        tabs.newTab();
      }}>+</button
    >
  </div>
  <div class="actions">
    <button type="button" class="icon" aria-label={t("tabs.splitRight")} title={t("tabs.splitRight")} disabled={full} onclick={() => splitFrom("row")}>
      <svg viewBox="0 0 16 16" width="14" height="14"><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M8 3v10" /></svg>
    </button>
    <button type="button" class="icon" aria-label={t("tabs.splitDown")} title={t("tabs.splitDown")} disabled={full} onclick={() => splitFrom("column")}>
      <svg viewBox="0 0 16 16" width="14" height="14"><rect x="2" y="3" width="12" height="10" rx="1.5" /><path d="M2 8h12" /></svg>
    </button>
  </div>
</div>

<style>
  .bar {
    display: flex;
    align-items: stretch;
    height: 36px;
    border-bottom: 1px solid var(--border);
  }
  .tabs {
    display: flex;
    align-items: stretch;
    flex: 1;
    min-width: 0;
    overflow-x: auto;
  }
  .actions {
    display: flex;
    align-items: center;
    flex: none;
    gap: 2px;
    padding: 0 6px;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .icon svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.25;
  }
  .icon:hover:not(:disabled) {
    color: var(--text);
    background: var(--raised);
  }
  .icon:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .tab {
    position: relative;
    display: flex;
    align-items: center;
    flex: none;
    color: var(--muted);
  }
  .tab.on {
    color: var(--text);
  }
  /* Where a dragged tab would land. */
  .tab.mark-before {
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .tab.mark-after {
    box-shadow: inset -2px 0 0 var(--accent);
  }
  .tab.on::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 1px;
    background: var(--env);
  }
  .main {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 100%;
    padding: 0 6px 0 14px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    max-width: 220px;
  }
  .lab {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    flex: none;
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  /* Unsaved changes: a hollow ring, so it never reads as the environment dot. */
  .mod {
    flex: none;
    width: 7px;
    height: 7px;
    margin-left: -2px;
    border: 1.5px solid var(--soft);
    border-radius: 50%;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
  .x {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    margin-right: 8px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--muted);
    cursor: pointer;
    opacity: 0;
  }
  .x svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .tab:hover .x,
  .tab.on .x,
  .x:focus-visible {
    opacity: 1;
  }
  .x:hover {
    color: var(--text);
    background: var(--raised);
  }
  .add {
    flex: none;
    width: 36px;
    border: 0;
    background: none;
    color: var(--muted);
    font: inherit;
    font-size: 16px;
    cursor: pointer;
  }
  .add:hover {
    color: var(--text);
  }
  button:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }
</style>
