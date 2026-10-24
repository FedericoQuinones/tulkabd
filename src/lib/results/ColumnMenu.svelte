<script lang="ts">
  import { onMount, tick } from "svelte";
  import { t } from "../i18n/t";
  import type { ColumnFilter } from "../ipc";

  interface Props {
    /** Column name, for the menu's label. */
    name: string;
    column: number;
    /** The column's current filter. */
    filter: ColumnFilter | undefined;
    x: number;
    y: number;
    /** The new filter of `column`, or null to clear it. */
    onpick: (column: number, filter: ColumnFilter | null) => void;
    /** `refocus` is true when the menu was dismissed from the keyboard. */
    onclose: (refocus: boolean) => void;
  }
  let { name, column, filter, x, y, onpick, onclose }: Props = $props();

  let el: HTMLDivElement;
  let left = $state(0);
  let top = $state(0);
  let editing = $state(false);
  let text = $state("");

  function place() {
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect();
    left = Math.max(4, Math.min(x, window.innerWidth - r.width - 4));
    top = Math.max(4, Math.min(y, window.innerHeight - r.height - 4));
  }

  onMount(() => {
    place();
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  });

  async function startEquals() {
    text = filter?.kind === "equals" ? filter.text : "";
    editing = true;
    await tick();
    place();
    el.querySelector<HTMLInputElement>("input")?.select();
  }

  function pick(next: ColumnFilter | null) {
    onpick(column, next);
    onclose(true);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" || (e.key === "Tab" && !editing)) {
      e.preventDefault();
      e.stopPropagation();
      onclose(true);
    } else if (!editing && ["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      const list = [...el.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
      const i = list.indexOf(document.activeElement as HTMLButtonElement);
      const next =
        e.key === "Home" ? 0 : e.key === "End" ? list.length - 1 : (i + (e.key === "ArrowDown" ? 1 : -1) + list.length) % list.length;
      list[next]?.focus();
    }
  }
</script>

<svelte:window onpointerdown={(e) => !el.contains(e.target as Node) && onclose(false)} onblur={() => onclose(false)} />

<div
  class="menu"
  role={editing ? "dialog" : "menu"}
  aria-label={t("results.columnMenu", { name })}
  tabindex="-1"
  bind:this={el}
  style:left="{left}px"
  style:top="{top}px"
  {onkeydown}
>
  {#if editing}
    <form
      onsubmit={(e) => {
        e.preventDefault();
        pick({ kind: "equals", column, text });
      }}
    >
      <label for="column-equals">{t("results.filter.equalsLabel")}</label>
      <input id="column-equals" type="text" spellcheck="false" autocomplete="off" bind:value={text} />
      <button type="submit" class="apply">{t("results.filter.apply")}</button>
    </form>
  {:else}
    <button
      type="button"
      role="menuitemradio"
      aria-checked={filter?.kind === "isNull"}
      tabindex="-1"
      onclick={() => pick({ kind: "isNull", column })}>{t("results.filter.isNull")}</button
    >
    <button
      type="button"
      role="menuitemradio"
      aria-checked={filter?.kind === "notNull"}
      tabindex="-1"
      onclick={() => pick({ kind: "notNull", column })}>{t("results.filter.notNull")}</button
    >
    <button type="button" role="menuitemradio" aria-checked={filter?.kind === "equals"} tabindex="-1" onclick={startEquals}>
      <span>{t("results.filter.equals")}</span>
      {#if filter?.kind === "equals"}<small>{filter.text}</small>{/if}
    </button>
    <div class="hr" role="separator"></div>
    <button type="button" role="menuitem" tabindex="-1" disabled={!filter} onclick={() => pick(null)}>{t("results.filter.clear")}</button>
  {/if}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 30;
    min-width: 180px;
    max-width: 360px;
    padding: 5px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    display: flex;
    flex-direction: column;
    outline: none;
  }
  button {
    height: 30px;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 0 10px;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--soft);
    font: inherit;
    text-align: left;
    white-space: nowrap;
    cursor: pointer;
  }
  button:hover:not(:disabled),
  button:focus-visible {
    outline: none;
    color: var(--text);
    border-color: var(--accent);
  }
  button:disabled {
    opacity: 0.4;
    cursor: default;
  }
  [aria-checked="true"] {
    color: var(--text);
  }
  [aria-checked="true"]::before {
    content: "•";
    margin-left: -4px;
  }
  small {
    margin-left: auto;
    padding-left: 16px;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .hr {
    height: 1px;
    margin: 4px 2px;
    background: var(--control);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 4px;
  }
  label {
    color: var(--muted);
    font-size: 12px;
  }
  input {
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: var(--surface);
    color: var(--text);
    font-family: var(--mono);
    font-size: 12.5px;
    outline: none;
  }
  input:focus {
    border-color: var(--accent);
  }
  .apply {
    align-self: flex-end;
    height: 28px;
    border-color: var(--control);
  }
</style>
