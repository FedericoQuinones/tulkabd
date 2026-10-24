<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { t } from "./lib/i18n/t";
  import Rail from "./lib/shell/Rail.svelte";
  import StatusBar from "./lib/shell/StatusBar.svelte";
  import { loadSession, onAppEvent, saveSession, type ConnectionConfig, type ParsedConnection } from "./lib/ipc";
  import ConnectionDialog from "./lib/connections/ConnectionDialog.svelte";
  import PasswordPrompt from "./lib/connections/PasswordPrompt.svelte";
  import TransactionDialog from "./lib/tabs/TransactionDialog.svelte";
  import { connections } from "./lib/connections/store.svelte";
  import ExplorerTree from "./lib/explorer/ExplorerTree.svelte";
  import { explorer } from "./lib/explorer/tree.svelte";
  import { clampWidth, loadLayout, saveLayout, MAX_WIDTH, MIN_WIDTH } from "./lib/shell/layout";
  import Palette from "./lib/palette/Palette.svelte";
  import { palette } from "./lib/palette/palette.svelte";
  import NameDialog from "./lib/queries/NameDialog.svelte";
  import SavedQueriesPanel from "./lib/queries/SavedQueriesPanel.svelte";
  import { queries } from "./lib/queries/queries.svelte";
  import { shortcutAction } from "./lib/tabs/shortcuts";
  import { zoom } from "./lib/zoom.svelte";
  import { prefs } from "./lib/settings/prefs.svelte";
  import { tabs } from "./lib/tabs/tabs.svelte";
  import { runLog } from "./lib/runlog/runlog.svelte";
  import { showRunLog } from "./lib/runlog/reveal";
  import Settings from "./lib/settings/Settings.svelte";
  import Workspace from "./lib/workspace/Workspace.svelte";
  import { parseSession, restore } from "./lib/workspace/session";

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

  void explorer.init();
  // Every finished run, of any tab, joins the Run log.
  void onAppEvent((e) => {
    if (e.kind === "runLogged") runLog.prepend(e.entry);
  }).catch(() => {});
  void queries.reload();
  void connections.loadStartupError();
  zoom.init();

  // The saved session is restored before the workspace first renders (a mounting editor takes the
  // focus only in the active group). Nothing connects. Autosave starts after, so an empty
  // workspace never overwrites the saved one.
  let restored = $state(false);
  let destroyed = false;
  let stopAutosave: (() => void) | null = null;
  let stopDrop: (() => void) | null = null;
  onDestroy(() => {
    destroyed = true;
    stopAutosave?.();
    stopDrop?.();
  });

  // `.sql` files dropped on the window open in tabs. The native drop (dragDropEnabled) only sees
  // files from outside: tabs and tree items move with pointer events, not HTML drag and drop.
  try {
    getCurrentWebview()
      .onDragDropEvent((e) => {
        if (e.payload.type === "drop") void tabs.dropFiles(e.payload.paths);
      })
      .then((fn) => {
        if (destroyed) fn();
        else stopDrop = fn;
      })
      .catch(() => {});
  } catch {
    // Outside Tauri (tests): nothing is ever dropped.
  }
  async function restoreSession() {
    // `undefined` when the saved session could not be loaded at all.
    const [json, loaded] = await Promise.all([
      loadSession().catch(() => undefined),
      prefs.load(),
      connections.refresh(),
      connections.loadGuardOff(),
    ]);
    if (destroyed) return;
    // With "open with the last session" off the workspace starts empty, and the saved session stays
    // on disk until the user edits something, like one this version cannot restore.
    const session = loaded.restoreSession ? parseSession(json ?? null) : null;
    if (session) {
      // Unreadable connections file: keep the bindings rather than drop them all.
      const ids = connections.error
        ? session.tabs.flatMap((x) => (x.connectionId ? [x.connectionId] : []))
        : connections.list.map((c) => c.id);
      restore(tabs, session, { connectionIds: new Set(ids) });
    }
    restored = true;
    // A saved session this version cannot restore (or could not load) stays on disk until the
    // user edits something.
    stopAutosave = tabs.startAutosave(saveSession, { holdUntilEdit: !session && json !== null });
    // Tells the backend the restored workspace is painted (it only prints with TULKA_STARTUP_PROBE=1).
    await tick();
    requestAnimationFrame(() => {
      if (!destroyed) void invoke("startup_probe").catch(() => {});
    });
  }
  void restoreSession();

  const layout = loadLayout();
  let explorerWidth = $state(layout.width);
  let explorerHidden = $state(layout.hidden);
  // What the left panel shows: the Explorer tree or the saved queries (Rail icons).
  let leftPanel = $state<"rail.explorer" | "rail.saved">("rail.explorer");
  const remember = () => saveLayout({ width: explorerWidth, hidden: explorerHidden });

  // The Settings screen takes the place of the workspace; the tabs stay mounted underneath.
  let settingsOpen = $state(false);

  /** Leaves Settings. The focus goes back to the gear unless something else is taking it. */
  function closeSettings(focusGear = true) {
    if (!settingsOpen) return;
    settingsOpen = false;
    if (focusGear) void tick().then(() => document.querySelector<HTMLElement>(`nav button[aria-label="${t("rail.settings")}"]`)?.focus());
  }

  // Opening or showing a tab (Explorer, palette, saved queries, files) leaves Settings.
  $effect(() => {
    void tabs.shown;
    untrack(() => closeSettings(false));
  });

  /** A panel icon shows its panel; pressed again while shown, it hides the left panel. */
  function onrail(key: string) {
    if (key === "rail.settings") {
      if (settingsOpen) closeSettings();
      else settingsOpen = true;
      return;
    }
    // Any other icon acts on the workspace, which Settings covers.
    closeSettings(false);
    if (key === "rail.search") palette.show();
    if (key === "rail.log") void showRunLog();
    if (key !== "rail.explorer" && key !== "rail.saved") return;
    if (!explorerHidden && leftPanel !== key) leftPanel = key;
    else {
      leftPanel = key;
      explorerHidden = !explorerHidden;
    }
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
    // A shortcut that acts on the tabs leaves Settings first; the zoom applies to both.
    if (action !== "zoomIn" && action !== "zoomOut" && action !== "zoomReset") closeSettings(false);
    e.preventDefault();
    if (action === "zoomIn" || action === "zoomOut" || action === "zoomReset") zoom.apply(action);
    else if (action === "palette") palette.show();
    else if (action === "save") void tabs.saveActive();
    else if (action === "open") void tabs.pickFiles();
    else if (action === "new") tabs.newTab();
    else if (action === "splitRight") tabs.splitActive("row");
    else if (action === "splitDown") tabs.splitActive("column");
    else if (tabs.activeId) void tabs.requestClose(tabs.activeId);
  }
</script>

<svelte:window {onkeydown} />

<div class="win">
  <div class="cols" class:no-explorer={explorerHidden}>
    <Rail active={explorerHidden ? null : leftPanel} onselect={onrail} {settingsOpen} />
    {#if !explorerHidden}
      <aside class="explorer" style:width="{explorerWidth}px">
        {#if leftPanel === "rail.saved"}
          <SavedQueriesPanel />
        {:else}
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
        {/if}
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
      <div class="ws" hidden={settingsOpen}>
        {#if restored}<Workspace />{/if}
      </div>
      {#if settingsOpen}<Settings onclose={() => closeSettings()} />{/if}
      {#if tabs.fileError}
        <div class="file-error" role="alert">
          <span>{tabs.fileError}</span>
          <button type="button" aria-label={t("files.dismiss")} title={t("files.dismiss")} onclick={() => (tabs.fileError = null)}>
            <svg viewBox="0 0 16 16" width="10" height="10"><path d="M3 3l10 10M13 3L3 13" /></svg>
          </button>
        </div>
      {/if}
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
{#if tabs.closePrompt}
  <TransactionDialog prompt={tabs.closePrompt} onanswer={(commit) => void tabs.answerClose(commit)} oncancel={() => tabs.cancelClose()} />
{/if}
{#if tabs.nameRequest}
  {#key tabs.nameRequest}
    <NameDialog {...tabs.nameRequest} />
  {/key}
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
    position: relative;
    background: var(--ground);
    min-width: 0;
    min-height: 0;
  }
  .ws {
    height: 100%;
  }
  .ws[hidden] {
    display: none;
  }
  /* Files that could not be opened, over the top right of the workspace. */
  .file-error {
    position: absolute;
    top: 44px;
    right: 14px;
    z-index: 4;
    display: flex;
    align-items: flex-start;
    gap: 10px;
    max-width: 420px;
    padding: 10px 10px 10px 14px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    color: var(--negative);
    white-space: pre-line;
    word-break: break-word;
  }
  .file-error button {
    display: grid;
    place-items: center;
    flex: none;
    width: 20px;
    height: 20px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--muted);
    cursor: pointer;
  }
  .file-error button svg {
    fill: none;
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .file-error button:hover,
  .file-error button:focus-visible {
    outline: none;
    color: var(--text);
    background: var(--ground);
  }
</style>
