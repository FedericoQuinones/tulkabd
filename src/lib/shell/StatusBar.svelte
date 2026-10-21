<script lang="ts">
  import { t } from "../i18n/t";
  import type { PaneStatus } from "../pane/pane.svelte";
  import { shortVersion } from "../pane/reduce";
  import type { SessionInfo } from "../ipc";

  interface Props {
    status: PaneStatus;
    session: SessionInfo | null;
    cursor: { line: number; col: number };
    onreconnect: () => void;
  }
  let { status, session, cursor, onreconnect }: Props = $props();
</script>

<footer class="status">
  {#if status === "disconnected"}
    <span class="lost">{t("status.disconnected")} · <button onclick={onreconnect}>{t("pane.reconnect")}</button></span>
  {:else if session}
    <span>{t("status.connected")}</span>
  {:else}
    <span>{t("status.noConnection")}</span>
  {/if}
  {#if session}
    <span class="mono">{t("status.server", { version: shortVersion(session.server.version), user: session.server.user })}</span>
  {/if}
  <span class="r">
    {#if session}<span class="mono">{t("status.position", { line: cursor.line, col: cursor.col })}</span>{/if}
    <span>{status === "running" ? t("status.running") : t("status.ready")}</span>
  </span>
</footer>

<style>
  .status {
    display: flex;
    align-items: center;
    gap: 22px;
    padding: 0 14px;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 12px;
  }
  .r {
    margin-left: auto;
    display: flex;
    gap: 22px;
  }
  .mono {
    font-family: var(--mono);
  }
  .lost {
    color: var(--negative);
  }
  button {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    text-decoration: underline;
    cursor: pointer;
  }
</style>
