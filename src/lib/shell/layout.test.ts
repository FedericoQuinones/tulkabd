import { afterEach, expect, test, vi } from "vitest";
import { clampWidth, loadLayout, saveLayout, DEFAULT_WIDTH, MAX_WIDTH, MIN_WIDTH } from "./layout";

afterEach(() => vi.unstubAllGlobals());

function fakeStorage(initial: Record<string, string> = {}) {
  const data = { ...initial };
  return {
    data,
    getItem: (k: string) => data[k] ?? null,
    setItem: (k: string, v: string) => void (data[k] = v),
  };
}

test("clampWidth keeps the explorer between 180 and 480 px", () => {
  expect([MIN_WIDTH, MAX_WIDTH, DEFAULT_WIDTH]).toEqual([180, 480, 272]);
  expect(clampWidth(100)).toBe(180);
  expect(clampWidth(900)).toBe(480);
  expect(clampWidth(300.6)).toBe(301);
  expect(clampWidth(Number.NaN)).toBe(272);
});

test("the layout round-trips through localStorage", () => {
  const storage = fakeStorage();
  vi.stubGlobal("localStorage", storage);
  saveLayout({ width: 320, hidden: true });
  expect(loadLayout()).toEqual({ width: 320, hidden: true });
});

test("bad or missing values fall back to the defaults", () => {
  vi.stubGlobal("localStorage", fakeStorage({ "tulkabd.explorer": "{oops" }));
  expect(loadLayout()).toEqual({ width: 272, hidden: false });
  vi.stubGlobal("localStorage", fakeStorage({ "tulkabd.explorer": '{"width":9000}' }));
  expect(loadLayout()).toEqual({ width: 480, hidden: false });
});

test("a throwing storage is ignored", () => {
  const boom = () => {
    throw new Error("denied");
  };
  vi.stubGlobal("localStorage", { getItem: boom, setItem: boom });
  expect(loadLayout()).toEqual({ width: 272, hidden: false });
  expect(() => saveLayout({ width: 200, hidden: false })).not.toThrow();
});
