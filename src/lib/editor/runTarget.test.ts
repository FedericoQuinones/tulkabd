import { expect, test } from "vitest";
import { runTarget } from "./runTarget";

test("whole doc without selection", () =>
  expect(runTarget("SELECT 1\nSELECT 2", 3, 3)).toEqual({ sql: "SELECT 1\nSELECT 2", lineOffset: 0 }));

test("selection with line offset", () => {
  const d = "SELECT 1\nSELECT 2\nSELECT 3";
  const from = d.indexOf("SELECT 2");
  expect(runTarget(d, from, d.length)).toEqual({ sql: "SELECT 2\nSELECT 3", lineOffset: 1 });
});

test("empty selection runs whole doc", () => expect(runTarget("SELECT 1", 0, 0).sql).toBe("SELECT 1"));

test("whitespace-only selection runs whole doc", () =>
  expect(runTarget("SELECT 1\n  \nSELECT 2", 8, 12)).toEqual({ sql: "SELECT 1\n  \nSELECT 2", lineOffset: 0 }));

test("reversed selection bounds are normalized", () => {
  const d = "a\nb\nc";
  expect(runTarget(d, d.length, 2)).toEqual({ sql: "b\nc", lineOffset: 1 });
});
