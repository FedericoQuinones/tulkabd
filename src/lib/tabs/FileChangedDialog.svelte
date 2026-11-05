<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import type { FileConflict } from "./tabs.svelte";

  interface Props {
    conflict: FileConflict;
    onanswer: (choice: "overwrite" | "reload") => void;
    oncancel: () => void;
  }
  let { conflict, onanswer, oncancel }: Props = $props();

  let panel: HTMLElement;
  let cancelButton: HTMLButtonElement;

  // Focus goes back where it was when the dialog closes.
  const before = typeof document !== "undefined" ? (document.activeElement as HTMLElement | null) : null;

  onMount(() => {
    // Cancel gets the focus: a stray Enter must neither overwrite nor reload.
    cancelButton.focus();
    return () => before?.focus?.();
  });

  // The focus must not be lost to the page behind the dialog.
  $effect(() => {
    if (!panel.contains(document.activeElement)) cancelButton.focus();
  });

  /** Keeps Tab and Shift+Tab inside the dialog. */
  function trap(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      oncancel();
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
  <div class="panel" role="dialog" aria-modal="true" aria-labelledby="changed-title" tabindex="-1" bind:this={panel}>
    <div class="head" id="changed-title">{t("files.changedTitle")}</div>
    <p class="lead">{t("files.changedBody", { name: conflict.title })}</p>
    <div class="foot">
      <button type="button" class="ghost" bind:this={cancelButton} onclick={oncancel}>{t("files.changedCancel")}</button>
      <button type="button" class="ghost" onclick={() => onanswer("reload")}>{t("files.changedReload")}</button>
      <button type="button" class="primary" onclick={() => onanswer("overwrite")}>{t("files.changedOverwrite")}</button>
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
  .lead {
    margin: 0 0 10px;
    color: var(--soft);
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
