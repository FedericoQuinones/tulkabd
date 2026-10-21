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
  import { pane } from "./lib/pane/pane.svelte";

  // `undefined` = closed, `null` = new connection, otherwise edit/duplicate this one.
  let dialog = $state<ConnectionConfig | null | undefined>(undefined);
  let prompting = $state<ConnectionConfig | null>(null);
  let connectError = $state<string | null>(null);
  // Remembers the database to restore when a password prompt was opened by Reconnect.
  let promptDatabase: string | undefined;

  // Tells the backend the first frame is painted (it only prints with TULKA_STARTUP_PROBE=1).
  onMount(() => {
    requestAnimationFrame(() => void invoke("startup_probe").catch(() => {}));
  });

  void connections.refresh();
  void connections.loadStartupError();

  async function connect(conn: ConnectionConfig, password?: string, database?: string) {
    if (pane.status === "connecting" || pane.status === "running") return;
    connectError = null;
    if (!conn.passwordSaved && password === undefined) {
      promptDatabase = database;
      prompting = conn;
      return;
    }
    const result = await pane.open(conn, password, database);
    if (result.kind === "passwordRequired") {
      promptDatabase = database;
      prompting = conn;
    } else if (result.kind === "error") {
      connectError = t("explorer.connectFailed", { error: result.error });
    }
  }

  function reconnect() {
    const conn = connections.list.find((c) => c.id === pane.connectionId);
    if (conn) void connect(conn, undefined, pane.database ?? undefined);
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
      {#if connections.startupError}
        <p class="error" role="alert">{t("explorer.startupError", { error: connections.startupError })}</p>
      {/if}
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
    <main class="workspace"><QueryPane onreconnect={reconnect} /></main>
  </div>
  <StatusBar status={pane.status} session={pane.session} cursor={pane.cursor} onreconnect={reconnect} />
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
      void connect(c, pw, promptDatabase);
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
    min-width: 0;
    min-height: 0;
  }
</style>
