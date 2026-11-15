<script lang="ts">
  import { onMount } from "svelte";
  import { modal } from "../dialogFocus";
  import { t } from "../i18n/t";

  interface Props {
    /** What failed, as the dialog's title. */
    title: string;
    /** The full error text. */
    message: string;
    /** Runs the notice's action again; the dialog closes after it is started. */
    onretry: () => void;
    onclose: () => void;
  }
  let { title, message, onretry, onclose }: Props = $props();

  let panel: HTMLDivElement;
  let copied = $state(false);
  let failure = $state<string | null>(null);

  onMount(() => panel.querySelector<HTMLElement>("[data-autofocus]")?.focus());

  /** The web clipboard: a write started by a click needs no Tauri permission. */
  async function copy() {
    failure = null;
    copied = false;
    try {
      await navigator.clipboard.writeText(message);
      copied = true;
    } catch (e) {
      failure = t("explorer.copyFailed", { error: String(e) });
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onkeydown={(e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }}
>
  <div class="panel" bind:this={panel} use:modal role="dialog" aria-modal="true" aria-labelledby="ed-title">
    <div class="title" id="ed-title">{title}</div>
    <pre class="detail">{message}</pre>
    {#if failure}<p class="err" role="alert">{failure}</p>{/if}
    {#if copied}<p class="ok-note" role="status">{t("explorer.error.copied")}</p>{/if}
    <div class="foot">
      <button type="button" class="ghost" onclick={copy}>{t("explorer.error.copy")}</button>
      <button type="button" class="ghost" onclick={onretry}>{t("explorer.retry")}</button>
      <button type="button" class="ok" data-autofocus onclick={onclose}>{t("explorer.error.close")}</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .panel {
    width: 460px;
    max-width: calc(100vw - 32px);
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  .title {
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .detail {
    margin: 0;
    max-height: 50vh;
    overflow: auto;
    padding: 8px 10px;
    background: var(--ground);
    border: 1px solid var(--border);
    border-radius: var(--radius-control);
    color: var(--negative);
    font: inherit;
    font-size: 12px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
    cursor: text;
  }
  .err,
  .ok-note {
    margin: 0;
    font-size: 12px;
  }
  .err {
    color: var(--negative);
  }
  .ok-note {
    color: var(--muted);
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .ghost,
  .ok {
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
    background: none;
  }
  .ghost {
    color: var(--soft);
    border: 1px solid var(--control);
  }
  .ok {
    background: var(--accent);
    color: var(--text);
    border: 1px solid var(--accent);
    font-weight: 500;
  }
  button:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
