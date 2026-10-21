<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import type { ConnectionConfig } from "../ipc";

  interface Props {
    connection: ConnectionConfig;
    /** Called with the typed password; it is used for this connection attempt only. */
    onsubmit: (password: string) => void;
    oncancel: () => void;
  }
  let { connection, onsubmit, oncancel }: Props = $props();

  let password = $state("");
  let input: HTMLInputElement;
  onMount(() => input.focus());
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
  <div class="panel" role="dialog" aria-modal="true" aria-labelledby="pw-title">
  <form
    onsubmit={(e) => {
      e.preventDefault();
      onsubmit(password);
    }}
  >
    <div class="head" id="pw-title">
      <span class="dot" style:background={connection.color}></span>
      {t("password.title", { name: connection.name })}
    </div>
    <label for="pw-input">{t("password.label")}</label>
    <input id="pw-input" bind:this={input} type="password" bind:value={password} autocomplete="off" />
    <div class="foot">
      <button type="button" class="ghost" onclick={oncancel}>{t("connection.cancel")}</button>
      <button type="submit" class="run">{t("password.connect")}</button>
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
    display: flex;
    flex-direction: column;
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    margin-bottom: 4px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
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
    font-family: var(--mono);
  }
  input:focus-visible,
  button:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
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
</style>
