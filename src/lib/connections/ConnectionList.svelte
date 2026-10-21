<script lang="ts">
  import { tick } from "svelte";
  import { t } from "../i18n/t";
  import type { ConnectionConfig } from "../ipc";
  import { connections } from "./store.svelte";

  interface Props {
    onconnect: (conn: ConnectionConfig) => void;
    onedit: (conn: ConnectionConfig) => void;
    onduplicate: (conn: ConnectionConfig) => void;
  }
  let { onconnect, onedit, onduplicate }: Props = $props();

  let menu = $state<{ conn: ConnectionConfig; x: number; y: number } | null>(null);
  let renaming = $state<string | null>(null);
  let renameValue = $state("");
  let deleting = $state<ConnectionConfig | null>(null);
  let actionError = $state<string | null>(null);
  let menuEl = $state<HTMLElement>();

  async function openMenu(conn: ConnectionConfig, x: number, y: number) {
    menu = { conn, x, y };
    await tick();
    menuEl?.querySelector<HTMLButtonElement>("button")?.focus();
  }

  function rowKeydown(e: KeyboardEvent, conn: ConnectionConfig) {
    if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
      e.preventDefault();
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      void openMenu(conn, r.left + 24, r.bottom);
    } else if (e.key === "Enter") {
      onconnect(conn);
    }
  }

  function closeMenu(refocus = false) {
    const id = menu?.conn.id;
    menu = null;
    if (refocus && id) document.getElementById(`conn-${id}`)?.focus();
  }

  function menuKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeMenu(true);
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...menuEl!.querySelectorAll<HTMLButtonElement>("button")];
      const i = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = e.key === "ArrowDown" ? i + 1 : i - 1;
      items[(next + items.length) % items.length].focus();
    }
  }

  async function startRename(conn: ConnectionConfig) {
    closeMenu();
    renaming = conn.id;
    renameValue = conn.name;
    await tick();
    const el = document.getElementById("conn-rename") as HTMLInputElement | null;
    el?.focus();
    el?.select();
  }

  async function commitRename(conn: ConnectionConfig) {
    const name = renameValue.trim();
    renaming = null;
    if (name === "" || name === conn.name) return;
    try {
      await connections.save({ ...conn, name });
    } catch (e) {
      actionError = String(e);
    }
  }

  async function confirmDelete() {
    if (!deleting) return;
    const id = deleting.id;
    deleting = null;
    try {
      await connections.remove(id);
    } catch (e) {
      actionError = String(e);
    }
  }

  function run(action: (c: ConnectionConfig) => void) {
    const conn = menu!.conn;
    closeMenu();
    action(conn);
  }
</script>

<svelte:window onclick={() => closeMenu()} />

<ul class="list">
  {#each connections.list as conn (conn.id)}
    <li>
      {#if renaming === conn.id}
        <div class="row">
          <span class="dot" style:background={conn.color}></span>
          <input
            id="conn-rename"
            aria-label={t("connection.rename.label")}
            bind:value={renameValue}
            onblur={() => commitRename(conn)}
            onkeydown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              else if (e.key === "Escape") {
                e.stopPropagation();
                renaming = null;
              }
            }}
          />
        </div>
      {:else}
        <button
          id={`conn-${conn.id}`}
          class="row"
          aria-haspopup="menu"
          ondblclick={() => onconnect(conn)}
          oncontextmenu={(e) => {
            e.preventDefault();
            e.stopPropagation();
            void openMenu(conn, e.clientX, e.clientY);
          }}
          onkeydown={(e) => rowKeydown(e, conn)}
        >
          <span class="dot" style:background={conn.color}></span>
          <span class="name">{conn.name}</span>
        </button>
      {/if}
    </li>
  {/each}
</ul>

{#if actionError}<p class="err" role="alert">{actionError}</p>{/if}

{#if menu}
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div
    class="menu"
    role="menu"
    tabindex="-1"
    bind:this={menuEl}
    style:left="{menu.x}px"
    style:top="{menu.y}px"
    onkeydown={menuKeydown}
    onclick={(e) => e.stopPropagation()}
  >
    <button role="menuitem" onclick={() => run(startRename)}>{t("connection.menu.rename")}</button>
    <button role="menuitem" onclick={() => run(onedit)}>{t("connection.menu.edit")}</button>
    <button role="menuitem" onclick={() => run(onduplicate)}>{t("connection.menu.duplicate")}</button>
    <button
      role="menuitem"
      class="danger"
      onclick={() => {
        const c = menu!.conn;
        closeMenu();
        deleting = c;
      }}
    >
      {t("connection.menu.delete")}
    </button>
  </div>
{/if}

{#if deleting}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="backdrop"
    onkeydown={(e) => {
      if (e.key === "Escape") deleting = null;
    }}
  >
    <div class="confirm" role="alertdialog" aria-modal="true" aria-labelledby="del-title" aria-describedby="del-msg">
      <div id="del-title" class="title">{t("connection.delete.title")}</div>
      <p id="del-msg">{t("connection.delete.message", { name: deleting.name })}</p>
      <div class="foot">
        <!-- svelte-ignore a11y_autofocus -->
        <button class="ghost" autofocus onclick={() => (deleting = null)}>{t("connection.cancel")}</button>
        <button class="del" onclick={confirmDelete}>{t("connection.menu.delete")}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .list {
    list-style: none;
    margin: 0;
    padding: 4px 8px;
  }
  .row {
    width: 100%;
    height: 30px;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 0 8px;
    background: none;
    border: 1px solid transparent;
    border-radius: var(--radius-card);
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--raised);
  }
  .row:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  input {
    flex: 1;
    min-width: 0;
    height: 26px;
    padding: 0 8px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
    border-radius: var(--radius-card);
    font: inherit;
    outline: none;
  }
  .err {
    margin: 4px 16px;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .menu {
    position: fixed;
    z-index: 30;
    min-width: 150px;
    padding: 4px;
    background: var(--raised);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    display: flex;
    flex-direction: column;
  }
  .menu button {
    height: 28px;
    padding: 0 10px;
    background: none;
    border: 1px solid transparent;
    border-radius: 4px;
    color: var(--text);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .menu button:hover,
  .menu button:focus-visible {
    outline: none;
    border-color: var(--accent);
  }
  .menu .danger {
    color: var(--negative);
  }
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .confirm {
    width: 360px;
    max-width: calc(100vw - 32px);
    padding: 16px 18px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  .title {
    font-weight: 600;
  }
  .confirm p {
    color: var(--soft);
    margin: 8px 0 16px;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .ghost,
  .del {
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
    background: none;
  }
  .ghost {
    color: var(--soft);
    border: 1px solid var(--control);
  }
  .del {
    color: var(--negative);
    border: 1px solid var(--negative);
  }
  .ghost:focus-visible,
  .del:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
    border-color: var(--accent);
  }
</style>
