<script lang="ts">
  import { t } from "../i18n/t";
  import type { RunLogEntry } from "../ipc";
  import { tabs } from "../tabs/tabs.svelte";
  import { runLog } from "./runlog.svelte";

  interface Props {
    /** The pane that shows the table; `showRunLog` finds the table by it. */
    tabId: string;
  }
  let { tabId }: Props = $props();

  const NEAR_END_PX = 80;
  const TITLE_CHARS = 2000;
  const SHOWN_CHARS = 200;

  const sameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();

  function when(atMs: number): string {
    const d = new Date(atMs);
    if (sameDay(d, new Date())) return d.toLocaleTimeString("en-GB");
    const day = `${d.toLocaleDateString("en-US", { month: "short" })} ${d.getDate()}`;
    return `${day} ${d.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" })}`;
  }

  /** One line of the statement, cut short; the whole text is in the row's title. */
  const oneLine = (sql: string) => sql.slice(0, SHOWN_CHARS * 2).replace(/\s+/g, " ").trim().slice(0, SHOWN_CHARS);

  function rows(e: RunLogEntry): { text: string; failed: boolean } {
    if (e.outcome === "ok") return { text: e.rows === null ? "—" : e.rows.toLocaleString("en-US"), failed: false };
    return { text: t(`runlog.outcome.${e.outcome}`), failed: true };
  }

  function time(ms: number | null): string {
    if (ms === null) return "—";
    if (ms < 1000) return t("results.elapsed", { ms: ms.toLocaleString("en-US") });
    return t("runlog.seconds", { s: (ms / 1000).toFixed(2) });
  }

  // The row that takes the Tab key; the arrows move it.
  let focusId = $state<number | null>(null);
  const tabbable = $derived(runLog.entries.some((e) => e.id === focusId) ? focusId : (runLog.entries[0]?.id ?? null));

  function onscroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - NEAR_END_PX) void runLog.loadMore();
  }

  function onkey(e: KeyboardEvent, entry: RunLogEntry) {
    const row = e.currentTarget as HTMLElement;
    if (e.key === "Enter") {
      e.preventDefault();
      tabs.openFromRunLog(entry);
      return;
    }
    const moves: Record<string, () => Element | null> = {
      ArrowDown: () => row.nextElementSibling,
      ArrowUp: () => row.previousElementSibling,
      Home: () => row.parentElement!.firstElementChild,
      End: () => row.parentElement!.lastElementChild,
    };
    const move = moves[e.key];
    if (!move) return;
    e.preventDefault();
    const target = move() as HTMLElement | null;
    target?.focus();
    // On the last row there is nothing below: older runs come in.
    if (!row.nextElementSibling) void runLog.loadMore();
  }
</script>

<div class="runlog" data-runlog={tabId}>
  {#if runLog.entries.length === 0}
    {#if runLog.loaded}<p class="empty">{t("runlog.empty")}</p>{/if}
  {:else}
    <div class="scroller" {onscroll}>
      <table role="grid" aria-label={t("runlog.tab")}>
        <thead>
          <tr>
            <th role="columnheader">{t("runlog.when")}</th>
            <th role="columnheader">{t("runlog.where")}</th>
            <th role="columnheader">{t("runlog.statement")}</th>
            <th role="columnheader" class="r">{t("runlog.rows")}</th>
            <th role="columnheader" class="r">{t("runlog.time")}</th>
          </tr>
        </thead>
        <tbody>
          {#each runLog.entries as e (e.id)}
            {@const out = rows(e)}
            <tr
              data-id={e.id}
              tabindex={e.id === tabbable ? 0 : -1}
              title={e.sql.slice(0, TITLE_CHARS)}
              onclick={() => tabs.openFromRunLog(e)}
              onkeydown={(ev) => onkey(ev, e)}
              onfocus={() => (focusId = e.id)}
            >
              <td role="gridcell" class="when num">{when(e.atMs)}</td>
              <td role="gridcell" class="where">
                <span><span class="dot" style:background={e.color}></span>{e.connectionName} · {e.database}</span>
              </td>
              <td role="gridcell" class="stmt">{oneLine(e.sql)}</td>
              <td role="gridcell" class="out" class:no={out.failed}>{out.text}</td>
              <td role="gridcell" class="ms">{time(e.elapsedMs)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<style>
  .runlog,
  .scroller {
    height: 100%;
  }
  .scroller {
    overflow: auto;
  }
  .empty {
    margin: 0;
    padding: 12px 16px;
    color: var(--muted);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }
  th {
    position: sticky;
    top: 0;
    height: 32px;
    padding: 0 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    color: var(--muted);
    font-weight: 400;
    font-size: 12px;
    text-align: left;
  }
  th:first-child,
  td:first-child {
    padding-left: 16px;
  }
  th.r,
  td.out,
  td.ms {
    text-align: right;
  }
  td {
    height: 44px;
    padding: 0 12px;
    border-bottom: 1px solid var(--rule);
    vertical-align: middle;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  tbody tr {
    cursor: pointer;
  }
  tbody tr:hover {
    background: var(--raised);
  }
  tbody tr:focus-visible {
    outline: 1px solid var(--accent);
    outline-offset: -1px;
  }
  .when {
    width: 96px;
    color: var(--muted);
    font-family: var(--mono);
    font-size: 12px;
  }
  .where span {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }
  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .stmt {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--soft);
  }
  .out,
  .ms {
    width: 96px;
    font-family: var(--mono);
    font-size: 12px;
  }
  .out {
    color: var(--text);
  }
  .out.no {
    color: var(--warning);
    font-family: var(--font);
  }
  .ms {
    color: var(--muted);
  }
</style>
