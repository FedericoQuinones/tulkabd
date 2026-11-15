<script lang="ts">
  import { t } from "../i18n/t";
  import type { ConnectionConfig } from "../ipc";
  import { ENVIRONMENTS } from "../connections/env";
  import { connections } from "../connections/store.svelte";
  import GuardMenu from "../guard/GuardMenu.svelte";
  import { explorer } from "../explorer/tree.svelte";
  import type { PaneStatus } from "./pane.svelte";

  interface Props {
    connection: ConnectionConfig | null;
    database: string | null;
    databases: string[];
    status: PaneStatus;
    onrun: () => void;
    onstop: () => void;
    onreconnect: () => void;
    /** True while a database switch is in flight. */
    switching?: boolean;
    ondatabase: (name: string) => Promise<void>;
    /** Binds the tab to the connection picked in the picker of an unbound tab. */
    onconnect?: (conn: ConnectionConfig) => void;
  }
  let { connection, database, databases, status, switching = false, onrun, onstop, onreconnect, ondatabase, onconnect }: Props = $props();

  let picker = $state<HTMLSelectElement>();
  /** Moves the keyboard to the connection picker of an unbound tab. */
  export function focusPicker() {
    picker?.focus();
  }

  // Connected connections first, then the rest, each by name.
  const choices = $derived(
    [...connections.list].sort(
      (a, b) =>
        Number(explorer.conns[b.id]?.status === "connected") - Number(explorer.conns[a.id]?.status === "connected") ||
        a.name.localeCompare(b.name),
    ),
  );

  const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
  const runKey = t(isMac ? "pane.runKey.mac" : "pane.runKey.other");

  const envShort = $derived(connection ? t(`env.short.${connection.environment}`) : "");
  const envColor = $derived(connection ? ENVIRONMENTS.find((e) => e.id === connection.environment)!.color : "var(--muted)");
  const guardLabel = $derived(
    !connection ? "" : connections.guardOff.includes(connection.id) ? t("guard.offSession") : t(`guard.${connection.guard}`),
  );
  // An unbound tab can still press Run: that sends the user to the connection picker.
  const canRun = $derived(status === "idle" && !switching);
</script>

<div class="bar">
  {#if connection}
    <div class="pick">
      <span class="env" style:color={envColor}>{envShort}</span>
      <span>{connection.name}</span>
      <span class="sep">›</span>
      <select
        aria-label={t("pane.database")}
        value={database ?? ""}
        disabled={status !== "idle" || switching}
        onchange={async (e) => {
          const select = e.currentTarget;
          await ondatabase(select.value);
          select.value = database ?? "";
        }}
      >
        {#each databases as db (db)}<option value={db}>{db}</option>{/each}
      </select>
    </div>
    <GuardMenu {connection} label={guardLabel} />
  {:else}
    <div class="pick">
      <select
        bind:this={picker}
        aria-label={t("pane.connection")}
        value=""
        onchange={(e) => {
          const conn = connections.list.find((c) => c.id === e.currentTarget.value);
          e.currentTarget.value = "";
          if (conn) onconnect?.(conn);
        }}
      >
        <option value="" disabled>{t("pane.pickConnection")}</option>
        {#each choices as c (c.id)}<option value={c.id}>{c.name} · {t(`env.short.${c.environment}`)}</option>{/each}
      </select>
    </div>
  {/if}
  {#if status === "disconnected"}
    <button class="reconnect" onclick={onreconnect}>{t("pane.disconnected")} · {t("pane.reconnect")}</button>
  {:else if status === "connecting"}
    <span class="hint">{t("pane.connecting")}</span>
  {/if}
  <div class="grow"></div>
  <button class="ghost" disabled={status !== "running"} onclick={onstop}>
    <svg class="i" viewBox="0 0 16 16"><rect x="4" y="4" width="8" height="8" rx="1" /></svg>{t("pane.stop")}
  </button>
  <button class="run" disabled={!canRun} onclick={onrun}>
    <svg class="i" viewBox="0 0 16 16"><path d="M5 3l8 5-8 5z" fill="currentColor" /></svg>{t("pane.run")}
    <span class="k">{runKey}</span>
  </button>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 14px 0 16px;
    border-bottom: 1px solid var(--border);
  }
  .pick,
  .ghost,
  .reconnect {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    white-space: nowrap;
  }
  .env {
    font-family: var(--mono);
    font-size: 11.5px;
    font-weight: 500;
  }
  .sep {
    color: var(--muted);
  }
  select {
    background: none;
    border: none;
    color: var(--text);
    font: inherit;
    cursor: pointer;
    max-width: 180px;
  }
  select option {
    background: var(--raised);
  }
  select:disabled {
    cursor: default;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
  }
  .grow {
    flex: 1;
  }
  button {
    font: inherit;
    cursor: pointer;
  }
  .ghost {
    background: none;
    color: var(--soft);
  }
  .reconnect {
    background: none;
    color: var(--negative);
    border-color: var(--negative);
  }
  .run {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 14px;
    border: none;
    border-radius: var(--radius-control);
    background: #fff;
    color: #000;
    font-weight: 600;
    white-space: nowrap;
  }
  .run .k {
    font-family: var(--mono);
    font-size: 11.5px;
    color: #55565b;
    font-weight: 500;
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  button:focus-visible,
  select:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
