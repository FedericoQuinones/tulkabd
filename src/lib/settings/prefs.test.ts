import { beforeEach, expect, it, vi } from "vitest";

vi.mock("../ipc", () => ({ getPreferences: vi.fn(), setPreferences: vi.fn() }));

import * as ipc from "../ipc";
import { DEFAULT_PREFERENCES, prefs } from "./prefs.svelte";

beforeEach(() => {
  vi.mocked(ipc.getPreferences).mockReset();
  vi.mocked(ipc.setPreferences).mockReset();
});

it("update waits for the load in flight and saves the whole object", async () => {
  let finish!: (p: typeof DEFAULT_PREFERENCES) => void;
  vi.mocked(ipc.getPreferences).mockReturnValue(new Promise((r) => (finish = r)));
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => ({ ...p, maxRows: 2_000 }));
  const loading = prefs.load();
  const updating = prefs.update({ zoom: 150 });
  expect(ipc.setPreferences).not.toHaveBeenCalled();
  finish({ ...DEFAULT_PREFERENCES, fontSize: 18 });
  await loading;
  const kept = await updating;
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0]).toMatchObject({ zoom: 150, fontSize: 18 });
  // The backend's answer replaces the local value.
  expect(kept.maxRows).toBe(2_000);
  expect(prefs.maxRows).toBe(2_000);
});

it("a failed load keeps the defaults", async () => {
  vi.mocked(ipc.getPreferences).mockRejectedValue("boom");
  const loaded = await prefs.load();
  expect(loaded.restoreSession).toBe(true);
});

it("a failed load is retried before an update, and a second failure writes nothing", async () => {
  vi.mocked(ipc.getPreferences).mockRejectedValue("boom");
  await prefs.load();
  vi.mocked(ipc.getPreferences).mockRejectedValue("still down");
  await expect(prefs.update({ zoom: 120 })).rejects.toBeDefined();
  expect(ipc.setPreferences).not.toHaveBeenCalled();

  vi.mocked(ipc.getPreferences).mockResolvedValue({ ...DEFAULT_PREFERENCES, fontSize: 20 });
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => p);
  await prefs.update({ zoom: 120 });
  expect(vi.mocked(ipc.setPreferences).mock.calls[0][0]).toMatchObject({ zoom: 120, fontSize: 20 });
});

it("updates reach the backend in call order and the last one stays", async () => {
  vi.mocked(ipc.getPreferences).mockResolvedValue({ ...DEFAULT_PREFERENCES });
  await prefs.load();
  const sent: number[] = [];
  vi.mocked(ipc.setPreferences).mockImplementation(async (p) => {
    // The first call is slower; without a queue the second would overtake it.
    await new Promise((r) => setTimeout(r, p.zoom === 110 ? 30 : 0));
    sent.push(p.zoom);
    return p;
  });
  await Promise.all([prefs.update({ zoom: 110 }), prefs.update({ zoom: 120 })]);
  expect(sent).toEqual([110, 120]);
  expect(prefs.zoom).toBe(120);
});

it("a rejected save puts back the last value the backend confirmed", async () => {
  vi.mocked(ipc.getPreferences).mockResolvedValue({ ...DEFAULT_PREFERENCES, maxRows: 5_000 });
  await prefs.load();
  vi.mocked(ipc.setPreferences).mockRejectedValue("read-only");
  const saving = prefs.update({ maxRows: 9_000 });
  await expect(saving).rejects.toBeDefined();
  expect(prefs.maxRows).toBe(5_000);
  expect(prefs.confirmed.maxRows).toBe(5_000);
});

it("confirmed lags behind a write in flight", async () => {
  vi.mocked(ipc.getPreferences).mockResolvedValue({ ...DEFAULT_PREFERENCES, maxRows: 5_000 });
  await prefs.load();
  let answer!: (p: typeof DEFAULT_PREFERENCES) => void;
  vi.mocked(ipc.setPreferences).mockReturnValue(new Promise((r) => (answer = r)));
  const saving = prefs.update({ maxRows: 9_000 });
  await new Promise((r) => setTimeout(r, 0));
  expect(prefs.maxRows).toBe(9_000);
  expect(prefs.confirmed.maxRows).toBe(5_000);
  answer({ ...DEFAULT_PREFERENCES, maxRows: 9_000 });
  await saving;
  expect(prefs.confirmed.maxRows).toBe(9_000);
});
