<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import type { ClosePrompt } from "./tabs.svelte";

  interface Props {
    prompt: ClosePrompt;
    /** Commit (true) or roll back (false) what is open, then close. */
    onanswer: (commit: boolean) => void;
    oncancel: () => void;
  }
  let { prompt, onanswer, oncancel }: Props = $props();

  const all = $derived(prompt.scope === "window");
  // A running query can only be rolled back: committing would keep half of its work.
  const canCommit = $derived(!prompt.items.some((x) => x.state === "unknown"));
  let panel: HTMLElement;
  let cancelButton: HTMLButtonElement;

  // Focus goes back where it was when the dialog closes.
  const before = typeof document !== "undefined" ? (document.activeElement as HTMLElement | null) : null;

  onMount(() => {
    // Cancel gets the focus: a stray Enter must neither commit nor roll back.
    cancelButton.focus();
    return () => before?.focus?.();
  });

  // A failed answer re-enables the buttons; the focus must not be lost with the disabled one.
  $effect(() => {
    if (!prompt.busy && !panel.contains(document.activeElement)) cancelButton.focus();
  });

  /** Keeps Tab and Shift+Tab inside the dialog. */
  function trap(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (!prompt.busy) oncancel();
      return;
    }
    if (e.key !== "Tab") return;
    const items = [...panel.querySelectorAll<HTMLElement>("button")].filter((el) => !(el as HTMLButtonElement).disabled);
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    const inside = active !== null && items.includes(active);
    if (e.shiftKey && (active === first || !inside)) {
      e.preventDefault();
      (inside ? last : cancelButton).focus();
    } else if (!e.shiftKey && (active === last || !inside)) {
      e.preventDefault();
      (inside ? first : cancelButton).focus();
    }
  }
</script>

<svelte:window onkeydown={trap} />
<div class="backdrop">
  <div class="panel" role="dialog" aria-modal="true" aria-labelledby="tx-title" tabindex="-1" bind:this={panel}>
    <div class="head" id="tx-title">{t("tx.title")}</div>
    <ul>
      {#each prompt.items as item (item.tabId)}
        <li>
          {item.state === "unknown"
            ? t("tx.running", { connection: item.connection, database: item.database })
            : t("tx.one", { connection: item.connection, database: item.database, count: item.count })}
        </li>
      {/each}
    </ul>
    {#if prompt.error}
      <p class="error" role="alert">{prompt.error}</p>
    {/if}
    <div class="foot">
      <button type="button" class="ghost" bind:this={cancelButton} disabled={prompt.busy} onclick={oncancel}>{t("tx.cancel")}</button>
      <button type="button" class="ghost" disabled={prompt.busy} onclick={() => onanswer(false)}>
        {t(all ? "tx.rollbackAll" : "tx.rollback")}
      </button>
      {#if canCommit}
        <button type="button" class="primary" disabled={prompt.busy} onclick={() => onanswer(true)}>
          {t(all ? "tx.commitAll" : "tx.commit")}
        </button>
      {/if}
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
    max-height: calc(100vh - 64px);
    overflow: auto;
    padding: 16px 18px;
    background: var(--surface);
    border: 2px solid var(--border);
    border-radius: var(--radius-card);
  }
  .head {
    font-weight: 600;
    margin-bottom: 10px;
  }
  ul {
    list-style: none;
    margin: 0 0 12px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
    color: var(--soft);
  }
  .error {
    margin: 0 0 12px;
    color: var(--negative);
    font-size: 12.5px;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
  .ghost,
  .primary {
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
  }
  .ghost {
    background: none;
    color: var(--soft);
    border: 1px solid var(--control);
  }
  .primary {
    background: none;
    color: var(--accent);
    border: 1px solid var(--accent);
    font-weight: 500;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
