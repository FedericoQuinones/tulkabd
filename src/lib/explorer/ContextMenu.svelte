<script lang="ts">
  import { onMount } from "svelte";
  import type { MenuItem } from "./tree.svelte";

  interface Props {
    items: MenuItem[];
    x: number;
    y: number;
    label: string;
    /** `refocus` is true when the menu was dismissed from the keyboard. */
    onclose: (refocus: boolean) => void;
  }
  let { items, x, y, label, onclose }: Props = $props();

  let el: HTMLDivElement;
  let left = $state(0);
  let top = $state(0);

  onMount(() => {
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect();
    left = Math.max(4, Math.min(x, window.innerWidth - r.width - 4));
    top = Math.max(4, Math.min(y, window.innerHeight - r.height - 4));
    el.querySelector<HTMLButtonElement>("button")?.focus();
  });

  function buttons() {
    return [...el.querySelectorAll<HTMLButtonElement>("button")];
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape" || e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      onclose(true);
    } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
      e.preventDefault();
      const list = buttons();
      const i = list.indexOf(document.activeElement as HTMLButtonElement);
      const next =
        e.key === "Home" ? 0 : e.key === "End" ? list.length - 1 : (i + (e.key === "ArrowDown" ? 1 : -1) + list.length) % list.length;
      list[next]?.focus();
    }
  }

  function pick(item: MenuItem) {
    onclose(false);
    void item.run();
  }
</script>

<svelte:window onpointerdown={(e) => !el.contains(e.target as Node) && onclose(false)} onblur={() => onclose(false)} />

<div class="menu" role="menu" aria-label={label} tabindex="-1" bind:this={el} style:left="{left}px" style:top="{top}px" {onkeydown}>
  {#each items as item (item.id)}
    {#if item.separator}<div class="hr" role="separator"></div>{/if}
    <button type="button" role="menuitem" tabindex="-1" class:danger={item.danger} onclick={() => pick(item)}>
      <span>{item.label}</span>
      {#if item.hint}<small>{item.hint}</small>{/if}
    </button>
  {/each}
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
  button:hover,
  button:focus-visible {
    outline: none;
    color: var(--text);
    border-color: var(--accent);
  }
  .danger {
    color: var(--negative);
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
</style>
