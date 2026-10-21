<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "./lib/i18n/t";
  import Rail from "./lib/shell/Rail.svelte";
  import StatusBar from "./lib/shell/StatusBar.svelte";
  import type { ConnectionConfig } from "./lib/ipc";
  import ConnectionDialog from "./lib/connections/ConnectionDialog.svelte";
  import ConnectionList from "./lib/connections/ConnectionList.svelte";
  import PasswordPrompt from "./lib/connections/PasswordPrompt.svelte";
  import { connections } from "./lib/connections/store.svelte";
  import QueryPane from "./lib/pane/QueryPane.svelte";
  import StructureView from "./lib/structure/StructureView.svelte";
  import TabStrip from "./lib/tabs/TabStrip.svelte";
  import { shortcutAction } from "./lib/tabs/shortcuts";
  import { tabs } from "./lib/tabs/tabs.svelte";

  // `undefined` = closed, `null` = new connection, otherwise edit/duplicate this one.
  let dialog = $state<ConnectionConfig | null | undefined>(undefined);

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const newKey = t(isMac ? "tabs.newKey.mac" : "tabs.newKey.other");
  const activeTab = $derived(tabs.active);
  const activePane = $derived(tabs.activePane);

  // Tells the backend the first frame is painted (it only prints with TULKA_STARTUP_PROBE=1).
  onMount(() => {
    requestAnimationFrame(() => void invoke("startup_probe").catch(() => {}));
  });

  void connections.refresh();
  void connections.loadStartupError();

  function onkeydown(e: KeyboardEvent) {
    // Dialogs and the password prompt own the keyboard while open.
    if (dialog !== undefined || tabs.passwordRequest) return;
    const action = shortcutAction(e, isMac);
    if (!action) return;
    e.preventDefault();
    if (action === "new") tabs.newTab();
    else if (tabs.activeId) tabs.close(tabs.activeId);
  }

  function duplicate(conn: ConnectionConfig) {
    dialog = {
      ...conn,
      id: crypto.randomUUID(),
      name: t("connection.copySuffix", { name: conn.name }),
      passwordSaved: false,
    };
  }
</script>

<svelte:window {onkeydown} />

<div class="win">
  <div class="cols">
    <Rail />
    <aside class="explorer">
      <div class="sect"><span>{t("explorer.title")}</span></div>
      {#if connections.startupError}
        <p class="error" role="alert">{t("explorer.startupError", { error: connections.startupError })}</p>
      {/if}
      {#if connections.error}
        <p class="error" role="alert">{t("explorer.loadFailed", { error: connections.error })}</p>
      {/if}
      {#if tabs.connectError}<p class="error" role="alert">{tabs.connectError}</p>{/if}
      {#if connections.list.length === 0}
        <div class="empty">
          <p>{t("explorer.empty")}</p>
          <button class="primary" onclick={() => (dialog = null)}>{t("explorer.new")}</button>
        </div>
      {:else}
        <ConnectionList onconnect={(c) => tabs.connect(c)} onedit={(c) => (dialog = c)} onduplicate={duplicate} />
        <div class="foot">
          <button class="primary" onclick={() => (dialog = null)}>{t("explorer.new")}</button>
        </div>
      {/if}
    </aside>
    <main class="workspace">
      <TabStrip />
      <div class="body">
        {#if activeTab?.pane}
          {#key activeTab.id}
            <QueryPane pane={activeTab.pane} onreconnect={() => tabs.reconnectActive()} onretry={() => tabs.retry(activeTab.id)} />
          {/key}
        {:else if activeTab?.kind === "structure"}
          {#key activeTab.id}
            <StructureView
              connectionId={activeTab.connectionId!}
              database={activeTab.database!}
              schema={activeTab.schema!}
              name={activeTab.name!}
            />
          {/key}
        {:else}
          <div class="blank">
            <p>{t("tabs.empty", { key: newKey })}</p>
            <p class="sub">{t("tabs.emptyHint")}</p>
          </div>
        {/if}
      </div>
    </main>
  </div>
  <StatusBar
    status={activePane?.status ?? "idle"}
    session={activePane?.session ?? null}
    cursor={activePane?.cursor ?? { line: 1, col: 1 }}
    onreconnect={() => tabs.reconnectActive()}
  />
</div>

{#if dialog !== undefined}
  {#key dialog?.id}
    <ConnectionDialog initial={dialog ?? undefined} onclose={() => (dialog = undefined)} />
  {/key}
{/if}
{#if tabs.passwordRequest}
  <PasswordPrompt
    connection={tabs.passwordRequest.conn}
    oncancel={() => tabs.cancelPassword()}
    onsubmit={(pw) => void tabs.submitPassword(pw)}
  />
{/if}

<style>
  .win {
    height: 100vh;
    background: var(--ground);
    display: grid;
    grid-template-rows: 1fr 26px;
  }
  .cols {
    display: grid;
    grid-template-columns: 54px 272px 1fr;
    min-height: 0;
  }
  .explorer {
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .sect {
    padding: 12px 16px 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin-top: 40px;
    color: var(--muted);
  }
  .empty p {
    margin: 0;
  }
  .primary {
    height: 34px;
    padding: 0 16px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--text);
    font: inherit;
    font-weight: 500;
    cursor: pointer;
  }
  .primary:hover {
    border-color: var(--accent);
  }
  .foot {
    margin-top: auto;
    padding: 12px 16px;
    display: flex;
    justify-content: center;
  }
  .error {
    margin: 4px 16px;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .workspace {
    background: var(--ground);
    min-width: 0;
    min-height: 0;
    display: grid;
    grid-template-rows: 36px minmax(0, 1fr);
  }
  .body {
    min-height: 0;
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
