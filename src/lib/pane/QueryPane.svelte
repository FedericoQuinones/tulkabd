<script lang="ts">
  import { t } from "../i18n/t";
  import { connections } from "../connections/store.svelte";
  import { ENVIRONMENTS } from "../connections/env";
  import SqlEditor from "../editor/SqlEditor.svelte";
  import ResultsPanel from "../results/ResultsPanel.svelte";
  import PaneToolbar from "./PaneToolbar.svelte";
  import type { PaneState } from "./pane.svelte";

  interface Props {
    pane: PaneState;
    onreconnect: () => void;
    /** Retries loading the text of a definition tab; shown with the pane's notice. */
    onretry?: () => void;
    /** Why the tab's last connect failed. */
    connectError?: string | null;
    /** Focus the editor when it mounts. */
    autofocus?: boolean;
    /** Runs the sql; defaults to the pane's own run, which needs an open session. */
    onrun?: (sql: string, lineOffset: number) => void;
  }
  let { pane, onreconnect, onretry, connectError = null, autofocus = true, onrun }: Props = $props();

  let editor = $state<SqlEditor>();

  const connection = $derived(connections.list.find((c) => c.id === pane.connectionId) ?? null);
  const envColor = $derived(
    connection ? ENVIRONMENTS.find((e) => e.id === connection.environment)!.color : "var(--control)",
  );

  function run(target = editor?.target() ?? { sql: "", lineOffset: 0 }) {
    if (target.sql.trim() === "" || pane.status !== "idle" || pane.switching || !connection) return;
    if (onrun) onrun(target.sql, target.lineOffset);
    else void pane.run(target.sql, target.lineOffset);
  }
</script>

<section class="pane" style:--env={envColor}>
  <PaneToolbar
    {connection}
    database={pane.database}
    databases={pane.session?.databases ?? (pane.database ? [pane.database] : [])}
    status={pane.status}
    switching={pane.switching}
    onrun={() => run()}
    onstop={() => pane.stop()}
    {onreconnect}
    ondatabase={(db) => pane.changeDatabase(db)}
  />
  <div class="ed">
    {#if connectError}<p class="error" role="alert">{connectError}</p>{/if}
    {#if pane.notice}
      <div class="notice" role="alert">
        <span>{pane.notice}</span>
        {#if onretry}<button type="button" onclick={onretry}>{t("tabs.retry")}</button>{/if}
      </div>
    {/if}
    <SqlEditor bind:this={editor} {pane} {autofocus} onrun={run} oncursor={(p) => (pane.cursor = p)} />
  </div>
  <div class="res"><ResultsPanel {pane} onjump={(line) => editor?.jumpToLine(line)} /></div>
</section>

<style>
  .pane {
    display: grid;
    grid-template-rows: 48px minmax(120px, 1fr) minmax(160px, 1fr);
    position: relative;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }
  .pane::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--env);
    z-index: 2;
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 16px;
    color: var(--warning);
    border-bottom: 1px solid var(--border);
  }
  .notice button {
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    cursor: pointer;
  }
  .error {
    margin: 0;
    padding: 8px 16px;
    color: var(--negative);
    border-bottom: 1px solid var(--border);
  }
  .ed {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-bottom: 1px solid var(--border);
    padding-left: 2px;
  }
  .res {
    min-height: 0;
  }
</style>
