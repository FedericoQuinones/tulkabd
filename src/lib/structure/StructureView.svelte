<script lang="ts">
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { TableStructure } from "../ipc";
  import { isNotConnected } from "../tabs/tabs.svelte";

  interface Props {
    connectionId: string;
    database: string;
    schema: string;
    name: string;
  }
  let { connectionId, database, schema, name }: Props = $props();

  let data = $state<TableStructure | null>(null);
  let notice = $state<string | null>(null);
  let loading = $state(true);
  let seq = 0;

  async function load() {
    const id = ++seq;
    loading = true;
    notice = null;
    try {
      const result = await ipc.tableStructure(connectionId, database, schema, name);
      if (id === seq) data = result;
    } catch (e) {
      if (id === seq) notice = isNotConnected(e) ? t("tabs.notConnected") : t("tabs.loadFailed", { error: String(e) });
    } finally {
      if (id === seq) loading = false;
    }
  }
  $effect(() => {
    void [connectionId, database, schema, name];
    void load();
    return () => seq++;
  });
</script>

<div class="struct">
  {#if notice}
    <div class="notice" role="alert">
      <span>{notice}</span>
      <button type="button" onclick={load}>{t("tabs.retry")}</button>
    </div>
  {:else if loading || !data}
    <p class="hint">{t("structure.loading")}</p>
  {:else}
    <h3>{t("structure.columns")}</h3>
    <table>
      <thead>
        <tr>
          <th>{t("structure.col.name")}</th><th>{t("structure.col.type")}</th><th>{t("structure.col.nullable")}</th>
          <th>{t("structure.col.key")}</th><th>{t("structure.col.default")}</th>
        </tr>
      </thead>
      <tbody>
        {#each data.columns as c (c.name)}
          <tr>
            <td>{c.name}</td>
            <td class="mono">{c.typeName}</td>
            <td>{t(c.nullable ? "structure.nullable.yes" : "structure.nullable.no")}</td>
            <td>{c.primaryKey ? t("structure.pk") : ""}</td>
            <td class="mono">{c.default ?? ""}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if data.foreignKeys.length > 0}
      <h3>{t("structure.foreignKeys")}</h3>
      <table>
        <thead>
          <tr><th>{t("structure.col.name")}</th><th>{t("structure.col.columns")}</th><th>{t("structure.col.references")}</th></tr>
        </thead>
        <tbody>
          {#each data.foreignKeys as fk (fk.name)}
            <tr>
              <td>{fk.name}</td>
              <td class="mono">{fk.columns.join(", ")}</td>
              <td class="mono">{fk.refSchema}.{fk.refTable} ({fk.refColumns.join(", ")})</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
    {#if data.indexes.length > 0}
      <h3>{t("structure.indexes")}</h3>
      <table>
        <thead>
          <tr><th>{t("structure.col.name")}</th><th>{t("structure.col.columns")}</th><th>{t("structure.col.kind")}</th></tr>
        </thead>
        <tbody>
          {#each data.indexes as ix (ix.name)}
            <tr>
              <td>{ix.name}</td>
              <td class="mono">{ix.columns.join(", ")}</td>
              <td>
                {[
                  ix.primaryKey ? t("structure.pk") : null,
                  ix.unique ? t("structure.unique") : null,
                  ix.clustered ? t("structure.clustered") : t("structure.nonclustered"),
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</div>

<style>
  .struct {
    height: 100%;
    overflow: auto;
    padding: 4px 16px 24px;
  }
  h3 {
    margin: 20px 0 6px;
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
  }
  table {
    border-collapse: collapse;
    width: 100%;
  }
  th {
    height: 36px;
    padding: 0 12px;
    text-align: left;
    font-weight: 500;
    font-size: 12px;
    color: var(--muted);
    border-bottom: 1px solid var(--border);
  }
  td {
    height: 34px;
    padding: 0 12px;
    border-bottom: 1px solid var(--rule);
    color: var(--soft);
    white-space: nowrap;
  }
  td:first-child {
    color: var(--text);
  }
  .mono {
    font-family: var(--mono);
    font-size: 12.5px;
  }
  .hint {
    color: var(--muted);
  }
  .notice {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 16px;
    color: var(--warning);
  }
  button {
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: none;
    color: var(--soft);
    font: inherit;
    cursor: pointer;
  }
  button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
