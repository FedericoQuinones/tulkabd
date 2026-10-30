<script lang="ts">
  import { t } from "../i18n/t";
  import type { ConnectionConfig, GuardFinding } from "../ipc";
  import { connections } from "../connections/store.svelte";
  import { consequence } from "./text";

  interface Props {
    connection: ConnectionConfig;
    /** The flagged statements, with 0-based editor lines. */
    findings: GuardFinding[];
  }
  let { connection, findings }: Props = $props();

  let error = $state<string | null>(null);

  async function turnOff() {
    error = null;
    try {
      await connections.setGuardOffForSession(connection.id, true);
    } catch (e) {
      error = t("guard.changeFailed", { error: String(e) });
    }
  }
</script>

<div class="bar" role="alert">
  <span class="msg">{t("guard.blocked", { count: findings.length })}</span>
  <ul>
    {#each findings as f, i (i)}
      <li>{t("guard.line", { line: f.line + 1 })} · {consequence(f)}</li>
    {/each}
  </ul>
  {#if error}<span class="err">{error}</span>{/if}
  {#if connections.guardOff.includes(connection.id)}
    <span class="off" role="status">{t("guard.offSession")}</span>
  {:else}
    <button type="button" onclick={turnOff}>{t("guard.offUntilClose")}</button>
  {/if}
</div>

<style>
  .bar {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    padding: 8px 16px;
    border-bottom: 1px solid var(--border);
    color: var(--negative);
  }
  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    color: var(--soft);
    font-size: 12px;
  }
  .off {
    color: var(--warning);
    font-size: 12px;
  }
  .err {
    font-size: 12px;
  }
  button {
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    cursor: pointer;
  }
  button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
