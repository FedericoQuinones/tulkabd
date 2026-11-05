import { expect, test } from "vitest";
import { fitWidth, forgetWidths, recallWidths, rememberWidth } from "./columnWidths";

test("widths are recalled by column name, per tab", () => {
  rememberWidth("t1", "b", 300);
  expect(recallWidths("t1", ["a", "b"])[1]).toBe(300);
  expect(recallWidths("t1", ["a", "b"])[0]).toBeUndefined();
  expect(recallWidths("t1", ["b", "a"])[0]).toBe(300);
  expect(recallWidths("t2", ["b"])[0]).toBeUndefined();
  forgetWidths("t1");
  expect(recallWidths("t1", ["b"])[0]).toBeUndefined();
});

test("fit takes the widest of header and cells within the limits", () => {
  expect(fitWidth(100, [80, 150.2])).toBe(151);
  expect(fitWidth(10, [])).toBe(48);
  expect(fitWidth(5000, [10])).toBe(600);
});
