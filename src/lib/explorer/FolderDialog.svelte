<script lang="ts" module>
  import type { Folder } from "../ipc";
  import type { DragItem } from "./tree";

  export type FolderDialogMode =
    | { kind: "create"; parent: Folder | null }
    | { kind: "move"; item: DragItem; name: string; current: string | null }
    | { kind: "confirm"; title: string; message: string; confirm: string; run: () => Promise<unknown> };
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import { canMoveFolder } from "./tree";
  import { explorer } from "./tree.svelte";

  interface Props {
    mode: FolderDialogMode;
    onclose: () => void;
  }
  let { mode, onclose }: Props = $props();

  let panel: HTMLDivElement;
  let name = $state("");
  let target = $state<string | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  onMount(() => {
    if (mode.kind === "move") target = mode.current;
    const first = panel.querySelector<HTMLElement>("input:not([disabled]), [data-autofocus]");
    first?.focus();
  });

  /** Folders in tree order with their depth; a folder being moved hides itself and its subtree. */
  const choices = $derived.by(() => {
    const out: { folder: Folder; depth: number; allowed: boolean }[] = [];
    const add = (parent: string | null, depth: number) => {
      const kids = explorer.folders
        .filter((f) => (f.parentId ?? null) === parent)
        .sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
      for (const f of kids) {
        const allowed = mode.kind !== "move" || mode.item.kind !== "folder" || canMoveFolder(explorer.folders, mode.item.id, f.id);
        out.push({ folder: f, depth, allowed });
        if (depth < 32) add(f.id, depth + 1);
      }
    };
    add(null, 0);
    return out;
  });

  async function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    busy = true;
    error = null;
    explorer.error = null;
    let ok = true;
    if (mode.kind === "create") {
      if (name.trim() === "") {
        error = t("explorer.errors.emptyName");
        ok = false;
      } else ok = await explorer.createFolder(name, mode.parent?.id ?? null);
    } else if (mode.kind === "move") {
      await explorer.move(mode.item, target);
      ok = explorer.error === null;
    } else {
      await mode.run();
      ok = explorer.error === null;
    }
    busy = false;
    if (ok) onclose();
    else {
      // Shown here only: the tree's inline error behind the dialog stays clear.
      error ??= explorer.error;
      explorer.error = null;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onkeydown={(e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }}
>
  <div
    class="panel"
    bind:this={panel}
    role={mode.kind === "confirm" ? "alertdialog" : "dialog"}
    aria-modal="true"
    aria-labelledby="fd-title"
  >
    <form onsubmit={submit}>
      {#if mode.kind === "create"}
        <div class="title" id="fd-title">
          {mode.parent ? t("explorer.folder.newIn", { name: mode.parent.name }) : t("explorer.folder.new")}
        </div>
        <label for="fd-name">{t("explorer.folder.name")}</label>
        <input id="fd-name" bind:value={name} autocomplete="off" />
      {:else if mode.kind === "move"}
        <div class="title" id="fd-title">{t("explorer.move.title", { name: mode.name })}</div>
        <div class="list" role="radiogroup" aria-labelledby="fd-title">
          <label class="opt">
            <input type="radio" name="fd-target" value={null} bind:group={target} />
            <span>{t("explorer.move.root")}</span>
            {#if mode.current === null}<small>{t("explorer.move.current")}</small>{/if}
          </label>
          {#each choices as c (c.folder.id)}
            <label class="opt" class:off={!c.allowed} style:padding-left="{8 + (c.depth + 1) * 14}px">
              <input type="radio" name="fd-target" value={c.folder.id} bind:group={target} disabled={!c.allowed} />
              <span>{c.folder.name}</span>
              {#if mode.current === c.folder.id}<small>{t("explorer.move.current")}</small>{/if}
            </label>
          {/each}
        </div>
      {:else}
        <div class="title" id="fd-title">{mode.title}</div>
        <p class="msg">{mode.message}</p>
      {/if}

      {#if error}<p class="err" role="alert">{error}</p>{/if}

      <div class="foot">
        <button type="button" class="ghost" data-autofocus={mode.kind === "confirm" ? "" : undefined} onclick={onclose}>
          {t("connection.cancel")}
        </button>
        {#if mode.kind === "create"}
          <button type="submit" class="ok" disabled={busy}>{t("explorer.folder.create")}</button>
        {:else if mode.kind === "move"}
          <button type="submit" class="ok" disabled={busy || target === mode.current}>{t("explorer.move.action")}</button>
        {:else}
          <button type="submit" class="del" disabled={busy}>{mode.confirm}</button>
        {/if}
      </div>
    </form>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.55);
    display: grid;
    place-items: center;
    z-index: 20;
  }
  .panel {
    width: 380px;
    max-width: calc(100vw - 32px);
    padding: 16px 18px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .title {
    font-weight: 600;
    margin-bottom: 4px;
    overflow-wrap: anywhere;
  }
  label {
    color: var(--muted);
    font-size: 12px;
  }
  #fd-name {
    height: 32px;
    padding: 0 12px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
  }
  .list {
    max-height: 280px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
    padding: 4px 0;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
    color: var(--soft);
    font-size: 13px;
    cursor: pointer;
  }
  .opt.off {
    color: var(--muted);
    cursor: default;
  }
  .opt small {
    margin-left: auto;
    color: var(--muted);
    font-size: 11px;
  }
  .msg {
    color: var(--soft);
    margin: 0 0 8px;
    overflow-wrap: anywhere;
  }
  .err {
    margin: 0;
    color: var(--negative);
    font-size: 12px;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }
  .ghost,
  .ok,
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
  .ok {
    background: var(--accent);
    color: var(--text);
    border: 1px solid var(--accent);
    font-weight: 500;
  }
  .del {
    color: var(--negative);
    border: 1px solid var(--negative);
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
  #fd-name:focus-visible,
  button:focus-visible,
  .opt:focus-within {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
