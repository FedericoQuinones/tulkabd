<script lang="ts">
  import { onMount } from "svelte";
  import { t } from "../i18n/t";
  import type { ConnectionConfig, GuardFinding } from "../ipc";
  import { ENVIRONMENTS } from "../connections/env";
  import { consequence, findingLine } from "./text";

  interface Props {
    connection: ConnectionConfig;
    prompt: { sql: string; lineOffset: number; findings: GuardFinding[] };
    /** Run the held script once; `dontAskAgain` also turns the connection's guard off. */
    onconfirm: (dontAskAgain: boolean) => void;
    oncancel: () => void;
  }
  let { connection, prompt, onconfirm, oncancel }: Props = $props();

  const envColor = $derived(ENVIRONMENTS.find((e) => e.id === connection.environment)?.color ?? "var(--control)");
  let dontAsk = $state(false);
  let panel: HTMLElement;
  let cancelButton: HTMLButtonElement;

  // Focus goes back where it was (the editor, usually) when the dialog closes.
  const before = typeof document !== "undefined" ? (document.activeElement as HTMLElement | null) : null;

  onMount(() => {
    // Cancel gets the focus, never Run anyway: a stray Enter must not run the script.
    cancelButton.focus();
    return () => before?.focus?.();
  });

  /** Keeps Tab and Shift+Tab inside the dialog. */
  function trap(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      oncancel();
      return;
    }
    if (e.key !== "Tab") return;
    const items = [...panel.querySelectorAll<HTMLElement>("button, input")].filter((el) => !(el as HTMLButtonElement).disabled);
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    // Focus on the panel itself or on the page (after a click on plain text) counts as outside.
    const inside = active !== null && items.includes(active);
    if (e.shiftKey && (active === first || !inside)) {
      e.preventDefault();
      (inside ? last : cancelButton).focus();
    } else if (!e.shiftKey && (active === last || !inside)) {
      e.preventDefault();
      (inside ? first : cancelButton).focus();
    }
  }
</script>

<!-- Keys are heard on the window: a click on plain text can leave the focus outside the buttons. -->
<svelte:window onkeydown={trap} />
<div class="backdrop">
  <div
    class="panel"
    role="dialog"
    aria-modal="true"
    aria-labelledby="guard-title"
    tabindex="-1"
    bind:this={panel}
    style:border-color={envColor}
  >
    <div class="head" id="guard-title">{t("guard.confirmTitle", { connection: connection.name })}</div>
    <ul>
      {#each prompt.findings as f, i (i)}
        <li>
          <span class="line">{t("guard.line", { line: findingLine(f, prompt.lineOffset) })}</span>
          <code>{f.statement}</code>
          <span class="why">{consequence(f)}</span>
        </li>
      {/each}
    </ul>
    <label class="ask"><input type="checkbox" bind:checked={dontAsk} />{t("guard.dontAsk")}</label>
    <div class="foot">
      <button type="button" class="ghost" bind:this={cancelButton} onclick={oncancel}>{t("guard.cancel")}</button>
      <button type="button" class="danger" onclick={() => onconfirm(dontAsk)}>{t("guard.runAnyway")}</button>
    </div>
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
    width: 520px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 64px);
    overflow: auto;
    padding: 16px 18px;
    background: var(--surface);
    border: 2px solid var(--border);
    border-radius: var(--radius-card);
  }
  .head {
    font-weight: 600;
    margin-bottom: 10px;
  }
  ul {
    list-style: none;
    margin: 0 0 12px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  li {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .line {
    color: var(--muted);
    font-size: 11.5px;
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
    padding: 6px 8px;
    background: var(--ground);
    border-radius: var(--radius-control);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .why {
    color: var(--warning);
    font-size: 12.5px;
  }
  .ask {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--soft);
    font-size: 12.5px;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
  .ghost,
  .danger {
    height: 32px;
    padding: 0 14px;
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
  }
  .ghost {
    background: none;
    color: var(--soft);
    border: 1px solid var(--control);
  }
  .danger {
    background: none;
    color: var(--negative);
    border: 1px solid var(--negative);
    font-weight: 500;
  }
  button:focus-visible,
  input:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px var(--accent);
  }
</style>
