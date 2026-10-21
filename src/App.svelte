<script lang="ts">
  import { t } from "./lib/i18n/t";
  import Rail from "./lib/shell/Rail.svelte";
  import StatusBar from "./lib/shell/StatusBar.svelte";
</script>

<div class="win">
  <div class="cols">
    <Rail />
    <aside class="explorer">
      <div class="sect"><span>{t("explorer.title")}</span></div>
      <div class="empty">
        <p>{t("explorer.empty")}</p>
        <button class="primary">{t("explorer.new")}</button>
      </div>
    </aside>
    <main class="workspace"></main>
  </div>
  <StatusBar />
</div>

<style>
  .win {
    height: 100vh;
    background: var(--ground);
    display: grid;
    grid-template-rows: 1fr 26px;
  }
  .cols {
    display: grid;
    grid-template-columns: 54px 272px 1fr;
    min-height: 0;
  }
  .explorer {
    background: var(--surface);
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .sect {
    padding: 12px 16px 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    margin-top: 40px;
    color: var(--muted);
  }
  .empty p {
    margin: 0;
  }
  .primary {
    height: 34px;
    padding: 0 16px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--text);
    font: inherit;
    font-weight: 500;
    cursor: pointer;
  }
  .primary:hover {
    border-color: var(--accent);
  }
  .workspace {
    background: var(--ground);
  }
</style>
