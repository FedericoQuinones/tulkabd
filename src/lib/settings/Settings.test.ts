// @vitest-environment jsdom
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

vi.mock("../ipc", () => ({
  getPreferences: vi.fn(),
  setPreferences: vi.fn(),
  clearRunLog: vi.fn(),
  dataLocations: vi.fn(),
  revealDataLocation: vi.fn(),
  clearMetadataCache: vi.fn(),
}));

import * as ipc from "../ipc";
import { DEFAULT_PREFERENCES, prefs } from "./prefs.svelte";
import { runLog } from "../runlog/runlog.svelte";
import Settings from "./Settings.svelte";

let cmp: ReturnType<typeof mount> | undefined;
const onclose = vi.fn();

const locations = {
  configFile: "/cfg/config.json",
  dataDir: "/data",
  queriesDir: "/data/queries",
  cacheFile: "/data/tulkabd.db",
  runLogFile: "/data/runlog.db",
  sessionFile: "/data/session.db",
};

const settle = async () => {
  for (let i = 0; i < 30; i++) await Promise.resolve();
  flushSync();
};

async function open() {
  cmp = mount(Settings, { target: document.body, props: { onclose } });
  await settle();
}

beforeEach(() => {
  vi.resetAllMocks();
  prefs.value = structuredClone(DEFAULT_PREFERENCES);
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => p);
  vi.mocked(ipc.dataLocations).mockResolvedValue(locations);
  vi.mocked(ipc.revealDataLocation).mockResolvedValue();
  vi.mocked(ipc.clearRunLog).mockResolvedValue();
  vi.mocked(ipc.clearMetadataCache).mockResolvedValue();
});
afterEach(() => {
  if (cmp) unmount(cmp);
  cmp = undefined;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

const byLabel = (text: string) => {
  const l = [...document.querySelectorAll("label")].find((x) => x.textContent!.trim().startsWith(text));
  return document.getElementById(l!.htmlFor) as HTMLInputElement | HTMLSelectElement;
};
const type = (el: HTMLInputElement, value: string) => {
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
};
const button = (text: string) => [...document.querySelectorAll("button")].find((b) => b.textContent!.trim() === text)!;
const saved = () => document.querySelector("section[data-section='general'] [aria-live='polite']")!.textContent!.trim();

test("renders the six sections in order", async () => {
  await open();
  const titles = [...document.querySelectorAll("h2")].map((h) => h.textContent!.replace("Saved", "").trim());
  expect(titles).toEqual(["General", "Environments", "Execution", "Copy", "Run log", "Data"]);
  expect(document.querySelector("h1")?.textContent).toBe("Settings");
});

test("every field has a label", async () => {
  await open();
  for (const el of document.querySelectorAll<HTMLElement>("input, select")) {
    const named = el.getAttribute("aria-label") || (el.id && document.querySelector(`label[for="${el.id}"]`));
    expect(named, el.outerHTML).toBeTruthy();
  }
});

test("changing font size saves once after typing stops and shows Saved", async () => {
  vi.useFakeTimers();
  await open();
  const input = byLabel("Font size") as HTMLInputElement;
  type(input, "1");
  type(input, "15");
  type(input, "16");
  vi.advanceTimersByTime(299);
  expect(ipc.setPreferences).not.toHaveBeenCalled();
  vi.advanceTimersByTime(1);
  await settle();
  expect(ipc.setPreferences).toHaveBeenCalledTimes(1);
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0].fontSize).toBe(16);
  expect(saved()).toBe("Saved");
  vi.advanceTimersByTime(1500);
  flushSync();
  expect(saved()).toBe("");
});

test("an out-of-range value shows the clamped value from the backend", async () => {
  vi.useFakeTimers();
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => ({ ...p, fontSize: Math.min(24, p.fontSize) }));
  await open();
  const input = byLabel("Font size") as HTMLInputElement;
  type(input, "99");
  vi.advanceTimersByTime(300);
  await settle();
  expect(input.value).toBe("24");
});

test("a rejected save puts the field back to the confirmed value", async () => {
  vi.useFakeTimers();
  vi.mocked(ipc.setPreferences).mockRejectedValue("read-only");
  await open();
  const input = byLabel("Rows kept in memory") as HTMLInputElement;
  type(input, "5000");
  vi.advanceTimersByTime(300);
  await settle();
  expect(prefs.maxRows).toBe(100_000);
  expect(input.value).toBe("100000");
  expect(document.querySelector('[role="alert"]')!.textContent).toContain("read-only");
});

test("while a field has the focus the backend value waits for the blur", async () => {
  vi.useFakeTimers();
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => ({ ...p, fontSize: Math.max(10, p.fontSize) }));
  await open();
  const input = byLabel("Font size") as HTMLInputElement;
  input.focus();
  type(input, "1");
  vi.advanceTimersByTime(300);
  await settle();
  // "1" was brought up to 10 by the backend, but the user is still typing.
  expect(input.value).toBe("1");
  type(input, "16");
  vi.advanceTimersByTime(300);
  await settle();
  expect(input.value).toBe("16");
  expect(vi.mocked(ipc.setPreferences).mock.calls.map((c) => c[0].fontSize)).toEqual([1, 16]);
  input.blur();
  await settle();
  expect(input.value).toBe("16");
  // Blurring right after a save did not save again.
  expect(ipc.setPreferences).toHaveBeenCalledTimes(2);
});

test("blurring twice during a save saves once", async () => {
  vi.useFakeTimers();
  let answer!: (p: typeof DEFAULT_PREFERENCES) => void;
  vi.mocked(ipc.setPreferences).mockReturnValue(new Promise((r) => (answer = r)));
  await open();
  const input = byLabel("Font size") as HTMLInputElement;
  input.focus();
  type(input, "18");
  input.blur();
  input.dispatchEvent(new Event("blur"));
  await settle();
  expect(ipc.setPreferences).toHaveBeenCalledTimes(1);
  answer({ ...DEFAULT_PREFERENCES, fontSize: 18 });
  await settle();
  expect(input.value).toBe("18");
});

test("the Clear run log dialog starts on Cancel, keeps Tab inside and gives the focus back", async () => {
  await open();
  const clear = button("Clear run log");
  clear.focus();
  clear.click();
  flushSync();
  await settle();
  const dialog = document.querySelector('[role="dialog"]')!;
  const [cancel, confirm] = [...dialog.querySelectorAll("button")];
  expect(document.activeElement).toBe(cancel);
  const key = (shiftKey = false) => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true, cancelable: true }));
  confirm.focus();
  key();
  expect(document.activeElement).toBe(cancel);
  key(true);
  expect(document.activeElement).toBe(confirm);
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await settle();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(document.activeElement).toBe(clear);
});

test("a failed Reveal shows an error", async () => {
  vi.mocked(ipc.revealDataLocation).mockRejectedValue("no file manager");
  await open();
  button("Reveal in file manager").click();
  await settle();
  expect(document.querySelector('[role="alert"]')!.textContent).toContain("no file manager");
});

test("toggling the last session saves at once", async () => {
  await open();
  const box = byLabel("Open with the last session") as HTMLInputElement;
  box.click();
  await settle();
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0].restoreSession).toBe(false);
});

test("environment color change calls setPreferences with only that env changed", async () => {
  await open();
  const color = document.querySelector<HTMLInputElement>('input[type="color"][aria-label="Staging color"]')!;
  color.value = "#112233";
  color.dispatchEvent(new Event("change", { bubbles: true }));
  await settle();
  const sent = vi.mocked(ipc.setPreferences).mock.calls[0][0];
  expect(sent.environments).toEqual({
    ...DEFAULT_PREFERENCES.environments,
    staging: { ...DEFAULT_PREFERENCES.environments.staging, color: "#112233" },
  });
  expect(document.querySelector("section[data-section='environments']")!.textContent).toContain("Changes apply to new connections.");
});

test("an environment guard is chosen from three levels", async () => {
  await open();
  const select = document.querySelector<HTMLSelectElement>('select[aria-label="Local guard"]')!;
  expect([...select.options].map((o) => o.textContent)).toEqual(["Off", "Confirm", "Block"]);
  select.value = "block";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  await settle();
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0].environments.local.guard).toBe("block");
});

test("the number format is a choice of two", async () => {
  await open();
  const select = byLabel("Numbers") as HTMLSelectElement;
  const texts = [...select.options].map((o) => o.textContent);
  expect(texts[0]).toMatch(/^System format \(.\ decimal\)$/);
  expect(texts[1]).toBe("Invariant (. decimal)");
  select.value = "invariant";
  select.dispatchEvent(new Event("change", { bubbles: true }));
  await settle();
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0].copyNumbers).toBe("invariant");
});

test("Clear run log asks first and calls clearRunLog on confirm", async () => {
  await open();
  button("Clear run log").click();
  flushSync();
  const dialog = document.querySelector('[role="dialog"]')!;
  expect(dialog.getAttribute("aria-modal")).toBe("true");
  expect(ipc.clearRunLog).not.toHaveBeenCalled();
  // Cancel keeps it.
  [...dialog.querySelectorAll("button")].find((b) => b.textContent === "Cancel")!.click();
  flushSync();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(ipc.clearRunLog).not.toHaveBeenCalled();

  button("Clear run log").click();
  flushSync();
  [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((b) => b.textContent === "Clear")!.click();
  await settle();
  expect(ipc.clearRunLog).toHaveBeenCalledTimes(1);
  expect(document.querySelector('[role="dialog"]')).toBeNull();
});

test("a cleared run log empties the Run log tab's store", async () => {
  const reset = vi.spyOn(runLog, "reset");
  await open();
  button("Clear run log").click();
  flushSync();
  [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((b) => b.textContent === "Clear")!.click();
  await settle();
  expect(reset).toHaveBeenCalledTimes(1);
});

test("Escape inside the confirmation closes only the confirmation", async () => {
  await open();
  button("Clear run log").click();
  flushSync();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  flushSync();
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(onclose).not.toHaveBeenCalled();
});

test("Reveal calls revealDataLocation with the right key", async () => {
  await open();
  const reveals = [...document.querySelectorAll("button")].filter((b) => b.textContent === "Reveal in file manager");
  expect(reveals).toHaveLength(4);
  for (const b of reveals) b.click();
  await settle();
  expect(vi.mocked(ipc.revealDataLocation).mock.calls.map((c) => c[0])).toEqual(["config", "queries", "data", "data"]);
  expect(document.body.textContent).toContain("/cfg/config.json");
  expect(document.body.textContent).toContain("/data/runlog.db");
});

test("Clear metadata cache calls the backend and says so", async () => {
  await open();
  button("Clear metadata cache").click();
  await settle();
  expect(ipc.clearMetadataCache).toHaveBeenCalledTimes(1);
  expect(document.querySelector("section[data-section='data'] [aria-live='polite']")!.textContent).toContain("Cleared");
});

test("a failed save is reported", async () => {
  vi.mocked(ipc.setPreferences).mockRejectedValue("the connections file is read-only");
  await open();
  (byLabel("Open with the last session") as HTMLInputElement).click();
  await settle();
  expect(document.querySelector('[role="alert"]')!.textContent).toContain("read-only");
});

test("Escape asks to close", async () => {
  await open();
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  expect(onclose).toHaveBeenCalledTimes(1);
});

test("pending typing is saved when the screen closes", async () => {
  vi.useFakeTimers();
  await open();
  type(byLabel("Rows kept in memory") as HTMLInputElement, "5000");
  unmount(cmp!);
  cmp = undefined;
  await settle();
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0].maxRows).toBe(5000);
});
