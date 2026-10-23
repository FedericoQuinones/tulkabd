<script lang="ts">
  import { t } from "../i18n/t";
  import { groups, MAX_GROUPS } from "../workspace/layout";
  import { envColor, tabs } from "./tabs.svelte";

  let { groupId }: { groupId: string } = $props();

  const full = $derived(groups(tabs.layout).length >= MAX_GROUPS);

  function splitFrom(dir: "row" | "column") {
    tabs.focusGroup(groupId);
    tabs.splitActive(dir);
  }
</script>

<div class="bar">
  <div class="tabs" role="tablist" aria-label={t("tabs.label")}>
    {#each tabs.tabsIn(groupId) as tab (tab.id)}
      {@const on = tab.id === tabs.activeIn(groupId)?.id}
      <div class="tab" class:on style:--env={envColor(tab)}>
        <button type="button" role="tab" aria-selected={on} class="main" onclick={() => tabs.activate(tab.id)}>
          <span class="dot" style:background={envColor(tab)}></span>
          <span class="lab">{tab.title}</span>
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
