import { tick } from "svelte";
import { tabs } from "../tabs/tabs.svelte";
import { runLog } from "./runlog.svelte";

/**
 * Shows the Run log in the active pane and puts the keyboard on its table. The first time, the
 * table only exists once the log has been read, so the focus waits for that.
 */
export async function showRunLog(): Promise<void> {
  const pane = tabs.activePane;
  if (!pane) return;
  pane.bottomTab = "runlog";
  const id = pane.tabId;
  await runLog.ensureLoaded();
  await tick();
  const root = [...document.querySelectorAll<HTMLElement>("[data-runlog]")].find((el) => el.dataset.runlog === id);
  root?.querySelector<HTMLElement>('[tabindex="0"], button')?.focus();
}
