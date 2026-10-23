<script lang="ts">
  import { t } from "../i18n/t";
  import QueryPane from "../pane/QueryPane.svelte";
  import StructureView from "../structure/StructureView.svelte";
  import TabStrip, { tabDrop } from "../tabs/TabStrip.svelte";
  import { envColor, tabs } from "../tabs/tabs.svelte";

  let { groupId }: { groupId: string } = $props();

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const newKey = t(isMac ? "tabs.newKey.mac" : "tabs.newKey.other");
  // Only the active tab is mounted: one editor per group.
  const tab = $derived(tabs.activeIn(groupId));
  const focus = () => tabs.focusGroup(groupId);
</script>

<!-- Pointer and focus inside the group make it the active one (where Ctrl+T, the palette and the Explorer open tabs). -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="group" class:active={tabs.activeGroupId === groupId} style:--env={envColor(tab)} onpointerdowncapture={focus} onfocusin={focus}>
  <TabStrip {groupId} />
  <!-- A tab dropped on the editor area of another group goes last in it. -->
  <div class="body" class:drop={tabDrop.over?.groupId === groupId && tabDrop.over.slot === null} data-drop="group:{groupId}">
    {#if tab?.pane}
      {#key tab.id}
        <!-- Several groups remount at once (a split collapses, a session restores): only the active one takes the focus. -->
        <QueryPane pane={tab.pane} autofocus={tabs.activeGroupId === groupId} connectError={tab.connectError} onreconnect={() => tabs.reconnect(tab.id)} onretry={() => tabs.retry(tab.id)} />
      {/key}
    {:else if tab?.kind === "structure"}
      {#key tab.id}
        <StructureView connectionId={tab.connectionId!} database={tab.database!} schema={tab.schema!} name={tab.name!} />
      {/key}
    {:else}
      <div class="blank">
        <p>{t("tabs.empty", { key: newKey })}</p>
        <p class="sub">{t("tabs.emptyHint")}</p>
      </div>
    {/if}
  </div>
</div>

<style>
  .group {
    position: relative;
    height: 100%;
    min-width: 0;
    min-height: 0;
    display: grid;
    grid-template-rows: 36px minmax(0, 1fr);
    background: var(--ground);
  }
  /* Environment signal (§5.1): a 2 px line on the left edge, colored by the active tab's connection. */
  .group::before {
    content: "";
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 2px;
    background: var(--env);
    z-index: 1;
    pointer-events: none;
  }
  /* Only the active group's tab reads as current: that is where new tabs open. */
  .group:not(.active) :global(.tab.on) {
    color: var(--muted);
  }
  .body {
    min-height: 0;
    min-width: 0;
  }
  .body.drop {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .blank {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--muted);
  }
  .blank p {
    margin: 0;
  }
  .blank .sub {
    font-size: 12px;
  }
</style>
