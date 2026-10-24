<script lang="ts">
  import { t } from "../i18n/t";
  import type { ConnectionConfig, GuardLevel } from "../ipc";
  import { connections } from "../connections/store.svelte";

  interface Props {
    connection: ConnectionConfig;
    /** The current state shown on the button (the level, or "Off until restart"). */
    label: string;
  }
  let { connection, label }: Props = $props();

  const levels: GuardLevel[] = ["off", "confirm", "block"];
  let open = $state(false);
  let error = $state<string | null>(null);
  let root: HTMLElement;
  let trigger: HTMLButtonElement;

  function close() {
    open = false;
    trigger.focus();
  }

  async function choose(level: GuardLevel) {
    error = null;
    try {
      await connections.setGuard(connection.id, level);
      close();
    } catch (e) {
      error = t("guard.changeFailed", { error: String(e) });
    }
  }

  async function offUntilClose() {
    error = null;
    try {
      await connections.setGuardOffForSession(connection.id, true);
      close();
    } catch (e) {
      error = t("guard.changeFailed", { error: String(e) });
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  }

  // The menu closes when the pointer goes down anywhere else.
  function outside(e: PointerEvent) {
    if (open && !root.contains(e.target as Node)) open = false;
  }

  const current = $derived(connections.guardOff.includes(connection.id) ? null : connection.guard);
</script>

<svelte:window onpointerdown={outside} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="wrap" bind:this={root} {onkeydown}>
  <button
    type="button"
    class="guard"
    class:confirm={current === "confirm"}
    class:block={current === "block"}
    bind:this={trigger}
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={t("guard.menuLabel")}
    onclick={() => (open = !open)}
  >
    <svg class="i" viewBox="0 0 16 16"><path d="M8 2l5 2v4c0 3-2.2 5-5 6-2.8-1-5-3-5-6V4z" /></svg>
    {label}
  </button>
  {#if open}
    <div class="menu" role="menu" aria-label={t("guard.menuLabel")}>
      {#each levels as level (level)}
        <button type="button" role="menuitemradio" aria-checked={current === level} onclick={() => choose(level)}>
          {t(`guard.${level}`)}
        </button>
      {/each}
      <button type="button" role="menuitem" onclick={offUntilClose}>{t("guard.offUntilClose")}</button>
      {#if error}<div class="err" role="alert">{error}</div>{/if}
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
  }
  .guard {
    height: 32px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 0 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    white-space: nowrap;
    cursor: pointer;
  }
  .guard.confirm {
    color: var(--warning);
    border-color: #4a3d22;
  }
  .guard.block {
    color: var(--negative);
  }
  .menu {
    position: absolute;
    top: 36px;
    left: 0;
    z-index: 10;
    min-width: 220px;
    display: flex;
    flex-direction: column;
    padding: 4px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
  }
  .menu button {
    height: 30px;
    padding: 0 10px;
    text-align: left;
    background: none;
    border: none;
    border-radius: var(--radius-control);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .menu button[aria-checked="true"] {
    color: var(--accent);
    font-weight: 600;
  }
  .menu button:hover {
    background: var(--surface);
  }
  .err {
    padding: 6px 10px;
    color: var(--negative);
    font-size: 12px;
  }
  button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
