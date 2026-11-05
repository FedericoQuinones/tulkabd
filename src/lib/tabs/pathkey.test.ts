import { afterEach, expect, test, vi } from "vitest";
import { pathKey } from "./tabs.svelte";

afterEach(() => vi.unstubAllGlobals());

test("separators, doubled and trailing slashes do not tell two paths apart", () => {
  vi.stubGlobal("navigator", { platform: "Linux x86_64" });
  expect(pathKey("C:\\sql\\a.sql")).toBe(pathKey("C:/sql//a.sql"));
  expect(pathKey("/home/u/a.sql")).not.toBe(pathKey("/home/u/A.sql"));
});

test("case does not tell two paths apart on Windows and macOS", () => {
  for (const platform of ["Win32", "MacIntel"]) {
    vi.stubGlobal("navigator", { platform });
    expect(pathKey("C:\\SQL\\A.sql")).toBe(pathKey("c:/sql/a.sql"));
  }
});
