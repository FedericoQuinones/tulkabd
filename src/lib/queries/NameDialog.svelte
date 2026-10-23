<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { t } from "../i18n/t";
  import { queryError } from "./queries.svelte";

  interface Props {
    title: string;
    initial: string;
    /**
     * Saves under `name`; resolves to null when done (the host closes the dialog), else an error
     * code. `"exists"` is shown inline with a Replace button that submits again with `overwrite`.
     */
    onsubmit: (name: string, overwrite: boolean) => Promise<string | null>;
    oncancel: () => void;
  }
  let { title, initial, onsubmit, oncancel }: Props = $props();

  // The dialog is remounted for each request, so the initial name is read once.
  let name = $state(untrack(() => initial));
  let error = $state<string | null>(null);
  let busy = $state(false);
  let input: HTMLInputElement;

  onMount(() => {
    input.focus();
    input.select();
  });

  async function submit(overwrite: boolean) {
    if (busy) return;
    if (name.trim() === "") {
      error = "empty_name";
      return;
    }
    busy = true;
    try {
      error = await onsubmit(name.trim(), overwrite);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onkeydown={(e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      oncancel();
    }
  }}
>
  <div class="panel" role="dialog" aria-modal="true" aria-labelledby="name-title">
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void submit(false);
      }}
    >
      <div class="head" id="name-title">{title}</div>
      <label for="name-input">{t("queries.name")}</label>
      <input
        id="name-input"
        bind:this={input}
        bind:value={name}
        autocomplete="off"
        spellcheck="false"
        oninput={() => (error = null)}
      />
      {#if error}<p class="error" role="alert">{queryError(error)}</p>{/if}
      <div class="foot">
        <button type="button" class="ghost" onclick={oncancel}>{t("queries.cancel")}</button>
        {#if error === "exists"}
          <button type="button" class="run" disabled={busy} onclick={() => void submit(true)}>{t("queries.replace")}</button>
        {:else}
          <button type="submit" class="run" disabled={busy}>{t("queries.save")}</button>
        {/if}
      </div>
    </form>
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
    width: 360px;
    max-width: calc(100vw - 32px);
    padding: 16px 18px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .head {
    font-weight: 600;
    margin-bottom: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  label {
    color: var(--muted);
    font-size: 12px;
  }
  input {
    height: 32px;
    padding: 0 12px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
  }
  input:focus-visible,
  button:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .error {
    margin: 0;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }
  .ghost,
  .run {
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
  .run {
    background: var(--accent);
    color: var(--text);
    border: 1px solid var(--accent);
    font-weight: 500;
  }
  .run:disabled {
    opacity: 0.6;
    cursor: default;
  }
</style>
