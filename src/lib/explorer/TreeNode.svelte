<script lang="ts">
  import { tick } from "svelte";
  import { t } from "../i18n/t";
  import type { Row } from "./tree";

  interface Props {
    row: Row;
    /** Holds the roving tabindex. */
    focused: boolean;
    /** Color of the row's connection, for the selection line. */
    env: string | null;
    renaming: boolean;
    dropTarget: boolean;
    onclick: (row: Row) => void;
    onmenu: (row: Row, x: number, y: number) => void;
    onrename: (row: Row, name: string | null) => void;
    /** A press on a folder or connection row that is not being renamed: it may become a drag. */
    ondragpress: (row: Row, e: PointerEvent) => void;
  }
  let { row, focused, env, renaming, dropTarget, onclick, onmenu, onrename, ondragpress }: Props = $props();

  // 16x16 icons drawn with the 1.5px stroke of svg.i (theme.css).
  const icons = {
    folder: '<path d="M2 4.2c0-.7.5-1.2 1.2-1.2h3l1.5 1.7h5.1c.7 0 1.2.5 1.2 1.2v6.4c0 .7-.5 1.2-1.2 1.2H3.2c-.7 0-1.2-.5-1.2-1.2z"/>',
    database: '<ellipse cx="8" cy="3.5" rx="5.5" ry="2"/><path d="M2.5 3.5v9c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2v-9M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>',
    table: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M2 6.5h12M6 6.5V13"/>',
    view: '<rect x="2" y="3" width="12" height="10" rx="1"/><path d="M2 6.5h12" stroke-dasharray="2 1.5"/>',
    procedure: '<path d="M5 3 2 8l3 5M11 3l3 5-3 5"/>',
    saved: '<path d="M8 1.8l1.9 3.9 4.3.6-3.1 3 .7 4.3L8 11.6l-3.8 2 .7-4.3-3.1-3 4.3-.6z"/>',
    savedQuery: '<path d="M4 1.5h5.5L13 5v9.5H4z"/><path d="M9.5 1.5V5H13M6.5 8.5h4M6.5 11h4"/>',
  };

  const name = $derived(
    row.kind === "folder" ? row.folder.name : row.kind === "connection" ? row.conn.name : row.kind === "savedQuery" ? row.name : "",
  );
  const movable = $derived((row.kind === "folder" || row.kind === "connection") && !renaming);
  const dimmed = $derived(
    (row.kind === "connection" && row.status !== "connected") || (row.kind === "database" && row.db.noAccess),
  );

  let input = $state<HTMLInputElement>();
  let value = $state("");
  let done = false;
  $effect(() => {
    if (renaming) {
      value = name;
      done = false;
      void tick().then(() => {
        input?.focus();
        input?.select();
      });
    }
  });

  function finish(save: boolean) {
    if (done) return;
    done = true;
    onrename(row, save ? value : null);
  }
</script>

<!-- Keys are handled once by the tree (roving tabindex), not per row. -->
<!-- svelte-ignore a11y_click_events_have_key_events -->
<div
  role="treeitem"
  aria-level={row.level}
  aria-expanded={row.expandable ? row.expanded : undefined}
  aria-selected={focused}
  tabindex={focused ? 0 : -1}
  class="t {row.kind}"
  class:dimmed
  class:focused
  class:drop={dropTarget}
  style:padding-left="{6 + (row.level - 1) * 14}px"
  style:--env={env ?? "var(--accent)"}
  data-drop="tree:{row.key}"
  onclick={() => onclick(row)}
  oncontextmenu={(e) => {
    e.preventDefault();
    onmenu(row, e.clientX, e.clientY);
  }}
  onpointerdown={(e) => {
    if (movable) ondragpress(row, e);
  }}
>
  <span class="c" aria-hidden="true">{row.expandable ? (row.expanded ? "▾" : "▸") : ""}</span>
  {#if row.kind === "folder"}
    <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true">{@html icons.folder}</svg>
  {:else if row.kind === "connection"}
    <span class="dot" style:background={row.conn.color}></span>
  {:else if row.kind === "database"}
    <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true">{@html icons.database}</svg>
  {:else if row.kind === "group"}
    <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true">{@html icons[row.objectKind]}</svg>
  {:else if row.kind === "saved" || row.kind === "savedQuery"}
    <svg class="i ic" viewBox="0 0 16 16" aria-hidden="true">{@html icons[row.kind]}</svg>
  {/if}

  {#if renaming}
    <input
      bind:this={input}
      bind:value
      aria-label={row.kind === "folder"
        ? t("explorer.renameFolder")
        : row.kind === "savedQuery"
          ? t("queries.rename.label")
          : t("connection.rename.label")}
      onclick={(e) => e.stopPropagation()}
      onblur={() => finish(true)}
      onkeydown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(true);
        else if (e.key === "Escape") finish(false);
      }}
    />
  {:else if row.kind === "folder" || row.kind === "connection"}
    <span class="lab">{name}</span>
    {#if row.kind === "connection" && row.status === "connecting"}<span class="tail">{t("explorer.connecting")}</span>{/if}
  {:else if row.kind === "database"}
    <span class="lab">{row.db.name}</span>
    {#if row.db.noAccess}
      <span class="tail">{t("explorer.noAccess")}</span>
    {:else if row.indexing}
      <span class="tail">{t("explorer.indexing")}</span>
    {/if}
  {:else if row.kind === "group"}
    <span class="lab">{t(`explorer.group.${row.objectKind}`)}</span>
    {#if row.count !== null}<span class="tail num">{row.count}</span>{/if}
  {:else if row.kind === "saved"}
    <span class="lab">{t("explorer.group.saved")}</span>
    <span class="tail num">{row.count}</span>
  {:else if row.kind === "savedQuery"}
    <span class="lab">{row.name}</span>
  {:else if row.kind === "object"}
    <span class="lab">{row.object.schema}.{row.object.name}</span>
  {:else if row.kind === "column"}
    <span class="lab">{row.column.name}</span>
    {#if row.column.primaryKey}<span class="pk">{t("explorer.pk")}</span>{/if}
    <span class="tail mono">{row.column.typeName}</span>
    <span class="nul mono">{row.column.nullable ? t("explorer.null") : t("explorer.notNull")}</span>
  {:else if row.kind === "notice"}
    <span class="lab note" class:err={row.tone === "error"} title={row.text}>{row.text}</span>
    {#if row.action}
      {@const action = row.action}
      <button
        type="button"
        class="act"
        tabindex="-1"
        onclick={(e) => {
          e.stopPropagation();
          void action.run();
        }}>{action.label}</button
      >
    {/if}
  {/if}
</div>

<style>
  .t {
    height: 28px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding-right: 10px;
    white-space: nowrap;
    color: var(--soft);
    border-radius: 4px;
    cursor: default;
    user-select: none;
    outline: none;
  }
  .t:hover,
  .t.drop {
    background: var(--raised);
  }
  .t.drop {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .t:focus-visible {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .connection {
    color: var(--text);
    font-weight: 600;
  }
  .connection.dimmed {
    color: var(--soft);
    font-weight: 500;
  }
  .database.dimmed {
    color: var(--muted);
  }
  .c {
    width: 10px;
    flex: none;
    color: var(--muted);
    font-size: 9px;
    text-align: center;
  }
  .ic {
    color: var(--muted);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .lab {
    position: relative;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  /* The selection is a 1px line in the connection's color (spec §5.1). */
  .focused .lab::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: 1px;
    height: 1px;
    background: var(--env);
  }
  .tail {
    margin-left: auto;
    flex: none;
    color: var(--muted);
    font-size: 12px;
  }
  .num,
  .mono {
    font-family: var(--mono);
  }
  .nul {
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .pk {
    flex: none;
    font-family: var(--mono);
    font-size: 11px;
    color: var(--warning);
  }
  .note {
    color: var(--muted);
    font-size: 12px;
  }
  .note.err {
    color: var(--negative);
  }
  .act {
    flex: none;
    padding: 0;
    background: none;
    border: 0;
    color: var(--accent);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .act:hover {
    text-decoration: underline;
  }
  input {
    flex: 1;
    min-width: 0;
    height: 24px;
    padding: 0 8px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--accent);
    border-radius: var(--radius-card);
    font: inherit;
    font-weight: 400;
    outline: none;
  }
</style>
