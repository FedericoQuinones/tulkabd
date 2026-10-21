<script lang="ts">
  import { t } from "../i18n/t";
  import type { ConnectionConfig } from "../ipc";
  import { ENVIRONMENTS } from "../connections/env";
  import type { PaneStatus } from "./pane.svelte";

  interface Props {
    connection: ConnectionConfig | null;
    database: string | null;
    databases: string[];
    status: PaneStatus;
    onrun: () => void;
    onstop: () => void;
    onreconnect: () => void;
    ondatabase: (name: string) => void;
  }
  let { connection, database, databases, status, onrun, onstop, onreconnect, ondatabase }: Props = $props();

  const envShort = $derived(connection ? t(`env.short.${connection.environment}`) : "");
  const envColor = $derived(connection ? ENVIRONMENTS.find((e) => e.id === connection.environment)!.color : "var(--muted)");
  const canRun = $derived(status === "idle" && connection !== null);
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
        disabled={status !== "idle"}
        onchange={(e) => ondatabase(e.currentTarget.value)}
      >
        {#each databases as db (db)}<option value={db}>{db}</option>{/each}
      </select>
    </div>
    <div class="guard" class:confirm={connection.guard === "confirm"} class:block={connection.guard === "block"}>
      <svg class="i" viewBox="0 0 16 16"><path d="M8 2l5 2v4c0 3-2.2 5-5 6-2.8-1-5-3-5-6V4z" /></svg>
      {t(`guard.${connection.guard}`)}
    </div>
  {:else}
    <span class="hint">{t("pane.noConnection")}</span>
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
    <span class="k">Ctrl ↵</span>
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
  .guard,
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
  .guard {
    gap: 7px;
    color: var(--soft);
    cursor: default;
  }
  .guard.confirm {
    color: var(--warning);
    border-color: #4a3d22;
  }
  .guard.block {
    color: var(--negative);
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
