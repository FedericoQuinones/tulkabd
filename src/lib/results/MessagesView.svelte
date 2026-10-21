<script lang="ts">
  import { t } from "../i18n/t";
  import type { PaneMessage } from "../pane/reduce";

  interface Props {
    messages: PaneMessage[];
    /** Receives the 1-based editor line (already offset by the pane). */
    onjump: (line: number) => void;
  }
  let { messages, onjump }: Props = $props();
</script>

<div class="msgs" role="log" aria-label={t("pane.messages")}>
  {#each messages as m, i (i)}
    <div class="m {m.kind}">
      <span class="text">{m.text}</span>
      {#if m.line !== null}
        {@const line = m.line}
        <button type="button" class="line" aria-label={t("messages.goToLine", { line })} onclick={() => onjump(line)}>
          {t("messages.line", { line })}
        </button>
      {/if}
    </div>
  {:else}
    <p class="empty">{t("messages.empty")}</p>
  {/each}
</div>

<style>
  .msgs {
    height: 100%;
    overflow: auto;
    padding: 8px 16px;
  }
  .m {
    display: flex;
    gap: 12px;
    padding: 4px 0;
    font-family: var(--mono);
    font-size: 12px;
    color: var(--soft);
  }
  .text {
    white-space: pre-wrap;
    word-break: break-word;
    min-width: 0;
  }
  .m.error {
    color: var(--negative);
  }
  .m.warning {
    color: var(--warning);
  }
  .line {
    flex: none;
    margin-left: auto;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
  }
  .empty {
    margin: 0;
    color: var(--muted);
    font-size: 12px;
  }
</style>
