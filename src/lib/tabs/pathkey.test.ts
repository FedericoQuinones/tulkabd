import { afterEach, expect, test, vi } from "vitest";
import { pathKey } from "./tabs.svelte";

afterEach(() => vi.unstubAllGlobals());

test("doubled and trailing slashes do not tell two paths apart", () => {
  vi.stubGlobal("navigator", { platform: "Linux x86_64" });
  expect(pathKey("/sql//a.sql/")).toBe(pathKey("/sql/a.sql"));
  // A backslash is part of a name outside Windows.
  expect(pathKey("/sql\\a.sql")).not.toBe(pathKey("/sql/a.sql"));
  expect(pathKey("/home/u/a.sql")).not.toBe(pathKey("/home/u/A.sql"));
});

test("case does not tell two paths apart on Windows and macOS", () => {
  for (const platform of ["Win32", "MacIntel"]) {
    vi.stubGlobal("navigator", { platform });
    expect(pathKey("/SQL/A.sql")).toBe(pathKey("/sql/a.sql"));
  }
});

test("on Windows backslashes separate folders and the extended prefix is ignored", () => {
  vi.stubGlobal("navigator", { platform: "Win32" });
  expect(pathKey("C:\\sql\\a.sql")).toBe(pathKey("C:/sql//a.sql"));
  expect(pathKey("\\\\?\\C:\\sql\\a.sql")).toBe(pathKey("C:\\sql\\a.sql"));
});
