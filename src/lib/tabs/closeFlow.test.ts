import { expect, test } from "vitest";
import { checkFrom, closePlan, type CloseCheck } from "./closeFlow";

const none = (tabId: string): CloseCheck => ({ tabId, state: "none", count: 0 });

test("no open transaction closes without asking", () => {
  expect(closePlan([])).toEqual({ ask: false });
  expect(closePlan([none("a"), none("b")])).toEqual({ ask: false });
});

test("open or unknown asks and lists only those tabs", () => {
  const open: CloseCheck = { tabId: "b", state: "open", count: 2 };
  const unknown: CloseCheck = { tabId: "d", state: "unknown", count: 0 };
  expect(closePlan([none("a"), open, none("c"), unknown])).toEqual({ ask: true, tabs: [open, unknown] });
});

test("disconnected never asks", () => {
  expect(checkFrom("a", { error: "disconnected" })).toEqual(none("a"));
  expect(checkFrom("a", { error: { value: "disconnected" } })).toEqual(none("a"));
  expect(closePlan([checkFrom("a", { error: "disconnected" })])).toEqual({ ask: false });
});

test("a count is open when above zero, and busy or unexpected errors are unknown", () => {
  expect(checkFrom("a", { count: 0 })).toEqual(none("a"));
  expect(checkFrom("a", { count: 3 })).toEqual({ tabId: "a", state: "open", count: 3 });
  expect(checkFrom("a", { error: "busy" })).toEqual({ tabId: "a", state: "unknown", count: 0 });
  // Not knowing is never treated as having nothing to lose.
  expect(checkFrom("a", { error: "something odd" })).toEqual({ tabId: "a", state: "unknown", count: 0 });
});
