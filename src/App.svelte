<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { t } from "./lib/i18n/t";
  import Rail from "./lib/shell/Rail.svelte";
  import StatusBar from "./lib/shell/StatusBar.svelte";
  import type { ConnectionConfig, ParsedConnection } from "./lib/ipc";
  import ConnectionDialog from "./lib/connections/ConnectionDialog.svelte";
  import PasswordPrompt from "./lib/connections/PasswordPrompt.svelte";
  import { connections } from "./lib/connections/store.svelte";
  import ExplorerTree from "./lib/explorer/ExplorerTree.svelte";
  import { explorer } from "./lib/explorer/tree.svelte";
  import { clampWidth, loadLayout, saveLayout, MAX_WIDTH, MIN_WIDTH } from "./lib/shell/layout";
  import Palette from "./lib/palette/Palette.svelte";
  import { palette } from "./lib/palette/palette.svelte";
  import { shortcutAction } from "./lib/tabs/shortcuts";
  import { tabs } from "./lib/tabs/tabs.svelte";
  import Workspace from "./lib/workspace/Workspace.svelte";

  // `undefined` = closed, `null` = new connection, otherwise edit this one.
  let dialog = $state<ConnectionConfig | null | undefined>(undefined);
  // Folder preselected when the new-connection dialog opens from a folder menu.
  let dialogFolder = $state<string | null>(null);
  // A new connection from a connection string: already parsed (a paste on the tree),
  // or with the connection string field focused (New from connection string…).
  let dialogParsed = $state<ParsedConnection | undefined>(undefined);
  let dialogFocusConnStr = $state(false);

  function openDialog(conn: ConnectionConfig | null, folderId: string | null = null, parsed?: ParsedConnection, focusConnStr = false) {
    dialogFolder = folderId;
    dialogParsed = parsed;
    dialogFocusConnStr = focusConnStr;
    dialog = conn;
  }

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const activePane = $derived(tabs.activePane);

  // Tells the backend the first frame is painted (it only prints with TULKA_STARTUP_PROBE=1).
  onMount(() => {
    requestAnimationFrame(() => void invoke("startup_probe").catch(() => {}));
  });

  void explorer.init();
  void connections.loadStartupError();

  const layout = loadLayout();
  let explorerWidth = $state(layout.width);
  let explorerHidden = $state(layout.hidden);
  const remember = () => saveLayout({ width: explorerWidth, hidden: explorerHidden });

  function onrail(key: string) {
    if (key === "rail.search") palette.show();
    if (key !== "rail.explorer") return;
    explorerHidden = !explorerHidden;
    remember();
  }

  let resizing: { x: number; width: number } | null = null;
  function resizeStart(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    resizing = { x: e.clientX, width: explorerWidth };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }
  function resizeMove(e: PointerEvent) {
    if (resizing) explorerWidth = clampWidth(resizing.width + e.clientX - resizing.x);
  }
  function resizeEnd() {
    if (!resizing) return;
    resizing = null;
    remember();
  }
  function resizeKey(e: KeyboardEvent) {
    const step = e.key === "ArrowLeft" ? -16 : e.key === "ArrowRight" ? 16 : 0;
    if (step === 0) return;
    e.preventDefault();
    explorerWidth = clampWidth(explorerWidth + step);
    remember();
  }

  function onkeydown(e: KeyboardEvent) {
    // Dialogs and the password prompts own the keyboard while open.
    if (dialog !== undefined || tabs.passwordRequest || explorer.passwordRequest) return;
    if (document.querySelector('[aria-modal="true"]')) return;
    const action = shortcutAction(e, isMac);
    if (!action) return;
    e.preventDefault();
    if (action === "palette") palette.show();
    else if (action === "new") tabs.newTab();
    else if (action === "splitRight") tabs.splitActive("row");
    else if (action === "splitDown") tabs.splitActive("column");
    else if (tabs.activeId) tabs.close(tabs.activeId);
  }
</script>

<svelte:window {onkeydown} />

<div class="win">
  <div class="cols" class:no-explorer={explorerHidden}>
    <Rail active={explorerHidden ? null : "rail.explorer"} onselect={onrail} />
    {#if !explorerHidden}
      <aside class="explorer" style:width="{explorerWidth}px">
        {#if connections.startupError}
          <p class="error" role="alert">{t("explorer.startupError", { error: connections.startupError })}</p>
        {/if}
        {#if connections.error}
          <p class="error" role="alert">{t("explorer.loadFailed", { error: connections.error })}</p>
        {/if}
        <ExplorerTree
          onnewconnection={(folderId) => openDialog(null, folderId ?? null)}
          onedit={(c) => openDialog(c)}
          onnewfromstring={(folderId, parsed) => openDialog(null, folderId, parsed ?? undefined, parsed === null)}
        />
        <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
        <div
          class="resize"
          role="separator"
          aria-orientation="vertical"
          aria-label={t("explorer.resize")}
          aria-valuenow={explorerWidth}
          aria-valuemin={MIN_WIDTH}
          aria-valuemax={MAX_WIDTH}
          tabindex="0"
          onpointerdown={resizeStart}
          onpointermove={resizeMove}
          onpointerup={resizeEnd}
          onpointercancel={resizeEnd}
          onkeydown={resizeKey}
        ></div>
      </aside>
    {/if}
    <main class="workspace">
      <Workspace />
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
    <ConnectionDialog
      initial={dialog ?? undefined}
      folderId={dialogFolder}
      parsed={dialogParsed}
      focusConnStr={dialogFocusConnStr}
      onclose={() => {
        dialog = undefined;
        dialogParsed = undefined;
      }}
    />
  {/key}
{/if}
{#if palette.isOpen}
  <Palette />
{/if}
{#if tabs.passwordRequest}
  {#key tabs.passwordRequest.tabId}
    <PasswordPrompt
      connection={tabs.passwordRequest.conn}
      oncancel={() => tabs.cancelPassword()}
      onsubmit={(pw) => void tabs.submitPassword(pw)}
    />
  {/key}
{:else if explorer.passwordRequest}
  {#key explorer.passwordRequest.conn.id}
    <PasswordPrompt
      connection={explorer.passwordRequest.conn}
      oncancel={() => explorer.cancelPassword()}
      onsubmit={(pw) => void explorer.submitPassword(pw)}
    />
  {/key}
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
    grid-template-columns: 54px auto minmax(0, 1fr);
    min-height: 0;
  }
  /* Without the aside the workspace would land in the auto track. */
  .cols.no-explorer {
    grid-template-columns: 54px minmax(0, 1fr);
  }
  .explorer {
    position: relative;
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
    min-width: 0;
  }
  .resize {
    position: absolute;
    top: 0;
    bottom: 0;
    right: -3px;
    width: 6px;
    cursor: col-resize;
    z-index: 2;
    outline: none;
  }
  .resize:hover,
  .resize:focus-visible {
    background: linear-gradient(var(--accent), var(--accent)) center / 1px 100% no-repeat;
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
  }
</style>
