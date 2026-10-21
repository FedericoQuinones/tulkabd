<script lang="ts">
  import { t } from "../i18n/t";
  import Logo from "./Logo.svelte";

  // Icon paths are 16x16, drawn with a 1.5px stroke (see svg.i in theme.css).
  const top = [
    { key: "rail.explorer", active: true, d: '<ellipse cx="8" cy="3.5" rx="5.5" ry="2"/><path d="M2.5 3.5v9c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2v-9M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>' },
    { key: "rail.search", active: false, d: '<circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5 14 14"/>' },
    { key: "rail.saved", active: false, d: '<path d="M8 1.8l1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z"/>' },
    { key: "rail.log", active: false, d: '<circle cx="8" cy="8" r="6"/><path d="M8 4.5V8l2.5 1.5"/>' },
  ];
  const bottom = [
    { key: "rail.drivers", active: false, d: '<path d="M6 2v3M10 2v3M4 5h8v2.5a4 4 0 0 1-8 0zM8 11.5V14"/>' },
    { key: "rail.settings", active: false, d: '<circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4"/>' },
  ];
</script>

<nav class="rail" aria-label={t("rail.label")}>
  <div class="mark"><Logo size={28} /></div>
  {#each top as item (item.key)}
    <button class="ri" class:on={item.active} title={t(item.key)} aria-label={t(item.key)}>
      <svg class="i" viewBox="0 0 16 16" aria-hidden="true">{@html item.d}</svg>
    </button>
  {/each}
  <div class="gap"></div>
  {#each bottom as item (item.key)}
    <button class="ri" title={t(item.key)} aria-label={t(item.key)}>
      <svg class="i" viewBox="0 0 16 16" aria-hidden="true">{@html item.d}</svg>
    </button>
  {/each}
</nav>

<style>
  .rail {
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 12px 0;
    gap: 6px;
  }
  .mark {
    width: 28px;
    height: 28px;
    margin-bottom: 14px;
  }
  .ri {
    width: 54px;
    height: 40px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--muted);
    background: none;
    border: 0;
    padding: 0;
    cursor: pointer;
    position: relative;
  }
  .ri:hover,
  .ri.on {
    color: var(--text);
  }
  .ri.on::before {
    content: "";
    position: absolute;
    left: 0;
    top: 10px;
    bottom: 10px;
    width: 1px;
    background: var(--accent);
  }
  .ri:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -4px;
  }
  .gap {
    flex: 1;
  }
</style>
