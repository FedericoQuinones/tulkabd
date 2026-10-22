<script lang="ts">
  import { t } from "../i18n/t";

  interface Props {
    kind: "all" | "selected";
    /** Chosen database names; kept while the kind is All so switching back restores them. */
    names: string[];
    /** Asks the server for its databases using the form's current values. */
    load: () => Promise<string[]>;
    error?: string;
  }
  let { kind = $bindable(), names = $bindable(), load, error }: Props = $props();

  // `null` until "Load databases" succeeds; then the server's list.
  let loaded = $state<string[] | null>(null);
  let loading = $state(false);
  let loadError = $state<string | null>(null);
  let filter = $state("");

  const rows = $derived.by(() => {
    const server = loaded ?? [];
    const known = new Set(server);
    const all = [
      ...server.map((name) => ({ name, missing: false })),
      // Chosen but no longer on the server: kept checked so the user can drop them.
      ...names.filter((n) => loaded !== null && !known.has(n)).map((name) => ({ name, missing: true })),
      // Before the first load only the saved choice is known.
      ...(loaded === null ? names.map((name) => ({ name, missing: false })) : []),
    ];
    const q = filter.trim().toLowerCase();
    return q === "" ? all : all.filter((r) => r.name.toLowerCase().includes(q));
  });

  async function onload() {
    loading = true;
    loadError = null;
    try {
      loaded = await load();
    } catch (e) {
      loadError =
        String(e) === "password_required"
          ? t("connection.databases.passwordNeeded")
          : t("connection.databases.loadFailed", { error: String(e) });
    } finally {
      loading = false;
    }
  }

  function toggle(name: string, on: boolean) {
    names = on ? [...names, name] : names.filter((n) => n !== name);
  }
</script>

<div class="fl">
  <span class="lbl" id="c-dbs">{t("connection.field.databases")}</span>
  <div class="seg" role="radiogroup" aria-labelledby="c-dbs">
    {#each ["all", "selected"] as const as k (k)}
      <button type="button" role="radio" aria-checked={kind === k} class:on={kind === k} onclick={() => (kind = k)}>
        {t(`connection.databases.${k}`)}
      </button>
    {/each}
  </div>
  {#if kind === "selected"}
    <div class="bar">
      <button type="button" class="ghost" disabled={loading} onclick={onload}>
        {loading ? t("connection.databases.loading") : t("connection.databases.load")}
      </button>
      <span class="hint">{t("connection.databases.count", { count: names.length })}</span>
    </div>
    {#if loaded !== null || names.length > 0}
      <input
        class="filter"
        bind:value={filter}
        placeholder={t("connection.databases.filter")}
        aria-label={t("connection.databases.filter")}
        autocomplete="off"
      />
      <div class="list" role="group" aria-labelledby="c-dbs">
        {#each rows as r (r.name)}
          <label class="chk">
            <input type="checkbox" checked={names.includes(r.name)} onchange={(e) => toggle(r.name, e.currentTarget.checked)} />
            <span class="n">{r.name}</span>
            {#if r.missing}<span class="missing">{t("connection.databases.missing")}</span>{/if}
          </label>
        {:else}
          <div class="hint">{t("connection.databases.none")}</div>
        {/each}
      </div>
    {/if}
    {#if loadError}<div class="err" role="alert">{loadError}</div>{/if}
  {/if}
  {#if error}<div class="err">{t(error)}</div>{/if}
</div>

<style>
  .lbl {
    display: block;
    color: var(--muted);
    font-size: 12px;
    margin-bottom: 6px;
  }
  .seg {
    display: flex;
    gap: 6px;
  }
  .seg button,
  .ghost {
    height: 32px;
    padding: 0 12px;
    background: none;
    color: var(--soft);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
  }
  .seg button.on {
    color: var(--text);
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  button:focus-visible,
  .filter:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 8px;
  }
  .filter {
    width: 100%;
    height: 32px;
    margin-top: 8px;
    padding: 0 12px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
  }
  .list {
    margin-top: 8px;
    max-height: 160px;
    overflow: auto;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    padding: 6px 12px;
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 0;
    color: var(--soft);
  }
  .n {
    font-family: var(--mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .missing {
    color: var(--warning);
    font-size: 12px;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
  }
  .err {
    color: var(--negative);
    font-size: 12px;
    margin-top: 4px;
    word-break: break-word;
  }
</style>
