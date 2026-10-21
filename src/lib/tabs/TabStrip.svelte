<script lang="ts">
  import { t } from "../i18n/t";
  import { connections } from "../connections/store.svelte";
  import { ENVIRONMENTS } from "../connections/env";
  import { tabs, type Tab } from "./tabs.svelte";

  function color(tab: Tab): string {
    const id = tabs.binding(tab).connectionId;
    const conn = connections.list.find((c) => c.id === id);
    return conn ? ENVIRONMENTS.find((e) => e.id === conn.environment)!.color : "var(--control)";
  }
</script>

<div class="tabs" role="tablist" aria-label={t("tabs.label")}>
  {#each tabs.list as tab (tab.id)}
    {@const on = tab.id === tabs.activeId}
    <div class="tab" class:on style:--env={color(tab)}>
      <button type="button" role="tab" aria-selected={on} class="main" onclick={() => tabs.activate(tab.id)}>
        <span class="dot" style:background={color(tab)}></span>
        <span class="lab">{tab.title}</span>
      </button>
      <button type="button" class="x" aria-label={t("tabs.close", { title: tab.title })} onclick={() => tabs.close(tab.id)}>
        <svg viewBox="0 0 16 16" width="10" height="10"><path d="M3 3l10 10M13 3L3 13" /></svg>
      </button>
    </div>
  {/each}
  <button type="button" class="add" aria-label={t("tabs.new")} onclick={() => tabs.newTab()}>+</button>
</div>

<style>
  .tabs {
    display: flex;
    align-items: stretch;
    height: 36px;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
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
