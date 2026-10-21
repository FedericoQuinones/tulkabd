<script lang="ts">
  import { t } from "./lib/i18n/t";
  import Rail from "./lib/shell/Rail.svelte";
  import StatusBar from "./lib/shell/StatusBar.svelte";
  import * as ipc from "./lib/ipc";
  import type { ConnectionConfig, SessionInfo } from "./lib/ipc";
  import ConnectionDialog from "./lib/connections/ConnectionDialog.svelte";
  import ConnectionList from "./lib/connections/ConnectionList.svelte";
  import PasswordPrompt from "./lib/connections/PasswordPrompt.svelte";
  import { connections } from "./lib/connections/store.svelte";

  // `undefined` = closed, `null` = new connection, otherwise edit/duplicate this one.
  let dialog = $state<ConnectionConfig | null | undefined>(undefined);
  let prompting = $state<ConnectionConfig | null>(null);
  let connectError = $state<string | null>(null);
  // Sessions are wired to tabs in a later task; the last opened one is kept here for now.
  let session = $state<SessionInfo | null>(null);
  let sessionTab: string | null = null;
  let connecting = false;

  void connections.refresh();

  async function connect(conn: ConnectionConfig, password?: string) {
    if (connecting) return;
    connectError = null;
    if (!conn.passwordSaved && password === undefined) {
      prompting = conn;
      return;
    }
    connecting = true;
    try {
      const previous = sessionTab;
      if (previous) {
        sessionTab = null;
        session = null;
        await ipc.closeSession(previous).catch(() => {});
      }
      const tabId = crypto.randomUUID();
      session = await ipc.openSession(tabId, conn.id, undefined, password);
      sessionTab = tabId;
    } catch (e) {
      if (String(e) === "password_required") prompting = conn;
      else connectError = t("explorer.connectFailed", { error: String(e) });
    } finally {
      connecting = false;
    }
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

<div class="win">
  <div class="cols">
    <Rail />
    <aside class="explorer">
      <div class="sect"><span>{t("explorer.title")}</span></div>
      {#if connections.error}
        <p class="error" role="alert">{t("explorer.loadFailed", { error: connections.error })}</p>
      {/if}
      {#if connectError}<p class="error" role="alert">{connectError}</p>{/if}
      {#if connections.list.length === 0}
        <div class="empty">
          <p>{t("explorer.empty")}</p>
          <button class="primary" onclick={() => (dialog = null)}>{t("explorer.new")}</button>
        </div>
      {:else}
        <ConnectionList onconnect={(c) => connect(c)} onedit={(c) => (dialog = c)} onduplicate={duplicate} />
        <div class="foot">
          <button class="primary" onclick={() => (dialog = null)}>{t("explorer.new")}</button>
        </div>
      {/if}
    </aside>
    <main class="workspace"></main>
  </div>
  <StatusBar />
</div>

{#if dialog !== undefined}
  {#key dialog?.id}
    <ConnectionDialog initial={dialog ?? undefined} onclose={() => (dialog = undefined)} />
  {/key}
{/if}
{#if prompting}
  <PasswordPrompt
    connection={prompting}
    oncancel={() => (prompting = null)}
    onsubmit={(pw) => {
      const c = prompting!;
      prompting = null;
      void connect(c, pw);
    }}
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
  }
</style>
