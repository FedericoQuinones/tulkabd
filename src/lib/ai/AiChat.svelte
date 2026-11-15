<script lang="ts">
  import { onMount, tick } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import { tabs } from "../tabs/tabs.svelte";
  import { aiChat, contextFor, type ChatItem } from "./chat.svelte";
  import { parseMarkdown, type Inline } from "./markdown";
  import { aiStatus } from "./status.svelte";

  type ToolItem = Extract<ChatItem, { kind: "tool" }>;

  let draft = $state("");
  let copied = $state<string | null>(null);
  let logEl = $state<HTMLElement>();
  let stick = true;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  const pane = $derived(tabs.activePane);
  const context = $derived(contextFor(pane?.connectionId ?? null, pane?.database ?? null, aiStatus.visible));
  const notInstalled = $derived(aiStatus.status !== null && !aiStatus.status.installed);
  const lastLen = $derived.by(() => {
    const last = aiChat.items[aiChat.items.length - 1];
    return last && (last.kind === "assistant" || last.kind === "user" || last.kind === "error") ? last.text.length : 0;
  });

  onMount(() => {
    void aiStatus.refresh();
    return () => clearTimeout(copyTimer);
  });

  // A run that just ended may have changed what is shared.
  let wasRunning = false;
  $effect(() => {
    const r = aiChat.running;
    if (wasRunning && !r) void aiStatus.refreshVisible();
    wasRunning = r;
  });

  $effect(() => {
    void aiChat.items.length;
    void lastLen;
    if (!stick) return;
    void tick().then(() => {
      if (logEl) logEl.scrollTop = logEl.scrollHeight;
    });
  });

  function onScroll() {
    if (logEl) stick = logEl.scrollHeight - logEl.scrollTop - logEl.clientHeight < 24;
  }

  function submit() {
    const text = draft;
    if (aiChat.running || !text.trim()) return;
    draft = "";
    stick = true;
    void aiChat.send(text, context);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
      e.preventDefault();
      submit();
    }
  }

  async function copy(id: string, text: string) {
    try {
      await navigator.clipboard.writeText(text);
      copied = id;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = null), 2000);
    } catch {
      /* clipboard unavailable */
    }
  }

  const str = (v: unknown): string => (typeof v === "string" ? v : "");

  function toolLabel(item: ToolItem): string {
    const i = item.input;
    switch (item.tool) {
      case "run_query": {
        const where = [str(i.connection), str(i.database)].filter(Boolean).join("/");
        return where ? `run_query · ${where}` : "run_query";
      }
      case "describe_table":
        return `describe_table ${str(i.table)}`.trim();
      case "search_objects":
        return `search_objects "${str(i.query)}"`;
      case "list_databases":
        return str(i.connection) ? `list_databases · ${str(i.connection)}` : "list_databases";
      default:
        return item.tool;
    }
  }

  const connectionIdOf = (item: ToolItem) => aiStatus.visible.find((v) => v.label === str(item.input.connection))?.connectionId;

  function openInTab(item: ToolItem) {
    const connectionId = connectionIdOf(item);
    if (!connectionId) return;
    const database = str(item.input.database);
    tabs.open({ kind: "query", connectionId, database: database || undefined, sql: str(item.input.sql) });
  }
</script>

<div class="wrap">
  <div class="bar">
    <span class="sect">{t("ai.chat.messages")}</span>
    <button type="button" class="link" onclick={() => void aiChat.reset()}>{t("ai.chat.new")}</button>
  </div>

  <div class="log" role="log" aria-live="polite" aria-label={t("ai.chat.messages")} bind:this={logEl} onscroll={onScroll}>
    {#if aiChat.items.length === 0}
      <p class="empty">{t("ai.chat.example")}</p>
    {/if}
    {#each aiChat.items as item, n (item.kind === "tool" ? `tool:${item.id}` : n)}
      {#if item.kind === "user"}
        <div class="user">{item.text}</div>
      {:else if item.kind === "assistant"}
        <div class="assistant">
          {#each parseMarkdown(item.text) as block, b (b)}
            {#if block.kind === "p"}
              <p>{@render inline(block.inline)}</p>
            {:else if block.kind === "heading"}
              <p class="h h{block.level}">{@render inline(block.inline)}</p>
            {:else if block.kind === "list"}
              <svelte:element this={block.ordered ? "ol" : "ul"}>
                {#each block.items as li, l (l)}<li>{@render inline(li)}</li>{/each}
              </svelte:element>
            {:else}
              <div class="codebox">
                <button type="button" class="link copy" onclick={() => void copy(`${n}:${b}`, block.text)}>
                  {copied === `${n}:${b}` ? t("ai.chat.copied") : t("ai.chat.copy")}
                </button>
                <pre><code>{block.text}</code></pre>
              </div>
            {/if}
          {/each}
        </div>
      {:else if item.kind === "tool"}
        {@const open = aiChat.expanded[item.id] === true}
        <div class="tool">
          <button type="button" class="trow" aria-label={toolLabel(item)} aria-expanded={open} onclick={() => (aiChat.expanded[item.id] = !open)}>
            <span class="icon" aria-hidden="true">{open ? "▾" : "▸"}</span>
            <span class="tlabel">{toolLabel(item)}</span>
            {#if item.ok === false}<span class="bad">{t("ai.chat.failedTool")}</span>{/if}
          </button>
          {#if open}
            <div class="details">
              {#if item.tool === "run_query" && str(item.input.sql)}
                <pre><code>{str(item.input.sql)}</code></pre>
                <button type="button" class="link" disabled={!connectionIdOf(item)} onclick={() => openInTab(item)}>{t("ai.chat.openInTab")}</button>
              {:else}
                <pre><code>{JSON.stringify(item.input, null, 2)}</code></pre>
              {/if}
              {#if item.preview}
                <div class="sect">{t("ai.chat.result")}</div>
                <pre class:bad={item.ok === false}><code>{item.preview}</code></pre>
              {/if}
            </div>
          {/if}
        </div>
      {:else}
        <p class="error" role="alert">{item.text}</p>
      {/if}
    {/each}
  </div>

  {#if notInstalled}
    <div class="hintbox">
      <p>{t("ai.connect.notInstalled")}</p>
      <button type="button" class="link" onclick={() => void ipc.openClaudeInstallDocs().catch(() => {})}>{t("ai.connect.install")}</button>
    </div>
  {:else}
    <div class="composer">
      {#if context}
        <button type="button" class="chip" aria-pressed={aiChat.useContext} title={t("ai.chat.contextHint")} onclick={() => (aiChat.useContext = !aiChat.useContext)}>
          {context.connectionLabel}{context.database ? ` / ${context.database}` : ""}
        </button>
      {/if}
      <div class="row">
        <textarea rows="2" placeholder={t("ai.chat.placeholder")} aria-label={t("ai.chat.placeholder")} bind:value={draft} onkeydown={onKey}></textarea>
        {#if aiChat.running}
          <button type="button" class="act" onclick={() => void aiChat.stop()}>{t("ai.chat.stop")}</button>
        {:else}
          <button type="button" class="act primary" disabled={!draft.trim()} onclick={submit}>{t("ai.chat.send")}</button>
        {/if}
      </div>
    </div>
  {/if}
</div>

{#snippet inline(parts: Inline[])}
  {#each parts as p, i (i)}
    {#if p.kind === "code"}<code>{p.text}</code>{:else if p.kind === "bold"}<strong>{p.text}</strong>{:else if p.kind === "italic"}<em>{p.text}</em>{:else}{p.text}{/if}
  {/each}
{/snippet}

<style>
  .wrap {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: 13px;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 14px;
  }
  .sect {
    color: var(--muted);
    font-size: 12px;
  }
  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .empty {
    margin: 8px 4px;
    color: var(--muted);
  }
  .user {
    align-self: flex-end;
    max-width: 90%;
    padding: 6px 10px;
    background: var(--raised);
    border-radius: var(--radius-card);
    color: var(--text);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .assistant {
    color: var(--soft);
    word-break: break-word;
  }
  .assistant p {
    margin: 0 0 6px;
  }
  .assistant .h {
    color: var(--text);
    font-weight: 600;
  }
  .assistant ul,
  .assistant ol {
    margin: 0 0 6px;
    padding-left: 20px;
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
  pre {
    margin: 0;
    padding: 6px 8px;
    overflow-x: auto;
    background: var(--surface);
    border-radius: var(--radius-card);
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text);
  }
  pre.bad,
  .bad {
    color: var(--negative);
  }
  .codebox {
    position: relative;
    margin: 0 0 6px;
  }
  .copy {
    position: absolute;
    top: 2px;
    right: 6px;
    font-size: 11px;
  }
  .tool {
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  .trow {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    background: none;
    border: 0;
    color: var(--soft);
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .tlabel {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--mono);
    font-size: 12px;
  }
  .details {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    padding: 0 8px 8px;
  }
  .details pre {
    align-self: stretch;
  }
  .error {
    margin: 0;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
  .hintbox {
    flex: none;
    padding: 8px 16px;
    border-top: 1px solid var(--border);
    color: var(--soft);
  }
  .hintbox p {
    margin: 0 0 4px;
  }
  .composer {
    flex: none;
    padding: 8px 12px 10px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .row {
    display: flex;
    gap: 6px;
    align-items: flex-end;
  }
  textarea {
    flex: 1;
    min-width: 0;
    resize: none;
    padding: 6px 8px;
    background: var(--surface);
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    color: var(--text);
    font: inherit;
  }
  textarea:focus-visible {
    outline: none;
    border-color: var(--accent);
  }
  .act,
  .chip {
    height: 28px;
    padding: 0 12px;
    background: none;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .act.primary {
    border-color: var(--accent);
  }
  .act:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .chip {
    align-self: flex-start;
    max-width: 100%;
    height: 22px;
    padding: 0 10px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
    font-size: 12px;
  }
  .chip[aria-pressed="true"] {
    border-color: var(--accent);
    color: var(--text);
  }
  .link {
    background: none;
    border: 0;
    padding: 0 2px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:disabled {
    opacity: 0.6;
    cursor: default;
  }
  button:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }
</style>
