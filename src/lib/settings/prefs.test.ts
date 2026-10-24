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
