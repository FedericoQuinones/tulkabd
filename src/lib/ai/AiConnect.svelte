<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { AiVisible, ClaudeStatus, McpSetupKind } from "../ipc";

  let status = $state<ClaudeStatus | null>(null);
  let visible = $state<AiVisible[]>([]);
  let error = $state<string | null>(null);
  let note = $state<string | null>(null);
  let busy = $state(false);
  let expanded = $state(false);
  let menuOpen = $state(false);
  let loaded = $state(false);
  let othersEl = $state<HTMLElement>();
  let noteTimer: ReturnType<typeof setTimeout> | undefined;

  function message(e: unknown): string {
    const text = e instanceof Error ? e.message : typeof e === "string" ? e : JSON.stringify(e);
    return text.includes("claude_not_found") ? t("ai.connect.notFound") : t("ai.connect.failed", { error: text });
  }

  /** Runs one action at a time, so a slow refresh can never overwrite a newer result. */
  async function run(action: () => Promise<unknown>) {
    error = null;
    note = null;
    busy = true;
    try {
      await action();
    } catch (e) {
      error = message(e);
    } finally {
      busy = false;
    }
  }

  const refresh = () =>
    run(async () => {
      [status, visible] = await Promise.all([ipc.claudeStatus(), ipc.aiVisibleConnections()]);
    }).finally(() => (loaded = true));
  onMount(() => {
    void refresh();
    return () => clearTimeout(noteTimer);
  });

  const connect = () =>
    run(async () => {
      status = await ipc.connectClaude();
      expanded = false;
    });
  const copy = (kind: McpSetupKind) =>
    run(async () => {
      menuOpen = false;
      await ipc.copyMcpSetup(kind);
      note = t("ai.connect.copied");
      clearTimeout(noteTimer);
      noteTimer = setTimeout(() => (note = null), 3000);
    });

  function closeMenu(e: Event) {
    if (!menuOpen) return;
    if (e instanceof KeyboardEvent) {
      if (e.key !== "Escape") return;
      e.stopPropagation();
    } else if (othersEl?.contains(e.target as Node)) return;
    menuOpen = false;
  }
</script>

<svelte:window onkeydown={closeMenu} onclick={closeMenu} />

<section class="connect">
  <div class="head">
    <span class="sect">{t("ai.panel.label")}</span>
    <button type="button" class="link" disabled={busy} onclick={() => void refresh()}>{t("ai.connect.refresh")}</button>
  </div>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if !loaded}
    <p class="text muted">{t("ai.connect.loading")}</p>
  {:else if status && !status.installed}
    <p class="text">{t("ai.connect.notInstalled")}</p>
    <button type="button" class="link" disabled={busy} onclick={() => void run(() => ipc.openClaudeInstallDocs())}>{t("ai.connect.install")}</button>
  {:else if status?.error}
    <p class="text">{t("ai.connect.startFailed", { error: status.error })}</p>
  {:else if status}
    {#if status.connected}
      <div class="summary">
        <span class="ok">✓ {t("ai.connect.connected")}</span>
        <span class="right">
          <button type="button" class="link" disabled={busy} onclick={() => void run(() => ipc.openClaudeTerminal())}>{t("ai.connect.openTerminal")}</button>
          <button type="button" class="link" aria-expanded={expanded} aria-label={t("ai.connect.details")} onclick={() => (expanded = !expanded)}>{expanded ? "▴" : "▾"}</button>
        </span>
      </div>
    {/if}
    {#if !status.connected || expanded}
      {#if status.version}<p class="text muted">{t("ai.connect.version", { version: status.version })}</p>{/if}
      {#if status.connectedElsewhere}<p class="text">{t("ai.connect.elsewhere")}</p>{/if}
    {/if}
    {#if !status.connected}
      <div class="actions">
        <button type="button" class="primary" disabled={busy} onclick={() => void connect()}>
          {status.connectedElsewhere ? t("ai.connect.reconnect") : t("ai.connect.connect")}
        </button>
        <button type="button" disabled={busy} onclick={() => void run(() => ipc.openClaudeTerminal())}>{t("ai.connect.openTerminal")}</button>
      </div>
    {/if}
  {/if}

  <div class="others" bind:this={othersEl}>
    <button type="button" class="link" aria-expanded={menuOpen} disabled={busy} onclick={() => (menuOpen = !menuOpen)}>{t("ai.connect.otherAgents")}</button>
    {#if menuOpen}
      <div class="menu">
        <button type="button" disabled={busy} onclick={() => void copy("json")}>{t("ai.connect.copyJson")}</button>
        <button type="button" disabled={busy} onclick={() => void copy("vscode")}>{t("ai.connect.copyVscode")}</button>
      </div>
    {/if}
    <span class="muted" role="status">{note ?? ""}</span>
  </div>

  <div class="sect shared" id="ai-shared-title">{t("ai.connect.sharedTitle")}</div>
  {#if visible.length === 0}
    <p class="text muted">{t("ai.connect.noneShared")}</p>
  {:else}
    <ul aria-labelledby="ai-shared-title">
      {#each visible as c (c.connectionId)}
        <li>
          <span class="lab" title={c.label}>{c.label}</span>
          {#if !c.passwordSaved}<span class="hint">{t("ai.connect.passwordNotSaved")}</span>{/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .connect {
    flex: none;
    padding: 6px 0 10px;
    border-bottom: 1px solid var(--border);
    font-size: 13px;
  }
  .head,
  .summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 14px;
  }
  .sect {
    padding: 8px 0 4px;
    color: var(--muted);
    font-size: 12px;
  }
  .shared {
    padding: 8px 16px 4px;
  }
  .text {
    margin: 4px 16px;
    color: var(--soft);
  }
  .muted {
    color: var(--muted);
    font-size: 12px;
  }
  .ok {
    color: var(--text);
  }
  .right {
    display: flex;
    gap: 8px;
  }
  .link:disabled,
  .menu button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .actions {
    display: flex;
    gap: 8px;
    margin: 8px 16px;
  }
  .actions button {
    height: 28px;
    padding: 0 12px;
    background: none;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .actions .primary {
    border-color: var(--accent);
  }
  .actions button:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .link {
    background: none;
    border: 0;
    padding: 0 2px;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .others {
    margin: 4px 16px;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px 10px;
  }
  .menu {
    flex-basis: 100%;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    background: var(--raised);
  }
  .menu button {
    text-align: left;
    height: 28px;
    padding: 0 10px;
    background: none;
    border: 0;
    color: var(--text);
    font: inherit;
    cursor: pointer;
  }
  .menu button:hover {
    background: var(--control);
  }
  button:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0 6px;
  }
  li {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 26px;
    padding: 0 10px;
    color: var(--soft);
  }
  .lab {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    flex: none;
    color: var(--muted);
    font-size: 11px;
  }
  .error {
    margin: 4px 16px;
    color: var(--negative);
    font-size: 12px;
    word-break: break-word;
  }
</style>
