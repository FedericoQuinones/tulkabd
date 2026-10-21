import { expect, test } from "vitest";
import { cellText, groupDigits } from "./cellText";

test("null cell", () => expect(cellText({ kind: "null" })).toEqual({ text: "NULL", kind: "null" }));
test("decimal shows grouping", () => expect(cellText({ kind: "decimal", value: "8912450.50" }).text).toBe("8,912,450.50"));
test("negative decimal", () => expect(cellText({ kind: "decimal", value: "-1234.5" }).text).toBe("-1,234.5"));
test("small numbers are not grouped", () => expect(cellText({ kind: "decimal", value: "-123.456" }).text).toBe("-123.456"));
test("int keeps 64-bit precision", () =>
  expect(cellText({ kind: "int", value: "9007199254740993" })).toEqual({
    text: "9,007,199,254,740,993",
    kind: "number",
  }));
test("int min", () => expect(cellText({ kind: "int", value: "-9223372036854775808" }).text).toBe("-9,223,372,036,854,775,808"));
test("float", () => expect(cellText({ kind: "float", value: 1234567.25 })).toEqual({ text: "1,234,567.25", kind: "number" }));
test("float exponent is left as is", () => expect(cellText({ kind: "float", value: 1e21 }).text).toBe("1e+21"));
test("grouping ignores non numeric text", () => expect(groupDigits("abc1234")).toBe("abc1234"));
test("bytes as upper-case hex", () =>
  expect(cellText({ kind: "bytes", value: [0, 171, 255] })).toEqual({ text: "0x00ABFF", kind: "binary" }));
test("bytes keep 32 then ellipsis", () => {
  const exact = cellText({ kind: "bytes", value: new Array(32).fill(255) }).text;
  expect(exact).toBe("0x" + "FF".repeat(32));
  const long = cellText({ kind: "bytes", value: new Array(33).fill(255) }).text;
  expect(long).toBe("0x" + "FF".repeat(32) + "…");
});
test("bool shows 1/0", () => {
  expect(cellText({ kind: "bool", value: true }).text).toBe("1");
  expect(cellText({ kind: "bool", value: false })).toEqual({ text: "0", kind: "text" });
});
test("other kinds are text", () => {
  expect(cellText({ kind: "dateTime", value: "2026-10-20 14:03:00.123" })).toEqual({
    text: "2026-10-20 14:03:00.123",
    kind: "text",
  });
  expect(cellText({ kind: "uuid", value: "ABC" }).kind).toBe("text");
});
