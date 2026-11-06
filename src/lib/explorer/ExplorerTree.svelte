<script lang="ts">
  import { tick, untrack } from "svelte";
  import { t } from "../i18n/t";
  import type { ConnectionConfig, Folder, ParsedConnection } from "../ipc";
  import { parsePasted } from "../connections/connstr";
  import { connections } from "../connections/store.svelte";
  import { startDrag } from "../drag/pointerDrag";
  import ContextMenu from "./ContextMenu.svelte";
  import FolderDialog, { type FolderDialogMode } from "./FolderDialog.svelte";
  import TreeNode from "./TreeNode.svelte";
  import { revealRowTop, spacerHeight, isScaled, maxVirtualTop, toRealTop, toVirtualTop, visibleRange, type ScrollGeometry } from "../results/virtual";
  import { canMoveFolder, keyOf, navigate, siblingPositions, type DragItem, type MenuActions, type MenuItem, type Row } from "./tree";
  import { explorer, menuFor } from "./tree.svelte";
  import { queryError } from "../queries/queries.svelte";
  import { tabs } from "../tabs/tabs.svelte";

  /** Fallback row height in px; the stylesheet's `--row-h` is what counts. */
  const ROW_HEIGHT = 28;
  /** Rows kept rendered above and below the view, so a fast scroll does not show blanks. */
  const OVERSCAN = 8;
  /** Height assumed before the tree is measured, in px. */
  const UNMEASURED_HEIGHT = 600;

  interface Props {
    onnewconnection: (folderId?: string | null) => void;
    onedit: (conn: ConnectionConfig) => void;
    /**
     * New connection from a connection string: `parsed` fills the form (a paste on
     * the tree); `null` opens it with the connection string field focused.
     */
    onnewfromstring?: (folderId: string | null, parsed: ParsedConnection | null) => void;
  }
  let { onnewconnection, onedit, onnewfromstring = () => {} }: Props = $props();

  let treeEl = $state<HTMLDivElement>();
  let filterEl = $state<HTMLInputElement>();
  let addEl = $state<HTMLButtonElement>();
  let menu = $state<{ items: MenuItem[]; x: number; y: number; label: string; returnTo: HTMLElement | null } | null>(null);
  let dialog = $state<FolderDialogMode | null>(null);
  let renaming = $state<string | null>(null);
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

  // ---- virtual scrolling ----
  // Only the rows in view (plus a few) are in the DOM; a spacer gives the scrollbar the full height.
  // `scrollTop` is the position in row space, `realTop` the element's own; they differ only when the
  // rows are taller than the capped spacer (see results/virtual.ts, shared with the result grid).

  let scrollTop = $state(0);
  let realTop = $state(0);
  /** The scrollTop last set from code: the scroll event it causes must not re-derive `scrollTop`. */
  let programmatic: number | null = null;
  let viewportH = $state(0);
  let rowH = $state(ROW_HEIGHT);
  /** The first visible row and how far into it the view starts, to hold still when rows above change. */
  let anchor: { key: string; delta: number } | null = null;

  const geo = $derived<ScrollGeometry>({ total: rows.length, rowH, bodyH: viewportH });
  // An unmeasured tree (not laid out yet) still renders a screenful.
  const range = $derived(visibleRange(scrollTop, viewportH || UNMEASURED_HEIGHT, rowH, rows.length, OVERSCAN));
  const page = $derived(Math.max(1, Math.floor(viewportH / rowH) - 1));
  const positions = $derived(siblingPositions(rows));
  /**
   * Indexes to render, in tree order: the window, plus the focused and the renamed row wherever they
   * are, so the roving tab stop and a rename in progress survive being scrolled out of view.
   */
  const indexes = $derived.by(() => {
    const out: number[] = [];
    const pinned = new Set<number>();
    for (const k of [tabKey, renaming]) {
      const i = k === null ? -1 : rows.findIndex((r) => r.key === k);
      if (i >= 0 && (i < range.start || i >= range.end)) pinned.add(i);
    }
    const before = [...pinned].filter((i) => i < range.start).sort((a, b) => a - b);
    const after = [...pinned].filter((i) => i >= range.end).sort((a, b) => a - b);
    out.push(...before);
    for (let i = range.start; i < range.end; i++) out.push(i);
    out.push(...after);
    return out;
  });

  function setScroll(v: number) {
    if (!treeEl) return;
    v = Math.min(Math.max(v, 0), maxVirtualTop(geo));
    const real = toRealTop(v, geo);
    programmatic = real;
    treeEl.scrollTop = real;
    realTop = treeEl.scrollTop;
    scrollTop = isScaled(geo) ? v : realTop;
  }

  function takeAnchor() {
    const i = Math.floor(scrollTop / rowH);
    anchor = scrollTop > 0 && rows[i] ? { key: rows[i].key, delta: scrollTop - i * rowH } : null;
  }

  function onscroll(e: Event & { currentTarget: HTMLDivElement }) {
    const top = e.currentTarget.scrollTop;
    realTop = top;
    if (programmatic !== null && Math.abs(top - programmatic) <= 1) {
      programmatic = null;
      if (!isScaled(geo)) scrollTop = top;
    } else {
      programmatic = null;
      scrollTop = toVirtualTop(top, geo);
    }
    takeAnchor();
  }

  /** Scrolls the minimum that shows row `i` whole. */
  function reveal(i: number) {
    const next = revealRowTop(i, scrollTop, geo);
    if (next !== scrollTop) setScroll(next);
  }

  // The viewport height and the row height (a CSS variable, so the stylesheet owns it).
  $effect(() => {
    const el = treeEl;
    if (!el) return;
    const measure = () => {
      viewportH = el.clientHeight;
      const h = parseFloat(getComputedStyle(el).getPropertyValue("--row-h"));
      rowH = h > 0 ? h : ROW_HEIGHT;
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  });

  // Rows appearing or going away above the view (a refresh, a load finishing) must not move what
  // the user is looking at: the first visible row stays where it was.
  $effect(() => {
    const list = rows;
    untrack(() => {
      const held = anchor;
      void tick().then(() => {
        if (list !== explorer.rows) return;
        if (held) {
          const i = list.findIndex((r) => r.key === held.key);
          if (i >= 0 && i * rowH + held.delta !== scrollTop) setScroll(i * rowH + held.delta);
        }
        takeAnchor();
      });
    });
  });

  // A row that takes the focus (keyboard, a new connection, a reveal from elsewhere) is scrolled to.
  $effect(() => {
    const key = explorer.focusKey;
    if (key === null) return;
    untrack(() => {
      void tick().then(() => {
        const i = explorer.rows.findIndex((r) => r.key === key);
        if (i >= 0 && key === explorer.focusKey) reveal(i);
      });
    });
  });

  async function focusRow(key: string) {
    explorer.focusKey = key;
    await tick();
    const i = explorer.rows.findIndex((r) => r.key === key);
    if (i < 0) return;
    reveal(i);
    await tick();
    // The scroll is ours: the browser must not also scroll the element into view.
    treeEl?.querySelector<HTMLElement>(`[data-index="${i}"]`)?.focus({ preventScroll: true });
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
      } else if (row.kind === "savedQuery") {
        const { database, name } = row;
        dialog = {
          kind: "confirm",
          title: t("queries.delete.title"),
          message: t("queries.delete.message", { name, database }),
          confirm: t("explorer.menu.delete"),
          run: () => tabs.deleteSavedQuery(database, name).catch((e) => (explorer.error = queryError(e))),
        };
      }
    },
    newSubfolder: (folder: Folder) => (dialog = { kind: "create", parent: folder }),
    newConnection: (folder: Folder) => onnewconnection(folder.id),
    newFromConnStr: (folder: Folder) => onnewfromstring(folder.id, null),
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
      case "savedQuery":
        return row.name;
      default:
        return "";
    }
  }

  function openMenu(row: Row, x: number, y: number) {
    explorer.focusKey = row.key;
    const items = menuFor(row, ui);
    if (items.length === 0) return;
    const returnTo = treeEl?.querySelector<HTMLElement>(`[data-index="${rows.findIndex((r) => r.key === row.key)}"]`) ?? null;
    menu = { items, x, y, label: t("explorer.menu.label", { name: rowName(row) }), returnTo };
  }

  function openAddMenu() {
    const r = addEl!.getBoundingClientRect();
    menu = {
      items: [
        { id: "newFolder", label: t("explorer.menu.newFolder"), run: () => (dialog = { kind: "create", parent: null }) },
        { id: "newConnection", label: t("explorer.menu.newConnection"), run: () => onnewconnection(null) },
        { id: "newFromConnStr", label: t("explorer.menu.newFromConnStr"), run: () => onnewfromstring(null, null) },
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
    else if (row.kind === "savedQuery") void explorer.openSavedQuery(row.connectionId, row.database, row.name);
  }

  async function onrename(row: Row, value: string | null) {
    renaming = null;
    const name = value?.trim() ?? "";
    void focusRow(row.key);
    if (name === "") return;
    if (row.kind === "folder" && name !== row.folder.name) await explorer.renameFolder(row.folder.id, name);
    else if (row.kind === "connection" && name !== row.conn.name) await explorer.renameConnection(row.conn, name);
    else if (row.kind === "savedQuery" && name !== row.name) {
      try {
        await tabs.renameSavedQuery(row.database, row.name, name);
        explorer.focusKey = keyOf.savedQuery(row.connectionId, row.database, name);
      } catch (e) {
        explorer.error = queryError(e);
      }
    }
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
    if (row && e.key === "F2" && (row.kind === "folder" || row.kind === "connection" || row.kind === "savedQuery")) {
      e.preventDefault();
      renaming = row.key;
      return;
    }
    const nav = navigate(rows, tabKey, e.key, page);
    if (!nav) return;
    e.preventDefault();
    if ("focus" in nav) void focusRow(nav.focus);
    else if ("expand" in nav) void explorer.expand(nav.expand);
    else if ("collapse" in nav) explorer.collapse(nav.collapse);
    else {
      const target = rows.find((r) => r.key === nav.activate);
      if (target?.kind === "notice") void target.action?.run();
      else if (target?.kind === "savedQuery") void explorer.openSavedQuery(target.connectionId, target.database, target.name);
    }
  }

  // ---- paste a connection string ----

  /** The folder a new connection pasted on this row goes to: the row's folder, or its connection's. */
  function folderOf(row: Row | undefined): string | null {
    if (!row) return null;
    if (row.kind === "folder") return row.folder.id;
    if (row.kind === "connection") return row.conn.folderId ?? null;
    if ("connectionId" in row) return connections.list.find((c) => c.id === row.connectionId)?.folderId ?? null;
    return null;
  }

  /**
   * Ctrl+V on the tree (the `paste` event, so no clipboard permission is needed):
   * a recognized connection string opens New connection filled in; anything else is ignored.
   */
  const editable = (t: EventTarget | null) =>
    t instanceof HTMLInputElement || t instanceof HTMLTextAreaElement || (t instanceof HTMLElement && t.isContentEditable);

  async function onpaste(e: ClipboardEvent) {
    if (editable(e.target)) return;
    const folder = folderOf(rows.find((r) => r.key === tabKey));
    const parsed = await parsePasted(e.clipboardData?.getData("text/plain"));
    if (parsed) onnewfromstring(folder, parsed);
  }

  // WebKit fires `paste` on a non-editable element only when its `beforepaste`
  // was cancelled. Not a Svelte event attribute, so it is attached here.
  $effect(() => {
    const el = treeEl;
    if (!el) return;
    const onbeforepaste = (e: Event) => {
      if (!editable(e.target)) e.preventDefault();
    };
    el.addEventListener("beforepaste", onbeforepaste);
    return () => el.removeEventListener("beforepaste", onbeforepaste);
  });

  // ---- dragging connections and folders (pointer events) ----

  /**
   * The row a drop on `el` lands on: every row is marked `tree:<key>` and the empty area `tree`
   * (`null`, the top level). `undefined` for anything else, such as a tab strip.
   */
  function dropRow(el: Element | null): Row | null | undefined {
    const v = el?.getAttribute("data-drop");
    if (v === "tree") return null;
    if (!v?.startsWith("tree:")) return undefined;
    return rows.find((r) => r.key === v.slice("tree:".length));
  }

  /** The folder a drop on this row lands in; `undefined` when the row takes no drops. */
  function dropFolder(row: Row | null | undefined): string | null | undefined {
    if (row === null) return null;
    if (row?.kind === "folder") return row.folder.id;
    if (row?.kind === "connection") return row.conn.folderId ?? null;
    return undefined;
  }

  function canDrop(item: DragItem, target: string | null | undefined): target is string | null {
    if (target === undefined) return false;
    if (item.kind === "folder") {
      const f = explorer.folders.find((x) => x.id === item.id);
      return !!f && (f.parentId ?? null) !== target && canMoveFolder(explorer.folders, item.id, target);
    }
    const c = connections.list.find((x) => x.id === item.id);
    return !!c && (c.folderId ?? null) !== target;
  }

  function ondragpress(row: Row, e: PointerEvent) {
    if (row.kind !== "folder" && row.kind !== "connection") return;
    const item: DragItem = row.kind === "folder" ? { kind: "folder", id: row.folder.id } : { kind: "connection", id: row.conn.id };
    startDrag(e, {
      payload: { kind: "tree", key: row.key },
      onover: (el) => {
        const over = dropRow(el);
        dropKey = over?.kind === "folder" && canDrop(item, dropFolder(over)) ? over.key : null;
      },
      ondrop: (el) => {
        dropKey = null;
        const target = dropFolder(dropRow(el));
        if (canDrop(item, target)) void explorer.move(item, target);
      },
      oncancel: () => (dropKey = null),
    });
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
      {onpaste}
      {onscroll}
      data-drop="tree"
    >
      <div class="inner" style:height="{spacerHeight(geo)}px">
        {#each indexes as i (rows[i].key)}
          {@const row = rows[i]}
          <TreeNode
            {row}
            focused={row.key === tabKey}
            env={envOf(row)}
            renaming={renaming === row.key}
            dropTarget={dropKey === row.key}
            index={i}
            top={realTop + (i * rowH - scrollTop)}
            setsize={positions[i].setsize}
            posinset={positions[i].posinset}
            onclick={onrowclick}
            onmenu={openMenu}
            {onrename}
            {ondragpress}
          />
        {/each}
      </div>
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
    --row-h: 28px;
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0 6px 10px;
    outline: none;
  }
  /* The rows are absolutely placed in a box as tall as the whole tree; clipping keeps a pinned row far from the view from stretching it. */
  .inner {
    position: relative;
    overflow: clip;
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
