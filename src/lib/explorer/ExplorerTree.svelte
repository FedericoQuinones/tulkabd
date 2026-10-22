<script lang="ts">
  import { tick } from "svelte";
  import { t } from "../i18n/t";
  import type { ConnectionConfig, Folder } from "../ipc";
  import { connections } from "../connections/store.svelte";
  import ContextMenu from "./ContextMenu.svelte";
  import FolderDialog, { type FolderDialogMode } from "./FolderDialog.svelte";
  import TreeNode from "./TreeNode.svelte";
  import { canMoveFolder, navigate, type DragItem, type MenuActions, type MenuItem, type Row } from "./tree";
  import { explorer, menuFor } from "./tree.svelte";

  interface Props {
    onnewconnection: (folderId?: string | null) => void;
    onedit: (conn: ConnectionConfig) => void;
  }
  let { onnewconnection, onedit }: Props = $props();

  let treeEl = $state<HTMLDivElement>();
  let filterEl = $state<HTMLInputElement>();
  let addEl = $state<HTMLButtonElement>();
  let menu = $state<{ items: MenuItem[]; x: number; y: number; label: string; returnTo: HTMLElement | null } | null>(null);
  let dialog = $state<FolderDialogMode | null>(null);
  let renaming = $state<string | null>(null);
  let drag = $state<DragItem | null>(null);
  let dropKey = $state<string | null>(null);

  const rows = $derived(explorer.rows);
  // The roving tabindex: the focused row, or the first one.
  const tabKey = $derived(rows.some((r) => r.key === explorer.focusKey) ? explorer.focusKey : (rows[0]?.key ?? null));
  const colors = $derived(new Map(connections.list.map((c) => [c.id, c.color])));
  const empty = $derived(connections.list.length === 0 && explorer.folders.length === 0);

  function envOf(row: Row): string | null {
    if (row.kind === "connection") return row.conn.color;
    if ("connectionId" in row) return colors.get(row.connectionId) ?? null;
    return null;
  }

  async function focusRow(key: string) {
    explorer.focusKey = key;
    await tick();
    const i = explorer.rows.findIndex((r) => r.key === key);
    treeEl?.querySelectorAll<HTMLElement>('[role="treeitem"]')[i]?.focus();
  }

  const ui: MenuActions = {
    rename: (row) => (renaming = row.key),
    edit: (conn) => onedit(conn),
    moveTo: (row) => {
      if (row.kind === "folder") {
        dialog = { kind: "move", item: { kind: "folder", id: row.folder.id }, name: row.folder.name, current: row.folder.parentId ?? null };
      } else if (row.kind === "connection") {
        dialog = { kind: "move", item: { kind: "connection", id: row.conn.id }, name: row.conn.name, current: row.conn.folderId ?? null };
      }
    },
    remove: (row) => {
      if (row.kind === "connection") {
        const conn = row.conn;
        dialog = {
          kind: "confirm",
          title: t("connection.delete.title"),
          message: t("connection.delete.message", { name: conn.name }),
          confirm: t("explorer.menu.delete"),
          run: () => explorer.deleteConnection(conn.id),
        };
      } else if (row.kind === "folder") {
        const f = row.folder;
        const parent = explorer.folders.find((x) => x.id === f.parentId);
        dialog = {
          kind: "confirm",
          title: t("explorer.folder.delete.title"),
          message: parent
            ? t("explorer.folder.delete.toParent", { name: f.name, parent: parent.name })
            : t("explorer.folder.delete.toRoot", { name: f.name }),
          confirm: t("explorer.menu.delete"),
          run: () => explorer.deleteFolder(f.id),
        };
      }
    },
    newSubfolder: (folder: Folder) => (dialog = { kind: "create", parent: folder }),
    newConnection: (folder: Folder) => onnewconnection(folder.id),
  };

  function rowName(row: Row): string {
    switch (row.kind) {
      case "folder":
        return row.folder.name;
      case "connection":
        return row.conn.name;
      case "database":
        return row.db.name;
      case "object":
        return `${row.object.schema}.${row.object.name}`;
      default:
        return "";
    }
  }

  function openMenu(row: Row, x: number, y: number) {
    explorer.focusKey = row.key;
    const items = menuFor(row, ui);
    if (items.length === 0) return;
    const returnTo = treeEl?.querySelectorAll<HTMLElement>('[role="treeitem"]')[rows.findIndex((r) => r.key === row.key)] ?? null;
    menu = { items, x, y, label: t("explorer.menu.label", { name: rowName(row) }), returnTo };
  }

  function openAddMenu() {
    const r = addEl!.getBoundingClientRect();
    menu = {
      items: [
        { id: "newFolder", label: t("explorer.menu.newFolder"), run: () => (dialog = { kind: "create", parent: null }) },
        { id: "newConnection", label: t("explorer.menu.newConnection"), run: () => onnewconnection(null) },
      ],
      x: r.left,
      y: r.bottom + 4,
      label: t("explorer.add"),
      returnTo: addEl!,
    };
  }

  function closeMenu(refocus: boolean) {
    const back = menu?.returnTo;
    menu = null;
    if (refocus) back?.focus();
  }

  function onrowclick(row: Row) {
    explorer.focusKey = row.key;
    if (row.expandable) explorer.toggle(row.key);
  }

  async function onrename(row: Row, value: string | null) {
    renaming = null;
    const name = value?.trim() ?? "";
    void focusRow(row.key);
    if (name === "") return;
    if (row.kind === "folder" && name !== row.folder.name) await explorer.renameFolder(row.folder.id, name);
    else if (row.kind === "connection" && name !== row.conn.name) await explorer.renameConnection(row.conn, name);
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.target instanceof HTMLInputElement) return;
    const row = rows.find((r) => r.key === tabKey);
    if (row && (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey))) {
      e.preventDefault();
      const el = e.target as HTMLElement;
      const r = el.getBoundingClientRect();
      openMenu(row, r.left + 24, r.bottom);
      return;
    }
    if (row && e.key === "F2" && (row.kind === "folder" || row.kind === "connection")) {
      e.preventDefault();
      renaming = row.key;
      return;
    }
    const nav = navigate(rows, tabKey, e.key);
    if (!nav) return;
    e.preventDefault();
    if ("focus" in nav) void focusRow(nav.focus);
    else if ("expand" in nav) void explorer.expand(nav.expand);
    else if ("collapse" in nav) explorer.collapse(nav.collapse);
    else {
      const target = rows.find((r) => r.key === nav.activate);
      if (target?.kind === "notice") void target.action?.run();
    }
  }

  // ---- drag and drop (connections and folders) ----

  /** The folder a drop on this row lands in; `undefined` when the row takes no drops. */
  function dropFolder(row: Row | null): string | null | undefined {
    if (row === null) return null;
    if (row.kind === "folder") return row.folder.id;
    if (row.kind === "connection") return row.conn.folderId ?? null;
    return undefined;
  }

  function canDrop(target: string | null | undefined): target is string | null {
    if (!drag || target === undefined) return false;
    if (drag.kind === "folder") {
      const f = explorer.folders.find((x) => x.id === drag!.id);
      return !!f && (f.parentId ?? null) !== target && canMoveFolder(explorer.folders, drag.id, target);
    }
    const c = connections.list.find((x) => x.id === drag!.id);
    return !!c && (c.folderId ?? null) !== target;
  }

  function ondragstart(row: Row, e: DragEvent) {
    if (row.kind !== "folder" && row.kind !== "connection") return;
    drag = row.kind === "folder" ? { kind: "folder", id: row.folder.id } : { kind: "connection", id: row.conn.id };
    // WebKit only starts a drag that carries data.
    e.dataTransfer?.setData("text/plain", rowName(row));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
  }

  function ondragover(row: Row | null, e: DragEvent) {
    e.stopPropagation();
    const target = dropFolder(row);
    if (!canDrop(target)) {
      dropKey = null;
      return;
    }
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "move";
    dropKey = row?.kind === "folder" ? row.key : null;
  }

  function ondrop(row: Row | null, e: DragEvent) {
    e.stopPropagation();
    e.preventDefault();
    const target = dropFolder(row);
    const item = drag;
    const ok = canDrop(target);
    ondragend();
    if (item && ok) void explorer.move(item, target as string | null);
  }

  function ondragend() {
    drag = null;
    dropKey = null;
  }
</script>

<div class="explorer-tree">
  <div class="filter">
    <svg class="i" viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
    <input
      bind:this={filterEl}
      type="search"
      value={explorer.filter}
      placeholder={t("explorer.filter.placeholder")}
      aria-label={t("explorer.filter.label")}
      autocomplete="off"
      spellcheck="false"
      oninput={(e) => void explorer.setFilter(e.currentTarget.value)}
      onkeydown={(e) => {
        if (e.key === "Escape" && explorer.filter !== "") {
          e.preventDefault();
          e.stopPropagation();
          void explorer.setFilter("");
        } else if (e.key === "ArrowDown" && rows.length > 0) {
          e.preventDefault();
          void focusRow(tabKey ?? rows[0].key);
        }
      }}
    />
    {#if explorer.filter !== ""}
      <button
        type="button"
        class="clear"
        aria-label={t("explorer.filter.clear")}
        onclick={() => {
          void explorer.setFilter("");
          filterEl?.focus();
        }}
      >
        <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"><path d="M3 3l10 10M13 3L3 13" /></svg>
      </button>
    {/if}
  </div>

  <div class="sect">
    <span>{t("explorer.title")}</span>
    <button type="button" class="add" bind:this={addEl} aria-label={t("explorer.add")} title={t("explorer.add")} aria-haspopup="menu" onclick={openAddMenu}>
      <svg class="i" viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg>
    </button>
  </div>

  {#if explorer.error}
    <p class="error" role="alert">
      <span>{explorer.error}</span>
      <button type="button" class="dismiss" aria-label={t("explorer.dismiss")} onclick={() => (explorer.error = null)}>
        <svg viewBox="0 0 16 16" width="10" height="10" aria-hidden="true"><path d="M3 3l10 10M13 3L3 13" /></svg>
      </button>
    </p>
  {/if}

  {#if empty}
    <div class="empty">
      <p>{t("explorer.empty")}</p>
      <button type="button" class="primary" onclick={() => onnewconnection(null)}>{t("explorer.new")}</button>
    </div>
  {:else}
    <!-- Drops on the empty area move the item to the top level. -->
    <div
      class="tree"
      role="tree"
      aria-label={t("explorer.tree")}
      tabindex="-1"
      bind:this={treeEl}
      {onkeydown}
      ondragover={(e) => ondragover(null, e)}
      ondrop={(e) => ondrop(null, e)}
    >
      {#each rows as row (row.key)}
        <TreeNode
          {row}
          focused={row.key === tabKey}
          env={envOf(row)}
          renaming={renaming === row.key}
          dropTarget={dropKey === row.key}
          onclick={onrowclick}
          onmenu={openMenu}
          {onrename}
          {ondragstart}
          {ondragover}
          {ondrop}
          {ondragend}
        />
      {/each}
      {#if rows.length === 0 && explorer.filter !== ""}
        <p class="none">{t("explorer.filter.none")}</p>
      {/if}
    </div>
  {/if}

  {#if explorer.toast}<div class="toast" role="status">{explorer.toast}</div>{/if}
</div>

{#if menu}
  <ContextMenu items={menu.items} x={menu.x} y={menu.y} label={menu.label} onclose={closeMenu} />
{/if}

{#if dialog}
  <FolderDialog
    mode={dialog}
    onclose={() => {
      dialog = null;
      treeEl?.querySelector<HTMLElement>('[tabindex="0"]')?.focus();
    }}
  />
{/if}

<style>
  .explorer-tree {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  .filter {
    margin: 14px 14px 6px;
    height: 34px;
    flex: none;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 12px;
    color: var(--muted);
  }
  .filter:focus-within {
    border-color: var(--accent);
  }
  .filter input {
    flex: 1;
    min-width: 0;
    height: 100%;
    background: none;
    border: 0;
    outline: none;
    color: var(--text);
    font: inherit;
  }
  .filter input::placeholder {
    color: var(--muted);
  }
  .filter input::-webkit-search-cancel-button {
    display: none;
  }
  .clear,
  .dismiss {
    flex: none;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    padding: 0;
    background: none;
    border: 0;
    color: var(--muted);
    cursor: pointer;
  }
  .clear svg,
  .dismiss svg {
    stroke: currentColor;
    stroke-width: 1.5;
  }
  .clear:hover,
  .dismiss:hover {
    color: var(--text);
  }
  .sect {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 10px 4px 16px;
    color: var(--muted);
    font-size: 12px;
  }
  .add {
    width: 24px;
    height: 24px;
    display: grid;
    place-items: center;
    padding: 0;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--muted);
    cursor: pointer;
  }
  .add:hover {
    color: var(--text);
  }
  .add:focus-visible,
  .clear:focus-visible,
  .dismiss:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .tree {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 6px 10px;
    outline: none;
  }
  .error {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 4px 16px;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .error span {
    flex: 1;
  }
  .none {
    margin: 8px 10px;
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
  .primary:hover,
  .primary:focus-visible {
    outline: none;
    border-color: var(--accent);
  }
  .toast {
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: 12px;
    padding: 8px 12px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
