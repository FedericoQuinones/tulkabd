<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { t } from "../i18n/t";
  import * as ipc from "../ipc";
  import type { ConnectionConfig, GuardLevel } from "../ipc";
  import { applyEnvironment, ENVIRONMENTS, type EnvState } from "./env";
  import { connections } from "./store.svelte";
  import { validateConnection } from "./validate";

  interface Props {
    /** Connection to edit, or a prefilled copy (Duplicate) to create. */
    initial?: ConnectionConfig;
    onclose: () => void;
  }
  let { initial: initialProp, onclose }: Props = $props();
  // The dialog is keyed by connection in App, so the initial value is read once.
  const initial = untrack(() => initialProp);

  const isEdit = initial !== undefined && connections.list.some((c) => c.id === initial.id);
  const first = ENVIRONMENTS[3];

  let name = $state(initial?.name ?? "");
  let server = $state(initial?.server ?? "");
  let port = $state(initial?.port ?? 1433);
  let user = $state(initial?.user ?? "");
  let password = $state("");
  let encrypt = $state(initial?.encrypt ?? true);
  let trust = $state(initial?.trustServerCertificate ?? false);
  let timeout = $state(initial?.connectTimeoutS ?? 15);
  // Editing keeps the stored color/guard as-is; a new connection follows the environment.
  let env = $state<EnvState>({
    environment: initial?.environment ?? first.id,
    color: initial?.color ?? first.color,
    guard: initial?.guard ?? first.guard,
    colorTouched: isEdit,
    guardTouched: isEdit,
  });

  let errors = $state<Record<string, string>>({});
  let testResult = $state<{ ok: boolean; text: string } | null>(null);
  let busy = $state<"test" | "save" | null>(null);
  let keyringUnavailable = $state(false);
  let saveError = $state<string | null>(null);
  let nameInput: HTMLInputElement;

  onMount(() => nameInput.focus());

  function build(): ConnectionConfig {
    return {
      id: initial?.id ?? crypto.randomUUID(),
      name: name.trim(),
      driver: initial?.driver ?? "mssql",
      server: server.trim(),
      port,
      user,
      databases: initial?.databases ?? { kind: "all" },
      environment: env.environment,
      color: env.color,
      guard: env.guard,
      encrypt,
      trustServerCertificate: trust,
      connectTimeoutS: timeout,
      passwordSaved: initial?.passwordSaved ?? false,
    };
  }

  function validate(): boolean {
    errors = validateConnection({ name, server, port });
    return Object.keys(errors).length === 0;
  }

  async function test() {
    if (!validate()) return;
    busy = "test";
    testResult = null;
    try {
      const info = await ipc.testConnection(build(), password || undefined);
      testResult = { ok: true, text: t("connection.test.ok", { version: info.version }) };
    } catch (e) {
      testResult = { ok: false, text: String(e) };
    } finally {
      busy = null;
    }
  }

  async function save() {
    if (!validate()) return;
    busy = "save";
    saveError = null;
    try {
      const result = await connections.save(build(), password || undefined);
      if (result.keyringUnavailable) {
        // Keep the dialog open so the warning is seen; the connection itself is saved.
        keyringUnavailable = true;
        password = "";
        return;
      }
      onclose();
    } catch (e) {
      saveError = t("connection.errors.saveFailed", { error: String(e) });
    } finally {
      busy = null;
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      onclose();
    }
  }

  const guards: GuardLevel[] = ["off", "confirm", "block"];
  const authOptions = [
    { key: "connection.auth.windows", disabled: true },
    { key: "connection.auth.ad", disabled: true },
    { key: "connection.auth.entra", disabled: true },
  ];
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="backdrop" {onkeydown}>
  <div class="panel" role="dialog" aria-modal="true" aria-labelledby="conn-title">
    <div class="panel-h" id="conn-title">
      {isEdit ? t("connection.dialog.edit") : t("connection.dialog.new")}
    </div>
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div class="panel-b">
        <div class="fl">
          <label for="c-name">{t("connection.field.name")}</label>
          <input id="c-name" bind:this={nameInput} bind:value={name} aria-invalid={!!errors.name} />
          {#if errors.name}<div class="err">{t(errors.name)}</div>{/if}
        </div>
        <div class="two">
          <div class="fl">
            <label for="c-server">{t("connection.field.server")}</label>
            <input id="c-server" class="m" bind:value={server} aria-invalid={!!errors.server} />
            {#if errors.server}<div class="err">{t(errors.server)}</div>{/if}
          </div>
          <div class="fl">
            <label for="c-port">{t("connection.field.port")}</label>
            <input id="c-port" class="m" type="number" bind:value={port} aria-invalid={!!errors.port} />
            {#if errors.port}<div class="err">{t(errors.port)}</div>{/if}
          </div>
        </div>
        <div class="fl">
          <label for="c-auth">{t("connection.field.auth")}</label>
          <select id="c-auth">
            <option>{t("connection.auth.sql")}</option>
            {#each authOptions as o (o.key)}
              <option disabled>{t(o.key)} ({t("connection.auth.comingLater")})</option>
            {/each}
          </select>
        </div>
        <div class="half">
          <div class="fl">
            <label for="c-user">{t("connection.field.user")}</label>
            <input id="c-user" class="m" bind:value={user} autocomplete="off" />
          </div>
          <div class="fl">
            <label for="c-pass">{t("connection.field.password")}</label>
            <input
              id="c-pass"
              class="m"
              type="password"
              bind:value={password}
              autocomplete="off"
              placeholder={isEdit && initial?.passwordSaved ? t("connection.field.passwordSaved") : ""}
            />
          </div>
        </div>
        <div class="fl">
          <span class="lbl" id="c-env">{t("connection.field.environment")}</span>
          <div class="envs" role="radiogroup" aria-labelledby="c-env">
            {#each ENVIRONMENTS as e (e.id)}
              <button
                type="button"
                role="radio"
                aria-checked={env.environment === e.id}
                class:on={env.environment === e.id}
                onclick={() => (env = applyEnvironment(env, e.id))}
              >
                <span class="dot" style:background={`var(${e.token})`}></span>{t(e.label)}
              </button>
            {/each}
          </div>
        </div>
        <div class="two">
          <div class="fl">
            <label for="c-color">{t("connection.field.color")}</label>
            <div class="swatch">
              <input
                id="c-color"
                type="color"
                value={env.color}
                oninput={(e) => (env = { ...env, color: e.currentTarget.value, colorTouched: true })}
              />
              <span class="mono">{env.color}</span>
            </div>
          </div>
          <div class="fl">
            <span class="lbl" id="c-guard">{t("connection.field.guard")}</span>
            <div class="seg" role="radiogroup" aria-labelledby="c-guard">
              {#each guards as g (g)}
                <button
                  type="button"
                  role="radio"
                  aria-checked={env.guard === g}
                  class:on={env.guard === g}
                  onclick={() => (env = { ...env, guard: g, guardTouched: true })}
                >
                  {t(`guard.${g}`)}
                </button>
              {/each}
            </div>
          </div>
        </div>
        <div class="hint">{t("connection.guard.hint")}</div>
        <details class="adv">
          <summary>{t("connection.advanced")}</summary>
          <label class="chk"><input type="checkbox" bind:checked={encrypt} />{t("connection.field.encrypt")}</label>
          <label class="chk"><input type="checkbox" bind:checked={trust} />{t("connection.field.trust")}</label>
          <div class="fl">
            <label for="c-timeout">{t("connection.field.timeout")}</label>
            <input id="c-timeout" class="m" type="number" min="1" bind:value={timeout} />
          </div>
        </details>
        {#if keyringUnavailable}
          <div class="note warn" role="status">{t("connection.keyringUnavailable")}</div>
        {/if}
        {#if testResult}
          <div class="note" class:ok={testResult.ok} class:bad={!testResult.ok} role="status">{testResult.text}</div>
        {/if}
        {#if saveError}<div class="note bad" role="alert">{saveError}</div>{/if}
      </div>
      <div class="panel-f">
        <button type="button" class="ghost l" disabled={busy !== null} onclick={test}>
          {busy === "test" ? t("connection.testing") : t("connection.test")}
        </button>
        <button type="button" class="ghost" onclick={onclose}>
          {keyringUnavailable ? t("connection.close") : t("connection.cancel")}
        </button>
        <button type="submit" class="run" disabled={busy !== null}>
          {busy === "save" ? t("connection.saving") : t("connection.save")}
        </button>
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
    z-index: 10;
  }
  .panel {
    width: 480px;
    max-width: calc(100vw - 32px);
    max-height: calc(100vh - 32px);
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-card);
  }
  form {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .panel-h {
    padding: 14px 18px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .panel-b {
    padding: 14px 18px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .panel-f {
    display: flex;
    gap: 8px;
    padding: 12px 18px;
    border-top: 1px solid var(--border);
  }
  .two {
    display: grid;
    grid-template-columns: 1fr 110px;
    gap: 12px;
  }
  .half {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }
  .fl label,
  .lbl {
    display: block;
    color: var(--muted);
    font-size: 12px;
    margin-bottom: 6px;
  }
  input:not([type="checkbox"]):not([type="color"]),
  select {
    width: 100%;
    height: 32px;
    padding: 0 12px;
    background: var(--ground);
    color: var(--text);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
  }
  input.m {
    font-family: var(--mono);
  }
  input:focus-visible,
  select:focus-visible,
  button:focus-visible,
  summary:focus-visible {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  input[aria-invalid="true"] {
    border-color: var(--negative);
  }
  .err {
    color: var(--negative);
    font-size: 12px;
    margin-top: 4px;
  }
  .hint {
    color: var(--muted);
    font-size: 12px;
  }
  .envs {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .envs button,
  .seg button {
    height: 32px;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 0 12px;
    background: none;
    color: var(--soft);
    border: 1px solid var(--control);
    border-radius: var(--radius-control);
    font: inherit;
    cursor: pointer;
  }
  .envs button.on,
  .seg button.on {
    color: var(--text);
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .seg {
    display: flex;
    gap: 6px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .swatch {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
  }
  .swatch input[type="color"] {
    width: 28px;
    height: 28px;
    padding: 0;
    border: 1px solid var(--control);
    border-radius: 50%;
    background: none;
    cursor: pointer;
  }
  .adv summary {
    color: var(--muted);
    cursor: pointer;
    margin-bottom: 8px;
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    color: var(--soft);
  }
  .note {
    padding: 8px 12px;
    border: 1px solid var(--control);
    border-radius: var(--radius-card);
    color: var(--soft);
    word-break: break-word;
  }
  .note.ok {
    color: var(--positive);
    border-color: var(--positive);
  }
  .note.bad {
    color: var(--negative);
    border-color: var(--negative);
  }
  .note.warn {
    color: var(--warning);
    border-color: var(--warning);
  }
  .ghost,
  .run {
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
  .ghost.l {
    margin-right: auto;
  }
  .run {
    background: var(--accent);
    color: var(--text);
    border: 1px solid var(--accent);
    font-weight: 500;
  }
  button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
