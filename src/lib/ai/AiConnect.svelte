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

  function message(e: unknown): string {
    const text = e instanceof Error ? e.message : typeof e === "string" ? e : JSON.stringify(e);
    return text.includes("claude_not_found") ? t("ai.connect.notFound") : t("ai.connect.failed", { error: text });
  }

  async function refresh() {
    error = null;
    note = null;
    try {
      [status, visible] = await Promise.all([ipc.claudeStatus(), ipc.aiVisibleConnections()]);
    } catch (e) {
      error = message(e);
    }
  }
  onMount(() => void refresh());

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
    });

  const collapsed = $derived(!!status?.connected && !expanded);
</script>

<section class="connect">
  <div class="head">
    <span class="sect">{t("ai.panel.label")}</span>
    <button type="button" class="link" onclick={() => void refresh()}>{t("ai.connect.refresh")}</button>
  </div>

  {#if error}<p class="error" role="alert">{error}</p>{/if}

  {#if status && collapsed}
    <div class="summary">
      <span class="ok">✓ {t("ai.connect.connected")}</span>
      <button type="button" class="link" aria-expanded="false" aria-label={t("ai.connect.expand")} onclick={() => (expanded = true)}>▾</button>
    </div>
  {:else if status}
    {#if !status.installed}
      <p class="text">{t("ai.connect.notInstalled")}</p>
      <button type="button" class="link" onclick={() => void run(() => ipc.openClaudeInstallDocs())}>{t("ai.connect.install")}</button>
    {:else}
      {#if status.connected}
        <div class="summary">
          <span class="ok">✓ {t("ai.connect.connected")}</span>
          <button type="button" class="link" aria-expanded="true" aria-label={t("ai.connect.collapse")} onclick={() => (expanded = false)}>▴</button>
        </div>
      {/if}
      {#if status.version}<p class="text muted">{t("ai.connect.version", { version: status.version })}</p>{/if}
      {#if status.connectedElsewhere}<p class="text">{t("ai.connect.elsewhere")}</p>{/if}
      <div class="actions">
        {#if !status.connected}
          <button type="button" class="primary" disabled={busy} onclick={() => void connect()}>
            {status.connectedElsewhere ? t("ai.connect.reconnect") : t("ai.connect.connect")}
          </button>
        {/if}
        <button type="button" disabled={busy} onclick={() => void run(() => ipc.openClaudeTerminal())}>{t("ai.connect.openTerminal")}</button>
      </div>
    {/if}

    <div class="others">
      <button type="button" class="link" aria-expanded={menuOpen} onclick={() => (menuOpen = !menuOpen)}>{t("ai.connect.otherAgents")}</button>
      {#if menuOpen}
        <div class="menu">
          <button type="button" onclick={() => void copy("json")}>{t("ai.connect.copyJson")}</button>
          <button type="button" onclick={() => void copy("vscode")}>{t("ai.connect.copyVscode")}</button>
        </div>
      {/if}
      {#if note}<span class="muted" role="status">{note}</span>{/if}
    </div>

    <div class="sect shared">{t("ai.connect.sharedTitle")}</div>
    {#if visible.length === 0}
      <p class="text muted">{t("ai.connect.noneShared")}</p>
    {:else}
      <ul>
        {#each visible as c (c.connectionId)}
          <li>
            <span class="lab" title={c.label}>{c.label}</span>
            {#if !c.passwordSaved}<span class="hint">{t("ai.connect.passwordNotSaved")}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}
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
