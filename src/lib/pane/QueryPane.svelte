<script lang="ts">
  import { t } from "../i18n/t";
  import { connections } from "../connections/store.svelte";
  import { ENVIRONMENTS } from "../connections/env";
  import SqlEditor from "../editor/SqlEditor.svelte";
  import PaneToolbar from "./PaneToolbar.svelte";
  import { pane } from "./pane.svelte";

  interface Props {
    onreconnect: () => void;
  }
  let { onreconnect }: Props = $props();

  let editor: SqlEditor;

  const connection = $derived(connections.list.find((c) => c.id === pane.connectionId) ?? null);
  const envColor = $derived(
    connection ? ENVIRONMENTS.find((e) => e.id === connection.environment)!.color : "var(--control)",
  );

  function run(target = editor.target()) {
    if (target.sql.trim() === "" || pane.status !== "idle" || pane.switching || !connection) return;
    void pane.run(target.sql, target.lineOffset);
  }
</script>

<section class="pane" style:--env={envColor}>
  <div class="tabs">
    <div class="tab"><span class="dot" style:background={envColor}></span><span class="lab">{t("pane.tab")}</span></div>
  </div>
  <PaneToolbar
    {connection}
    database={pane.database}
    databases={pane.session?.databases ?? []}
    status={pane.status}
    switching={pane.switching}
    onrun={() => run()}
    onstop={() => pane.stop()}
    {onreconnect}
    ondatabase={(db) => pane.changeDatabase(db)}
  />
  <div class="ed"><SqlEditor bind:this={editor} onrun={run} oncursor={(p) => (pane.cursor = p)} /></div>
  <div class="msgs" aria-label={t("pane.messages")} role="log">
    {#each pane.messages as m, i (i)}
      <p class={m.kind}>{m.text}</p>
    {/each}
  </div>
</section>

<style>
  .pane {
    display: grid;
    grid-template-rows: 36px 48px minmax(120px, 1fr) minmax(0, auto);
    position: relative;
    min-width: 0;
    min-height: 0;
    height: 100%;
  }
  .pane::before {
    content: "";
    position: absolute;
    left: 0;
    top: 36px;
    bottom: 0;
    width: 2px;
    background: var(--env);
    z-index: 2;
  }
  .tabs {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--border);
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 14px;
  }
  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
  }
  .lab {
    position: relative;
  }
  .lab::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: -11px;
    height: 1px;
    background: var(--env);
  }
  .ed {
    min-height: 0;
    border-bottom: 1px solid var(--border);
    padding-left: 2px;
  }
  .msgs {
    max-height: 160px;
    overflow: auto;
    padding: 0 16px;
  }
  .msgs p {
    margin: 6px 0;
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--soft);
  }
  .msgs p.error {
    color: var(--negative);
  }
  .msgs p.warning {
    color: var(--warning);
  }
</style>
